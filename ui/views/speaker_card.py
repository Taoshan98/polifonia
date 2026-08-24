"""Channel Strip Card Widget for Horizontal Studio Mixing Console."""

import gi
gi.require_version('Gtk', '4.0')
gi.require_version('Adw', '1')
from gi.repository import Gtk, Adw, GLib
from core.models import SpeakerConfig, SpeakerRole


class SpeakerCard(Gtk.Box):
    """Vertical Channel Strip module representing a physical speaker or monitor."""

    def __init__(self, channel: SpeakerConfig, on_change_cb, on_test_cb):
        super().__init__(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        self.channel = channel
        self.on_change = on_change_cb
        self.on_test = on_test_cb
        self._updating_ui = False

        self.add_css_class("channel-card")
        self.set_size_request(190, 520)

        self._build_ui()
        self._sync_state()

    def _build_ui(self):
        # 1. Header Box (Icon, Title, Connection Badge, Enable Switch)
        header_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        header_box.add_css_class("strip-header")

        # Top line: Icon + Badge + Switch
        top_line = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
        
        # Hardware Icon
        icon_name = self._resolve_icon()
        self.icon_widget = Gtk.Image.new_from_icon_name(icon_name)
        self.icon_widget.set_pixel_size(24)
        self.icon_widget.add_css_class("strip-icon")
        top_line.append(self.icon_widget)

        # Connection Badge (e.g. HDMI, DP, USB, PCI)
        conn_type = self._resolve_connection_badge()
        self.badge_label = Gtk.Label(label=conn_type)
        self.badge_label.add_css_class("conn-badge")
        self.badge_label.set_hexpand(True)
        self.badge_label.set_halign(Gtk.Align.START)
        top_line.append(self.badge_label)

        # Enable/Mute Switch
        self.enable_switch = Gtk.Switch()
        self.enable_switch.set_valign(Gtk.Align.CENTER)
        self.enable_switch.connect("notify::active", self._on_enable_toggled)
        top_line.append(self.enable_switch)
        header_box.append(top_line)

        # Title / Commercial Device Name
        short_title = self._short_name(self.channel.display_name or self.channel.sink_name)
        self.title_label = Gtk.Label(label=short_title)
        self.title_label.set_tooltip_text(self.channel.display_name or self.channel.sink_name)
        self.title_label.add_css_class("strip-title")
        self.title_label.set_ellipsize(3)  # PANGO_ELLIPSIZE_END
        self.title_label.set_halign(Gtk.Align.CENTER)
        header_box.append(self.title_label)

        self.append(header_box)

        # Separator
        self.append(Gtk.Separator(orientation=Gtk.Orientation.HORIZONTAL))

        # 2. Role Selector Group (Pill Buttons: L | R | C | SUB | ALL | SURR)
        role_label = Gtk.Label(label="CHANNEL ROLE")
        role_label.add_css_class("strip-section-label")
        self.append(role_label)

        role_grid = Gtk.Grid()
        role_grid.set_column_spacing(4)
        role_grid.set_row_spacing(4)
        role_grid.set_halign(Gtk.Align.CENTER)
        self.role_buttons = {}

        roles_def = [
            ("L", SpeakerRole.LEFT, 0, 0, "Left Channel (FL)"),
            ("R", SpeakerRole.RIGHT, 1, 0, "Right Channel (FR)"),
            ("C", SpeakerRole.CENTER, 0, 1, "Center Channel (Mono)"),
            ("SUB", SpeakerRole.SUBWOOFER, 1, 1, "Subwoofer (Low Frequencies)"),
            ("ALL", SpeakerRole.STEREO, 0, 2, "Full Stereo (FL+FR)"),
            ("SURR", SpeakerRole.SURROUND_LEFT, 1, 2, "Surround Channel")
        ]

        for code, role_val, col, row, tip in roles_def:
            btn = Gtk.Button(label=code)
            btn.set_tooltip_text(tip)
            btn.add_css_class("role-pill")
            btn.connect("clicked", lambda b, r=role_val: self._on_role_clicked(r))
            role_grid.attach(btn, col, row, 1, 1)
            self.role_buttons[role_val] = btn

        self.append(role_grid)

        # Separator
        self.append(Gtk.Separator(orientation=Gtk.Orientation.HORIZONTAL))

        # 3. Vertical Volume Fader Section
        fader_header = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL)
        fader_lbl = Gtk.Label(label="LEVEL")
        fader_lbl.add_css_class("strip-section-label")
        fader_lbl.set_hexpand(True)
        fader_lbl.set_halign(Gtk.Align.START)
        fader_header.append(fader_lbl)

        self.vol_badge = Gtk.Label(label=f"{int(self.channel.volume_gain * 100)}%")
        self.vol_badge.add_css_class("vol-badge")
        fader_header.append(self.vol_badge)
        self.append(fader_header)

        # Fader scale (Vertical, inverted so up is louder)
        self.vol_scale = Gtk.Scale.new_with_range(Gtk.Orientation.VERTICAL, 0.0, 1.5, 0.05)
        self.vol_scale.set_inverted(True)
        self.vol_scale.set_vexpand(True)
        self.vol_scale.set_value(self.channel.volume_gain)
        self.vol_scale.add_css_class("vertical-fader")
        self.vol_scale.connect("value-changed", self._on_vol_changed)
        self.append(self.vol_scale)

        # 4. Delay Alignment Control
        delay_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        delay_hdr = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL)
        delay_lbl = Gtk.Label(label="DELAY / PHASE")
        delay_lbl.add_css_class("strip-section-label")
        delay_lbl.set_hexpand(True)
        delay_lbl.set_halign(Gtk.Align.START)
        delay_hdr.append(delay_lbl)

        self.delay_badge = Gtk.Label(label=f"{self.channel.delay_ms:.1f}ms")
        self.delay_badge.add_css_class("delay-badge")
        delay_hdr.append(self.delay_badge)
        delay_box.append(delay_hdr)

        self.delay_scale = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, 0.0, 80.0, 1.0)
        self.delay_scale.set_value(self.channel.delay_ms)
        self.delay_scale.add_css_class("horizontal-delay-slider")
        self.delay_scale.connect("value-changed", self._on_delay_changed)
        delay_box.append(self.delay_scale)
        self.append(delay_box)

        # 5. Acoustic Audit (Test Tone) Button
        self.test_btn = Gtk.Button(label=" Test Output")
        self.test_btn.set_icon_name("audio-volume-high-symbolic")
        self.test_btn.add_css_class("strip-test-btn")
        self.test_btn.connect("clicked", self._on_test_clicked)
        self.append(self.test_btn)

    def _resolve_icon(self) -> str:
        name = (self.channel.display_name or self.channel.sink_name).lower()
        if "monitor" in name or "hdmi" in name or "displayport" in name:
            return "video-display-symbolic"
        elif "speaker" in name or "laptop" in name or "internal" in name:
            return "audio-speakers-symbolic"
        elif "usb" in name or "dac" in name:
            return "audio-card-analog-usb-symbolic"
        elif "headphone" in name:
            return "audio-headphones-symbolic"
        elif "bluetooth" in name or "bluez" in name:
            return "bluetooth-active-symbolic"
        return "audio-speakers-symbolic"

    def _resolve_connection_badge(self) -> str:
        name = (self.channel.display_name or self.channel.sink_name).lower()
        if "displayport" in name or "dp" in name:
            return "DP"
        elif "hdmi" in name:
            return "HDMI"
        elif "usb" in name:
            return "USB"
        elif "bluetooth" in name:
            return "BT"
        elif "pci" in name or "speaker" in name:
            return "PCI"
        return "OUT"

    def _short_name(self, full_name: str) -> str:
        cleaned = full_name.replace("Monitor ", "").replace("Audio ", "").replace("Integrated Speakers ", "")
        # Cut after parenthesis if too long
        if "(" in cleaned and len(cleaned) > 20:
            cleaned = cleaned.split("(")[0].strip()
        return cleaned[:22]

    def _sync_state(self):
        self._updating_ui = True
        is_active = self.channel.role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED)
        self.enable_switch.set_active(is_active)
        self._update_role_pills(self.channel.role)
        self._updating_ui = False

    def _update_role_pills(self, active_role: SpeakerRole):
        for role_val, btn in self.role_buttons.items():
            btn.remove_css_class("role-pill-active")
            btn.remove_css_class("role-pill-sub")
            btn.remove_css_class("role-pill-left")
            btn.remove_css_class("role-pill-right")

            if role_val == active_role:
                btn.add_css_class("role-pill-active")
                if role_val == SpeakerRole.SUBWOOFER:
                    btn.add_css_class("role-pill-sub")
                elif role_val == SpeakerRole.LEFT:
                    btn.add_css_class("role-pill-left")
                elif role_val == SpeakerRole.RIGHT:
                    btn.add_css_class("role-pill-right")

    def _on_enable_toggled(self, switch, param):
        if self._updating_ui:
            return
        active = switch.get_active()
        if active:
            if self.channel.role in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED):
                self.channel.role = SpeakerRole.LEFT
        else:
            self.channel.role = SpeakerRole.EXCLUDED
        self._sync_state()
        self.on_change()

    def _on_role_clicked(self, new_role: SpeakerRole):
        self.channel.role = new_role
        self.enable_switch.set_active(True)
        self._sync_state()
        self.on_change()

    def _on_vol_changed(self, scale):
        val = scale.get_value()
        self.channel.volume_gain = round(val, 2)
        self.vol_badge.set_text(f"{int(self.channel.volume_gain * 100)}%")
        self.on_change()

    def _on_delay_changed(self, scale):
        val = scale.get_value()
        self.channel.delay_ms = round(val, 1)
        # 1ms ~ 0.343m
        dist_m = self.channel.delay_ms * 0.343
        self.delay_badge.set_text(f"{self.channel.delay_ms:.1f}ms ({dist_m:.1f}m)")
        self.on_change()

    def _on_test_clicked(self, btn):
        target = self.channel.sink_name or str(self.channel.sink_id)
        self.on_test(target, self.channel.display_name or self.channel.sink_name)
