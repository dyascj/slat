//! Small app state that is not a user setting (the original kept these in
//! UserDefaults). Shared by the agent and window processes.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::paths;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    pub onboarding_completed: bool,
    /// Accessibility was trusted at least once; an untrusted check after this
    /// means a stale TCC entry (needs toggling off and on).
    pub accessibility_was_granted: bool,
    /// Unix seconds of the last successful update check.
    pub updates_last_checked_at: Option<u64>,
    pub updates_latest_version: Option<String>,
}

fn file() -> PathBuf {
    paths::support_dir().join("prefs.json")
}

impl Prefs {
    pub fn load() -> Prefs {
        fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    /// Read-modify-write so the agent and windows don't clobber each other.
    pub fn update(f: impl FnOnce(&mut Prefs)) {
        let mut p = Prefs::load();
        f(&mut p);
        paths::ensure_dirs();
        let tmp = file().with_extension("json.tmp");
        if fs::write(&tmp, serde_json::to_vec_pretty(&p).unwrap_or_default()).is_ok() {
            let _ = fs::rename(tmp, file());
        }
    }
}
