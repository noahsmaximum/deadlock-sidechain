//! Look and feel, styled after Deadlock's own UI: its fonts (Radiance, Reaver, VALVE Oracle, read
//! from the player's Deadlock install, never shipped), warm off-white on off-black, Amber / Sapphire
//! for full / calm, brand green for "on". Plus the small widget kit the pages are built from (cards,
//! toggles, segmented controls, a two-handle range slider, meters).

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};

pub use egui_phosphor::regular as icon;

// ---------------------------------------------------------------------------- palette

pub const BG: Color32 = Color32::from_rgb(13, 15, 12); // Deadlock's offBlack, a touch deeper
pub const RAIL: Color32 = Color32::from_rgb(18, 20, 16);
pub const DEEP: Color32 = Color32::from_rgb(16, 18, 14); // wells: plots, logs, handles
pub const SURFACE: Color32 = Color32::from_rgb(24, 26, 21);
pub const SURFACE_2: Color32 = Color32::from_rgb(34, 36, 30);
pub const SURFACE_3: Color32 = Color32::from_rgb(48, 50, 42);
pub const BORDER: Color32 = Color32::from_rgb(50, 49, 41);
pub const TEXT: Color32 = Color32::from_rgb(255, 239, 215); // offWhite #FFEFD7
pub const TEXT_2: Color32 = Color32::from_rgb(186, 174, 154);
pub const TEXT_3: Color32 = Color32::from_rgb(126, 119, 105);
pub const ACCENT: Color32 = Color32::from_rgb(238, 190, 100); // Amber: full energy
pub const CALM: Color32 = Color32::from_rgb(112, 148, 232); // Sapphire: calm
pub const GOOD: Color32 = Color32::from_rgb(95, 230, 158); // brandGreen #5FE69E
pub const WARN: Color32 = Color32::from_rgb(255, 214, 110);
pub const BAD: Color32 = Color32::from_rgb(238, 89, 69); // warningRed #EE5945
/// Text on an Amber / green fill.
pub const INK: Color32 = Color32::from_rgb(26, 20, 10);

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// The energy gradient: calm Sapphire -> Amber.
pub fn energy_color(t: f32) -> Color32 {
    lerp_color(CALM, ACCENT, t)
}

// ---------------------------------------------------------------------------- fonts / style

const SEMI: &str = "semibold";
const DISPLAY: &str = "display";
const ORACLE: &str = "oracle";

pub fn semi(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMI.into()))
}

/// Deadlock's serif (Reaver): page titles, names, the big numbers.
pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(DISPLAY.into()))
}

/// Deadlock's heading face (VALVE Oracle): section heads, nav, labels. Use upper case.
pub fn oracle(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(ORACLE.into()))
}

/// Deadlock's fonts from the game's own folder (`citadel/panorama/fonts`), so nothing of Valve's
/// is redistributed; without Deadlock, Segoe UI / Georgia from Windows. Phosphor for icons.
fn fonts(ctx: &egui::Context) {
    let mut f = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut f, egui_phosphor::Variant::Regular);
    let game = crate::system::deadlock_game_dir().map(|g| g.join("citadel").join("panorama").join("fonts"));
    let win = std::path::Path::new(r"C:\Windows\Fonts");
    let mut load = |key: &str, files: &[&str]| -> Option<String> {
        for name in files {
            for d in game.iter().map(|d| d.as_path()).chain(std::iter::once(win)) {
                if let Ok(b) = std::fs::read(d.join(name)) {
                    f.font_data.insert(key.into(), egui::FontData::from_owned(b).into());
                    return Some(key.into());
                }
            }
        }
        None
    };
    let body = load("body", &["radiance-regular.otf", "segoeui.ttf"]);
    let body_sb = load("body_sb", &["radiance-semibold.otf", "seguisb.ttf"]);
    let disp = load("display", &["reaver-semibold.otf", "georgiab.ttf"]);
    let orc = load("oracle", &["valveoracle-semibold.ttf", "seguisb.ttf"]);
    // Segoe UI right behind Deadlock's faces, for the symbols they lack (arrows, dashes, ...)
    let segoe = load("segoe", &["segoeui.ttf"]);
    let base = f.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let fam = |first: Option<String>, then: Option<String>| -> Vec<String> {
        let mut v: Vec<String> = first.into_iter().chain(then).chain(segoe.clone()).collect();
        v.dedup();
        v.extend(base.iter().cloned());
        v
    };
    f.families.insert(FontFamily::Proportional, fam(body.clone(), None));
    f.families.insert(FontFamily::Name(SEMI.into()), fam(body_sb.clone(), body.clone()));
    f.families.insert(FontFamily::Name(DISPLAY.into()), fam(disp, body_sb.clone()));
    f.families.insert(FontFamily::Name(ORACLE.into()), fam(orc, body_sb));
    ctx.set_fonts(f);
}

