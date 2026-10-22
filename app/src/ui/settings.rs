//! Settings window: sidebar plus one page per section. Every change is saved
//! to settings.json (which regenerates engine.toml) as soon as it happens.

use std::path::PathBuf;

use eframe::egui::{self, Align, Layout, Margin, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::about::About;
use super::checker::Checker;
use super::theme::{self, Kind, Mark, card, hairline, row, row_below, section};
use super::{art, chrome, open, reveal};
use crate::login::{self, LOGIN_ITEMS_URL, LoginStatus};
use crate::paths::{self, APP_NAME, BUNDLE_ID};
use crate::runtime::{self, EngineState, Runtime};
use crate::settings::{
    AppRule, Animation, BORDER_COLORS, Layout as RowLayout, ModKey, Padding, RuleAction, Settings, SwipeDirection, WIDTHS, width_label,
};
use crate::setup;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    General,
    Layout,
    Gestures,
    Shortcuts,
    Workspaces,
    AppRules,
    Setup,
    About,
}

const PAGES: [(Page, &str, &str, &str); 8] = [
    (Page::General, "general", "General", "Startup, engine and focus behaviour."),
    (Page::Layout, "layout", "Layout", "How windows sit in the row."),
    (Page::Gestures, "gestures", "Gestures", "Slide the row with your trackpad or scroll wheel."),
    (Page::Shortcuts, "shortcuts", "Shortcuts", "Keyboard control of the row."),
    (Page::Workspaces, "workspaces", "Workspaces", "Stack separate rows of windows, one above the other."),
    (Page::AppRules, "appRules", "App Rules", "Per-app behaviour for new windows."),
    (Page::Setup, "setup", "Mac Setup", "macOS settings and permissions that can stop a feature."),
    (Page::About, "about", "About", "Version and credits."),
];

const SCROLL_MODIFIERS: [(&str, &str); 6] = [
    ("alt", "⌥ Option"),
    ("cmd", "⌘ Command"),
    ("ctrl", "⌃ Control"),
    ("ctrl + alt", "⌃⌥ Control-Option"),
    ("cmd + alt", "⌘⌥ Command-Option"),
    ("rcmd", "Right ⌘ Command"),
];

/// Hold to resize the window under the pointer by moving it: (engine value, label).
const RESIZE_MODIFIERS: [(&str, &str); 4] =
    [("", "Off"), ("ctrl + cmd", "⌃⌘ Control-Command"), ("alt + shift", "⌥⇧ Option-Shift"), ("cmd + shift", "⌘⇧ Command-Shift")];

/// (group, title, shift, keys, columns only)
const SHORTCUTS: [(&str, &str, bool, &[&str], bool); 20] = [
    ("Focus", "Focus left", false, &["H", "←"], false),
    ("Focus", "Focus right", false, &["L", "→"], false),
    ("Focus", "Focus up", false, &["K", "↑"], false),
    ("Focus", "Focus down", false, &["J", "↓"], false),
    ("Move", "Move left", true, &["H", "←"], false),
    ("Move", "Move right", true, &["L", "→"], false),
    ("Move", "Move up in a stack", true, &["K", "↑"], true),
    ("Move", "Move down in a stack", true, &["J", "↓"], true),
    ("Size", "Cycle width", false, &["R"], true),
    ("Size", "Shrink width", true, &["R"], true),
    ("Size", "Full width", false, &["F"], true),
    ("Size", "Balance widths", false, &["="], true),
    ("Size", "Center window", false, &["C"], true),
    ("Windows", "Float / tile", false, &["T"], false),
    ("Windows", "Show column as tabs", true, &["T"], true),
    ("Windows", "Send to other display", false, &["N"], false),
    ("Stacks", "Stack left", false, &["["], true),
    ("Stacks", "Stack right", false, &["]"], true),
    ("Stacks", "Pop out left", true, &["["], true),
    ("Stacks", "Pop out right", true, &["]"], true),
];

