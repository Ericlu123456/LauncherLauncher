//! Application data locations, kept next to the usual user directories.
//!
//! `LL_HOME` overrides the base directory (used by the test suite and portable
//! installs); otherwise the platform data/config directories are used.

use std::fs;
use std::io;
use std::path::PathBuf;

pub const APP_DIR: &str = "LauncherLauncher";

fn override_home() -> Option<PathBuf> {
    std::env::var_os("LL_HOME").map(PathBuf::from)
}

pub fn data_dir() -> PathBuf {
    match override_home() {
        Some(p) => p.join(APP_DIR),
        None => dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP_DIR),
    }
}

pub fn config_dir() -> PathBuf {
    match override_home() {
        Some(p) => p.join(APP_DIR).join("config"),
        None => dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP_DIR),
    }
}

pub fn backups_dir() -> PathBuf {
    data_dir().join("backups")
}

pub fn patched_dir() -> PathBuf {
    data_dir().join("patched")
}

pub fn cache_dir() -> PathBuf {
    data_dir().join("cache")
}

pub fn state_file() -> PathBuf {
    data_dir().join("state.json")
}

pub fn ensure_dirs() -> io::Result<()> {
    for p in [
        data_dir(),
        config_dir(),
        backups_dir(),
        patched_dir(),
        cache_dir(),
    ] {
        fs::create_dir_all(p)?;
    }
    Ok(())
}
