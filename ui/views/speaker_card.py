"""Channel Strip Card Widget for Horizontal Studio Mixing Console."""

import gi
gi.require_version('Gtk', '4.0')
gi.require_version('Adw', '1')
from gi.repository import Gtk, Adw, GLib
from core.models import SpeakerConfig, SpeakerRole


class SpeakerCard(Gtk.Box):
    """Vertical Channel Strip module representing a physical speaker or monitor."""

    def __init__(self, channel: SpeakerConfig, on_change_cb, on_test_cb, audio_service=None):
        super().__init__(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        self.channel = channel
        self.on_change = on_change_cb
        self.on_test = on_test_cb
        self.audio_service = audio_service
        self._updating_ui = False
        self._vol_debounce_id = 0
        self._delay_debounce_id = 0
        self._enable_debounce_id = 0

        self.add_css_class("channel-card")
        self.set_size_request(190, 470)

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

        # Title Row: Label + Rename MenuButton
        title_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=4)
        title_box.set_halign(Gtk.Align.CENTER)
        title_box.set_hexpand(True)

        display_title = self._get_display_title()
        self.title_label = Gtk.Label(label=display_title)
        self.title_label.set_tooltip_text(self._get_title_tooltip())
        self.title_label.add_css_class("strip-title")
        if self.channel.custom_name:
            self.title_label.add_css_class("strip-title-custom")
        self.title_label.set_ellipsize(3)  # PANGO_ELLIPSIZE_END
        self.title_label.set_max_width_chars(15)

        # Rename Popover and Button
        self.edit_btn = Gtk.Button.new_from_icon_name("document-edit-symbolic")
        self.edit_btn.set_tooltip_text("Personalizza etichetta uscita audio")
        self.edit_btn.add_css_class("flat")
        self.edit_btn.add_css_class("strip-edit-btn")
        self.edit_btn.set_valign(Gtk.Align.CENTER)

        self._setup_rename_popover()
        self.edit_btn.connect("clicked", lambda b: self.popover.popup())

        # Allow clicking directly on the title to open the rename popover
        title_click = Gtk.GestureClick()
        title_click.connect("released", lambda g, n, x, y: self.popover.popup())
        self.title_label.add_controller(title_click)

        title_box.append(self.title_label)
        title_box.append(self.edit_btn)
        header_box.append(title_box)

        self.append(header_box)

        # Separator
        self.append(Gtk.Separator(orientation=Gtk.Orientation.HORIZONTAL))

        # 2. Vertical Volume Fader Section
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

        # Separator
        self.append(Gtk.Separator(orientation=Gtk.Orientation.HORIZONTAL))

        # 3. Precision Delay Alignment Control (-150ms to +300ms)
        delay_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
        delay_hdr = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL)
        delay_lbl = Gtk.Label(label="DELAY / PHASE")
        delay_lbl.add_css_class("strip-section-label")
        delay_lbl.set_hexpand(True)
        delay_lbl.set_halign(Gtk.Align.START)
        delay_hdr.append(delay_lbl)

        self.delay_badge = Gtk.Label()
        self.delay_badge.add_css_class("delay-badge")
        delay_hdr.append(self.delay_badge)
        delay_box.append(delay_hdr)

        # Controls Row: [-1] [Slider] [+1] [0]
        ctrl_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=4)
        ctrl_row.set_halign(Gtk.Align.FILL)

        self.btn_delay_minus = Gtk.Button(label="-1")
        self.btn_delay_minus.set_tooltip_text("Riduci delay (-1ms)")
        self.btn_delay_minus.add_css_class("delay-btn")
        self.btn_delay_minus.connect("clicked", lambda b: self._step_delay(-1.0))
        ctrl_row.append(self.btn_delay_minus)

        self.delay_scale = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, -150.0, 300.0, 1.0)
        self.delay_scale.set_hexpand(True)
        self.delay_scale.set_value(self.channel.delay_ms)
        self.delay_scale.add_css_class("horizontal-delay-slider")
        self.delay_scale.connect("value-changed", self._on_delay_changed)
        ctrl_row.append(self.delay_scale)

        self.btn_delay_plus = Gtk.Button(label="+1")
        self.btn_delay_plus.set_tooltip_text("Aumenta delay (+1ms)")
        self.btn_delay_plus.add_css_class("delay-btn")
        self.btn_delay_plus.connect("clicked", lambda b: self._step_delay(+1.0))
        ctrl_row.append(self.btn_delay_plus)

        self.btn_delay_zero = Gtk.Button(label="0")
        self.btn_delay_zero.set_tooltip_text("Azzera delay (0ms)")
        self.btn_delay_zero.add_css_class("delay-reset-btn")
        self.btn_delay_zero.connect("clicked", lambda b: self._reset_delay())
        ctrl_row.append(self.btn_delay_zero)

        delay_box.append(ctrl_row)
        self._update_delay_badge()
        self.append(delay_box)

        # Separator
        self.append(Gtk.Separator(orientation=Gtk.Orientation.HORIZONTAL))

        # 4. Acoustic Audit (Test Tone) Button
        self.test_btn = Gtk.Button(label=" Test Output")
        self.test_btn.set_icon_name("audio-volume-high-symbolic")
        self.test_btn.add_css_class("strip-test-btn")
        self.test_btn.connect("clicked", self._on_test_clicked)
        self.append(self.test_btn)

    def _resolve_icon(self) -> str:
        name = (self.channel.display_name or self.channel.sink_name).lower()
        if getattr(self.channel, "bus_type", "") == "bluetooth" or "bluetooth" in name or "bluez" in name:
            return "audio-headphones-bluetooth"
        elif "monitor" in name or "hdmi" in name or "displayport" in name:
            return "video-display-symbolic"
        elif "speaker" in name or "laptop" in name or "internal" in name:
            return "audio-speakers-symbolic"
        elif "usb" in name or "dac" in name:
            return "audio-card-analog-usb-symbolic"
        elif "headphone" in name:
            return "audio-headphones-symbolic"
        return "audio-speakers-symbolic"

    def _resolve_connection_badge(self) -> str:
        name = (self.channel.display_name or self.channel.sink_name).lower()
        if getattr(self.channel, "bus_type", "") == "bluetooth" or "bluetooth" in name or "bluez" in name:
            return "BT"
        elif "displayport" in name or "dp" in name:
            return "DP"
        elif "hdmi" in name:
            return "HDMI"
        elif "usb" in name:
            return "USB"
        elif "pci" in name or "speaker" in name:
            return "PCI"
        return "OUT"

    def _short_name(self, full_name: str) -> str:
        cleaned = full_name.replace("Monitor ", "").replace("Integrated Speakers ", "")
        if "Bluetooth Audio (" in cleaned and cleaned.endswith(")"):
            cleaned = cleaned[len("Bluetooth Audio ("):-1].strip()
        elif "Bluetooth (" in cleaned and cleaned.endswith(")"):
            cleaned = cleaned[len("Bluetooth ("):-1].strip()
        else:
            if cleaned.endswith(" Audio"):
                cleaned = cleaned[:-6]
            cleaned = cleaned.replace("Audio ", "")
            if "/" in cleaned and ("HDMI" in cleaned or "DisplayPort" in cleaned):
                parts = [p.strip() for p in cleaned.split("/")]
                cleaned = parts[-1]
            if "(" in cleaned and len(cleaned) > 30:
                cleaned = cleaned.split("(")[0].strip()
        return cleaned.strip()

    def _sync_state(self):
        self._updating_ui = True
        is_active = self.channel.role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED)
        self.enable_switch.set_active(is_active)
        self._update_delay_badge()
        self._updating_ui = False

    def _on_enable_toggled(self, switch, param):
        if self._updating_ui:
            return
        active = switch.get_active()
        if active:
            if self.channel.role in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED):
                self.channel.role = SpeakerRole.STEREO
            # Guarantee audible volume upon activation (Bluetooth uses cubic attenuation where <0.50 is inaudible)
            is_bt = (getattr(self.channel, "bus_type", "") == "bluetooth") or ("bluez" in self.channel.sink_name.lower())
            min_gain = 0.50 if is_bt else 0.15
            if self.channel.volume_gain < min_gain:
                target_gain = 0.85 if is_bt else 1.0
                self.channel.volume_gain = target_gain
                self.vol_scale.set_value(target_gain)
                self.vol_badge.set_text(f"{int(target_gain * 100)}%")
        else:
            self.channel.role = SpeakerRole.EXCLUDED
        self._sync_state()
        if self._enable_debounce_id:
            GLib.source_remove(self._enable_debounce_id)
        self._enable_debounce_id = GLib.timeout_add(150, self._emit_enable_change)

    def _on_role_clicked(self, new_role: SpeakerRole):
        self.channel.role = new_role
        self.enable_switch.set_active(new_role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED))
        self._sync_state()
        if self._enable_debounce_id:
            GLib.source_remove(self._enable_debounce_id)
        self._enable_debounce_id = GLib.timeout_add(150, self._emit_enable_change)

    def _emit_enable_change(self):
        self._enable_debounce_id = 0
        self.on_change()
        return False

    def _on_vol_changed(self, scale):
        val = scale.get_value()
        self.channel.volume_gain = round(val, 2)
        self.vol_badge.set_text(f"{int(self.channel.volume_gain * 100)}%")
        # Fast non-blocking real-time volume adjustment to PipeWire hardware
        if self.audio_service:
            self.audio_service.set_speaker_gain(self.channel.sink_name, self.channel.volume_gain, persist=False)
        # Debounce: only notify config change and disk save after 300ms of inactivity
        if self._vol_debounce_id:
            GLib.source_remove(self._vol_debounce_id)
        self._vol_debounce_id = GLib.timeout_add(300, self._emit_vol_change)

    def _emit_vol_change(self):
        self._vol_debounce_id = 0
        self.on_change()
        return False  # Remove timeout

    def _update_delay_badge(self):
        d = self.channel.delay_ms
        sign = "+" if d > 0 else ""
        val_str = f"{sign}{d:.1f}ms" if d != 0.0 else "0.0ms"
        hw_lat = getattr(self.channel, "hardware_latency_ms", 0.0)
        if hw_lat > 0:
            self.delay_badge.set_text(f"{val_str} (HW:{hw_lat:.0f}ms)")
        else:
            dist_m = d * 0.343
            self.delay_badge.set_text(f"{val_str} ({dist_m:+.1f}m)" if d != 0.0 else val_str)

    def _step_delay(self, delta: float):
        new_val = max(-150.0, min(300.0, round(self.channel.delay_ms + delta, 1)))
        self.delay_scale.set_value(new_val)

    def _reset_delay(self):
        self.delay_scale.set_value(0.0)

    def _on_delay_changed(self, scale):
        val = scale.get_value()
        self.channel.delay_ms = round(val, 1)
        self._update_delay_badge()
        # Debounce: apply delay change after 120ms of inactivity for snappy responsive feel
        if self._delay_debounce_id:
            GLib.source_remove(self._delay_debounce_id)
        self._delay_debounce_id = GLib.timeout_add(120, self._emit_delay_change)

    def _emit_delay_change(self):
        self._delay_debounce_id = 0
        self.on_change()
        return False  # Remove timeout

    def _setup_rename_popover(self):
        self.popover = Gtk.Popover()
        self.popover.set_parent(self.edit_btn)
        self.popover.set_autohide(True)
        # Compatibility attribute so card.edit_btn.get_popover() returns self.popover
        self.edit_btn.get_popover = lambda: self.popover

        content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        content.set_margin_top(10)
        content.set_margin_bottom(10)
        content.set_margin_start(10)
        content.set_margin_end(10)
        content.set_size_request(230, -1)

        pop_title = Gtk.Label(label="Personalizza etichetta")
        pop_title.add_css_class("heading")
        pop_title.set_halign(Gtk.Align.START)
        content.append(pop_title)

        hw_name = self.channel.display_name or self.channel.sink_name
        hw_label = Gtk.Label(label=f"Hardware: {self._short_name(hw_name)}")
        hw_label.add_css_class("dim-label")
        hw_label.set_ellipsize(3)
        hw_label.set_halign(Gtk.Align.START)
        content.append(hw_label)

        self.name_entry = Gtk.Entry()
        self.name_entry.set_placeholder_text("Es. Monitor Studio, Cuffie...")
        if self.channel.custom_name:
            self.name_entry.set_text(self.channel.custom_name)
        self.name_entry.connect("activate", lambda e: self._on_save_custom_name(self.popover))
        content.append(self.name_entry)

        btn_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
        btn_box.set_halign(Gtk.Align.END)

        reset_btn = Gtk.Button(label="Ripristina")
        reset_btn.set_tooltip_text("Ripristina nome predefinito dell'hardware")
        reset_btn.add_css_class("flat")
        reset_btn.connect("clicked", lambda b: self._on_reset_custom_name(self.popover))
        btn_box.append(reset_btn)

        save_btn = Gtk.Button(label="Salva")
        save_btn.add_css_class("suggested-action")
        save_btn.connect("clicked", lambda b: self._on_save_custom_name(self.popover))
        btn_box.append(save_btn)

        content.append(btn_box)
        self.popover.set_child(content)
        self.popover.connect("show", self._on_popover_show)

    def _on_popover_show(self, popover):
        if self.channel.custom_name:
            self.name_entry.set_text(self.channel.custom_name)
        else:
            self.name_entry.set_text("")
        self.name_entry.grab_focus()

    def _get_display_title(self) -> str:
        if self.channel.custom_name:
            return self.channel.custom_name
        return self._short_name(self.channel.display_name or self.channel.sink_name)

    def _get_title_tooltip(self) -> str:
        hw_name = self.channel.display_name or self.channel.sink_name
        if self.channel.custom_name:
            return f"{self.channel.custom_name}\n(Dispositivo: {hw_name})"
        return hw_name

    def _on_save_custom_name(self, popover):
        text = self.name_entry.get_text().strip()
        hw_name = self.channel.display_name or self.channel.sink_name
        if text and text != hw_name:
            self.channel.custom_name = text
            self.title_label.add_css_class("strip-title-custom")
        else:
            self.channel.custom_name = None
            self.title_label.remove_css_class("strip-title-custom")

        self.title_label.set_text(self._get_display_title())
        self.title_label.set_tooltip_text(self._get_title_tooltip())
        popover.popdown()
        self.on_change()

    def _on_reset_custom_name(self, popover):
        self.channel.custom_name = None
        self.name_entry.set_text("")
        self.title_label.remove_css_class("strip-title-custom")
        self.title_label.set_text(self._get_display_title())
        self.title_label.set_tooltip_text(self._get_title_tooltip())
        popover.popdown()
        self.on_change()

    def _on_test_clicked(self, btn):
        target = self.channel.sink_name or str(self.channel.sink_id)
        display_label = self.channel.custom_name or self.channel.display_name or self.channel.sink_name
        self.on_test(target, display_label)

