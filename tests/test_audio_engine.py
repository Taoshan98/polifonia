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
            self.assertEqual(channel_map["alsa.hdmi.1"].role, SpeakerRole.STEREO)
            self.assertEqual(channel_map["alsa.hdmi.2"].role, SpeakerRole.STEREO)
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

    def test_set_speaker_gain_fast_non_persisting_and_mute(self):
        """Verify set_speaker_gain with persist=False does not save to storage and mutes on zero gain."""
        self.service.config.channels = [
            SpeakerConfig(sink_id=10, sink_name="hdmi_l", role=SpeakerRole.LEFT, volume_gain=1.0)
        ]
        with patch.object(self.service.storage, "save") as mock_save, \
             patch("subprocess.run") as mock_run:
            self.service._is_active = True
            
            # Fast gain adjustment without disk save
            self.service.set_speaker_gain("hdmi_l", 0.65, persist=False)
            self.assertEqual(self.service.config.channels[0].volume_gain, 0.65)
            self.assertFalse(mock_save.called)

            time.sleep(0.05)  # wait for daemon thread
            self.assertTrue(mock_run.called)
            vol_call = mock_run.call_args_list[-1][0][0]
            self.assertIn("65%", vol_call)

            # Test mute on zero gain
            mock_run.reset_mock()
            self.service.set_speaker_gain("hdmi_l", 0.0, persist=False)
            time.sleep(0.05)
            mute_call = mock_run.call_args_list[-1][0][0]
            self.assertEqual(mute_call, ["pactl", "set-sink-mute", "hdmi_l", "1"])

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
            self.assertIn("15", first_cmd)

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

    def test_bluetooth_loopback_parameters(self):
        """Verify Bluetooth loopback does not use node.passive=true and does not force -l 15."""
        self.service.config.channels = [
            SpeakerConfig(
                sink_id=139,
                sink_name="bluez_output.4C_87_5D_9F_7E_60.1",
                role=SpeakerRole.STEREO,
                bus_type="bluetooth",
                hardware_latency_ms=180.0
            )
        ]
        mock_proc = MagicMock(poll=MagicMock(return_value=None))
        with patch.object(self.service, "_ensure_master_sink", return_value=True), \
             patch("subprocess.Popen", return_value=mock_proc) as mock_popen, \
             patch("subprocess.run"):
            self.service.start_unison_sink()
            self.assertTrue(self.service.is_running())
            self.assertEqual(mock_popen.call_count, 1)

            bt_cmd = mock_popen.call_args[0][0]
            self.assertIn("pw-loopback", bt_cmd)
            # Must NOT be passive
            self.assertTrue(any("node.passive=false" in arg for arg in bt_cmd))
            self.assertFalse(any("node.passive=true" in arg for arg in bt_cmd))
            # Must NOT force -l 15
            self.assertNotIn("-l", bt_cmd)

    def test_auto_latency_compensation_between_monitor_and_bluetooth(self):
        """Verify faster monitor sink is automatically delayed to match higher-latency Bluetooth sink."""
        self.service.config.channels = [
            SpeakerConfig(
                sink_id=62,
                sink_name="alsa_output.pci-gpu.pro-output-3",
                role=SpeakerRole.LEFT,
                delay_ms=0.0,
                bus_type="hdmi",
                hardware_latency_ms=10.0
            ),
            SpeakerConfig(
                sink_id=139,
                sink_name="bluez_output.4C_87_5D_9F_7E_60.1",
                role=SpeakerRole.RIGHT,
                delay_ms=0.0,
                bus_type="bluetooth",
                hardware_latency_ms=180.0
            )
        ]
        mock_proc1 = MagicMock(poll=MagicMock(return_value=None))
        mock_proc2 = MagicMock(poll=MagicMock(return_value=None))
        with patch.object(self.service, "_ensure_master_sink", return_value=True), \
             patch("subprocess.Popen", side_effect=[mock_proc1, mock_proc2]) as mock_popen, \
             patch("subprocess.run"):
            self.service.start_unison_sink()
            self.assertEqual(mock_popen.call_count, 2)

            cmd_map = {}
            for call_args in mock_popen.call_args_list:
                cmd = call_args[0][0]
                for arg in cmd:
                    if "polifonia_play_62" in arg:
                        cmd_map["monitor"] = cmd
                    elif "polifonia_play_139" in arg:
                        cmd_map["bluetooth"] = cmd

            # Monitor has 10ms HW latency, BT has 180ms -> Monitor receives 170ms auto delay
            self.assertIn("monitor", cmd_map)
            mon_cmd = cmd_map["monitor"]
            self.assertIn("--delay", mon_cmd)
            delay_idx = mon_cmd.index("--delay")
            self.assertEqual(mon_cmd[delay_idx + 1], "0.1700")

            # Bluetooth has max HW latency -> no extra delay
            self.assertIn("bluetooth", cmd_map)
            bt_cmd = cmd_map["bluetooth"]
            self.assertNotIn("--delay", bt_cmd)

    def test_negative_delay_relative_normalization(self):
        """Verify negative delay on one sink shifts all other sinks forward relatively."""
        self.service.config.channels = [
            SpeakerConfig(
                sink_id=51,
                sink_name="alsa_output.sink1",
                role=SpeakerRole.STEREO,
                delay_ms=-25.0,
                hardware_latency_ms=0.0
            ),
            SpeakerConfig(
                sink_id=52,
                sink_name="alsa_output.sink2",
                role=SpeakerRole.STEREO,
                delay_ms=0.0,
                hardware_latency_ms=0.0
            )
        ]
        mock_proc1 = MagicMock(poll=MagicMock(return_value=None))
        mock_proc2 = MagicMock(poll=MagicMock(return_value=None))
        with patch.object(self.service, "_ensure_master_sink", return_value=True), \
             patch("subprocess.Popen", side_effect=[mock_proc1, mock_proc2]) as mock_popen, \
             patch("subprocess.run"):
            self.service.start_unison_sink()
            self.assertEqual(mock_popen.call_count, 2)

            cmd_map = {}
            for call_args in mock_popen.call_args_list:
                cmd = call_args[0][0]
                for arg in cmd:
                    if "polifonia_play_51" in arg:
                        cmd_map["sink1"] = cmd
                    elif "polifonia_play_52" in arg:
                        cmd_map["sink2"] = cmd

            # sink1 had -25ms delay (user min) -> normalized to 0ms extra delay
            self.assertIn("sink1", cmd_map)
            self.assertNotIn("--delay", cmd_map["sink1"])

            # sink2 had 0ms delay -> normalized to 0 - (-25) = +25ms delay
            self.assertIn("sink2", cmd_map)
            sink2_cmd = cmd_map["sink2"]
            self.assertIn("--delay", sink2_cmd)
            delay_idx = sink2_cmd.index("--delay")
            self.assertEqual(sink2_cmd[delay_idx + 1], "0.0250")

    def test_alias_audio_service(self):
        """Verify AudioService is an alias to AudioEngineService."""
        self.assertIs(AudioService, AudioEngineService)

    def test_repeated_toggle_no_duplicate_processes_and_no_sink_muting(self):
        """Verify rapid enable/disable cycles terminate old processes, kill by target, and never mute hardware sinks."""
        self.service.config.channels = [
            SpeakerConfig(
                sink_id=60,
                sink_name="alsa_output.pci.front",
                role=SpeakerRole.STEREO,
                delay_ms=0.0,
                hardware_latency_ms=10.0
            ),
            SpeakerConfig(
                sink_id=194,
                sink_name="bluez_output.00_0C_8A_43_12_0E.1",
                role=SpeakerRole.STEREO,
                bus_type="bluetooth",
                hardware_latency_ms=150.0
            )
        ]

        def _make_mock_proc():
            p = MagicMock()
            p.poll = MagicMock(return_value=None)
            p.terminate = MagicMock()
            p.wait = MagicMock(return_value=0)
            return p

        with patch.object(self.service, "_ensure_master_sink", return_value=True), \
             patch("subprocess.Popen", side_effect=[_make_mock_proc() for _ in range(30)]) as mock_popen, \
             patch("subprocess.run") as mock_run:
            # 1. Start unison
            self.service.start_unison_sink()
            self.assertEqual(len(self.service._sync_processes), 2)

            # 2. Toggle Bluetooth off and on 5 times
            bt_spk = next(s for s in self.service.config.channels if "bluez" in s.sink_name)
            for _ in range(5):
                # Disable BT
                bt_spk.role = SpeakerRole.EXCLUDED
                self.service.sync_active_branches()
                self.assertEqual(len(self.service._sync_processes), 1)
                self.assertNotIn(bt_spk.sink_name, self.service._sync_processes)

                # Re-enable BT
                bt_spk.role = SpeakerRole.STEREO
                self.service.sync_active_branches()
                self.assertEqual(len(self.service._sync_processes), 2)
                self.assertIn(bt_spk.sink_name, self.service._sync_processes)

            # 3. Verify that pactl set-sink-mute ... 1 was NEVER called on disabling
            mute_1_calls = [
                call_args[0][0] for call_args in mock_run.call_args_list
                if "set-sink-mute" in call_args[0][0] and "1" in call_args[0][0]
            ]
            self.assertEqual(len(mute_1_calls), 0, f"Hardware sink was muted when disabled: {mute_1_calls}")

            # 4. Verify that pkill -9 -f target.object was called to prevent duplicate loopbacks
            pkill_target_calls = [
                call_args[0][0] for call_args in mock_run.call_args_list
                if len(call_args[0][0]) >= 4 and "pkill" in call_args[0][0][0] and "target.object" in call_args[0][0][3]
            ]
            self.assertTrue(len(pkill_target_calls) > 0)

    def test_get_routing_status(self):
        """Verify get_routing_status aggregates scanner inputs, sink states and channel latency."""
        with patch.object(self.service.scanner, "get_sink_states", return_value={"polifonia_master": "RUNNING", "bluez_sink": "RUNNING"}), \
             patch.object(self.service.scanner, "get_active_sink_inputs", return_value=[{"id": "1", "app_name": "Chrome", "is_active": True}]), \
             patch.object(self.service, "get_available_sinks", return_value=[AudioSink(id=1, name="bluez_sink", description="BT", media_class="Audio/Sink", is_internal=False)]):
            self.service._is_active = True
            status = self.service.get_routing_status()

            self.assertTrue(status["is_unison_active"])
            self.assertEqual(status["master_state"], "RUNNING")
            self.assertEqual(len(status["sources"]), 1)
            self.assertEqual(status["sources"][0]["app_name"], "Chrome")
            self.assertIn("destinations", status)


    def test_bluetooth_startup_priming_and_phased_launch(self):
        """Verify Bluetooth endpoints are primed with silent PCM and launched prior to wired endpoints."""
        self.service.config.channels = [
            SpeakerConfig(sink_id=1, sink_name="alsa_wired", display_name="Wired", role=SpeakerRole.STEREO, hardware_latency_ms=10.0, volume_gain=0.8),
            SpeakerConfig(sink_id=2, sink_name="bluez_output.test_bt", display_name="BT Speaker", role=SpeakerRole.STEREO, bus_type="bluetooth", hardware_latency_ms=150.0, volume_gain=0.25)
        ]

        def _make_mock_proc():
            p = MagicMock()
            p.poll = MagicMock(return_value=None)
            p.terminate = MagicMock()
            p.wait = MagicMock(return_value=0)
            return p

        spawn_order = []
        def _mock_popen(cmd, *args, **kwargs):
            for part in cmd:
                if part.startswith("--playback-props="):
                    props = part.split("--playback-props=", 1)[1]
                    for token in props.split():
                        if token.startswith("target.object="):
                            spawn_order.append(token.split("=")[1])
            return _make_mock_proc()

        with patch.object(self.service, "_ensure_master_sink", return_value=True), \
             patch("subprocess.Popen", side_effect=_mock_popen), \
             patch("subprocess.run") as mock_run, \
             patch("time.sleep"):
            self.service.start_unison_sink()

            # 1. Verify Bluetooth volume was raised to audible level (>= 0.50)
            bt_spk = next(s for s in self.service.config.channels if "bluez" in s.sink_name)
            self.assertGreaterEqual(bt_spk.volume_gain, 0.50)

            # 2. Verify pw-play priming call was made for Bluetooth
            pw_play_calls = [
                call_args[0][0] for call_args in mock_run.call_args_list
                if len(call_args[0][0]) > 0 and call_args[0][0][0] == "pw-play" and any("bluez" in c for c in call_args[0][0])
            ]
            self.assertEqual(len(pw_play_calls), 1)

            # 3. Verify Bluetooth was launched first in spawn_order
            self.assertEqual(len(spawn_order), 2)
            self.assertIn("bluez_output.test_bt", spawn_order[0])
            self.assertIn("alsa_wired", spawn_order[1])


if __name__ == "__main__":
    unittest.main()

