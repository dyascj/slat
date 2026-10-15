//! Ash tokens (black glass, grey ramp, pastel pops), SF fonts and the
//! hand-drawn controls every window shares. Always dark.

use std::sync::Arc;

use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, FontData, FontFamily, FontId, Galley, Layout, Margin, Rect, Response, RichText, Sense,
    Stroke, StrokeKind, TextStyle, Ui, UiBuilder, Vec2, WidgetInfo, pos2, vec2,
};
use eframe::epaint::text::{FontTweak, VariationCoords};

pub const REGULAR: f32 = 400.0;
pub const MEDIUM: f32 = 500.0;
pub const SEMIBOLD: f32 = 600.0;
pub const BOLD: f32 = 700.0;
pub const HEAVY: f32 = 860.0;

/// Window glass: black at 85%, blurred by the window server.
pub const GLASS: [f32; 4] = [0.0, 0.0, 0.0, 0.85];

#[derive(Clone, Copy)]
pub struct Pal {
    /// Opaque black, the text colour on inverted (primary) fills.
    pub bg: Color32,
    /// Card fill, barely there over the glass.
    pub raised: Color32,
    pub ink: Color32,
    pub bright: Color32,
    pub muted: Color32,
    pub dim: Color32,
    pub faint: Color32,
    pub line: Color32,
    /// Hover fill.
    pub chip: Color32,
    /// Active / selected fill.
    pub knob: Color32,
    pub elevated: Color32,
    pub focus: Color32,
    pub accent: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

fn white(a: u8) -> Color32 {
    Color32::from_white_alpha(a)
}

pub fn pal() -> Pal {
    Pal {
        bg: Color32::BLACK,
        raised: white(0x05),
        ink: hex(0xe6e6e6),
        bright: Color32::WHITE,
        muted: hex(0x8c8c8c),
        dim: hex(0x5a5a5a),
        faint: hex(0x3a3a3a),
        line: white(0x12),
        chip: white(0x0d),
        knob: white(0x1a),
        elevated: Color32::from_rgba_unmultiplied(0x0e, 0x0e, 0x0e, 0xf2),
        focus: Color32::from_rgba_unmultiplied(0xa6, 0xc8, 0xf4, 0x66),
        accent: hex(0xa6c8f4),
        success: hex(0xa6dcb8),
        warning: hex(0xf2dc9c),
        danger: hex(0xf4a6b4),
    }
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    a.lerp_to_gamma(b, t.clamp(0.0, 1.0))
}

// ---- setup ----

pub fn install(ctx: &egui::Context) {
    let mut defs = egui::FontDefinitions::default();
    let mut add = |name: &str, path: &str, family: FontFamily, tweak: FontTweak| {
        if let Ok(bytes) = std::fs::read(path) {
            defs.font_data.insert(name.into(), Arc::new(FontData::from_owned(bytes).tweak(tweak)));
            defs.families.entry(family).or_default().insert(0, name.into());
        }
    };
    let text_cut = FontTweak { coords: VariationCoords::new([(b"opsz", 17.0)]), ..FontTweak::default() };
    add("sf", "/System/Library/Fonts/SFNS.ttf", FontFamily::Proportional, text_cut);
    // SF Mono's default instance is Light; start from Regular.
    let regular = FontTweak { coords: VariationCoords::new([(b"wght", REGULAR)]), ..FontTweak::default() };
    add("sf-mono", "/System/Library/Fonts/SFNSMono.ttf", FontFamily::Monospace, regular);
    ctx.set_fonts(defs);
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_style_of(egui::Theme::Dark, style());
}

fn style() -> egui::Style {
    let p = pal();
    let mut s = egui::Style::default();
    let mut v = egui::Visuals::dark();
    v.override_text_color = None;
    v.weak_text_color = Some(p.muted);
    v.hyperlink_color = p.accent;
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = p.elevated;
    v.extreme_bg_color = Color32::TRANSPARENT;
    v.faint_bg_color = p.raised;
    v.code_bg_color = p.chip;
    v.window_stroke = Stroke::new(1.0, p.line);
    v.window_corner_radius = 10.into();
    v.menu_corner_radius = 10.into();
    v.window_shadow = egui::Shadow::NONE;
    v.popup_shadow = egui::Shadow::NONE;
    v.selection.bg_fill = hex(0x262626);
    v.selection.stroke = Stroke::new(1.0, p.focus);
    v.text_cursor.stroke = Stroke::new(1.5, p.ink);
    for (w, fill) in [
        (&mut v.widgets.noninteractive, Color32::TRANSPARENT),
        (&mut v.widgets.inactive, p.knob),
        (&mut v.widgets.hovered, white(0x33)),
        (&mut v.widgets.active, white(0x33)),
        (&mut v.widgets.open, p.knob),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = Color32::TRANSPARENT;
        w.bg_stroke = Stroke::new(1.0, p.line);
        w.fg_stroke = Stroke::new(1.0, p.ink);
        w.corner_radius = 6.into();
        w.expansion = 0.0;
    }
    v.widgets.hovered.weak_bg_fill = p.chip;
    v.widgets.open.weak_bg_fill = p.chip;
    s.visuals = v;
    s.spacing.item_spacing = vec2(8.0, 8.0);
    s.spacing.button_padding = vec2(12.0, 6.0);
    s.spacing.interact_size = vec2(40.0, 28.0);
    s.spacing.menu_margin = Margin::same(6);
    s.animation_time = 0.15;
    s.text_styles = [
        (TextStyle::Small, FontId::monospace(11.0)),
        (TextStyle::Body, FontId::monospace(13.0)),
        (TextStyle::Button, FontId::monospace(13.0)),
        (TextStyle::Heading, FontId::monospace(15.0)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ]
    .into();
    s
}

// ---- text ----

/// SF Mono, the voice of the app: labels, values, keycaps, headings.
pub fn text(s: impl Into<String>, size: f32, weight: f32) -> RichText {
    RichText::new(s).family(FontFamily::Monospace).size(size).variation(b"wght", weight)
}

/// SF Pro for longer copy, with Ghostty's roomier line height.
pub fn sans(s: impl Into<String>, size: f32) -> RichText {
    RichText::new(s).family(FontFamily::Proportional).size(size).line_height(Some((size * 1.45).round()))
}

pub fn body(s: impl Into<String>) -> RichText {
    sans(s, 13.0)
}

pub fn caption(s: impl Into<String>) -> RichText {
    sans(s, 12.0).weak()
}

pub fn mono(s: impl Into<String>, size: f32) -> RichText {
    text(s, size, REGULAR)
}

pub fn title(s: impl Into<String>, size: f32) -> RichText {
    text(s, size, SEMIBOLD).color(pal().bright).extra_letter_spacing(-0.01 * size)
}

pub fn eyebrow(s: &str) -> RichText {
    text(s.to_lowercase(), 11.0, MEDIUM).color(pal().dim).extra_letter_spacing(0.3)
}

pub fn galley(ui: &Ui, rt: RichText, wrap: f32) -> Arc<Galley> {
    egui::WidgetText::from(rt).into_galley(ui, Some(egui::TextWrapMode::Wrap), wrap, TextStyle::Body)
}

/// Label that wraps to the available width.
pub fn para(ui: &mut Ui, rt: RichText) -> Response {
    ui.add(egui::Label::new(rt).wrap())
}

// ---- containers ----

/// Transparent panel frame (the glass shows through).
pub fn glass(margin: Margin) -> egui::Frame {
    egui::Frame::new().fill(Color32::TRANSPARENT).inner_margin(margin)
}

pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = pal();
    egui::Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0, p.line))
        .corner_radius(10)
        .inner_margin(Margin::symmetric(16, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui)
        })
        .inner
}

