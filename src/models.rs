//! Domain data models for PipeWire audio sinks, speaker roles, and system configuration.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

/// Acoustic channel role assigned to a physical audio output sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpeakerRole {
    Stereo,
    Left,
    Right,
    Subwoofer,
    Center,
    SurroundLeft,
    SurroundRight,
    #[default]
    Excluded,
}

impl SpeakerRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            SpeakerRole::Stereo => "stereo",
            SpeakerRole::Left => "left",
            SpeakerRole::Right => "right",
            SpeakerRole::Subwoofer => "subwoofer",
            SpeakerRole::Center => "center",
            SpeakerRole::SurroundLeft => "surround_left",
            SpeakerRole::SurroundRight => "surround_right",
            SpeakerRole::Excluded => "excluded",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "stereo" => SpeakerRole::Stereo,
            "left" => SpeakerRole::Left,
            "right" => SpeakerRole::Right,
            "subwoofer" => SpeakerRole::Subwoofer,
            "center" => SpeakerRole::Center,
            "surround_left" => SpeakerRole::SurroundLeft,
            "surround_right" => SpeakerRole::SurroundRight,
            _ => SpeakerRole::Excluded,
        }
    }

    pub fn is_enabled(&self) -> bool {
        !matches!(self, SpeakerRole::Excluded)
    }

    pub fn short_label(&self) -> &'static str {
        match self {
            SpeakerRole::Stereo => "ST",
            SpeakerRole::Left => "L",
            SpeakerRole::Right => "R",
            SpeakerRole::Subwoofer => "SUB",
            SpeakerRole::Center => "C",
            SpeakerRole::SurroundLeft => "SL",
            SpeakerRole::SurroundRight => "SR",
            SpeakerRole::Excluded => "OFF",
        }
    }
}

impl Serialize for SpeakerRole {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SpeakerRole {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer).unwrap_or_default();
        Ok(SpeakerRole::from_str_lossy(&s))
    }
}

/// Discovered live PipeWire physical or virtual audio output node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioSink {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub media_class: String,
    pub channel_count: u32,
    pub is_default: bool,
    pub is_internal: bool,
    pub volume: f64,
    pub mute: bool,
    pub latency_ms: f64,
    pub bus_type: String,
}

impl Default for AudioSink {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            description: String::new(),
            media_class: "Audio/Sink".to_string(),
            channel_count: 2,
            is_default: false,
            is_internal: false,
            volume: 1.0,
            mute: false,
            latency_ms: 0.0,
            bus_type: "other".to_string(),
        }
    }
}

/// Persisted acoustic and DSP parameters for an individual speaker endpoint.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SpeakerConfig {
    pub sink_id: u32,
    pub sink_name: String,
    pub display_name: String,
    pub role: SpeakerRole,
    pub volume_gain: f64,
    pub delay_ms: f64,
    pub mute: bool,
    pub custom_name: Option<String>,
    pub phase_inverted: bool,
    pub hardware_latency_ms: f64,
    pub bus_type: String,
    pub is_internal: bool,
    #[serde(skip)]
    pub acoustic_role: SpeakerRole,
}

impl Default for SpeakerConfig {
    fn default() -> Self {
        Self {
            sink_id: 0,
            sink_name: String::new(),
            display_name: String::new(),
            role: SpeakerRole::Excluded,
            volume_gain: 1.0,
            delay_ms: 0.0,
            mute: false,
            custom_name: None,
            phase_inverted: false,
            hardware_latency_ms: 0.0,
            bus_type: "other".to_string(),
            is_internal: false,
            acoustic_role: SpeakerRole::Stereo,
        }
    }
}

impl SpeakerConfig {
    /// Builds a new configuration entry (initially Excluded) for a newly discovered sink.
    pub fn from_sink(sink: &AudioSink) -> Self {
        let mut sp = Self {
            sink_id: sink.id,
            sink_name: sink.name.clone(),
            display_name: sink.description.clone(),
            role: SpeakerRole::Excluded,
            volume_gain: 1.0,
            delay_ms: 0.0,
            mute: false,
            custom_name: None,
            phase_inverted: false,
            hardware_latency_ms: sink.latency_ms,
            bus_type: sink.bus_type.clone(),
            is_internal: sink.is_internal,
            acoustic_role: SpeakerRole::Stereo,
        };
        sp.normalize();
        sp
    }

