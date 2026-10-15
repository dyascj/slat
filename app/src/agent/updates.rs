//! Auto-update from GitHub releases. A check at launch (when a day has passed
//! since the last success) and hourly asks GitHub for the latest release; when
//! it is newer and carries a signed build, the menu offers "Update to <v> and
//! Restart". Installing downloads the build, checks its minisign signature
//! against the key built into the app, swaps the bundle in place and relaunches.
//! Downloads go through /usr/bin/curl.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{fs, thread};

use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSApplication};
use objc2_foundation::NSString;

use super::{Agent, on_main, open_url, terminate};
use crate::paths::REPO_URL;
use crate::prefs::Prefs;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const FEED: &str = "https://api.github.com/repos/dyascj/slat/releases/latest";
/// Release builds are signed in CI with the matching secret key.
const PUBLIC_KEY: &str = "RWQu7txC0CMjQ+FHVnTOeKtvpVT4v4d1PuDskQZ8hA4benWiLzHfkLSv";
const ASSET: &str = "Slat-macos.app.tar.gz";

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    url: String,
    sig: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    Available(Release),
    Installing(Release),
    /// Installed; relaunching into it.
    Ready(String),
    Failed(Release, String),
}

impl Status {
    /// (`update_available`, `update_status`) for runtime.json.
    pub fn runtime(&self) -> (Option<String>, Option<String>) {
        match self {
            Status::Idle => (None, None),
            Status::Available(r) => (Some(r.version.clone()), None),
            Status::Installing(r) => (Some(r.version.clone()), Some("installing".into())),
            Status::Ready(v) => (Some(v.clone()), Some("ready".into())),
            Status::Failed(r, e) => (Some(r.version.clone()), Some(format!("failed: {e}"))),
        }
    }
}

pub fn releases_url() -> String {
    format!("{REPO_URL}/releases/latest")
}

/// Development builds, `SLAT_NO_UPDATE`, and copies outside an .app never self-update.
pub fn off() -> bool {
    cfg!(debug_assertions) || std::env::var_os("SLAT_NO_UPDATE").is_some() || bundle().is_err()
}

/// Whether release tag `tag` ("v1.2.0") is a later version than `current`.
pub fn newer(tag: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> { s.trim_start_matches('v').split(['.', '-', '+']).map_while(|p| p.parse().ok()).collect() };
    let (a, b) = (parse(tag), parse(current));
    !a.is_empty() && a > b
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Checks at launch when due (a day since the last success, or a newer
/// version was already known), then hourly.
pub fn start() {
    if off() {
        return;
    }
    thread::spawn(|| {
        loop {
            let p = Prefs::load();
            let known = p.updates_latest_version.as_deref().is_some_and(|v| newer(v, CURRENT));
            if known || p.updates_last_checked_at.is_none_or(|t| now().saturating_sub(t) >= 86_400) {
                check_now(false);
            }
            thread::sleep(Duration::from_secs(3600));
        }
    });
}

/// Blocking check that publishes what it finds. `user` reports the outcome
/// in an alert when nothing new turns up.
pub fn check_now(user: bool) {
    let found = check();
    if let Ok(Some(r)) = &found {
        let r = r.clone();
        on_main(move |a| {
            if matches!(a.update, Status::Idle | Status::Available(_) | Status::Failed(..)) {
                a.update = Status::Available(r);
                a.changed();
            }
        });
    } else if user {
        let text = if found.is_ok() { "You're on the latest version." } else { "Couldn't check for updates. Try again later." };
        // Outside the agent borrow: the modal loop keeps serving the main queue.
        DispatchQueue::main().exec_async(move || alert(text));
    }
}

/// Menu "Check for Updates…" and SIGHUP.
pub fn check_in_background() {
    if off() {
        return open_url(releases_url());
    }
    thread::spawn(|| check_now(true));
}

fn alert(text: &str) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    NSApplication::sharedApplication(mtm).activate();
    let a = NSAlert::new(mtm);
    a.setMessageText(&NSString::from_str(text));
    a.runModal();
}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("/usr/bin/curl")
        .args(["-fsSL", "--retry", "2", "-H", &format!("User-Agent: Slat/{CURRENT}")])
        .args(args)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(out.stdout)
}

/// The latest release when it is newer than us and has a signed build.
fn check() -> Result<Option<Release>, String> {
    let body = curl(&["-m", "10", FEED])?;
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().unwrap_or_default();
    let version = tag.trim_start_matches('v').to_string();
    Prefs::update(|p| {
        p.updates_last_checked_at = Some(now());
        p.updates_latest_version = Some(version.clone());
    });
    if !newer(tag, CURRENT) {
        return Ok(None);
    }
    let url_of = |name: &str| {
        v["assets"].as_array()?.iter().find(|x| x["name"] == name)?["browser_download_url"].as_str().map(String::from)
    };
    Ok(match (url_of(ASSET), url_of(&format!("{ASSET}.minisig"))) {
        (Some(url), Some(sig)) => Some(Release { version, url, sig }),
        _ => None,
    })
}

/// `sig` (a .minisig file's text) is a valid signature of `data` by `key`, and
/// its trusted comment is exactly `comment`, so an older signed build can't be
/// passed off as a newer one.
fn verified_with(key: &str, data: &[u8], sig: &str, comment: &str) -> bool {
    let Ok(pk) = minisign_verify::PublicKey::from_base64(key) else { return false };
    let Ok(sig) = minisign_verify::Signature::decode(sig) else { return false };
    pk.verify(data, &sig, false).is_ok() && sig.trusted_comment() == comment
}

fn signed_comment(version: &str) -> String {
    format!("Slat v{version} {ASSET}")
}

