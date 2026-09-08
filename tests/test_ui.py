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


class TestUI(unittest.TestCase):
    """Test suite for horizontal studio mixing console presentation layer."""

    @classmethod
    def setUpClass(cls):
        Adw.init()
        cls.app = Adw.Application(
            application_id="io.polifonia.TestApp",
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
        self.assertTrue(on_change_mock.called)

        # Test disable switch toggling
        on_change_mock.reset_mock()
        card.enable_switch.set_active(False)
        self.assertEqual(channel.role, SpeakerRole.EXCLUDED)
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



if __name__ == "__main__":
    unittest.main()
