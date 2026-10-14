//! The piano: plays reference notes to sing against, through PipeWire.
//!
//! Each key press adds a voice with a piano-like tone: a fast attack, then partials that ring
//! and fade, the higher ones sooner. The playback stream starts on the first press and
//! pauses once the piano has been quiet a few seconds, so the output can sleep. With a test
//! signal running, nothing is played: the voices are mixed into the made-up input instead
//! (see `audio::Synth`), as if the microphone heard them.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::pod::Pod;

pub const RATE: u32 = 48_000;
/// The keyboard's range: a baritone's low F to a tenor's high C.
pub const LOW: i32 = 41;
pub const HIGH: i32 = 72;
/// How long a note rings, s, the last `RELEASE` of it fading out.
const LENGTH: f64 = 2.2;
const RELEASE: f64 = 0.3;
/// (amplitude, decay per second) of each partial, fundamental first.
const PARTIALS: [(f64, f64); 7] = [(1.0, 1.3), (0.5, 1.9), (0.3, 2.6), (0.2, 3.4), (0.12, 4.2), (0.08, 5.2), (0.05, 6.2)];
/// Piano strings are slightly stiff, so partial k sits a little above k·f.
const STIFFNESS: f64 = 0.0002;
const GAIN: f64 = 0.22;
const MAX_VOICES: usize = 8;
/// Pause the playback stream after this long without a note.
const IDLE: Duration = Duration::from_secs(3);

pub struct Voice {
    hz: f64,
    n: u64,
}

impl Voice {
    fn sample(&self) -> f64 {
        let t = self.n as f64 / RATE as f64;
        let envelope = (t / 0.004).min(1.0) * ((LENGTH - t) / RELEASE).clamp(0.0, 1.0);
        let tone: f64 = PARTIALS
            .iter()
            .enumerate()
            .map(|(k, &(amp, decay))| {
                let h = (k + 1) as f64;
                let f = self.hz * h * (1.0 + STIFFNESS * h * h).sqrt();
                amp * (-decay * t).exp() * (std::f64::consts::TAU * f * t).sin()
            })
            .sum();
        envelope * tone
    }

    fn done(&self) -> bool {
        self.n as f64 >= LENGTH * RATE as f64
    }
}

pub type Voices = Arc<Mutex<Vec<Voice>>>;

/// Mixes the next `out.len()` samples of every voice into `out`, and drops finished voices.
pub fn render(voices: &mut Vec<Voice>, out: &mut [f32]) {
    for o in out.iter_mut() {
        let mut sum = 0.0;
        for v in voices.iter_mut() {
            sum += v.sample();
            v.n += 1;
        }
        // Soft-clip, so chords stay clean of hard clipping.
        *o += (GAIN * sum).tanh() as f32;
    }
    voices.retain(|v| !v.done());
}

pub struct Piano {
    voices: Voices,
    /// Whether to play through PipeWire (not with a test signal).
    audible: bool,
    output: Option<pw::channel::Sender<()>>,
}

impl Piano {
    pub fn new(voices: Voices, audible: bool) -> Self {
        Self { voices, audible, output: None }
    }

    pub fn play(&mut self, hz: f32) {
        {
            let mut voices = self.voices.lock().unwrap();
            if voices.len() >= MAX_VOICES {
                voices.remove(0);
            }
            voices.push(Voice { hz: hz as f64, n: 0 });
        }
        if !self.audible {
            return;
        }
        match &self.output {
            Some(wake) => {
                let _ = wake.send(());
            }
            None => {
                let (wake, woken) = pw::channel::channel();
                let voices = self.voices.clone();
                thread::Builder::new()
                    .name("piano".into())
                    .spawn(move || {
                        if let Err(e) = run(voices, woken) {
                            eprintln!("guitar: piano output failed: {e}");
                        }
                    })
                    .expect("failed to spawn piano thread");
                self.output = Some(wake);
            }
        }
    }
}

