//! Menu-bar agent: status item, engine supervision, update checks.
//!
//! All state lives in [`Agent`] on the main thread. Background threads (engine
//! watchers, pollers, signals) hop back with [`on_main`].

mod engine;
mod icon;
mod menu;
mod updates;

use std::cell::RefCell;
use std::collections::HashMap;
use std::process::Command;
use std::ptr::NonNull;
use std::time::{Duration, Instant};
use std::{fs, thread};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchTime};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAccessibility, NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy,
    NSApplicationWillTerminateNotification, NSRunningApplication,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSString};
use tray_icon::menu::MenuEvent;
use tray_icon::{TrayIcon, TrayIconBuilder};

use crate::prefs::Prefs;
use crate::runtime::{self, EngineState, Runtime};
use crate::settings::Settings;
use crate::{config, paths};
use engine::{Engine, Proc, State};

pub struct Agent {
    tray: TrayIcon,
    engine: Engine,
    workspace: Option<i64>,
    focused_app: Option<String>,
    update: updates::Status,
    ax_trusted: bool,
    badge_shown: bool,
    windows: HashMap<String, Proc>,
    /// What the tray and runtime.json last showed, so an engine event that
    /// changes neither (most focus moves) costs nothing.
    shown: Option<(String, Runtime)>,
}

thread_local! {
    static AGENT: RefCell<Option<Agent>> = const { RefCell::new(None) };
}

/// Runs `f` with the agent (main thread only). None before init or if the
/// agent is already borrowed further up the stack.
fn with<R>(f: impl FnOnce(&mut Agent) -> R) -> Option<R> {
    AGENT.with(|cell| cell.try_borrow_mut().ok()?.as_mut().map(f))
}

/// From any thread: run `f` with the agent on the main thread.
pub(crate) fn on_main(f: impl FnOnce(&mut Agent) + Send + 'static) {
    DispatchQueue::main().exec_async(move || {
        with(f);
    });
}

/// Like [`on_main`], after `delay`.
pub(crate) fn after(delay: Duration, f: impl FnOnce(&mut Agent) + Send + 'static) {
    let when = DispatchTime::try_from(delay).unwrap_or(DispatchTime::NOW);
    let _ = DispatchQueue::main().after(when, move || {
        with(f);
    });
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

fn ax_trusted() -> bool {
    // SAFETY: no arguments, no side effects.
    unsafe { AXIsProcessTrusted() != 0 }
}

fn open_url(url: String) {
    thread::spawn(move || Command::new("/usr/bin/open").arg(url).status());
}

fn terminate() {
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).terminate(None);
    }
}

pub fn run() {
    let mtm = MainThreadMarker::new().expect("agent must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    paths::ensure_dirs();
    config::first_launch(&Settings::load());

    // NSApp.terminate (Quit, signals, logout) stops the engine before exiting.
    let on_terminate = RcBlock::new(|_: NonNull<NSNotification>| {
        with(Agent::shutdown);
    });
    // SAFETY: the block only touches main-thread state and runs on the posting (main) thread.
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSApplicationWillTerminateNotification),
            None,
            None,
            &on_terminate,
        )
    };
    std::mem::forget(observer);

    let sigs = [
        libc::SIGTERM,
        libc::SIGINT,
        runtime::SIG_RESTART,
        runtime::SIG_START,
        runtime::SIG_CHECK_UPDATES,
        runtime::SIG_INSTALL_UPDATE,
    ];
    match signal_hook::iterator::Signals::new(sigs) {
        Ok(mut signals) => {
            thread::spawn(move || {
                for sig in signals.forever() {
                    DispatchQueue::main().exec_async(move || match sig {
                        runtime::SIG_RESTART => {
                            with(Agent::restart);
                        }
                        runtime::SIG_START => {
                            with(|a| {
                                a.start();
                                a.changed();
                            });
                        }
                        runtime::SIG_CHECK_UPDATES => updates::check_in_background(),
                        runtime::SIG_INSTALL_UPDATE => {
                            with(Agent::install_update);
                        }
                        _ => terminate(),
                    });
                }
            });
        }
        Err(e) => eprintln!("Slat: signal handlers unavailable: {e}"),
    }

    // The status item needs a running NSApplication, so build it from the run loop.
    DispatchQueue::main().exec_async(init);
    app.run();
}

fn init() {
    let tray = TrayIconBuilder::new()
        .with_icon_templated(icon::glyph(false))
        .with_tooltip("Slat")
        .build()
        .expect("status item");
    let agent = Agent {
        tray,
        engine: Engine::default(),
        workspace: None,
        focused_app: None,
        update: updates::Status::default(),
        ax_trusted: ax_trusted(),
        badge_shown: false,
        windows: HashMap::new(),
        shown: None,
    };
    AGENT.with(|cell| *cell.borrow_mut() = Some(agent));
    MenuEvent::set_event_handler(Some(|e: MenuEvent| {
        let id = e.id.0;
        DispatchQueue::main().exec_async(move || {
            if id == "quit" {
                terminate();
            } else {
                with(|a| a.on_menu(&id));
            }
        });
    }));
    updates::start();
    with(|a| {
        a.changed();
        if Prefs::load().onboarding_completed {
            a.start();
        } else {
            a.open_window("onboarding", None);
        }
    });
    thread::spawn(poll);
}

