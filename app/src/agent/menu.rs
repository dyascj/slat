//! The status item's menu, rebuilt from scratch whenever state changes.

use tray_icon::menu::accelerator::{Accelerator, Code, Modifiers};
use tray_icon::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};

use super::Agent;
use super::engine::State;
use super::updates::Status;
use crate::paths::APP_NAME;
use crate::settings::{Layout, ModKey, Settings};

/// Menu ids for engine commands carry this prefix; the rest is the command.
pub const SEND: &str = "send:";

/// (title, key, shift, engine command, columns only), grouped by divider.
type Shortcut = (&'static str, Code, bool, &'static str, bool);
const SHORTCUTS: [&[Shortcut]; 4] = [
    &[
        ("Focus Left", Code::ArrowLeft, false, "window focus west", false),
        ("Focus Right", Code::ArrowRight, false, "window focus east", false),
        ("Move Left", Code::ArrowLeft, true, "window swap west", false),
        ("Move Right", Code::ArrowRight, true, "window swap east", false),
    ],
    &[
        ("Centre", Code::KeyC, false, "window center", true),
        ("Cycle Width", Code::KeyR, false, "window resize", true),
        ("Narrower", Code::KeyR, true, "window shrink", true),
        ("Full Width", Code::KeyF, false, "window fullwidth", true),
    ],
    &[
        ("Stack Left", Code::BracketLeft, false, "window stack west", true),
        ("Stack Right", Code::BracketRight, false, "window stack east", true),
        ("Pop Out Left", Code::BracketLeft, true, "window unstack west", true),
        ("Pop Out Right", Code::BracketRight, true, "window unstack east", true),
        ("Tabs", Code::KeyT, true, "window tabbeddisplay", true),
    ],
    &[
        ("Float", Code::KeyT, false, "window manage", false),
        ("Other Display", Code::KeyN, false, "window nextdisplay", false),
    ],
];

fn mods(k: ModKey) -> Modifiers {
    match k {
        ModKey::Alt => Modifiers::ALT,
        ModKey::CtrlAlt => Modifiers::CONTROL | Modifiers::ALT,
        ModKey::CmdAlt => Modifiers::META | Modifiers::ALT,
    }
}

fn item(id: &str, text: &str, enabled: bool, accel: Option<(Modifiers, Code)>) -> MenuItem {
    MenuItem::with_id(id, text, enabled, accel.map(|(m, c)| Accelerator::new(m, c)))
}

fn shortcuts(s: &Settings) -> Submenu {
    let canvas = s.layout() == Layout::Canvas;
    let base = mods(s.mod_key);
    let mut items: Vec<Box<dyn IsMenuItem>> = Vec::new();
    for (i, group) in SHORTCUTS.iter().enumerate() {
        if i > 0 {
            items.push(Box::new(PredefinedMenuItem::separator()));
        }
        for &(title, key, shift, cmd, columns_only) in *group {
            let unused = columns_only && canvas;
            let label = if unused { format!("{title} (not used in Canvas)") } else { title.to_string() };
            let m = if shift { base | Modifiers::SHIFT } else { base };
            items.push(Box::new(item(&format!("{SEND}{cmd}"), &label, !unused, Some((m, key)))));
        }
    }
    items.push(Box::new(PredefinedMenuItem::separator()));
    items.push(Box::new(item("", &format!("{}1–9  Switch Workspace", s.mod_key.symbol()), false, None)));
    items.push(Box::new(item("", "⇧1–9  Move to Workspace", false, None)));
    let refs: Vec<&dyn IsMenuItem> = items.iter().map(AsRef::as_ref).collect();
    Submenu::with_items("Shortcuts", true, &refs).expect("submenu")
}

pub fn build(a: &Agent) -> Menu {
    let s = Settings::load();
    let menu = Menu::new();
    let sep = || PredefinedMenuItem::separator();
    let add = |i: &dyn IsMenuItem| {
        let _ = menu.append(i);
    };
    if !a.ax_trusted {
        add(&item("setup", "⚠ Setup needs attention…", true, None));
        add(&sep());
    }
    if let State::Crashed(msg) = &a.engine.state {
        add(&item("", &format!("Engine stopped: {msg}"), false, None));
        add(&sep());
    }
    let toggle = if a.engine_running() { "Pause Tiling" } else { "Resume Tiling" };
    add(&item("toggle", toggle, true, Some((Modifiers::META, Code::KeyP))));
    add(&sep());
    add(&item("settings", "Settings…", true, Some((Modifiers::META, Code::Comma))));
    add(&shortcuts(&s));
    add(&item("onboarding", "Show Onboarding", true, None));
    add(&item(&format!("{SEND}window copyrule"), "Copy Window Rule", a.engine_running(), None));
    add(&item("feedback", "Report a Bug or Send Feedback…", true, None));
    match &a.update {
        Status::Available(r) => add(&item("update", &format!("Update to {} and Restart", r.version), true, None)),
        Status::Installing(_) | Status::Ready(_) => add(&item("", "Updating…", false, None)),
        Status::Failed(..) => add(&item("update", "Update failed, try again", true, None)),
        Status::Idle => {}
    }
    add(&item("check", "Check for Updates…", true, None));
    add(&sep());
    add(&item("about", &format!("About {APP_NAME}"), true, None));
    add(&item("quit", &format!("Quit {APP_NAME}"), true, Some((Modifiers::META, Code::KeyQ))));
    menu
}