pub fn install(ctx: &egui::Context) {
    fonts(ctx);
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(4);
    v.menu_corner_radius = CornerRadius::same(4);
    v.extreme_bg_color = DEEP;
    v.faint_bg_color = Color32::from_rgb(20, 22, 18);
    v.selection.bg_fill = ACCENT.linear_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    v.slider_trailing_fill = true;
    v.override_text_color = Some(TEXT);
    let r = CornerRadius::same(3);
    for (w, fill, stroke) in [
        (&mut v.widgets.noninteractive, SURFACE, BORDER),
        (&mut v.widgets.inactive, SURFACE_2, SURFACE_2),
        (&mut v.widgets.hovered, SURFACE_3, TEXT.gamma_multiply(0.22)),
        (&mut v.widgets.active, SURFACE_3, ACCENT.linear_multiply(0.7)),
        (&mut v.widgets.open, SURFACE_3, SURFACE_3),
    ] {
        w.corner_radius = r;
        w.weak_bg_fill = fill;
        w.bg_fill = fill;
        w.bg_stroke = Stroke::new(1.0, stroke);
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_2);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        use egui::TextStyle::*;
        s.text_styles = [
            (Heading, display(22.0)),
            (Body, FontId::proportional(14.0)),
            (Button, FontId::proportional(14.0)),
            (Small, FontId::proportional(12.0)),
            (Monospace, FontId::monospace(12.5)),
        ]
        .into();
        s.spacing.item_spacing = vec2(8.0, 8.0);
        s.spacing.button_padding = vec2(10.0, 5.0);
        s.spacing.interact_size = vec2(36.0, 26.0);
        s.spacing.slider_width = 200.0;
        s.spacing.slider_rail_height = 3.0;
        s.spacing.combo_width = 160.0;
        // Deadlock's sliders: a thin rail and an upright notch, not a knob
        s.visuals.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.45 };
        s.interaction.tooltip_delay = 0.35;
    });
}

// ---------------------------------------------------------------------------- text helpers

/// A name in Deadlock's serif.
pub fn title(text: &str, size: f32) -> RichText {
    RichText::new(text).font(display(size)).color(TEXT)
}

/// An upper-case heading in VALVE Oracle, tracked out the way Deadlock sets them.
pub fn heading(text: &str, size: f32) -> RichText {
    RichText::new(text.to_uppercase()).font(oracle(size)).color(TEXT).extra_letter_spacing(size * 0.09)
}

pub fn muted(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(TEXT_2)
}

pub fn caption(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(11.5).color(TEXT_3)
}

/// Small uppercase label above a group of controls.
pub fn eyebrow(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).font(oracle(11.0)).color(TEXT_3).extra_letter_spacing(1.3));
}

// ---------------------------------------------------------------------------- containers

/// A panel: flat, square-cornered, a hairline border with Amber corner brackets.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let r = egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
    brackets(ui.painter(), r.response.rect.shrink(3.0), 8.0, Stroke::new(1.5, ACCENT.gamma_multiply(0.5)));
    r.inner
}

/// Art-deco corner brackets just inside `rect`.
pub fn brackets(p: &egui::Painter, rect: Rect, len: f32, stroke: Stroke) {
    let r = rect.shrink(0.5);
    for (c, dx, dy) in [(r.left_top(), 1.0, 1.0), (r.right_top(), -1.0, 1.0), (r.left_bottom(), 1.0, -1.0), (r.right_bottom(), -1.0, -1.0)] {
        p.line_segment([c, c + vec2(len * dx, 0.0)], stroke);
        p.line_segment([c, c + vec2(0.0, len * dy)], stroke);
    }
}

/// A hairline rule with a small Amber diamond at its start, under a page title.
pub fn rule(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
    let p = ui.painter();
    let y = rect.center().y;
    let c = pos2(rect.left() + 4.5, y);
    p.line_segment([pos2(rect.left() + 12.0, y), pos2(rect.right(), y)], Stroke::new(1.0, BORDER));
    p.line_segment([pos2(rect.left() + 12.0, y), pos2(rect.left() + 140.0, y)], Stroke::new(1.0, ACCENT.gamma_multiply(0.45)));
    let d = 4.0;
    p.add(egui::Shape::convex_polygon(vec![c + vec2(0.0, -d), c + vec2(d, 0.0), c + vec2(0.0, d), c + vec2(-d, 0.0)], ACCENT, Stroke::NONE));
}

