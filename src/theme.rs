//! Matte Carbon & Studio Copper design system, typography, and custom DAW widgets.

use crate::icons::{self, UiIcon};
use egui::epaint::Shadow;
use egui::{
    Align, Color32, FontData, FontDefinitions, FontFamily, FontId, Pos2, Rect, Response,
    Rounding, Sense, Stroke, Ui, Vec2, Visuals,
};
use std::ops::RangeInclusive;

// ---------------------------------------------------------------------------
// Palette: Matte Carbon & Studio Copper (zero cool/blue cast)
// ---------------------------------------------------------------------------

pub const BG_WINDOW: Color32 = Color32::from_rgb(20, 20, 21);
pub const BG_HEADER: Color32 = Color32::from_rgb(24, 24, 27);
pub const BG_RACK: Color32 = Color32::from_rgb(17, 17, 19);
pub const BG_STRIP: Color32 = Color32::from_rgb(29, 29, 32);
pub const BG_STRIP_EXCLUDED: Color32 = Color32::from_rgb(23, 23, 25);
pub const BG_MASTER_STRIP: Color32 = Color32::from_rgb(33, 30, 28);
pub const BG_WELL: Color32 = Color32::from_rgb(13, 13, 15);
pub const BG_CONTROL: Color32 = Color32::from_rgb(38, 38, 43);
pub const BG_CONTROL_HOVER: Color32 = Color32::from_rgb(52, 52, 59);
pub const BG_MODAL: Color32 = Color32::from_rgb(26, 26, 30);

pub const BORDER_SUBTLE: Color32 = Color32::from_rgb(44, 44, 50);
pub const BORDER_STRONG: Color32 = Color32::from_rgb(66, 66, 74);

pub const COPPER: Color32 = Color32::from_rgb(249, 115, 22);
pub const COPPER_DARK: Color32 = Color32::from_rgb(194, 65, 12);
pub const COPPER_GLOW: Color32 = Color32::from_rgb(253, 186, 116);

pub const EMERALD: Color32 = Color32::from_rgb(16, 185, 129);
pub const AMBER: Color32 = Color32::from_rgb(245, 158, 11);
pub const ROSE: Color32 = Color32::from_rgb(239, 68, 68);

pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(244, 244, 245);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(161, 161, 170);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(113, 113, 122);

/// Linearly blends `accent` over `base` by `alpha/255`, returning a 100% opaque `Color32::from_rgb`.
/// Pre-compositing avoids `epaint` semi-transparent stroke/fill feathering overlap halos.
pub const fn tint_opaque(base: Color32, accent: Color32, alpha: u8) -> Color32 {
    let a = alpha as u16;
    let inv = 255 - a;
    let r = ((base.r() as u16 * inv + accent.r() as u16 * a + 127) / 255) as u8;
    let g = ((base.g() as u16 * inv + accent.g() as u16 * a + 127) / 255) as u8;
    let b = ((base.b() as u16 * inv + accent.b() as u16 * a + 127) / 255) as u8;
    Color32::from_rgb(r, g, b)
}

/// Draws a rounded box using concentric `rect_filled` passes instead of `PathStroke::outside`.
/// This prevents `epaint` double-composited border feathering and corner outline halos.
pub fn paint_crisp_box(
    painter: &egui::Painter,
    rect: Rect,
    radius: f32,
    fill: Color32,
    border: Option<Color32>,
) {
    match border {
        Some(border_col) => {
            painter.rect_filled(rect, Rounding::same(radius), border_col);
            let inner = rect.shrink(1.0);
            if inner.is_positive() {
                painter.rect_filled(inner, Rounding::same((radius - 1.0).max(0.0)), fill);
            }
        }
        None => {
            painter.rect_filled(rect, Rounding::same(radius), fill);
        }
    }
}

// ---------------------------------------------------------------------------
// Typography
// ---------------------------------------------------------------------------

pub fn font_regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn font_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("ui-bold".into()))
}