pub struct SettingsApp {
    s: Settings,
    saved: Settings,
    page: Page,
    rt: Option<Runtime>,
    login: LoginStatus,
    login_err: Option<String>,
    next_poll: f64,
    checker: Checker,
    about: About,
    import: Option<PathBuf>,
}

/// An existing Paneru config that can be imported (first existing file wins).
fn importable(s: &Settings) -> Option<PathBuf> {
    if paths::settings_file().exists() && s.use_custom_config_file {
        return None;
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
    [home.join(".paneru"), home.join(".paneru.toml"), xdg.join("paneru/paneru.toml")].into_iter().find(|p| p.is_file())
}

impl SettingsApp {
    pub fn new(section: Option<&str>) -> SettingsApp {
        let s = Settings::load();
        let page = match section {
            Some("app-rules") => Page::AppRules,
            Some("mac-setup") => Page::Setup,
            Some(id) => PAGES.iter().find(|p| p.1 == id).map_or(Page::General, |p| p.0),
            None => Page::General,
        };
        SettingsApp {
            import: importable(&s),
            saved: s.clone(),
            s,
            page,
            rt: None,
            login: login::status(),
            login_err: None,
            next_poll: 0.0,
            checker: Checker::new(),
            about: About::default(),
        }
    }
}

impl eframe::App for SettingsApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::GLASS
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        chrome(ui);
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        if now >= self.next_poll {
            self.rt = Runtime::read();
            self.login = login::status();
            self.next_poll = now + 1.0;
        }
        ctx.request_repaint_after_secs(1.0);
        self.checker.tick(&ctx, &self.s, if self.page == Page::Setup { 4.0 } else { 30.0 });

        let p = theme::pal();
        egui::Panel::left("sidebar")
            .exact_size(196.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(theme::glass(Margin::symmetric(10, 0)))
            .show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default().frame(theme::glass(Margin::ZERO)).show(ui, |ui| {
            let sep = ui.max_rect();
            ui.painter().vline(sep.left(), sep.y_range(), Stroke::new(1.0, p.line));
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                egui::Frame::new().inner_margin(Margin { left: 32, right: 32, top: 40, bottom: 32 }).show(ui, |ui| {
                    ui.set_max_width(ui.available_width().min(640.0));
                    let (_, _, title, sub) = PAGES.iter().find(|x| x.0 == self.page).copied().unwrap_or(PAGES[0]);
                    theme::page_header(ui, title, sub);
                    match self.page {
                        Page::General => self.general(ui),
                        Page::Layout => self.layout(ui),
                        Page::Gestures => self.gestures(ui),
                        Page::Shortcuts => self.shortcuts(ui),
                        Page::Workspaces => self.workspaces(ui),
                        Page::AppRules => self.app_rules(ui),
                        Page::Setup => self.setup(ui),
                        Page::About => {
                            ui.add_space(12.0);
                            card(ui, |ui| {
                                ui.add_space(28.0);
                                self.about.ui(ui);
                                ui.add_space(24.0);
                            });
                        }
                    }
                });
            });
        });

        // Save on change; a slider drag commits when released.
        if self.s != self.saved && ctx.dragged_id().is_none() {
            if let Err(e) = self.s.save() {
                eprintln!("{APP_NAME}: couldn't save settings: {e}");
            }
            self.saved = self.s.clone();
        }
    }
}

