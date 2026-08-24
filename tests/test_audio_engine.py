"""Unit tests for AudioEngineService coordination, test signal synthesis, and process supervisor."""

import unittest
import tempfile
import shutil
import time
from unittest.mock import patch, MagicMock
from pathlib import Path
from core.models import SystemConfig, AudioSink, SpeakerConfig, SpeakerRole
from storage.settings_store import StorageService
from services.audio_engine import AudioEngineService, AudioService


class TestAudioEngine(unittest.TestCase):
    """Test suite for AudioEngineService."""

    def setUp(self):
        self.temp_dir = tempfile.mkdtemp(prefix="polifonia_engine_test_")
        self.patcher_storage = patch("services.audio_engine.StorageService", return_value=StorageService(config_dir=self.temp_dir))
        self.mock_storage_cls = self.patcher_storage.start()
        self.service = AudioEngineService()

    def tearDown(self):
        self.service.stop_unison_sink()
        self.patcher_storage.stop()
        if Path(self.temp_dir).exists():
            shutil.rmtree(self.temp_dir)

    def test_initial_state(self):
        """Verify initial inactive state and storage configuration loading."""
        self.assertFalse(self.service.is_running())
        self.assertIsInstance(self.service.config, SystemConfig)

    def test_get_devices_merges_new_sinks_into_config(self):
        """Verify discovering new hardware sinks synchronizes channels and auto-excludes internal speakers."""
        mock_sinks = [
            AudioSink(id=1, name="alsa.hdmi.1", description="Monitor Left", media_class="Audio/Sink", is_internal=False),
            AudioSink(id=2, name="alsa.hdmi.2", description="Monitor Right", media_class="Audio/Sink", is_internal=False),
            AudioSink(id=3, name="alsa.speaker", description="Internal Speaker", media_class="Audio/Sink", is_internal=True)
        ]

        with patch.object(self.service.scanner, "get_sinks", return_value=mock_sinks):
            returned_sinks = self.service.get_devices()

            self.assertEqual(len(returned_sinks), 3)
            channels = self.service.config.channels
            self.assertEqual(len(channels), 3)

            channel_map = {c.sink_name: c for c in channels}
            self.assertEqual(channel_map["alsa.hdmi.1"].role, SpeakerRole.LEFT)
            self.assertEqual(channel_map["alsa.hdmi.2"].role, SpeakerRole.LEFT)
            self.assertEqual(channel_map["alsa.speaker"].role, SpeakerRole.EXCLUDED)

    def test_set_speaker_parameters(self):
        """Verify modifying speaker role, delay, and gain updates configuration and saves state."""
        self.service.config.channels = [
            SpeakerConfig(sink_id=10, sink_name="hdmi_l", role=SpeakerRole.LEFT, delay_ms=0.0, volume_gain=1.0)
        ]
        self.service.storage.save(self.service.config)

        # Set role
        self.service.set_speaker_role("hdmi_l", SpeakerRole.RIGHT)
        self.assertEqual(self.service.config.channels[0].role, SpeakerRole.RIGHT)

        # Set delay
        self.service.set_speaker_delay("hdmi_l", 15.5)
        self.assertEqual(self.service.config.channels[0].delay_ms, 15.5)

        # Set gain with clamping
        self.service.set_speaker_gain("hdmi_l", 1.4)
        self.assertEqual(self.service.config.channels[0].volume_gain, 1.4)

        # Reload from storage to ensure persistence
        loaded = self.service.storage.load()
        loaded_spk = loaded.speakers.get("hdmi_l")
        self.assertIsNotNone(loaded_spk)
        self.assertEqual(loaded_spk.role, SpeakerRole.RIGHT)
        self.assertEqual(loaded_spk.delay_ms, 15.5)
        self.assertEqual(loaded_spk.volume_gain, 1.4)

    def test_set_crossover_and_master_gain(self):
        """Verify crossover and master volume modifications."""
        self.service.set_crossover(enabled=True, freq=130)
        self.assertTrue(self.service.config.crossover_enabled)
        self.assertEqual(self.service.config.crossover_freq, 130)

        self.service.set_master_gain(0.75)
        self.assertEqual(self.service.config.master_volume, 0.75)

    def test_play_test_tone_synthesizer(self):
        """Verify synthetic WAV generation and pw-play target invocation."""
        with patch("subprocess.run") as mock_run:
            self.service.play_test_tone(sink_id=55, freq=440, duration=0.1)
            time.sleep(0.15)  # Wait for daemon thread execution

            self.assertTrue(mock_run.called)
            call_args = mock_run.call_args[0][0]
            self.assertEqual(call_args[0], "pw-play")
            self.assertEqual(call_args[1], "--target=55")

    def test_unison_lifecycle(self):
        """Verify starting and stopping unison loops."""
        self.service.config.channels = [
            SpeakerConfig(sink_id=10, sink_name="hdmi_l", role=SpeakerRole.LEFT, delay_ms=12.0),
            SpeakerConfig(sink_id=20, sink_name="hdmi_r", role=SpeakerRole.RIGHT, delay_ms=12.0),
            SpeakerConfig(sink_id=30, sink_name="sub_aux", role=SpeakerRole.SUBWOOFER, delay_ms=0.0)
        ]

        mock_proc1 = MagicMock()
        mock_proc2 = MagicMock()
        mock_proc3 = MagicMock()

        with patch("subprocess.Popen", side_effect=[mock_proc1, mock_proc2, mock_proc3]) as mock_popen, \
             patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(stdout="536870999\n", returncode=0)

            # Start unison
            started = self.service.start_unison_sink()
            self.assertTrue(started)
            self.assertTrue(self.service.is_running())
            self.assertEqual(mock_popen.call_count, 3)

            # Inspect loopback command flags
            first_cmd = mock_popen.call_args_list[0][0][0]
            self.assertIn("pw-loopback", first_cmd)
            self.assertTrue(any("target.object=polifonia_master" in arg for arg in first_cmd))
            self.assertTrue(any("stream.capture.sink=true" in arg for arg in first_cmd))
            self.assertTrue(any("hdmi_l" in arg for arg in first_cmd))
            self.assertIn("-l", first_cmd)
            self.assertIn("12", first_cmd)

            # Stop unison
            self.service.stop_unison_sink()
            self.assertFalse(self.service.is_running())
            self.assertEqual(len(self.service._sync_processes), 0)
            self.assertTrue(mock_proc1.terminate.called)
            self.assertTrue(mock_proc2.terminate.called)
            self.assertTrue(mock_proc3.terminate.called)


    def test_start_unison_with_no_active_speakers_returns_false(self):
        """Verify unison returns False if all channels are excluded/disabled."""
        self.service.config.channels = [
            SpeakerConfig(sink_id=1, sink_name="spk1", role=SpeakerRole.EXCLUDED),
            SpeakerConfig(sink_id=2, sink_name="spk2", role=SpeakerRole.DISABLED)
        ]
        result = self.service.start_unison_sink()
        self.assertFalse(result)
        self.assertFalse(self.service.is_running())

    def test_activate_and_deactivate_unison_methods(self):
        """Verify activate_unison and deactivate_unison aliases."""
        cfg = SystemConfig(
            speakers={"mon1": SpeakerConfig(sink_id=100, sink_name="mon1", role=SpeakerRole.LEFT)}
        )
        with patch.object(self.service, "start_unison_sink", return_value=True) as mock_start, \
             patch.object(self.service, "stop_unison_sink") as mock_stop:
            res_act = self.service.activate_unison(cfg)
            self.assertTrue(res_act)
            self.assertTrue(mock_start.called)
            self.assertEqual(self.service.config, cfg)

            res_deact = self.service.deactivate_unison()
            self.assertTrue(res_deact)
            self.assertTrue(mock_stop.called)

    def test_alias_audio_service(self):
        """Verify AudioService is an alias to AudioEngineService."""
        self.assertIs(AudioService, AudioEngineService)


if __name__ == "__main__":
    unittest.main()
