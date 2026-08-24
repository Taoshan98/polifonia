"""Unit tests for PipeWire filter-chain and combine-sink SPA configuration generator."""

import unittest
from core.models import SystemConfig, SpeakerConfig, SpeakerRole
from backend.pipewire_config import PipeWireConfigGenerator


class TestConfigGenerator(unittest.TestCase):
    """Test suite for PipeWire configuration generation logic."""

    def setUp(self):
        self.generator = PipeWireConfigGenerator()

    def test_generate_config_requires_profile(self):
        """Verify generate_config raises ValueError if no configuration profile provided."""
        with self.assertRaises(ValueError):
            self.generator.generate_config(None)

    def test_filter_chain_generation_with_active_crossover(self):
        """Verify filter chain SPA structure when 2.1 Crossover is enabled."""
        cfg = SystemConfig(
            profile_name="Pro 2.1 Studio",
            master_volume=0.95,
            crossover_enabled=True,
            crossover_freq=115,
            speakers={
                "left": SpeakerConfig(sink_id=10, sink_name="hdmi_left", role=SpeakerRole.LEFT, volume_gain=1.0, delay_ms=8.0),
                "right": SpeakerConfig(sink_id=20, sink_name="hdmi_right", role=SpeakerRole.RIGHT, volume_gain=1.0, delay_ms=8.0),
                "sub": SpeakerConfig(sink_id=30, sink_name="usb_sub", role=SpeakerRole.SUBWOOFER, volume_gain=1.2, delay_ms=0.0),
                "disabled": SpeakerConfig(sink_id=40, sink_name="internal_spk", role=SpeakerRole.EXCLUDED)
            }
        )

        config_text = self.generator.generate_config(cfg)

        # Basic SPA modules and headers
        self.assertIn("libpipewire-module-filter-chain", config_text)
        self.assertIn("filter-chain/libspa-filter-graph", config_text)
        self.assertIn("audioconvert/libspa-audioconvert", config_text)
        self.assertIn('node.name      = "polifonia_sink"', config_text)

        # High-pass filter for Satellites (Left / Right)
        self.assertIn("bq_highpass", config_text)
        self.assertIn('"Freq" = 115', config_text)
        self.assertIn('"Q" = 0.707', config_text)
        self.assertIn("filter_10", config_text)
        self.assertIn("filter_20", config_text)

        # Low-pass filter for Subwoofer
        self.assertIn("bq_lowpass", config_text)
        self.assertIn("filter_30", config_text)

        # Excluded speaker (40) must NOT appear in active filter nodes or links
        self.assertNotIn("filter_40", config_text)

    def test_filter_chain_generation_with_disabled_crossover(self):
        """Verify copy/passthrough filters when Crossover is disabled."""
        cfg = SystemConfig(
            profile_name="Flat Stereo",
            master_volume=1.0,
            crossover_enabled=False,
            crossover_freq=90,
            speakers={
                "left": SpeakerConfig(sink_id=1, sink_name="sink_l", role=SpeakerRole.LEFT),
                "right": SpeakerConfig(sink_id=2, sink_name="sink_r", role=SpeakerRole.RIGHT)
            }
        )

        config_text = self.generator.generate_config(cfg)
        self.assertNotIn("bq_highpass", config_text)
        self.assertNotIn("bq_lowpass", config_text)
        self.assertIn("label = copy", config_text)

    def test_build_combine_script(self):
        """Verify generation of dynamic python runner script for loopback nodes."""
        cfg = SystemConfig(
            speakers={
                "s1": SpeakerConfig(sink_id=55, sink_name="hdmi_left", role=SpeakerRole.LEFT, delay_ms=15.0),
                "s2": SpeakerConfig(sink_id=43, sink_name="hdmi_right", role=SpeakerRole.RIGHT, delay_ms=15.0),
                "s3": SpeakerConfig(sink_id=99, sink_name="laptop_spk", role=SpeakerRole.EXCLUDED)
            }
        )

        script = self.generator.build_combine_script(cfg)
        self.assertIn("#!/usr/bin/env python3", script)
        self.assertIn("pw-loopback", script)
        self.assertIn("polifonia_sink_55", script)
        self.assertIn("polifonia_sink_43", script)
        self.assertNotIn("polifonia_sink_99", script)


if __name__ == "__main__":
    unittest.main()