impl SettingsApp {
    fn sidebar(&mut self, ui: &mut Ui) {
        let p = theme::pal();
        ui.add_space(50.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 14.0), Sense::hover());
            art::mark(ui.painter(), r, &p);
            ui.label(theme::text(APP_NAME, 15.0, theme::BOLD).extra_letter_spacing(-0.3));
        });
        ui.add_space(14.0);
        ui.spacing_mut().item_spacing.y = 2.0;
        let attention = self.checker.attention();
        for (page, _, title, _) in PAGES {
            let selected = self.page == page;
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let fill = if selected { p.knob } else if resp.hovered() { p.chip } else { egui::Color32::TRANSPARENT };
            ui.painter().rect_filled(rect, 6, fill);
            let g = theme::galley(ui, theme::text(title.to_lowercase(), 13.0, theme::REGULAR), f32::INFINITY);
            let color = if selected { p.bright } else if resp.hovered() { p.ink } else { p.muted };
            ui.painter().galley(pos2(rect.left() + 10.0, rect.center().y - g.size().y / 2.0), g, color);
            if page == Page::Setup && attention > 0 {
                let c = pos2(rect.right() - 16.0, rect.center().y);
                ui.painter().circle_filled(c, 8.0, p.warning);
                let g = theme::galley(ui, theme::mono(attention.to_string(), 10.5), f32::INFINITY);
                ui.painter().galley(c - g.size() / 2.0, g, p.bg);
            }
            if resp.clicked() {
                self.page = page;
                if page == Page::Setup {
                    self.checker.recheck();
                }
            }
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.label(theme::mono(format!("v{}", env!("CARGO_PKG_VERSION")), 11.0).weak());
            });
        });
    }

    fn general(&mut self, ui: &mut Ui) {
        if let Some(path) = self.import.clone() {
            ui.add_space(10.0);
            let mut import = false;
            card(ui, |ui| {
                let body = format!("Import {} and keep editing it by hand.", paths::tilde(&path));
                row(ui, "Existing Paneru config found", Some(&body), None, |ui| {
                    import = theme::small_button(ui, "Import", Kind::Primary).clicked();
                });
            });
            if import {
                self.s.use_custom_config_file = true;
                let _ = self.s.save();
                self.saved = self.s.clone();
                paths::ensure_dirs();
                if let Err(e) = std::fs::copy(&path, paths::engine_config()) {
                    eprintln!("{APP_NAME}: import failed: {e}");
                }
                self.import = None;
            }
        }

        section(ui, "Engine");
        let state = self.rt.as_ref().map_or(EngineState::Stopped, |r| r.engine);
        let (mark, label, headline) = match state {
            EngineState::Running => (Mark::On, "Running", "Windows are being tiled"),
            EngineState::Stopped => (Mark::Off, "Stopped", "Tiling is paused"),
            EngineState::Crashed => (Mark::Problem, "Crashed", "Tiling is paused"),
        };
        let crash = self.rt.as_ref().and_then(|r| r.message.clone()).filter(|_| state == EngineState::Crashed);
        let detail = crash.map_or_else(|| "Tiling engine".to_string(), |m| format!("Tiling engine · {m}"));
        card(ui, |ui| {
            row(ui, headline, Some(&detail), None, |ui| {
                if theme::small_button(ui, "Restart", Kind::Secondary).clicked() && !runtime::signal_agent(runtime::SIG_RESTART) {
                    // No agent running: start it; it brings the engine up.
                    if let Ok(exe) = std::env::current_exe() {
                        let _ = std::process::Command::new(exe).spawn();
                    }
                }
                theme::pill(ui, mark, label);
            });
            hairline(ui);
            let caption = self.login_err.clone().unwrap_or_else(|| {
                if !self.s.start_at_login {
                    return "Off".into();
                }
                match self.login {
                    LoginStatus::Enabled => format!("{APP_NAME} opens when you log in"),
                    LoginStatus::RequiresApproval => "Waiting for approval in System Settings".into(),
                    LoginStatus::NotFound => format!("Available once {APP_NAME} is in Applications"),
                    LoginStatus::NotRegistered => "Off".into(),
                }
            });
            let mut on = self.s.start_at_login;
            row(ui, "Start at login", Some(&caption), None, |ui| {
                if theme::toggle(ui, &mut on).changed() {
                    self.s.start_at_login = on;
                    self.login_err = login::apply(on).err().map(|e| format!("Couldn't update: {e}"));
                    self.login = login::status();
                }
            });
            if self.s.start_at_login && self.login == LoginStatus::RequiresApproval {
                let detail = format!("Allow {APP_NAME} in System Settings › General › Login Items.");
                if theme::callout(ui, "Approval needed", &detail, Some("Open Login Items")) {
                    open(LOGIN_ITEMS_URL);
                }
                ui.add_space(14.0);
            }
        });

        section(ui, "Config");
        card(ui, |ui| {
            row(ui, "Use custom config file", Some("Edit engine.toml by hand"), None, |ui| {
                theme::toggle(ui, &mut self.s.use_custom_config_file);
            });
            if self.s.use_custom_config_file {
                hairline(ui);
                ui.add_space(12.0);
                let path = paths::engine_config();
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(theme::mono(paths::tilde(&path), 11.5).weak()).truncate());
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if theme::small_button(ui, "Reveal", Kind::Secondary).clicked() {
                        reveal(&path);
                    }
                    if theme::small_button(ui, "Open in Editor", Kind::Secondary).clicked() {
                        super::open_with(&["-b", "com.apple.TextEdit", &path.to_string_lossy()]);
                    }
                });
                ui.add_space(10.0);
                theme::para(
                    ui,
                    theme::caption(
                        "While this is on, this window stops writing engine.toml, so your hand edits are kept. The engine reloads the file as soon as you save it.",
                    ),
                );
                ui.add_space(12.0);
            }
        });

        section(ui, "Focus");
        card(ui, |ui| {
            row(ui, "Focus follows mouse", Some("Hovering a window focuses it"), None, |ui| {
                theme::toggle(ui, &mut self.s.focus_follows_mouse);
            });
            hairline(ui);
            row(ui, "Mouse follows focus", Some("The pointer jumps to the focused window"), None, |ui| {
                theme::toggle(ui, &mut self.s.mouse_follows_focus);
            });
            hairline(ui);
            row(ui, "Resize with the pointer", Some("Hold these keys and move the pointer near a window's edge"), None, |ui| {
                let current = RESIZE_MODIFIERS.iter().find(|m| m.0 == self.s.resize_modifier).map_or("Off", |m| m.1);
                egui::ComboBox::from_id_salt("resize-modifier").width(190.0).selected_text(current).show_ui(ui, |ui| {
                    for (value, label) in RESIZE_MODIFIERS {
                        ui.selectable_value(&mut self.s.resize_modifier, value.to_string(), label);
                    }
                });
            });
        });
    }

    fn layout(&mut self, ui: &mut Ui) {
        section(ui, "Layout");
        let current = self.s.layout();
        ui.columns(2, |cols| {
            for (col, (mode, name, about)) in cols.iter_mut().zip([
                (RowLayout::Columns, "Columns", "Windows line up side by side in a row and share the screen neatly."),
                (RowLayout::Canvas, "Canvas", "One long desk. Windows keep their own size; you slide the whole desk."),
            ]) {
                if layout_card(col, mode, name, about, current == mode) {
                    self.s.layout = (mode == RowLayout::Canvas).then_some(RowLayout::Canvas);
                }
            }
        });

        section(ui, "Row preview");
        let p = theme::pal();
        card(ui, |ui| {
            ui.add_space(16.0);
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 116.0), Sense::hover());
            art::preview(ui, rect, &p, &self.s);
            ui.add_space(10.0);
            let summary = format!("New {} · {} · {}", width_label(self.s.default_column_width), self.s.padding.raw(), self.s.animation.raw());
            ui.label(theme::mono(summary, 11.5).weak());
            ui.add_space(14.0);
        });

        let canvas = (current == RowLayout::Canvas).then_some("Not used in Canvas");
        section(ui, "Widths");
        card(ui, |ui| {
            row_below(ui, "New window width", Some("How wide a new window is when it joins the row"), canvas, |ui| {
                for w in WIDTHS {
                    if theme::chip(ui, width_label(w), (self.s.default_column_width - w).abs() < 0.005).clicked() {
                        self.s.default_column_width = w;
                    }
                }
            });
            hairline(ui);
            row_below(ui, "Cycle width presets", Some("Widths that “cycle width” steps through. Pick at least two."), canvas, |ui| {
                for w in WIDTHS {
                    let presets = &mut self.s.preset_column_widths;
                    let pos = presets.iter().position(|x| (x - w).abs() < 0.005);
                    if theme::chip(ui, width_label(w), pos.is_some()).clicked() {
                        match pos {
                            // Two minimum: with one preset, cycle width just snaps to it.
                            Some(i) if presets.len() > 2 => {
                                presets.remove(i);
                            }
                            Some(_) => {}
                            None => {
                                presets.push(w);
                                presets.sort_by(f64::total_cmp);
                            }
                        }
                    }
                }
            });
            hairline(ui);
            row(ui, "Center the focused window", Some("Keep the focused column in the middle of the screen"), None, |ui| {
                theme::toggle(ui, &mut self.s.auto_center);
            });
        });

        section(ui, "Feel");
        card(ui, |ui| {
            row(ui, "Space between windows", Some("Also sets the margin at the screen edges."), None, |ui| {
                let opts: Vec<_> = Padding::ALL.iter().map(|p| (*p, p.label())).collect();
                theme::segmented(ui, &mut self.s.padding, &opts);
            });
            hairline(ui);
            row(ui, "Animation", Some("How the row slides"), None, |ui| {
                let opts: Vec<_> = Animation::ALL.iter().map(|a| (*a, a.label())).collect();
                theme::segmented(ui, &mut self.s.animation, &opts);
            });
            hairline(ui);
            row(ui, "Dim inactive windows", Some("Darken every window except the focused one"), None, |ui| {
                theme::toggle(ui, &mut self.s.dim_inactive_windows);
            });
            if self.s.dim_inactive_windows {
                hairline(ui);
                row(ui, "Dim strength", None, None, |ui| {
                    ui.label(theme::caption("Dark"));
                    theme::slider(ui, &mut self.s.dim_strength, 0.05, 0.4, 180.0);
                    ui.label(theme::caption("Light"));
                });
            }
            hairline(ui);
            row(ui, "Focus border", Some("Outline the focused window"), Some("Beta"), |ui| {
                theme::toggle(ui, &mut self.s.focus_border_enabled);
            });
            if self.s.focus_border_enabled {
                hairline(ui);
                row_below(ui, "Border color", None, None, |ui| {
                    for (hex, name) in BORDER_COLORS {
                        if theme::chip(ui, name, self.s.focus_border_color.eq_ignore_ascii_case(hex)).clicked() {
                            self.s.focus_border_color = hex.to_string();
                        }
                    }
                });
            }
        });
    }

    fn gestures(&mut self, ui: &mut Ui) {
        section(ui, "Trackpad");
        card(ui, |ui| {
            row(ui, "Swipe to slide the row", Some("Move along the row with a sideways swipe"), None, |ui| {
                theme::toggle(ui, &mut self.s.swipe_enabled);
            });
            ui.add_enabled_ui(self.s.swipe_enabled, |ui| {
                hairline(ui);
                row(ui, "Fingers", None, None, |ui| {
                    theme::segmented(ui, &mut self.s.swipe_fingers, &[(3, "3 fingers"), (4, "4 fingers")]);
                });
                if self.s.swipe_fingers == 3 {
                    theme::callout(
                        ui,
                        "3 fingers conflicts with the macOS Spaces swipe",
                        "Use 4 fingers, or turn off “Swipe between full-screen apps” in System Settings › Trackpad › More Gestures.",
                        None,
                    );
                    ui.add_space(14.0);
                }
                hairline(ui);
                row(ui, "Direction", Some("Natural slides the row the way you swipe"), None, |ui| {
                    theme::segmented(
                        ui,
                        &mut self.s.swipe_direction,
                        &[(SwipeDirection::Natural, "Natural"), (SwipeDirection::Reversed, "Reversed")],
                    );
                });
                hairline(ui);
                row(ui, "Sensitivity", None, None, |ui| {
                    ui.add_sized([44.0, 20.0], egui::Label::new(theme::mono(format!("{:.2}×", self.s.swipe_sensitivity), 12.0)));
                    ui.label(theme::caption("Fast"));
                    theme::slider(ui, &mut self.s.swipe_sensitivity, 0.1, 2.0, 180.0);
                    ui.label(theme::caption("Slow"));
                });
                hairline(ui);
                row(ui, "Glide", Some("How far the row keeps sliding after you lift your fingers"), None, |ui| {
                    // Trailing widgets lay out right to left. More glide is to the
                    // right, which is less deceleration.
                    ui.label(theme::caption("Long"));
                    let mut glide = 11.0 - self.s.swipe_deceleration;
                    if theme::slider(ui, &mut glide, 1.0, 10.0, 180.0).changed() {
                        self.s.swipe_deceleration = 11.0 - glide;
                    }
                    ui.label(theme::caption("Short"));
                });
            });
        });

        section(ui, "Edges");
        card(ui, |ui| {
            row(ui, "Stop at the first and last window", Some("The row won't slide past either end into empty desktop"), None, |ui| {
                theme::toggle(ui, &mut self.s.swipe_stop_at_ends);
            });
        });

        section(ui, "Scroll wheel");
        card(ui, |ui| {
            row(ui, "Scroll wheel modifier", Some("Hold this key and turn the scroll wheel to slide the row."), None, |ui| {
                let current = SCROLL_MODIFIERS
                    .iter()
                    .find(|m| m.0 == self.s.scroll_modifier)
                    .map_or(self.s.scroll_modifier.clone(), |m| m.1.to_string());
                egui::ComboBox::from_id_salt("scroll-modifier").width(190.0).selected_text(current).show_ui(ui, |ui| {
                    for (value, label) in SCROLL_MODIFIERS {
                        ui.selectable_value(&mut self.s.scroll_modifier, value.to_string(), label);
                    }
                });
            });
        });
    }

    fn shortcuts(&mut self, ui: &mut Ui) {
        ui.add_space(10.0);
        card(ui, |ui| {
            row_below(ui, "Mod key", Some("All shortcuts start with this key. Pick one that doesn't clash with your apps."), None, |ui| {
                for m in ModKey::ALL {
                    if theme::chip(ui, m.symbol(), self.s.mod_key == m).clicked() {
                        self.s.mod_key = m;
                    }
                }
            });
        });
        let canvas = self.s.layout() == RowLayout::Canvas;
        let next_row = self.s.virtual_workspaces_enabled && !canvas;
        let mod_sym = self.s.mod_key.symbol();
        for group in ["Focus", "Move", "Size", "Windows", "Stacks"] {
            section(ui, group);
            let rows = SHORTCUTS.iter().filter(|s| s.0 == group);
            card(ui, |ui| {
                for (j, (_, title, shift, keys, cols_only)) in rows.into_iter().enumerate() {
                    if j > 0 {
                        hairline(ui);
                    }
                    let title = if *cols_only && canvas { format!("{title} (not used in Canvas)") } else { title.to_string() };
                    // With workspace rows, up and down carry on to the next row.
                    let title = match title.as_str() {
                        "Focus up" if next_row => "Focus up, then the row above".to_string(),
                        "Focus down" if next_row => "Focus down, then the row below".to_string(),
                        _ => title,
                    };
                    shortcut_row(ui, &title, mod_sym, *shift, keys);
                }
            });
        }
        let n = format!("1…{}", self.s.virtual_workspace_count);
        section(ui, if canvas { "Workspaces (not used in Canvas)" } else { "Workspaces" });
        card(ui, |ui| {
            shortcut_row(ui, "Go to workspace", mod_sym, false, &[&n]);
            hairline(ui);
            shortcut_row(ui, "Move to workspace", mod_sym, true, &[&n]);
        });
    }

    fn workspaces(&mut self, ui: &mut Ui) {
        ui.add_space(10.0);
        let p = theme::pal();
        let n = self.s.virtual_workspace_count;
        card(ui, |ui| {
            ui.add_space(18.0);
            ui.horizontal_top(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(150.0, 118.0), Sense::hover());
                let label = theme::galley(ui, theme::mono(format!("Workspace 1 / {n}"), 10.5), f32::INFINITY);
                ui.painter().galley(rect.min, label, p.muted);
                let rows = Rect::from_min_max(pos2(rect.left(), rect.top() + 18.0), rect.max);
                art::workspace_rows(ui.painter(), rows, &p, (n as usize).clamp(1, 3), 0.0);
                ui.add_space(14.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui.label(theme::title("A row per task", 14.0));
                    let m = self.s.mod_key.symbol();
                    theme::para(ui, theme::caption(format!(
                        "Each workspace is its own row of windows, stacked one above the other. Jump between them with {m} 1…{n}, and add ⇧ to take the focused window along. They live inside one macOS Space, so switching is instant."
                    )));
                });
            });
            ui.add_space(18.0);
        });
        ui.add_space(12.0);
        card(ui, |ui| {
            row(ui, "Virtual workspaces", Some("Stack several rows on each display"), None, |ui| {
                theme::toggle(ui, &mut self.s.virtual_workspaces_enabled);
            });
            ui.add_enabled_ui(self.s.virtual_workspaces_enabled, |ui| {
                hairline(ui);
                row(ui, "Number of workspaces", None, None, |ui| {
                    theme::stepper(ui, &mut self.s.virtual_workspace_count, 1, 9);
                });
                hairline(ui);
                row(ui, "Create workspaces automatically", Some("Moving past the last workspace adds a new one"), None, |ui| {
                    theme::toggle(ui, &mut self.s.create_workspaces_automatically);
                });
                hairline(ui);
                row(ui, "Animate switching", Some("Slide between rows instead of switching instantly"), None, |ui| {
                    theme::toggle(ui, &mut self.s.animate_workspace_switch);
                });
            });
        });
    }

    fn app_rules(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(theme::body("New windows of these apps open floating or at a fixed width."));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let button = theme::button(ui, "Add App", Kind::Secondary);
                egui::Popup::menu(&button).width(260.0).show(|ui| {
                    let mut apps: Vec<_> = setup::running_apps()
                        .into_iter()
                        .filter(|a| a.regular && a.bundle_id != BUNDLE_ID && !self.s.app_rules.iter().any(|r| r.bundle_id == a.bundle_id))
                        .collect();
                    apps.sort_by_key(|a| a.name.to_lowercase());
                    apps.dedup_by(|a, b| a.bundle_id == b.bundle_id);
                    if apps.is_empty() {
                        ui.label(theme::caption("No other running apps"));
                    }
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        for app in apps {
                            if menu_item(ui, &app.name) {
                                self.s.app_rules.push(AppRule { bundle_id: app.bundle_id, app_name: app.name, action: RuleAction::Float {} });
                                ui.close();
                            }
                        }
                    });
                });
            });
        });
        ui.add_space(10.0);
        if self.s.app_rules.is_empty() {
            card(ui, |ui| {
                ui.add_space(32.0);
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.label(theme::text("No app rules yet", 14.0, theme::SEMIBOLD));
                    ui.label(theme::caption("Float a calculator, or always open your editor at ⅔ width."));
                });
                ui.add_space(32.0);
            });
            return;
        }
        let mut remove = None;
        card(ui, |ui| {
            for (i, rule) in self.s.app_rules.iter_mut().enumerate() {
                if i > 0 {
                    hairline(ui);
                }
                let mut choice = match rule.action {
                    RuleAction::Float {} => 0,
                    RuleAction::Width { w } if w < 0.5625 => 1,
                    RuleAction::Width { w } if w < 0.875 => 2,
                    RuleAction::Width { .. } => 3,
                };
                row_below(ui, &rule.app_name, Some(&rule.bundle_id), None, |ui| {
                    if theme::segmented(ui, &mut choice, &[(0, "Float"), (1, "Width ½"), (2, "Width ⅔"), (3, "Full width")]).changed() {
                        rule.action = match choice {
                            0 => RuleAction::Float {},
                            1 => RuleAction::Width { w: 0.5 },
                            2 => RuleAction::Width { w: 0.66 },
                            _ => RuleAction::Width { w: 1.0 },
                        };
                    }
                    if theme::small_button(ui, "Remove", Kind::Plain).on_hover_text("Remove rule").clicked() {
                        remove = Some(i);
                    }
                });
            }
        });
        if let Some(i) = remove {
            self.s.app_rules.remove(i);
        }
    }

    fn setup(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        let n = self.checker.attention();
        let headline = if self.checker.loading() {
            "Checking your Mac…".to_string()
        } else {
            match n {
                0 => "Your Mac is ready".into(),
                1 => "One thing to look at".into(),
                n => format!("{n} things to look at"),
            }
        };
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(theme::title(headline, 14.0));
                if setup::required_failure(&self.checker.items) {
                    ui.label(theme::caption(format!("{APP_NAME} can't tile until this is fixed")));
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::small_button(ui, "Check again", Kind::Secondary).clicked() {
                    self.checker.recheck();
                }
            });
        });
        ui.add_space(12.0);
        if !self.checker.loading() && n == 0 {
            card(ui, |ui| {
                ui.add_space(14.0);
                ui.label(theme::eyebrow("Everything checked"));
                ui.add_space(4.0);
                ui.label(theme::text("All good", 14.0, theme::SEMIBOLD));
                ui.add_space(2.0);
                ui.label(theme::caption(format!("{APP_NAME} checks again whenever you come back to it.")));
                ui.add_space(14.0);
            });
            ui.add_space(12.0);
        }
        self.checker.rows(ui, &mut self.s);
    }
}

