//! Mac Setup checker: macOS settings and permissions that can stop a feature.
//! `build_items` is pure; the readers below gather its input.

use std::ffi::c_void;
use std::process::Command;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication, NSScreen, NSWorkspace};
use objc2_foundation::{NSDictionary, NSNumber, NSString};

use crate::login::{LOGIN_ITEMS_URL, LoginStatus};
use crate::paths::APP_NAME;

pub const ACCESSIBILITY_URL: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
pub const TRACKPAD_URL: &str = "x-apple.systempreferences:com.apple.Trackpad-Settings.extension";
pub const DESKTOP_URL: &str = "x-apple.systempreferences:com.apple.Desktop-Settings.extension";
pub const DISPLAYS_URL: &str = "x-apple.systempreferences:com.apple.Displays-Settings.extension";

/// Other window managers, matched by bundle id.
const WM_APPS: [(&str, &str); 7] = [
    ("com.knollsoft.Rectangle", "Rectangle"),
    ("com.crowdcafe.windowmagnet", "Magnet"),
    ("bobko.aerospace", "AeroSpace"),
    ("com.amethyst.Amethyst", "Amethyst"),
    ("com.MrKai77.Loop", "Loop"),
    ("com.manytricks.Moom", "Moom"),
    ("com.hegenberg.BetterSnapTool", "BetterSnapTool"),
];
/// Other window managers, matched by process name.
const WM_PROCS: [(&str, &str); 2] = [("yabai", "yabai"), ("paneru", "Paneru")];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warning,
    Missing,
    Info,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fix {
    /// Prompt for Accessibility, then open its pane.
    Accessibility,
    Open(&'static str),
    Fingers(i64),
    /// Quit another window manager (pid, is a GUI app).
    Quit(i32, bool),
    StartAtLogin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: &'static str,
    pub status: Status,
    pub title: String,
    pub detail: String,
    pub affects: String,
    pub fixes: Vec<(String, Fix)>,
}

/// `defaults read` values; None when unset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Defaults {
    pub three_swipe: Option<i64>,
    pub four_swipe: Option<i64>,
    pub three_drag: Option<bool>,
    pub spans_displays: Option<bool>,
    pub stage_manager: Option<bool>,
    pub mru_spaces: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Env {
    pub ax_trusted: bool,
    pub ax_was_granted: bool,
    pub defaults: Defaults,
    /// Screen frames as (x, y, w, h).
    pub screens: Vec<[f64; 4]>,
    /// Running window managers: (name, pid, is a GUI app).
    pub wms: Vec<(String, i32, bool)>,
    pub login: LoginStatus,
    pub swipe_enabled: bool,
    pub swipe_fingers: i64,
    pub start_at_login: bool,
}

pub fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "y" => Some(true),
        "0" | "false" | "no" | "n" => Some(false),
        _ => None,
    }
}

pub fn parse_int(s: &str) -> Option<i64> {
    s.trim().parse().ok().or_else(|| parse_bool(s).map(i64::from))
}

/// Any two displays sharing a horizontal band sit side by side.
pub fn side_by_side(frames: &[[f64; 4]]) -> bool {
    frames.iter().enumerate().any(|(i, a)| {
        frames[i + 1..].iter().any(|b| (a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1]) > 1.0)
    })
}

fn item(id: &'static str, status: Status, title: impl Into<String>, detail: impl Into<String>, affects: &str) -> Item {
    Item { id, status, title: title.into(), detail: detail.into(), affects: affects.into(), fixes: vec![] }
}

fn word(n: i64) -> &'static str {
    if n == 3 { "Three" } else { "Four" }
}

