//! First-run onboarding: seven pages from welcome to "Start tiling".

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicPtr, AtomicU8, Ordering};

use eframe::egui::{self, Align, Layout, Margin, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::art::{self, Win, smooth};
use super::checker::Checker;
use super::theme::{self, Kind, Mark, card, hairline};
use super::{chrome, open};
use crate::paths::APP_NAME;
use crate::prefs::Prefs;
use crate::runtime::{self, EngineState, Runtime};
use crate::settings::{Animation, ModKey, Padding, Settings, width_label};
use crate::setup;

const PAGES: [(&str, &str); 7] = [
    ("Welcome", "Welcome"),
    ("How it works", "How it works"),
    ("Accessibility", "Accessibility"),
    ("Mac setup check", "Setup check"),
    ("Pick your style", "Your style"),
    ("Learn 5 shortcuts", "Practice"),
    ("Done", "Done"),
];
const PRACTICE: usize = 5;
const DONE: usize = 6;

/// (title, detail, needs shift, keys)
const MOVES: [(&str, &str, bool, &[&str]); 5] = [
    ("Move between windows", "Focus left or right", false, &["←", "→"]),
    ("Move a window", "Carry it one place left or right", true, &["←", "→"]),
    ("Change the width", "Step through ½, ⅔ and full", false, &["R"]),
    ("Fill the screen", "Make the window full width", false, &["F"]),
    ("Bring it to the centre", "Slide the window to the middle", false, &["C"]),
];

pub struct Onboarding {
    page: usize,
    entered: f64,
    s: Settings,
    saved: Settings,
    checker: Checker,
    ax: bool,
    ax_requested: bool,
    ax_next: f64,
    advance_at: Option<f64>,
    tab: usize,
    tab_manual: bool,
    mock: [bool; 2],
    reduce_motion: bool,
    engine_running: bool,
}

impl Onboarding {
    pub fn new() -> Onboarding {
        let s = Settings::load();
        // Dev builds: SLAT_UI_PAGE=<n> opens a page directly (for snapshots).
        let page = if cfg!(debug_assertions) { std::env::var("SLAT_UI_PAGE").ok().and_then(|p| p.parse().ok()).unwrap_or(0) } else { 0 };
        Onboarding {
            page,
            entered: 0.0,
            saved: s.clone(),
            s,
            checker: Checker::new(),
            ax: setup::ax_trusted(),
            ax_requested: false,
            ax_next: 0.0,
            advance_at: None,
            tab: 0,
            tab_manual: false,
            mock: [false, true],
            reduce_motion: super::reduce_motion(),
            engine_running: false,
        }
    }

    fn go(&mut self, page: usize, now: f64) {
        self.page = page.min(DONE);
        self.entered = now;
        self.advance_at = None;
        if self.page == PRACTICE {
            start_key_monitor();
            self.engine_running = Runtime::read().is_some_and(|r| r.engine == EngineState::Running);
        }
        if self.page == 3 {
            self.checker.recheck();
        }
    }

    /// Completes onboarding, tells the agent to start tiling, and closes.
    fn finish(&mut self, ctx: &egui::Context, start: bool) {
        let _ = self.s.save();
        Prefs::update(|p| p.onboarding_completed = true);
        if start {
            let _ = crate::login::apply(self.s.start_at_login);
        }
        runtime::signal_agent(runtime::SIG_START);
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Start time for page fade-ins (never, with reduce motion).
    fn fade(&self) -> f64 {
        if self.reduce_motion { f64::NEG_INFINITY } else { self.entered }
    }

    fn animate(&self, ui: &Ui) -> f64 {
        if self.reduce_motion {
            0.0
        } else {
            ui.ctx().request_repaint();
            ui.input(|i| i.time)
        }
    }
}

impl eframe::App for Onboarding {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::GLASS
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        chrome(ui);
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let p = theme::pal();
        MOD.store(self.s.mod_key as u8, Ordering::Relaxed);
        let _ = CTX.set(ctx.clone());
        if self.page == PRACTICE {
            check_local_keys(&ctx);
        }
        if let Some(t) = self.advance_at
            && now >= t
        {
            self.go(self.page + 1, now);
        }

        egui::Panel::top("top").exact_size(56.0).show_separator_line(false).frame(theme::glass(Margin::ZERO)).show(ui, |ui| {
            let r = ui.max_rect();
            let mark = Rect::from_center_size(pos2(r.left() + 92.0, r.center().y + 2.0), vec2(16.0, 14.0));
            art::mark(ui.painter(), mark, &p);
            let g = theme::galley(ui, theme::text(APP_NAME, 15.0, theme::BOLD).extra_letter_spacing(-0.3), f32::INFINITY);
            ui.painter().galley(pos2(mark.right() + 7.0, mark.center().y - g.size().y / 2.0), g, p.ink);
            // progress
            let mut x = r.right() - 28.0;
            for i in (0..PAGES.len()).rev() {
                let w = if i == self.page { 26.0 } else { 14.0 };
                let w = ctx.animate_value_with_time(ui.scope_id().with(("progress", i)), w, 0.2);
                let seg = Rect::from_min_size(pos2(x - w, r.center().y), vec2(w, 4.0));
                let color = if i <= self.page { p.ink } else { p.line };
                ui.painter().rect_filled(seg, 2, color);
                let resp = ui.interact(seg.expand(6.0), ui.scope_id().with(("seg", i)), Sense::hover());
                resp.on_hover_text(format!("{}, page {} of {}", PAGES[i].0, i + 1, PAGES.len()));
                x -= w + 5.0;
            }
        });

        egui::Panel::bottom("bottom").exact_size(68.0).show_separator_line(false).frame(theme::glass(Margin::symmetric(40, 0))).show(ui, |ui| {
            let r = ui.max_rect();
            ui.painter().hline(r.x_range(), r.top(), Stroke::new(1.0, p.line));
            ui.horizontal_centered(|ui| {
                if self.page < DONE {
                    let later = if self.page == PRACTICE { "Skip for now" } else { "Set up later" };
                    if theme::button(ui, later, Kind::Plain).clicked() {
                        self.finish(&ctx, false);
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let primary = match self.page {
                        0 => "Get started",
                        2 | 3 | PRACTICE => "Continue",
                        DONE => "Start tiling",
                        _ => "Next",
                    };
                    if theme::button(ui, &format!("{primary}  →"), Kind::Primary).clicked() {
                        if self.page == DONE {
                            self.finish(&ctx, true);
                        } else {
                            self.go(self.page + 1, now);
                        }
                    }
                    if self.page > 0 && theme::button(ui, "←  Back", Kind::Secondary).clicked() {
                        self.go(self.page - 1, now);
                    }
                });
            });
        });

        egui::CentralPanel::default().frame(theme::glass(Margin { left: 48, right: 48, top: 16, bottom: 12 })).show(
            ui,
            |ui| {
                let f = self.fade();
                rise(ui, f, 0, |ui| {
                    ui.label(theme::eyebrow(PAGES[self.page].1));
                });
                ui.add_space(2.0);
                match self.page {
                    0 => self.welcome(ui),
                    1 => self.how_it_works(ui),
                    2 => self.accessibility(ui, now),
                    3 => self.setup_check(ui),
                    4 => self.style(ui),
                    PRACTICE => self.practice(ui),
                    _ => self.done(ui),
                }
            },
        );

        if self.s != self.saved && ctx.dragged_id().is_none() {
            let _ = self.s.save();
            self.saved = self.s.clone();
        }
    }
}

/// Fade-in for the i-th block of a page.
fn rise(ui: &mut Ui, start: f64, i: usize, add: impl FnOnce(&mut Ui)) {
    let now = ui.input(|inp| inp.time);
    let t = smooth(((now - start) as f32 - i as f32 * 0.07) / 0.35);
    if t < 1.0 {
        ui.ctx().request_repaint();
    }
    ui.scope(|ui| {
        ui.multiply_opacity(t);
        add(ui)
    });
}

fn heading(ui: &mut Ui, s: &str) {
    ui.add(egui::Label::new(theme::title(s, 20.0)).wrap());
}

fn screen_rect(ui: &mut Ui, h: f32) -> Rect {
    ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover()).0
}

