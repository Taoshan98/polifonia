//! Procedural vector icons and brand emblem rendering for Polifonia Audio Studio.
//!
//! Draws the "Polyphonic Fader Matrix" hero icon (MSAA RGBA/ARGB bitmaps for the
//! window viewport and system tray, plus native `egui::Painter` header emblem) and
//! crisp hardware/transport vector glyphs on an explicit coordinate grid.

use egui::{Color32, Pos2, Rect, Rounding, Stroke, Vec2};

/// Visual icon set rendered natively through [`egui::Painter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiIcon {
    Display,
    Bluetooth,
    UsbDac,
    Spdif,
    Headphones,
    Network,
    Laptop,
    UnisonPower,
    AutoSyncMic,
    TestTone,
    Mute,
    SpeakerOn,
    Polarity180,
    AcousticTune,
    StripDensity,
    Refresh,
    EditPen,
    Mixer,
    SignalFlow,
    WinMinimize,
    WinMaximize,
    WinRestore,
    WinClose,
}

/// Maps a discovered PipeWire `bus_type` and internal flag to its hardware icon.
pub fn icon_for_bus(bus_type: &str, is_internal: bool) -> UiIcon {
    match bus_type {
        "hdmi" => UiIcon::Display,
        "bluetooth" => UiIcon::Bluetooth,
        "usb" => UiIcon::UsbDac,
        "spdif" => UiIcon::Spdif,
        "jack" => UiIcon::Headphones,
        "network" => UiIcon::Network,
        "internal" => UiIcon::Laptop,
        _ if is_internal => UiIcon::Laptop,
        _ => UiIcon::SpeakerOn,
    }
}

/// Renders the "Polyphonic Fader Matrix" brand icon at `size x size` using 4x4 MSAA.
///
/// Set `copper_accent = true` for active studio chrome (window icon / active tray),
/// or `false` for monochrome titanium standby tray state.
/// Set `argb_order = true` for `ksni` ARGB32 tray pixmaps, or `false` for `egui::IconData` RGBA.
pub fn render_fader_matrix_icon(size: usize, copper_accent: bool, argb_order: bool) -> Vec<u8> {
    let mut out = vec![0u8; size * size * 4];
    let samples = 4usize;
    let inv_total = 1.0 / ((samples * samples) as f32);

    // 4 symmetrical vertical channels matching assets/io.github.taoshan98.Polifonia.svg
    let track_xs = [0.266_f32, 0.422, 0.578, 0.734];
    let track_half_w = 0.023_f32;
    let track_top = 0.226_f32;
    let track_bot = 0.774_f32;
    let unity_y = 0.472_f32;

    let cap_half_w = 0.046_f32;
    let cap_half_h = 0.064_f32;

    for py in 0..size {
        for px in 0..size {
            let mut acc_r = 0.0_f32;
            let mut acc_g = 0.0_f32;
            let mut acc_b = 0.0_f32;
            let mut acc_a = 0.0_f32;

            for sy in 0..samples {
                for sx in 0..samples {
                    let u = (px as f32 + (sx as f32 + 0.5) / samples as f32) / size as f32;
                    let v = (py as f32 + (sy as f32 + 0.5) / samples as f32) / size as f32;

                    let (r, g, b, a) = sample_fader_matrix(
                        u,
                        v,
                        copper_accent,
                        &track_xs,
                        track_half_w,
                        track_top,
                        track_bot,
                        unity_y,
                        cap_half_w,
                        cap_half_h,
                    );
                    acc_r += r;
                    acc_g += g;
                    acc_b += b;
                    acc_a += a;
                }
            }

            let r8 = (acc_r * inv_total * 255.0).round().clamp(0.0, 255.0) as u8;
            let g8 = (acc_g * inv_total * 255.0).round().clamp(0.0, 255.0) as u8;
            let b8 = (acc_b * inv_total * 255.0).round().clamp(0.0, 255.0) as u8;
            let a8 = (acc_a * inv_total * 255.0).round().clamp(0.0, 255.0) as u8;

            let idx = (py * size + px) * 4;
            if argb_order {
                out[idx] = a8;
                out[idx + 1] = r8;
                out[idx + 2] = g8;
                out[idx + 3] = b8;
            } else {
                out[idx] = r8;
                out[idx + 1] = g8;
                out[idx + 2] = b8;
                out[idx + 3] = a8;
            }
        }
    }

    out
}

