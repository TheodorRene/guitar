//! Pitch detection with the McLeod Pitch Method (MPM).
//!
//! For a window of samples it computes the normalised square difference function (NSDF),
//! which is 1 at lags where the signal repeats exactly, finds its "key maxima" (the highest
//! point of each positive lobe), and takes the first one that comes within `K` of the
//! highest. Picking the first good lobe rather than the best one is what keeps it from
//! landing an octave low, and working on the period rather than the spectrum is what keeps
//! it from landing an octave high on a string whose second harmonic is louder than its
//! fundamental.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// Lowest pitch reported: below a drop-C low string (65 Hz), with room for it to be flat.
pub const MIN_HZ: f32 = 55.0;
/// Highest pitch reported: past the top of a guitar neck and a soprano.
pub const MAX_HZ: f32 = 1500.0;
/// How close to the highest key maximum the chosen one must be (McLeod's k).
const K: f32 = 0.9;
/// Below this energy per sample the window is treated as silence.
const SILENT: f32 = 1e-10;

/// The input is low-passed here before detection. The period is plain below it, and hiss
/// above it makes the reading wander as a note fades: on a decaying low string in light
/// noise, this took the jitter from 3.6 to 0.5 cents RMS.
pub const PREFILTER_HZ: f32 = 1000.0;

/// A 2nd-order Butterworth low-pass (RBJ cookbook biquad), run over the input stream.
pub struct Lowpass {
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
}

impl Lowpass {
    pub fn new(rate: u32) -> Self {
        let w = std::f32::consts::TAU * PREFILTER_HZ / rate as f32;
        let (s, c) = w.sin_cos();
        let alpha = s * std::f32::consts::FRAC_1_SQRT_2; // sin(w) / (2·Q), Q = 1/√2
        let a0 = 1.0 + alpha;
        let b1 = (1.0 - c) / a0;
        Self { b: [b1 / 2.0, b1, b1 / 2.0], a: [-2.0 * c / a0, (1.0 - alpha) / a0], x: [0.0; 2], y: [0.0; 2] }
    }

    pub fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1] - self.a[0] * self.y[0] - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pitch {
    pub hz: f32,
    /// How periodic the window is at that pitch, 0–1 (the NSDF peak).
    pub clarity: f32,
}

/// Window length for a sample rate: about 43 ms, three periods of the lowest string.
pub fn window_for(rate: u32) -> usize {
    match rate {
        0..=50_000 => 2048,
        50_001..=100_000 => 4096,
        _ => 8192,
    }
}

pub struct Detector {
    rate: f32,
    window: usize,
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    /// The window without its mean, zero-padded to twice its length.
    padded: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    acf: Vec<f32>,
    nsdf: Vec<f32>,
    scratch_fwd: Vec<Complex<f32>>,
    scratch_inv: Vec<Complex<f32>>,
}