impl Onboarding {
    fn welcome(&mut self, ui: &mut Ui) {
        let f = self.fade();
        let p = theme::pal();
        rise(ui, f, 1, |ui| heading(ui, "Your windows, neatly in a row."));
        ui.add_space(4.0);
        rise(ui, f, 2, |ui| {
            theme::para(ui, theme::body("Every window gets its own column, side by side in a row. Open a new one and it joins the row; the others keep their size and stay in view.").weak());
        });
        ui.add_space(14.0);
        let t = self.animate(ui);
        rise(ui, f, 3, |ui| {
            let r = screen_rect(ui, 168.0);
            art::screen(ui.painter(), r, &p);
            art::sliding_row(ui.painter(), r, &p, t, 2.4, false);
        });
        ui.add_space(14.0);
        rise(ui, f, 4, |ui| {
            ui.horizontal(|ui| {
                for c in ["Every window keeps its size", "Keys, trackpad or wheel", "Rows for workspaces"] {
                    theme::pill(ui, Mark::On, c);
                }
            });
        });
    }

    fn how_it_works(&mut self, ui: &mut Ui) {
        let f = self.fade();
        let p = theme::pal();
        let m = self.s.mod_key.symbol();
        let swipe = if self.s.swipe_enabled { format!("{}-finger swipe", self.s.swipe_fingers) } else { "Swipe".into() };
        let focus_tab = format!("{m} ← →");
        rise(ui, f, 1, |ui| heading(ui, "Scroll, don't squeeze."));
        ui.add_space(10.0);
        let t = self.animate(ui);
        if !self.tab_manual && !self.reduce_motion {
            self.tab = ((t - self.entered).max(0.0) / 4.5) as usize % 4;
        }
        rise(ui, f, 2, |ui| {
            let mut tab = self.tab;
            if theme::segmented(ui, &mut tab, &[(0, "Open any app"), (1, &focus_tab), (2, &swipe), (3, "Workspaces")]).changed() {
                self.tab = tab;
                self.tab_manual = true;
            }
        });
        ui.add_space(12.0);
        rise(ui, f, 3, |ui| {
            let r = screen_rect(ui, 176.0);
            let painter = ui.painter();
            art::screen(painter, r, &p);
            match self.tab {
                0 => art::opening_row(painter, r, &p, t),
                1 => art::sliding_row(painter, r, &p, t, 1.6, true),
                2 => {
                    let wins = [0.5, 0.33, 0.66, 0.5, 0.25, 0.5].map(|w| Win::new(w, 0.0));
                    art::strip(painter, r, &p, &wins, 0.6 + 0.55 * (t * 0.9).sin() as f32, 6.0);
                }
                _ => {
                    let active = 1.0 - (t * 0.7).cos() as f32;
                    art::workspace_rows(painter, r.shrink(10.0), &p, 3, active);
                }
            }
        });
        ui.add_space(10.0);
        let caption = match self.tab {
            0 => "A new window joins the row next to the one you're using. The others keep their size.".to_string(),
            1 => format!("Hold {m} and press ← or → to move focus. The row slides just far enough."),
            2 => "Swipe on the trackpad, or scroll with a modifier held, to slide the row.".to_string(),
            _ => format!("Need more room? Add more rows and jump between them with {m}1–9."),
        };
        rise(ui, f, 4, |ui| {
            theme::para(ui, theme::body(caption).weak());
        });
    }

