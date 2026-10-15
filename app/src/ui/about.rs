//! About content (window and settings page) and the feedback window.

use eframe::egui::{self, Margin, Sense, Ui, vec2};

use super::theme::{self, Kind};
use super::{art, chrome, open, reveal, spawn_window};
use crate::paths::{self, APP_NAME, REPO_URL};
use crate::runtime::{self, Runtime};
use crate::settings::Settings;

pub const PANERU_CREDIT: &str = "Window engine based on Paneru (MIT), Copyright (c) 2025 Karinushka@github";

#[derive(Default)]
pub struct About {
    rt: Option<Runtime>,
    next_poll: f64,
    checked_at: Option<f64>,
}

impl About {
    pub fn ui(&mut self, ui: &mut Ui) {
        let now = ui.input(|i| i.time);
        if now >= self.next_poll {
            self.rt = Runtime::read();
            self.next_poll = now + 1.0;
        }
        ui.ctx().request_repaint_after_secs(1.0);
        let p = theme::pal();
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            let (r, _) = ui.allocate_exact_size(vec2(30.0, 26.0), Sense::hover());
            art::mark(ui.painter(), r, &p);
            ui.add_space(4.0);
            ui.label(theme::text(APP_NAME, 30.0, theme::HEAVY).color(theme::pal().bright).extra_letter_spacing(-1.0));
            ui.label(theme::mono(format!("Version {}", env!("CARGO_PKG_VERSION")), 11.5).weak());
            ui.add_space(6.0);
            ui.label(theme::text("Your windows, neatly in a row.", 14.0, theme::MEDIUM));
            ui.add_space(14.0);

            let update = self.rt.as_ref().and_then(|r| r.update_available.clone());
            let status = self.rt.as_ref().and_then(|r| r.update_status.clone()).unwrap_or_default();
            if let Some(v) = update {
                let installing = status == "installing";
                let label = if installing { "Updating…".to_string() } else { format!("Update to {v} and Restart") };
                ui.add_enabled_ui(!installing, |ui| {
                    if theme::button(ui, &label, Kind::Primary).clicked() {
                        runtime::signal_agent(runtime::SIG_INSTALL_UPDATE);
                    }
                });
                if let Some(reason) = status.strip_prefix("failed:") {
                    theme::para(ui, theme::caption(format!("Update failed: {}", reason.trim())));
                }
            }
            theme::centered_row(ui, "about-buttons", |ui| {
                if theme::small_button(ui, "Check for Updates…", Kind::Secondary).clicked() {
                    if runtime::signal_agent(runtime::SIG_CHECK_UPDATES) {
                        self.checked_at = Some(now);
                    } else {
                        open(&format!("{REPO_URL}/releases/latest"));
                    }
                }
                if theme::small_button(ui, "Send feedback", Kind::Secondary).clicked() {
                    spawn_window("feedback");
                }
            });
            if let Some(t) = self.checked_at
                && self.rt.as_ref().is_some_and(|r| r.update_available.is_none())
            {
                let note = if now - t < 5.0 { "Checking for updates…" } else { "No update found." };
                ui.label(theme::caption(note));
            }
            ui.add_space(12.0);
            theme::centered_row(ui, "about-links", |ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                if theme::link(ui, "github.com/dyascj/slat").clicked() {
                    open(REPO_URL);
                }
                if theme::link(ui, "Made by dyascj").clicked() {
                    open("https://github.com/dyascj");
                }
            });
            ui.add_space(14.0);
            ui.add(egui::Label::new(theme::text(PANERU_CREDIT, 10.5, theme::REGULAR).weak()).wrap());
        });
    }
}

#[derive(Default)]
pub struct AboutWindow {
    about: About,
}

impl eframe::App for AboutWindow {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::GLASS
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        chrome(ui);
        egui::CentralPanel::default().frame(theme::glass(Margin::symmetric(28, 0))).show(
            ui,
            |ui| {
                ui.add_space(44.0);
                self.about.ui(ui);
            },
        );
    }
}

// ---- feedback ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FeedbackKind {
    Bug,
    Idea,
    Other,
}

pub struct Feedback {
    kind: FeedbackKind,
    message: String,
    include: bool,
    show_included: bool,
    diagnostics: Option<String>,
    result: Option<String>,
}

impl Default for Feedback {
    fn default() -> Self {
        Feedback { kind: FeedbackKind::Bug, message: String::new(), include: true, show_included: false, diagnostics: None, result: None }
    }
}