pub fn font_mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

pub fn font_mono_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("mono-bold".into()))
}

fn load_first_font(paths: &[&str]) -> Option<Vec<u8>> {
    for &p in paths {
        if let Ok(bytes) = std::fs::read(p) {
            if !bytes.is_empty() {
                return Some(bytes);
            }
        }
    }
    None
}

/// Installs crisp static system UI and monospace Regular/Bold font faces with egui fallbacks.
pub fn install_studio_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    let default_prop = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let default_mono = fonts
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();

    let ui_reg_bytes = load_first_font(&[
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf",
    ]);
    let ui_bold_bytes = load_first_font(&[
        "/usr/share/fonts/noto/NotoSans-Bold.ttf",
        "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/liberation/LiberationSans-Bold.ttf",
    ]);
    let mono_reg_bytes = load_first_font(&[
        "/usr/share/fonts/Adwaita/AdwaitaMono-Regular.ttf",
        "/usr/share/fonts/noto/NotoSansMono-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        "/usr/share/fonts/liberation/LiberationMono-Regular.ttf",
    ]);
    let mono_bold_bytes = load_first_font(&[
        "/usr/share/fonts/Adwaita/AdwaitaMono-Bold.ttf",
        "/usr/share/fonts/noto/NotoSansMono-Bold.ttf",
        "/usr/share/fonts/TTF/DejaVuSansMono-Bold.ttf",
        "/usr/share/fonts/liberation/LiberationMono-Bold.ttf",
    ]);

    if let Some(bytes) = ui_reg_bytes {
        fonts
            .font_data
            .insert("studio-ui-regular".into(), FontData::from_owned(bytes));
        let mut chain = vec!["studio-ui-regular".to_string()];
        chain.extend(default_prop.clone());
        fonts.families.insert(FontFamily::Proportional, chain);
    }

    let mut bold_chain = Vec::new();
    if let Some(bytes) = ui_bold_bytes {
        fonts
            .font_data
            .insert("studio-ui-bold".into(), FontData::from_owned(bytes));
        bold_chain.push("studio-ui-bold".to_string());
    }
    if fonts.font_data.contains_key("studio-ui-regular") {
        bold_chain.push("studio-ui-regular".to_string());
    }
    bold_chain.extend(default_prop);
    fonts
        .families
        .insert(FontFamily::Name("ui-bold".into()), bold_chain);

    if let Some(bytes) = mono_reg_bytes {
        fonts
            .font_data
            .insert("studio-mono-regular".into(), FontData::from_owned(bytes));
        let mut chain = vec!["studio-mono-regular".to_string()];
        chain.extend(default_mono.clone());
        fonts.families.insert(FontFamily::Monospace, chain);
    }

    let mut mono_bold_chain = Vec::new();
    if let Some(bytes) = mono_bold_bytes {
        fonts
            .font_data
            .insert("studio-mono-bold".into(), FontData::from_owned(bytes));
        mono_bold_chain.push("studio-mono-bold".to_string());
    }
    if fonts.font_data.contains_key("studio-mono-regular") {
        mono_bold_chain.push("studio-mono-regular".to_string());
    }
    mono_bold_chain.extend(default_mono);
    fonts
        .families
        .insert(FontFamily::Name("mono-bold".into()), mono_bold_chain);

    ctx.set_fonts(fonts);
}

