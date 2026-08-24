"""Profile persistence service storing configurations in standard XDG user directories."""

import os
import json
from pathlib import Path
from typing import Optional, List
from polifonia.core.models import SystemConfig


class StorageService:
    def __init__(self, config_dir: Optional[str] = None):
        if config_dir:
            self.base_dir = Path(config_dir)
        else:
            xdg_config = os.getenv("XDG_CONFIG_HOME", str(Path.home() / ".config"))
            self.base_dir = Path(xdg_config) / "polifonia"
        
        self.profiles_dir = self.base_dir / "profiles"
        self.profiles_dir.mkdir(parents=True, exist_ok=True)
        self.active_profile_file = self.base_dir / "current.json"

    def save_profile(self, config: SystemConfig, profile_name: Optional[str] = None) -> None:
        name = profile_name or config.profile_name or "default"
        config.profile_name = name
        target = self.profiles_dir / f"{name}.json"
        
        with open(target, "w", encoding="utf-8") as f:
            json.dump(config.to_dict(), f, indent=2)
            
        with open(self.active_profile_file, "w", encoding="utf-8") as f:
            json.dump(config.to_dict(), f, indent=2)

    def load_active_profile(self) -> SystemConfig:
        if self.active_profile_file.exists():
            try:
                with open(self.active_profile_file, "r", encoding="utf-8") as f:
                    data = json.load(f)
                    return SystemConfig.from_dict(data)
            except Exception:
                pass
        return SystemConfig()

    def list_profiles(self) -> List[str]:
        return [f.stem for f in self.profiles_dir.glob("*.json")]

    def load_profile(self, name: str) -> Optional[SystemConfig]:
        target = self.profiles_dir / f"{name}.json"
        if not target.exists():
            return None
        with open(target, "r", encoding="utf-8") as f:
            return SystemConfig.from_dict(json.load(f))

    def load(self) -> SystemConfig:
        return self.load_active_profile()

    def save(self, config: SystemConfig) -> None:
        self.save_profile(config)

ProfileStorageService = StorageService
SettingsStore = StorageService
