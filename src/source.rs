//! The name of the default input, for the header: from `wpctl inspect
//! @DEFAULT_AUDIO_SOURCE@`, refreshed every few seconds in case the default changes.

use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub type Shared = Arc<Mutex<Option<String>>>;

pub fn start() -> Shared {
    let shared: Shared = Arc::default();
    let name = shared.clone();
    thread::Builder::new()
        .name("input-name".into())
        .spawn(move || loop {
            let found = Command::new("wpctl")
                .args(["inspect", "@DEFAULT_AUDIO_SOURCE@"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| description(&String::from_utf8_lossy(&o.stdout)));
            *name.lock().unwrap() = found;
            thread::sleep(Duration::from_secs(3));
        })
        .expect("failed to spawn input name thread");
    shared
}

/// `node.description` (else `node.nick`) from wpctl's property list.
fn description(inspect: &str) -> Option<String> {
    let value = |key: &str| {
        inspect.lines().find_map(|line| {
            let line = line.trim_start_matches([' ', '*']);
            let rest = line.strip_prefix(key)?.trim_start().strip_prefix('=')?;
            Some(rest.trim().trim_matches('"').to_string())
        })
    };
    value("node.description").or_else(|| value("node.nick")).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_description_from_wpctl() {
        let out = r#"id 57, type PipeWire:Interface:Node
    alsa.card = "0"
  * node.description = "Built-in Audio Digital Microphone"
    node.nick = "DMIC"
"#;
        assert_eq!(description(out).as_deref(), Some("Built-in Audio Digital Microphone"));
        assert_eq!(description("  node.nick = \"Scarlett\"\n").as_deref(), Some("Scarlett"));
        assert_eq!(description("nothing here"), None);
    }
}
