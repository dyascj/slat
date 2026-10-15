//! Plain-text diagnostics attached to feedback.

use crate::paths::APP_NAME;
use crate::runtime::{EngineState, Runtime};
use crate::settings::{Settings, width_label};

fn sysctl(name: &str) -> String {
    let Ok(cname) = std::ffi::CString::new(name) else { return String::new() };
    let mut buf = [0u8; 256];
    let mut len = buf.len();
    // SAFETY: sysctlbyname writes at most `len` bytes into buf.
    let rc = unsafe { libc::sysctlbyname(cname.as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) };
    if rc != 0 {
        return "unknown".into();
    }
    String::from_utf8_lossy(&buf[..len]).trim_end_matches('\0').to_string()
}

pub fn text(s: &Settings) -> String {
    let displays = crate::setup::screens()
        .iter()
        .map(|d| {
            let [x, y, w, h] = d.frame;
            format!("{w}x{h} @{}x at ({x}, {y}){}", d.scale, if d.main { ", main" } else { "" })
        })
        .collect::<Vec<_>>()
        .join("; ");
    let widths = s.preset_column_widths.iter().map(|w| width_label(*w)).collect::<Vec<_>>().join(", ");
    let swipe = if s.swipe_enabled {
        format!("{} fingers {:?}", s.swipe_fingers, s.swipe_direction)
    } else {
        "off".into()
    };
    let engine = match Runtime::read() {
        Some(r) if r.engine == EngineState::Running => "running".to_string(),
        Some(r) if r.engine == EngineState::Crashed => {
            format!("stopped (crashed: {})", r.message.unwrap_or_default())
        }
        _ => "stopped".into(),
    };
    [
        "Diagnostics".to_string(),
        format!("App: {APP_NAME} {}", env!("CARGO_PKG_VERSION")),
        format!("macOS: {} ({})", sysctl("kern.osproductversion"), sysctl("kern.osversion")),
        format!("Mac: {}, {}", sysctl("hw.model"), sysctl("machdep.cpu.brand_string")),
        format!("Displays: {displays}"),
        format!("Mod key: {} ({})", s.mod_key.symbol(), s.mod_key.raw()),
        format!("Spacing: {} ({} pt)", s.padding.raw(), s.padding.points()),
        format!("New window width: {}", width_label(s.default_column_width)),
        format!("Width presets: {widths}"),
        format!("Swipe: {swipe}"),
        format!("Engine: {engine}"),
    ]
    .join("\n")
}