pub fn section(ui: &mut Ui, label: &str) {
    ui.add_space(18.0);
    ui.label(eyebrow(label));
    ui.add_space(2.0);
}

pub fn hairline(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, pal().line));
}

pub fn page_header(ui: &mut Ui, heading: &str, sub: &str) {
    ui.label(title(heading, 15.0));
    ui.add_space(2.0);
    para(ui, caption(sub));
    ui.add_space(4.0);
}

fn badge_galley(ui: &Ui, badge: &str) -> Arc<Galley> {
    galley(ui, text(badge, 10.5, REGULAR).color(pal().muted), f32::INFINITY)
}

fn paint_badge(ui: &Ui, g: Arc<Galley>, left_center: egui::Pos2) {
    let p = pal();
    let r = Rect::from_min_size(pos2(left_center.x, left_center.y - 9.0), vec2(g.size().x + 14.0, 18.0));
    ui.painter().rect_stroke(r, 9, Stroke::new(1.0, p.line), StrokeKind::Inside);
    ui.painter().galley(r.center() - g.size() / 2.0, g, p.muted);
}

/// Title, optional badge and caption on the left; `trailing` right-aligned and centred.
pub fn row(ui: &mut Ui, title: &str, caption_text: Option<&str>, badge: Option<&str>, trailing: impl FnOnce(&mut Ui)) {
    let id = ui.scope_id().with(("row", title));
    let trailing_w: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(120.0);
    let w = ui.available_width();
    let text_w = (w - trailing_w - 24.0).max(140.0);
    let tg = galley(ui, text(title, 13.0, REGULAR), text_w);
    let cg = caption_text.map(|c| galley(ui, caption(c), text_w));
    let text_h = tg.size().y + cg.as_ref().map_or(0.0, |g| g.size().y + 1.0);
    let h = text_h.max(28.0) + if cg.is_some() { 22.0 } else { 16.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let p = pal();
    let mut y = rect.center().y - text_h / 2.0;
    let title_size = tg.size();
    ui.painter().galley(pos2(rect.left(), y), tg, p.ink);
    if let Some(b) = badge {
        paint_badge(ui, badge_galley(ui, b), pos2(rect.left() + title_size.x + 8.0, y + title_size.y / 2.0));
    }
    y += title_size.y + 1.0;
    if let Some(g) = cg {
        ui.painter().galley(pos2(rect.left(), y), g, p.muted);
    }
    let inner = ui.scope_builder(UiBuilder::new().max_rect(rect).layout(Layout::right_to_left(Align::Center)), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        trailing(ui)
    });
    let measured = inner.response.rect.width();
    if (measured - trailing_w).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(id, measured));
        ui.ctx().request_discard("row width");
    }
}

