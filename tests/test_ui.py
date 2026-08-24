"""Unit tests for GTK4 / Libadwaita UI components (MainWindow and SpeakerRow)."""

import unittest
from unittest.mock import MagicMock, patch
import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Gtk, Adw, Gio

from core.models import SystemConfig, SpeakerConfig, SpeakerRole, AudioSink
from ui.views.speaker_row import SpeakerRow
from ui.views.main_window import MainWindow


class TestUI(unittest.TestCase):
    """Test suite for GTK4 / Libadwaita presentation layer."""

    @classmethod
    def setUpClass(cls):
        # Initialize Libadwaita environment once for the test suite
        Adw.init()
        cls.app = Adw.Application(
            application_id="io.polifonia.TestApp",
            flags=Gio.ApplicationFlags.FLAGS_NONE
        )

    def test_speaker_row_instantiation_and_callbacks(self):
        """Verify SpeakerRow widget configuration, delay/gain changes and callbacks."""
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

        row = SpeakerRow(channel, on_change_mock, on_test_mock)

        # Title and expansion state
        self.assertEqual(row.get_title(), "HDMI Monitor Left")
        self.assertTrue(row.get_enable_expansion())

        # Test delay adjustment
        row.delay_scale.set_value(22.5)
        self.assertEqual(channel.delay_ms, 22.5)
        self.assertTrue(on_change_mock.called)

        # Test gain adjustment
        on_change_mock.reset_mock()
        row.gain_scale.set_value(1.2)
        self.assertEqual(channel.gain, 1.2)
        self.assertTrue(on_change_mock.called)

        # Test disable expansion switch toggling
        on_change_mock.reset_mock()
        row.set_enable_expansion(False)
        self.assertEqual(channel.role, SpeakerRole.EXCLUDED)
        self.assertTrue(on_change_mock.called)

        # Test enable expansion switch toggling
        on_change_mock.reset_mock()
        row.set_enable_expansion(True)
        self.assertNotEqual(channel.role, SpeakerRole.EXCLUDED)
        self.assertTrue(on_change_mock.called)

        # Test direct test tone callback invocation
        on_test_mock(channel.sink_id)
        on_test_mock.assert_called_with(55)

    def test_main_window_lifecycle_and_presets(self):
        """Verify MainWindow construction, sink synchronization and 2.1 preset application."""
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

        # Verify live sinks were synced
        self.assertEqual(len(win.config.channels), 3)

        # Apply Preset 2.1
        win._on_apply_preset_21(None)
        self.assertTrue(win.config.crossover.enabled)
        self.assertEqual(win.config.crossover.frequency_hz, 120)

        # Toggle Unison Activation
        win._on_toggle_unison_clicked(None)
        self.assertTrue(win.config.is_active)
        mock_audio_service.activate_unison.assert_called_with(win.config)
        self.assertEqual(win.apply_btn.get_label(), "Disattiva")

        # Toggle Unison Deactivation
        mock_audio_service.is_running.return_value = True
        win._on_toggle_unison_clicked(None)
        self.assertFalse(win.config.is_active)
        mock_audio_service.deactivate_unison.assert_called()
        self.assertEqual(win.apply_btn.get_label(), "Attiva Unisono")


if __name__ == "__main__":
    unittest.main()
