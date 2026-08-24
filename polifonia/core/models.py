"""Domain data models for audio sinks, speaker roles, and acoustic configuration."""

from dataclasses import dataclass, field, asdict
from enum import Enum
from typing import Dict, Any, Optional


class SpeakerRole(str, Enum):
    LEFT = "left"
    RIGHT = "right"
    SUBWOOFER = "subwoofer"
    CENTER = "center"
    EXCLUDED = "excluded"


@dataclass
class AudioSink:
    """Represents a physical or virtual PipeWire audio sink output."""
    id: int
    name: str
    description: str
    media_class: str
    channel_count: int = 2
    is_default: bool = False
    is_internal: bool = False
    volume: float = 1.0
    mute: bool = False

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)


@dataclass
class SpeakerConfig:
    """Acoustic DSP settings for an assigned speaker sink."""
    sink_id: int
    sink_name: str
    role: SpeakerRole = SpeakerRole.EXCLUDED
    volume_gain: float = 1.0  # Multiplier (0.0 to 1.5)
    delay_ms: float = 0.0     # Time-alignment delay in milliseconds (0 to 100ms)
    mute: bool = False

    def to_dict(self) -> Dict[str, Any]:
        data = asdict(self)
        data['role'] = self.role.value
        return data

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "SpeakerConfig":
        return SpeakerConfig(
            sink_id=int(d.get("sink_id", 0)),
            sink_name=str(d.get("sink_name", "")),
            role=SpeakerRole(d.get("role", "excluded")),
            volume_gain=float(d.get("volume_gain", 1.0)),
            delay_ms=float(d.get("delay_ms", 0.0)),
            mute=bool(d.get("mute", False))
        )


@dataclass
class SystemConfig:
    """Full setup profile with 2.1 crossover and master configuration."""
    profile_name: str = "Default Setup"
    master_volume: float = 1.0
    crossover_enabled: bool = True
    crossover_freq: int = 90  # Cutoff frequency in Hz (50Hz - 250Hz)
    speakers: Dict[str, SpeakerConfig] = field(default_factory=dict)
    auto_start: bool = False

    def to_dict(self) -> Dict[str, Any]:
        return {
            "profile_name": self.profile_name,
            "master_volume": self.master_volume,
            "crossover_enabled": self.crossover_enabled,
            "crossover_freq": self.crossover_freq,
            "auto_start": self.auto_start,
            "speakers": {k: v.to_dict() for k, v in self.speakers.items()}
        }

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "SystemConfig":
        speakers = {}
        if "speakers" in data and isinstance(data["speakers"], dict):
            for k, v in data["speakers"].items():
                speakers[k] = SpeakerConfig.from_dict(v)

        return cls(
            profile_name=data.get("profile_name", "Default Setup"),
            master_volume=float(data.get("master_volume", 1.0)),
            crossover_enabled=bool(data.get("crossover_enabled", True)),
            crossover_freq=int(data.get("crossover_freq", 90)),
            speakers=speakers,
            auto_start=bool(data.get("auto_start", False))
        )

# Aliases
SpeakerChannel = SpeakerConfig
SinkDevice = AudioSink
