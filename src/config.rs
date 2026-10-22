//! Persisted user configuration.

use crate::paths;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Last selected launcher, restored on next start.
    pub launcher_path: Option<PathBuf>,
    /// Apply the "no update check" hook profile.
    pub remove_updates: bool,
    /// Start the built-in keygen tune.
    pub music_enabled: bool,
    /// Keep the tune running but at zero volume.
    pub muted: bool,
    /// 0.0 ..= 1.0
    pub volume: f32,
    /// Optional user rule set (JSON). Appended after the built-in profile.
    pub custom_rules: String,
    /// Last directory used by the file picker.
    pub last_dir: Option<PathBuf>,
    /// Also try the built-in "gate hunter" scan (aggressive, opt-in).
    pub auto_scan: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            launcher_path: None,
            remove_updates: true,
            music_enabled: true,
            muted: false,
            volume: 0.35,
            custom_rules: String::new(),
            last_dir: None,
            auto_scan: false,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let file = paths::config_dir().join("config.json");
        fs::read_to_string(file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let file = paths::config_dir().join("config.json");
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = crate::util::atomic_write(&file, &json);
        }
    }
}