/// Fine paper grain over `rect` (Deadlock's menus sit on a paper texture).
pub fn grain(ui: &Ui, rect: Rect, alpha: u8) {
    let ctx = ui.ctx();
    let id = egui::Id::new("sidechain_grain");
    let tex = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)).unwrap_or_else(|| {
        let n = 192usize;
        let mut seed = 0x9e37_79b9u32;
        let mut px = Vec::with_capacity(n * n);
        for _ in 0..n * n {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            px.push(Color32::from_white_alpha(((seed >> 24) as u8) / 22));
        }
        let img = egui::ColorImage { size: [n, n], pixels: px, source_size: vec2(n as f32, n as f32) };
        let opts = egui::TextureOptions { wrap_mode: egui::TextureWrapMode::Repeat, ..egui::TextureOptions::NEAREST };
        let t = ctx.load_texture("grain", img, opts);
        ctx.data_mut(|d| d.insert_temp(id, t.clone()));
        t
    });
    let uv = Rect::from_min_size(Pos2::ZERO, rect.size() / 192.0);
    ui.painter().image(tex.id(), rect, uv, Color32::from_white_alpha(alpha));
}

/// Icon + title (+ an optional one-line explanation) at the top of a card.
pub fn card_head(ui: &mut Ui, glyph: &str, heading: &str, sub: Option<&str>) {
    card_head_with(ui, glyph, heading, sub, |_| {});
}

/// card_head with controls on the right of the title row.
pub fn card_head_with(ui: &mut Ui, glyph: &str, heading: &str, sub: Option<&str>, right: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(glyph).size(17.0).color(ACCENT));
        ui.label(self::heading(heading, 14.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
    });
    if let Some(s) = sub {
        ui.add_space(-3.0);
        ui.label(RichText::new(s).size(12.5).color(TEXT_2));
    }
    ui.add_space(6.0);
}

// ---------------------------------------------------------------------------- controls

/// iOS-style switch.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(36.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let p = ui.painter();
    let bg = lerp_color(SURFACE_3, GOOD.gamma_multiply(0.85), t);
    p.rect_filled(rect, CornerRadius::same(10), if resp.hovered() { bg.gamma_multiply(1.12) } else { bg });
    p.rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0, TEXT.gamma_multiply(0.12)), StrokeKind::Inside);
    let x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
    p.circle_filled(pos2(x, rect.center().y), 7.0, if *on { INK } else { TEXT });
    resp
}

/// A row of mutually exclusive options in one pill.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, opts: &[(T, &str)]) -> bool {
    let mut changed = false;
    egui::Frame::new().fill(SURFACE_2).stroke(Stroke::new(1.0, BORDER)).corner_radius(CornerRadius::same(3)).inner_margin(egui::Margin::same(3)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.horizontal(|ui| {
            for (v, label) in opts {
                let sel = *value == *v;
                let b = egui::Button::new(RichText::new(*label).font(if sel { semi(13.5) } else { FontId::proportional(13.5) }).color(if sel { INK } else { TEXT_2 }))
                    .fill(if sel { ACCENT } else { Color32::TRANSPARENT })
                    .stroke(Stroke::NONE)
                    .corner_radius(CornerRadius::same(2))
                    .min_size(vec2(0.0, 24.0));
                if ui.add(b).clicked() && !sel {
                    *value = *v;
                    changed = true;
                }
            }
        });
    });
    changed
}

/// Frameless icon button.
pub fn icon_button(ui: &mut Ui, glyph: &str, tip: &str) -> Response {
    let b = egui::Button::new(RichText::new(glyph).size(15.0).color(TEXT_2)).fill(Color32::TRANSPARENT).stroke(Stroke::NONE).min_size(vec2(26.0, 26.0));
    ui.add(b).on_hover_text(tip)
}

pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    let b = egui::Button::new(RichText::new(text.to_uppercase()).font(oracle(13.0)).color(INK).extra_letter_spacing(1.0))
        .fill(ACCENT)
        .stroke(Stroke::new(1.0, TEXT.gamma_multiply(0.35)))
        .corner_radius(CornerRadius::same(2))
        .min_size(vec2(0.0, 30.0));
    ui.add(b)
}

pub fn ghost_button(ui: &mut Ui, text: impl Into<egui::WidgetText>) -> Response {
    ui.add(egui::Button::new(text).fill(SURFACE_2).stroke(Stroke::new(1.0, BORDER)).corner_radius(CornerRadius::same(2)).min_size(vec2(0.0, 28.0)))
}

