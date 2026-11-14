//! Atomic XDG profile persistence service for Polifonia configuration files.

use crate::models::SystemConfig;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Manages reading and atomic writing of `current.json` and named profile presets.
#[derive(Debug, Clone)]
pub struct StorageService {
    #[allow(dead_code)]
    pub base_dir: PathBuf,
    pub profiles_dir: PathBuf,
    pub active_profile_file: PathBuf,
}

impl StorageService {
    /// Initializes storage directories under `config_dir` or `$XDG_CONFIG_HOME/polifonia`.
    pub fn new(config_dir: Option<&Path>) -> Self {
        let base_dir = match config_dir {
            Some(dir) => dir.to_path_buf(),
            None => {
                let xdg = env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or_else(|| {
                        let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
                        home.join(".config")
                    });
                xdg.join("polifonia")
            }
        };

        let profiles_dir = base_dir.join("profiles");
        let _ = fs::create_dir_all(&profiles_dir);
        let active_profile_file = base_dir.join("current.json");

        Self {
            base_dir,
            profiles_dir,
            active_profile_file,
        }
    }

    pub fn default_store() -> Self {
        Self::new(None)
    }

    /// Loads `current.json`, returning a clean default `SystemConfig` if missing or corrupted.
    pub fn load(&self) -> SystemConfig {
        if let Ok(contents) = fs::read_to_string(&self.active_profile_file) {
            if let Ok(cfg) = serde_json::from_str::<SystemConfig>(&contents) {
                return cfg;
            }
        }
        SystemConfig::default()
    }

    /// Atomically saves both `profiles/<name>.json` and `current.json` via temporary files.
    pub fn save(&self, config: &mut SystemConfig, profile_name: Option<&str>) -> io::Result<()> {
        let resolved_name = profile_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                if config.profile_name.trim().is_empty() {
                    "default".to_string()
                } else {
                    config.profile_name.clone()
                }
            });

        config.profile_name = resolved_name.clone();
        let _ = fs::create_dir_all(&self.profiles_dir);

        let sanitized = resolved_name.replace(['/', '\\'], "_");
        let profile_target = self.profiles_dir.join(format!("{sanitized}.json"));
        let json_payload = serde_json::to_string_pretty(config)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        Self::atomic_write(&profile_target, json_payload.as_bytes())?;
        Self::atomic_write(&self.active_profile_file, json_payload.as_bytes())?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn list_profiles(&self) -> Vec<String> {
        let mut names = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.profiles_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|s| s.to_str()) == Some("json") {
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                        names.push(stem.to_string());
                    }
                }
            }
        }
        names.sort();
        names
    }

    #[allow(dead_code)]
    pub fn load_profile(&self, name: &str) -> Option<SystemConfig> {
        let target = self.profiles_dir.join(format!("{name}.json"));
        let content = fs::read_to_string(target).ok()?;
        serde_json::from_str::<SystemConfig>(&content).ok()
    }

    fn atomic_write(target: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut tmp_name = target.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(".tmp");
        let tmp_path = target.with_file_name(tmp_name);
        fs::write(&tmp_path, bytes)?;
        fs::rename(&tmp_path, target)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{SpeakerConfig, SpeakerRole};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        env::temp_dir().join(format!("polifonia_storage_test_{label}_{nanos}"))
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = temp_dir("roundtrip");
        let store = StorageService::new(Some(&dir));
        let mut cfg = SystemConfig {
            profile_name: "Studio Test".to_string(),
            master_volume: 0.85,
            is_active: true,
            ..Default::default()
        };
        cfg.speakers.insert(
            "hdmi_1".to_string(),
            SpeakerConfig {
                sink_id: 42,
                sink_name: "hdmi_1".to_string(),
                display_name: "Monitor HDMI".to_string(),
                role: SpeakerRole::Stereo,
                volume_gain: 0.9,
                ..Default::default()
            },
        );

        store.save(&mut cfg, None).unwrap();
        let loaded = store.load();
        assert_eq!(loaded.profile_name, "Studio Test");
        assert_eq!(loaded.master_volume, 0.85);
        assert!(loaded.is_active);
        assert_eq!(loaded.speakers["hdmi_1"].sink_id, 42);
        assert!(store.list_profiles().contains(&"Studio Test".to_string()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_corrupt_json_falls_back_cleanly() {
        let dir = temp_dir("corrupt");
        let store = StorageService::new(Some(&dir));
        fs::write(&store.active_profile_file, "{not_valid_json").unwrap();
        let loaded = store.load();
        assert_eq!(loaded.profile_name, "Default Setup");
        assert!(loaded.speakers.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }
}
