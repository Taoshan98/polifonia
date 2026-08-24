"""Unit tests for Polifonia storage, preset management, and XDG persistence."""

import unittest
import tempfile
import shutil
import json
from pathlib import Path
from core.models import SystemConfig, SpeakerConfig, SpeakerRole
from storage.settings_store import StorageService, ProfileStorageService, SettingsStore, PresetManager


class TestStorage(unittest.TestCase):
    """Test suite for settings store and profile serialization."""

    def setUp(self):
        self.temp_dir = tempfile.mkdtemp(prefix="polifonia_test_config_")
        self.storage = StorageService(config_dir=self.temp_dir)

    def tearDown(self):
        if Path(self.temp_dir).exists():
            shutil.rmtree(self.temp_dir)

    def test_directory_structure_initialization(self):
        """Verify storage directories are automatically created."""
        base = Path(self.temp_dir)
        profiles_dir = base / "profiles"
        self.assertTrue(base.exists())
        self.assertTrue(profiles_dir.exists())

    def test_save_and_load_profile(self):
        """Verify saving and retrieving named setup profiles."""
        cfg = SystemConfig(
            profile_name="Studio Gaming 2.1",
            master_volume=0.8,
            crossover_enabled=True,
            crossover_freq=120,
            speakers={
                "mon_left": SpeakerConfig(sink_id=1, sink_name="hdmi_l", role=SpeakerRole.LEFT, delay_ms=10.0),
                "mon_right": SpeakerConfig(sink_id=2, sink_name="hdmi_r", role=SpeakerRole.RIGHT, delay_ms=10.0),
                "sub_aux": SpeakerConfig(sink_id=3, sink_name="aux_sub", role=SpeakerRole.SUBWOOFER, delay_ms=0.0)
            }
        )

        self.storage.save_profile(cfg, "gaming_21")

        # Check file presence
        profile_file = Path(self.temp_dir) / "profiles" / "gaming_21.json"
        current_file = Path(self.temp_dir) / "current.json"
        self.assertTrue(profile_file.exists())
        self.assertTrue(current_file.exists())

        # Load named profile
        loaded_named = self.storage.load_profile("gaming_21")
        self.assertIsNotNone(loaded_named)
        self.assertEqual(loaded_named.profile_name, "gaming_21")
        self.assertEqual(loaded_named.master_volume, 0.8)
        self.assertEqual(len(loaded_named.speakers), 3)
        self.assertEqual(loaded_named.speakers["mon_left"].delay_ms, 10.0)

        # Load active profile
        loaded_active = self.storage.load_active_profile()
        self.assertEqual(loaded_active.profile_name, "gaming_21")
        self.assertEqual(loaded_active.crossover_freq, 120)

    def test_list_profiles(self):
        """Verify listing all saved profile file stems."""
        cfg1 = SystemConfig(profile_name="Profile 1")
        cfg2 = SystemConfig(profile_name="Profile 2")
        cfg3 = SystemConfig(profile_name="Profile 3")

        self.storage.save_profile(cfg1, "profile_alpha")
        self.storage.save_profile(cfg2, "profile_beta")
        self.storage.save_profile(cfg3, "profile_gamma")

        profiles = sorted(self.storage.list_profiles())
        self.assertEqual(profiles, ["profile_alpha", "profile_beta", "profile_gamma"])

    def test_load_non_existent_profile(self):
        """Verify loading an unknown profile returns None."""
        result = self.storage.load_profile("non_existent_profile_404")
        self.assertIsNone(result)

    def test_load_corrupted_active_profile_fallback(self):
        """Verify corrupt current.json gracefully defaults to fresh SystemConfig."""
        current_file = Path(self.temp_dir) / "current.json"
        with open(current_file, "w", encoding="utf-8") as f:
            f.write("{ INVALID JSON CONTENT :::")

        result = self.storage.load_active_profile()
        self.assertIsInstance(result, SystemConfig)
        self.assertEqual(result.profile_name, "Default Setup")

    def test_load_active_profile_when_file_absent(self):
        """Verify default SystemConfig returned when current.json is missing."""
        current_file = Path(self.temp_dir) / "current.json"
        if current_file.exists():
            current_file.unlink()

        result = self.storage.load_active_profile()
        self.assertIsInstance(result, SystemConfig)
        self.assertEqual(result.master_volume, 1.0)

    def test_storage_alias_methods(self):
        """Verify load(), save(), load_config(), save_config() aliases."""
        cfg = SystemConfig(profile_name="Alias Test", master_volume=0.65)
        self.storage.save_config(cfg)

        loaded_cfg = self.storage.load_config()
        self.assertEqual(loaded_cfg.profile_name, "Alias Test")
        self.assertEqual(loaded_cfg.master_volume, 0.65)

        self.storage.save(cfg)
        loaded = self.storage.load()
        self.assertEqual(loaded.profile_name, "Alias Test")

    def test_class_aliases(self):
        """Verify class aliases point to StorageService."""
        self.assertIs(ProfileStorageService, StorageService)
        self.assertIs(SettingsStore, StorageService)
        self.assertIs(PresetManager, StorageService)


if __name__ == "__main__":
    unittest.main()