/// The running Slat.app.
fn bundle() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let b = exe.ancestors().nth(3).ok_or("not running from an app bundle")?;
    if b.extension().is_none_or(|e| e != "app") {
        return Err("not running from Slat.app".into());
    }
    Ok(b.to_path_buf())
}

fn remove(p: &Path) {
    let _ = if p.is_dir() { fs::remove_dir_all(p) } else { fs::remove_file(p) };
}

fn install(r: &Release) -> Result<(), String> {
    let data = curl(&["-m", "600", &r.url])?;
    let sig = String::from_utf8_lossy(&curl(&["-m", "30", &r.sig])?).into_owned();
    if !verified_with(PUBLIC_KEY, &data, &sig, &signed_comment(&r.version)) {
        return Err("the download's signature doesn't match".into());
    }
    let target = bundle()?;
    let parent = target.parent().ok_or("no install folder")?;
    // Unpack next to the bundle so the swap is a rename on one disk.
    let stage = parent.join(".slat-update");
    remove(&stage);
    fs::create_dir_all(&stage).map_err(|e| format!("{}: {e}", stage.display()))?;
    let archive = stage.join(ASSET);
    fs::write(&archive, &data).map_err(|e| e.to_string())?;
    let ok = Command::new("/usr/bin/tar").arg("-xzf").arg(&archive).arg("-C").arg(&stage).status().is_ok_and(|s| s.success());
    let fresh = stage.join("Slat.app");
    if !ok || !fresh.exists() {
        remove(&stage);
        return Err(if ok { "the download is missing the app" } else { "couldn't unpack the download" }.into());
    }
    // Move the running copy aside (allowed while it runs), put the new one in its place.
    let old = parent.join(".Slat-old.app");
    remove(&old);
    fs::rename(&target, &old).map_err(|e| format!("{}: {e}", target.display()))?;
    if let Err(e) = fs::rename(&fresh, &target) {
        let _ = fs::rename(&old, &target);
        remove(&stage);
        return Err(e.to_string());
    }
    remove(&old);
    remove(&stage);
    Ok(())
}

/// Opens the new bundle once this process has exited, then quits (which stops the engine).
fn relaunch() {
    let Ok(target) = bundle() else { return };
    let t = target.display().to_string().replace('\'', "'\\''");
    let script = format!("while kill -0 {} 2>/dev/null; do sleep 0.2; done; open '{t}'", std::process::id());
    if Command::new("/bin/sh").args(["-c", &script]).spawn().is_ok() {
        terminate();
    }
}

impl Agent {
    /// Menu item and SIGINFO: install the available update (or retry a failed one).
    pub fn install_update(&mut self) {
        let r = match &self.update {
            Status::Available(r) | Status::Failed(r, _) => r.clone(),
            _ => return,
        };
        self.update = Status::Installing(r.clone());
        self.changed();
        thread::spawn(move || {
            let result = install(&r);
            on_main(move |a| {
                match result {
                    Ok(()) => {
                        a.update = Status::Ready(r.version);
                        // Not inside this borrow: quitting runs the agent's shutdown.
                        DispatchQueue::main().exec_async(relaunch);
                    }
                    Err(e) => a.update = Status::Failed(r, e),
                }
                a.changed();
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(newer("v1.0.1", "1.0.0"));
        assert!(newer("v1.10.0", "1.9.9"));
        assert!(newer("2.0.0", "1.99.99"));
        assert!(!newer("v1.0.0", "1.0.0"));
        assert!(!newer("v0.9.0", "1.0.0"));
        assert!(!newer("v1.0.0-beta.2", "1.0.0"));
        assert!(!newer("nightly", "1.0.0"));
    }

    // Throwaway key pair, generated with `minisign -G -W`; the secret was discarded.
    const TEST_KEY: &str = "RWSsyJHMPuwwmyn9dmC2inHz+asnAiUrN54jo+cjbAiQYyxmBwPwcs6f";
    const TEST_SIG: &str = "untrusted comment: signature from minisign secret key
RUSsyJHMPuwwm2tDSHB6wlQ/r0mq39CZjsnezlQ0mmlzEsEFCA+5Gf7EiCtaLdGtSnAefB8tJ2EjQNiNqA3TCbYAzs9GZx8dJQE=
trusted comment: Slat v9.9.9 Slat-macos.app.tar.gz
vTogFoHMKpjaNNuXaKGJihdOFn+mHqs7qHZcrCoZfVR12V1XcN1c39EGbzYdcqNcs27GGf2Nc3BS5dn3v2ZoBw==
";

    #[test]
    fn only_correctly_signed_downloads_install() {
        let comment = signed_comment("9.9.9");
        assert!(verified_with(TEST_KEY, b"hello slat\n", TEST_SIG, &comment));
        assert!(!verified_with(TEST_KEY, b"hello slat!\n", TEST_SIG, &comment), "a changed download fails");
        assert!(!verified_with(TEST_KEY, b"hello slat\n", TEST_SIG, &signed_comment("9.9.8")), "another version's signature fails");
        assert!(!verified_with(PUBLIC_KEY, b"hello slat\n", TEST_SIG, &comment), "another key's signature fails");
        assert!(!verified_with(TEST_KEY, b"hello slat\n", "garbage", &comment));
    }

    #[test]
    fn runtime_status() {
        let r = Release { version: "1.2.0".into(), url: String::new(), sig: String::new() };
        assert_eq!(Status::Idle.runtime(), (None, None));
        assert_eq!(Status::Installing(r.clone()).runtime(), (Some("1.2.0".into()), Some("installing".into())));
        assert_eq!(Status::Failed(r, "x".into()).runtime().1.as_deref(), Some("failed: x"));
    }
}