#[allow(clippy::too_many_arguments)]
fn sample_fader_matrix(
    u: f32,
    v: f32,
    copper_accent: bool,
    track_xs: &[f32; 4],
    track_half_w: f32,
    track_top: f32,
    track_bot: f32,
    unity_y: f32,
    cap_half_w: f32,
    cap_half_h: f32,
) -> (f32, f32, f32, f32) {
    // Outer chassis squircle (464/512 -> half extent 0.4531, rx=108/512=0.2109)
    let outer_d = sd_rounded_box(u - 0.5, v - 0.5, 0.4531, 0.4531, 0.2109);
    if outer_d > 0.0 {
        return (0.0, 0.0, 0.0, 0.0);
    }

    // Matte Carbon metallic body gradient (#222226 -> #18181B -> #111113)
    let t_v = v.clamp(0.0, 1.0);
    let mut r = 0.133 - 0.066 * t_v;
    let mut g = 0.133 - 0.066 * t_v;
    let mut b = 0.149 - 0.074 * t_v;

    // Outer dark chassis stroke (#09090B) and metallic rim bevel (#484852 -> #18181B)
    if outer_d > -0.009 {
        r = 0.035;
        g = 0.035;
        b = 0.043;
    } else if outer_d > -0.017 {
        let bevel = (1.0 - t_v) * 0.16;
        r += bevel;
        g += bevel;
        b += bevel * 1.12;
    }

    // Inner recessed matrix bay (#0B0B0D -> #121215) with #26262B border
    let bay_d = sd_rounded_box(u - 0.5, v - 0.5, 0.3711, 0.3711, 0.1484);
    if bay_d <= 0.0 {
        r = 0.043 + 0.027 * t_v;
        g = 0.043 + 0.027 * t_v;
        b = 0.051 + 0.031 * t_v;

        if bay_d > -0.008 {
            r = 0.149;
            g = 0.149;
            b = 0.169;
        } else if copper_accent {
            // Soft elliptical copper horizon glow centered on unity rail
            let gx = (u - 0.5) / 0.361;
            let gy = (v - unity_y) / 0.176;
            let dist2 = gx * gx + gy * gy;
            if dist2 < 1.0 {
                let glow = (1.0 - dist2).powf(2.0) * 0.30;
                r += 0.976 * glow;
                g += 0.451 * glow;
                b += 0.086 * glow;
            }
        }
    }

    // Calibrated dB tick marks flanking left & right edges of bay (matches SVG 132..374)
    for i in 0..12 {
        let tick_y = (132.0 + 22.0 * (i as f32)) / 512.0;
        let is_unity_tick = i == 5;
        if (v - tick_y).abs() <= 0.0042 {
            let left_end = if is_unity_tick { 0.1953 } else { 0.1855 };
            let right_start = if is_unity_tick { 0.8047 } else { 0.8145 };
            if (u >= 0.1719 && u <= left_end) || (u >= right_start && u <= 0.8281) {
                if is_unity_tick {
                    if copper_accent {
                        r = 0.976;
                        g = 0.451;
                        b = 0.086;
                    } else {
                        r = 0.62;
                        g = 0.62;
                        b = 0.66;
                    }
                } else {
                    r = 0.247;
                    g = 0.247;
                    b = 0.275;
                }
            }
        }
    }

    // Unity calibration line across bay
    if u >= 0.184 && u <= 0.816 && (v - unity_y).abs() <= 0.007 {
        let edge_fade = 1.0 - ((u - 0.5).abs() / 0.316).powf(2.5);
        if copper_accent {
            r = r * (1.0 - edge_fade) + 0.992 * edge_fade;
            g = g * (1.0 - edge_fade) + 0.729 * edge_fade;
            b = b * (1.0 - edge_fade) + 0.455 * edge_fade;
        } else {
            r = r * (1.0 - edge_fade) + 0.56 * edge_fade;
            g = g * (1.0 - edge_fade) + 0.56 * edge_fade;
            b = b * (1.0 - edge_fade) + 0.60 * edge_fade;
        }
    }

    // Track slots, staggered ghost unaligned markers, lower active fill, and tactile fader caps
    let track_cy = 0.5 * (track_top + track_bot);
    let track_hh = 0.5 * (track_bot - track_top);
    let ghost_vs = [0.2969_f32, 0.6367, 0.3398, 0.6719];

    for (idx, &cx) in track_xs.iter().enumerate() {
        let slot_d = sd_rounded_box(u - cx, v - track_cy, track_half_w, track_hh, track_half_w);
        if slot_d <= 0.0 {
            r = 0.094;
            g = 0.094;
            b = 0.106;
            if v >= unity_y && (u - cx).abs() <= track_half_w * 0.65 {
                let fill_t = ((v - unity_y) / (track_bot - unity_y)).clamp(0.0, 1.0);
                if copper_accent {
                    r = 0.984 - 0.62 * fill_t;
                    g = 0.573 - 0.45 * fill_t;
                    b = 0.235 - 0.17 * fill_t;
                } else {
                    r = 0.45 - 0.18 * fill_t;
                    g = 0.45 - 0.18 * fill_t;
                    b = 0.49 - 0.18 * fill_t;
                }
            }
        }

        // Ghost previous unaligned latency marker outline (#52525B)
        let gv = ghost_vs[idx];
        let ghost_d = sd_rounded_box(u - cx, v - gv, 0.03125, 0.0098, 0.0078);
        if ghost_d <= 0.0 && ghost_d > -0.0045 {
            r = r * 0.45 + 0.322 * 0.55;
            g = g * 0.45 + 0.322 * 0.55;
            b = b * 0.45 + 0.357 * 0.55;
        }

        // Tactile drop shadow slightly beneath cap
        let shadow_d = sd_rounded_box(
            u - cx,
            v - (unity_y + 0.010),
            cap_half_w,
            cap_half_h,
            0.0195,
        );
        if shadow_d <= 0.0 {
            r *= 0.45;
            g *= 0.45;
            b *= 0.45;
        }

        // Machined fader cap aligned at unity rail
        let cap_d = sd_rounded_box(u - cx, v - unity_y, cap_half_w, cap_half_h, 0.0195);
        if cap_d <= 0.0 {
            let cap_t = ((v - (unity_y - cap_half_h)) / (2.0 * cap_half_h)).clamp(0.0, 1.0);
            if copper_accent {
                r = 0.992 - 0.232 * cap_t;
                g = 0.729 - 0.474 * cap_t;
                b = 0.455 - 0.408 * cap_t;
            } else {
                r = 0.84 - 0.28 * cap_t;
                g = 0.84 - 0.28 * cap_t;
                b = 0.87 - 0.28 * cap_t;
            }
            if cap_d > -0.006 {
                // Dark bronze perimeter rim (#7C2D12)
                if copper_accent {
                    r = 0.486;
                    g = 0.176;
                    b = 0.071;
                } else {
                    r *= 0.55;
                    g *= 0.55;
                    b *= 0.55;
                }
            } else if (v - unity_y).abs() <= 0.0065 && (u - cx).abs() <= cap_half_w * 0.72 {
                // Warm cream index notch (#FFF7ED)
                r = 1.0;
                g = 0.969;
                b = 0.929;
            } else if v < unity_y - 0.0065 && (u - cx).abs() <= cap_half_w * 0.82 {
                // Top machined chamfer highlight
                r = r * 0.82 + 0.18;
                g = g * 0.82 + 0.15;
                b = b * 0.82 + 0.12;
            }
        }
    }

    (r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0), 1.0)
}