    fn accessibility(&mut self, ui: &mut Ui, now: f64) {
        let f = self.fade();
        if now >= self.ax_next {
            let was = self.ax;
            self.ax = setup::ax_trusted();
            self.ax_next = now + 1.0;
            if self.ax {
                Prefs::update(|p| p.accessibility_was_granted = true);
                if !was {
                    self.advance_at = Some(now + 0.9);
                }
            }
        }
        ui.ctx().request_repaint_after_secs(1.0);
        let p = theme::pal();
        let full = ui.available_width();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(full - 290.0, ui.available_height()), Layout::top_down(Align::Min), |ui| {
                let title = if self.ax { "You're good to go.".to_string() } else { format!("Let {APP_NAME} arrange your windows.") };
                rise(ui, f, 1, |ui| heading(ui, &title));
                ui.add_space(4.0);
                rise(ui, f, 2, |ui| {
                    theme::para(ui, theme::body(format!("macOS only lets apps move and resize other apps' windows with Accessibility access. {APP_NAME} uses it to arrange your windows in a row, and nothing else.")).weak());
                });
                ui.add_space(12.0);
                rise(ui, f, 3, |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    for reason in ["Moves and resizes windows in the row", "Never reads what's inside your windows", "Nothing leaves your Mac"] {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(10.0, 16.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 2.5, p.ink);
                            ui.label(theme::body(reason));
                        });
                    }
                });
                ui.add_space(16.0);
                rise(ui, f, 4, |ui| {
                    if self.ax {
                        theme::pill(ui, Mark::On, "Accessibility access granted");
                    } else if !self.ax_requested {
                        if theme::button(ui, "Grant Access ↗", Kind::Primary).clicked() {
                            self.request_access();
                        }
                    } else {
                        ui.label(theme::text("Waiting for permission…", 13.0, theme::SEMIBOLD));
                        theme::para(ui, theme::caption(format!("Turn on {APP_NAME} in the list. If it's already there, switch it off and on again.")));
                        ui.add_space(6.0);
                        if theme::small_button(ui, "Open System Settings", Kind::Secondary).clicked() {
                            open(setup::ACCESSIBILITY_URL);
                        }
                    }
                });
            });
            ui.add_space(18.0);
            rise(ui, f, 3, |ui| {
                ui.vertical(|ui| {
                    ui.set_width(270.0);
                    card(ui, |ui| {
                        ui.add_space(12.0);
                        ui.label(theme::text("Privacy & Security › Accessibility", 10.5, theme::MEDIUM).weak());
                        ui.add_space(6.0);
                        ui.label(theme::text("Accessibility", 15.0, theme::BOLD));
                        ui.add_space(2.0);
                        theme::para(ui, theme::caption("Allow these apps to control your computer."));
                        ui.add_space(8.0);
                        let mut slat = self.ax;
                        let [term, ray] = &mut self.mock;
                        let mut clicked = false;
                        for (name, on) in [(APP_NAME, &mut slat), ("Terminal", term), ("Raycast", ray)] {
                            hairline(ui);
                            ui.horizontal(|ui| {
                                ui.set_height(36.0);
                                ui.label(theme::text(name, 12.5, theme::MEDIUM));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if theme::toggle(ui, on).changed() && name == APP_NAME {
                                        clicked = true;
                                    }
                                });
                            });
                        }
                        if clicked && !self.ax {
                            self.request_access();
                        }
                        ui.add_space(4.0);
                    });
                });
            });
        });
    }

    fn request_access(&mut self) {
        setup::ax_prompt();
        open(setup::ACCESSIBILITY_URL);
        self.ax_requested = true;
    }

    fn setup_check(&mut self, ui: &mut Ui) {
        let f = self.fade();
        let ctx = ui.ctx().clone();
        self.checker.tick(&ctx, &self.s, 4.0);
        let n = self.checker.attention();
        let headline = if self.checker.loading() {
            "Checking your Mac…".to_string()
        } else {
            match n {
                0 => "Your Mac is ready.".into(),
                1 => "One thing to look at.".into(),
                n => format!("{n} things to look at."),
            }
        };
        rise(ui, f, 1, |ui| heading(ui, &headline));
        ui.add_space(4.0);
        rise(ui, f, 2, |ui| {
            theme::para(ui, theme::body("A few things can get in the way of the row. Fix what's flagged, or do it later.").weak());
        });
        ui.add_space(10.0);
        let summary = if self.checker.loading() {
            "Checking your Mac".to_string()
        } else {
            let fix = if n == 0 { " · all clear".to_string() } else { format!(" · {n} to fix") };
            format!("{} checks{fix}", self.checker.items.len())
        };
        ui.horizontal(|ui| {
            ui.label(theme::mono(summary, 11.5).weak());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::small_button(ui, "Check again", Kind::Secondary).clicked() {
                    self.checker.recheck();
                }
            });
        });
        ui.add_space(6.0);
        rise(ui, f, 3, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.checker.rows(ui, &mut self.s));
        });
    }

    fn style(&mut self, ui: &mut Ui) {
        let f = self.fade();
        let p = theme::pal();
        rise(ui, f, 1, |ui| heading(ui, "Pick your style."));
        rise(ui, f, 2, |ui| {
            ui.label(theme::body("You can change all of this later in Settings.").weak());
        });
        ui.add_space(12.0);
        let full = ui.available_width();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(full - 270.0, ui.available_height()), Layout::top_down(Align::Min), |ui| {
                rise(ui, f, 3, |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    let label = |ui: &mut Ui, s: &str| {
                        ui.add_space(4.0);
                        ui.label(theme::text(s, 12.0, theme::SEMIBOLD));
                    };
                    label(ui, "New windows");
                    ui.horizontal(|ui| {
                        for (w, l) in [(0.5, "½ width"), (0.66, "⅔ width"), (1.0, "Full width")] {
                            if theme::chip(ui, l, (self.s.default_column_width - w).abs() < 0.005).clicked() {
                                self.s.default_column_width = w;
                            }
                        }
                    });
                    label(ui, "Gaps");
                    ui.horizontal(|ui| {
                        for g in Padding::ALL {
                            if theme::chip(ui, g.label(), self.s.padding == g).clicked() {
                                self.s.padding = g;
                            }
                        }
                    });
                    label(ui, "Animation");
                    ui.horizontal(|ui| {
                        for a in Animation::ALL {
                            if theme::chip(ui, a.label(), self.s.animation == a).clicked() {
                                self.s.animation = a;
                            }
                        }
                    });
                    label(ui, "Shortcut key");
                    ui.horizontal(|ui| {
                        for m in ModKey::ALL {
                            if theme::chip(ui, m.symbol(), self.s.mod_key == m).clicked() {
                                self.s.mod_key = m;
                            }
                        }
                        let note = if self.s.mod_key == ModKey::CtrlAlt { "Recommended" } else { "⌃⌥ recommended" };
                        ui.label(theme::caption(note));
                    });
                    label(ui, "Trackpad");
                    ui.horizontal(|ui| {
                        theme::toggle(ui, &mut self.s.swipe_enabled);
                        let l = if self.s.swipe_enabled { format!("{}-finger swipe", self.s.swipe_fingers) } else { "Swipe off".into() };
                        ui.label(theme::body(l));
                    });
                });
            });
            ui.add_space(14.0);
            rise(ui, f, 4, |ui| {
                ui.vertical(|ui| {
                    let w = ui.available_width();
                    ui.add_space(4.0);
                    let (r, _) = ui.allocate_exact_size(vec2(w, 176.0), Sense::hover());
                    art::preview(ui, r, &p, &self.s);
                    ui.add_space(6.0);
                    let summary = format!("New {} · {} · {}", width_label(self.s.default_column_width), self.s.padding.raw(), self.s.animation.raw());
                    ui.label(theme::mono(summary, 11.0).weak());
                });
            });
        });
    }

    fn practice(&mut self, ui: &mut Ui) {
        let f = self.fade();
        let done = DONE_MASK.load(Ordering::Relaxed);
        let count = done.count_ones() as usize;
        let all = count == MOVES.len();
        let m = self.s.mod_key.symbol();
        rise(ui, f, 1, |ui| heading(ui, if all { "You've got the moves." } else { "Learn five moves." }));
        let body = if all {
            "All five, nicely done.".to_string()
        } else if self.engine_running {
            "Tiling is on. Press each shortcut for real and watch it tick off.".to_string()
        } else {
            "Press each shortcut for real and watch it tick off.".to_string()
        };
        rise(ui, f, 2, |ui| {
            ui.label(theme::body(body).weak());
        });
        ui.add_space(10.0);
        let p = theme::pal();
        rise(ui, f, 3, |ui| {
            card(ui, |ui| {
                for (i, (title, detail, shift, keys)) in MOVES.iter().enumerate() {
                    if i > 0 {
                        hairline(ui);
                    }
                    let ticked = done & (1 << i) != 0;
                    let t = ui.ctx().animate_bool_with_time(ui.scope_id().with(("tick", i)), ticked, 0.2);
                    ui.horizontal(|ui| {
                        ui.set_height(44.0);
                        let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                        let c = r.center();
                        ui.painter().circle_stroke(c, 8.0, Stroke::new(1.2, theme::mix(p.line, p.ink, t)));
                        ui.painter().circle_filled(c, 8.0 * t, p.ink);
                        if t > 0.5 {
                            ui.painter().line(vec![c + vec2(-3.5, 0.2), c + vec2(-1.0, 2.7), c + vec2(3.6, -2.6)], Stroke::new(1.6, p.bg));
                        }
                        ui.add_space(4.0);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.add_space(5.0);
                            ui.label(theme::text(*title, 13.0, theme::MEDIUM));
                            ui.label(theme::caption(*detail));
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for (j, k) in keys.iter().enumerate().rev() {
                                theme::keycap(ui, k);
                                if j > 0 {
                                    ui.label(theme::caption("/"));
                                }
                            }
                            ui.label(theme::caption("· then"));
                            if *shift {
                                theme::keycap(ui, "⇧");
                            }
                            theme::keycap(ui, m);
                            ui.label(theme::caption("Hold"));
                        });
                    });
                }
            });
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(theme::mono(format!("{count} / {}", MOVES.len()), 12.0));
            if !all {
                let t = self.animate(ui);
                let pulse = 0.35 + 0.65 * (0.5 + 0.5 * (t * 3.0).sin() as f32);
                let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.5, p.ink.gamma_multiply(pulse));
                ui.label(theme::caption("Listening"));
                if let Some(next) = (0..MOVES.len()).find(|i| done & (1 << i) == 0) {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if theme::small_button(ui, "Skip", Kind::Plain).clicked() {
                            DONE_MASK.fetch_or(1 << next, Ordering::Relaxed);
                        }
                        ui.label(theme::caption(format!("Try: {}", MOVES[next].0)));
                    });
                }
            }
        });
    }

    fn done(&mut self, ui: &mut Ui) {
        let f = self.fade();
        let m = self.s.mod_key.symbol();
        rise(ui, f, 1, |ui| heading(ui, "Ready when you are."));
        ui.add_space(4.0);
        rise(ui, f, 2, |ui| {
            theme::para(ui, theme::body(format!("Press {m}← and {m}→ to move along the row. Settings live in the menu bar.")).weak());
        });
        ui.add_space(18.0);
        let swipe = if self.s.swipe_enabled { format!("{} fingers", self.s.swipe_fingers) } else { "Off".into() };
        let cells = [
            ("Width", width_label(self.s.default_column_width).to_string()),
            ("Gaps", self.s.padding.label().to_string()),
            ("Swipe", swipe),
            ("Keys", m.to_string()),
        ];
        rise(ui, f, 3, |ui| {
            ui.columns(4, |cols| {
                for (col, (label, value)) in cols.iter_mut().zip(cells) {
                    card(col, |ui| {
                        ui.add_space(14.0);
                        ui.label(theme::eyebrow(label));
                        ui.add_space(6.0);
                        ui.label(theme::title(value, 20.0));
                        ui.add_space(14.0);
                    });
                }
            });
        });
        ui.add_space(22.0);
        rise(ui, f, 4, |ui| {
            if theme::checkbox(ui, &mut self.s.start_at_login, &format!("Open {APP_NAME} when I log in")).changed() {
                let _ = crate::login::apply(self.s.start_at_login);
            }
        });
        ui.add_space(26.0);
        rise(ui, f, 5, |ui| {
            let p = theme::pal();
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
            let t = self.animate(ui);
            art::screen(ui.painter(), r, &p);
            art::sliding_row(ui.painter(), r, &p, t, 3.0, false);
        });
    }
}