pub fn build_items(e: &Env) -> Vec<Item> {
    let mut items = Vec::new();
    let d = &e.defaults;

    let affects = "Everything: moving windows, shortcuts and swipes";
    items.push(if e.ax_trusted {
        item("accessibility", Status::Ok, "Accessibility is on", format!("{APP_NAME} can move your windows."), affects)
    } else {
        let mut it = if e.ax_was_granted {
            item(
                "accessibility",
                Status::Missing,
                "Accessibility needs turning off and on again",
                format!("macOS needs you to turn {APP_NAME} off and on again in Accessibility, or remove it with the minus button and add it back. This happens after an update."),
                affects,
            )
        } else {
            item(
                "accessibility",
                Status::Missing,
                format!("Turn on Accessibility for {APP_NAME}"),
                format!("{APP_NAME} can't move windows without it. Turn it on in Privacy & Security › Accessibility."),
                affects,
            )
        };
        it.fixes.push(("Open Accessibility".into(), Fix::Accessibility));
        it
    });

    if e.swipe_enabled {
        let n = if e.swipe_fingers == 3 { 3 } else { 4 };
        let m = 7 - n;
        let affects = format!("{n}-finger swipe");
        let macos_uses = |k: i64| if k == 3 { d.three_swipe == Some(2) } else { d.four_swipe == Some(2) };
        let drag = d.three_drag == Some(true);
        items.push(if n == 3 && drag {
            let mut it = item(
                "swipe",
                Status::Warning,
                "Three-finger drag is on",
                format!("Swipe won't work yet: three-finger drag uses the same movement. Turn it off in Accessibility › Pointer Control, or pick four fingers in {APP_NAME}."),
                &affects,
            );
            it.fixes.push(("Use 4 fingers instead".into(), Fix::Fingers(4)));
            it
        } else if macos_uses(n) {
            let mut it = item(
                "swipe",
                Status::Warning,
                format!("macOS also uses {n}-finger swipes"),
                format!("Swipe won't work yet: macOS uses {n}-finger swipes to switch desktops. Turn that off in Trackpad › More Gestures, or pick {m} fingers in {APP_NAME}."),
                &affects,
            );
            if !macos_uses(m) && !(m == 3 && drag) {
                it.fixes.push((format!("Use {m} fingers instead"), Fix::Fingers(m)));
            }
            it.fixes.push(("Open Trackpad Settings".into(), Fix::Open(TRACKPAD_URL)));
            it
        } else {
            item("swipe", Status::Ok, "Trackpad swipe is free", format!("{}-finger swipes slide the row.", word(n)), &affects)
        });
    }

    let affects = "Rows on each display";
    items.push(if d.spans_displays != Some(true) {
        item("spaces", Status::Ok, "Displays have separate Spaces", "Each display gets its own row.", affects)
    } else {
        let mut it = item(
            "spaces",
            Status::Warning,
            "Turn on “Displays have separate Spaces”",
            "Each display needs its own Spaces. Turn it on in Desktop & Dock › Mission Control, then log out and back in.",
            affects,
        );
        it.fixes.push(("Open Desktop & Dock".into(), Fix::Open(DESKTOP_URL)));
        it
    });

    let affects = "The row of windows";
    items.push(if d.stage_manager != Some(true) {
        item("stage", Status::Ok, "Stage Manager is off", "Windows keep their full size.", affects)
    } else {
        let mut it = item(
            "stage",
            Status::Warning,
            "Stage Manager is on",
            "Stage Manager shrinks and groups windows, which gets in the way of the row. Turn it off in Control Center or Desktop & Dock.",
            affects,
        );
        it.fixes.push(("Open Desktop & Dock".into(), Fix::Open(DESKTOP_URL)));
        it
    });

    let affects = "Workspaces and rows staying in order";
    items.push(if d.mru_spaces == Some(false) {
        item("mru-spaces", Status::Ok, "Spaces stay in order", "macOS won't shuffle your Spaces.", affects)
    } else {
        let mut it = item(
            "mru-spaces",
            Status::Warning,
            "Turn off “Automatically rearrange Spaces”",
            "macOS reorders Spaces as you use them, so rows can move around. Turn it off in Desktop & Dock › Mission Control.",
            affects,
        );
        it.fixes.push(("Open Desktop & Dock".into(), Fix::Open(DESKTOP_URL)));
        it
    });

    let affects = "Windows waiting off-screen";
    items.push(if e.screens.len() <= 1 {
        item("displays", Status::Ok, "One display", "Nothing to arrange.", affects)
    } else if side_by_side(&e.screens) {
        let mut it = item(
            "displays",
            Status::Warning,
            "Your displays sit side by side",
            "Windows waiting off-screen can spill onto the next display. Drag your displays above one another in Displays › Arrange.",
            affects,
        );
        it.fixes.push(("Open Displays".into(), Fix::Open(DISPLAYS_URL)));
        it
    } else {
        item("displays", Status::Ok, "Displays are stacked", "Off-screen windows stay out of sight.", affects)
    });

    let affects = "Tiling: two window managers fight over the same windows";
    if e.wms.is_empty() {
        items.push(item(
            "wm",
            Status::Ok,
            "No other window managers running",
            format!("{APP_NAME} has your windows to itself."),
            affects,
        ));
    }
    for (name, pid, app) in &e.wms {
        let mut it = item(
            "wm",
            Status::Warning,
            format!("{name} is running"),
            format!("Quit {name} and turn off its launch at login."),
            affects,
        );
        it.fixes.push((format!("Quit {name}"), Fix::Quit(*pid, *app)));
        items.push(it);
    }

    let affects = "Tiling after a restart";
    items.push(match (e.start_at_login, e.login) {
        (true, LoginStatus::Enabled) => item("login", Status::Ok, "Starts at login", "Tiling is ready as soon as you log in.", affects),
        (true, LoginStatus::RequiresApproval) => {
            let mut it = item(
                "login",
                Status::Warning,
                "Start at login is waiting for approval",
                format!("Allow {APP_NAME} in General › Login Items."),
                affects,
            );
            it.fixes.push(("Open Login Items".into(), Fix::Open(LOGIN_ITEMS_URL)));
            it
        }
        (true, LoginStatus::NotFound) => item(
            "login",
            Status::Info,
            "Doesn't start at login",
            format!("Available once {APP_NAME} is in Applications."),
            affects,
        ),
        _ => {
            let mut it = item(
                "login",
                Status::Info,
                "Doesn't start at login",
                "Turn on Start at login in Settings › General to tile right after a restart.",
                affects,
            );
            it.fixes.push(("Fix…".into(), Fix::StartAtLogin));
            it
        }
    });
    items
}

