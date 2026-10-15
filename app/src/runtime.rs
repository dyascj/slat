//! Contract between the menu-bar agent (`Slat`) and window processes
//! (`Slat ui <window>`). The agent publishes its state to `runtime.json`;
//! windows ask it to act through signals.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::paths;

/// Window -> agent: restart the engine (or start it when stopped).
pub const SIG_RESTART: i32 = libc::SIGUSR1;
/// Window -> agent: onboarding finished; start tiling and refresh state.
pub const SIG_START: i32 = libc::SIGUSR2;
/// Window -> agent: check GitHub for an update now.
pub const SIG_CHECK_UPDATES: i32 = libc::SIGHUP;
/// Window -> agent: install the available update and relaunch.
pub const SIG_INSTALL_UPDATE: i32 = libc::SIGINFO;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineState {
    Running,
    #[default]
    Stopped,
    Crashed,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Runtime {
    pub pid: i32,
    pub engine: EngineState,
    /// Crash message, e.g. "Engine exited with status 1".
    pub message: Option<String>,
    pub workspace: Option<i64>,
    pub focused_app: Option<String>,
    /// Newer version found by the update checker.
    pub update_available: Option<String>,
    /// "installing", "ready" or "failed: <reason>" while an update is in flight.
    pub update_status: Option<String>,
}

pub fn file() -> PathBuf {
    paths::support_dir().join("runtime.json")
}

impl Runtime {
    pub fn read() -> Option<Runtime> {
        let r: Runtime = serde_json::from_str(&fs::read_to_string(file()).ok()?).ok()?;
        // SAFETY: kill with signal 0 only probes for existence.
        (unsafe { libc::kill(r.pid, 0) } == 0).then_some(r)
    }

    pub fn write(&self) {
        let tmp = file().with_extension("json.tmp");
        if fs::write(&tmp, serde_json::to_vec(self).unwrap_or_default()).is_ok() {
            let _ = fs::rename(tmp, file());
        }
    }
}

/// Signals the running agent; false when no agent is running.
pub fn signal_agent(sig: i32) -> bool {
    // SAFETY: plain kill(2) on the agent's pid.
    Runtime::read().is_some_and(|r| unsafe { libc::kill(r.pid, sig) } == 0)
}