// ---- shortcut trainer: listen-only key monitor ----

static DONE_MASK: AtomicU8 = AtomicU8::new(0);
static MOD: AtomicU8 = AtomicU8::new(0);
static CTX: OnceLock<egui::Context> = OnceLock::new();
static TAP: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

const SHIFT: u64 = 0x2_0000;
const CTRL: u64 = 0x4_0000;
const ALT: u64 = 0x8_0000;
const CMD: u64 = 0x10_0000;

/// Which of the five moves a key press is, for the configured mod key.
fn match_move(keycode: i64, flags: u64, mod_key: ModKey) -> Option<usize> {
    let need = match mod_key {
        ModKey::CtrlAlt => CTRL | ALT,
        ModKey::Alt => ALT,
        ModKey::CmdAlt => CMD | ALT,
    };
    let mods = flags & (SHIFT | CTRL | ALT | CMD);
    if mods & !SHIFT != need {
        return None;
    }
    let shift = mods & SHIFT != 0;
    match (keycode, shift) {
        (123 | 124 | 4 | 37, false) => Some(0),
        (123 | 124 | 4 | 37, true) => Some(1),
        (15, false) => Some(2),
        (3, false) => Some(3),
        (8, false) => Some(4),
        _ => None,
    }
}

fn record(keycode: i64, flags: u64) {
    let mod_key = ModKey::ALL.into_iter().find(|m| *m as u8 == MOD.load(Ordering::Relaxed)).unwrap_or(ModKey::CtrlAlt);
    if let Some(i) = match_move(keycode, flags, mod_key) {
        DONE_MASK.fetch_or(1 << i, Ordering::Relaxed);
        if let Some(ctx) = CTX.get() {
            ctx.request_repaint();
        }
    }
}