/// Items that need the user's attention (Info is a choice, not a problem).
pub fn attention(items: &[Item]) -> usize {
    items.iter().filter(|i| matches!(i.status, Status::Warning | Status::Missing)).count()
}

pub fn required_failure(items: &[Item]) -> bool {
    items.iter().any(|i| i.id == "accessibility" && i.status != Status::Ok)
}

// ---- readers ----

fn read_default(domain: &str, key: &str) -> Option<String> {
    let out = Command::new("/usr/bin/defaults").args(["read", domain, key]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Spawns `defaults`; call off the UI thread.
pub fn read_defaults() -> Defaults {
    let pads = ["com.apple.AppleMultitouchTrackpad", "com.apple.driver.AppleBluetoothMultitouch.trackpad"];
    let int = |key| pads.iter().filter_map(|d| read_default(d, key).as_deref().and_then(parse_int)).max();
    let flag = |d, key| read_default(d, key).as_deref().and_then(parse_bool);
    Defaults {
        three_swipe: int("TrackpadThreeFingerHorizSwipeGesture"),
        four_swipe: int("TrackpadFourFingerHorizSwipeGesture"),
        three_drag: pads.iter().filter_map(|d| flag(d, "TrackpadThreeFingerDrag")).max(),
        spans_displays: flag("com.apple.spaces", "spans-displays"),
        stage_manager: flag("com.apple.WindowManager", "GloballyEnabled"),
        mru_spaces: flag("com.apple.dock", "mru-spaces"),
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
}

pub fn ax_trusted() -> bool {
    // SAFETY: no arguments, returns a Boolean.
    unsafe { AXIsProcessTrusted() != 0 }
}

/// Shows the system prompt that adds the app to the Accessibility list.
pub fn ax_prompt() -> bool {
    let key = NSString::from_str("AXTrustedCheckOptionPrompt");
    let yes = NSNumber::new_bool(true);
    let dict = NSDictionary::from_slices(&[&*key], &[&*yes]);
    // SAFETY: NSDictionary is toll-free bridged with CFDictionary.
    unsafe { AXIsProcessTrustedWithOptions(objc2::rc::Retained::as_ptr(&dict).cast()) != 0 }
}

pub struct RunningApp {
    pub pid: i32,
    pub bundle_id: String,
    pub name: String,
    pub regular: bool,
}

pub fn running_apps() -> Vec<RunningApp> {
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter_map(|a| {
            Some(RunningApp {
                pid: a.processIdentifier(),
                bundle_id: a.bundleIdentifier()?.to_string(),
                name: a.localizedName().map(|n| n.to_string()).unwrap_or_default(),
                regular: a.activationPolicy() == NSApplicationActivationPolicy::Regular,
            })
        })
        .collect()
}

fn process_names() -> Vec<(i32, String)> {
    let mut pids = vec![0i32; 4096];
    // SAFETY: the buffer size is passed in bytes.
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), (pids.len() * 4) as i32) };
    pids.truncate(n.max(0) as usize);
    pids.into_iter()
        .filter_map(|pid| {
            let mut buf = [0u8; 64];
            // SAFETY: proc_name writes at most buf.len() bytes.
            let len = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
            (len > 0).then(|| (pid, String::from_utf8_lossy(&buf[..len as usize]).into_owned()))
        })
        .collect()
}