/// Applies global Matte Carbon & Studio Copper spacing, rounding, and selection visuals.
pub fn apply_studio_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 7.0);
    style.spacing.button_padding = Vec2::new(11.0, 6.0);
    style.spacing.scroll.bar_width = 7.0;
    style.spacing.scroll.bar_inner_margin = 3.0;
    style.spacing.scroll.bar_outer_margin = 2.0;
    style.spacing.scroll.floating = false;

    let mut visuals = Visuals::dark();
    visuals.window_fill = BG_WINDOW;
    visuals.panel_fill = BG_WINDOW;
    visuals.faint_bg_color = BG_WELL;
    visuals.extreme_bg_color = BG_WELL;
    visuals.code_bg_color = BG_WELL;
    visuals.window_rounding = Rounding::same(14.0);
    visuals.menu_rounding = Rounding::same(10.0);
    visuals.window_stroke = Stroke::new(1.0_f32, BORDER_SUBTLE);
    visuals.window_shadow = Shadow::NONE;
    visuals.popup_shadow = Shadow::NONE;

    visuals.selection.bg_fill = tint_opaque(BG_WELL, COPPER, 75);
    visuals.selection.stroke = Stroke::new(1.0_f32, COPPER);

    visuals.widgets.noninteractive.bg_fill = BG_STRIP;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER_SUBTLE);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.noninteractive.rounding = Rounding::same(8.0);

    visuals.widgets.inactive.bg_fill = BG_CONTROL;
    visuals.widgets.inactive.weak_bg_fill = BG_CONTROL;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER_SUBTLE);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.inactive.rounding = Rounding::same(8.0);

    visuals.widgets.hovered.bg_fill = BG_CONTROL_HOVER;
    visuals.widgets.hovered.weak_bg_fill = BG_CONTROL_HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, BORDER_STRONG);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.2_f32, TEXT_PRIMARY);
    visuals.widgets.hovered.rounding = Rounding::same(8.0);

    visuals.widgets.active.bg_fill = Color32::from_rgb(60, 60, 68);
    visuals.widgets.active.weak_bg_fill = Color32::from_rgb(60, 60, 68);
    visuals.widgets.active.bg_stroke = Stroke::new(1.2_f32, COPPER);
    visuals.widgets.active.fg_stroke = Stroke::new(1.5_f32, TEXT_PRIMARY);
    visuals.widgets.active.rounding = Rounding::same(8.0);

    style.visuals = visuals;
    ctx.set_style(style);
}

// ---------------------------------------------------------------------------
// Formatting Helpers
// ---------------------------------------------------------------------------

/// Converts linear gain (`0.0..=1.5`) into a calibrated decibel string.
pub fn gain_to_db_str(gain: f64) -> String {
    if gain <= 0.0005 {
        return "-∞ dB".to_string();
    }
    let db = 20.0 * gain.log10();
    if db.abs() < 0.05 {
        "+0.0 dB".to_string()
    } else {
        format!("{db:+.1} dB")
    }
}