/// Two handles on one 0..1 track (the slice of energy a stem fades in over). `marker` draws the
/// live value. Returns true when a handle moved.
pub fn range_slider(ui: &mut Ui, lo: &mut f32, hi: &mut f32, width: f32, marker: Option<f32>, enabled: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 24.0), if enabled { Sense::click_and_drag() } else { Sense::hover() });
    let track = Rect::from_center_size(rect.center(), vec2(width - 18.0, 6.0));
    let x_of = |v: f32| track.left() + track.width() * v.clamp(0.0, 1.0);
    let v_of = |x: f32| ((x - track.left()) / track.width()).clamp(0.0, 1.0);
    let mut changed = false;
    if enabled {
        let id = resp.id;
        if resp.drag_started() || resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let v = v_of(p.x);
                let which = u8::from((v - *lo).abs() > (v - *hi).abs());
                ui.memory_mut(|m| m.data.insert_temp(id, which));
            }
        }
        if resp.dragged() || resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let v = (v_of(p.x) * 100.0).round() / 100.0;
                if ui.memory(|m| m.data.get_temp::<u8>(id).unwrap_or(0)) == 0 {
                    *lo = v.min(*hi - 0.02);
                } else {
                    *hi = v.max(*lo + 0.02);
                }
                changed = true;
            }
        }
    }
    let p = ui.painter();
    let dim = if enabled { 1.0 } else { 0.35 };
    p.rect_filled(track, CornerRadius::same(1), SURFACE_3);
    // the active slice, coloured by where it sits on the calm -> full gradient
    let n = 24;
    for i in 0..n {
        let a = *lo + (*hi - *lo) * i as f32 / n as f32;
        let b = *lo + (*hi - *lo) * (i + 1) as f32 / n as f32;
        let r = Rect::from_min_max(pos2(x_of(a), track.top()), pos2(x_of(b), track.bottom()));
        p.rect_filled(r, CornerRadius::ZERO, energy_color(a).gamma_multiply(dim));
    }
    if let Some(m) = marker {
        let x = x_of(m);
        p.line_segment([pos2(x, rect.top() + 2.0), pos2(x, rect.bottom() - 2.0)], Stroke::new(2.0, TEXT.gamma_multiply(0.85 * dim)));
    }
    for v in [*lo, *hi] {
        let n = Rect::from_center_size(pos2(x_of(v), track.center().y), vec2(9.0, 20.0));
        p.rect_filled(n, CornerRadius::same(2), DEEP.gamma_multiply(dim.max(0.6)));
        p.rect_stroke(n, CornerRadius::same(2), Stroke::new(2.0, energy_color(v).gamma_multiply(dim)), StrokeKind::Inside);
    }
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    changed
}

/// Horizontal bar meter.
pub fn meter(ui: &mut Ui, v: f32, size: Vec2, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    let r = CornerRadius::same((size.y / 2.0).min(2.0) as u8);
    p.rect_filled(rect, r, SURFACE_3);
    let mut f = rect;
    f.set_width(rect.width() * v.clamp(0.0, 1.0));
    if f.width() > 0.5 {
        p.rect_filled(f, r, color);
    }
    resp
}

/// Bar filled along the calm -> full gradient up to `v`.
pub fn energy_bar(ui: &mut Ui, v: f32, size: Vec2, ticks: &[(f32, &str)]) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    let r = CornerRadius::same(2);
    p.rect_filled(rect, r, SURFACE_3);
    let n = 64;
    let mut fill = Vec::new();
    for i in 0..n {
        let t0 = i as f32 / n as f32;
        if t0 >= v {
            break;
        }
        let t1 = ((i + 1) as f32 / n as f32).min(v);
        fill.push((Rect::from_min_max(pos2(rect.left() + rect.width() * t0, rect.top()), pos2(rect.left() + rect.width() * t1, rect.bottom())), energy_color(t0)));
    }
    // round only the outer ends
    for (i, (fr, c)) in fill.iter().enumerate() {
        let cr = if i == 0 { CornerRadius { nw: r.nw, sw: r.sw, ne: 0, se: 0 } } else { CornerRadius::ZERO };
        p.rect_filled(*fr, cr, *c);
    }
    for (t, label) in ticks {
        let x = rect.left() + rect.width() * t;
        p.line_segment([pos2(x, rect.top() - 3.0), pos2(x, rect.bottom() + 3.0)], Stroke::new(1.0, TEXT.gamma_multiply(0.45)));
        p.text(pos2(x + 4.0, rect.bottom() + 4.0), Align2::LEFT_TOP, label.to_uppercase(), oracle(10.0), TEXT_3);
    }
    p.rect_stroke(rect, r, Stroke::new(1.0, TEXT.gamma_multiply(0.1)), StrokeKind::Inside);
    resp
}

