import unittest
from polifonia.core.models import SystemConfig, SpeakerConfig, SpeakerRole, AudioSink
from polifonia.storage.settings_store import StorageService
from polifonia.backend.pipewire_config import PipeWireConfigGenerator

class TestPolifonia(unittest.TestCase):
    def setUp(self):
        self.storage = StorageService()
        self.generator = PipeWireConfigGenerator()

    def test_model_serialization(self):
        speaker = SpeakerConfig(
            sink_id=45,
            sink_name="alsa_output.pci.hdmi-stereo",
            role=SpeakerRole.LEFT,
            volume_gain=1.0,
            delay_ms=5.0
        )
        data = speaker.to_dict()
        self.assertEqual(data["role"], "left")
        self.assertEqual(data["delay_ms"], 5.0)

        restored = SpeakerConfig.from_dict(data)
        self.assertEqual(restored.role, SpeakerRole.LEFT)
        self.assertEqual(restored.sink_name, "alsa_output.pci.hdmi-stereo")

    def test_system_config_lifecycle(self):
        cfg = SystemConfig(
            profile_name="2.1 Tri-Monitor Studio",
            master_volume=0.9,
            crossover_enabled=True,
            crossover_freq=110,
            speakers={
                "mon1": SpeakerConfig(sink_id=10, sink_name="hdmi_left", role=SpeakerRole.LEFT),
                "mon2": SpeakerConfig(sink_id=20, sink_name="hdmi_right", role=SpeakerRole.RIGHT),
                "mon3_sub": SpeakerConfig(sink_id=30, sink_name="aux_sub", role=SpeakerRole.SUBWOOFER),
            }
        )
        saved_dict = cfg.to_dict()
        loaded_cfg = SystemConfig.from_dict(saved_dict)
        self.assertEqual(loaded_cfg.crossover_freq, 110)
        self.assertEqual(len(loaded_cfg.speakers), 3)

    def test_pipewire_filter_chain_generation(self):
        cfg = SystemConfig(
            profile_name="Test Setup",
            speakers={
                "s1": SpeakerConfig(sink_id=1, sink_name="sink_left", role=SpeakerRole.LEFT),
                "s2": SpeakerConfig(sink_id=2, sink_name="sink_right", role=SpeakerRole.RIGHT),
                "s3": SpeakerConfig(sink_id=3, sink_name="sink_sub", role=SpeakerRole.SUBWOOFER),
            }
        )
        config_text = self.generator.generate_config(cfg)
        self.assertIn("Polifonia 2.1 Studio Master", config_text)
        self.assertIn("filter-chain", config_text)
        self.assertIn("bq_highpass", config_text)
        self.assertIn("bq_lowpass", config_text)

if __name__ == "__main__":
    unittest.main()