// ---------------------------------------------------------------------------
// Custom DAW Studio Widgets
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct FaderResponse {
    pub changed: bool,
    pub released: bool,
    pub hovered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadStyle {
    Neutral,
    CopperPrimary,
    CopperTinted,
    RoseMute,
    AmberWarn,
}

/// Precision vertical DAW fader (`0.0..=1.5`, unity at `1.0`).
///
/// Supports pointer drag, double-click reset to `1.0`, and fine mouse-wheel tuning.
#[allow(clippy::too_many_arguments)]
pub fn vertical_studio_fader(
    ui: &mut Ui,
    id: egui::Id,
    gain: &mut f64,
    width: f32,
    height: f32,
    accent: Color32,
    dimmed: bool,
    is_streaming: bool,
) -> FaderResponse {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let painter = ui.painter();

    let pad_y = 14.0;
    let travel_top = rect.top() + pad_y;
    let travel_bot = rect.bottom() - pad_y;
    let travel_h = (travel_bot - travel_top).max(20.0);

    let mut out = FaderResponse {
        hovered: response.hovered() || response.dragged(),
        ..Default::default()
    };

    if response.double_clicked() {
        *gain = 1.0;
        out.changed = true;
        out.released = true;
    } else if response.dragged() {
        if let Some(pos) = response.interact_pointer_pos() {
            let norm = ((travel_bot - pos.y) / travel_h).clamp(0.0, 1.0) as f64;
            let new_gain = (norm * 1.5).clamp(0.0, 1.5);
            if (new_gain - *gain).abs() > 0.0005 {
                *gain = new_gain;
                out.changed = true;
            }
        }
    } else if response.hovered() {
        let scroll_y = ui.input(|i| i.raw_scroll_delta.y);
        if scroll_y.abs() > 0.5 {
            let step = scroll_y.signum() as f64 * 0.025;
            let new_gain = (*gain + step).clamp(0.0, 1.5);
            if (new_gain - *gain).abs() > 0.0005 {
                *gain = new_gain;
                out.changed = true;
                out.released = true;
            }
        }
    }

    if response.drag_stopped() {
        out.released = true;
    }

    let norm_val = (*gain / 1.5).clamp(0.0, 1.0) as f32;
    let unity_norm = (1.0_f64 / 1.5_f64) as f32;
    let unity_y = travel_bot - unity_norm * travel_h;
    let cap_y = travel_bot - norm_val * travel_h;

    let cx = rect.center().x;

    // Subtle brushed backplate well (concentric fills avoid AA corner halos)
    paint_crisp_box(painter, rect, 10.0, BG_WELL, Some(BORDER_SUBTLE));

    // Calibrated dB tick marks flanking the central slot
    let tick_norms: [(f32, bool); 6] = [
        (1.0, false),        // +3.5 dB top
        (unity_norm, true),  // 0.0 dB unity
        (0.45, false),       // -3.5 dB
        (0.28, false),       // -7.5 dB
        (0.14, false),       // -13.5 dB
        (0.0, false),        // -∞ dB
    ];
    for &(tn, is_unity) in &tick_norms {
        let ty = travel_bot - tn * travel_h;
        let len = if is_unity { 9.0 } else { 5.0 };
        let col = if is_unity {
            if dimmed {
                TEXT_MUTED
            } else {
                COPPER_GLOW
            }
        } else {
            Color32::from_rgb(66, 66, 74)
        };
        let stroke = Stroke::new(if is_unity { 1.6_f32 } else { 1.0_f32 }, col);
        painter.line_segment([Pos2::new(cx - 16.0 - len, ty), Pos2::new(cx - 14.0, ty)], stroke);
        painter.line_segment([Pos2::new(cx + 14.0, ty), Pos2::new(cx + 16.0 + len, ty)], stroke);
    }

    // Recessed vertical fader slot
    let slot_w = 8.0;
    let slot_rect = Rect::from_min_max(
        Pos2::new(cx - slot_w * 0.5, travel_top - 4.0),
        Pos2::new(cx + slot_w * 0.5, travel_bot + 4.0),
    );
    paint_crisp_box(
        painter,
        slot_rect,
        4.0,
        Color32::from_rgb(7, 7, 9),
        Some(Color32::from_rgb(30, 30, 35)),
    );

    // Active illuminated travel bar inside slot
    let active_accent = if dimmed {
        TEXT_MUTED
    } else if *gain > 1.02 {
        AMBER
    } else {
        accent
    };
    if cap_y < travel_bot {
        let fill_rect = Rect::from_min_max(
            Pos2::new(cx - 2.5, cap_y.max(travel_top)),
            Pos2::new(cx + 2.5, travel_bot + 1.0),
        );
        if is_streaming && !dimmed {
            let halo = fill_rect.expand2(Vec2::new(3.0, 0.0));
            painter.rect_filled(
                halo,
                Rounding::same(4.0),
                Color32::from_rgba_unmultiplied(
                    active_accent.r(),
                    active_accent.g(),
                    active_accent.b(),
                    38,
                ),
            );
        }
        painter.rect_filled(fill_rect, Rounding::same(2.5), active_accent);
    }

    // Unity hairline across slot
    painter.line_segment(
        [Pos2::new(cx - 6.0, unity_y), Pos2::new(cx + 6.0, unity_y)],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(253, 186, 116, 120)),
    );

    // Machined metallic fader cap
    let cap_w = (width - 14.0).clamp(24.0, 34.0);
    let cap_h = 18.0;
    let cap_rect = Rect::from_center_size(Pos2::new(cx, cap_y), Vec2::new(cap_w, cap_h));

    // Drop shadow directly below cap (inset horizontally so corners stay sharp)
    painter.rect_filled(
        cap_rect
            .shrink2(Vec2::new(1.5, 0.0))
            .translate(Vec2::new(0.0, 2.0)),
        Rounding::same(4.0),
        Color32::from_black_alpha(110),
    );

    let cap_fill = if dimmed {
        Color32::from_rgb(56, 56, 62)
    } else if response.dragged() {
        Color32::from_rgb(235, 235, 240)
    } else if response.hovered() {
        Color32::from_rgb(208, 208, 214)
    } else {
        Color32::from_rgb(176, 176, 184)
    };
    let cap_border = if response.dragged() || response.hovered() {
        active_accent
    } else {
        Color32::from_rgb(88, 88, 96)
    };

    paint_crisp_box(painter, cap_rect, 4.5, cap_fill, Some(cap_border));

    // Top specular chamfer on cap
    let top_shine = Rect::from_min_max(
        Pos2::new(cap_rect.left() + 2.0, cap_rect.top() + 1.5),
        Pos2::new(cap_rect.right() - 2.0, cap_rect.top() + 4.5),
    );
    painter.rect_filled(
        top_shine,
        Rounding::same(1.5),
        Color32::from_rgba_unmultiplied(255, 255, 255, if dimmed { 18 } else { 85 }),
    );

    // Recessed center hairline
    painter.line_segment(
        [
            Pos2::new(cap_rect.left() + 3.0, cap_rect.center().y),
            Pos2::new(cap_rect.right() - 3.0, cap_rect.center().y),
        ],
        Stroke::new(2.0_f32, if dimmed { TEXT_MUTED } else { Color32::from_rgb(24, 24, 28) }),
    );

    out
}