/// Watches settings.json (menu reflects layout and modifier) and AX trust.
fn poll() {
    let mtime = || fs::metadata(paths::settings_file()).and_then(|m| m.modified()).ok();
    let mut last = mtime();
    let mut trusted = ax_trusted();
    let mut next_ax = Instant::now();
    loop {
        thread::sleep(Duration::from_secs(1));
        let m = mtime();
        if m != last {
            last = m;
            on_main(Agent::changed);
        }
        if Instant::now() >= next_ax {
            let t = ax_trusted();
            next_ax = Instant::now() + Duration::from_secs(if t { 60 } else { 2 });
            if t != trusted {
                trusted = t;
                on_main(move |a| {
                    a.ax_trusted = t;
                    a.changed();
                });
            }
        }
    }
}

impl Agent {
    /// Publishes state and rebuilds the menu and icon.
    fn changed(&mut self) {
        let badge = !self.ax_trusted;
        if badge != self.badge_shown {
            self.badge_shown = badge;
            let _ = self.tray.set_icon_templated(Some(icon::glyph(badge)));
        }
        self.tray.set_menu(Some(Box::new(menu::build(self))));
        self.status_changed();
    }

    /// Title, tooltip and runtime.json (cheap; runs on every engine event).
    fn status_changed(&mut self) {
        let mut label = "Slat".to_string();
        if let Some(w) = self.workspace {
            label += &format!(", workspace {w}");
        }
        if !self.ax_trusted {
            label += ", setup needs attention";
        }
        let (engine, message) = match &self.engine.state {
            State::Running => (EngineState::Running, None),
            State::Stopped => (EngineState::Stopped, None),
            State::Crashed(m) => (EngineState::Crashed, Some(m.clone())),
        };
        let (update_available, update_status) = self.update.runtime();
        let runtime = Runtime {
            pid: std::process::id() as i32,
            engine,
            message,
            workspace: self.workspace,
            focused_app: self.focused_app.clone(),
            update_available,
            update_status,
        };
        let now = (label, runtime);
        if self.shown.as_ref() == Some(&now) {
            return;
        }
        let (label, runtime) = &now;
        let title = self.workspace.map(|w| w.to_string());
        self.tray.set_title(title.as_deref());
        let _ = self.tray.set_tooltip(Some(label));
        if let (Some(item), Some(mtm)) = (self.tray.ns_status_item(), MainThreadMarker::new())
            && let Some(button) = item.button(mtm)
        {
            button.setAccessibilityLabel(Some(&NSString::from_str(label)));
        }
        runtime.write();
        self.shown = Some(now);
    }

    fn apply(&mut self, u: engine::Update) {
        if u.workspace.is_some() {
            self.workspace = u.workspace;
        }
        let name = u.app_name.or_else(|| {
            let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(&u.bundle_id?));
            Some(apps.firstObject()?.localizedName()?.to_string())
        });
        if name.is_some() {
            self.focused_app = name;
        }
        self.status_changed();
    }

    fn on_menu(&mut self, id: &str) {
        match id {
            "setup" => self.open_window("settings", Some("setup")),
            "toggle" if self.engine_running() => self.stop(),
            "toggle" => self.start(),
            "settings" => self.open_window("settings", None),
            "onboarding" => self.open_window("onboarding", None),
            "feedback" => self.open_window("feedback", None),
            "about" => self.open_window("about", None),
            "update" => self.install_update(),
            "check" => updates::check_in_background(),
            _ => {
                if let Some(cmd) = id.strip_prefix(menu::SEND) {
                    engine::send_cmd(cmd.split(' ').map(String::from).collect());
                }
            }
        }
    }

    /// `Slat ui <kind> [section]`: one process per kind. An open window is
    /// brought forward, or replaced when a section is requested.
    fn open_window(&mut self, kind: &str, section: Option<&str>) {
        if let Some(p) = self.windows.get(kind).filter(|p| p.alive()) {
            if section.is_none() {
                if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(p.pid) {
                    app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows);
                }
                return;
            }
            p.signal(libc::SIGTERM);
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let mut cmd = Command::new(exe);
        cmd.arg("ui").arg(kind).args(section);
        match cmd.spawn() {
            Ok(child) => {
                self.windows.insert(kind.to_string(), Proc::watch(child, |_| {}));
            }
            Err(e) => eprintln!("Slat: couldn't open {kind} window: {e}"),
        }
    }

    fn shutdown(&mut self) {
        self.stop_for_termination();
        for p in self.windows.values() {
            p.signal(libc::SIGTERM);
        }
        let _ = fs::remove_file(runtime::file());
    }
}
