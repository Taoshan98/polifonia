"""Interactive real-time audio routing graph and connection topology visualization."""

import math
import time
from typing import List, Dict, Optional, Callable
import gi

gi.require_version('Gtk', '4.0')
gi.require_version('Adw', '1')
from gi.repository import Gtk, Adw, GLib, Pango, PangoCairo, Gdk
import cairo

from core.models import SystemConfig, SpeakerRole


def bezier_point(p0, p1, p2, p3, t: float):
    """Calculates a point along a cubic Bézier curve at parameter t in [0.0, 1.0]."""
    u = 1.0 - t
    tt = t * t
    uu = u * u
    uuu = uu * u
    ttt = tt * t
    x = uuu * p0[0] + 3 * uu * t * p1[0] + 3 * u * tt * p2[0] + ttt * p3[0]
    y = uuu * p0[1] + 3 * uu * t * p1[1] + 3 * u * tt * p2[1] + ttt * p3[1]
    return (x, y)


def draw_rounded_rect(cr: cairo.Context, x: float, y: float, w: float, h: float, r: float):
    """Draws a closed rounded rectangle path in Cairo."""
    r = min(r, w / 2.0, h / 2.0)
    cr.new_sub_path()
    cr.arc(x + w - r, y + r, r, -math.pi / 2, 0)
    cr.arc(x + w - r, y + h - r, r, 0, math.pi / 2)
    cr.arc(x + r, y + h - r, r, math.pi / 2, math.pi)
    cr.arc(x + r, y + r, r, math.pi, 3 * math.pi / 2)
    cr.close_path()