/// Precision horizontal studio slider (used in the Acoustic Tuning modal).
pub fn horizontal_studio_slider(
    ui: &mut Ui,
    id: egui::Id,
    value: &mut f64,
    range: RangeInclusive<f64>,
    width: f32,
    accent: Color32,
) -> FaderResponse {
    let height = 28.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let painter = ui.painter();

    let min_v = *range.start();
    let max_v = *range.end();
    let span = (max_v - min_v).max(1e-6);

    let pad_x = 12.0;
    let left_x = rect.left() + pad_x;
    let right_x = rect.right() - pad_x;
    let track_w = (right_x - left_x).max(20.0);

    let mut out = FaderResponse {
        hovered: response.hovered() || response.dragged(),
        ..Default::default()
    };

    if response.double_clicked() {
        *value = min_v;
        out.changed = true;
        out.released = true;
    } else if response.dragged() {
        if let Some(pos) = response.interact_pointer_pos() {
            let norm = ((pos.x - left_x) / track_w).clamp(0.0, 1.0) as f64;
            let new_v = min_v + norm * span;
            if (new_v - *value).abs() > 0.01 {
                *value = (new_v * 10.0).round() / 10.0;
                out.changed = true;
            }
        }
    } else if response.hovered() {
        let scroll_y = ui.input(|i| i.raw_scroll_delta.y);
        if scroll_y.abs() > 0.5 {
            let step = scroll_y.signum() as f64 * 0.5;
            let new_v = (*value + step).clamp(min_v, max_v);
            if (new_v - *value).abs() > 0.01 {
                *value = (new_v * 10.0).round() / 10.0;
                out.changed = true;
                out.released = true;
            }
        }
    }

    if response.drag_stopped() {
        out.released = true;
    }

    let cy = rect.center().y;
    let groove = Rect::from_min_max(Pos2::new(left_x, cy - 3.5), Pos2::new(right_x, cy + 3.5));
    paint_crisp_box(painter, groove, 3.5, BG_WELL, Some(BORDER_SUBTLE));

    let norm = ((*value - min_v) / span).clamp(0.0, 1.0) as f32;
    let knob_x = left_x + norm * track_w;

    if knob_x > left_x {
        let active = Rect::from_min_max(Pos2::new(left_x + 1.0, cy - 2.0), Pos2::new(knob_x, cy + 2.0));
        painter.rect_filled(active, Rounding::same(2.0), accent);
    }

    let knob_r = if response.dragged() || response.hovered() { 8.5 } else { 7.5 };
    painter.circle_filled(Pos2::new(knob_x, cy + 1.5), knob_r, Color32::from_black_alpha(110));
    painter.circle_filled(Pos2::new(knob_x, cy), knob_r + 1.2, accent);
    painter.circle_filled(Pos2::new(knob_x, cy), knob_r - 0.5, TEXT_PRIMARY);

    out
}