const LOG_LINES: usize = 2000;
const MAX_URL: usize = 7000;

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// Prefilled GitHub issue URL, body trimmed so the URL stays under 7000 chars.
fn issue_url(kind: &str, message: &str, diagnostics: Option<&str>) -> String {
    let one_line = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = one_line.chars().take(60).collect();
    let title = if short.is_empty() { format!("{APP_NAME} {kind}") } else { format!("{APP_NAME} {kind}: {short}") };
    let mut body = message.trim().to_string();
    if let Some(d) = diagnostics {
        body.push_str("\n\n");
        body.push_str(d);
    }
    let mut url = format!("{REPO_URL}/issues/new?title={}&body=", encode(&title));
    for c in body.chars() {
        let e = encode(c.encode_utf8(&mut [0; 4]));
        if url.len() + e.len() >= MAX_URL {
            break;
        }
        url.push_str(&e);
    }
    url
}

/// Copies the last lines of engine.log to a temp file for dragging into the issue.
fn copy_log() -> Option<std::path::PathBuf> {
    let text = std::fs::read_to_string(paths::engine_log()).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    let tail = lines[lines.len().saturating_sub(LOG_LINES)..].join("\n");
    let dir = std::env::temp_dir().join(format!("{APP_NAME}-feedback"));
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{APP_NAME}-engine-log.txt"));
    std::fs::write(&path, tail).ok()?;
    Some(path)
}

impl eframe::App for Feedback {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::GLASS
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        chrome(ui);
        let p = theme::pal();
        egui::CentralPanel::default().frame(theme::glass(Margin::symmetric(28, 0))).show(ui, |ui| {
            ui.add_space(40.0);
            ui.label(theme::title("Send feedback", 22.0));
            ui.add_space(12.0);
            theme::segmented(ui, &mut self.kind, &[(FeedbackKind::Bug, "Bug"), (FeedbackKind::Idea, "Idea"), (FeedbackKind::Other, "Other")]);
            ui.add_space(14.0);
            let prompt = if self.kind == FeedbackKind::Bug { "What happened?" } else { "What would you like?" };
            ui.label(theme::text(prompt, 13.0, theme::MEDIUM));
            ui.add_space(2.0);
            egui::Frame::new()
                .fill(p.raised)
                .stroke(egui::Stroke::new(1.0, p.line))
                .corner_radius(10)
                .inner_margin(Margin::same(10))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                        ui.add_sized(
                            [ui.available_width(), 150.0],
                            egui::TextEdit::multiline(&mut self.message).frame(egui::Frame::NONE).font(egui::TextStyle::Body),
                        );
                    });
                });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                theme::checkbox(ui, &mut self.include, "Include diagnostics and logs");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if self.show_included { "What's included ⌃" } else { "What's included ⌄" };
                    if theme::small_button(ui, label, Kind::Plain).clicked() {
                        self.show_included = !self.show_included;
                    }
                });
            });
            if self.show_included {
                let settings = Settings::load();
                let diag = self.diagnostics.get_or_insert_with(|| crate::diagnostics::text(&settings));
                ui.add_space(6.0);
                egui::Frame::new().fill(p.raised).corner_radius(8).inner_margin(Margin::same(10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                        ui.label(theme::mono(
                            format!("{diag}\n\nPlus the last {LOG_LINES} lines of engine.log, saved to a file you can drag into the issue."),
                            11.0,
                        ));
                    });
                });
            }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                ui.add_space(22.0);
                ui.horizontal(|ui| {
                    if let Some(r) = &self.result {
                        ui.add(egui::Label::new(theme::caption(r)).wrap());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::button(ui, "Open issue", Kind::Primary).clicked() {
                            self.submit();
                        }
                        if theme::button(ui, "Cancel", Kind::Secondary).clicked() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });
            });
        });
    }
}

impl Feedback {
    fn submit(&mut self) {
        let diag = self.include.then(|| self.diagnostics.clone().unwrap_or_else(|| crate::diagnostics::text(&Settings::load())));
        let url = issue_url(&format!("{:?}", self.kind), &self.message, diag.as_deref());
        let log = if self.include { copy_log() } else { None };
        self.result = Some(if !open(&url) {
            "Couldn't open your browser. File an issue at github.com/dyascj/slat.".into()
        } else if let Some(path) = log {
            reveal(&path);
            "Opened a new issue in your browser. Drag the revealed log file into it.".into()
        } else {
            "Opened a new issue in your browser.".into()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_url_is_encoded_and_capped() {
        let url = issue_url("Bug", "Windows  jump\nwhen I swipe & scroll", Some("Diagnostics\nApp: Slat"));
        assert!(url.starts_with("https://github.com/dyascj/slat/issues/new?title=Slat%20Bug%3A%20Windows%20jump%20when%20I%20swipe%20%26%20scroll&body="));
        assert!(url.contains("%0A%0ADiagnostics%0AApp%3A%20Slat"));
        let long = "é".repeat(5000);
        assert!(issue_url("Idea", &long, None).len() < MAX_URL);
        assert!(issue_url("Other", "", None).contains("title=Slat%20Other&body="));
    }
}
