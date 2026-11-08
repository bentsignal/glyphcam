//! Settings persistence and app directories.

use glyphcam_core::Settings;
use std::path::PathBuf;

fn dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("dev", "bentsignal", "glyphcam")
}

pub fn config_dir() -> PathBuf {
    dirs()
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("glyphcam"))
}

pub fn cache_dir() -> PathBuf {
    dirs()
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("glyphcam"))
}

pub fn log_dir() -> PathBuf {
    dirs()
        .map(|d| d.data_local_dir().join("logs"))
        .unwrap_or_else(|| std::env::temp_dir().join("glyphcam"))
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn load() -> Settings {
    let mut s = std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Settings>(&t).ok())
        .unwrap_or_default();
    s.sanitize();
    s
}

pub fn save(s: &Settings) {
    let path = settings_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string_pretty(s) {
        Ok(text) => {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
        Err(e) => log::warn!("saving settings: {e}"),
    }
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_SHA: &str = env!("GLYPHCAM_BUILD_SHA");
/// "stable", "nightly" or "dev".
pub const CHANNEL: &str = env!("GLYPHCAM_CHANNEL");

pub fn version_string() -> String {
    match CHANNEL {
        "stable" => format!("v{VERSION}"),
        ch => format!("v{VERSION} ({ch} {BUILD_SHA})"),
    }
}