/// Tactile hardware pill toggle switch (`38x20`).
pub fn pill_switch(ui: &mut Ui, on: bool, accent: Color32) -> Response {
    let size = Vec2::new(38.0, 20.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();

    let bg = if on {
        tint_opaque(BG_CONTROL, accent, if response.hovered() { 235 } else { 205 })
    } else if response.hovered() {
        BG_CONTROL_HOVER
    } else {
        BG_CONTROL
    };
    let border = if on {
        None
    } else if response.hovered() {
        Some(BORDER_STRONG)
    } else {
        Some(BORDER_SUBTLE)
    };

    paint_crisp_box(painter, rect, 10.0, bg, border);

    let thumb_x = if on {
        rect.right() - 10.0
    } else {
        rect.left() + 10.0
    };
    painter.circle_filled(
        Pos2::new(thumb_x, rect.center().y),
        6.8,
        if on { TEXT_PRIMARY } else { TEXT_SECONDARY },
    );

    response
}

/// Custom tactile studio pad button with optional painted vector icon and crisp label.
pub fn studio_pad_button(
    ui: &mut Ui,
    icon: Option<UiIcon>,
    label: &str,
    min_size: Vec2,
    style: PadStyle,
) -> Response {
    let text_empty = label.trim().is_empty();
    let has_icon = icon.is_some();

    let galley = if !text_empty {
        Some(ui.painter().layout_no_wrap(
            label.to_string(),
            font_bold(11.5),
            TEXT_PRIMARY,
        ))
    } else {
        None
    };

    let icon_w = if has_icon { 14.0 } else { 0.0 };
    let gap = if has_icon && !text_empty { 6.0 } else { 0.0 };
    let text_w = galley.as_ref().map_or(0.0, |g| g.size().x);
    let needed_w = (icon_w + gap + text_w + 18.0).max(min_size.x);
    let size = Vec2::new(needed_w, min_size.y);

    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    let hovered = response.hovered();
    let pressed = response.is_pointer_button_down_on();

    let (bg, border, fg) = match style {
        PadStyle::Neutral => (
            if pressed {
                Color32::from_rgb(58, 58, 66)
            } else if hovered {
                Color32::from_rgb(50, 50, 57)
            } else {
                Color32::from_rgb(38, 38, 43)
            },
            Some(if hovered {
                Color32::from_rgb(74, 74, 84)
            } else {
                Color32::from_rgb(54, 54, 62)
            }),
            TEXT_PRIMARY,
        ),
        PadStyle::CopperPrimary => (
            if pressed {
                COPPER_DARK
            } else if hovered {
                Color32::from_rgb(251, 132, 44)
            } else {
                COPPER
            },
            None,
            Color32::from_rgb(18, 13, 10),
        ),
        PadStyle::CopperTinted => (
            if pressed || hovered {
                Color32::from_rgb(62, 38, 24)
            } else {
                Color32::from_rgb(48, 30, 20)
            },
            Some(if hovered {
                Color32::from_rgb(162, 80, 30)
            } else {
                Color32::from_rgb(120, 62, 26)
            }),
            COPPER_GLOW,
        ),
        PadStyle::RoseMute => (
            if pressed || hovered {
                Color32::from_rgb(68, 30, 34)
            } else {
                Color32::from_rgb(52, 24, 27)
            },
            Some(Color32::from_rgb(132, 46, 52)),
            Color32::from_rgb(252, 165, 165),
        ),
        PadStyle::AmberWarn => (
            if pressed || hovered {
                Color32::from_rgb(64, 46, 22)
            } else {
                Color32::from_rgb(50, 36, 18)
            },
            Some(Color32::from_rgb(138, 94, 24)),
            Color32::from_rgb(253, 224, 135),
        ),
    };

    paint_crisp_box(painter, rect, 8.0, bg, border);

    let content_w = icon_w + gap + text_w;
    let start_x = rect.center().x - content_w * 0.5;

    if let Some(ic) = icon {
        let ic_rect = Rect::from_center_size(
            Pos2::new(start_x + icon_w * 0.5, rect.center().y),
            Vec2::splat(icon_w),
        );
        icons::paint_icon(painter, ic_rect, ic, fg);
    }

    if let Some(g) = galley {
        let g = ui
            .painter()
            .layout_no_wrap(g.text().to_string(), font_bold(11.5), fg);
        let text_pos = Pos2::new(
            start_x + icon_w + gap,
            rect.center().y - g.size().y * 0.5,
        );
        painter.galley(text_pos, g, fg);
    }

    response
}

/// Hardware status pill with an illuminated circular LED indicator.
pub fn status_led_pill(
    ui: &mut Ui,
    dot_color: Color32,
    text: &str,
    text_color: Color32,
    bg: Color32,
    border: Color32,
) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font_bold(11.0), text_color);
    let w = galley.size().x + 28.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 24.0), Sense::hover());
    let painter = ui.painter();

    paint_crisp_box(painter, rect, 12.0, bg, Some(border));

    let led_pos = Pos2::new(rect.left() + 11.0, rect.center().y);
    painter.circle_filled(led_pos, 5.5, tint_opaque(bg, dot_color, 58));
    painter.circle_filled(led_pos, 3.3, dot_color);

    painter.text(
        Pos2::new(rect.left() + 19.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        font_bold(11.0),
        text_color,
    );
}

