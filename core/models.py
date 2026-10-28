"""Domain data models for audio sinks, speaker roles, and acoustic configuration."""

from dataclasses import asdict, dataclass
from enum import Enum
from typing import Any, Dict, List, Optional


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
    latency_ms: float = 0.0
    bus_type: str = "other"

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)


@dataclass
class SpeakerConfig:
    """Acoustic DSP settings for an assigned speaker sink."""
    sink_id: int = 0
    sink_name: str = ""
    display_name: str = ""
    role: SpeakerRole = SpeakerRole.EXCLUDED
    volume_gain: float = 1.0
    delay_ms: float = 0.0
    mute: bool = False
    custom_name: Optional[str] = None
    phase_inverted: bool = False
    hardware_latency_ms: float = 0.0
    bus_type: str = "other"
    is_internal: bool = False
    _acoustic_role: SpeakerRole = SpeakerRole.STEREO

    def __post_init__(self):
        if self.bus_type == "internal" or "hifi__speaker" in self.sink_name.lower():
            self.is_internal = True

    @property
    def effective_role(self) -> SpeakerRole:
        return getattr(self, "_acoustic_role", SpeakerRole.STEREO) if self.role in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED) else self.role

    id = property(lambda self: self.sink_id, lambda self, v: setattr(self, "sink_id", v))
    name = property(lambda self: self.sink_name, lambda self, v: setattr(self, "sink_name", v))
    gain = property(lambda self: self.volume_gain, lambda self, v: setattr(self, "volume_gain", v))

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
            "phase_inverted": self.phase_inverted,
            "hardware_latency_ms": self.hardware_latency_ms,
            "bus_type": self.bus_type,
            "is_internal": self.is_internal,
        }

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "SpeakerConfig":
        try:
            role = SpeakerRole(d.get("role", "excluded"))
        except ValueError:
            role = SpeakerRole.EXCLUDED

        cfg = cls(
            sink_id=int(d.get("sink_id", d.get("id", 0))),
            sink_name=str(d.get("sink_name", d.get("name", ""))),
            display_name=str(d.get("display_name", d.get("description", ""))),
            role=role,
            volume_gain=float(d.get("volume_gain", d.get("gain", 1.0))),
            delay_ms=float(d.get("delay_ms", 0.0)),
            mute=bool(d.get("mute", False)),
            custom_name=d.get("custom_name"),
            phase_inverted=bool(d.get("phase_inverted", False)),
            hardware_latency_ms=float(d.get("hardware_latency_ms", 0.0)),
            bus_type=str(d.get("bus_type", "other")),
            is_internal=bool(d.get("is_internal", False)),
        )
        if role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED):
            cfg._acoustic_role = role
        return cfg


class CrossoverConfig:
    """Crossover frequency filtering configuration with live parent synchronization."""
    def __init__(self, parent: Optional["SystemConfig"] = None, enabled: bool = True, frequency_hz: int = 90):
        self._parent = parent
        self._enabled = bool(enabled)
        self._frequency_hz = int(frequency_hz)

    @property
    def enabled(self) -> bool:
        return self._parent._crossover_enabled if self._parent is not None else self._enabled

    @enabled.setter
    def enabled(self, val: bool):
        self._enabled = bool(val)
        if self._parent is not None:
            self._parent._crossover_enabled = self._enabled

    @property
    def frequency_hz(self) -> int:
        return self._parent._crossover_freq if self._parent is not None else self._frequency_hz

    @frequency_hz.setter
    def frequency_hz(self, val: int):
        self._frequency_hz = int(val)
        if self._parent is not None:
            self._parent._crossover_freq = self._frequency_hz

    freq = property(lambda self: self.frequency_hz, lambda self, v: setattr(self, "frequency_hz", v))

    def to_dict(self) -> Dict[str, Any]:
        return {"enabled": self.enabled, "frequency_hz": self.frequency_hz}

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "CrossoverConfig":
        return cls(
            enabled=bool(data.get("enabled", True)),
            frequency_hz=int(data.get("frequency_hz", data.get("freq", 90))),
        )


class SystemConfig:
    """Full setup profile with crossover and master configuration."""
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
        **kwargs,
    ):
        self.profile_name = profile_name
        self.master_volume = float(kwargs.get("master_gain", master_volume))
        self._crossover_enabled = bool(crossover_enabled)
        self._crossover_freq = int(kwargs.get("crossover_frequency", crossover_freq))
        self.auto_start = bool(auto_start)
        self.is_active = bool(is_active)
        self.set_as_default = bool(set_as_default)

        src = speakers if speakers is not None else channels
        if isinstance(src, dict):
            self.speakers = dict(src)
        elif isinstance(src, list):
            self.speakers = {s.sink_name or f"sink_{s.sink_id}": s for s in src}
        else:
            self.speakers = {}

        if isinstance(crossover, dict):
            self._crossover_enabled = bool(crossover.get("enabled", self._crossover_enabled))
            self._crossover_freq = int(crossover.get("frequency_hz", crossover.get("freq", self._crossover_freq)))
        elif hasattr(crossover, "enabled"):
            self._crossover_enabled = bool(crossover.enabled)
            self._crossover_freq = int(getattr(crossover, "frequency_hz", getattr(crossover, "freq", self._crossover_freq)))

        self.crossover = CrossoverConfig(parent=self, enabled=self._crossover_enabled, frequency_hz=self._crossover_freq)

    crossover_enabled = property(lambda self: self._crossover_enabled, lambda self, v: setattr(self, "_crossover_enabled", bool(v)))
    crossover_freq = property(lambda self: self._crossover_freq, lambda self, v: setattr(self, "_crossover_freq", int(v)))
    master_gain = property(lambda self: self.master_volume, lambda self, v: setattr(self, "master_volume", float(v)))
    channels = property(
        lambda self: list(self.speakers.values()),
        lambda self, val: setattr(self, "speakers", {s.sink_name or f"sink_{s.sink_id}": s for s in val}),
    )

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
            "speakers": {k: v.to_dict() for k, v in self.speakers.items()},
        }

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "SystemConfig":
        speakers: Dict[str, SpeakerConfig] = {}
        raw_spks = data.get("speakers", data.get("channels"))
        if isinstance(raw_spks, dict):
            speakers = {k: SpeakerConfig.from_dict(v) for k, v in raw_spks.items()}
        elif isinstance(raw_spks, list):
            for item in raw_spks:
                spk = SpeakerConfig.from_dict(item)
                speakers[spk.sink_name or f"sink_{spk.sink_id}"] = spk

        cross_data = data.get("crossover")
        crossover = CrossoverConfig.from_dict(cross_data) if isinstance(cross_data, dict) else None

        return cls(
            profile_name=data.get("profile_name", "Default Setup"),
            master_volume=float(data.get("master_volume", data.get("master_gain", 1.0))),
            crossover_enabled=bool(data.get("crossover_enabled", True)),
            crossover_freq=int(data.get("crossover_freq", data.get("crossover_frequency", 90))),
            crossover=crossover,
            speakers=speakers,
            auto_start=bool(data.get("auto_start", False)),
            is_active=bool(data.get("is_active", False)),
            set_as_default=bool(data.get("set_as_default", False)),
        )


SpeakerChannel = SpeakerConfig
SinkDevice = AudioSink