pub fn window_managers() -> Vec<(String, i32, bool)> {
    let mut found: Vec<(String, i32, bool)> = running_apps()
        .iter()
        .filter_map(|a| WM_APPS.iter().find(|(id, _)| *id == a.bundle_id).map(|(_, n)| (n.to_string(), a.pid, true)))
        .collect();
    let me = std::process::id() as i32;
    for (pid, name) in process_names() {
        if let Some((_, label)) = WM_PROCS.iter().find(|(p, _)| *p == name)
            && pid != me
        {
            found.push((label.to_string(), pid, false));
        }
    }
    found
}

pub fn quit(pid: i32, app: bool) {
    if app {
        if let Some(a) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            a.terminate();
        }
    } else {
        // SAFETY: plain kill(2).
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
}

pub struct Screen {
    pub frame: [f64; 4],
    pub scale: f64,
    pub main: bool,
}

/// Main thread only; empty elsewhere.
pub fn screens() -> Vec<Screen> {
    let Some(mtm) = MainThreadMarker::new() else { return vec![] };
    let main = NSScreen::mainScreen(mtm).map(|s| s.frame());
    NSScreen::screens(mtm)
        .iter()
        .map(|s| {
            let f = s.frame();
            Screen {
                frame: [f.origin.x, f.origin.y, f.size.width, f.size.height],
                scale: s.backingScaleFactor(),
                main: main == Some(f),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env {
            ax_trusted: true,
            ax_was_granted: true,
            defaults: Defaults { mru_spaces: Some(false), ..Defaults::default() },
            screens: vec![[0.0, 0.0, 1512.0, 982.0]],
            wms: vec![],
            login: LoginStatus::Enabled,
            swipe_enabled: true,
            swipe_fingers: 4,
            start_at_login: true,
        }
    }

    #[test]
    fn parsing() {
        assert_eq!(parse_bool("1\n"), Some(true));
        assert_eq!(parse_bool("YES"), Some(true));
        assert_eq!(parse_bool("0"), Some(false));
        assert_eq!(parse_bool("maybe"), None);
        assert_eq!(parse_int(" 2\n"), Some(2));
        assert_eq!(parse_int("true"), Some(1));
        assert!(side_by_side(&[[0.0, 0.0, 1512.0, 982.0], [1512.0, 100.0, 1920.0, 1080.0]]));
        assert!(!side_by_side(&[[0.0, 0.0, 1512.0, 982.0], [-200.0, 982.0, 1920.0, 1080.0]]));
    }

    #[test]
    fn clean_mac_has_nothing_to_fix() {
        let items = build_items(&env());
        assert_eq!(items.len(), 8);
        assert_eq!(attention(&items), 0);
        assert!(!required_failure(&items));
    }

    #[test]
    fn conflicts() {
        let mut e = env();
        e.ax_trusted = false;
        e.swipe_fingers = 3;
        e.defaults = Defaults { three_drag: Some(true), four_swipe: Some(2), spans_displays: Some(true), ..Defaults::default() };
        e.screens.push([1512.0, 0.0, 1920.0, 1080.0]);
        e.wms = vec![("Rectangle".into(), 42, true), ("yabai".into(), 43, false)];
        e.login = LoginStatus::RequiresApproval;
        let items = build_items(&e);
        assert!(required_failure(&items));
        assert_eq!(items[0].title, "Accessibility needs turning off and on again");
        assert_eq!(items[1].title, "Three-finger drag is on");
        assert_eq!(items[1].fixes[0].1, Fix::Fingers(4));
        // spaces, stage ok, mru (missing = rearranging), side by side, two wms, login approval
        assert_eq!(attention(&items), 8);
        assert_eq!(items[6].title, "Rectangle is running");

        e.swipe_fingers = 4;
        e.defaults.three_drag = None;
        let swipe = &build_items(&e)[1];
        assert_eq!(swipe.title, "macOS also uses 4-finger swipes");
        assert_eq!(swipe.fixes[0], ("Use 3 fingers instead".into(), Fix::Fingers(3)));
        e.defaults.three_swipe = Some(2);
        assert_eq!(build_items(&e)[1].fixes.len(), 1);
    }
}
