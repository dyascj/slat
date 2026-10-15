//! Every file the app touches lives under these roots.

use std::path::PathBuf;

pub const APP_NAME: &str = "Slat";
pub const BUNDLE_ID: &str = "io.github.dyascj.slat";
pub const REPO_URL: &str = "https://github.com/dyascj/slat";

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

pub fn support_dir() -> PathBuf {
    home().join("Library/Application Support").join(APP_NAME)
}
pub fn state_dir() -> PathBuf {
    support_dir().join("state")
}
pub fn logs_dir() -> PathBuf {
    home().join("Library/Logs").join(APP_NAME)
}
pub fn settings_file() -> PathBuf {
    support_dir().join("settings.json")
}
pub fn engine_config() -> PathBuf {
    support_dir().join("engine.toml")
}
pub fn engine_log() -> PathBuf {
    logs_dir().join("engine.log")
}
pub fn engine_stdout_log() -> PathBuf {
    logs_dir().join("engine-stdout.log")
}

pub fn ensure_dirs() {
    for d in [support_dir(), state_dir(), logs_dir()] {
        let _ = std::fs::create_dir_all(d);
    }
}

/// `~`-abbreviated path for display.
pub fn tilde(p: &std::path::Path) -> String {
    let s = p.display().to_string();
    let h = home().display().to_string();
    s.strip_prefix(&h).map_or(s.clone(), |rest| format!("~{rest}"))
}
