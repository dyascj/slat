//! Engine supervision: launch `slat-engine`, restart it after crashes, stop it
//! gracefully, and follow its event stream.

use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use std::{env, fs, thread};

use super::{Agent, after, on_main};
use crate::paths;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    Running,
    #[default]
    Stopped,
    Crashed(String),
}

#[derive(Default)]
pub struct Engine {
    pub state: State,
    proc: Option<Proc>,
    generation: u64,
    launched: Option<Instant>,
    stopping: bool,
    start_after_stop: bool,
    restart_attempts: usize,
    restart_token: u64,
    subscriber: Option<Proc>,
    subscriber_generation: u64,
}

/// A child reaped by its own thread; `exited` flips once it has been waited on.
#[derive(Clone)]
pub struct Proc {
    pub pid: i32,
    exited: Arc<AtomicBool>,
}

impl Proc {
    pub fn watch(mut child: Child, on_exit: impl FnOnce(Option<ExitStatus>) + Send + 'static) -> Proc {
        let exited = Arc::new(AtomicBool::new(false));
        let flag = exited.clone();
        let pid = child.id() as i32;
        thread::spawn(move || {
            let status = child.wait().ok();
            flag.store(true, Ordering::SeqCst);
            on_exit(status);
        });
        Proc { pid, exited }
    }

    pub fn alive(&self) -> bool {
        !self.exited.load(Ordering::SeqCst)
    }

    pub fn signal(&self, sig: i32) {
        if self.alive() {
            // SAFETY: kill(2) on a child we have not reaped yet.
            unsafe { libc::kill(self.pid, sig) };
        }
    }

    /// Waits up to `timeout` for the process to exit; true when it did.
    fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while self.alive() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        !self.alive()
    }
}

/// `SLAT_ENGINE_PATH` (tilde-expanded) if executable, else `slat-engine` next to us.
pub fn locate() -> Option<PathBuf> {
    let executable = |p: &Path| fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
    if let Some(p) = env::var("SLAT_ENGINE_PATH").ok().filter(|p| !p.is_empty()) {
        let p = match p.strip_prefix("~/") {
            Some(rest) => PathBuf::from(env::var("HOME").unwrap_or_default()).join(rest),
            None => PathBuf::from(p),
        };
        if executable(&p) {
            return Some(p);
        }
    }
    let p = env::current_exe().ok()?.with_file_name("slat-engine");
    executable(&p).then_some(p)
}

fn command(bin: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(bin);
    c.args(args)
        .env("PANERU_CONFIG", paths::engine_config())
        .env("XDG_STATE_HOME", paths::state_dir())
        .env("SLAT_EMBEDDED", "1")
        .stdin(Stdio::null());
    c
}