class RoutingGraphCanvas(Gtk.DrawingArea):
    """Hardware-accelerated Cairo canvas rendering dynamic audio interconnects."""

    def __init__(self, get_status_fn: Callable[[], dict], on_test_fn: Callable[[int, str], None], on_toggle_unison_fn: Callable[[], None]):
        super().__init__()
        self.get_status_fn = get_status_fn
        self.on_test_fn = on_test_fn
        self.on_toggle_unison_fn = on_toggle_unison_fn

        self.set_hexpand(True)
        self.set_vexpand(True)
        self.set_draw_func(self._on_draw)

        # Hit test bounds: dict mapping key -> (x, y, w, h, data)
        self._cards_bounds: Dict[str, dict] = {}
        self._hovered_key: Optional[str] = None
        self._pings: Dict[str, float] = {}  # key -> timestamp of test tone

        # Animation state
        self.phase: float = 0.0
        self._tick_id: int = 0
        self._is_active_view: bool = False

        # Controllers
        click_gesture = Gtk.GestureClick.new()
        click_gesture.connect("pressed", self._on_click_pressed)
        self.add_controller(click_gesture)

        motion_ctrl = Gtk.EventControllerMotion.new()
        motion_ctrl.connect("motion", self._on_mouse_motion)
        motion_ctrl.connect("leave", self._on_mouse_leave)
        self.add_controller(motion_ctrl)

    def start_animation(self):
        self._is_active_view = True
        if self._tick_id == 0:
            self._tick_id = self.add_tick_callback(self._on_tick)

    def stop_animation(self):
        self._is_active_view = False
        if self._tick_id != 0:
            self.remove_tick_callback(self._tick_id)
            self._tick_id = 0

    def trigger_ping(self, key: str):
        self._pings[key] = time.time()
        self.queue_draw()

    def _on_tick(self, widget, frame_clock):
        if not self._is_active_view:
            return GLib.SOURCE_REMOVE
        self.phase = (self.phase + 0.016) % 1.0
        self.queue_draw()
        return GLib.SOURCE_CONTINUE

    def _on_click_pressed(self, gesture, n_press, x, y):
        for key, info in self._cards_bounds.items():
            bx, by, bw, bh = info["x"], info["y"], info["w"], info["h"]
            if bx <= x <= bx + bw and by <= y <= by + bh:
                node_type = info["type"]
                if node_type == "destination":
                    dest_data = info["data"]
                    sink_id = dest_data.get("sink_id", 0)
                    display_name = dest_data.get("display_name", "")
                    self.trigger_ping(key)
                    if self.on_test_fn:
                        self.on_test_fn(sink_id, display_name)
                elif node_type == "master":
                    self.trigger_ping(key)
                    if self.on_toggle_unison_fn:
                        self.on_toggle_unison_fn()
                return

    def _on_mouse_motion(self, ctrl, x, y):
        hovered = None
        for key, info in self._cards_bounds.items():
            bx, by, bw, bh = info["x"], info["y"], info["w"], info["h"]
            if bx <= x <= bx + bw and by <= y <= by + bh:
                hovered = key
                break

        if hovered != self._hovered_key:
            self._hovered_key = hovered
            if hovered:
                cursor = Gdk.Cursor.new_from_name("pointer", None)
                self.set_cursor(cursor)
            else:
                self.set_cursor(None)
            self.queue_draw()

    def _on_mouse_leave(self, ctrl):
        if self._hovered_key is not None:
            self._hovered_key = None
            self.set_cursor(None)
            self.queue_draw()

    def _on_draw(self, area, cr: cairo.Context, width: int, height: int):
        status = self.get_status_fn() or {}
        sources = status.get("sources", [])
        destinations = status.get("destinations", [])
        is_unison_active = status.get("is_unison_active", False)
        master_vol = status.get("master_volume", 1.0)
        master_state = status.get("master_state", "STOPPED")
        max_hw_latency = status.get("max_hw_latency", 0.0)

        # Fallback dummy source if no application stream is active
        if not sources:
            sources = [{
                "id": "sys_default",
                "app_name": "System Audio / Apps",
                "icon_name": "audio-speakers-symbolic",
                "media_name": "Operating System Stream",
                "is_active": is_unison_active,
                "corked": not is_unison_active,
                "volume_pct": 100
            }]

        now = time.time()
        self._cards_bounds.clear()

        # 1. Background
        bg_pattern = cairo.LinearGradient(0, 0, width, height)
        bg_pattern.add_color_stop_rgb(0.0, 0.043, 0.055, 0.075)  # #0b0e13
        bg_pattern.add_color_stop_rgb(1.0, 0.027, 0.035, 0.051)  # #07090d
        cr.set_source(bg_pattern)
        cr.paint()

        # Blueprint Matrix Grid (subtle cross dots)
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.03)
        grid_step = 28
        for gx in range(grid_step, width, grid_step):
            for gy in range(grid_step, height, grid_step):
                cr.rectangle(gx - 1, gy - 1, 2, 2)
        cr.fill()

        # Geometry calculations
        col2_w = 310.0
        col_side_w = max(240.0, min(290.0, (width - col2_w - 110.0) / 2.0))
        col1_w = col_side_w
        col3_w = col_side_w

        col1_x = 35.0
        col2_x = (width - col2_w) / 2.0
        col3_x = width - col3_w - 35.0

        top_margin = 65.0
        avail_h = max(400.0, height - top_margin - 30.0)

        # Column section titles
        self._draw_column_headers(cr, col1_x, col2_x, col3_x, col1_w, col2_w, col3_w, top_margin)

        # 2. Layout Nodes
        # Sources layout
        src_cards = []
        n_src = max(1, len(sources))
        src_card_h = 66.0
        src_spacing = min(30.0, max(12.0, (avail_h - n_src * src_card_h) / max(1, n_src + 1)))
        src_start_y = top_margin + 36.0 + max(0.0, (avail_h - (n_src * src_card_h + (n_src - 1) * src_spacing)) / 2.0)

        for i, src in enumerate(sources):
            sy = src_start_y + i * (src_card_h + src_spacing)
            s_key = f"src_{src.get('id', i)}"
            src_cards.append({
                "key": s_key,
                "x": col1_x,
                "y": sy,
                "w": col1_w,
                "h": src_card_h,
                "data": src,
                "pin_out": (col1_x + col1_w, sy + src_card_h / 2.0)
            })
            self._cards_bounds[s_key] = {"x": col1_x, "y": sy, "w": col1_w, "h": src_card_h, "type": "source", "data": src}

        # Master Node layout (Center)
        mst_h = 164.0
        mst_y = top_margin + 36.0 + max(0.0, (avail_h - mst_h) / 2.0)
        mst_key = "master_node"
        mst_card = {
            "key": mst_key,
            "x": col2_x,
            "y": mst_y,
            "w": col2_w,
            "h": mst_h,
            "pin_in": (col2_x, mst_y + mst_h / 2.0),
            "pin_out": (col2_x + col2_w, mst_y + mst_h / 2.0)
        }
        self._cards_bounds[mst_key] = {"x": col2_x, "y": mst_y, "w": col2_w, "h": mst_h, "type": "master", "data": status}

        # Destinations layout
        dst_cards = []
        n_dst = max(1, len(destinations))
        dst_card_h = 76.0
        dst_spacing = min(22.0, max(10.0, (avail_h - n_dst * dst_card_h) / max(1, n_dst + 1)))
        dst_start_y = top_margin + 36.0 + max(0.0, (avail_h - (n_dst * dst_card_h + (n_dst - 1) * dst_spacing)) / 2.0)

        for i, dst in enumerate(destinations):
            dy = dst_start_y + i * (dst_card_h + dst_spacing)
            d_key = f"dst_{dst.get('sink_id', i)}"
            dst_cards.append({
                "key": d_key,
                "x": col3_x,
                "y": dy,
                "w": col3_w,
                "h": dst_card_h,
                "data": dst,
                "pin_in": (col3_x, dy + dst_card_h / 2.0)
            })
            self._cards_bounds[d_key] = {"x": col3_x, "y": dy, "w": col3_w, "h": dst_card_h, "type": "destination", "data": dst}

        # 3. Draw Connecting S-Curves & Real-time Flow Particles
        # Sources -> Master
        for sc in src_cards:
            src_active = sc["data"].get("is_active", False) and is_unison_active
            is_hovered = (self._hovered_key in (sc["key"], mst_key))
            self._draw_wire(cr, sc["pin_out"], mst_card["pin_in"], is_active=src_active, is_hovered=is_hovered, color_accent="cyan")

        # Master -> Destinations
        for dc in dst_cards:
            dst_data = dc["data"]
            is_enabled = dst_data.get("is_enabled", False)
            is_streaming = dst_data.get("is_streaming", False) and is_unison_active
            is_hovered = (self._hovered_key in (dc["key"], mst_key))
            self._draw_wire(cr, mst_card["pin_out"], dc["pin_in"], is_active=is_streaming, is_hovered=is_hovered, color_accent="emerald" if is_streaming else "muted")

        # 4. Draw Cards
        # Draw Source Cards
        for sc in src_cards:
            self._draw_source_card(cr, sc, is_hovered=(self._hovered_key == sc["key"]))

        # Draw Master Hub Card
        self._draw_master_card(cr, mst_card, is_unison_active, master_vol, master_state, max_hw_latency, is_hovered=(self._hovered_key == mst_key))

        # Draw Destination Cards
        for dc in dst_cards:
            self._draw_destination_card(cr, dc, is_hovered=(self._hovered_key == dc["key"]))

        # 5. Draw Ping Ripples (expanding test tone animation)
        to_del = []
        for p_key, p_t in self._pings.items():
            elapsed = now - p_t
            if elapsed > 0.45:
                to_del.append(p_key)
                continue
            card_info = self._cards_bounds.get(p_key)
            if card_info:
                alpha = 1.0 - (elapsed / 0.45)
                expand = elapsed * 35.0
                cr.save()
                cr.set_source_rgba(0.0, 0.9, 1.0, alpha * 0.7)
                cr.set_line_width(2.5)
                draw_rounded_rect(cr, card_info["x"] - expand, card_info["y"] - expand, card_info["w"] + expand * 2, card_info["h"] + expand * 2, 12.0)
                cr.stroke()
                cr.restore()
        for k in to_del:
            self._pings.pop(k, None)

    def _draw_column_headers(self, cr: cairo.Context, c1_x, c2_x, c3_x, c1_w, c2_w, c3_w, top_y):
        """Draws column section title headers with neon badges."""
        headers = [
            (c1_x, c1_w, "1. AUDIO SOURCES (CLIENTS)", (0.0, 0.8, 1.0)),
            (c2_x, c2_w, "2. POLIFONIA MASTER DSP", (0.0, 0.9, 0.7)),
            (c3_x, c3_w, "3. DESTINATIONS (SINKS)", (0.95, 0.7, 0.1))
        ]
        layout = PangoCairo.create_layout(cr)
        font_desc = Pango.FontDescription.from_string("Sans Bold 9")
        layout.set_font_description(font_desc)

        for cx, cw, title, (r, g, b) in headers:
            layout.set_text(title, -1)
            ink, log = layout.get_pixel_extents()
            tx = cx + (cw - log.width) / 2.0
            ty = top_y + 8.0

            # Subtle pill behind title
            cr.set_source_rgba(r, g, b, 0.10)
            draw_rounded_rect(cr, tx - 10, ty - 3, log.width + 20, log.height + 6, 10.0)
            cr.fill()

            cr.set_source_rgba(r, g, b, 0.40)
            cr.set_line_width(1.0)
            draw_rounded_rect(cr, tx - 10, ty - 3, log.width + 20, log.height + 6, 10.0)
            cr.stroke()

            cr.set_source_rgba(r, g, b, 0.95)
            cr.move_to(tx, ty)
            PangoCairo.show_layout(cr, layout)

    def _draw_wire(self, cr: cairo.Context, p0, p3, is_active: bool, is_hovered: bool, color_accent: str = "cyan"):
        """Draws a smooth cubic Bézier interconnect with animated glowing signal particles."""
        dx = max(40.0, abs(p3[0] - p0[0]) * 0.5)
        p1 = (p0[0] + dx, p0[1])
        p2 = (p3[0] - dx, p3[1])

        cr.save()
        cr.new_path()
        cr.move_to(p0[0], p0[1])
        cr.curve_to(p1[0], p1[1], p2[0], p2[1], p3[0], p3[1])

        if is_active:
            if color_accent == "emerald":
                core_col = (0.0, 0.92, 0.55, 0.90)
                glow_col = (0.0, 0.92, 0.55, 0.18)
                pulse_core = (0.7, 1.0, 0.8, 1.0)
                pulse_glow = (0.0, 0.92, 0.55, 0.45)
            else:
                core_col = (0.0, 0.85, 1.0, 0.90)
                glow_col = (0.0, 0.85, 1.0, 0.18)
                pulse_core = (0.8, 0.95, 1.0, 1.0)
                pulse_glow = (0.0, 0.85, 1.0, 0.45)

            # Outer diffuse glow
            cr.set_source_rgba(*glow_col)
            cr.set_line_width(6.5 if is_hovered else 4.5)
            cr.stroke_preserve()

            # Crisp core line
            cr.set_source_rgba(*core_col)
            cr.set_line_width(2.6 if is_hovered else 2.0)
            cr.stroke()

            # Draw traveling energy pulses (Signal particles flowing left to right)
            for offset in (0.0, 0.333, 0.666):
                t = (self.phase + offset) % 1.0
                px, py = bezier_point(p0, p1, p2, p3, t)

                # Outer particle glow
                cr.arc(px, py, 6.0, 0, 2 * math.pi)
                cr.set_source_rgba(*pulse_glow)
                cr.fill()

                # Core white dot
                cr.arc(px, py, 2.4, 0, 2 * math.pi)
                cr.set_source_rgba(*pulse_core)
                cr.fill()

        else:
            # Idle / Standby wire
            if is_hovered:
                cr.set_source_rgba(0.25, 0.35, 0.5, 0.7)
                cr.set_line_width(1.8)
                cr.set_dash([])
            else:
                cr.set_source_rgba(0.18, 0.22, 0.32, 0.45)
                cr.set_line_width(1.4)
                cr.set_dash([4.0, 4.0])
            cr.stroke()

        cr.restore()

        # Draw socket pins
        self._draw_pin(cr, p0[0], p0[1], is_active, color_accent)
        self._draw_pin(cr, p3[0], p3[1], is_active, color_accent)

    def _draw_pin(self, cr: cairo.Context, x: float, y: float, is_active: bool, accent: str):
        """Draws an audio socket connection pin."""
        cr.save()
        cr.arc(x, y, 4.5, 0, 2 * math.pi)
        if is_active:
            cr.set_source_rgb(0.0, 0.9, 1.0 if accent == "cyan" else 0.6)
            cr.fill_preserve()
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.8)
            cr.set_line_width(1.5)
            cr.stroke()
        else:
            cr.set_source_rgb(0.12, 0.15, 0.22)
            cr.fill_preserve()
            cr.set_source_rgba(0.3, 0.38, 0.5, 0.8)
            cr.set_line_width(1.2)
            cr.stroke()
        cr.restore()

    def _draw_source_card(self, cr: cairo.Context, card: dict, is_hovered: bool):
        x, y, w, h = card["x"], card["y"], card["w"], card["h"]
        data = card["data"]
        app_name = data.get("app_name", "Audio Stream")
        media_name = data.get("media_name", "Playback")
        is_active = data.get("is_active", False)

        cr.save()
        # Card body
        card_pat = cairo.LinearGradient(x, y, x, y + h)
        card_pat.add_color_stop_rgba(0.0, 0.10, 0.13, 0.19, 0.96)
        card_pat.add_color_stop_rgba(1.0, 0.07, 0.09, 0.14, 0.96)
        cr.set_source(card_pat)
        draw_rounded_rect(cr, x, y, w, h, 8.0)
        cr.fill()

        # Border
        if is_hovered:
            cr.set_source_rgba(0.0, 0.9, 1.0, 0.9)
            cr.set_line_width(1.8)
        elif is_active:
            cr.set_source_rgba(0.0, 0.8, 1.0, 0.55)
            cr.set_line_width(1.3)
        else:
            cr.set_source_rgba(0.18, 0.23, 0.33, 0.8)
            cr.set_line_width(1.0)
        draw_rounded_rect(cr, x, y, w, h, 8.0)
        cr.stroke()

        # Left Icon circular badge
        cr.arc(x + 22.0, y + h / 2.0, 13.0, 0, 2 * math.pi)
        cr.set_source_rgba(0.0, 0.85, 1.0, 0.15 if is_active else 0.07)
        cr.fill_preserve()
        cr.set_source_rgba(0.0, 0.85, 1.0, 0.4 if is_active else 0.2)
        cr.set_line_width(1.0)
        cr.stroke()

        # App glyph (musical note / signal)
        cr.set_source_rgba(0.0, 0.9, 1.0, 0.95 if is_active else 0.5)
        layout = PangoCairo.create_layout(cr)
        layout.set_font_description(Pango.FontDescription.from_string("Sans Bold 10"))
        layout.set_text("♫", -1)
        cr.move_to(x + 17.0, y + h / 2.0 - 7.0)
        PangoCairo.show_layout(cr, layout)

        # Title & Subtitle text
        layout.set_font_description(Pango.FontDescription.from_string("Sans Bold 9.5"))
        layout.set_text(app_name, -1)
        layout.set_width(int((w - 75) * Pango.SCALE))
        layout.set_ellipsize(Pango.EllipsizeMode.END)
        cr.set_source_rgb(0.95, 0.97, 1.0)
        cr.move_to(x + 44.0, y + 14.0)
        PangoCairo.show_layout(cr, layout)

        layout.set_font_description(Pango.FontDescription.from_string("Sans 8"))
        status_txt = f"{media_name} • Active" if is_active else f"{media_name} • Standby"
        layout.set_text(status_txt, -1)
        cr.set_source_rgba(0.0, 0.95, 0.6, 0.9) if is_active else cr.set_source_rgba(0.55, 0.62, 0.72, 0.8)
        cr.move_to(x + 44.0, y + 36.0)
        PangoCairo.show_layout(cr, layout)

        cr.restore()

    def _draw_master_card(self, cr: cairo.Context, card: dict, is_active: bool, vol: float, state: str, max_hw_latency: float, is_hovered: bool):
        x, y, w, h = card["x"], card["y"], card["w"], card["h"]
        cr.save()

        # Card body
        card_pat = cairo.LinearGradient(x, y, x + w, y + h)
        card_pat.add_color_stop_rgba(0.0, 0.11, 0.15, 0.24, 0.98)
        card_pat.add_color_stop_rgba(1.0, 0.08, 0.10, 0.17, 0.98)
        cr.set_source(card_pat)
        draw_rounded_rect(cr, x, y, w, h, 12.0)
        cr.fill()

        # Master Outer Halo & Border
        if is_active:
            cr.set_source_rgba(0.0, 0.85, 1.0, 0.18)
            cr.set_line_width(6.0)
            draw_rounded_rect(cr, x, y, w, h, 12.0)
            cr.stroke()

            border_pat = cairo.LinearGradient(x, y, x + w, y)
            border_pat.add_color_stop_rgb(0.0, 0.0, 0.85, 1.0)
            border_pat.add_color_stop_rgb(1.0, 0.2, 0.5, 1.0)
            cr.set_source(border_pat)
            cr.set_line_width(2.2 if is_hovered else 1.8)
        else:
            cr.set_source_rgba(0.24, 0.29, 0.40, 0.8)
            cr.set_line_width(1.4 if is_hovered else 1.0)
        draw_rounded_rect(cr, x, y, w, h, 12.0)
        cr.stroke()

        # Header Title
        layout = PangoCairo.create_layout(cr)
        layout.set_font_description(Pango.FontDescription.from_string("Sans Bold 9.5"))
        layout.set_text("✦ POLIFONIA DSP HUB", -1)
        cr.set_source_rgb(1.0, 1.0, 1.0)
        cr.move_to(x + 18.0, y + 16.0)
        PangoCairo.show_layout(cr, layout)

        # Status Badge Pill
        status_text = "● UNISON ACTIVE" if is_active else "○ ENGINE STANDBY"
        status_color = (0.0, 0.95, 0.65) if is_active else (0.55, 0.62, 0.72)
        layout.set_font_description(Pango.FontDescription.from_string("Sans Bold 8.5"))
        layout.set_text(status_text, -1)
        ink, log = layout.get_pixel_extents()

        bx, by = x + w - log.width - 26.0, y + 14.0
        cr.set_source_rgba(status_color[0], status_color[1], status_color[2], 0.15)
        draw_rounded_rect(cr, bx - 6, by - 2, log.width + 12, log.height + 4, 8.0)
        cr.fill()
        cr.set_source_rgba(status_color[0], status_color[1], status_color[2], 0.4)
        cr.set_line_width(1.0)
        draw_rounded_rect(cr, bx - 6, by - 2, log.width + 12, log.height + 4, 8.0)
        cr.stroke()

        cr.set_source_rgb(*status_color)
        cr.move_to(bx, by)
        PangoCairo.show_layout(cr, layout)

        # Divider line
        cr.set_source_rgba(0.2, 0.26, 0.38, 0.5)
        cr.set_line_width(1.0)
        cr.move_to(x + 18.0, y + 42.0)
        cr.line_to(x + w - 18.0, y + 42.0)
        cr.stroke()

        # Master Gain Bar
        vol_pct = int(vol * 100)
        layout.set_font_description(Pango.FontDescription.from_string("Sans Bold 8.5"))
        layout.set_text(f"MASTER VOLUME: {vol_pct}%", -1)
        cr.set_source_rgba(0.8, 0.88, 1.0, 0.9)
        cr.move_to(x + 18.0, y + 52.0)
        PangoCairo.show_layout(cr, layout)

        # Gauge trough
        gx, gy, gw, gh = x + 18.0, y + 70.0, w - 36.0, 7.0
        cr.set_source_rgba(0.07, 0.09, 0.13, 0.9)
        draw_rounded_rect(cr, gx, gy, gw, gh, 3.5)
        cr.fill()

        # Gauge fill
        fill_w = max(0.0, min(gw, gw * (vol / 1.5)))
        gauge_pat = cairo.LinearGradient(gx, gy, gx + gw, gy)
        gauge_pat.add_color_stop_rgb(0.0, 0.0, 0.9, 0.6)
        gauge_pat.add_color_stop_rgb(0.7, 0.0, 0.8, 1.0)
        gauge_pat.add_color_stop_rgb(1.0, 0.95, 0.6, 0.1)
        cr.set_source(gauge_pat)
        draw_rounded_rect(cr, gx, gy, fill_w, gh, 3.5)
        cr.fill()

        # Specifications details
        layout.set_font_description(Pango.FontDescription.from_string("Sans 8"))
        layout.set_text("Format: 48.0 kHz • 32-bit Float • Stereo", -1)
        cr.set_source_rgba(0.65, 0.72, 0.85, 0.85)
        cr.move_to(x + 18.0, y + 90.0)
        PangoCairo.show_layout(cr, layout)

        sync_info = f"Auto-Sync: Adaptive Engine ({max_hw_latency:.0f} ms max)"
        layout.set_text(sync_info, -1)
        cr.set_source_rgba(0.0, 0.85, 1.0, 0.85)
        cr.move_to(x + 18.0, y + 108.0)
        PangoCairo.show_layout(cr, layout)

        # Click instruction hint
        hint = "⚡ Click to toggle Unison" if is_hovered else ("PipeWire Native Audio Hub" if is_active else "Click ACTIVATE to start")
        layout.set_font_description(Pango.FontDescription.from_string("Sans Italic 7.5"))
        layout.set_text(hint, -1)
        cr.set_source_rgba(0.0, 0.85, 1.0 if is_hovered else 0.45, 0.7)
        cr.move_to(x + 18.0, y + 130.0)
        PangoCairo.show_layout(cr, layout)

        cr.restore()

    def _draw_destination_card(self, cr: cairo.Context, card: dict, is_hovered: bool):
        x, y, w, h = card["x"], card["y"], card["w"], card["h"]
        dst = card["data"]
        name = dst.get("display_name", "Endpoint")
        bus = dst.get("bus_type", "other").lower()
        role = dst.get("role", "STEREO")
        total_delay = dst.get("total_delay_ms", 0.0)
        is_streaming = dst.get("is_streaming", False)
        is_enabled = dst.get("is_enabled", False)
        vol_pct = int(dst.get("volume_gain", 1.0) * 100)

        cr.save()
        # Card Body
        card_pat = cairo.LinearGradient(x, y, x, y + h)
        card_pat.add_color_stop_rgba(0.0, 0.10, 0.13, 0.19, 0.96)
        card_pat.add_color_stop_rgba(1.0, 0.07, 0.09, 0.14, 0.96)
        cr.set_source(card_pat)
        draw_rounded_rect(cr, x, y, w, h, 8.0)
        cr.fill()

        # Border
        if is_hovered:
            cr.set_source_rgba(0.0, 0.92, 0.55, 0.9)
            cr.set_line_width(1.8)
        elif is_streaming:
            cr.set_source_rgba(0.0, 0.92, 0.55, 0.6)
            cr.set_line_width(1.3)
        elif is_enabled:
            cr.set_source_rgba(0.2, 0.28, 0.4, 0.8)
            cr.set_line_width(1.0)
        else:
            cr.set_source_rgba(0.14, 0.17, 0.24, 0.6)
            cr.set_line_width(1.0)
        draw_rounded_rect(cr, x, y, w, h, 8.0)
        cr.stroke()

        # Bus Pill Badge
        bus_label = "BLUETOOTH" if bus == "bluetooth" else ("HDMI / DP" if bus == "hdmi" else ("USB AUDIO" if bus == "usb" else "INTERNAL"))
        bus_col = (0.75, 0.5, 1.0) if bus == "bluetooth" else ((0.0, 0.8, 1.0) if bus == "hdmi" else ((0.95, 0.7, 0.1) if bus == "usb" else (0.6, 0.68, 0.8)))

        layout = PangoCairo.create_layout(cr)
        layout.set_font_description(Pango.FontDescription.from_string("Sans Bold 7.5"))
        layout.set_text(bus_label, -1)
        ink, log = layout.get_pixel_extents()

        cr.set_source_rgba(bus_col[0], bus_col[1], bus_col[2], 0.15)
        draw_rounded_rect(cr, x + 12.0, y + 10.0, log.width + 8, log.height + 4, 4.0)
        cr.fill()
        cr.set_source_rgb(*bus_col)
        cr.move_to(x + 16.0, y + 12.0)
        PangoCairo.show_layout(cr, layout)

        # Role Pill Badge
        role_x = x + 12.0 + log.width + 14.0
        layout.set_text(role.upper(), -1)
        ink, log_r = layout.get_pixel_extents()
        cr.set_source_rgba(0.2, 0.26, 0.38, 0.3)
        draw_rounded_rect(cr, role_x, y + 10.0, log_r.width + 8, log_r.height + 4, 4.0)
        cr.fill()
        cr.set_source_rgb(0.7, 0.8, 0.95)
        cr.move_to(role_x + 4.0, y + 12.0)
        PangoCairo.show_layout(cr, layout)

        # State indicator on right
        state_txt = "● STREAMING" if is_streaming else ("○ READY" if is_enabled else "✕ EXCLUDED")
        state_col = (0.0, 0.95, 0.55) if is_streaming else ((0.55, 0.62, 0.72) if is_enabled else (0.4, 0.45, 0.55))
        layout.set_font_description(Pango.FontDescription.from_string("Sans Bold 7.5"))
        layout.set_text(state_txt, -1)
        ink, log_s = layout.get_pixel_extents()
        cr.set_source_rgb(*state_col)
        cr.move_to(x + w - log_s.width - 12.0, y + 12.0)
        PangoCairo.show_layout(cr, layout)

        # Speaker Display Name
        layout.set_font_description(Pango.FontDescription.from_string("Sans Bold 9.5"))
        layout.set_text(name, -1)
        layout.set_width(int((w - 24) * Pango.SCALE))
        layout.set_ellipsize(Pango.EllipsizeMode.END)
        cr.set_source_rgb(1.0, 1.0, 1.0 if is_enabled else 0.55)
        cr.move_to(x + 12.0, y + 30.0)
        PangoCairo.show_layout(cr, layout)

        # Bottom specs line: Gain & Latency
        layout.set_font_description(Pango.FontDescription.from_string("Sans 8"))
        if is_enabled:
            delay_str = f"+{total_delay:.1f}ms auto-sync" if total_delay > 0.05 else "0.0ms delay"
            info_txt = f"Vol: {vol_pct}% • {delay_str}"
            cr.set_source_rgba(0.0, 0.85, 1.0, 0.9)
        else:
            info_txt = "Channel Excluded from Mix"
            cr.set_source_rgba(0.5, 0.55, 0.65, 0.7)

        if is_hovered:
            info_txt = "⚡ Click to test tone"
            cr.set_source_rgba(0.95, 0.7, 0.1, 1.0)

        layout.set_text(info_txt, -1)
        cr.move_to(x + 12.0, y + 52.0)
        PangoCairo.show_layout(cr, layout)

        cr.restore()