/// Compact pill badge used for channel bus type, latency, and streaming tags.
pub fn badge_chip(ui: &mut Ui, text: &str, bg: Color32, fg: Color32, border: Color32) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font_mono_bold(10.0), fg);
    let w = galley.size().x + 12.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 19.0), Sense::hover());
    let painter = ui.painter();
    paint_crisp_box(painter, rect, 5.0, bg, Some(border));
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        font_mono_bold(10.0),
        fg,
    );
}

/// Helper layout wrapping content vertically/horizontally centered inside a strip header.
#[allow(dead_code)]
pub fn center_h_layout() -> egui::Layout {
    egui::Layout::left_to_right(Align::Center)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gain_to_db_str_formatting() {
        assert_eq!(gain_to_db_str(0.0), "-∞ dB");
        assert_eq!(gain_to_db_str(0.0002), "-∞ dB");
        assert_eq!(gain_to_db_str(1.0), "+0.0 dB");
        assert_eq!(gain_to_db_str(1.5), "+3.5 dB");
        assert_eq!(gain_to_db_str(0.5), "-6.0 dB");
    }

    #[test]
    fn test_tint_opaque_blends_without_alpha() {
        let base = Color32::from_rgb(20, 20, 20);
        let accent = Color32::from_rgb(240, 120, 40);
        assert_eq!(tint_opaque(base, accent, 0), base);
        assert_eq!(tint_opaque(base, accent, 255), accent);
        let mid = tint_opaque(base, accent, 128);
        assert_eq!(mid.a(), 255);
        assert!(mid.r() > 125 && mid.r() < 135);
    }
}
