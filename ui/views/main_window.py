"""Studio Mixing Console Application Window for Polifonia."""

import os
import sys
import time
import subprocess
import threading
import logging
from typing import Optional
import gi
gi.require_version('Gtk', '4.0')
gi.require_version('Adw', '1')
from gi.repository import Gtk, Adw, GLib, Gdk
from services.audio_service import AudioService
from storage.preset_manager import PresetManager
from core.models import SystemConfig, SpeakerRole, SpeakerConfig
from ui.views.speaker_card import SpeakerCard
from ui.views.routing_graph import RoutingGraphView
from ui.tray.tray_service import TrayService

logger = logging.getLogger("polifonia.main_window")


class MainWindow(Adw.ApplicationWindow):
    """Professional Studio Mixing Console for Polifonia."""

    def __init__(self, app, audio_service: AudioService, preset_manager: PresetManager):
        super().__init__(application=app, title="Polifonia Audio Studio")
        self.set_default_size(1080, 680)

        css_path = os.path.join(os.path.dirname(__file__), "..", "styles", "studio_theme.css")
        if os.path.exists(css_path):
            css_provider = Gtk.CssProvider()
            css_provider.load_from_path(css_path)
            display = Gdk.Display.get_default()
            if display:
                Gtk.StyleContext.add_provider_for_display(
                    display, css_provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
                )

        self.audio_service = audio_service
        self.preset_manager = preset_manager
        self.config: SystemConfig = self.preset_manager.load_config()
        self._speaker_cards = []
        self._master_vol_debounce_id = 0
        self._known_sink_names = {s.name for s in self.audio_service.get_available_sinks()}
        self._hotplug_proc: Optional[subprocess.Popen] = None
        self._hotplug_thread: Optional[threading.Thread] = None
        self._hotplug_debounce_id = 0
        self._sync_in_flight = False
        self._sync_pending = False

        self._sync_live_sinks()

        if self.config.is_active:
            active_speakers = [s for s in self.config.channels if s.role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED)]
            if active_speakers and not self.audio_service.activate_unison(self.config):
                self.config.is_active = False

        self._build_ui()

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

        self.connect("close-request", self._on_close_request)
        self._refresh_master_ui()
        self._start_hotplug_listener()

    def _persist_config(self):
        self.preset_manager.save_config(self.config)
        if self.preset_manager is getattr(self.audio_service, "storage", None):
            self.audio_service.update_config(self.config, persist=False)
        else:
            self.audio_service.update_config(self.config)

    def _trigger_bg_sync(self):
        if self._sync_in_flight:
            self._sync_pending = True
            return
        self._sync_in_flight = True

        def _worker():
            try:
                while True:
                    self._sync_pending = False
                    self.audio_service.sync_active_branches()
                    if not self._sync_pending:
                        break
            finally:
                self._sync_in_flight = False

        threading.Thread(target=_worker, daemon=True).start()

    def _sync_live_sinks(self, live_sinks=None, force: bool = False):
        if live_sinks is None:
            live_sinks = self.audio_service.get_available_sinks(force=force)
        self._known_sink_names = {s.name for s in live_sinks}
        saved_map = {c.sink_name: c for c in self.config.channels}

        for live in live_sinks:
            if live.name in saved_map:
                saved = saved_map[live.name]
                saved.sink_id = live.id
                saved.display_name = live.description
                saved.hardware_latency_ms = live.latency_ms
                saved.bus_type = live.bus_type
                if saved.role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED):
                    saved.role = SpeakerRole.STEREO
                    saved._acoustic_role = SpeakerRole.STEREO
            else:
                role = SpeakerRole.EXCLUDED if (live.is_internal or live.bus_type == "bluetooth") else SpeakerRole.STEREO
                spk = SpeakerConfig(
                    sink_id=live.id,
                    sink_name=live.name,
                    display_name=live.description,
                    role=role,
                    hardware_latency_ms=live.latency_ms,
                    bus_type=live.bus_type
                )
                spk._acoustic_role = SpeakerRole.STEREO
                saved_map[live.name] = spk

        self.config.channels = [c for c in saved_map.values() if c.sink_name != "combined"]
        self._persist_config()

    def _start_hotplug_listener(self):
        def _set_pdeathsig():
            try:
                import ctypes, signal
                ctypes.CDLL("libc.so.6").prctl(1, signal.SIGTERM)
            except Exception:
                pass

        def _listen():
            while not getattr(self, "_quitting", False):
                try:
                    self._hotplug_proc = subprocess.Popen(
                        ["pactl", "subscribe"],
                        stdout=subprocess.PIPE,
                        stderr=subprocess.DEVNULL,
                        text=True,
                        bufsize=1,
                        preexec_fn=_set_pdeathsig
                    )
                    for line in self._hotplug_proc.stdout:
                        if not line:
                            break
                        ll = line.lower()
                        if "sink-input" in ll or "source-output" in ll or "polifonia" in ll:
                            continue
                        if ("sink #" in ll or "card #" in ll) and ("new" in ll or "remove" in ll):
                            GLib.idle_add(self._trigger_hotplug_resync)
                    if self._hotplug_proc:
                        try:
                            self._hotplug_proc.wait(timeout=0.2)
                        except Exception:
                            pass
                except Exception:
                    pass
                if getattr(self, "_quitting", False):
                    break
                time.sleep(1.0)

        self._hotplug_thread = threading.Thread(target=_listen, daemon=True)
        self._hotplug_thread.start()

    def _trigger_hotplug_resync(self):
        if getattr(self.config, "is_active", False) or self.audio_service.is_running():
            threading.Thread(target=self.audio_service.reassert_master_default, daemon=True).start()
        if self._hotplug_debounce_id:
            GLib.source_remove(self._hotplug_debounce_id)
        self._hotplug_debounce_id = GLib.timeout_add(500, self._on_hotplug_event)
        return False

    def _on_hotplug_event(self):
        self._hotplug_debounce_id = 0
        def _bg_sync():
            try:
                live_sinks = self.audio_service.get_available_sinks(force=True)
                live_names = {s.name for s in live_sinks}
                if live_names == self._known_sink_names:
                    return
                self._known_sink_names = live_names
                self._sync_live_sinks(live_sinks, force=True)
                if getattr(self.config, "is_active", False) or self.audio_service.is_running():
                    self.audio_service.reassert_master_default()
                    self.audio_service.sync_active_branches(live_sinks=live_sinks)
                    self.audio_service.reassert_master_default()
                GLib.idle_add(self._apply_hotplug_ui_updates, live_sinks)
            except Exception as e:
                logger.error(f"Error in hotplug background sync: {e}")

        threading.Thread(target=_bg_sync, daemon=True).start()
        return False

    def _apply_hotplug_ui_updates(self, live_sinks):
        self._populate_rack(live_sinks)
        self._sync_tray_state(live_sinks=live_sinks)
        if hasattr(self, "routing_graph"):
            self.routing_graph.refresh_topology()
        return False

    def _build_ui(self):
        self.toast_overlay = Adw.ToastOverlay()
        self.set_content(self.toast_overlay)

        main_layout = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        self.toast_overlay.set_child(main_layout)

        header_bar = Adw.HeaderBar()
        rescan_btn = Gtk.Button.new_from_icon_name("view-refresh-symbolic")
        rescan_btn.set_tooltip_text("Scan PipeWire audio outputs")
        rescan_btn.connect("clicked", self._on_rescan_clicked)
        header_bar.pack_start(rescan_btn)
        main_layout.append(header_bar)

        # Master Control Bar
        master_bar = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=16)
        master_bar.add_css_class("master-header-box")

        master_gain_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
        master_gain_box.set_hexpand(True)
        master_gain_box.set_valign(Gtk.Align.CENTER)
        master_gain_box.set_halign(Gtk.Align.START)

        master_icon = Gtk.Image.new_from_icon_name("audio-volume-high-symbolic")
        master_icon.add_css_class("dim-label")
        master_gain_box.append(master_icon)

        master_lbl = Gtk.Label(label="Master Volume")
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

        # View Stack (Channel Deck vs Topology)
        self.view_stack = Adw.ViewStack()
        self.view_stack.set_vexpand(True)

        page_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        deck_header = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
        deck_header.set_margin_start(20)
        deck_header.set_margin_end(20)
        deck_header.set_margin_top(16)
        deck_header.set_margin_bottom(10)

        deck_title = Gtk.Label(label="Connected Audio Outputs (Direct Controls)")
        deck_title.add_css_class("deck-header-label")
        deck_title.set_hexpand(True)
        deck_title.set_halign(Gtk.Align.START)
        deck_header.append(deck_title)

        self.btn_auto_align = Gtk.Button()
        self.btn_auto_align.set_tooltip_text("Automatically synchronize and align all active speakers using microphone")
        self.btn_auto_align.add_css_class("auto-align-btn")
        btn_align_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
        self.icon_auto_align = Gtk.Image.new_from_icon_name("media-playlist-shuffle-symbolic")
        self.lbl_auto_align = Gtk.Label(label="Auto-Sync Speakers")
        btn_align_box.append(self.icon_auto_align)
        btn_align_box.append(self.lbl_auto_align)
        self.btn_auto_align.set_child(btn_align_box)
        self.btn_auto_align.connect("clicked", self._on_auto_align_all_clicked)
        deck_header.append(self.btn_auto_align)

        self.btn_reset_delays = Gtk.Button()
        self.btn_reset_delays.set_tooltip_text("Reset all manual delays (relies on automatic PipeWire hardware latency compensation)")
        self.btn_reset_delays.add_css_class("reset-delays-btn")
        btn_reset_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
        btn_reset_box.append(Gtk.Image.new_from_icon_name("edit-undo-symbolic"))
        btn_reset_box.append(Gtk.Label(label="Reset Delays"))
        self.btn_reset_delays.set_child(btn_reset_box)
        self.btn_reset_delays.connect("clicked", self._on_reset_delays_clicked)
        deck_header.append(self.btn_reset_delays)

        page_box.append(deck_header)

        scroll_window = Gtk.ScrolledWindow()
        scroll_window.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        scroll_window.set_vexpand(True)
        scroll_window.add_css_class("rack-scroll-window")

        self.rack_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        self.rack_box.set_margin_start(20)
        self.rack_box.set_margin_end(20)
        self.rack_box.set_margin_bottom(16)
        self.rack_box.set_halign(Gtk.Align.FILL)
        scroll_window.set_child(self.rack_box)
        page_box.append(scroll_window)

        page_mixer = self.view_stack.add_titled(page_box, "mixer", "Mixer Console")
        page_mixer.set_icon_name("view-grid-symbolic")

        self.routing_graph = RoutingGraphView(
            audio_service=self.audio_service,
            config=self.config,
            on_test_speaker=self._on_test_speaker,
            on_toggle_unison=self._on_toggle_unison_clicked
        )
        page_graph = self.view_stack.add_titled(self.routing_graph, "routing", "Signal Flow")
        page_graph.set_icon_name("network-workgroup-symbolic")

        self.view_switcher = Adw.ViewSwitcher(stack=self.view_stack, policy=Adw.ViewSwitcherPolicy.WIDE)
        header_bar.set_title_widget(self.view_switcher)
        self.view_stack.connect("notify::visible-child-name", self._on_view_changed)
        main_layout.append(self.view_stack)

        self._populate_rack()

        # Footer Bar
        footer_bar = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=16)
        footer_bar.add_css_class("footer-dsp-bar")

        engine_info = Gtk.Label(label="PipeWire Audio Engine • Phase synchronization active")
        engine_info.add_css_class("dim-label")
        engine_info.set_valign(Gtk.Align.CENTER)
        engine_info.set_halign(Gtk.Align.START)
        footer_bar.append(engine_info)

        default_sink_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        default_sink_box.set_valign(Gtk.Align.CENTER)
        default_sink_box.set_hexpand(True)
        default_sink_box.set_halign(Gtk.Align.END)

        def_lbl = Gtk.Label(label="Set as default output")
        def_lbl.add_css_class("dim-label")
        default_sink_box.append(def_lbl)

        self.default_switch = Gtk.Switch()
        self.default_switch.set_active(self.config.set_as_default)
        self.default_switch.connect("notify::active", self._on_default_toggled)
        default_sink_box.append(self.default_switch)

        footer_bar.append(default_sink_box)
        main_layout.append(footer_bar)

    def _populate_rack(self, live_sinks=None):
        live_names = {s.name for s in live_sinks} if live_sinks is not None else getattr(self, "_known_sink_names", set())
        displayed = [ch for ch in self.config.channels if not live_names or ch.sink_name in live_names]
        displayed_names = {ch.sink_name for ch in displayed}

        kept = []
        for card in self._speaker_cards:
            if card.channel.sink_name in displayed_names:
                card._sync_state()
                kept.append(card)
            else:
                self.rack_box.remove(card)
        self._speaker_cards = kept

        existing = {c.channel.sink_name for c in self._speaker_cards}
        for ch in displayed:
            if ch.sink_name not in existing:
                card = SpeakerCard(ch, self._on_config_changed, self._on_test_speaker, audio_service=self.audio_service)
                self.rack_box.append(card)
                self._speaker_cards.append(card)

    def _serialize_channels_for_tray(self, live_sinks=None) -> list:
        live_names = {s.name for s in live_sinks} if live_sinks is not None else getattr(self, "_known_sink_names", set())
        return [
            {
                "sink_id": ch.sink_id,
                "sink_name": ch.sink_name,
                "display_name": ch.custom_name or ch.display_name or ch.sink_name,
                "role": ch.role.value if hasattr(ch.role, "value") else str(ch.role),
                "volume_gain": ch.volume_gain,
                "delay_ms": ch.delay_ms,
                "is_enabled": ch.role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED),
            }
            for ch in self.config.channels
            if not live_names or ch.sink_name in live_names
        ]

    def _sync_tray_state(self, is_active=None, window_visible=None, live_sinks=None):
        if not hasattr(self, "tray_service"):
            return
        active = (self.config.is_active or self.audio_service.is_running()) if is_active is None else is_active
        vis = self.is_visible() if window_visible is None else window_visible
        self.tray_service.sync_state(active, self.config.master_volume, vis, self._serialize_channels_for_tray(live_sinks))

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
        if hasattr(self, "routing_graph"):
            self.routing_graph.refresh_topology()

    def _on_view_changed(self, stack, param):
        if stack.get_visible_child_name() == "routing":
            self.routing_graph.refresh_topology()
            self.routing_graph.start_polling()
        else:
            self.routing_graph.stop_polling()

    def _on_close_request(self, window):
        if hasattr(self, "routing_graph"):
            self.routing_graph.stop_polling()
        self.hide()
        self._sync_tray_state(window_visible=False)
        return True

    def _toggle_window_visibility(self):
        if self.is_visible():
            self._on_close_request(self)
        else:
            self.present()
            self._sync_tray_state(window_visible=True)
            if hasattr(self, "routing_graph") and self.view_stack.get_visible_child_name() == "routing":
                self.routing_graph.refresh_topology()
                self.routing_graph.start_polling()

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
                ch._acoustic_role = SpeakerRole.STEREO
                ch.role = SpeakerRole.STEREO if enabled else SpeakerRole.EXCLUDED
                if enabled and ch.volume_gain < 0.05:
                    ch.volume_gain = 1.0
                break
        for card in self._speaker_cards:
            if card.channel.sink_name == sink_name:
                card._sync_state()
                break
        self._on_config_changed()

    def _on_tray_set_channel_role(self, sink_name: str, role_str: str):
        target_role = getattr(SpeakerRole, role_str.upper(), SpeakerRole.STEREO)
        for i, ch in enumerate(self.config.channels):
            if ch.sink_name == sink_name:
                ch.role = target_role
                if i < len(self._speaker_cards):
                    self._speaker_cards[i]._sync_state()
                break
        self._on_config_changed()

    def _on_tray_set_channel_volume(self, sink_name: str, volume: float):
        for i, ch in enumerate(self.config.channels):
            if ch.sink_name == sink_name:
                ch.volume_gain = volume
                if i < len(self._speaker_cards):
                    card = self._speaker_cards[i]
                    card.vol_scale.set_value(volume)
                    card.vol_badge.set_text(f"{int(volume * 100)}%")
                break
        self._on_config_changed()

    def _on_tray_test_channel(self, sink_name: str, display_name: str):
        self._on_test_speaker(sink_name, display_name)

    def _quit_application(self):
        self._quitting = True
        if self._hotplug_proc:
            try:
                self._hotplug_proc.terminate()
                self._hotplug_proc.wait(timeout=0.2)
            except Exception:
                pass
            self._hotplug_proc = None
        if hasattr(self, "tray_service"):
            self.tray_service.stop()
        if hasattr(self, "routing_graph"):
            self.routing_graph.stop_polling()
        self.audio_service.stop_unison_sink(unload_master=True)
        app = self.get_application()
        if app:
            app.quit()
        else:
            sys.exit(0)

    def _on_config_changed(self):
        self._persist_config()
        active_speakers = [s for s in self.config.channels if s.role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED)]
        if self.config.is_active and not self.audio_service.is_running() and active_speakers:
            def _auto_start():
                if self.audio_service.activate_unison(self.config):
                    if "unittest" in sys.modules:
                        self._refresh_master_ui()
                    else:
                        GLib.idle_add(self._refresh_master_ui)
            if "unittest" in sys.modules:
                _auto_start()
            else:
                threading.Thread(target=_auto_start, daemon=True).start()
        elif self.config.is_active or self.audio_service.is_running():
            self._trigger_bg_sync()
        self._sync_tray_state()
        if hasattr(self, "routing_graph") and hasattr(self, "view_stack") and self.view_stack.get_visible_child_name() == "routing":
            self.routing_graph.refresh_topology()

    def _on_master_volume_changed(self, scale):
        self.config.master_volume = round(scale.get_value(), 2)
        self.master_vol_badge.set_text(f"{int(self.config.master_volume * 100)}%")
        self.audio_service.set_master_gain(self.config.master_volume, persist=False)
        if self._master_vol_debounce_id:
            GLib.source_remove(self._master_vol_debounce_id)
        self._master_vol_debounce_id = GLib.timeout_add(300, self._emit_master_vol_save)

    def _emit_master_vol_save(self):
        self._master_vol_debounce_id = 0
        self._persist_config()
        self._sync_tray_state()
        return False

    def _on_default_toggled(self, switch, param):
        self.config.set_as_default = switch.get_active()
        self._on_config_changed()

    def _on_test_speaker(self, target, display_name=None):
        self.audio_service.test_tone(target)
        self.toast_overlay.add_toast(Adw.Toast.new(f"Test signal sent to: {display_name or target}"))

    def _on_toggle_unison_clicked(self, btn):
        is_test = "unittest" in sys.modules
        if self.config.is_active or self.audio_service.is_running():
            def _deact():
                try:
                    self.audio_service.deactivate_unison()
                    self.config.is_active = False
                    self.preset_manager.save_config(self.config)
                finally:
                    if is_test:
                        self._on_unison_toggled_done(False, "Unison Engine deactivated.")
                    else:
                        GLib.idle_add(self._on_unison_toggled_done, False, "Unison Engine deactivated.")
            if is_test:
                _deact()
            else:
                self.master_toggle_btn.set_sensitive(False)
                threading.Thread(target=_deact, daemon=True).start()
        else:
            def _act():
                try:
                    active_speakers = [s for s in self.config.channels if s.role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED)]
                    if not active_speakers:
                        self.config.is_active = True
                        self.preset_manager.save_config(self.config)
                        msg, success = "Unison Engine armed: activate an output channel to play audio.", True
                    else:
                        success = self.audio_service.activate_unison(self.config)
                        if success:
                            self.config.is_active = True
                            self.preset_manager.save_config(self.config)
                            msg = "Unison Engine activated successfully!"
                        else:
                            msg = "Activation error."
                    if is_test:
                        self._on_unison_toggled_done(success, msg)
                    else:
                        GLib.idle_add(self._on_unison_toggled_done, success, msg)
                except Exception as e:
                    if is_test:
                        self._on_unison_toggled_done(False, f"Activation error: {e}")
                    else:
                        GLib.idle_add(self._on_unison_toggled_done, False, f"Activation error: {e}")
            if is_test:
                _act()
            else:
                self.master_toggle_btn.set_sensitive(False)
                threading.Thread(target=_act, daemon=True).start()

    def _on_unison_toggled_done(self, is_active: bool, toast_msg: str):
        self.master_toggle_btn.set_sensitive(True)
        self._refresh_master_ui()
        self.toast_overlay.add_toast(Adw.Toast.new(toast_msg))
        return False

    def _on_rescan_clicked(self, btn):
        self._sync_live_sinks(force=True)
        self._populate_rack()
        if self.config.is_active or self.audio_service.is_running():
            self._trigger_bg_sync()
        self._sync_tray_state()
        if hasattr(self, "routing_graph"):
            self.routing_graph.refresh_topology()
        self.toast_overlay.add_toast(Adw.Toast.new("PipeWire audio devices refreshed."))

    def _on_auto_align_all_clicked(self, btn):
        self.btn_auto_align.set_sensitive(False)
        self.lbl_auto_align.set_text("Calibrating...")
        self.toast_overlay.add_toast(Adw.Toast.new("Acoustic synchronization in progress... please remain quiet (4s)"))

        def _worker():
            success, msg, delays = self.audio_service.auto_align_all_speakers(
                progress_cb=lambda m: GLib.idle_add(lambda: self.lbl_auto_align.set_text(m[:24]))
            )
            GLib.idle_add(self._on_auto_align_finished, success, msg, delays)

        threading.Thread(target=_worker, daemon=True).start()

    def _on_auto_align_finished(self, success: bool, msg: str, delays: dict):
        self.btn_auto_align.set_sensitive(True)
        self.lbl_auto_align.set_text("Auto-Sync Speakers")
        self._populate_rack()
        toast_txt = "✓ Speakers synchronized to the millisecond!" if success else f"Notice: {msg}"
        self.toast_overlay.add_toast(Adw.Toast.new(toast_txt))
        return False

    def _on_reset_delays_clicked(self, btn):
        self.audio_service.reset_all_delays()
        self._populate_rack()
        self.toast_overlay.add_toast(Adw.Toast.new("All speaker delays reset to 0.0 ms (Hardware compensation active)"))
        return False