fn shortcut_row(ui: &mut Ui, title: &str, mod_sym: &str, shift: bool, keys: &[&str]) {
    row(ui, title, None, None, |ui| {
        let mods: Vec<&str> = if shift { vec![mod_sym, "⇧"] } else { vec![mod_sym] };
        theme::combo(ui, &mods, keys);
    });
}

fn menu_item(ui: &mut Ui, label: &str) -> bool {
    let p = theme::pal();
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 6, p.chip);
    }
    let initial = label.chars().next().unwrap_or('?').to_uppercase().to_string();
    let tile = Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), vec2(18.0, 18.0));
    ui.painter().rect_stroke(tile, 5, Stroke::new(1.0, p.line), StrokeKind::Inside);
    let g = theme::galley(ui, theme::text(initial, 10.5, theme::SEMIBOLD), f32::INFINITY);
    ui.painter().galley(tile.center() - g.size() / 2.0, g, p.ink);
    let g = theme::galley(ui, theme::body(label), f32::INFINITY);
    ui.painter().galley(pos2(tile.right() + 8.0, rect.center().y - g.size().y / 2.0), g, p.ink);
    resp.clicked()
}

fn layout_card(ui: &mut Ui, mode: RowLayout, name: &str, about: &str, selected: bool) -> bool {
    let p = theme::pal();
    let w = ui.available_width();
    let about_g = theme::galley(ui, theme::caption(about), w - 28.0);
    let h = 14.0 + 72.0 + 12.0 + 18.0 + about_g.size().y + 14.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let t = ui.ctx().animate_bool_with_time(resp.id, selected, 0.15);
    let painter = ui.painter();
    painter.rect_filled(rect, 14, if resp.hovered() { p.chip } else { p.raised });
    let stroke = Stroke::new(1.0, theme::mix(p.line, p.accent.gamma_multiply(0.6), t));
    painter.rect_stroke(rect, 14, stroke, StrokeKind::Inside);
    let mini = Rect::from_min_size(rect.min + vec2(14.0, 14.0), vec2(w - 28.0, 72.0));
    art::layout_mini(painter, mini, &p, mode, t);
    let tg = theme::galley(ui, theme::text(name, 13.5, theme::SEMIBOLD), f32::INFINITY);
    let ty = mini.bottom() + 12.0;
    painter.galley(pos2(rect.left() + 14.0, ty), tg, p.ink);
    painter.galley(pos2(rect.left() + 14.0, ty + 20.0), about_g, p.muted);
    if t > 0.0 {
        let c = pos2(rect.right() - 20.0, ty + 8.0);
        painter.circle_filled(c, 8.0, p.accent.gamma_multiply(t));
        painter.line(vec![c + vec2(-3.5, 0.2), c + vec2(-1.0, 2.7), c + vec2(3.6, -2.6)], Stroke::new(1.6, p.bg.gamma_multiply(t)));
    }
    resp.clicked()
}