    /// Ensures derived flags (`is_internal`, `acoustic_role`) stay consistent with sink metadata.
    pub fn normalize(&mut self) {
        if self.bus_type == "internal" || self.sink_name.to_ascii_lowercase().contains("hifi__speaker") {
            self.is_internal = true;
        }
        if self.role.is_enabled() {
            self.acoustic_role = self.role;
        } else if !self.acoustic_role.is_enabled() {
            self.acoustic_role = SpeakerRole::Stereo;
        }
    }

    /// Returns the acoustic routing role to apply (preserving the active role when temporarily excluded).
    pub fn effective_role(&self) -> SpeakerRole {
        if self.role.is_enabled() {
            self.role
        } else if self.acoustic_role.is_enabled() {
            self.acoustic_role
        } else {
            SpeakerRole::Stereo
        }
    }

    pub fn label(&self) -> &str {
        if let Some(ref custom) = self.custom_name {
            if !custom.trim().is_empty() {
                return custom.as_str();
            }
        }
        if !self.display_name.is_empty() {
            &self.display_name
        } else {
            &self.sink_name
        }
    }
}

impl<'de> Deserialize<'de> for SpeakerConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct RawSpeaker {
            sink_id: Option<u32>,
            id: Option<u32>,
            sink_name: Option<String>,
            name: Option<String>,
            display_name: Option<String>,
            description: Option<String>,
            role: Option<String>,
            volume_gain: Option<f64>,
            gain: Option<f64>,
            delay_ms: Option<f64>,
            mute: Option<bool>,
            custom_name: Option<String>,
            phase_inverted: Option<bool>,
            hardware_latency_ms: Option<f64>,
            bus_type: Option<String>,
            is_internal: Option<bool>,
        }

        let raw = RawSpeaker::deserialize(deserializer)?;
        let role = raw
            .role
            .as_deref()
            .map(SpeakerRole::from_str_lossy)
            .unwrap_or(SpeakerRole::Excluded);
        let acoustic_role = if role.is_enabled() {
            role
        } else {
            SpeakerRole::Stereo
        };

        let mut cfg = SpeakerConfig {
            sink_id: raw.sink_id.or(raw.id).unwrap_or(0),
            sink_name: raw.sink_name.or(raw.name).unwrap_or_default(),
            display_name: raw.display_name.or(raw.description).unwrap_or_default(),
            role,
            volume_gain: raw.volume_gain.or(raw.gain).unwrap_or(1.0),
            delay_ms: raw.delay_ms.unwrap_or(0.0),
            mute: raw.mute.unwrap_or(false),
            custom_name: raw.custom_name,
            phase_inverted: raw.phase_inverted.unwrap_or(false),
            hardware_latency_ms: raw.hardware_latency_ms.unwrap_or(0.0),
            bus_type: raw.bus_type.unwrap_or_else(|| "other".to_string()),
            is_internal: raw.is_internal.unwrap_or(false),
            acoustic_role,
        };
        cfg.normalize();
        Ok(cfg)
    }
}

/// Crossover configuration maintained alongside `SystemConfig`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CrossoverConfig {
    pub enabled: bool,
    pub frequency_hz: u32,
}

impl Default for CrossoverConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            frequency_hz: 90,
        }
    }
}

impl<'de> Deserialize<'de> for CrossoverConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct RawCross {
            enabled: Option<bool>,
            frequency_hz: Option<u32>,
            freq: Option<u32>,
        }
        let raw = RawCross::deserialize(deserializer)?;
        Ok(Self {
            enabled: raw.enabled.unwrap_or(false),
            frequency_hz: raw.frequency_hz.or(raw.freq).unwrap_or(90),
        })
    }
}

/// Full system setup profile serialized to `~/.config/polifonia/current.json`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SystemConfig {
    pub profile_name: String,
    pub master_volume: f64,
    pub crossover_enabled: bool,
    pub crossover_freq: u32,
    pub crossover: CrossoverConfig,
    pub auto_start: bool,
    pub is_active: bool,
    pub set_as_default: bool,
    pub speakers: BTreeMap<String, SpeakerConfig>,
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            profile_name: "Default Setup".to_string(),
            master_volume: 1.0,
            crossover_enabled: false,
            crossover_freq: 90,
            crossover: CrossoverConfig::default(),
            auto_start: false,
            is_active: false,
            set_as_default: true,
            speakers: BTreeMap::new(),
        }
    }
}

impl SystemConfig {
    #[allow(dead_code)]
    pub fn set_crossover(&mut self, enabled: bool, freq: u32) {
        self.crossover_enabled = enabled;
        self.crossover_freq = freq;
        self.crossover.enabled = enabled;
        self.crossover.frequency_hz = freq;
    }

