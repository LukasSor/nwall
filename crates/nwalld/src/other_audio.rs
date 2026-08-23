
use serde_json::Value;
use std::process::Command;
use std::time::{Duration, Instant};

/// Cached probe so we don't spawn `pactl` every compositor tick.
pub struct OtherAudioProbe {
    last_check: Option<Instant>,
    active: bool,
    interval: Duration,
}

impl Default for OtherAudioProbe {
    fn default() -> Self {
        Self {
            last_check: None,
            active: false,
            interval: Duration::from_millis(900),
        }
    }
}

impl OtherAudioProbe {
    pub fn active(&mut self, except_pids: &[u32]) -> bool {
        let due = self
            .last_check
            .map(|t| t.elapsed() >= self.interval)
            .unwrap_or(true);
        if due {
            self.active = other_audio_playing(except_pids);
            self.last_check = Some(Instant::now());
        }
        self.active
    }
}

fn other_audio_playing(except_pids: &[u32]) -> bool {
    let Ok(out) = Command::new("pactl")
        .args(["-f", "json", "list", "sink-inputs"])
        .output()
    else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let Ok(v) = serde_json::from_slice::<Value>(&out.stdout) else {
        return false;
    };
    let Some(arr) = v.as_array() else {
        return false;
    };
    for si in arr {
        if sink_input_is_foreign_playback(si, except_pids) {
            return true;
        }
    }
    false
}

fn sink_input_is_foreign_playback(si: &Value, except_pids: &[u32]) -> bool {
    if si.get("corked").and_then(|v| v.as_bool()).unwrap_or(false) {
        return false;
    }
    if si.get("mute").and_then(|v| v.as_bool()).unwrap_or(false) {
        return false;
    }
    let props = si
        .get("properties")
        .or_else(|| si.get("proplist"))
        .cloned()
        .unwrap_or(Value::Null);
    let app = prop_str(&props, "application.name");
    let media = prop_str(&props, "media.name");
    let node = prop_str(&props, "node.name");
    let binary = prop_str(&props, "application.process.binary");
    let pid = prop_str(&props, "application.process.id")
        .and_then(|s| s.parse::<u32>().ok());

    if let Some(pid) = pid {
        if except_pids.contains(&pid) {
            return false;
        }
    }

    let blob = format!(
        "{} {} {} {}",
        app.unwrap_or(""),
        media.unwrap_or(""),
        node.unwrap_or(""),
        binary.unwrap_or("")
    )
    .to_ascii_lowercase();

    // Our own streams (video audio sink name is `nwall`, music is `nwall-bg`).
    if blob.contains("nwall") {
        return false;
    }
    // Always-on dummies / accessibility.
    if blob.contains("speech-dispatcher") || blob.contains("sd_dummy") {
        return false;
    }
    // Chat/VoIP clients often keep an uncorked stream while idle in a call UI.
    const SKIP_VOIP: &[&str] = &[
        "discord",
        "legcord",
        "vesktop",
        "zoom",
        "teams",
        "skype",
        "slack",
        "element",
        "signal",
        "telegram",
        "webex",
        "mumble",
        "teamspeak",
    ];
    if SKIP_VOIP.iter().any(|k| blob.contains(k)) {
        return false;
    }

    true
}

fn prop_str<'a>(props: &'a Value, key: &str) -> Option<&'a str> {
    props
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}