/// Title and caption with the control on its own line below.
pub fn row_below(ui: &mut Ui, title: &str, caption_text: Option<&str>, badge: Option<&str>, below: impl FnOnce(&mut Ui)) {
    ui.add_space(12.0);
    let p = pal();
    let tg = galley(ui, text(title, 13.0, REGULAR), ui.available_width());
    let (rect, _) = ui.allocate_exact_size(tg.size(), Sense::hover());
    let tw = tg.size().x;
    ui.painter().galley(rect.min, tg, p.ink);
    if let Some(b) = badge {
        paint_badge(ui, badge_galley(ui, b), pos2(rect.left() + tw + 8.0, rect.center().y));
    }
    if let Some(c) = caption_text {
        ui.add_space(1.0);
        para(ui, caption(c));
    }
    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        below(ui)
    });
    ui.add_space(12.0);
}

/// Horizontally centred row (width measured on the previous frame).
pub fn centered_row(ui: &mut Ui, salt: &str, add: impl FnOnce(&mut Ui)) {
    let id = ui.scope_id().with(("centered", salt));
    let w: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0);
    let avail = ui.available_width();
    ui.horizontal(|ui| {
        ui.add_space(((avail - w) / 2.0).max(0.0));
        let start = ui.cursor().left();
        add(ui);
        let measured = ui.min_rect().right() - start;
        if (measured - w).abs() > 0.5 {
            ui.data_mut(|d| d.insert_temp(id, measured));
            ui.ctx().request_discard("centered row");
        }
    });
}

// ---- controls ----

fn press_scale(resp: &Response, rect: Rect) -> Rect {
    if resp.is_pointer_button_down_on() { Rect::from_center_size(rect.center(), rect.size() * 0.96) } else { rect }
}

pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(34.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let p = pal();
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.15);
    let off_track = if resp.hovered() { white(0x26) } else { p.knob };
    let painter = ui.painter();
    painter.rect_filled(rect, 10, mix(off_track, p.ink, t));
    let x = egui::lerp(rect.left() + 10.0..=rect.right() - 10.0, t);
    painter.circle_filled(pos2(x, rect.center().y), 7.0, mix(p.muted, p.bg, t));
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::selected(egui::Role::CheckBox, enabled, *on, ""));
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Segmented control with equal-width segments and a sliding selection.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> Response {
    let p = pal();
    let gs: Vec<_> = options.iter().map(|(_, l)| galley(ui, text(*l, 12.0, REGULAR), f32::INFINITY)).collect();
    let seg_w = gs.iter().map(|g| g.size().x).fold(0.0, f32::max) + 22.0;
    let h = 28.0;
    let (rect, mut resp) = ui.allocate_exact_size(vec2(seg_w * options.len() as f32 + 4.0, h), Sense::hover());
    let idx = options.iter().position(|(v, _)| v == value).unwrap_or(0);
    let x = ui.ctx().animate_value_with_time(resp.id.with("x"), idx as f32, 0.15);
    let painter = ui.painter();
    painter.rect_stroke(rect, 8, Stroke::new(1.0, p.line), StrokeKind::Inside);
    let seg = |i: f32| Rect::from_min_size(rect.min + vec2(2.0 + i * seg_w, 2.0), vec2(seg_w, h - 4.0));
    painter.rect_filled(seg(x), 6, p.knob);
    for (i, ((v, _), g)) in options.iter().zip(gs).enumerate() {
        let r = seg(i as f32);
        let sr = ui.interact(r, resp.id.with(i), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        if sr.clicked() && *value != *v {
            *value = *v;
            resp.mark_changed();
        }
        if sr.hovered() && i != idx {
            ui.painter().rect_filled(r, 6, p.chip);
        }
        let color = if i == idx { p.bright } else if sr.hovered() { p.ink } else { p.muted };
        ui.painter().galley(r.center() - g.size() / 2.0, g, color);
    }
    resp
}

/// Pill chip; selected chips get the active fill and a brighter edge.
pub fn chip(ui: &mut Ui, label: &str, selected: bool) -> Response {
    let p = pal();
    let g = galley(ui, text(label, 12.0, REGULAR), f32::INFINITY);
    let size = vec2((g.size().x + 22.0).max(40.0), 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, selected, 0.15);
    let r = press_scale(&resp, rect);
    let idle = if resp.hovered() { p.chip } else { Color32::TRANSPARENT };
    let painter = ui.painter();
    painter.rect_filled(r, 14, mix(idle, p.knob, t));
    painter.rect_stroke(r, 14, Stroke::new(1.0, mix(p.line, white(0x40), t)), StrokeKind::Inside);
    painter.galley(r.center() - g.size() / 2.0, g, mix(p.muted, p.bright, t.max(if resp.hovered() { 0.6 } else { 0.0 })));
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::selected(egui::Role::Button, enabled, selected, label));
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Inverted pill.
    Primary,
    /// Hairline ghost.
    Secondary,
    /// Text only.
    Plain,
}

pub fn button(ui: &mut Ui, label: &str, kind: Kind) -> Response {
    sized_button(ui, label, kind, 30.0)
}

pub fn small_button(ui: &mut Ui, label: &str, kind: Kind) -> Response {
    sized_button(ui, label, kind, 26.0)
}

fn sized_button(ui: &mut Ui, label: &str, kind: Kind, h: f32) -> Response {
    let p = pal();
    let size = if h > 28.0 { 13.0 } else { 12.0 };
    let weight = if kind == Kind::Primary { MEDIUM } else { REGULAR };
    let g = galley(ui, text(label, size, weight), f32::INFINITY);
    let pad = if kind == Kind::Plain { 10.0 } else { h * 0.5 };
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + pad * 2.0, h), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let r = press_scale(&resp, rect);
    let painter = ui.painter();
    let fg = match kind {
        Kind::Primary => {
            painter.rect_filled(r, h / 2.0, mix(p.ink, p.bright, hover));
            p.bg
        }
        Kind::Secondary => {
            painter.rect_filled(r, h / 2.0, p.chip.gamma_multiply(hover));
            painter.rect_stroke(r, h / 2.0, Stroke::new(1.0, mix(p.line, white(0x26), hover)), StrokeKind::Inside);
            p.ink
        }
        Kind::Plain => {
            painter.rect_filled(r, h / 2.0, p.chip.gamma_multiply(hover));
            mix(p.muted, p.ink, hover)
        }
    };
    painter.galley(r.center() - g.size() / 2.0, g, fg);
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::labeled(egui::Role::Button, enabled, label));
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Text link with a trailing arrow; turns accent blue on hover.
pub fn link(ui: &mut Ui, label: &str) -> Response {
    let p = pal();
    let g = galley(ui, text(format!("{label} ↗"), 12.0, REGULAR), f32::INFINITY);
    let (rect, resp) = ui.allocate_exact_size(g.size(), Sense::click());
    let color = if resp.hovered() { p.accent } else { p.muted };
    ui.painter().galley(rect.min, g, color);
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::labeled(egui::Role::Link, enabled, label));
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

