//! Window processes: `Slat ui <settings|onboarding|about|feedback> [section]`.
//! One eframe window per process; the process exits when it closes.

mod about;
mod art;
mod checker;
mod onboarding;
mod settings;
mod theme;

use eframe::egui;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSColor, NSWorkspace};
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};

use crate::paths::APP_NAME;

pub fn run(args: &[String]) {
    let kind = args.first().map_or("settings", String::as_str);
    let section = args.get(1).cloned();
    let (title, size, resizable) = match kind {
        "onboarding" => (format!("Welcome to {APP_NAME}"), [720.0, 552.0], false),
        "about" => (format!("About {APP_NAME}"), [400.0, 350.0], false),
        "feedback" => ("Send feedback".to_string(), [500.0, 470.0], false),
        _ => ("Settings".to_string(), [760.0, 540.0], true),
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(&title)
        .with_inner_size(size)
        .with_resizable(resizable)
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false)
        .with_transparent(true)
        .with_maximize_button(resizable);
    if resizable {
        viewport = viewport.with_min_inner_size([680.0, 460.0]);
    }
    let options = eframe::NativeOptions {
        viewport,
        centered: true,
        event_loop_builder: Some(Box::new(|b| {
            b.with_activation_policy(ActivationPolicy::Accessory);
        })),
        ..Default::default()
    };
    let kind = kind.to_string();
    let result = eframe::run_native(
        &title,
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            let app: Box<dyn eframe::App> = match kind.as_str() {
                "onboarding" => Box::new(onboarding::Onboarding::new()),
                "about" => Box::new(about::AboutWindow::default()),
                "feedback" => Box::new(about::Feedback::default()),
                _ => Box::new(settings::SettingsApp::new(section.as_deref())),
            };
            Ok(app)
        }),
    );
    if let Err(e) = result {
        eprintln!("{APP_NAME}: window failed: {e}");
    }
}

/// Per-frame chrome shared by every window: bring to front once, ⌘W / ⌘Q.
fn chrome(ui: &egui::Ui) {
    let ctx = ui.ctx();
    let frame = ctx.cumulative_frame_nr();
    if frame < 3
        && let Some(mtm) = MainThreadMarker::new()
    {
        let app = NSApplication::sharedApplication(mtm);
        if frame == 0 {
            app.activate();
        }
        glass(&app);
    }
    if ctx.input(|i| i.modifiers.command && (i.key_pressed(egui::Key::W) || i.key_pressed(egui::Key::Q))) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
    #[cfg(debug_assertions)]
    snapshot(ctx);
}

/// Dev builds only: `SLAT_UI_SNAPSHOT=out.bmp` renders a few frames, writes
/// the window's own framebuffer composited over a stand-in blurred wallpaper,
/// and exits. Needs no Screen Recording permission (the real compositor blur
/// is not in the framebuffer, so the wallpaper is simulated).
#[cfg(debug_assertions)]
fn snapshot(ctx: &egui::Context) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WRITTEN: AtomicBool = AtomicBool::new(false);
    let Ok(path) = std::env::var("SLAT_UI_SNAPSHOT") else { return };
    let frame = ctx.cumulative_frame_nr();
    if WRITTEN.load(Ordering::Relaxed) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        return;
    }
    ctx.request_repaint();
    if frame != 45 {
        return;
    }
    let ctx2 = ctx.clone();
    ctx.request_screenshot(move |img| {
        let [w, h] = img.size;
        let mut out = Vec::with_capacity(54 + w * h * 4);
        out.extend(b"BM");
        out.extend(((54 + w * h * 4) as u32).to_le_bytes());
        out.extend([0u8; 4]);
        out.extend(54u32.to_le_bytes());
        out.extend(40u32.to_le_bytes());
        out.extend((w as i32).to_le_bytes());
        out.extend((h as i32).to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend([0u8; 24]);
        for y in (0..h).rev() {
            for (x, px) in img.pixels[y * w..(y + 1) * w].iter().enumerate() {
                // soft wallpaper stand-in: diagonal colour bands
                let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
                let bg = [
                    40.0 + 160.0 * (fx * 3.1).sin().abs(),
                    60.0 + 120.0 * ((fx + fy) * 2.3).cos().abs(),
                    90.0 + 150.0 * (fy * 2.7).sin().abs(),
                ];
                let a = 1.0 - px.a() as f32 / 255.0;
                let ch = |c: u8, b: f32| (c as f32 + b * a).min(255.0) as u8;
                out.extend([ch(px.b(), bg[2]), ch(px.g(), bg[1]), ch(px.r(), bg[0]), 255]);
            }
        }
        let _ = std::fs::write(&path, out);
        WRITTEN.store(true, Ordering::Relaxed);
        ctx2.request_repaint();
    });
}

/// Ghostty-style glass: a see-through window whose backdrop the window
/// server blurs (private CGS call, looked up at runtime; skipped if absent).
fn glass(app: &NSApplication) {
    type MainConnection = unsafe extern "C" fn() -> i32;
    type SetBlur = unsafe extern "C" fn(i32, u32, i32) -> i32;
    // SAFETY: dlsym lookups of C functions with the signatures above.
    let (conn, set_blur) = unsafe {
        (
            libc::dlsym(libc::RTLD_DEFAULT, c"CGSMainConnectionID".as_ptr()),
            libc::dlsym(libc::RTLD_DEFAULT, c"CGSSetWindowBackgroundBlurRadius".as_ptr()),
        )
    };
    for window in app.windows().iter() {
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        if !conn.is_null() && !set_blur.is_null() {
            // SAFETY: both symbols exist and match the declared signatures.
            unsafe {
                let conn: MainConnection = std::mem::transmute(conn);
                let set_blur: SetBlur = std::mem::transmute(set_blur);
                set_blur(conn(), window.windowNumber() as u32, 23);
            }
        }
    }
}

pub fn reduce_motion() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

fn open_with(args: &[&str]) -> bool {
    std::process::Command::new("/usr/bin/open").args(args).status().is_ok_and(|s| s.success())
}

pub fn open(url: &str) -> bool {
    open_with(&[url])
}

pub fn reveal(path: &std::path::Path) -> bool {
    open_with(&["-R", &path.to_string_lossy()])
}

/// Spawns another window process of this binary.
pub fn spawn_window(kind: &str) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).args(["ui", kind]).spawn();
    }
}