impl Detector {
    pub fn new(rate: u32, window: usize) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(2 * window);
        let inverse = planner.plan_fft_inverse(2 * window);
        Self {
            rate: rate as f32,
            window,
            spectrum: forward.make_output_vec(),
            scratch_fwd: forward.make_scratch_vec(),
            scratch_inv: inverse.make_scratch_vec(),
            padded: vec![0.0; 2 * window],
            acf: vec![0.0; 2 * window],
            nsdf: vec![0.0; window],
            forward,
            inverse,
        }
    }

    pub fn window(&self) -> usize {
        self.window
    }

    /// The pitch of `samples` (exactly `window()` of them), if it has one in range. The
    /// caller decides from `clarity` and the level whether to believe it.
    pub fn detect(&mut self, samples: &[f32]) -> Option<Pitch> {
        let n = self.window;
        assert_eq!(samples.len(), n, "detector window");

        let mean = samples.iter().sum::<f32>() / n as f32;
        for (p, &s) in self.padded.iter_mut().zip(samples) {
            *p = s - mean;
        }
        self.padded[n..].fill(0.0);
        let x = &self.padded[..n];
        let energy: f32 = x.iter().map(|v| v * v).sum();
        if energy < SILENT * n as f32 {
            return None;
        }

        // Autocorrelation by FFT: the inverse transform of the power spectrum. Padding to
        // twice the length keeps it linear (no wrap-around).
        self.forward
            .process_with_scratch(&mut self.padded, &mut self.spectrum, &mut self.scratch_fwd)
            .ok()?;
        for c in self.spectrum.iter_mut() {
            *c = Complex::new(c.norm_sqr(), 0.0);
        }
        self.inverse
            .process_with_scratch(&mut self.spectrum, &mut self.acf, &mut self.scratch_inv)
            .ok()?;
        // realfft leaves the forward transform's input scrambled; the window's samples are
        // still needed for m(τ), so take them from the caller's slice again.
        let x = |i: usize| samples[i] - mean;

        // NSDF(τ) = 2·r(τ) / m(τ), where m(τ) = Σ x[j]² + x[j+τ]² over the overlap, updated
        // by dropping one sample from each end as τ grows.
        let scale = 1.0 / (2 * n) as f32;
        let mut m = 2.0 * energy;
        let max_lag = ((self.rate / MIN_HZ).ceil() as usize + 2).min(n / 2);
        for tau in 0..max_lag {
            if tau > 0 {
                m -= x(tau - 1).powi(2) + x(n - tau).powi(2);
            }
            self.nsdf[tau] = if m > SILENT { 2.0 * self.acf[tau] * scale / m } else { 0.0 };
        }
        let nsdf = &self.nsdf[..max_lag];

        // Key maxima: the top of each positive lobe, skipping the one around lag 0.
        let min_lag = ((self.rate / MAX_HZ).floor() as usize).max(1);
        let mut peaks: Vec<usize> = Vec::new();
        let mut tau = 1;
        while tau < max_lag && nsdf[tau] > 0.0 {
            tau += 1;
        }
        let mut best: Option<usize> = None;
        while tau < max_lag - 1 {
            if nsdf[tau] > 0.0 {
                if best.is_none_or(|b| nsdf[tau] > nsdf[b]) {
                    best = Some(tau);
                }
            } else if let Some(b) = best.take() {
                peaks.push(b);
            }
            tau += 1;
        }
        // A lobe still open at the end counts only if it has turned down already.
        if let Some(b) = best {
            if b < max_lag - 2 {
                peaks.push(b);
            }
        }
        peaks.retain(|&p| p >= min_lag && p + 1 < max_lag);

        let highest = peaks.iter().map(|&p| nsdf[p]).fold(f32::MIN, f32::max);
        let &chosen = peaks.iter().find(|&&p| nsdf[p] >= K * highest)?;

        // Parabolic interpolation through the peak and its neighbours.
        let (a, b, c) = (nsdf[chosen - 1], nsdf[chosen], nsdf[chosen + 1]);
        let denom = a - 2.0 * b + c;
        let (shift, value) = if denom.abs() > f32::EPSILON {
            let d = 0.5 * (a - c) / denom;
            (d, b - 0.25 * (a - c) * d)
        } else {
            (0.0, b)
        };
        let hz = self.rate / (chosen as f32 + shift);
        (MIN_HZ * 0.97..=MAX_HZ * 1.03)
            .contains(&hz)
            .then_some(Pitch { hz, clarity: value.min(1.0) })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const RATE: u32 = 48_000;

    /// A tone with the given harmonic amplitudes (fundamental first), starting at `phase`.
    pub fn tone(hz: f32, harmonics: &[f32], n: usize, phase: f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                harmonics
                    .iter()
                    .enumerate()
                    .map(|(k, a)| a * (TAU * hz * (k + 1) as f32 * t + phase * (k + 1) as f32).sin())
                    .sum::<f32>()
                    * 0.3
            })
            .collect()
    }

    fn cents(a: f32, b: f32) -> f32 {
        1200.0 * (a / b).log2()
    }

    fn noise(n: usize, mut seed: u32) -> Vec<f32> {
        (0..n)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as f32 / u32::MAX as f32 - 0.5
            })
            .collect()
    }

    #[test]
    fn finds_sines_across_the_range_to_within_a_cent() {
        let mut d = Detector::new(RATE, window_for(RATE));
        for hz in [61.7, 73.42, 82.41, 110.0, 146.83, 196.0, 246.94, 329.63, 440.0, 659.26, 987.77] {
            let p = d.detect(&tone(hz, &[1.0], d.window(), 0.3)).expect("a pitch");
            assert!(cents(p.hz, hz).abs() < 1.0, "{hz} Hz read as {} Hz", p.hz);
            assert!(p.clarity > 0.95, "{hz} Hz clarity {}", p.clarity);
        }
    }

    #[test]
    fn a_plucked_string_with_a_weak_fundamental_reads_at_its_fundamental() {
        // Low E: the second and third harmonics are far louder than the first.
        let mut d = Detector::new(RATE, window_for(RATE));
        let harmonics = [0.25, 1.0, 0.8, 0.5, 0.4, 0.2, 0.15, 0.1];
        for hz in [82.41, 110.0, 146.83] {
            let p = d.detect(&tone(hz, &harmonics, d.window(), 1.1)).expect("a pitch");
            assert!(cents(p.hz, hz).abs() < 1.0, "{hz} Hz read as {} Hz", p.hz);
        }
    }

    #[test]
    fn a_voice_like_tone_in_light_noise_stays_within_two_cents() {
        let mut d = Detector::new(RATE, window_for(RATE));
        let n = d.window();
        for hz in [130.81, 220.0, 392.0] {
            let mut x = tone(hz, &[1.0, 0.7, 0.5, 0.35, 0.25, 0.18, 0.12], n, 0.0);
            for (s, z) in x.iter_mut().zip(noise(n, 99)) {
                *s += 0.03 * z;
            }
            let p = d.detect(&x).expect("a pitch");
            assert!(cents(p.hz, hz).abs() < 2.0, "{hz} Hz read as {} Hz", p.hz);
            assert!(p.clarity > 0.85);
        }
    }

    #[test]
    fn noise_has_no_clear_pitch_and_silence_none_at_all() {
        let mut d = Detector::new(RATE, window_for(RATE));
        let n = d.window();
        if let Some(p) = d.detect(&noise(n, 12345)) {
            assert!(p.clarity < 0.5, "noise clarity {}", p.clarity);
        }
        assert_eq!(d.detect(&vec![0.0; n]), None);
        // A DC offset alone is silence too.
        assert_eq!(d.detect(&vec![0.2; n]), None);
    }

    #[test]
    fn the_prefilter_keeps_the_low_end_and_cuts_hiss() {
        // Steady-state gain of a sine through the filter.
        let gain = |hz: f32| {
            let mut lp = Lowpass::new(RATE);
            let out: Vec<f32> = tone(hz, &[1.0], RATE as usize / 2, 0.0).into_iter().map(|x| lp.run(x)).collect();
            let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            peak(&out[RATE as usize / 4..]) / 0.3
        };
        assert!((gain(110.0) - 1.0).abs() < 0.01);
        assert!((gain(PREFILTER_HZ) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "−3 dB at the corner");
        assert!(gain(8000.0) < 0.02);
    }

    #[test]
    fn works_at_44_1_and_96_khz() {
        for rate in [44_100, 96_000] {
            let mut d = Detector::new(rate, window_for(rate));
            let n = d.window();
            let x: Vec<f32> = (0..n).map(|i| (TAU * 82.41 * i as f32 / rate as f32).sin()).collect();
            let p = d.detect(&x).expect("a pitch");
            assert!(cents(p.hz, 82.41).abs() < 1.0, "{rate}: {} Hz", p.hz);
        }
    }
}