/// Vertical channel strip: `gain` 0..1 bar with `activity` 0..1 sliver beside it.
pub fn vmeter(ui: &mut Ui, gain: f32, activity: f32, size: Vec2, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    let bar = Rect::from_min_max(rect.min, pos2(rect.right() - 7.0, rect.bottom()));
    let act = Rect::from_min_max(pos2(rect.right() - 4.0, rect.top()), rect.max);
    p.rect_filled(bar, CornerRadius::same(2), SURFACE_3);
    p.rect_filled(act, CornerRadius::same(1), SURFACE_3);
    let up = |r: Rect, v: f32| Rect::from_min_max(pos2(r.left(), r.bottom() - r.height() * v.clamp(0.0, 1.0)), r.max);
    if gain > 0.002 {
        p.rect_filled(up(bar, gain), CornerRadius::same(2), color);
    }
    if activity > 0.002 {
        p.rect_filled(up(act, activity), CornerRadius::same(1), TEXT_2);
    }
    resp
}

pub fn dot(ui: &mut Ui, color: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
    // a small diamond, Deadlock's status pip
    let c = r.center();
    ui.painter().add(egui::Shape::convex_polygon(vec![c + vec2(0.0, -4.5), c + vec2(4.5, 0.0), c + vec2(0.0, 4.5), c + vec2(-4.5, 0.0)], color, Stroke::NONE));
}

/// Rounded tag with optional colour.
pub fn chip(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(12.0), color);
    let size = galley.size() + vec2(14.0, 6.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(2), SURFACE_2);
    ui.painter().rect_stroke(rect, CornerRadius::same(2), Stroke::new(1.0, color.gamma_multiply(0.22)), StrokeKind::Inside);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

/// EQ response plot: curves in dB over 20 Hz..20 kHz.
pub fn eq_plot(ui: &mut Ui, freqs: &[f32], curves: &[(Vec<f32>, Stroke)], height: f32) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, height), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(2), DEEP);
    let (db_top, db_bot) = (12.0f32, -30.0f32);
    let x_of = |f: f32| rect.left() + rect.width() * ((f / 20.0).log10() / 3.0);
    let y_of = |db: f32| rect.top() + rect.height() * ((db_top - db.clamp(db_bot, db_top)) / (db_top - db_bot));
    let grid = Stroke::new(1.0, Color32::from_rgb(34, 35, 29));
    for db in [6.0, 0.0, -6.0, -12.0, -18.0, -24.0] {
        let y = y_of(db);
        p.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], if db == 0.0 { Stroke::new(1.0, Color32::from_rgb(64, 62, 52)) } else { grid });
        if db == 0.0 || db == -12.0 || db == -24.0 {
            p.text(Pos2::new(rect.left() + 6.0, y - 1.0), Align2::LEFT_BOTTOM, format!("{db:+.0} dB"), FontId::proportional(10.0), TEXT_3);
        }
    }
    for (f, lbl) in [(50.0, ""), (100.0, "100"), (200.0, ""), (500.0, ""), (1000.0, "1k"), (2000.0, ""), (5000.0, ""), (10000.0, "10k")] {
        let x = x_of(f);
        p.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], grid);
        if !lbl.is_empty() {
            p.text(Pos2::new(x + 4.0, rect.bottom() - 3.0), Align2::LEFT_BOTTOM, lbl, FontId::proportional(10.0), TEXT_3);
        }
    }
    for (dbs, stroke) in curves {
        let pts: Vec<Pos2> = freqs.iter().zip(dbs).map(|(&f, &d)| Pos2::new(x_of(f), y_of(d))).collect();
        p.add(egui::Shape::line(pts, *stroke));
    }
    p.rect_stroke(rect, CornerRadius::same(2), Stroke::new(1.0, BORDER), StrokeKind::Inside);
}

/// Line swatch + label, for plot legends.
pub fn legend(ui: &mut Ui, items: &[(&str, Color32, f32)]) {
    ui.horizontal(|ui| {
        for (name, c, w) in items {
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 10.0), Sense::hover());
            ui.painter().line_segment([r.left_center(), r.right_center()], Stroke::new(*w, *c));
            ui.label(RichText::new(*name).size(11.5).color(TEXT_2));
            ui.add_space(6.0);
        }
    });
}
