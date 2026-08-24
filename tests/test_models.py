"""Unit tests for Polifonia domain models, enums, and data validation."""

import unittest
from core.models import (
    SpeakerRole,
    AudioSink,
    SpeakerConfig,
    CrossoverConfig,
    SystemConfig,
    SpeakerChannel,
    SinkDevice
)


class TestModels(unittest.TestCase):
    """Test suite for core data models and configuration classes."""

    def test_speaker_role_values_and_aliases(self):
        """Verify enum string representations and alias compatibility."""
        self.assertEqual(SpeakerRole.LEFT.value, "left")
        self.assertEqual(SpeakerRole.RIGHT.value, "right")
        self.assertEqual(SpeakerRole.SUBWOOFER.value, "subwoofer")
        self.assertEqual(SpeakerRole.CENTER.value, "center")
        self.assertEqual(SpeakerRole.STEREO.value, "stereo")
        self.assertEqual(SpeakerRole.SURROUND_LEFT.value, "surround_left")
        self.assertEqual(SpeakerRole.SURROUND_RIGHT.value, "surround_right")
        self.assertEqual(SpeakerRole.EXCLUDED.value, "excluded")
        # DISABLED is an alias of EXCLUDED
        self.assertEqual(SpeakerRole.DISABLED, SpeakerRole.EXCLUDED)
        self.assertEqual(SpeakerRole.DISABLED.value, "excluded")

    def test_audio_sink_defaults_and_serialization(self):
        """Verify AudioSink dataclass fields and to_dict method."""
        sink = AudioSink(
            id=42,
            name="alsa_output.pci.hdmi",
            description="HDMI Digital Audio",
            media_class="Audio/Sink"
        )
        self.assertEqual(sink.id, 42)
        self.assertEqual(sink.name, "alsa_output.pci.hdmi")
        self.assertEqual(sink.channel_count, 2)
        self.assertFalse(sink.is_default)
        self.assertFalse(sink.is_internal)
        self.assertEqual(sink.volume, 1.0)
        self.assertFalse(sink.mute)

        data = sink.to_dict()
        self.assertEqual(data["id"], 42)
        self.assertEqual(data["name"], "alsa_output.pci.hdmi")
        self.assertEqual(data["description"], "HDMI Digital Audio")

    def test_speaker_config_properties_and_serialization(self):
        """Verify SpeakerConfig property aliases and dictionary conversions."""
        spk = SpeakerConfig(
            sink_id=101,
            sink_name="sink_usb_dac",
            display_name="USB HiFi DAC",
            role=SpeakerRole.SUBWOOFER,
            volume_gain=1.25,
            delay_ms=14.5,
            mute=False,
            custom_name="Studio Sub",
            phase_inverted=True
        )

        # Property access
        self.assertEqual(spk.id, 101)
        self.assertEqual(spk.name, "sink_usb_dac")
        self.assertEqual(spk.gain, 1.25)

        # Property setters
        spk.id = 202
        self.assertEqual(spk.sink_id, 202)
        spk.name = "sink_usb_modified"
        self.assertEqual(spk.sink_name, "sink_usb_modified")
        spk.gain = 0.95
        self.assertEqual(spk.volume_gain, 0.95)

        # Serialization
        data = spk.to_dict()
        self.assertEqual(data["sink_id"], 202)
        self.assertEqual(data["sink_name"], "sink_usb_modified")
        self.assertEqual(data["role"], "subwoofer")
        self.assertEqual(data["volume_gain"], 0.95)
        self.assertEqual(data["delay_ms"], 14.5)
        self.assertEqual(data["custom_name"], "Studio Sub")
        self.assertTrue(data["phase_inverted"])

        # Deserialization
        restored = SpeakerConfig.from_dict(data)
        self.assertEqual(restored.sink_id, 202)
        self.assertEqual(restored.sink_name, "sink_usb_modified")
        self.assertEqual(restored.role, SpeakerRole.SUBWOOFER)
        self.assertEqual(restored.volume_gain, 0.95)
        self.assertEqual(restored.delay_ms, 14.5)
        self.assertTrue(restored.phase_inverted)

    def test_speaker_config_from_dict_fallback(self):
        """Verify graceful fallback for invalid or legacy role strings."""
        raw = {
            "id": 55,
            "name": "legacy_sink",
            "role": "non_existent_role_xyz",
            "gain": 1.1
        }
        spk = SpeakerConfig.from_dict(raw)
        self.assertEqual(spk.sink_id, 55)
        self.assertEqual(spk.sink_name, "legacy_sink")
        self.assertEqual(spk.role, SpeakerRole.EXCLUDED)
        self.assertEqual(spk.volume_gain, 1.1)

    def test_crossover_config_standalone_and_parent_sync(self):
        """Verify CrossoverConfig standalone behavior and two-way parent binding."""
        # Standalone
        cross = CrossoverConfig(enabled=False, frequency_hz=160)
        self.assertFalse(cross.enabled)
        self.assertEqual(cross.frequency_hz, 160)
        self.assertEqual(cross.freq, 160)
        cross.freq = 180
        self.assertEqual(cross.frequency_hz, 180)

        cross_dict = cross.to_dict()
        self.assertEqual(cross_dict["enabled"], False)
        self.assertEqual(cross_dict["frequency_hz"], 180)

        restored_cross = CrossoverConfig.from_dict({"enabled": True, "freq": 135})
        self.assertTrue(restored_cross.enabled)
        self.assertEqual(restored_cross.frequency_hz, 135)

        # Parent Synchronization
        sys_cfg = SystemConfig(crossover_enabled=True, crossover_freq=95)
        self.assertTrue(sys_cfg.crossover.enabled)
        self.assertEqual(sys_cfg.crossover.frequency_hz, 95)

        # Mutating via crossover object modifies parent properties
        sys_cfg.crossover.enabled = False
        sys_cfg.crossover.frequency_hz = 140
        self.assertFalse(sys_cfg.crossover_enabled)
        self.assertEqual(sys_cfg.crossover_freq, 140)

        # Mutating parent properties modifies crossover object view
        sys_cfg.crossover_enabled = True
        sys_cfg.crossover_freq = 80
        self.assertTrue(sys_cfg.crossover.enabled)
        self.assertEqual(sys_cfg.crossover.frequency_hz, 80)

    def test_system_config_channels_property_and_serialization(self):
        """Verify SystemConfig channel list manipulation and serialization roundtrip."""
        spk1 = SpeakerConfig(sink_id=1, sink_name="left_mon", role=SpeakerRole.LEFT, volume_gain=1.0)
        spk2 = SpeakerConfig(sink_id=2, sink_name="right_mon", role=SpeakerRole.RIGHT, volume_gain=1.0)
        spk3 = SpeakerConfig(sink_id=3, sink_name="sub_unit", role=SpeakerRole.SUBWOOFER, volume_gain=1.2)

        cfg = SystemConfig(
            profile_name="Triple Monitor 2.1",
            master_volume=0.9,
            crossover_enabled=True,
            crossover_freq=105,
            speakers={"left_mon": spk1, "right_mon": spk2, "sub_unit": spk3},
            auto_start=True,
            is_active=True,
            set_as_default=True
        )

        # Channels property getter
        channels = cfg.channels
        self.assertEqual(len(channels), 3)
        self.assertIn(spk1, channels)

        # Channels property setter
        spk4 = SpeakerConfig(sink_id=4, sink_name="center_mon", role=SpeakerRole.CENTER)
        cfg.channels = [spk1, spk4]
        self.assertEqual(len(cfg.speakers), 2)
        self.assertIn("left_mon", cfg.speakers)
        self.assertIn("center_mon", cfg.speakers)

        # Master gain alias
        self.assertEqual(cfg.master_gain, 0.9)
        cfg.master_gain = 0.85
        self.assertEqual(cfg.master_volume, 0.85)

        # Full roundtrip serialization
        serialized = cfg.to_dict()
        self.assertEqual(serialized["profile_name"], "Triple Monitor 2.1")
        self.assertEqual(serialized["master_volume"], 0.85)
        self.assertTrue(serialized["crossover_enabled"])
        self.assertEqual(serialized["crossover_freq"], 105)
        self.assertTrue(serialized["is_active"])

        restored_cfg = SystemConfig.from_dict(serialized)
        self.assertEqual(restored_cfg.profile_name, "Triple Monitor 2.1")
        self.assertEqual(restored_cfg.master_volume, 0.85)
        self.assertEqual(restored_cfg.crossover_freq, 105)
        self.assertEqual(len(restored_cfg.speakers), 2)
        self.assertEqual(restored_cfg.speakers["center_mon"].role, SpeakerRole.CENTER)

    def test_system_config_from_dict_legacy_and_edge_cases(self):
        """Verify parsing configuration from list format and nested crossover dict."""
        payload = {
            "profile_name": "Legacy Setup",
            "master_gain": 0.75,
            "crossover": {
                "enabled": True,
                "frequency_hz": 125
            },
            "channels": [
                {"sink_id": 10, "sink_name": "hdmi_l", "role": "left"},
                {"sink_id": 20, "sink_name": "hdmi_r", "role": "right"}
            ]
        }
        cfg = SystemConfig.from_dict(payload)
        self.assertEqual(cfg.profile_name, "Legacy Setup")
        self.assertEqual(cfg.master_volume, 0.75)
        self.assertTrue(cfg.crossover_enabled)
        self.assertEqual(cfg.crossover_freq, 125)
        self.assertEqual(len(cfg.channels), 2)


if __name__ == "__main__":
    unittest.main()
