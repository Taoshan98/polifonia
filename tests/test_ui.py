"""Unit tests for GTK4 UI components (MainWindow and SpeakerCard)."""

import unittest
from unittest.mock import MagicMock
import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Gtk, Adw, Gio

from core.models import SystemConfig, SpeakerConfig, SpeakerRole, AudioSink
from ui.views.speaker_card import SpeakerCard
from ui.views.main_window import MainWindow
from ui.views.routing_graph import RoutingGraphView, RoutingGraphCanvas
import cairo


class TestUI(unittest.TestCase):
    """Test suite for horizontal studio mixing console presentation layer."""

    @classmethod
    def setUpClass(cls):
        Adw.init()
        cls.app = Adw.Application(
            application_id="io.github.taoshan98.Polifonia.TestApp",
            flags=Gio.ApplicationFlags.FLAGS_NONE
        )

    def test_speaker_card_instantiation_and_callbacks(self):
        """Verify SpeakerCard widget configuration, delay/gain changes and role pills."""
        channel = SpeakerConfig(
            sink_id=55,
            sink_name="alsa_output.pci.hdmi_left",
            display_name="HDMI Monitor Left",
            role=SpeakerRole.LEFT,
            volume_gain=1.0,
            delay_ms=10.0
        )

        on_change_mock = MagicMock()
        on_test_mock = MagicMock()

        card = SpeakerCard(channel, on_change_mock, on_test_mock)

        # Title and enable state
        self.assertIn("HDMI Monitor Left", card.title_label.get_tooltip_text())
        self.assertTrue(card.enable_switch.get_active())

        # Test delay adjustment (debounced — verify value update is immediate, then flush debounce)
        card.delay_scale.set_value(22.5)
        self.assertEqual(channel.delay_ms, 22.5)
        card._emit_delay_change()  # Flush debounce timer
        self.assertTrue(on_change_mock.called)

        # Test gain adjustment (debounced — verify value update is immediate, then flush debounce)
        on_change_mock.reset_mock()
        card.vol_scale.set_value(1.2)
        self.assertEqual(channel.volume_gain, 1.2)
        card._emit_vol_change()  # Flush debounce timer
        self.assertTrue(on_change_mock.called)

        # Test role pill button click
        on_change_mock.reset_mock()
        card._on_role_clicked(SpeakerRole.SUBWOOFER)
        self.assertEqual(channel.role, SpeakerRole.SUBWOOFER)
        card._emit_enable_change()  # Flush debounce timer
        self.assertTrue(on_change_mock.called)

        # Test disable switch toggling
        on_change_mock.reset_mock()
        card.enable_switch.set_active(False)
        self.assertEqual(channel.role, SpeakerRole.EXCLUDED)
        card._emit_enable_change()  # Flush debounce timer
        self.assertTrue(on_change_mock.called)

        # Test direct test tone callback invocation
        card.on_test(channel.sink_id, channel.display_name)
        on_test_mock.assert_called_with(55, "HDMI Monitor Left")

    def test_main_window_lifecycle(self):
        """Verify horizontal MainWindow construction, sink rack synchronization and unison toggle."""
        mock_audio_service = MagicMock()
        mock_audio_service.get_available_sinks.return_value = [
            AudioSink(id=10, name="alsa_output.pci.hdmi1", description="Left Display", media_class="Audio/Sink", is_internal=False),
            AudioSink(id=20, name="alsa_output.pci.hdmi2", description="Right Display", media_class="Audio/Sink", is_internal=False),
            AudioSink(id=30, name="alsa_output.usb.sub", description="USB Subwoofer", media_class="Audio/Sink", is_internal=False)
        ]
        mock_audio_service.is_running.return_value = False
        mock_audio_service.activate_unison.return_value = True

        mock_preset_manager = MagicMock()
        initial_cfg = SystemConfig(
            profile_name="Initial Setup",
            master_volume=1.0,
            crossover_enabled=False,
            crossover_freq=90,
            speakers={}
        )
        mock_preset_manager.load_config.return_value = initial_cfg

        win = MainWindow(self.app, mock_audio_service, mock_preset_manager)

        # Verify live sinks were synced into the horizontal rack
        self.assertEqual(len(win.config.channels), 3)
        self.assertEqual(len(win._speaker_cards), 3)

        # Toggle Unison Activation
        win._on_toggle_unison_clicked(None)
        self.assertTrue(win.config.is_active)
        mock_audio_service.activate_unison.assert_called_with(win.config)
        self.assertIn("DEACTIVATE", win.master_toggle_label.get_text())

        # Toggle Unison Deactivation
        mock_audio_service.is_running.return_value = False
        win._on_toggle_unison_clicked(None)
        self.assertFalse(win.config.is_active)
        mock_audio_service.deactivate_unison.assert_called()
        self.assertIn("ACTIVATE UNISON", win.master_toggle_label.get_text())

        # Test Tray Destination Controls
        channels = win._serialize_channels_for_tray()
        self.assertEqual(len(channels), 3)

        # Toggle enable via tray
        win._on_tray_toggle_channel_enable("alsa_output.pci.hdmi1", True)
        self.assertNotEqual(win.config.channels[0].role, SpeakerRole.EXCLUDED)

        # Toggle disable via tray
        win._on_tray_toggle_channel_enable("alsa_output.pci.hdmi1", False)
        self.assertEqual(win.config.channels[0].role, SpeakerRole.EXCLUDED)

        # Change role via tray
        win._on_tray_set_channel_role("alsa_output.pci.hdmi1", "SUBWOOFER")
        self.assertEqual(win.config.channels[0].role, SpeakerRole.SUBWOOFER)

        # Change channel volume via tray
        win._on_tray_set_channel_volume("alsa_output.pci.hdmi1", 0.75)
        self.assertEqual(win.config.channels[0].volume_gain, 0.75)

    def test_speaker_card_custom_label_editing_and_reset(self):
        """Verify custom label assignment, UI visual badge, and reset to hardware default."""
        channel = SpeakerConfig(
            sink_id=42,
            sink_name="alsa_output.pci.hdmi_stereo",
            display_name="HDMI / DisplayPort 2 Audio",
            role=SpeakerRole.LEFT
        )
        on_change_mock = MagicMock()
        on_test_mock = MagicMock()

        card = SpeakerCard(channel, on_change_mock, on_test_mock)

        # 1. Initial state (no custom name)
        self.assertIsNone(channel.custom_name)
        self.assertEqual(card._get_display_title(), "DisplayPort 2")
        self.assertFalse(card.title_label.has_css_class("strip-title-custom"))

        # 2. Save custom name
        card.name_entry.set_text("Studio Left Main")
        popover = card.edit_btn.get_popover()
        card._on_save_custom_name(popover)

        self.assertEqual(channel.custom_name, "Studio Left Main")
        self.assertEqual(card._get_display_title(), "Studio Left Main")
        self.assertEqual(card.title_label.get_text(), "Studio Left Main")
        self.assertTrue(card.title_label.has_css_class("strip-title-custom"))
        self.assertIn("Studio Left Main", card._get_title_tooltip())
        self.assertIn("HDMI / DisplayPort 2 Audio", card._get_title_tooltip())
        self.assertTrue(on_change_mock.called)

        # 3. Test button uses custom label in toast notification
        card._on_test_clicked(None)
        on_test_mock.assert_called_with("alsa_output.pci.hdmi_stereo", "Studio Left Main")

        # 4. Reset custom name to default
        on_change_mock.reset_mock()
        card._on_reset_custom_name(popover)

        self.assertIsNone(channel.custom_name)
        self.assertEqual(card._get_display_title(), "DisplayPort 2")
        self.assertFalse(card.title_label.has_css_class("strip-title-custom"))
        self.assertTrue(on_change_mock.called)

    def test_main_window_custom_name_preservation_across_disconnects(self):
        """Verify MainWindow preserves custom names and offline endpoints across live sink scans."""
        mock_audio_service = MagicMock()
        mock_audio_service.get_available_sinks.return_value = [
            AudioSink(id=1, name="sink.alpha", description="Alpha Monitor", media_class="Audio/Sink"),
            AudioSink(id=2, name="sink.beta", description="Beta Bluetooth", media_class="Audio/Sink", bus_type="bluetooth")
        ]
        mock_audio_service.is_running.return_value = False

        mock_preset_manager = MagicMock()
        mock_preset_manager.load_config.return_value = SystemConfig()

        win = MainWindow(self.app, mock_audio_service, mock_preset_manager)

        # Personalize Beta Bluetooth sink
        beta_ch = next(c for c in win.config.channels if c.sink_name == "sink.beta")
        beta_ch.custom_name = "Cuffie Ufficio"
        beta_ch.delay_ms = 180.0

        # Tray should serialize custom name
        tray_channels = win._serialize_channels_for_tray()
        beta_tray = next(c for c in tray_channels if c["sink_name"] == "sink.beta")
        self.assertEqual(beta_tray["display_name"], "Cuffie Ufficio")

        # Simulate Beta device disconnecting (Bluetooth turned off / unplugged)
        mock_audio_service.get_available_sinks.return_value = [
            AudioSink(id=1, name="sink.alpha", description="Alpha Monitor", media_class="Audio/Sink")
        ]
        win._sync_live_sinks()
        win._populate_rack()

        # Rack only shows online card
        self.assertEqual(len(win._speaker_cards), 1)

        # Config STILL preserves offline Beta device with custom_name
        saved_beta = next(c for c in win.config.channels if c.sink_name == "sink.beta")
        self.assertEqual(saved_beta.custom_name, "Cuffie Ufficio")
        self.assertEqual(saved_beta.delay_ms, 180.0)

        # Simulate Beta device reconnecting
        mock_audio_service.get_available_sinks.return_value = [
            AudioSink(id=1, name="sink.alpha", description="Alpha Monitor", media_class="Audio/Sink"),
            AudioSink(id=99, name="sink.beta", description="Beta Bluetooth", media_class="Audio/Sink", bus_type="bluetooth")
        ]
        win._sync_live_sinks()
        win._populate_rack()

        self.assertEqual(len(win._speaker_cards), 2)
        reconnected_beta = next(c for c in win.config.channels if c.sink_name == "sink.beta")
        self.assertEqual(reconnected_beta.sink_id, 99)
        self.assertEqual(reconnected_beta.custom_name, "Cuffie Ufficio")
        self.assertEqual(reconnected_beta.delay_ms, 180.0)

    def test_main_window_duplicate_description_does_not_swap_labels(self):
        """Verify multiple endpoints sharing identical hardware descriptions never swap custom labels."""
        mock_audio_service = MagicMock()
        mock_audio_service.get_available_sinks.return_value = [
            AudioSink(id=1, name="alsa.gpu.hdmi1", description="HDA NVidia Digital Stereo (HDMI)", media_class="Audio/Sink"),
            AudioSink(id=2, name="alsa.gpu.hdmi2", description="HDA NVidia Digital Stereo (HDMI)", media_class="Audio/Sink")
        ]
        mock_audio_service.is_running.return_value = False

        mock_preset_manager = MagicMock()
        mock_preset_manager.load_config.return_value = SystemConfig()

        win = MainWindow(self.app, mock_audio_service, mock_preset_manager)

        # Personalize both devices
        hdmi1 = next(c for c in win.config.channels if c.sink_name == "alsa.gpu.hdmi1")
        hdmi1.custom_name = "Living Room TV"
        hdmi2 = next(c for c in win.config.channels if c.sink_name == "alsa.gpu.hdmi2")
        hdmi2.custom_name = "Studio Monitor"

        # Rescan live sinks (simulating app restart or hardware rescan)
        win._sync_live_sinks()

        # Verify labels remained strictly anchored to their respective sink_name
        hdmi1_after = next(c for c in win.config.channels if c.sink_name == "alsa.gpu.hdmi1")
        hdmi2_after = next(c for c in win.config.channels if c.sink_name == "alsa.gpu.hdmi2")
        self.assertEqual(hdmi1_after.custom_name, "Living Room TV")
        self.assertEqual(hdmi2_after.custom_name, "Studio Monitor")

    def test_speaker_card_precision_delay_and_negative_range(self):
        """Verify fine-grained +/-1ms step buttons, reset, and negative delay range."""
        channel = SpeakerConfig(
            sink_id=77,
            sink_name="alsa_output.pci.stereo",
            display_name="Main Speakers",
            role=SpeakerRole.STEREO,
            delay_ms=0.0
        )
        on_change = MagicMock()
        on_test = MagicMock()
        card = SpeakerCard(channel, on_change, on_test)

        # Negative delay setting
        card.delay_scale.set_value(-20.0)
        self.assertEqual(channel.delay_ms, -20.0)
        self.assertIn("-20.0ms", card.delay_badge.get_text())

        # Step down by 1ms
        card._step_delay(-1.0)
        self.assertEqual(channel.delay_ms, -21.0)
        self.assertIn("-21.0ms", card.delay_badge.get_text())

        # Step up by 1ms
        card._step_delay(+1.0)
        self.assertEqual(channel.delay_ms, -20.0)

        # Reset to 0ms
        card._reset_delay()
        self.assertEqual(channel.delay_ms, 0.0)
        self.assertIn("0.0ms", card.delay_badge.get_text())

    def test_speaker_card_fast_volume_and_audible_enable(self):
        """Verify real-time non-blocking volume callback and auto audible volume reset on enable."""
        channel = SpeakerConfig(
            sink_id=88,
            sink_name="alsa_output.pci.front",
            display_name="Front Speakers",
            role=SpeakerRole.EXCLUDED,
            volume_gain=0.02
        )
        on_change = MagicMock()
        on_test = MagicMock()
        mock_audio_service = MagicMock()

        card = SpeakerCard(channel, on_change, on_test, audio_service=mock_audio_service)

        # 1. Test fast volume callback
        card.vol_scale.set_value(0.85)
        self.assertEqual(channel.volume_gain, 0.85)
        self.assertEqual(card.vol_badge.get_text(), "85%")
        mock_audio_service.set_speaker_gain.assert_called_with("alsa_output.pci.front", 0.85, persist=False)

        # 2. Test enable toggle when volume is near 0
        channel.volume_gain = 0.04
        card.vol_badge.set_text("4%")
        card.enable_switch.set_active(True)

        self.assertEqual(channel.role, SpeakerRole.STEREO)
        self.assertEqual(channel.volume_gain, 1.0)
        self.assertEqual(card.vol_scale.get_value(), 1.0)
        self.assertEqual(card.vol_badge.get_text(), "100%")
        card._emit_enable_change()  # Flush debounce timer
        self.assertTrue(on_change.called)

    def test_routing_graph_view_and_canvas(self):
        """Verify RoutingGraphView construction, metrics pills, cairo rendering and hit testing."""
        mock_audio_service = MagicMock()
        mock_audio_service.get_routing_status.return_value = {
            "is_unison_active": True,
            "master_volume": 0.85,
            "master_state": "RUNNING",
            "sources": [
                {
                    "id": "1001",
                    "sink_id": 99,
                    "app_name": "Spotify Music",
                    "icon_name": "spotify",
                    "media_name": "Audio Playback",
                    "volume_pct": 100,
                    "is_active": True,
                    "corked": False,
                    "muted": False
                }
            ],
            "destinations": [
                {
                    "sink_id": 55,
                    "sink_name": "alsa_output.pci.hdmi_left",
                    "display_name": "HDMI Display Left",
                    "bus_type": "hdmi",
                    "role": "stereo",
                    "volume_gain": 1.0,
                    "delay_ms": 0.0,
                    "hardware_latency_ms": 10.0,
                    "auto_comp_delay_ms": 170.0,
                    "total_delay_ms": 170.0,
                    "is_enabled": True,
                    "is_streaming": True,
                    "state": "RUNNING"
                },
                {
                    "sink_id": 88,
                    "sink_name": "bluez_output.bt_speaker",
                    "display_name": "Bluetooth Speaker",
                    "bus_type": "bluetooth",
                    "role": "stereo",
                    "volume_gain": 0.9,
                    "delay_ms": 0.0,
                    "hardware_latency_ms": 180.0,
                    "auto_comp_delay_ms": 0.0,
                    "total_delay_ms": 0.0,
                    "is_enabled": True,
                    "is_streaming": True,
                    "state": "RUNNING"
                }
            ],
            "max_hw_latency": 180.0
        }

        on_test_mock = MagicMock()
        on_toggle_unison_mock = MagicMock()
        cfg = SystemConfig()

        view = RoutingGraphView(
            audio_service=mock_audio_service,
            config=cfg,
            on_test_speaker=on_test_mock,
            on_toggle_unison=on_toggle_unison_mock
        )

        # Check metrics pills
        self.assertIn("Sources: 1", view.sources_pill.get_text())
        self.assertIn("Active Sinks: 2/2", view.sinks_pill.get_text())
        self.assertIn("Max Delay Comp: 180 ms", view.latency_pill.get_text())
        self.assertIn("Active (48 kHz)", view.dsp_pill.get_text())

        # Render on a Cairo ImageSurface
        surf = cairo.ImageSurface(cairo.FORMAT_ARGB32, 1024, 700)
        cr = cairo.Context(surf)
        view.canvas._on_draw(view.canvas, cr, 1024, 700)

        # Verify cards bounds are populated
        self.assertIn("src_1001", view.canvas._cards_bounds)
        self.assertIn("master_node", view.canvas._cards_bounds)
        self.assertIn("dst_55", view.canvas._cards_bounds)
        self.assertIn("dst_88", view.canvas._cards_bounds)

        # Test clicking on a destination card
        dst_info = view.canvas._cards_bounds["dst_55"]
        click_x = dst_info["x"] + dst_info["w"] / 2.0
        click_y = dst_info["y"] + dst_info["h"] / 2.0
        view.canvas._on_click_pressed(None, 1, click_x, click_y)
        on_test_mock.assert_called_with(55, "HDMI Display Left")

        # Test clicking on the master node
        mst_info = view.canvas._cards_bounds["master_node"]
        m_x = mst_info["x"] + mst_info["w"] / 2.0
        m_y = mst_info["y"] + mst_info["h"] / 2.0
        view.canvas._on_click_pressed(None, 1, m_x, m_y)
        on_toggle_unison_mock.assert_called()

        # Test animation start & stop lifecycle
        view.start_polling()
        self.assertTrue(view.canvas._is_active_view)
        view.stop_polling()
        self.assertFalse(view.canvas._is_active_view)

    def test_main_window_view_switcher(self):
        """Verify ViewStack page switching and polling toggle in MainWindow."""
        mock_audio_service = MagicMock()
        mock_audio_service.get_available_sinks.return_value = [
            AudioSink(id=10, name="sink.alpha", description="Alpha Speaker", media_class="Audio/Sink", is_internal=False)
        ]
        mock_audio_service.get_routing_status.return_value = {
            "is_unison_active": False,
            "master_volume": 1.0,
            "master_state": "IDLE",
            "sources": [],
            "destinations": [],
            "max_hw_latency": 0.0
        }
        mock_audio_service.is_running.return_value = False

        mock_preset_manager = MagicMock()
        mock_preset_manager.load_config.return_value = SystemConfig(channels=[])

        win = MainWindow(self.app, mock_audio_service, mock_preset_manager)
        self.assertEqual(win.view_stack.get_visible_child_name(), "mixer")

        # Switch to Signal Flow graph page
        win.view_stack.set_visible_child_name("routing")
        self.assertEqual(win.view_stack.get_visible_child_name(), "routing")
        self.assertTrue(win.routing_graph.canvas._is_active_view)

        # Switch back to Mixer Console
        win.view_stack.set_visible_child_name("mixer")
        self.assertEqual(win.view_stack.get_visible_child_name(), "mixer")
        self.assertFalse(win.routing_graph.canvas._is_active_view)


if __name__ == "__main__":
    unittest.main()
