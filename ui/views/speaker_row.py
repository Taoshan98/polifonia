"""Modern GTK4 / Libadwaita row widget for speaker configuration."""

import gi
gi.require_version('Gtk', '4.0')
gi.require_version('Adw', '1')
from gi.repository import Gtk, Adw, GLib
from core.models import SpeakerChannel, SpeakerRole


class SpeakerRow(Adw.ExpanderRow):
    def __init__(self, channel: SpeakerChannel, on_change_callback, on_test_callback):
        super().__init__()
        self.channel = channel
        self.on_change = on_change_callback
        self.on_test = on_test_callback

        self.set_title(channel.display_name or channel.sink_name)
        self.set_subtitle(f"ID: {channel.sink_id} | Driver: {channel.sink_name}")
        self.set_show_enable_switch(True)

        # Initial state of the switch
        is_active = channel.role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED)
        self.set_enable_expansion(is_active)

        # Icon
        self._update_icon()

        # Connect switch toggled
        self.connect("notify::enable-expansion", self._on_enable_toggled)

        # Build Sub-Controls
        self._build_controls()

    def _update_icon(self):
        icon_name = "audio-speakers-symbolic"
        name_lower = (self.channel.sink_name or "").lower()
        if "hdmi" in name_lower:
            icon_name = "video-display-symbolic"
        elif "usb" in name_lower:
            icon_name = "audio-card-symbolic"
        elif "pci" in name_lower or "speaker" in name_lower:
            icon_name = "audio-headset-symbolic"
        self.set_icon_name(icon_name)

    def _build_controls(self):
        # 1. Role Selection (Left, Right, Center/Sub, Stereo, Mirror)
        self.role_row = Adw.ComboRow()
        self.role_row.set_title("Ruolo Canale")
        self.role_row.set_subtitle("Assegna la posizione spaziale o la funzione audio")
        
        self.role_options = [
            (SpeakerRole.STEREO, "Stereo (Completo L+R)"),
            (SpeakerRole.LEFT, "Sinistra (Left Satellite)"),
            (SpeakerRole.RIGHT, "Destra (Right Satellite)"),
            (SpeakerRole.SUBWOOFER, "Subwoofer / Cassa Bassi (LFE)"),
            (SpeakerRole.CENTER, "Centrale (Mono / Dialoghi)"),
            (SpeakerRole.SURROUND_LEFT, "Surround Sinistro"),
            (SpeakerRole.SURROUND_RIGHT, "Surround Destro"),
        ]
        
        model = Gtk.StringList()
        selected_idx = 0
        for i, (role, label) in enumerate(self.role_options):
            model.append(label)
            if self.channel.role == role:
                selected_idx = i
                
        self.role_row.set_model(model)
        self.role_row.set_selected(selected_idx)
        self.role_row.connect("notify::selected", self._on_role_selected)
        self.add_row(self.role_row)

        # 2. Time Alignment / Delay Slider
        self.delay_row = Adw.ActionRow()
        self.delay_row.set_title("Allineamento Temporale (Ritardo)")
        self.delay_row.set_subtitle(f"{self.channel.delay_ms:.1f} ms")

        self.delay_scale = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, 0.0, 150.0, 0.5)
        self.delay_scale.set_value(self.channel.delay_ms)
        self.delay_scale.set_hexpand(True)
        self.delay_scale.set_size_request(200, -1)
        self.delay_scale.connect("value-changed", self._on_delay_changed)
        self.delay_row.add_suffix(self.delay_scale)
        self.add_row(self.delay_row)

        # 3. Channel Gain / Calibration Slider
        self.gain_row = Adw.ActionRow()
        self.gain_row.set_title("Calibrazione Guadagno (Volume Singolo)")
        self.gain_row.set_subtitle(f"{int(self.channel.gain * 100)}%")

        self.gain_scale = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, 0.0, 1.5, 0.05)
        self.gain_scale.set_value(self.channel.gain)
        self.gain_scale.set_hexpand(True)
        self.gain_scale.set_size_request(200, -1)
        self.gain_scale.connect("value-changed", self._on_gain_changed)
        self.gain_row.add_suffix(self.gain_scale)
        self.add_row(self.gain_row)

        # 4. Test Tone Button
        self.test_row = Adw.ActionRow()
        self.test_row.set_title("Verifica Uscita")
        self.test_row.set_subtitle("Invia un segnale audio di test su questo singolo monitor/altoparlante")

        test_btn = Gtk.Button.new_from_icon_name("audio-volume-high-symbolic")
        test_btn.set_tooltip_text("Riproduci tono di test (Beep)")
        test_btn.set_valign(Gtk.Align.CENTER)
        test_btn.add_css_class("suggested-action")
        test_btn.connect("clicked", lambda b: self.on_test(self.channel.sink_name or self.channel.sink_id, self.channel.display_name or self.channel.sink_name))
        self.test_row.add_suffix(test_btn)
        self.add_row(self.test_row)

    def _on_enable_toggled(self, widget, param):
        enabled = self.get_enable_expansion()
        if not enabled:
            self.channel.role = SpeakerRole.EXCLUDED
        else:
            idx = self.role_row.get_selected()
            if 0 <= idx < len(self.role_options):
                self.channel.role = self.role_options[idx][0]
        self.on_change()

    def _on_role_selected(self, widget, param):
        idx = self.role_row.get_selected()
        if 0 <= idx < len(self.role_options):
            self.channel.role = self.role_options[idx][0]
        self.on_change()

    def _on_delay_changed(self, scale):
        val = scale.get_value()
        self.channel.delay_ms = round(val, 1)
        self.delay_row.set_subtitle(f"{self.channel.delay_ms:.1f} ms")
        self.on_change()

    def _on_gain_changed(self, scale):
        val = scale.get_value()
        self.channel.gain = round(val, 2)
        self.gain_row.set_subtitle(f"{int(self.channel.gain * 100)}%")
        self.on_change()

