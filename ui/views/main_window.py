"""Main Horizontal Studio Mixing Console Application Window."""

import os
import gi
gi.require_version('Gtk', '4.0')
gi.require_version('Adw', '1')
from gi.repository import Gtk, Adw, GLib, Gio, Gdk
from services.audio_service import AudioService
from storage.preset_manager import PresetManager
from core.models import SystemConfig, SpeakerRole, SpeakerConfig
from ui.views.speaker_card import SpeakerCard
from ui.tray.tray_service import TrayService


class MainWindow(Adw.ApplicationWindow):
    """Professional Horizontal Studio Mixing Console for Polifonia."""

    def __init__(self, app, audio_service: AudioService, preset_manager: PresetManager):
        super().__init__(application=app, title="Polifonia Audio Studio")
        self.set_default_size(1080, 680)

        # Load Custom Cross-Desktop Studio Theme
        css_provider = Gtk.CssProvider()
        css_path = os.path.join(os.path.dirname(__file__), "..", "styles", "studio_theme.css")
        if os.path.exists(css_path):
            css_provider.load_from_path(css_path)
            display = Gdk.Display.get_default()
            if display:
                Gtk.StyleContext.add_provider_for_display(
                    display,
                    css_provider,
                    Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
                )

        self.audio_service = audio_service
        self.preset_manager = preset_manager
        self.config: SystemConfig = self.preset_manager.load_config()
        self._speaker_cards = []

        # Merge live sinks with saved configuration
        self._sync_live_sinks()

        # If saved as active, auto-activate the audio unison engine
        if self.config.is_active:
            success = self.audio_service.activate_unison(self.config)
            if not success:
                self.config.is_active = False

        self._build_ui()

        # Initialize System Tray Companion Service
        self.tray_service = TrayService(
            on_toggle_window=self._toggle_window_visibility,
            on_toggle_unison=self._toggle_unison_from_tray,
            on_set_volume=self._set_volume_from_tray,
            on_toggle_channel_enable=self._on_tray_toggle_channel_enable,
            on_set_channel_role=self._on_tray_set_channel_role,
            on_set_channel_volume=self._on_tray_set_channel_volume,
            on_test_channel=self._on_tray_test_channel,
            on_quit=self._quit_application
        )
        self.tray_service.start()

        # Intercept close request to keep running in tray
        self.connect("close-request", self._on_close_request)

        self._refresh_master_ui()

    def _sync_live_sinks(self):
        live_sinks = self.audio_service.get_available_sinks()
        saved_channels_map = {c.sink_name: c for c in self.config.channels}
        
        updated_channels = []
        for live in live_sinks:
            if live.name in saved_channels_map:
                saved = saved_channels_map[live.name]
                saved.sink_id = live.id
                saved.display_name = live.description
                updated_channels.append(saved)
            else:
                role = SpeakerRole.EXCLUDED if (live.is_internal or "pci" in live.name.lower() or "speaker" in live.description.lower()) else SpeakerRole.LEFT
                spk = SpeakerConfig(
                    sink_id=live.id,
                    sink_name=live.name,
                    display_name=live.description,
                    role=role
                )
                updated_channels.append(spk)

        self.config.channels = updated_channels

    def _build_ui(self):
        # Toast overlay for notifications
        self.toast_overlay = Adw.ToastOverlay()
        self.set_content(self.toast_overlay)

        # Main Vertical Container
        main_layout = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        self.toast_overlay.set_child(main_layout)

        # 1. Native HeaderBar
        header_bar = Adw.HeaderBar()
        title_widget = Adw.WindowTitle(title="Polifonia Audio Studio", subtitle="Multi-Channel Mixing Console & Unison Engine")
        header_bar.set_title_widget(title_widget)

        # Hardware Rescan Button
        rescan_btn = Gtk.Button.new_from_icon_name("view-refresh-symbolic")
        rescan_btn.set_tooltip_text("Rescan PipeWire Audio Endpoints")
        rescan_btn.connect("clicked", self._on_rescan_clicked)
        header_bar.pack_start(rescan_btn)

        # Presets Menu Button
        preset_btn = Gtk.MenuButton()
        preset_btn.set_icon_name("document-open-recent-symbolic")
        preset_btn.set_tooltip_text("Profiles & Audio Presets")
        header_bar.pack_start(preset_btn)

        main_layout.append(header_bar)

        # 2. Master Control Bar (Master Gain, Big Neon Master Button)
        master_bar = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=16)
        master_bar.add_css_class("master-header-box")

        # Master Gain Fader
        master_gain_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
        master_gain_box.set_hexpand(True)
        master_gain_box.set_valign(Gtk.Align.CENTER)
        master_gain_box.set_halign(Gtk.Align.START)

        master_lbl = Gtk.Label(label="MASTER VOLUME")
        master_lbl.add_css_class("strip-section-label")
        master_gain_box.append(master_lbl)

        self.master_scale = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, 0.0, 1.5, 0.05)
        self.master_scale.set_size_request(260, -1)
        self.master_scale.set_value(self.config.master_volume)
        self.master_scale.connect("value-changed", self._on_master_volume_changed)
        master_gain_box.append(self.master_scale)

        self.master_vol_badge = Gtk.Label(label=f"{int(self.config.master_volume * 100)}%")
        self.master_vol_badge.add_css_class("vol-badge")
        master_gain_box.append(self.master_vol_badge)

        master_bar.append(master_gain_box)

        # Big Neon Master Toggle Button (Icon + Label)
        self.master_toggle_btn = Gtk.Button()
        self.master_toggle_btn.add_css_class("master-toggle-btn-off")
        self.master_toggle_btn.set_valign(Gtk.Align.CENTER)
        self.master_toggle_btn.connect("clicked", self._on_toggle_unison_clicked)

        btn_content = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        self.master_toggle_icon = Gtk.Image.new_from_icon_name("media-playback-start-symbolic")
        self.master_toggle_label = Gtk.Label(label="ACTIVATE UNISON")
        btn_content.append(self.master_toggle_icon)
        btn_content.append(self.master_toggle_label)
        self.master_toggle_btn.set_child(btn_content)

        master_bar.append(self.master_toggle_btn)

        main_layout.append(master_bar)

        # 3. Central Horizontal Channel Strips Rack
        scroll_window = Gtk.ScrolledWindow()
        scroll_window.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.NEVER)
        scroll_window.set_vexpand(True)
        scroll_window.add_css_class("rack-scroll-window")

        self.rack_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=14)
        self.rack_box.set_margin_start(16)
        self.rack_box.set_margin_end(16)
        self.rack_box.set_margin_top(12)
        self.rack_box.set_margin_bottom(12)
        self.rack_box.set_halign(Gtk.Align.START)
        
        scroll_window.set_child(self.rack_box)
        main_layout.append(scroll_window)

        # Populate Channel Strips
        self._populate_rack()

        # 4. Footer Tools & Engine Bar
        footer_bar = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=20)
        footer_bar.add_css_class("footer-dsp-bar")

        # Engine Status Info
        engine_info = Gtk.Label(label="PipeWire Native • Zero-Latency Multi-Channel Unison")
        engine_info.add_css_class("strip-section-label")
        engine_info.set_valign(Gtk.Align.CENTER)
        engine_info.set_halign(Gtk.Align.START)
        footer_bar.append(engine_info)

        # Default System Sink Toggle
        default_sink_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        default_sink_box.add_css_class("dsp-box")
        default_sink_box.set_valign(Gtk.Align.CENTER)
        default_sink_box.set_hexpand(True)
        default_sink_box.set_halign(Gtk.Align.END)

        def_lbl = Gtk.Label(label="SET AS SYSTEM DEFAULT SINK")
        def_lbl.add_css_class("strip-section-label")
        default_sink_box.append(def_lbl)

        self.default_switch = Gtk.Switch()
        self.default_switch.set_active(self.config.set_as_default)
        self.default_switch.connect("notify::active", self._on_default_toggled)
        default_sink_box.append(self.default_switch)

        footer_bar.append(default_sink_box)
        main_layout.append(footer_bar)

    def _populate_rack(self):
        # Clear existing cards
        while self.rack_box.get_first_child():
            self.rack_box.remove(self.rack_box.get_first_child())
        self._speaker_cards.clear()

        # Add speaker channel strip cards
        for ch in self.config.channels:
            card = SpeakerCard(ch, self._on_config_changed, self._on_test_speaker)
            self.rack_box.append(card)
            self._speaker_cards.append(card)

    def _serialize_channels_for_tray(self) -> list:
        res = []
        for ch in self.config.channels:
            is_enabled = ch.role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED)
            role_val = ch.role.value if hasattr(ch.role, "value") else str(ch.role)
            res.append({
                "sink_id": ch.sink_id,
                "sink_name": ch.sink_name,
                "display_name": ch.display_name or ch.sink_name,
                "role": role_val,
                "volume_gain": ch.volume_gain,
                "delay_ms": ch.delay_ms,
                "is_enabled": is_enabled
            })
        return res

    def _sync_tray_state(self, is_active=None, window_visible=None):
        if not hasattr(self, "tray_service"):
            return
        active = (self.config.is_active or self.audio_service.is_running()) if is_active is None else is_active
        vis = self.is_visible() if window_visible is None else window_visible
        self.tray_service.sync_state(
            active,
            self.config.master_volume,
            vis,
            self._serialize_channels_for_tray()
        )

    def _refresh_master_ui(self):
        is_active = self.config.is_active or self.audio_service.is_running()
        if is_active:
            self.master_toggle_icon.set_from_icon_name("media-playback-stop-symbolic")
            self.master_toggle_label.set_text("DEACTIVATE")
            self.master_toggle_btn.remove_css_class("master-toggle-btn-off")
            self.master_toggle_btn.add_css_class("master-toggle-btn-on")
        else:
            self.master_toggle_icon.set_from_icon_name("media-playback-start-symbolic")
            self.master_toggle_label.set_text("ACTIVATE UNISON")
            self.master_toggle_btn.remove_css_class("master-toggle-btn-on")
            self.master_toggle_btn.add_css_class("master-toggle-btn-off")

        self._sync_tray_state(is_active=is_active)

    def _on_close_request(self, window):
        """Hides the window instead of closing so background audio keeps playing."""
        self.hide()
        self._sync_tray_state(window_visible=False)
        return True  # Stop default window destruction

    def _toggle_window_visibility(self):
        if self.is_visible():
            self.hide()
            self._sync_tray_state(window_visible=False)
        else:
            self.present()
            self._sync_tray_state(window_visible=True)

    def _toggle_unison_from_tray(self):
        self._on_toggle_unison_clicked(None)

    def _set_volume_from_tray(self, volume: float):
        self.config.master_volume = volume
        self.master_scale.set_value(volume)
        self.master_vol_badge.set_text(f"{int(volume * 100)}%")
        self.audio_service.set_master_gain(volume)
        self.preset_manager.save_config(self.config)
        self._sync_tray_state()

    def _on_tray_toggle_channel_enable(self, sink_name: str, enabled: bool):
        for ch in self.config.channels:
            if ch.sink_name == sink_name:
                if enabled:
                    ch.role = SpeakerRole.STEREO if ch.role in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED) else ch.role
                else:
                    ch.role = SpeakerRole.EXCLUDED
                break
        self._populate_rack()
        self._on_config_changed()

    def _on_tray_set_channel_role(self, sink_name: str, role_str: str):
        role_map = {
            "LEFT": SpeakerRole.LEFT,
            "RIGHT": SpeakerRole.RIGHT,
            "CENTER": SpeakerRole.CENTER,
            "SUBWOOFER": SpeakerRole.SUBWOOFER,
            "STEREO": SpeakerRole.STEREO,
            "SURROUND_LEFT": SpeakerRole.SURROUND_LEFT,
            "EXCLUDED": SpeakerRole.EXCLUDED
        }
        target_role = role_map.get(role_str.upper(), SpeakerRole.STEREO)
        for ch in self.config.channels:
            if ch.sink_name == sink_name:
                ch.role = target_role
                break
        self._populate_rack()
        self._on_config_changed()

    def _on_tray_set_channel_volume(self, sink_name: str, volume: float):
        for ch in self.config.channels:
            if ch.sink_name == sink_name:
                ch.volume_gain = volume
                break
        self._populate_rack()
        self._on_config_changed()

    def _on_tray_test_channel(self, sink_name: str, display_name: str):
        self._on_test_speaker(sink_name, display_name)

    def _quit_application(self):
        """Cleanly quits Polifonia and stops audio services."""
        if hasattr(self, "tray_service"):
            self.tray_service.stop()
        self.audio_service.deactivate_unison()
        app = self.get_application()
        if app:
            app.quit()
        else:
            sys.exit(0)

    def _on_config_changed(self):
        self.preset_manager.save_config(self.config)
        if self.config.is_active or self.audio_service.is_running():
            self.audio_service.sync_active_branches()
        self._sync_tray_state()

    def _on_master_volume_changed(self, scale):
        val = scale.get_value()
        self.config.master_volume = round(val, 2)
        self.master_vol_badge.set_text(f"{int(self.config.master_volume * 100)}%")
        self.audio_service.set_master_gain(self.config.master_volume)
        self._sync_tray_state()

    def _on_default_toggled(self, switch, param):
        self.config.set_as_default = switch.get_active()
        self._on_config_changed()

    def _on_test_speaker(self, target, display_name=None):
        self.audio_service.test_tone(target)
        name_str = display_name or str(target)
        toast = Adw.Toast.new(f"Test tone sent to: {name_str}")
        self.toast_overlay.add_toast(toast)

    def _on_toggle_unison_clicked(self, btn):
        if self.config.is_active or self.audio_service.is_running():
            self.audio_service.deactivate_unison()
            self.config.is_active = False
            self.preset_manager.save_config(self.config)
            self._refresh_master_ui()
            self.toast_overlay.add_toast(Adw.Toast.new("Unison engine deactivated."))
        else:
            success = self.audio_service.activate_unison(self.config)
            if success:
                self.config.is_active = True
                self.preset_manager.save_config(self.config)
                self._refresh_master_ui()
                self.toast_overlay.add_toast(Adw.Toast.new("Unison engine activated successfully!"))
            else:
                self.toast_overlay.add_toast(Adw.Toast.new("No active channels selected."))

    def _on_rescan_clicked(self, btn):
        self._sync_live_sinks()
        self._populate_rack()
        self._sync_tray_state()
        self.toast_overlay.add_toast(Adw.Toast.new("Hardware audio devices refreshed."))
