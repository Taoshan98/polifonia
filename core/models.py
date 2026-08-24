"""Domain data models for audio sinks, speaker roles, and acoustic configuration."""

from dataclasses import dataclass, field, asdict
from enum import Enum
from typing import Dict, Any, Optional, List


class SpeakerRole(str, Enum):
    STEREO = "stereo"
    LEFT = "left"
    RIGHT = "right"
    SUBWOOFER = "subwoofer"
    CENTER = "center"
    SURROUND_LEFT = "surround_left"
    SURROUND_RIGHT = "surround_right"
    EXCLUDED = "excluded"
    DISABLED = "excluded"


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
    sink_id: int = 0
    sink_name: str = ""
    display_name: str = ""
    role: SpeakerRole = SpeakerRole.EXCLUDED
    volume_gain: float = 1.0  # Multiplier (0.0 to 1.5)
    delay_ms: float = 0.0     # Time-alignment delay in milliseconds (0 to 100ms)
    mute: bool = False
    custom_name: Optional[str] = None
    phase_inverted: bool = False

    @property
    def id(self) -> int:
        return self.sink_id

    @id.setter
    def id(self, val: int):
        self.sink_id = val

    @property
    def name(self) -> str:
        return self.sink_name

    @name.setter
    def name(self, val: str):
        self.sink_name = val

    @property
    def gain(self) -> float:
        return self.volume_gain

    @gain.setter
    def gain(self, val: float):
        self.volume_gain = val

    def to_dict(self) -> Dict[str, Any]:
        return {
            "sink_id": self.sink_id,
            "sink_name": self.sink_name,
            "display_name": self.display_name,
            "role": self.role.value if isinstance(self.role, SpeakerRole) else str(self.role),
            "volume_gain": self.volume_gain,
            "delay_ms": self.delay_ms,
            "mute": self.mute,
            "custom_name": self.custom_name,
            "phase_inverted": self.phase_inverted
        }

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "SpeakerConfig":
        role_val = d.get("role", "excluded")
        try:
            role = SpeakerRole(role_val)
        except ValueError:
            role = SpeakerRole.EXCLUDED

        return SpeakerConfig(
            sink_id=int(d.get("sink_id", d.get("id", 0))),
            sink_name=str(d.get("sink_name", d.get("name", ""))),
            display_name=str(d.get("display_name", d.get("description", ""))),
            role=role,
            volume_gain=float(d.get("volume_gain", d.get("gain", 1.0))),
            delay_ms=float(d.get("delay_ms", 0.0)),
            mute=bool(d.get("mute", False)),
            custom_name=d.get("custom_name"),
            phase_inverted=bool(d.get("phase_inverted", False))
        )


class CrossoverConfig:
    """Crossover frequency filtering configuration with live synchronization."""
    def __init__(self, parent: Optional['SystemConfig'] = None, enabled: bool = True, frequency_hz: int = 90):
        self._parent = parent
        self._enabled = enabled
        self._frequency_hz = frequency_hz

    @property
    def enabled(self) -> bool:
        if self._parent is not None:
            return self._parent._crossover_enabled
        return self._enabled

    @enabled.setter
    def enabled(self, val: bool):
        self._enabled = bool(val)
        if self._parent is not None:
            self._parent._crossover_enabled = bool(val)

    @property
    def frequency_hz(self) -> int:
        if self._parent is not None:
            return self._parent._crossover_freq
        return self._frequency_hz

    @frequency_hz.setter
    def frequency_hz(self, val: int):
        self._frequency_hz = int(val)
        if self._parent is not None:
            self._parent._crossover_freq = int(val)

    @property
    def freq(self) -> int:
        return self.frequency_hz

    @freq.setter
    def freq(self, val: int):
        self.frequency_hz = val

    def to_dict(self) -> Dict[str, Any]:
        return {
            "enabled": self.enabled,
            "frequency_hz": self.frequency_hz
        }

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "CrossoverConfig":
        return cls(
            enabled=bool(data.get("enabled", True)),
            frequency_hz=int(data.get("frequency_hz", data.get("freq", 90)))
        )