class RoutingGraphView(Gtk.Box):
    """Complete studio signal flow and routing topology view widget."""

    def __init__(self, audio_service, config: SystemConfig, on_test_speaker: Optional[Callable] = None, on_toggle_unison: Optional[Callable] = None):
        super().__init__(orientation=Gtk.Orientation.VERTICAL, spacing=0)
        self.audio_service = audio_service
        self.config = config
        self.on_test_speaker = on_test_speaker
        self.on_toggle_unison = on_toggle_unison

        self._poll_timeout_id: int = 0
        self._cached_status: dict = {}

        self._build_ui()

    def _build_ui(self):
        # 1. Top Statistics & Control Bar
        stats_bar = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=14)
        stats_bar.add_css_class("master-header-box")
        stats_bar.set_margin_start(16)
        stats_bar.set_margin_end(16)
        stats_bar.set_margin_top(8)
        stats_bar.set_margin_bottom(8)

        # Title icon and label
        title_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        title_icon = Gtk.Image.new_from_icon_name("network-workgroup-symbolic")
        title_icon.add_css_class("strip-icon")
        title_box.append(title_icon)

        title_lbl = Gtk.Label(label="SIGNAL ROUTING & TOPOLOGY")
        title_lbl.add_css_class("studio-brand-title")
        title_box.append(title_lbl)
        stats_bar.append(title_box)

        # Status Pills in Header
        self.sources_pill = Gtk.Label(label="Sources: 0")
        self.sources_pill.add_css_class("vol-badge")
        stats_bar.append(self.sources_pill)

        self.dsp_pill = Gtk.Label(label="DSP Hub: 48 kHz")
        self.dsp_pill.add_css_class("status-badge-active")
        stats_bar.append(self.dsp_pill)

        self.sinks_pill = Gtk.Label(label="Active Sinks: 0")
        self.sinks_pill.add_css_class("delay-badge")
        stats_bar.append(self.sinks_pill)

        self.latency_pill = Gtk.Label(label="Max Sync: 0 ms")
        self.latency_pill.add_css_class("vol-badge")
        stats_bar.append(self.latency_pill)

        # Spacer
        spacer = Gtk.Box()
        spacer.set_hexpand(True)
        stats_bar.append(spacer)

        # Refresh Button
        refresh_btn = Gtk.Button.new_from_icon_name("view-refresh-symbolic")
        refresh_btn.set_tooltip_text("Rescan Active PipeWire Streams & Topology")
        refresh_btn.connect("clicked", lambda b: self.refresh_topology())
        stats_bar.append(refresh_btn)

        self.append(stats_bar)

        # 2. Scrolled Window containing the Drawing Canvas
        self.scroll_window = Gtk.ScrolledWindow()
        self.scroll_window.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.AUTOMATIC)
        self.scroll_window.set_vexpand(True)
        self.scroll_window.set_hexpand(True)

        self.canvas = RoutingGraphCanvas(
            get_status_fn=self.get_current_status,
            on_test_fn=self._on_canvas_test_speaker,
            on_toggle_unison_fn=self._on_canvas_toggle_unison
        )
        self.scroll_window.set_child(self.canvas)
        self.append(self.scroll_window)

        # Initial fetch
        self.refresh_topology()

    def get_current_status(self) -> dict:
        if not self._cached_status:
            try:
                res = self.audio_service.get_routing_status()
                if isinstance(res, dict):
                    self._cached_status = res
                else:
                    self._cached_status = {}
            except Exception:
                self._cached_status = {}
        return self._cached_status

    def refresh_topology(self):
        try:
            res = self.audio_service.get_routing_status()
            if isinstance(res, dict):
                self._cached_status = res
            self._update_pills()
            self.canvas.queue_draw()
        except Exception:
            pass

    def _update_pills(self):
        st = self._cached_status
        n_src = len(st.get("sources", []))
        dests = st.get("destinations", [])
        n_active_dests = sum(1 for d in dests if d.get("is_streaming", False))
        max_lat = st.get("max_hw_latency", 0.0)
        is_active = st.get("is_unison_active", False)

        self.sources_pill.set_text(f"Sources: {n_src}")
        self.sinks_pill.set_text(f"Active Sinks: {n_active_dests}/{len(dests)}")
        self.latency_pill.set_text(f"Max Delay Comp: {max_lat:.0f} ms")

        if is_active:
            self.dsp_pill.set_text("DSP: Active (48 kHz)")
            self.dsp_pill.remove_css_class("status-badge-idle")
            self.dsp_pill.add_css_class("status-badge-active")
        else:
            self.dsp_pill.set_text("DSP: Standby")
            self.dsp_pill.remove_css_class("status-badge-active")
            self.dsp_pill.add_css_class("status-badge-idle")

    def _on_canvas_test_speaker(self, sink_id: int, display_name: str):
        if self.on_test_speaker:
            self.on_test_speaker(sink_id, display_name)

    def _on_canvas_toggle_unison(self):
        if self.on_toggle_unison:
            self.on_toggle_unison(None)
            self.refresh_topology()

    def start_polling(self):
        self.canvas.start_animation()
        if self._poll_timeout_id == 0:
            self._poll_timeout_id = GLib.timeout_add(900, self._on_poll_timer)

    def stop_polling(self):
        self.canvas.stop_animation()
        if self._poll_timeout_id != 0:
            GLib.source_remove(self._poll_timeout_id)
            self._poll_timeout_id = 0

    def _on_poll_timer(self):
        self.refresh_topology()
        return GLib.SOURCE_CONTINUE
