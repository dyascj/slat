//! Drawn illustrations: the Slat mark, rows of windows, layout minis.

use eframe::egui::{Color32, Painter, Rect, Stroke, StrokeKind, Ui, pos2, vec2};

use super::theme::{Pal, mix};
use crate::settings::{Layout, Settings};

/// Four thin slats, the third solid and taller.
pub fn mark(painter: &Painter, rect: Rect, p: &Pal) {
    let w = rect.width() / 4.0;
    for i in 0..4 {
        let x = rect.left() + i as f32 * w + w * 0.2;
        let tall = i == 2;
        let h = if tall { rect.height() } else { rect.height() * 0.78 };
        let r = Rect::from_min_size(pos2(x, rect.center().y - h / 2.0), vec2(w * 0.6, h));
        if tall {
            painter.rect_filled(r, 1.5, p.ink);
        } else {
            painter.rect_stroke(r, 1.5, Stroke::new(1.1, p.ink), StrokeKind::Inside);
        }
    }
}

pub fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One window in a row: width as a fraction of the screen, solidity 0..1
/// (1 = focused), height fraction and opacity.
#[derive(Clone, Copy)]
pub struct Win {
    pub w: f32,
    pub solid: f32,
    pub h: f32,
    pub alpha: f32,
}

impl Win {
    pub fn new(w: f32, solid: f32) -> Win {
        Win { w, solid, h: 1.0, alpha: 1.0 }
    }
}

pub fn window(painter: &Painter, r: Rect, p: &Pal, solid: f32, alpha: f32) {
    if r.width() < 1.0 {
        return;
    }
    // Unfocused: faint glass pane. Focused: accent-tinted with an accent edge.
    let fill = mix(Color32::from_white_alpha(0x08), p.accent.gamma_multiply(0.16), solid).gamma_multiply(alpha);
    let edge = mix(Color32::from_white_alpha(0x24), p.accent.gamma_multiply(0.85), solid).gamma_multiply(alpha);
    painter.rect_filled(r, 5, fill);
    painter.rect_stroke(r, 5, Stroke::new(1.0, edge), StrokeKind::Inside);
    if r.width() > 26.0 && r.height() > 20.0 {
        let dot = mix(p.faint, p.accent, solid).gamma_multiply(alpha);
        for i in 0..3 {
            painter.circle_filled(pos2(r.left() + 7.0 + i as f32 * 5.0, r.top() + 7.0), 1.4, dot);
        }
        painter.hline(r.shrink(1.0).x_range(), r.top() + 13.5, Stroke::new(1.0, edge.gamma_multiply(0.5)));
    }
}

/// A screen with a row of windows scrolled by `offset` (fraction of screen width).
pub fn strip(painter: &Painter, rect: Rect, p: &Pal, wins: &[Win], offset: f32, gap: f32) {
    let painter = painter.with_clip_rect(rect.shrink(1.0));
    let inner = rect.shrink(gap.max(1.0) + 6.0);
    let screen_w = inner.width();
    let mut x = inner.left() - offset * screen_w;
    for win in wins {
        let w = win.w * screen_w;
        let h = inner.height() * win.h;
        let r = Rect::from_min_size(pos2(x, inner.center().y - h / 2.0), vec2((w - gap).max(0.0), h));
        if r.right() > rect.left() && r.left() < rect.right() {
            window(&painter, r, p, win.solid, win.alpha);
        }
        x += w;
    }
}

pub fn screen(painter: &Painter, rect: Rect, p: &Pal) {
    painter.rect_filled(rect, 10, p.raised);
    painter.rect_stroke(rect, 10, Stroke::new(1.0, p.line), StrokeKind::Inside);
}

const PATTERN: [f32; 5] = [0.5, 0.33, 0.66, 0.5, 0.25];

fn prefix(i: i64) -> f32 {
    let n = PATTERN.len() as i64;
    let sum: f32 = PATTERN.iter().sum();
    let (q, r) = (i.div_euclid(n), i.rem_euclid(n));
    q as f32 * sum + PATTERN[..r as usize].iter().sum::<f32>()
}

/// Endless row where focus steps along every `period` seconds and the row
/// slides just enough to keep the focused window centred.
pub fn sliding_row(painter: &Painter, rect: Rect, p: &Pal, t: f64, period: f64, back_and_forth: bool) {
    let step_f = t / period;
    let mut step = step_f.floor() as i64;
    let frac = smooth(((step_f.fract()) as f32 - 0.55) / 0.45);
    let mut dir = 1;
    if back_and_forth {
        // 0,1,2,1,0,1,2,...
        let cycle = step.rem_euclid(4);
        (step, dir) = match cycle {
            0 => (0, 1),
            1 => (1, 1),
            2 => (2, -1),
            _ => (1, -1),
        };
    }
    let next = step + dir;
    let center = |i: i64| prefix(i) + PATTERN[i.rem_euclid(5) as usize] / 2.0;
    let focus_x = egui_lerp(center(step), center(next), frac);
    let offset = focus_x - 0.5;
    let first = step.min(next) - 4;
    let wins: Vec<Win> = (first..first + 10)
        .map(|i| {
            let solid = if i == step { 1.0 - frac } else if i == next { frac } else { 0.0 };
            Win::new(PATTERN[i.rem_euclid(5) as usize], solid)
        })
        .collect();
    strip(painter, rect, p, &wins, offset - prefix(first), 6.0);
}