class SystemConfig:
    """Full setup profile with 2.1 crossover and master configuration."""
    def __init__(
        self,
        profile_name: str = "Default Setup",
        master_volume: float = 1.0,
        crossover_enabled: bool = True,
        crossover_freq: int = 90,
        speakers: Optional[Any] = None,
        auto_start: bool = False,
        is_active: bool = False,
        set_as_default: bool = False,
        crossover: Optional[Any] = None,
        channels: Optional[List[SpeakerConfig]] = None,
        **kwargs
    ):
        self.profile_name = profile_name
        self.master_volume = float(kwargs.get("master_gain", master_volume))
        self._crossover_enabled = bool(crossover_enabled)
        self._crossover_freq = int(kwargs.get("crossover_frequency", crossover_freq))
        self.auto_start = bool(auto_start)
        self.is_active = bool(is_active)
        self.set_as_default = bool(set_as_default)

        if speakers is not None:
            if isinstance(speakers, dict):
                self.speakers = dict(speakers)
            elif isinstance(speakers, list):
                self.speakers = {spk.sink_name or f"sink_{spk.sink_id}": spk for spk in speakers}
            else:
                self.speakers = {}
        elif channels is not None:
            self.speakers = {spk.sink_name or f"sink_{spk.sink_id}": spk for spk in channels}
        else:
            self.speakers = {}

        if crossover is not None:
            if isinstance(crossover, dict):
                self._crossover_enabled = bool(crossover.get("enabled", self._crossover_enabled))
                self._crossover_freq = int(crossover.get("frequency_hz", crossover.get("freq", self._crossover_freq)))
            elif hasattr(crossover, "enabled"):
                self._crossover_enabled = bool(crossover.enabled)
                self._crossover_freq = int(getattr(crossover, "frequency_hz", getattr(crossover, "freq", self._crossover_freq)))

        self.crossover = CrossoverConfig(parent=self, enabled=self._crossover_enabled, frequency_hz=self._crossover_freq)

    @property
    def crossover_enabled(self) -> bool:
        return self._crossover_enabled

    @crossover_enabled.setter
    def crossover_enabled(self, val: bool):
        self._crossover_enabled = bool(val)

    @property
    def crossover_freq(self) -> int:
        return self._crossover_freq

    @crossover_freq.setter
    def crossover_freq(self, val: int):
        self._crossover_freq = int(val)

    @property
    def master_gain(self) -> float:
        return self.master_volume

    @master_gain.setter
    def master_gain(self, val: float):
        self.master_volume = float(val)

    @property
    def channels(self) -> List[SpeakerConfig]:
        return list(self.speakers.values())

    @channels.setter
    def channels(self, val: List[SpeakerConfig]):
        self.speakers = {spk.sink_name or f"sink_{spk.sink_id}": spk for spk in val}

    def to_dict(self) -> Dict[str, Any]:
        return {
            "profile_name": self.profile_name,
            "master_volume": self.master_volume,
            "crossover_enabled": self.crossover_enabled,
            "crossover_freq": self.crossover_freq,
            "crossover": self.crossover.to_dict(),
            "auto_start": self.auto_start,
            "is_active": self.is_active,
            "set_as_default": self.set_as_default,
            "speakers": {k: v.to_dict() for k, v in self.speakers.items()}
        }

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "SystemConfig":
        speakers = {}
        if "speakers" in data:
            if isinstance(data["speakers"], dict):
                for k, v in data["speakers"].items():
                    speakers[k] = SpeakerConfig.from_dict(v)
            elif isinstance(data["speakers"], list):
                for item in data["speakers"]:
                    spk = SpeakerConfig.from_dict(item)
                    speakers[spk.sink_name or f"sink_{spk.sink_id}"] = spk
        elif "channels" in data and isinstance(data["channels"], list):
            for item in data["channels"]:
                spk = SpeakerConfig.from_dict(item)
                speakers[spk.sink_name or f"sink_{spk.sink_id}"] = spk

        crossover = None
        if "crossover" in data and isinstance(data["crossover"], dict):
            crossover = CrossoverConfig.from_dict(data["crossover"])

        return cls(
            profile_name=data.get("profile_name", "Default Setup"),
            master_volume=float(data.get("master_volume", data.get("master_gain", 1.0))),
            crossover_enabled=bool(data.get("crossover_enabled", True)),
            crossover_freq=int(data.get("crossover_freq", data.get("crossover_frequency", 90))),
            crossover=crossover,
            speakers=speakers,
            auto_start=bool(data.get("auto_start", False)),
            is_active=bool(data.get("is_active", False)),
            set_as_default=bool(data.get("set_as_default", False))
        )



# Aliases
SpeakerChannel = SpeakerConfig
SinkDevice = AudioSink
