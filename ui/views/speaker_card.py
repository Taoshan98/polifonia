"""Horizontal Channel Row Card for Polifonia Audio Studio."""

import gi
gi.require_version('Gtk', '4.0')
gi.require_version('Adw', '1')
from gi.repository import Gtk, Adw, GLib
from core.models import SpeakerConfig, SpeakerRole


class SpeakerCard(Gtk.Box):
    """Refined horizontal channel row card representing an audio output endpoint."""

    def __init__(self, channel: SpeakerConfig, on_change_cb, on_test_cb, audio_service=None):
        super().__init__(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        self.channel = channel
        self.on_change = on_change_cb
        self.on_test = on_test_cb
        self.audio_service = audio_service
        self._updating_ui = False
        self._vol_debounce_id = 0
        self._delay_debounce_id = 0
        self._enable_debounce_id = 0

        self.add_css_class("channel-card")
        self.set_hexpand(True)

        self._build_ui()
        self._sync_state()

    def _build_ui(self):
        # 1. Header Row: Title & Device Info (Icon + Custom Title + Rename + Connection Badge + Subtitle)
        title_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        title_row.set_valign(Gtk.Align.CENTER)

        # Hardware Icon
        icon_name = self._resolve_icon()
        self.icon_widget = Gtk.Image.new_from_icon_name(icon_name)
        self.icon_widget.set_pixel_size(20)
        self.icon_widget.add_css_class("strip-icon")
        title_row.append(self.icon_widget)

        # Title Label
        display_title = self._get_display_title()
        self.title_label = Gtk.Label(label=display_title)
        self.title_label.set_tooltip_text(self._get_title_tooltip())
        self.title_label.add_css_class("strip-title")
        if self.channel.custom_name:
            self.title_label.add_css_class("strip-title-custom")
        self.title_label.set_halign(Gtk.Align.START)

        # Rename Edit Button & Popover
        self.edit_btn = Gtk.Button.new_from_icon_name("document-edit-symbolic")
        self.edit_btn.set_tooltip_text("Customize output name")
        self.edit_btn.add_css_class("flat")
        self.edit_btn.add_css_class("strip-edit-btn")
        self.edit_btn.set_valign(Gtk.Align.CENTER)
        self._setup_rename_popover()
        self.edit_btn.connect("clicked", lambda b: self.popover.popup())

        # Allow clicking title to open rename popover
        title_click = Gtk.GestureClick()
        title_click.connect("released", lambda g, n, x, y: self.popover.popup())
        self.title_label.add_controller(title_click)

        title_row.append(self.title_label)
        title_row.append(self.edit_btn)

        # Connection Badge (e.g. HDMI, DP, USB, BT, PCI)
        conn_type = self._resolve_connection_badge()
        self.badge_label = Gtk.Label(label=conn_type)
        self.badge_label.add_css_class("conn-badge")
        title_row.append(self.badge_label)

        # Hardware Description
        hw_desc = self._short_name(self.channel.display_name or self.channel.sink_name)
        self.desc_label = Gtk.Label(label=f"({hw_desc})")
        self.desc_label.add_css_class("dim-label")
        self.desc_label.set_halign(Gtk.Align.START)
        title_row.append(self.desc_label)

        self.append(title_row)

        # 2. Controls Row (Volume -> Ruolo -> Delay -> Test -> Switch On/Off)
        controls_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=14)
        controls_row.set_valign(Gtk.Align.CENTER)
        controls_row.set_hexpand(True)

        # 2A. Barra del volume (Expands to utilize available width proportionally)
        vol_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        vol_box.set_valign(Gtk.Align.CENTER)
        vol_box.set_hexpand(True)

        vol_icon = Gtk.Image.new_from_icon_name("audio-volume-medium-symbolic")
        vol_icon.add_css_class("dim-label")
        vol_box.append(vol_icon)

        self.vol_scale = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, 0.0, 1.5, 0.05)
        self.vol_scale.set_hexpand(True)
        self.vol_scale.set_value(self.channel.volume_gain)
        self.vol_scale.connect("value-changed", self._on_vol_changed)
        vol_box.append(self.vol_scale)

        self.vol_badge = Gtk.Label(label=f"{int(self.channel.volume_gain * 100)}%")
        self.vol_badge.add_css_class("vol-badge")
        self.vol_badge.set_size_request(46, -1)
        vol_box.append(self.vol_badge)
        controls_row.append(vol_box)

        # 2B. Selettore Ruolo (Nascosto su richiesta UI, default Stereo)
        self.role_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL)
        self.role_box.set_valign(Gtk.Align.CENTER)
        self.role_box.set_visible(False)

        self.role_btn = Gtk.MenuButton()
        self.role_btn.add_css_class("flat")
        self.role_btn.add_css_class("role-pill")
        self.role_btn.set_halign(Gtk.Align.FILL)
        self.role_btn.set_visible(False)
        self._setup_role_popover()
        self.role_box.append(self.role_btn)
        controls_row.append(self.role_box)

        # 2C. Sezione per la gestione del delay (senza la barra del delay!)
        delay_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=4)
        delay_box.set_valign(Gtk.Align.CENTER)
        delay_box.set_size_request(245, -1)

        delay_lbl = Gtk.Label(label="Delay:")
        delay_lbl.add_css_class("dim-label")
        delay_box.append(delay_lbl)

        self.btn_delay_minus_10 = Gtk.Button(label="-1")
        self.btn_delay_minus_10.set_tooltip_text("Decrease delay (-1 ms)")
        self.btn_delay_minus_10.add_css_class("delay-btn")
        self.btn_delay_minus_10.connect("clicked", lambda b: self._step_delay(-1.0))
        delay_box.append(self.btn_delay_minus_10)

        self.btn_delay_minus = Gtk.Button(label="-0.5")
        self.btn_delay_minus.set_tooltip_text("Decrease delay (-0.5 ms)")
        self.btn_delay_minus.add_css_class("delay-btn")
        self.btn_delay_minus.connect("clicked", lambda b: self._step_delay(-0.5))
        delay_box.append(self.btn_delay_minus)

        self.delay_badge = Gtk.Label()
        self.delay_badge.add_css_class("delay-badge")
        self.delay_badge.set_size_request(65, -1)
        delay_box.append(self.delay_badge)

        self.btn_delay_plus = Gtk.Button(label="+0.5")
        self.btn_delay_plus.set_tooltip_text("Increase delay (+0.5 ms)")
        self.btn_delay_plus.add_css_class("delay-btn")
        self.btn_delay_plus.connect("clicked", lambda b: self._step_delay(+0.5))
        delay_box.append(self.btn_delay_plus)

        self.btn_delay_plus_10 = Gtk.Button(label="+1")
        self.btn_delay_plus_10.set_tooltip_text("Increase delay (+1 ms)")
        self.btn_delay_plus_10.add_css_class("delay-btn")
        self.btn_delay_plus_10.connect("clicked", lambda b: self._step_delay(+1.0))
        delay_box.append(self.btn_delay_plus_10)

        self.btn_delay_zero = Gtk.Button(label="0")
        self.btn_delay_zero.set_tooltip_text("Reset delay (0 ms)")
        self.btn_delay_zero.add_css_class("delay-reset-btn")
        self.btn_delay_zero.connect("clicked", lambda b: self._reset_delay())
        delay_box.append(self.btn_delay_zero)

        # Headless scale to preserve test compatibility
        self.delay_scale = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, -300.0, 500.0, 1.0)
        self.delay_scale.set_value(self.channel.delay_ms)
        self.delay_scale.connect("value-changed", self._on_delay_changed)

        controls_row.append(delay_box)

        # 2D. Pulsante di test
        self.test_btn = Gtk.Button(label=" Test")
        self.test_btn.set_icon_name("audio-volume-high-symbolic")
        self.test_btn.add_css_class("flat")
        self.test_btn.add_css_class("strip-test-btn")
        self.test_btn.set_valign(Gtk.Align.CENTER)
        self.test_btn.set_size_request(90, -1)
        self.test_btn.connect("clicked", self._on_test_clicked)
        controls_row.append(self.test_btn)

        # 2E. Toggle per on/off dispositivo
        self.enable_switch = Gtk.Switch()
        self.enable_switch.set_valign(Gtk.Align.CENTER)
        self.enable_switch.connect("notify::active", self._on_enable_toggled)
        controls_row.append(self.enable_switch)

        self.append(controls_row)

    def _setup_role_popover(self):
        popover = Gtk.Popover()
        menu_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
        menu_box.set_margin_top(6)
        menu_box.set_margin_bottom(6)
        menu_box.set_margin_start(6)
        menu_box.set_margin_end(6)

        roles = [
            ("Stereo", SpeakerRole.STEREO),
            ("Left Channel (L)", SpeakerRole.LEFT),
            ("Right Channel (R)", SpeakerRole.RIGHT),
            ("Subwoofer (LFE)", SpeakerRole.SUBWOOFER),
            ("Center", SpeakerRole.CENTER),
            ("Surround Left", SpeakerRole.SURROUND_LEFT),
            ("Surround Right", SpeakerRole.SURROUND_RIGHT),
            ("Excluded / Disabled", SpeakerRole.EXCLUDED),
        ]

        for label, role in roles:
            btn = Gtk.Button(label=label)
            btn.add_css_class("flat")
            btn.set_halign(Gtk.Align.FILL)
            btn.connect("clicked", self._make_role_handler(role, popover))
            menu_box.append(btn)

        popover.set_child(menu_box)
        self.role_btn.set_popover(popover)
        self._update_role_button_label()

    def _make_role_handler(self, role, popover):
        def _handler(b):
            popover.popdown()
            self._on_role_clicked(role)
        return _handler

    def _update_role_button_label(self):
        role_names = {
            SpeakerRole.STEREO: "Stereo",
            SpeakerRole.LEFT: "Left (L)",
            SpeakerRole.RIGHT: "Right (R)",
            SpeakerRole.SUBWOOFER: "Subwoofer",
            SpeakerRole.CENTER: "Center",
            SpeakerRole.SURROUND_LEFT: "Surround L",
            SpeakerRole.SURROUND_RIGHT: "Surround R",
            SpeakerRole.EXCLUDED: "Excluded",
            SpeakerRole.DISABLED: "Disabled"
        }
        lbl = role_names.get(self.channel.role, str(self.channel.role))
        self.role_btn.set_label(f"{lbl}  ▾")

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
        if is_active:
            self.remove_css_class("channel-card-disabled")
        else:
            self.add_css_class("channel-card-disabled")
        self._update_delay_badge()
        self._update_role_button_label()
        self._updating_ui = False

    def _on_enable_toggled(self, switch, param):
        if self._updating_ui:
            return
        active = switch.get_active()
        if active:
            self.channel.role = SpeakerRole.STEREO
            self.channel._acoustic_role = SpeakerRole.STEREO
            self.channel.mute = False
            if self.channel.volume_gain < 0.05:
                target_gain = 1.0
                self.channel.volume_gain = target_gain
                self._updating_ui = True
                self.vol_scale.set_value(target_gain)
                self.vol_badge.set_text(f"{int(target_gain * 100)}%")
                self._updating_ui = False
            if self.audio_service:
                self.audio_service.set_speaker_gain(self.channel.sink_name, self.channel.volume_gain, persist=False)
                if hasattr(self.audio_service, "set_speaker_mute"):
                    self.audio_service.set_speaker_mute(self.channel.sink_name, False)
        else:
            self.channel._acoustic_role = SpeakerRole.STEREO
            self.channel.role = SpeakerRole.EXCLUDED
            self.channel.mute = True
            if self.audio_service and hasattr(self.audio_service, "set_speaker_mute"):
                self.audio_service.set_speaker_mute(self.channel.sink_name, True)
        self._sync_state()
        if self._enable_debounce_id:
            GLib.source_remove(self._enable_debounce_id)
            self._enable_debounce_id = 0
        self.on_change()

    def _on_role_clicked(self, new_role: SpeakerRole):
        self.channel.role = new_role
        if new_role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED):
            self.channel._acoustic_role = new_role
        is_active = new_role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED)
        self.channel.mute = not is_active
        self.enable_switch.set_active(is_active)
        if self.audio_service and hasattr(self.audio_service, "set_speaker_mute"):
            self.audio_service.set_speaker_mute(self.channel.sink_name, not is_active)
        self._sync_state()
        if self._enable_debounce_id:
            GLib.source_remove(self._enable_debounce_id)
            self._enable_debounce_id = 0
        self.on_change()

    def _emit_enable_change(self):
        self._enable_debounce_id = 0
        self.on_change()
        return False

    def _on_vol_changed(self, scale):
        val = scale.get_value()
        self.channel.volume_gain = round(val, 2)
        self.vol_badge.set_text(f"{int(self.channel.volume_gain * 100)}%")
        if self.audio_service:
            self.audio_service.set_speaker_gain(self.channel.sink_name, self.channel.volume_gain, persist=False)
        if self._vol_debounce_id:
            GLib.source_remove(self._vol_debounce_id)
        self._vol_debounce_id = GLib.timeout_add(300, self._emit_vol_change)

    def _emit_vol_change(self):
        self._vol_debounce_id = 0
        self.on_change()
        return False

    def _update_delay_badge(self):
        d = self.channel.delay_ms
        sign = "+" if d > 0 else ""
        val_str = f"{sign}{d:.1f}ms" if d != 0.0 else "0.0ms"
        hw_lat = getattr(self.channel, "hardware_latency_ms", 0.0)
        if hw_lat > 0:
            self.delay_badge.set_text(f"{val_str}")
        else:
            self.delay_badge.set_text(f"{val_str}")

    def _step_delay(self, delta: float):
        new_val = max(-300.0, min(500.0, round(self.channel.delay_ms + delta, 1)))
        self.channel.delay_ms = new_val
        self._updating_ui = True
        self.delay_scale.set_value(new_val)
        self._updating_ui = False
        self._update_delay_badge()
        if self._delay_debounce_id:
            GLib.source_remove(self._delay_debounce_id)
        self._delay_debounce_id = GLib.timeout_add(150, self._emit_delay_change)

    def _reset_delay(self):
        self.channel.delay_ms = 0.0
        self._updating_ui = True
        self.delay_scale.set_value(0.0)
        self._updating_ui = False
        self._update_delay_badge()
        if self._delay_debounce_id:
            GLib.source_remove(self._delay_debounce_id)
        self._delay_debounce_id = GLib.timeout_add(150, self._emit_delay_change)

    def _on_delay_changed(self, scale):
        if self._updating_ui:
            return
        val = scale.get_value()
        self.channel.delay_ms = round(val, 1)
        self._update_delay_badge()
        if self._delay_debounce_id:
            GLib.source_remove(self._delay_debounce_id)
        self._delay_debounce_id = GLib.timeout_add(150, self._emit_delay_change)

    def _emit_delay_change(self):
        self._delay_debounce_id = 0
        self.on_change()
        return False

    def _setup_rename_popover(self):
        self.popover = Gtk.Popover()
        self.popover.set_parent(self.edit_btn)
        self.popover.set_autohide(True)
        self.edit_btn.get_popover = lambda: self.popover

        content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        content.set_margin_top(12)
        content.set_margin_bottom(12)
        content.set_margin_start(12)
        content.set_margin_end(12)
        content.set_size_request(240, -1)

        pop_title = Gtk.Label(label="Customize Label")
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
        self.name_entry.set_placeholder_text("e.g. Left Monitor, Subwoofer...")
        if self.channel.custom_name:
            self.name_entry.set_text(self.channel.custom_name)
        self.name_entry.connect("activate", lambda e: self._on_save_custom_name(self.popover))
        content.append(self.name_entry)

        btn_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        btn_box.set_halign(Gtk.Align.END)

        reset_btn = Gtk.Button(label="Reset")
        reset_btn.set_tooltip_text("Reset to default hardware name")
        reset_btn.add_css_class("flat")
        reset_btn.connect("clicked", lambda b: self._on_reset_custom_name(self.popover))
        btn_box.append(reset_btn)

        save_btn = Gtk.Button(label="Save")
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
            return f"{self.channel.custom_name}\n(Device: {hw_name})"
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