/// Keys pressed while our own window is focused (the tap may be unavailable).
fn check_local_keys(ctx: &egui::Context) {
    ctx.input(|i| {
        for e in &i.events {
            if let egui::Event::Key { key, pressed: true, modifiers, .. } = e {
                let code = match key {
                    egui::Key::ArrowLeft => 123,
                    egui::Key::ArrowRight => 124,
                    egui::Key::H => 4,
                    egui::Key::L => 37,
                    egui::Key::R => 15,
                    egui::Key::F => 3,
                    egui::Key::C => 8,
                    _ => continue,
                };
                let mut flags = 0;
                for (on, bit) in [(modifiers.shift, SHIFT), (modifiers.ctrl, CTRL), (modifiers.alt, ALT), (modifiers.mac_cmd, CMD)] {
                    if on {
                        flags |= bit;
                    }
                }
                record(code, flags);
            }
        }
    });
}

type TapCallback = extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(tap: u32, place: u32, options: u32, mask: u64, callback: TapCallback, user: *mut c_void) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    fn CGEventGetFlags(event: *mut c_void) -> u64;
    fn CGEventGetIntegerValueField(event: *mut c_void, field: u32) -> i64;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopCommonModes: *const c_void;
    fn CFMachPortCreateRunLoopSource(alloc: *const c_void, port: *mut c_void, order: isize) -> *mut c_void;
    fn CFRunLoopGetMain() -> *mut c_void;
    fn CFRunLoopAddSource(rl: *mut c_void, source: *mut c_void, mode: *const c_void);
}

