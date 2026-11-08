//! Copy/paste a glyphcam configuration between machines as a short text
//! code (`glyphcam1:<base64 json>`). Only the look, framing and output format
//! travel; device-specific things (camera, ports, autostart…) stay put.

use base64::Engine as _;
use glyphcam_core::Settings;
use serde_json::{Map, Value};

const PREFIX: &str = "glyphcam1:";

/// Settings that describe the look and feel and are safe to apply anywhere.
const SHARED_KEYS: &[&str] = &[
    "mirror",
    "out_width",
    "out_height",
    "fps",
    "color_mode",
    "custom_fg",
    "custom_bg",
    "scripts",
    "mirror_katakana",
    "columns",
    "auto_exposure",
    "contrast",
    "brightness",
    "gamma",
    "edge",
    "depth",
    "glow",
    "churn",
    "saturation",
    "background",
    "bg_dim",
    "rain_speed",
    "stabilize",
    "face_size",
    "sway",
    "tilt",
    "idle_bob",
    "roll_lock",
    "vertical",
];

pub fn export(s: &Settings) -> String {
    let Ok(Value::Object(all)) = serde_json::to_value(s) else {
        return String::new();
    };
    let picked: Map<String, Value> = all
        .into_iter()
        .filter(|(k, _)| SHARED_KEYS.contains(&k.as_str()))
        .collect();
    let json = serde_json::to_vec(&Value::Object(picked)).unwrap_or_default();
    format!(
        "{PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    )
}

/// Apply a pasted code (or raw settings JSON) on top of `current`.
/// Returns the new settings and how many values were applied.
pub fn import(text: &str, current: &Settings) -> Result<(Settings, usize), String> {
    let text = text.trim().trim_matches('"').trim();
    let json: Vec<u8> = if let Some(rest) = text.strip_prefix(PREFIX) {
        let clean: String = rest.chars().filter(|c| !c.is_whitespace()).collect();
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(clean.trim_end_matches('='))
            .map_err(|_| "That code looks damaged. Copy it again.".to_string())?
    } else if text.starts_with('{') {
        text.as_bytes().to_vec()
    } else {
        return Err("The clipboard doesn't contain glyphcam settings.".into());
    };
    let incoming: Value =
        serde_json::from_slice(&json).map_err(|_| "Couldn't read those settings.".to_string())?;
    let Value::Object(incoming) = incoming else {
        return Err("Couldn't read those settings.".into());
    };
    let Ok(Value::Object(mut base)) = serde_json::to_value(current) else {
        return Err("internal error".into());
    };
    let mut n = 0;
    for (k, v) in incoming {
        if SHARED_KEYS.contains(&k.as_str()) {
            base.insert(k, v);
            n += 1;
        }
    }
    if n == 0 {
        return Err("No glyphcam settings found in that text.".into());
    }
    let mut s: Settings = serde_json::from_value(Value::Object(base))
        .map_err(|e| format!("Couldn't apply settings: {e}"))?;
    s.sanitize();
    Ok((s, n))
}

pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut c| c.set_text(text.to_owned()))
        .map_err(|e| e.to_string())
}

pub fn read_clipboard() -> Result<String, String> {
    arboard::Clipboard::new()
        .and_then(|mut c| c.get_text())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use glyphcam_core::settings::{ColorMode, SourceKind};

    #[test]
    fn roundtrip_keeps_look_but_not_device_settings() {
        let a = Settings {
            color_mode: ColorMode::Synthwave,
            sway: 0.9,
            columns: 200,
            stream_port: 9999,
            source: SourceKind::Demo,
            ..Settings::default()
        };
        let code = export(&a);
        assert!(code.starts_with(PREFIX));
        let b = Settings::default();
        let (c, n) = import(&code, &b).unwrap();
        assert!(n > 20);
        assert_eq!(c.color_mode, ColorMode::Synthwave);
        assert_eq!(c.sway, 0.9);
        assert_eq!(c.columns, 200);
        assert_eq!(c.stream_port, b.stream_port);
        assert_eq!(c.source, b.source);
    }

    #[test]
    fn rejects_garbage() {
        assert!(import("hello", &Settings::default()).is_err());
        assert!(import("glyphcam1:!!!", &Settings::default()).is_err());
    }
}