    #[allow(dead_code)]
    pub fn active_speakers(&self) -> Vec<&SpeakerConfig> {
        self.speakers.values().filter(|s| s.role.is_enabled()).collect()
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawSpeakersField {
    Map(BTreeMap<String, SpeakerConfig>),
    List(Vec<SpeakerConfig>),
}

impl<'de> Deserialize<'de> for SystemConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct RawSystem {
            profile_name: Option<String>,
            master_volume: Option<f64>,
            master_gain: Option<f64>,
            crossover_enabled: Option<bool>,
            crossover_freq: Option<u32>,
            crossover_frequency: Option<u32>,
            crossover: Option<CrossoverConfig>,
            auto_start: Option<bool>,
            is_active: Option<bool>,
            set_as_default: Option<bool>,
            speakers: Option<RawSpeakersField>,
            channels: Option<RawSpeakersField>,
        }

        let raw = RawSystem::deserialize(deserializer)?;
        let mut cross_en = raw.crossover_enabled.unwrap_or(false);
        let mut cross_freq = raw.crossover_freq.or(raw.crossover_frequency).unwrap_or(90);
        if let Some(c) = raw.crossover {
            cross_en = c.enabled;
            cross_freq = c.frequency_hz;
        }

        let mut speakers = BTreeMap::new();
        if let Some(field) = raw.speakers.or(raw.channels) {
            match field {
                RawSpeakersField::Map(map) => {
                    for (k, mut v) in map {
                        if v.sink_name.is_empty() {
                            v.sink_name = k.clone();
                        }
                        v.normalize();
                        speakers.insert(k, v);
                    }
                }
                RawSpeakersField::List(list) => {
                    for mut item in list {
                        item.normalize();
                        let key = if item.sink_name.is_empty() {
                            format!("sink_{}", item.sink_id)
                        } else {
                            item.sink_name.clone()
                        };
                        speakers.insert(key, item);
                    }
                }
            }
        }

        Ok(SystemConfig {
            profile_name: raw.profile_name.unwrap_or_else(|| "Default Setup".to_string()),
            master_volume: raw.master_volume.or(raw.master_gain).unwrap_or(1.0),
            crossover_enabled: cross_en,
            crossover_freq: cross_freq,
            crossover: CrossoverConfig {
                enabled: cross_en,
                frequency_hz: cross_freq,
            },
            auto_start: raw.auto_start.unwrap_or(false),
            is_active: raw.is_active.unwrap_or(false),
            set_as_default: raw.set_as_default.unwrap_or(true),
            speakers,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_speaker_role_roundtrip_and_fallback() {
        assert_eq!(SpeakerRole::from_str_lossy("subwoofer"), SpeakerRole::Subwoofer);
        assert_eq!(SpeakerRole::from_str_lossy("disabled"), SpeakerRole::Excluded);
        assert_eq!(SpeakerRole::from_str_lossy("invalid_xyz"), SpeakerRole::Excluded);
        assert_eq!(serde_json::to_string(&SpeakerRole::SurroundLeft).unwrap(), "\"surround_left\"");
    }

    #[test]
    fn test_speaker_config_internal_detection_and_aliases() {
        let legacy = r#"{
            "id": 55,
            "name": "alsa_output.pci-0000_00_1f.3.HiFi__Speaker__sink",
            "description": "Speaker",
            "role": "excluded",
            "gain": 0.9
        }"#;
        let spk: SpeakerConfig = serde_json::from_str(legacy).unwrap();
        assert_eq!(spk.sink_id, 55);
        assert_eq!(spk.volume_gain, 0.9);
        assert!(spk.is_internal);
    }

    #[test]
    fn test_system_config_legacy_list_and_crossover_sync() {
        let payload = r#"{
            "profile_name": "Legacy Setup",
            "master_gain": 0.75,
            "crossover": { "enabled": true, "freq": 125 },
            "channels": [
                {"sink_id": 10, "sink_name": "hdmi_l", "role": "left"},
                {"sink_id": 20, "sink_name": "hdmi_r", "role": "right"}
            ]
        }"#;
        let cfg: SystemConfig = serde_json::from_str(payload).unwrap();
        assert_eq!(cfg.profile_name, "Legacy Setup");
        assert_eq!(cfg.master_volume, 0.75);
        assert!(cfg.crossover_enabled);
        assert_eq!(cfg.crossover_freq, 125);
        assert_eq!(cfg.speakers.len(), 2);
        assert_eq!(cfg.speakers["hdmi_l"].role, SpeakerRole::Left);
    }
}