pub fn checkbox(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let p = pal();
    let g = galley(ui, text(label, 13.0, REGULAR), f32::INFINITY);
    let (rect, mut resp) = ui.allocate_exact_size(vec2(16.0 + 10.0 + g.size().x, 20.0f32.max(g.size().y)), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let b = Rect::from_center_size(pos2(rect.left() + 8.0, rect.center().y), Vec2::splat(16.0));
    let painter = ui.painter();
    painter.rect_filled(b, 4, mix(if resp.hovered() { p.chip } else { Color32::TRANSPARENT }, p.ink, t));
    painter.rect_stroke(b, 4, Stroke::new(1.0, mix(white(0x33), p.ink, t)), StrokeKind::Inside);
    if t > 0.0 {
        let s = Stroke::new(1.6, p.bg.gamma_multiply(t));
        let c = b.center();
        painter.line(vec![c + vec2(-4.0, 0.2), c + vec2(-1.2, 3.0), c + vec2(4.0, -3.2)], s);
    }
    painter.galley(pos2(b.right() + 10.0, rect.center().y - g.size().y / 2.0), g, p.ink);
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::selected(egui::Role::CheckBox, enabled, *on, label));
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

pub fn slider(ui: &mut Ui, v: &mut f64, lo: f64, hi: f64, width: f32) -> Response {
    let p = pal();
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click_and_drag());
    let inner = rect.shrink2(vec2(8.0, 0.0));
    if let Some(pos) = resp.interact_pointer_pos()
        && (resp.dragged() || resp.clicked())
    {
        let f = ((pos.x - inner.left()) / inner.width()).clamp(0.0, 1.0) as f64;
        let nv = ((lo + f * (hi - lo)) * 100.0).round() / 100.0;
        if nv != *v {
            *v = nv;
            resp.mark_changed();
        }
    }
    let f = ((*v - lo) / (hi - lo)).clamp(0.0, 1.0) as f32;
    let x = inner.left() + f * inner.width();
    let cy = rect.center().y;
    let painter = ui.painter();
    painter.rect_filled(Rect::from_x_y_ranges(inner.x_range(), cy - 1.5..=cy + 1.5), 2, p.knob);
    painter.rect_filled(Rect::from_x_y_ranges(inner.left()..=x, cy - 1.5..=cy + 1.5), 2, p.ink);
    let r = if resp.hovered() || resp.dragged() { 8.0 } else { 7.0 };
    painter.circle_filled(pos2(x, cy), r, p.ink);
    let enabled = ui.is_enabled();
    let value = *v;
    resp.widget_info(|| WidgetInfo::slider(enabled, value, ""));
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

pub fn stepper(ui: &mut Ui, v: &mut i64, lo: i64, hi: i64) -> Response {
    let p = pal();
    let (rect, mut resp) = ui.allocate_exact_size(vec2(100.0, 28.0), Sense::hover());
    ui.painter().rect_stroke(rect, 8, Stroke::new(1.0, p.line), StrokeKind::Inside);
    let minus = Rect::from_min_size(rect.min, vec2(30.0, 28.0));
    let plus = Rect::from_min_size(pos2(rect.right() - 30.0, rect.top()), vec2(30.0, 28.0));
    for (r, label, delta) in [(minus, "−", -1), (plus, "+", 1)] {
        let ok = (lo..=hi).contains(&(*v + delta));
        let br = ui.interact(r, resp.id.with(label), Sense::click());
        if br.clicked() && ok {
            *v += delta;
            resp.mark_changed();
        }
        if br.hovered() && ok {
            ui.painter().rect_filled(r.shrink(2.0), 6, p.chip);
        }
        let color = if ok { p.ink } else { p.faint };
        ui.painter().text(r.center(), Align2::CENTER_CENTER, label, FontId::monospace(14.0), color);
    }
    let g = galley(ui, mono(v.to_string(), 13.0).color(p.bright), f32::INFINITY);
    ui.painter().galley(rect.center() - g.size() / 2.0, g, p.bright);
    resp
}

pub fn keycap(ui: &mut Ui, s: &str) -> Response {
    let p = pal();
    let g = galley(ui, mono(s, 12.0), f32::INFINITY);
    let (rect, resp) = ui.allocate_exact_size(vec2((g.size().x + 12.0).max(22.0), 22.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 5, p.chip);
    painter.rect_stroke(rect, 5, Stroke::new(1.0, p.line), StrokeKind::Inside);
    painter.galley(rect.center() - g.size() / 2.0, g, p.ink);
    resp
}

/// `mod + [shift +] key`, with alternates joined by "or".
pub fn combo(ui: &mut Ui, mods: &[&str], keys: &[&str]) {
    ui.spacing_mut().item_spacing.x = 4.0;
    // Right-to-left callers add the last item first.
    let rtl = ui.layout().prefer_right_to_left();
    let mut parts: Vec<(bool, &str)> = Vec::new();
    for m in mods {
        parts.push((true, m));
        parts.push((false, "+"));
    }
    for (i, k) in keys.iter().enumerate() {
        if i > 0 {
            parts.push((false, "or"));
        }
        parts.push((true, k));
    }
    if rtl {
        parts.reverse();
    }
    for (cap, s) in parts {
        if cap {
            keycap(ui, s);
        } else {
            ui.label(text(s, 11.0, REGULAR).color(pal().dim));
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// Filled green dot: running, OK.
    On,
    /// Hollow ring: stopped, informational.
    Off,
    /// "!" on yellow: worth a look.
    Warn,
    /// "!" on pink: crashed, required.
    Problem,
}

pub fn paint_mark(ui: &Ui, center: egui::Pos2, mark: Mark) {
    let p = pal();
    let painter = ui.painter();
    match mark {
        Mark::On => {
            painter.circle_filled(center, 3.5, p.success);
        }
        Mark::Off => {
            painter.circle_stroke(center, 3.5, Stroke::new(1.2, p.muted));
        }
        Mark::Warn | Mark::Problem => {
            painter.circle_filled(center, 7.0, if mark == Mark::Warn { p.warning } else { p.danger });
            let g = galley(ui, text("!", 10.0, HEAVY), f32::INFINITY);
            painter.galley(center - g.size() / 2.0, g, p.bg);
        }
    }
}

/// Status pill: mark plus label in a hairline capsule.
pub fn pill(ui: &mut Ui, mark: Mark, label: &str) -> Response {
    let p = pal();
    let g = galley(ui, text(label, 12.0, REGULAR), f32::INFINITY);
    let lead = if matches!(mark, Mark::Warn | Mark::Problem) { 20.0 } else { 16.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + lead + 12.0, 24.0), Sense::hover());
    ui.painter().rect_stroke(rect, 12, Stroke::new(1.0, p.line), StrokeKind::Inside);
    paint_mark(ui, pos2(rect.left() + lead / 2.0 + 3.0, rect.center().y), mark);
    ui.painter().galley(pos2(rect.left() + lead + 3.0, rect.center().y - g.size().y / 2.0), g, p.ink);
    resp
}

/// Warning callout tinted with the warning pastel.
pub fn callout(ui: &mut Ui, title_text: &str, detail: &str, action: Option<&str>) -> bool {
    let p = pal();
    let mut clicked = false;
    egui::Frame::new()
        .fill(p.warning.gamma_multiply(0.08))
        .stroke(Stroke::new(1.0, p.warning.gamma_multiply(0.33)))
        .corner_radius(8)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
                paint_mark(ui, r.center(), Mark::Warn);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(text(title_text, 12.5, MEDIUM).color(p.warning));
                    para(ui, caption(detail));
                    if let Some(a) = action {
                        ui.add_space(6.0);
                        clicked = small_button(ui, a, Kind::Secondary).clicked();
                    }
                });
            });
        });
    clicked
}