/// A new window opens next to the focused one; the others keep their size.
pub fn opening_row(painter: &Painter, rect: Rect, p: &Pal, t: f64) {
    let ph = (t % 5.0) as f32;
    let grow = smooth((ph - 0.8) / 0.7) * (1.0 - smooth((ph - 4.2) / 0.6));
    let wins = [
        Win::new(0.33, 0.0),
        Win::new(0.5, 1.0 - grow),
        Win { w: 0.5 * grow, solid: grow, h: 1.0, alpha: grow },
        Win::new(0.5, 0.0),
    ];
    let offset = -0.1 + 0.25 * grow;
    strip(painter, rect, p, &wins, offset, 6.0);
}

/// Rows stacked like workspaces; the active row cycles.
pub fn workspace_rows(painter: &Painter, rect: Rect, p: &Pal, rows: usize, active: f32) {
    let rows = rows.max(1);
    let gap = 6.0;
    let h = (rect.height() - gap * (rows as f32 - 1.0)) / rows as f32;
    for i in 0..rows {
        let r = Rect::from_min_size(pos2(rect.left(), rect.top() + i as f32 * (h + gap)), vec2(rect.width(), h));
        let on = (1.0 - (active - i as f32).abs()).clamp(0.0, 1.0);
        painter.rect_stroke(r, 6, Stroke::new(1.0, mix(p.line, Color32::from_white_alpha(0x40), on)), StrokeKind::Inside);
        let widths = [[0.4, 0.3, 0.45], [0.3, 0.5, 0.3], [0.5, 0.35, 0.3]][i % 3];
        let wins: Vec<Win> = widths.iter().enumerate().map(|(j, w)| Win::new(*w, if j == 1 { on } else { 0.0 })).collect();
        strip(painter, r, p, &wins, 0.0, 3.0);
    }
}

/// Columns: equal-height windows in a row. Canvas: loose windows on a long desk.
pub fn layout_mini(painter: &Painter, rect: Rect, p: &Pal, layout: Layout, selected: f32) {
    screen(painter, rect, p);
    let inner = rect.shrink(8.0);
    let fill = |r: Rect, s: f32| window(painter, r, p, s, 1.0);
    match layout {
        Layout::Columns => {
            let ws = [0.3, 0.42, 0.28];
            let mut x = inner.left();
            for (i, w) in ws.iter().enumerate() {
                let r = Rect::from_min_size(pos2(x, inner.top()), vec2(inner.width() * w - 4.0, inner.height()));
                fill(r, if i == 1 { selected } else { 0.0 });
                x += inner.width() * w;
            }
        }
        Layout::Canvas => {
            let boxes = [(0.0, 0.12, 0.34, 0.58), (0.27, 0.3, 0.4, 0.7), (0.6, 0.0, 0.4, 0.55)];
            for (i, (x, y, w, h)) in boxes.iter().enumerate() {
                let r = Rect::from_min_size(
                    pos2(inner.left() + x * inner.width(), inner.top() + y * inner.height()),
                    vec2(w * inner.width(), h * inner.height()),
                );
                fill(r, if i == 1 { selected } else { 0.0 });
            }
        }
    }
}

/// Settings-driven preview: two existing windows, the focused one, and a new one.
pub fn preview(ui: &Ui, rect: Rect, p: &Pal, s: &Settings) {
    let ctx = ui.ctx();
    let id = ui.scope_id().with("preview");
    let anim = |k: &str, v: f32| ctx.animate_value_with_time(id.with(k), v, 0.25);
    let gap = anim("gap", s.padding.points() as f32 * 0.75 + 2.0);
    let new_w = anim("new", s.default_column_width as f32);
    let dim = anim("dim", if s.dim_inactive_windows { 0.45 } else { 1.0 });
    let canvas = anim("canvas", if s.layout() == Layout::Canvas { 1.0 } else { 0.0 });
    let widths = [0.33, 0.5, new_w];
    let focus = 1;
    let center = widths[..focus].iter().sum::<f32>() + widths[focus] / 2.0;
    let offset = anim("off", if s.auto_center { center - 0.5 } else { 0.0 });
    let heights = [0.7, 0.9, 0.62];
    let wins: Vec<Win> = widths
        .iter()
        .enumerate()
        .map(|(i, w)| Win {
            w: *w,
            solid: if i == focus { 1.0 } else { 0.0 },
            h: egui_lerp(1.0, heights[i], canvas),
            alpha: if i == focus { 1.0 } else { dim },
        })
        .collect();
    let painter = ui.painter();
    screen(painter, rect, p);
    strip(painter, rect, p, &wins, offset, gap);
}

fn egui_lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