fn run(voices: Voices, woken: pw::channel::Receiver<()>) -> Result<()> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;
    let stream = pw::stream::StreamRc::new(
        core,
        "guitar-piano",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::MEDIA_ROLE => "Music",
            *pw::keys::NODE_NAME => "guitar-piano",
            *pw::keys::APP_NAME => "guitar",
            *pw::keys::AUDIO_CHANNELS => "2",
            *pw::keys::NODE_LATENCY => "256/48000",
        },
    )?;

    let render_voices = voices.clone();
    let mut mono = Vec::new();
    let _listener = stream
        .add_local_listener_with_user_data(())
        .process(move |stream, _| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let requested = buffer.requested() as usize;
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else { return };
            const STRIDE: usize = 2 * 4;
            let frames = match data.data() {
                Some(bytes) => {
                    // Only what the graph asked for: filling the whole buffer adds latency.
                    let room = bytes.len() / STRIDE;
                    let frames = if requested > 0 { requested.min(room) } else { room.min(1024) };
                    mono.clear();
                    mono.resize(frames, 0.0);
                    render(&mut render_voices.lock().unwrap(), &mut mono);
                    for (frame, &x) in bytes.as_chunks_mut::<STRIDE>().0.iter_mut().zip(&mono) {
                        let b = x.to_le_bytes();
                        frame[..4].copy_from_slice(&b);
                        frame[4..].copy_from_slice(&b);
                    }
                    frames
                }
                None => 0,
            };
            let chunk = data.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = STRIDE as _;
            *chunk.size_mut() = (STRIDE * frames) as _;
        })
        .register()?;

    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(RATE);
    info.set_channels(2);
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);
    let values: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties: info.into(),
        }),
    )?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&values).expect("valid format pod")];
    stream.connect(
        spa::utils::Direction::Output,
        None,
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;

    // Pause when quiet for a while; a key press wakes it.
    let active = Rc::new(Cell::new(true));
    let quiet_since = Rc::new(Cell::new(None::<Instant>));
    let (idle_stream, idle_active) = (stream.clone(), active.clone());
    let timer = mainloop.loop_().add_timer(move |_| {
        if !idle_active.get() || !voices.lock().unwrap().is_empty() {
            quiet_since.set(None);
            return;
        }
        let since = quiet_since.get().unwrap_or_else(Instant::now);
        quiet_since.set(Some(since));
        if since.elapsed() >= IDLE {
            let _ = idle_stream.set_active(false);
            idle_active.set(false);
        }
    });
    timer.update_timer(Some(Duration::from_millis(500)), Some(Duration::from_millis(500))).into_result()?;
    let _woken = woken.attach(mainloop.loop_(), move |()| {
        if !active.get() {
            let _ = stream.set_active(true);
            active.set(true);
        }
    });

    mainloop.run();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitch::{window_for, Detector};

    fn note(hz: f64, secs: f64) -> Vec<f32> {
        let mut voices = vec![Voice { hz, n: 0 }];
        let mut out = vec![0.0; (secs * RATE as f64) as usize];
        render(&mut voices, &mut out);
        out
    }

    #[test]
    fn a_note_reads_at_its_pitch_and_dies_away() {
        let mut d = Detector::new(RATE, window_for(RATE));
        let n = d.window();
        for hz in [87.31, 146.83, 196.0, 261.63, 523.25] {
            let x = note(hz, LENGTH + 0.1);
            for at in [0.1, 0.8, 1.6] {
                let start = (at * RATE as f64) as usize;
                let p = d.detect(&x[start..start + n]).expect("a pitch");
                let cents = 1200.0 * (p.hz as f64 / hz).log2();
                assert!(cents.abs() < 2.0, "{hz} Hz at {at} s read {:.2} Hz", p.hz);
            }
            let peak = |from: f64, to: f64| {
                x[(from * RATE as f64) as usize..(to * RATE as f64) as usize].iter().fold(0.0f32, |m, v| m.max(v.abs()))
            };
            assert!(peak(0.0, 0.1) > 0.1 && peak(0.0, 0.1) < 0.6, "{hz}: loud but not clipping");
            assert!(peak(LENGTH, LENGTH + 0.1) == 0.0, "{hz}: silent after it ends");
        }
    }

    #[test]
    fn finished_voices_are_dropped_and_old_ones_make_room() {
        let mut voices = vec![Voice { hz: 220.0, n: 0 }];
        let mut out = vec![0.0; (LENGTH * RATE as f64) as usize + 1];
        render(&mut voices, &mut out);
        assert!(voices.is_empty());

        let mut piano = Piano::new(Voices::default(), false);
        for i in 0..MAX_VOICES + 3 {
            piano.play(100.0 + i as f32);
        }
        let voices = piano.voices.lock().unwrap();
        assert_eq!(voices.len(), MAX_VOICES);
        assert_eq!(voices[0].hz, 103.0, "the oldest went first");
    }
}