extern "C" fn on_event(_proxy: *mut c_void, ty: u32, event: *mut c_void, _user: *mut c_void) -> *mut c_void {
    const KEY_DOWN: u32 = 10;
    const KEYCODE_FIELD: u32 = 9;
    if ty == 0xFFFF_FFFE || ty == 0xFFFF_FFFF {
        // Disabled by timeout or user input: switch it back on.
        // SAFETY: TAP holds the live tap created below.
        unsafe { CGEventTapEnable(TAP.load(Ordering::Relaxed), true) };
    } else if ty == KEY_DOWN {
        // SAFETY: event is the key event macOS handed us.
        let (code, flags) = unsafe { (CGEventGetIntegerValueField(event, KEYCODE_FIELD), CGEventGetFlags(event)) };
        record(code, flags);
    }
    event
}

/// HID-level listen-only tap, so it sees shortcuts even when the engine
/// consumes them. Needs Accessibility/Input Monitoring; without it the
/// trainer still hears keys pressed in this window.
fn start_key_monitor() {
    if !TAP.load(Ordering::Relaxed).is_null() {
        return;
    }
    // SAFETY: standard CGEventTap setup on the main run loop.
    unsafe {
        let tap = CGEventTapCreate(0, 0, 1, 1 << 10, on_event, std::ptr::null_mut());
        if tap.is_null() {
            return;
        }
        TAP.store(tap, Ordering::Relaxed);
        let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
        CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopCommonModes);
        CGEventTapEnable(tap, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_match_the_mod_key() {
        assert_eq!(match_move(123, CTRL | ALT | 0x80_0000, ModKey::CtrlAlt), Some(0));
        assert_eq!(match_move(37, CTRL | ALT | SHIFT, ModKey::CtrlAlt), Some(1));
        assert_eq!(match_move(15, ALT, ModKey::Alt), Some(2));
        assert_eq!(match_move(15, ALT, ModKey::CtrlAlt), None);
        assert_eq!(match_move(3, CMD | ALT, ModKey::CmdAlt), Some(3));
        assert_eq!(match_move(8, CMD | ALT | SHIFT, ModKey::CmdAlt), None);
        assert_eq!(match_move(0, ALT, ModKey::Alt), None);
    }
}