#[inline]
fn sd_rounded_box(px: f32, py: f32, half_w: f32, half_h: f32, radius: f32) -> f32 {
    let qx = px.abs() - (half_w - radius);
    let qy = py.abs() - (half_h - radius);
    let outside = Vec2::new(qx.max(0.0), qy.max(0.0)).length();
    let inside = qx.max(qy).min(0.0);
    outside + inside - radius
}

/// Uploads the full-color 128x128 "Polyphonic Fader Matrix" brand emblem as a GPU texture
/// so the header emblem matches the GNOME desktop/tray SVG 1:1 in every engine state.
pub fn load_brand_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let size = 128usize;
    let rgba = render_fader_matrix_icon(size, true, false);
    let image = egui::ColorImage::from_rgba_unmultiplied([size, size], &rgba);
    ctx.load_texture(
        "polifonia_brand_matrix",
        image,
        egui::TextureOptions::LINEAR,
    )
}

/// Paints the pre-rendered "Polyphonic Fader Matrix" brand emblem inside `rect`.
pub fn paint_brand_emblem(painter: &egui::Painter, rect: Rect, tex: &egui::TextureHandle) {
    painter.image(
        tex.id(),
        rect,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );
}

/// Renders any [`UiIcon`] centered inside `rect` with `color`.
pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: UiIcon, color: Color32) {
    let side = rect.width().min(rect.height());
    let c = rect.center();
    let u = side / 24.0;
    let pt = |x: f32, y: f32| Pos2::new(c.x + (x - 12.0) * u, c.y + (y - 12.0) * u);
    let sw = (1.65 * u).max(1.25);
    let thin = (1.35 * u).max(1.0);
    let stroke = Stroke::new(sw, color);
    let thin_stroke = Stroke::new(thin, color);

    match icon {
        UiIcon::Display => {
            let screen = Rect::from_min_max(pt(3.5, 4.5), pt(20.5, 15.5));
            painter.rect_stroke(screen, Rounding::same(2.0 * u), stroke);
            painter.line_segment([pt(12.0, 15.5), pt(12.0, 19.0)], stroke);
            painter.line_segment([pt(8.0, 19.0), pt(16.0, 19.0)], stroke);
        }
        UiIcon::Bluetooth => {
            let pts = [
                pt(7.5, 7.5),
                pt(16.5, 16.0),
                pt(12.0, 20.0),
                pt(12.0, 4.0),
                pt(16.5, 8.0),
                pt(7.5, 16.5),
            ];
            for pair in pts.windows(2) {
                painter.line_segment([pair[0], pair[1]], stroke);
            }
        }
        UiIcon::UsbDac => {
            painter.rect_stroke(
                Rect::from_min_max(pt(7.0, 8.0), pt(17.0, 17.5)),
                Rounding::same(2.0 * u),
                stroke,
            );
            painter.rect_stroke(
                Rect::from_min_max(pt(9.0, 4.0), pt(15.0, 8.0)),
                Rounding::same(0.8 * u),
                thin_stroke,
            );
            painter.line_segment([pt(12.0, 17.5), pt(12.0, 20.5)], stroke);
            painter.line_segment([pt(10.5, 12.5), pt(13.5, 12.5)], thin_stroke);
        }
        UiIcon::Spdif => {
            painter.circle_stroke(pt(12.0, 12.0), 7.5 * u, stroke);
            painter.circle_filled(pt(12.0, 12.0), 2.5 * u, color);
            painter.line_segment([pt(12.0, 2.5), pt(12.0, 4.5)], thin_stroke);
            painter.line_segment([pt(12.0, 19.5), pt(12.0, 21.5)], thin_stroke);
        }
        UiIcon::Headphones => {
            let segments = 12usize;
            let radius = 7.2 * u;
            for i in 0..segments {
                let a0 = std::f32::consts::PI + (i as f32 / segments as f32) * std::f32::consts::PI;
                let a1 = std::f32::consts::PI
                    + ((i + 1) as f32 / segments as f32) * std::f32::consts::PI;
                let p0 = Pos2::new(c.x + radius * a0.cos(), c.y + 0.5 * u + radius * a0.sin());
                let p1 = Pos2::new(c.x + radius * a1.cos(), c.y + 0.5 * u + radius * a1.sin());
                painter.line_segment([p0, p1], stroke);
            }
            painter.rect_stroke(
                Rect::from_min_max(pt(4.0, 12.5), pt(7.5, 19.0)),
                Rounding::same(1.8 * u),
                stroke,
            );
            painter.rect_stroke(
                Rect::from_min_max(pt(16.5, 12.5), pt(20.0, 19.0)),
                Rounding::same(1.8 * u),
                stroke,
            );
        }
        UiIcon::Network => {
            painter.circle_stroke(pt(12.0, 12.0), 8.0 * u, stroke);
            painter.line_segment([pt(4.2, 12.0), pt(19.8, 12.0)], thin_stroke);
            painter.line_segment([pt(12.0, 4.0), pt(12.0, 20.0)], thin_stroke);
            painter.rect_stroke(
                Rect::from_min_max(pt(8.2, 4.2), pt(15.8, 19.8)),
                Rounding::same(3.8 * u),
                thin_stroke,
            );
        }
        UiIcon::Laptop => {
            painter.rect_stroke(
                Rect::from_min_max(pt(5.0, 5.5), pt(19.0, 15.0)),
                Rounding::same(1.5 * u),
                stroke,
            );
            painter.line_segment([pt(2.5, 18.0), pt(21.5, 18.0)], stroke);
        }
        UiIcon::UnisonPower => {
            // Open power arc at top + vertical power stem
            let segs = 14usize;
            let rad = 7.2 * u;
            let start = -0.30 * std::f32::consts::PI;
            let sweep = 1.60 * std::f32::consts::PI;
            for i in 0..segs {
                let a0 = start + (i as f32 / segs as f32) * sweep;
                let a1 = start + ((i + 1) as f32 / segs as f32) * sweep;
                let p0 = Pos2::new(c.x + rad * a0.cos(), c.y + 0.5 * u + rad * a0.sin());
                let p1 = Pos2::new(c.x + rad * a1.cos(), c.y + 0.5 * u + rad * a1.sin());
                painter.line_segment([p0, p1], stroke);
            }
            painter.line_segment([pt(12.0, 4.0), pt(12.0, 12.0)], stroke);
        }
        UiIcon::AutoSyncMic => {
            painter.rect_stroke(
                Rect::from_min_max(pt(9.5, 4.0), pt(14.5, 13.0)),
                Rounding::same(2.5 * u),
                stroke,
            );
            let segs = 10usize;
            let rad = 5.4 * u;
            for i in 0..segs {
                let a0 = (i as f32 / segs as f32) * std::f32::consts::PI;
                let a1 = ((i + 1) as f32 / segs as f32) * std::f32::consts::PI;
                let p0 = Pos2::new(c.x + rad * a0.cos(), c.y - 1.0 * u + rad * a0.sin());
                let p1 = Pos2::new(c.x + rad * a1.cos(), c.y - 1.0 * u + rad * a1.sin());
                painter.line_segment([p0, p1], stroke);
            }
            painter.line_segment([pt(12.0, 16.4), pt(12.0, 20.0)], stroke);
            painter.line_segment([pt(8.5, 20.0), pt(15.5, 20.0)], stroke);
        }
        UiIcon::TestTone => {
            // Smooth single-cycle sine waveform
            let pts = [
                pt(3.5, 12.0),
                pt(6.0, 7.0),
                pt(8.5, 6.2),
                pt(12.0, 12.0),
                pt(15.5, 17.8),
                pt(18.0, 17.0),
                pt(20.5, 12.0),
            ];
            for pair in pts.windows(2) {
                painter.line_segment([pair[0], pair[1]], stroke);
            }
        }
        UiIcon::Mute => {
            // Speaker cone + crisp diagonal slash
            painter.line_segment([pt(4.5, 9.5), pt(8.0, 9.5)], stroke);
            painter.line_segment([pt(8.0, 9.5), pt(12.5, 5.5)], stroke);
            painter.line_segment([pt(12.5, 5.5), pt(12.5, 18.5)], stroke);
            painter.line_segment([pt(12.5, 18.5), pt(8.0, 14.5)], stroke);
            painter.line_segment([pt(8.0, 14.5), pt(4.5, 14.5)], stroke);
            painter.line_segment([pt(4.5, 14.5), pt(4.5, 9.5)], stroke);
            painter.line_segment([pt(16.0, 9.0), pt(20.0, 15.0)], stroke);
            painter.line_segment([pt(20.0, 9.0), pt(16.0, 15.0)], stroke);
        }
        UiIcon::SpeakerOn => {
            painter.line_segment([pt(4.5, 9.5), pt(8.0, 9.5)], stroke);
            painter.line_segment([pt(8.0, 9.5), pt(12.5, 5.5)], stroke);
            painter.line_segment([pt(12.5, 5.5), pt(12.5, 18.5)], stroke);
            painter.line_segment([pt(12.5, 18.5), pt(8.0, 14.5)], stroke);
            painter.line_segment([pt(8.0, 14.5), pt(4.5, 14.5)], stroke);
            painter.line_segment([pt(4.5, 14.5), pt(4.5, 9.5)], stroke);
            // Two wave arcs
            painter.line_segment([pt(15.8, 9.0), pt(17.2, 12.0)], thin_stroke);
            painter.line_segment([pt(17.2, 12.0), pt(15.8, 15.0)], thin_stroke);
            painter.line_segment([pt(18.5, 6.8), pt(20.4, 12.0)], thin_stroke);
            painter.line_segment([pt(20.4, 12.0), pt(18.5, 17.2)], thin_stroke);
        }
        UiIcon::Polarity180 => {
            painter.circle_stroke(pt(12.0, 12.0), 6.2 * u, stroke);
            painter.line_segment([pt(6.2, 17.8), pt(17.8, 6.2)], stroke);
        }
        UiIcon::AcousticTune => {
            // Precision equalizer / latency sliders
            painter.line_segment([pt(6.5, 5.0), pt(6.5, 19.0)], thin_stroke);
            painter.line_segment([pt(12.0, 5.0), pt(12.0, 19.0)], thin_stroke);
            painter.line_segment([pt(17.5, 5.0), pt(17.5, 19.0)], thin_stroke);
            painter.circle_filled(pt(6.5, 15.0), 2.1 * u, color);
            painter.circle_filled(pt(12.0, 9.0), 2.1 * u, color);
            painter.circle_filled(pt(17.5, 13.5), 2.1 * u, color);
        }
        UiIcon::StripDensity => {
            painter.rect_stroke(
                Rect::from_min_max(pt(4.0, 5.0), pt(8.0, 19.0)),
                Rounding::same(1.0 * u),
                thin_stroke,
            );
            painter.rect_stroke(
                Rect::from_min_max(pt(10.0, 5.0), pt(14.0, 19.0)),
                Rounding::same(1.0 * u),
                thin_stroke,
            );
            painter.rect_stroke(
                Rect::from_min_max(pt(16.0, 5.0), pt(20.0, 19.0)),
                Rounding::same(1.0 * u),
                thin_stroke,
            );
        }
        UiIcon::Refresh => {
            let segs = 12usize;
            let rad = 6.8 * u;
            let start = -0.25 * std::f32::consts::PI;
            let sweep = 1.55 * std::f32::consts::PI;
            for i in 0..segs {
                let a0 = start + (i as f32 / segs as f32) * sweep;
                let a1 = start + ((i + 1) as f32 / segs as f32) * sweep;
                let p0 = Pos2::new(c.x + rad * a0.cos(), c.y + rad * a0.sin());
                let p1 = Pos2::new(c.x + rad * a1.cos(), c.y + rad * a1.sin());
                painter.line_segment([p0, p1], stroke);
            }
            // Arrow head
            painter.line_segment([pt(16.0, 5.5), pt(19.0, 8.2)], stroke);
            painter.line_segment([pt(19.0, 8.2), pt(15.2, 9.2)], stroke);
        }
        UiIcon::EditPen => {
            painter.line_segment([pt(5.5, 18.5), pt(16.5, 7.5)], stroke);
            painter.line_segment([pt(14.8, 5.8), pt(18.2, 9.2)], stroke);
            painter.line_segment([pt(5.5, 18.5), pt(8.5, 18.0)], thin_stroke);
            painter.line_segment([pt(5.5, 18.5), pt(6.0, 15.5)], thin_stroke);
        }
        UiIcon::Mixer => {
            painter.line_segment([pt(6.0, 5.0), pt(6.0, 19.0)], thin_stroke);
            painter.line_segment([pt(12.0, 5.0), pt(12.0, 19.0)], thin_stroke);
            painter.line_segment([pt(18.0, 5.0), pt(18.0, 19.0)], thin_stroke);
            painter.rect_filled(
                Rect::from_center_size(pt(6.0, 14.0), Vec2::new(4.2 * u, 2.6 * u)),
                Rounding::same(0.8 * u),
                color,
            );
            painter.rect_filled(
                Rect::from_center_size(pt(12.0, 8.5), Vec2::new(4.2 * u, 2.6 * u)),
                Rounding::same(0.8 * u),
                color,
            );
            painter.rect_filled(
                Rect::from_center_size(pt(18.0, 12.5), Vec2::new(4.2 * u, 2.6 * u)),
                Rounding::same(0.8 * u),
                color,
            );
        }
        UiIcon::SignalFlow => {
            painter.rect_stroke(
                Rect::from_center_size(pt(6.0, 12.0), Vec2::new(4.4 * u, 4.4 * u)),
                Rounding::same(1.2 * u),
                stroke,
            );
            painter.rect_stroke(
                Rect::from_center_size(pt(18.0, 7.0), Vec2::new(4.0 * u, 4.0 * u)),
                Rounding::same(1.0 * u),
                stroke,
            );
            painter.rect_stroke(
                Rect::from_center_size(pt(18.0, 17.0), Vec2::new(4.0 * u, 4.0 * u)),
                Rounding::same(1.0 * u),
                stroke,
            );
            painter.line_segment([pt(8.2, 12.0), pt(12.2, 12.0)], thin_stroke);
            painter.line_segment([pt(12.2, 7.0), pt(12.2, 17.0)], thin_stroke);
            painter.line_segment([pt(12.2, 7.0), pt(16.0, 7.0)], thin_stroke);
            painter.line_segment([pt(12.2, 17.0), pt(16.0, 17.0)], thin_stroke);
        }
        UiIcon::WinMinimize => {
            painter.line_segment([pt(7.0, 12.5), pt(17.0, 12.5)], stroke);
        }
        UiIcon::WinMaximize => {
            painter.rect_stroke(
                Rect::from_min_max(pt(7.2, 7.2), pt(16.8, 16.8)),
                Rounding::same(1.2 * u),
                thin_stroke,
            );
        }
        UiIcon::WinRestore => {
            painter.rect_stroke(
                Rect::from_min_max(pt(6.8, 8.8), pt(14.8, 16.8)),
                Rounding::same(1.0 * u),
                thin_stroke,
            );
            painter.line_segment([pt(9.2, 8.8), pt(9.2, 6.8)], thin_stroke);
            painter.line_segment([pt(9.2, 6.8), pt(17.2, 6.8)], thin_stroke);
            painter.line_segment([pt(17.2, 6.8), pt(17.2, 14.8)], thin_stroke);
            painter.line_segment([pt(17.2, 14.8), pt(14.8, 14.8)], thin_stroke);
        }
        UiIcon::WinClose => {
            painter.line_segment([pt(7.5, 7.5), pt(16.5, 16.5)], stroke);
            painter.line_segment([pt(16.5, 7.5), pt(7.5, 16.5)], stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fader_matrix_icon_rgba_and_argb() {
        let size = 32usize;
        let rgba = render_fader_matrix_icon(size, true, false);
        assert_eq!(rgba.len(), size * size * 4);
        // Top-left corner outside squircle is transparent
        assert_eq!(rgba[3], 0);
        // Center pixel inside bay is opaque
        let center_idx = ((size / 2) * size + (size / 2)) * 4;
        assert_eq!(rgba[center_idx + 3], 255);

        let argb = render_fader_matrix_icon(size, false, true);
        assert_eq!(argb.len(), size * size * 4);
        assert_eq!(argb[0], 0);
        assert_eq!(argb[center_idx], 255);
    }

    #[test]
    fn test_icon_for_bus_mapping() {
        assert_eq!(icon_for_bus("hdmi", false), UiIcon::Display);
        assert_eq!(icon_for_bus("bluetooth", false), UiIcon::Bluetooth);
        assert_eq!(icon_for_bus("usb", false), UiIcon::UsbDac);
        assert_eq!(icon_for_bus("spdif", false), UiIcon::Spdif);
        assert_eq!(icon_for_bus("jack", false), UiIcon::Headphones);
        assert_eq!(icon_for_bus("network", false), UiIcon::Network);
        assert_eq!(icon_for_bus("pci", true), UiIcon::Laptop);
        assert_eq!(icon_for_bus("other", false), UiIcon::SpeakerOn);
    }
}