/// Runs `slat-engine <args>` and returns its stdout; killed after `timeout`.
pub fn run_and_wait(args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = command(&locate()?, args).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut out = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + timeout;
    while matches!(child.try_wait(), Ok(None)) {
        if Instant::now() >= deadline {
            let _ = child.kill();
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let _ = child.wait();
    reader.join().ok()
}

/// Fire-and-forget `slat-engine send-cmd <args>` (menu actions, shortcuts).
pub fn send_cmd(args: Vec<String>) {
    thread::spawn(move || {
        let mut full = vec!["send-cmd"];
        full.extend(args.iter().map(String::as_str));
        run_and_wait(&full, Duration::from_secs(5));
    });
}

/// Delay before automatic restart number `attempts + 1`; None once exhausted.
pub fn restart_delay(attempts: usize) -> Option<Duration> {
    [1.0, 2.0, 5.0].get(attempts).map(|s| Duration::from_secs_f64(*s))
}

/// Healthy uptime after which the restart counter resets.
pub const RESET_AFTER: Duration = Duration::from_secs(60);

fn exit_message(status: Option<ExitStatus>) -> String {
    match status.and_then(|s| s.signal()) {
        Some(sig) => format!("Engine exited with signal {sig}"),
        None => format!("Engine exited with status {}", status.and_then(|s| s.code()).unwrap_or(-1)),
    }
}

/// Graceful quit, then SIGTERM, then SIGKILL.
fn stop_sequence(p: &Proc, quit_timeout: Duration) {
    run_and_wait(&["send-cmd", "quit"], quit_timeout);
    if p.wait(Duration::from_secs(2)) {
        return;
    }
    p.signal(libc::SIGTERM);
    if !p.wait(Duration::from_secs(2)) {
        p.signal(libc::SIGKILL);
    }
}

/// What an engine event or `query active` tells us.
#[derive(Debug, Default, PartialEq)]
pub struct Update {
    pub workspace: Option<i64>,
    pub app_name: Option<String>,
    pub bundle_id: Option<String>,
}

fn int(v: &Value, k: &str) -> Option<i64> {
    v.get(k)?.as_i64()
}
fn text(v: &Value, k: &str) -> Option<String> {
    v.get(k)?.as_str().filter(|s| !s.is_empty()).map(String::from)
}

fn active_update(a: &Value) -> Update {
    Update {
        workspace: int(a, "virtual_workspace_number"),
        app_name: text(a, "focused_app_name"),
        bundle_id: text(a, "focused_bundle_id"),
    }
}

/// One line of `subscribe --json`.
pub fn parse_event(line: &str) -> Option<Update> {
    let v: Value = serde_json::from_str(line).ok()?;
    match v.get("event")?.as_str()? {
        "virtual_workspace_changed" => Some(Update {
            workspace: int(&v, "virtual_workspace_number").or_else(|| int(v.get("active")?, "virtual_workspace_number")),
            ..Update::default()
        }),
        "windows_changed" => Some(active_update(v.get("active")?)),
        "window_focused" => Some(Update {
            workspace: int(&v, "virtual_workspace_number"),
            app_name: text(&v, "app_name"),
            bundle_id: text(&v, "bundle_id"),
        }),
        _ => None,
    }
}

/// Output of `query active --json`: `{"active": {...}}` (or the bare object).
pub fn parse_query(text: &str) -> Option<Update> {
    let v: Value = serde_json::from_str(text).ok()?;
    Some(active_update(v.get("active").filter(|a| a.is_object()).unwrap_or(&v)))
}

impl Agent {
    pub fn engine_running(&self) -> bool {
        self.engine.state == State::Running
    }

    fn engine_alive(&self) -> Option<Proc> {
        self.engine.proc.clone().filter(Proc::alive)
    }

    pub fn start(&mut self) {
        self.engine.restart_token += 1;
        if self.engine.stopping {
            self.engine.start_after_stop = true;
        } else if self.engine_alive().is_none() {
            self.spawn_engine();
        }
    }

    pub fn stop(&mut self) {
        self.engine.restart_token += 1;
        self.engine.start_after_stop = false;
        self.stop_subscriber();
        if let Some(p) = self.engine_alive() {
            self.engine.stopping = true;
            thread::spawn(move || stop_sequence(&p, Duration::from_secs(2)));
        } else {
            // Nothing running: forget any exit still in flight.
            self.engine.generation += 1;
            self.engine.proc = None;
        }
        self.engine.state = State::Stopped;
        self.changed();
    }

    /// Restart, or start when not running.
    pub fn restart(&mut self) {
        if self.engine_alive().is_some() {
            self.stop();
            self.engine.start_after_stop = true;
        } else {
            self.start();
        }
    }

    /// Synchronous stop used on quit and SIGTERM/SIGINT.
    pub fn stop_for_termination(&mut self) {
        self.engine.restart_token += 1;
        self.engine.start_after_stop = false;
        self.stop_subscriber();
        if let Some(p) = self.engine_alive() {
            self.engine.stopping = true;
            stop_sequence(&p, Duration::from_secs(1));
        }
        self.engine.state = State::Stopped;
    }

    fn spawn_engine(&mut self) {
        let Some(bin) = locate() else {
            self.engine.state = State::Crashed("Engine binary not found".into());
            return self.changed();
        };
        let log = fs::OpenOptions::new().create(true).append(true).open(paths::engine_stdout_log());
        let (out, err) = match log.and_then(|f| Ok((f.try_clone()?, f))) {
            Ok((a, b)) => (Stdio::from(a), Stdio::from(b)),
            Err(_) => (Stdio::null(), Stdio::null()),
        };
        match command(&bin, &["launch"]).stdout(out).stderr(err).spawn() {
            Ok(child) => {
                self.engine.generation += 1;
                let generation = self.engine.generation;
                self.engine.proc = Some(Proc::watch(child, move |st| on_main(move |a| a.engine_exited(generation, st))));
                self.engine.launched = Some(Instant::now());
                self.engine.state = State::Running;
                self.changed();
                after(Duration::from_millis(500), move |a| {
                    if a.engine.generation == generation && a.engine_running() {
                        a.start_subscriber();
                    }
                });
            }
            Err(e) => self.handle_crash(format!("Failed to launch engine: {e}")),
        }
    }

    fn engine_exited(&mut self, generation: u64, status: Option<ExitStatus>) {
        if generation != self.engine.generation {
            return;
        }
        self.engine.proc = None;
        self.stop_subscriber();
        if self.engine.stopping {
            self.engine.stopping = false;
            self.engine.state = State::Stopped;
            self.changed();
            if std::mem::take(&mut self.engine.start_after_stop) {
                self.start();
            }
        } else {
            if self.engine.launched.is_some_and(|t| t.elapsed() >= RESET_AFTER) {
                self.engine.restart_attempts = 0;
            }
            self.handle_crash(exit_message(status));
        }
    }

    fn handle_crash(&mut self, msg: String) {
        self.engine.state = State::Crashed(msg);
        if let Some(delay) = restart_delay(self.engine.restart_attempts) {
            self.engine.restart_attempts += 1;
            let token = self.engine.restart_token;
            after(delay, move |a| {
                if a.engine.restart_token == token && a.engine_alive().is_none() {
                    a.spawn_engine();
                }
            });
        }
        self.changed();
    }

    fn start_subscriber(&mut self) {
        self.stop_subscriber();
        let generation = self.engine.subscriber_generation;
        let resubscribe = move |a: &mut Agent| {
            if a.engine.subscriber_generation == generation {
                after(Duration::from_secs(1), move |a| {
                    if a.engine.subscriber_generation == generation {
                        a.start_subscriber();
                    }
                });
            }
        };
        let child = locate().and_then(|bin| {
            command(&bin, &["subscribe", "--json"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()
        });
        let Some(mut child) = child else { return resubscribe(self) };
        if let Some(out) = child.stdout.take() {
            thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    if let Some(u) = parse_event(&line) {
                        on_main(move |a| a.apply(u));
                    }
                }
            });
        }
        self.engine.subscriber = Some(Proc::watch(child, move |_| on_main(resubscribe)));
        thread::spawn(|| {
            if let Some(u) = run_and_wait(&["query", "active", "--json"], Duration::from_secs(3)).as_deref().and_then(parse_query) {
                on_main(move |a| a.apply(u));
            }
        });
    }

    fn stop_subscriber(&mut self) {
        self.engine.subscriber_generation += 1;
        if let Some(p) = self.engine.subscriber.take() {
            p.signal(libc::SIGTERM);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_policy() {
        let delays: Vec<_> = (0..5).map(restart_delay).collect();
        assert_eq!(
            delays,
            [Some(Duration::from_secs(1)), Some(Duration::from_secs(2)), Some(Duration::from_secs(5)), None, None]
        );
        assert_eq!(RESET_AFTER, Duration::from_secs(60));
    }

    #[test]
    fn exit_messages() {
        assert_eq!(exit_message(Some(ExitStatus::from_raw(1 << 8))), "Engine exited with status 1");
        assert_eq!(exit_message(Some(ExitStatus::from_raw(9))), "Engine exited with signal 9");
    }

    #[test]
    fn events() {
        let ws = |w| Update { workspace: Some(w), ..Update::default() };
        assert_eq!(parse_event(r#"{"event":"virtual_workspace_changed","virtual_workspace_number":2}"#), Some(ws(2)));
        assert_eq!(
            parse_event(r#"{"event":"virtual_workspace_changed","active":{"virtual_workspace_number":3,"focused_app_name":"X"}}"#),
            Some(ws(3))
        );
        let term = Update {
            workspace: Some(3),
            app_name: Some("Terminal".into()),
            bundle_id: Some("com.apple.Terminal".into()),
        };
        let wc = r#"{"event":"windows_changed","virtual_workspace_number":3,"active":{"display_id":1,"virtual_workspace_number":3,"focused_window_id":321,"focused_bundle_id":"com.apple.Terminal","focused_app_name":"Terminal"}}"#;
        assert_eq!(parse_event(wc), Some(term));
        assert_eq!(
            parse_event(r#"{"event":"window_focused","window_id":321,"bundle_id":"com.apple.Terminal","app_name":"","virtual_workspace_number":1}"#),
            Some(Update { workspace: Some(1), app_name: None, bundle_id: Some("com.apple.Terminal".into()) })
        );
        assert_eq!(parse_event(r#"{"event":"display_changed","display_id":1}"#), None);
        assert_eq!(parse_event("not json"), None);
        assert_eq!(parse_query(r#"{"active":{"virtual_workspace_number":4}}"#), Some(ws(4)));
        assert_eq!(parse_query(r#"{"virtual_workspace_number":5,"focused_app_name":null}"#), Some(ws(5)));
    }
}
