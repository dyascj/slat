//! Live Mac Setup state for the settings page and onboarding: re-reads on a
//! timer and on focus; `defaults` runs on a worker thread.

use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{self, Ui, pos2, vec2};

use super::theme::{self, Kind, Mark};
use crate::prefs::Prefs;
use crate::settings::Settings;
use crate::setup::{self, Defaults, Env, Fix, Item, Status};

pub struct Checker {
    pub items: Vec<Item>,
    env: Option<Env>,
    rx: Option<Receiver<Defaults>>,
    next: f64,
    soon: bool,
}

impl Checker {
    pub fn new() -> Checker {
        Checker { items: vec![], env: None, rx: None, next: 0.0, soon: false }
    }

    pub fn loading(&self) -> bool {
        self.env.is_none()
    }

    pub fn attention(&self) -> usize {
        setup::attention(&self.items)
    }

    pub fn recheck(&mut self) {
        self.next = 0.0;
    }

    /// Call every frame; re-reads at most every `interval` seconds.
    pub fn tick(&mut self, ctx: &egui::Context, s: &Settings, interval: f64) {
        let now = ctx.input(|i| i.time);
        if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(true)))) {
            self.next = now;
        }
        if std::mem::take(&mut self.soon) {
            self.next = now + 1.2;
        }
        if let Some(d) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.rx = None;
            let ax = setup::ax_trusted();
            let mut prefs = Prefs::load();
            if ax && !prefs.accessibility_was_granted {
                Prefs::update(|p| p.accessibility_was_granted = true);
                prefs.accessibility_was_granted = true;
            }
            let screens = setup::screens().into_iter().map(|s| s.frame).collect();
            self.env = Some(Env {
                ax_trusted: ax,
                ax_was_granted: prefs.accessibility_was_granted,
                defaults: d,
                screens,
                wms: setup::window_managers(),
                login: crate::login::status(),
                swipe_enabled: s.swipe_enabled,
                swipe_fingers: s.swipe_fingers,
                start_at_login: s.start_at_login,
            });
        }
        if let Some(env) = self.env.as_mut() {
            env.swipe_enabled = s.swipe_enabled;
            env.swipe_fingers = s.swipe_fingers;
            env.start_at_login = s.start_at_login;
            self.items = setup::build_items(env);
        }
        if now >= self.next && self.rx.is_none() {
            let (tx, rx) = channel();
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(setup::read_defaults());
                ctx2.request_repaint();
            });
            self.rx = Some(rx);
            self.next = now + interval;
        }
        ctx.request_repaint_after_secs((self.next - now).max(0.1) as f32);
    }

    pub fn apply(&mut self, fix: &Fix, s: &mut Settings) {
        match fix {
            Fix::Accessibility => {
                setup::ax_prompt();
                super::open(setup::ACCESSIBILITY_URL);
            }
            Fix::Open(url) => {
                super::open(url);
            }
            Fix::Fingers(n) => s.swipe_fingers = *n,
            Fix::Quit(pid, app) => setup::quit(*pid, *app),
            Fix::StartAtLogin => {
                s.start_at_login = true;
                let _ = crate::login::apply(true);
            }
        }
        // Give macOS a moment, then look again.
        self.soon = true;
    }

    /// The list of checks, one row each.
    pub fn rows(&mut self, ui: &mut Ui, s: &mut Settings) {
        let mut chosen: Option<Fix> = None;
        theme::card(ui, |ui| {
            for (i, it) in self.items.iter().enumerate() {
                if i > 0 {
                    theme::hairline(ui);
                }
                ui.add_space(12.0);
                ui.horizontal_top(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::hover());
                    let mark = match it.status {
                        Status::Ok => Mark::On,
                        Status::Info => Mark::Off,
                        Status::Warning => Mark::Warn,
                        Status::Missing => Mark::Problem,
                    };
                    theme::paint_mark(ui, pos2(r.center().x, r.center().y), mark);
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        ui.label(theme::text(&it.title, 13.0, theme::MEDIUM));
                        theme::para(ui, theme::caption(&it.detail));
                        ui.label(theme::text(format!("Affects: {}", it.affects), 11.0, theme::REGULAR).weak());
                        if !it.fixes.is_empty() {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                for (label, fix) in &it.fixes {
                                    if theme::small_button(ui, label, Kind::Secondary).clicked() {
                                        chosen = Some(fix.clone());
                                    }
                                }
                            });
                        }
                    });
                });
                ui.add_space(12.0);
            }
        });
        if let Some(f) = chosen {
            self.apply(&f, s);
        }
    }
}
