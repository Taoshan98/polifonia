//! Inspects system ALSA hardware and PipeWire topology to classify audio sinks and client streams.

use crate::models::AudioSink;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EldMonitor {
    pub name: String,
    pub connection: String,
    pub channels: u32,
    pub pin: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SinkInputInfo {
    pub id: String,
    pub sink_id: Option<u32>,
    pub app_name: String,
    pub icon_name: String,
    pub media_name: String,
    pub volume_pct: u32,
    pub is_active: bool,
    pub corked: bool,
    pub muted: bool,
}

#[derive(Default)]
struct ScannerCache {
    last_profile_check: Option<Instant>,
    initialized_iec958_cards: HashSet<u32>,
    last_eld_check: Option<Instant>,
    cached_eld_map: BTreeMap<u32, Vec<EldMonitor>>,
    last_scan_time: Option<Instant>,
    cached_sinks: Vec<AudioSink>,
}

fn cache() -> &'static Mutex<ScannerCache> {
    static CACHE: OnceLock<Mutex<ScannerCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(ScannerCache::default()))
}

/// Executes a short-lived CLI tool with a bounded wall-clock timeout, returning stdout if successful.
pub(crate) fn run_cmd(program: &str, args: &[&str], timeout_ms: u64) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let stdout_pipe = child.stdout.take();
    let reader_handle = stdout_pipe.map(|mut out| {
        thread::spawn(move || {
            let mut buf = String::new();
            let _ = std::io::Read::read_to_string(&mut out, &mut buf);
            buf
        })
    });

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let text = reader_handle.and_then(|h| h.join().ok()).unwrap_or_default();
                return if status.success() { Some(text) } else { None };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                thread::sleep(Duration::from_millis(8));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Runs a fire-and-forget CLI command quietly within the given timeout.
pub(crate) fn run_quiet(program: &str, args: &[&str], timeout_ms: u64) {
    let _ = run_cmd(program, args, timeout_ms);
}

fn prop_str<'a>(props: &'a Value, key: &str) -> &'a str {
    props.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

fn prop_u32(props: &Value, key: &str) -> Option<u32> {
    let v = props.get(key)?;
    if let Some(n) = v.as_u64() {
        Some(n as u32)
    } else if let Some(s) = v.as_str() {
        s.trim().parse::<u32>().ok()
    } else {
        None
    }
}

fn extract_leading_digits(s: &str) -> Option<u32> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse::<u32>().ok()
    }
}

fn is_generic_display_port_name(lower: &str) -> bool {
    let rest = if let Some(r) = lower.strip_prefix("hdmi") {
        r
    } else if let Some(r) = lower.strip_prefix("displayport") {
        r
    } else {
        return false;
    };
    let trimmed = rest.trim();
    trimmed.is_empty() || trimmed.chars().all(|c| c.is_ascii_digit())
}

pub struct DeviceScanner;

impl DeviceScanner {
    /// Returns the PipeWire node ID of the current default sink via `wpctl status`.
    pub fn get_default_sink_id() -> Option<u32> {
        let stdout = run_cmd("wpctl", &["status"], 1500)?;
        Self::parse_default_sink_id(&stdout)
    }

    pub fn parse_default_sink_id(stdout: &str) -> Option<u32> {
        for line in stdout.lines() {
            if line.contains('*') && (line.contains("Audio/Sink") || (line.contains('[') && line.contains("vol:"))) {
                if let Some(pos) = line.find('*') {
                    let after = line[pos + 1..].trim_start();
                    if let Some(dot_idx) = after.find('.') {
                        let num_str = after[..dot_idx].trim();
                        if let Ok(id) = num_str.parse::<u32>() {
                            return Some(id);
                        }
                    }
                }
            }
        }
        None
    }

    /// Parses kernel `/proc/asound/card*/eld*` files in deterministic sorted path order.
    pub fn get_eld_monitors(force: bool) -> BTreeMap<u32, Vec<EldMonitor>> {
        {
            let guard = cache().lock().unwrap();
            if !force {
                if let Some(t) = guard.last_eld_check {
                    if !guard.cached_eld_map.is_empty() && t.elapsed() < Duration::from_secs(15) {
                        return guard.cached_eld_map.clone();
                    }
                }
            }
        }

        let mut eld_paths: Vec<PathBuf> = Vec::new();
        if let Ok(asound_entries) = fs::read_dir("/proc/asound") {
            for entry in asound_entries.flatten() {
                let p = entry.path();
                if let Some(fname) = p.file_name().and_then(|s| s.to_str()) {
                    if fname.starts_with("card") && p.is_dir() {
                        if let Ok(sub_entries) = fs::read_dir(&p) {
                            for sub in sub_entries.flatten() {
                                let sp = sub.path();
                                if let Some(sf) = sp.file_name().and_then(|s| s.to_str()) {
                                    if sf.starts_with("eld") {
                                        eld_paths.push(sp);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        eld_paths.sort();

        let mut eld_map: BTreeMap<u32, Vec<EldMonitor>> = BTreeMap::new();
        for path in eld_paths {
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            let path_str = path.to_string_lossy();
            if let Some((card_idx, mon)) = Self::parse_eld_content(&path_str, &content) {
                eld_map.entry(card_idx).or_default().push(mon);
            }
        }

        let mut guard = cache().lock().unwrap();
        guard.last_eld_check = Some(Instant::now());
        guard.cached_eld_map = eld_map.clone();
        eld_map
    }

    pub fn parse_eld_content(path: &str, content: &str) -> Option<(u32, EldMonitor)> {
        let mut monitor_present = false;
        let mut name = "Display Audio".to_string();
        let mut connection = "HDMI".to_string();
        let mut channels = 2u32;

        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(val) = trimmed.strip_prefix("monitor_present") {
                if val.trim() == "1" {
                    monitor_present = true;
                }
            } else if let Some(val) = trimmed.strip_prefix("monitor_name") {
                let v = val.trim();
                if !v.is_empty() {
                    name = v.to_string();
                }
            } else if let Some(val) = trimmed.strip_prefix("connection_type") {
                let v = val.trim();
                if !v.is_empty() {
                    connection = v.to_string();
                }
            } else if let Some(val) = trimmed.strip_prefix("sad0_channels") {
                if let Ok(ch) = val.trim().parse::<u32>() {
                    channels = ch;
                }
            }
        }

        if !monitor_present {
            return None;
        }

        let card_idx = path
            .find("card")
            .and_then(|pos| extract_leading_digits(&path[pos + 4..]))?;
        let pin = path
            .rfind('.')
            .and_then(|pos| extract_leading_digits(&path[pos + 1..]))
            .unwrap_or(0);

        Some((
            card_idx,
            EldMonitor {
                name,
                connection,
                channels,
                pin,
            },
        ))
    }

    fn init_card_iec958(card_idx: u32) {
        let card_str = card_idx.to_string();
        for dev_idx in 0..4 {
            let dev_str = dev_idx.to_string();
            run_quiet(
                "iecset",
                &[
                    "-c", &card_str, "-n", &dev_str, "audio", "on", "pro", "off", "orig", "on",
                    "category", "0x02", "rate", "48000",
                ],
                400,
            );
        }
    }

    /// Ensures multi-monitor GPU sound cards use `pro-audio` with valid 2ch stereo converters.
    pub fn ensure_multimonitor_card_profiles() {
        {
            let mut guard = cache().lock().unwrap();
            if let Some(t) = guard.last_profile_check {
                if t.elapsed() < Duration::from_secs(30) {
                    return;
                }
            }
            guard.last_profile_check = Some(Instant::now());
        }

        let eld_map = Self::get_eld_monitors(false);
        if !eld_map.values().any(|mons| mons.len() > 1) {
            return;
        }

        let Some(card_info) = run_cmd("pactl", &["list", "cards"], 1000) else {
            return;
        };
        let sinks_short = run_cmd("pactl", &["list", "sinks", "short"], 1000).unwrap_or_default();

        for block in card_info.split("Card #") {
            let card_name = block
                .lines()
                .find_map(|l| l.trim().strip_prefix("Name:"))
                .map(|s| s.trim().to_string());
            let Some(card_name) = card_name else {
                continue;
            };
            let cn_low = card_name.to_ascii_lowercase();
            if cn_low.contains("sof") || cn_low.contains("realtek") || cn_low.contains("00_1f.3") {
                continue;
            }

            let card_idx = block
                .lines()
                .find_map(|l| {
                    let t = l.trim();
                    t.strip_prefix("alsa.card")
                        .and_then(|rest| rest.trim().strip_prefix('='))
                        .map(|v| v.trim().trim_matches('"').parse::<u32>().ok())
                        .flatten()
                })
                .unwrap_or(0);

            let mons = eld_map.get(&card_idx).cloned().unwrap_or_default();
            let card_prefix = card_name.replacen("alsa_card.", "alsa_output.", 1);
            let two_ch_pro = if sinks_short.is_empty() {
                mons.len()
            } else {
                sinks_short
                    .lines()
                    .filter(|l| l.contains(&card_prefix) && l.contains("pro-output") && l.contains("2ch"))
                    .count()
            };

            let is_pro = block.contains("Active Profile: pro-audio");
            if !is_pro || (!mons.is_empty() && two_ch_pro < mons.len()) {
                if is_pro {
                    run_quiet("pactl", &["set-card-profile", &card_name, "off"], 1000);
                    thread::sleep(Duration::from_millis(80));
                }
                run_quiet("pactl", &["set-card-profile", &card_name, "pro-audio"], 1000);
                thread::sleep(Duration::from_millis(80));
                Self::init_card_iec958(card_idx);

                let mut guard = cache().lock().unwrap();
                guard.initialized_iec958_cards.insert(card_idx);
                guard.cached_sinks.clear();
            } else if !mons.is_empty() {
                let needs_init = {
                    let guard = cache().lock().unwrap();
                    !guard.initialized_iec958_cards.contains(&card_idx)
                };
                if needs_init {
                    Self::init_card_iec958(card_idx);
                    cache().lock().unwrap().initialized_iec958_cards.insert(card_idx);
                }
            }
        }
    }

    /// Scans PipeWire graph for active audio sinks with dynamic hardware and latency telemetry.
    pub fn scan_sinks(force: bool) -> Vec<AudioSink> {
        if !force {
            let guard = cache().lock().unwrap();
            if let Some(t) = guard.last_scan_time {
                if !guard.cached_sinks.is_empty() && t.elapsed() < Duration::from_millis(2500) {
                    return guard.cached_sinks.clone();
                }
            }
        }

        Self::ensure_multimonitor_card_profiles();
        let eld_map = Self::get_eld_monitors(force);

        let Some(raw_dump) = run_cmd("pw-dump", &[], 2000) else {
            return Vec::new();
        };
        let Ok(Value::Array(objects)) = serde_json::from_str::<Value>(&raw_dump) else {
            return Vec::new();
        };

        let sinks = Self::parse_pw_dump_sinks(&objects, &eld_map);

        let mut guard = cache().lock().unwrap();
        guard.cached_sinks = sinks.clone();
        guard.last_scan_time = Some(Instant::now());
        sinks
    }

    /// Pure parser translating `pw-dump` node objects and ELD monitor map into classified `AudioSink`s.
    pub fn parse_pw_dump_sinks(objects: &[Value], eld_map: &BTreeMap<u32, Vec<EldMonitor>>) -> Vec<AudioSink> {
        let mut sinks = Vec::new();
        let mut card_allocated: BTreeMap<Option<u32>, HashSet<usize>> = BTreeMap::new();

        for obj in objects {
            if obj.get("type").and_then(|v| v.as_str()) != Some("PipeWire:Interface:Node") {
                continue;
            }
            let Some(info) = obj.get("info") else {
                continue;
            };
            let Some(props) = info.get("props") else {
                continue;
            };
            if prop_str(props, "media.class") != "Audio/Sink" {
                continue;
            }

            let name = prop_str(props, "node.name").to_string();
            let name_lower = name.to_ascii_lowercase();
            if name_lower.contains("polifonia") || name_lower.contains("filter-chain") {
                continue;
            }

            let node_id = obj.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let card_id = prop_u32(props, "alsa.card");
            let raw_desc = ["node.nick", "node.description", "device.description"]
                .iter()
                .map(|k| prop_str(props, k))
                .find(|s| !s.is_empty())
                .unwrap_or(&name)
                .to_string();
            let raw_desc_lower = raw_desc.to_ascii_lowercase();
            let bus = prop_str(props, "device.bus").to_ascii_lowercase();
            let api_lower = prop_str(props, "device.api").to_ascii_lowercase();
            let form_factor = prop_str(props, "device.form-factor").to_ascii_lowercase();
            let icon_lower = prop_str(props, "device.icon_name").to_ascii_lowercase();

            let is_bluetooth = bus == "bluetooth"
                || name_lower.contains("bluez")
                || api_lower.contains("bluez")
                || icon_lower.contains("bluetooth");
            let is_network = bus == "network"
                || ["raop", "airplay", "rtp", "roc", "snapcast", "cast", "tunnel"]
                    .iter()
                    .any(|k| name_lower.contains(k) || api_lower.contains(k));
            let is_usb = bus == "usb" || name_lower.contains("usb");
            let is_digital_hdmi = !is_usb
                && ["hdmi", "displayport", "pro-output"]
                    .iter()
                    .any(|k| name_lower.contains(k));
            let is_spdif = !is_digital_hdmi
                && (name_lower.contains("iec958")
                    || name_lower.contains("spdif")
                    || raw_desc_lower.contains("optical")
                    || raw_desc_lower.contains("digital"));

            let mut is_internal = false;
            let (description, bus_type): (String, String) = if is_bluetooth {
                let mut clean_name = [
                    "device.alias",
                    "device.description",
                    "node.description",
                    "media.name",
                    "node.nick",
                    "device.product.name",
                ]
                .iter()
                .map(|k| prop_str(props, k).trim())
                .find(|s| !s.is_empty())
                .unwrap_or("Bluetooth Device")
                .to_string();

                for prefix in ["Bluetooth Audio (", "Bluetooth ("] {
                    if clean_name.starts_with(prefix) && clean_name.ends_with(')') {
                        clean_name = clean_name[prefix.len()..clean_name.len() - 1].trim().to_string();
                    }
                }
                (clean_name, "bluetooth".to_string())
            } else if is_network {
                let clean_name = ["node.description", "device.description", "media.name"]
                    .iter()
                    .map(|k| prop_str(props, k).trim())
                    .find(|s| !s.is_empty())
                    .unwrap_or(raw_desc.trim());
                let desc = if clean_name.to_ascii_lowercase().contains("network") {
                    clean_name.to_string()
                } else {
                    format!("Network Audio ({clean_name})")
                };
                (desc, "network".to_string())
            } else if is_digital_hdmi {
                let monitors = card_id.and_then(|c| eld_map.get(&c)).map(|v| v.as_slice()).unwrap_or(&[]);
                let ch_cnt = prop_u32(props, "audio.channels").unwrap_or(2);
                if name_lower.contains("pro-output") && ch_cnt > 2 && monitors.iter().any(|m| m.channels <= 2) {
                    continue;
                }
                let used = card_allocated.entry(card_id).or_default();
                if used.len() >= monitors.len() {
                    continue;
                }

                let alsa_name = prop_str(props, "alsa.name").trim().to_ascii_lowercase();
                let mut matched_idx = None;
                if !alsa_name.is_empty() && !is_generic_display_port_name(&alsa_name) {
                    for (idx, mon_cand) in monitors.iter().enumerate() {
                        if used.contains(&idx) {
                            continue;
                        }
                        let mon_name_l = mon_cand.name.to_ascii_lowercase();
                        if alsa_name == mon_name_l || mon_name_l.contains(&alsa_name) || alsa_name.contains(&mon_name_l) {
                            matched_idx = Some(idx);
                            break;
                        }
                    }
                }
                let idx = matched_idx.unwrap_or_else(|| (0..monitors.len()).find(|i| !used.contains(i)).unwrap());
                used.insert(idx);
                let mon = &monitors[idx];
                (format!("Monitor {} ({})", mon.name, mon.connection), "hdmi".to_string())
            } else if is_usb {
                let raw_clean = raw_desc
                    .replace("Analog Stereo", "")
                    .replace("Stereo analogico", "")
                    .trim()
                    .to_string();
                let clean_name = ["node.nick", "device.product.name"]
                    .iter()
                    .map(|k| prop_str(props, k).trim())
                    .find(|s| !s.is_empty())
                    .unwrap_or(&raw_clean);
                (format!("USB Audio ({clean_name})"), "usb".to_string())
            } else if is_spdif {
                let raw_clean = raw_desc.replace("Digital Stereo (IEC958)", "").trim().to_string();
                let clean_name = ["node.nick", "device.product.name"]
                    .iter()
                    .map(|k| prop_str(props, k).trim())
                    .find(|s| !s.is_empty())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(if raw_clean.is_empty() { "S/PDIF" } else { &raw_clean });
                (format!("Digital S/PDIF ({clean_name})"), "spdif".to_string())
            } else if raw_desc_lower.contains("headphone") || name_lower.contains("headphone") {
                let clean_name = ["node.nick", "device.product.name"]
                    .iter()
                    .map(|k| prop_str(props, k).trim())
                    .find(|s| !s.is_empty())
                    .unwrap_or("Analog");
                (format!("Headphones / Audio Jack ({clean_name})"), "jack".to_string())
            } else if raw_desc_lower.contains("speaker")
                || name_lower.contains("speaker")
                || form_factor == "internal"
            {
                is_internal = true;
                ("Integrated Speakers (Internal)".to_string(), "internal".to_string())
            } else {
                (
                    raw_desc,
                    if bus.is_empty() { "pci".to_string() } else { bus },
                )
            };

            let mut latency_ms = 0.0f64;
            if let Some(params) = info.get("params") {
                if let Some(lats) = params.get("Latency").and_then(|v| v.as_array()) {
                    for lat in lats {
                        let min_ns = lat.get("minNs").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        let max_ns = lat.get("maxNs").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        if min_ns > 0.0 {
                            let est_ns = if max_ns > 0.0
                                && min_ns <= max_ns
                                && max_ns <= 2_000_000_000.0
                                && (max_ns - min_ns) <= 200_000_000.0
                            {
                                0.5 * (min_ns + max_ns)
                            } else {
                                min_ns
                            };
                            latency_ms = latency_ms.max(est_ns / 1_000_000.0);
                        }
                    }
                }
                if latency_ms <= 0.0 {
                    if let Some(proc_lats) = params.get("ProcessLatency").and_then(|v| v.as_array()) {
                        for proc_lat in proc_lats {
                            let ns = proc_lat.get("ns").and_then(|v| v.as_f64()).unwrap_or(0.0);
                            if ns > 0.0 {
                                latency_ms = latency_ms.max(ns / 1_000_000.0);
                            }
                        }
                    }
                }
            }

            if latency_ms <= 0.0 {
                if bus_type == "bluetooth" {
                    let codec = prop_str(props, "api.bluez5.codec").to_ascii_lowercase();
                    latency_ms = match codec.as_str() {
                        "aptx_ll" | "faststream" | "lc3" => 65.0,
                        "aptx" | "aptx_hd" => 130.0,
                        "ldac" => 170.0,
                        _ => 180.0,
                    };
                } else {
                    latency_ms = match bus_type.as_str() {
                        "network" => 250.0,
                        "hdmi" | "pci" | "spdif" => 10.0,
                        "usb" => 12.0,
                        "jack" => 6.0,
                        "internal" => 5.0,
                        _ => 10.0,
                    };
                }
            }

            let channel_count = prop_u32(props, "audio.channels").unwrap_or(2);
            sinks.push(AudioSink {
                id: node_id,
                name,
                description,
                media_class: "Audio/Sink".to_string(),
                channel_count,
                is_internal,
                latency_ms: (latency_ms * 10.0).round() / 10.0,
                bus_type,
                ..Default::default()
            });
        }

        // Disambiguate identical display descriptions cleanly.
        let mut counts: HashMap<String, usize> = HashMap::new();
        for s in &sinks {
            *counts.entry(s.description.clone()).or_insert(0) += 1;
        }
        if counts.values().any(|&c| c > 1) {
            let mut seen: HashMap<String, usize> = HashMap::new();
            for s in &mut sinks {
                if counts.get(&s.description).copied().unwrap_or(0) > 1 {
                    let idx = *seen.get(&s.description).unwrap_or(&1);
                    seen.insert(s.description.clone(), idx + 1);
                    s.description = format!("{} ({idx})", s.description);
                }
            }
        }

        sinks
    }

    /// Maps both sink ID and sink name to PipeWire playback state (`RUNNING`, `IDLE`, `SUSPENDED`).
    pub fn get_sink_states() -> HashMap<String, String> {
        let Some(stdout) = run_cmd("pactl", &["list", "sinks", "short"], 800) else {
            return HashMap::new();
        };
        Self::parse_sink_states(&stdout)
    }

    pub fn parse_sink_states(stdout: &str) -> HashMap<String, String> {
        let mut states = HashMap::new();
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 5 {
                let state = parts[parts.len() - 1].to_ascii_uppercase();
                states.insert(parts[0].to_string(), state.clone());
                states.insert(parts[1].to_string(), state);
            }
        }
        states
    }

    /// Parses active external client application playback streams (`pactl list sink-inputs`).
    pub fn get_active_sink_inputs() -> Vec<SinkInputInfo> {
        let Some(stdout) = run_cmd("pactl", &["list", "sink-inputs"], 800) else {
            return Vec::new();
        };
        Self::parse_active_sink_inputs(&stdout)
    }

    pub fn parse_active_sink_inputs(stdout: &str) -> Vec<SinkInputInfo> {
        let mut inputs = Vec::new();
        for block in stdout.split("Sink Input #") {
            let trimmed = block.trim();
            if trimmed.is_empty() {
                continue;
            }
            let input_id = trimmed.lines().next().unwrap_or("").trim().to_string();
            let b_lower = block.to_ascii_lowercase();

            let prop = |key: &str| -> String {
                for line in block.lines() {
                    let lt = line.trim();
                    if let Some(rest) = lt.strip_prefix(key) {
                        if let Some(after_eq) = rest.trim_start().strip_prefix('=') {
                            let val = after_eq.trim();
                            if val.starts_with('"') && val.ends_with('"') && val.len() >= 2 {
                                return val[1..val.len() - 1].to_string();
                            }
                            return val.trim_matches('"').to_string();
                        }
                    }
                }
                String::new()
            };

            let app_name = {
                let a = prop("application.name");
                let b = prop("application.process.binary");
                let n = prop("node.name");
                if !a.is_empty() {
                    a
                } else if !b.is_empty() {
                    b
                } else if !n.is_empty() {
                    n
                } else {
                    "Audio Stream".to_string()
                }
            };
            let media_name = prop("media.name");
            let node_name = prop("node.name");

            if app_name == "pw-loopback"
                || node_name.starts_with("polifonia")
                || media_name.starts_with("polifonia")
                || b_lower.contains("stream.capture.sink")
                || (b_lower.contains("loopback") && b_lower.contains("output"))
            {
                continue;
            }

            let mut sink_id = None;
            let mut is_corked = false;
            let mut is_muted = false;
            let mut volume_pct = 100u32;

            for line in block.lines() {
                let lt = line.trim();
                if let Some(val) = lt.strip_prefix("Sink:") {
                    sink_id = val.trim().parse::<u32>().ok();
                } else if let Some(val) = lt.strip_prefix("Corked:") {
                    is_corked = val.trim().eq_ignore_ascii_case("yes");
                } else if let Some(val) = lt.strip_prefix("Mute:") {
                    is_muted = val.trim().eq_ignore_ascii_case("yes");
                } else if lt.starts_with("Volume:") {
                    for seg in lt.split('/') {
                        let st = seg.trim();
                        if let Some(pct_str) = st.strip_suffix('%') {
                            if let Ok(v) = pct_str.trim().parse::<u32>() {
                                volume_pct = v;
                                break;
                            }
                        }
                    }
                }
            }

            let icon_name = {
                let i = prop("application.icon_name");
                if i.is_empty() {
                    "audio-x-generic".to_string()
                } else {
                    i
                }
            };

            inputs.push(SinkInputInfo {
                id: input_id,
                sink_id,
                app_name,
                icon_name,
                media_name,
                volume_pct,
                is_active: !is_corked && !is_muted,
                corked: is_corked,
                muted: is_muted,
            });
        }
        inputs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_default_sink_id_parsing() {
        let sample = "
PipeWire 'pipewire-0' [1.6.8]
 └─ Sinks:
      40. Built-in Audio Analog Stereo        [vol: 0.60]
    * 55. LG UltraWide HDMI Audio             [vol: 1.00]
";
        assert_eq!(DeviceScanner::parse_default_sink_id(sample), Some(55));
    }

    #[test]
    fn test_scan_sinks_matches_eld_by_alsa_name_and_skips_8ch_unplugged() {
        let objects = vec![
            json!({
                "id": 105,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "props": {
                        "media.class": "Audio/Sink",
                        "node.name": "alsa_output.pci-0000_01_00.1.pro-output-3",
                        "alsa.card": "0",
                        "alsa.name": "DELL SE2426HG",
                        "audio.channels": "2",
                        "device.bus": "pci"
                    }
                }
            }),
            json!({
                "id": 61,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "props": {
                        "media.class": "Audio/Sink",
                        "node.name": "alsa_output.pci-0000_01_00.1.pro-output-8",
                        "alsa.card": 0,
                        "alsa.name": "XG27AQDMES",
                        "audio.channels": 2,
                        "device.bus": "pci"
                    }
                }
            }),
            json!({
                "id": 62,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "props": {
                        "media.class": "Audio/Sink",
                        "node.name": "alsa_output.pci-0000_01_00.1.pro-output-9",
                        "alsa.card": 0,
                        "alsa.name": "HDMI 3",
                        "audio.channels": 8,
                        "device.bus": "pci"
                    }
                }
            }),
        ];

        let mut eld = BTreeMap::new();
        eld.insert(
            0,
            vec![
                EldMonitor {
                    name: "XG27AQDMES".to_string(),
                    connection: "HDMI".to_string(),
                    channels: 2,
                    pin: 0,
                },
                EldMonitor {
                    name: "DELL SE2426HG".to_string(),
                    connection: "DisplayPort".to_string(),
                    channels: 2,
                    pin: 6,
                },
            ],
        );

        let sinks = DeviceScanner::parse_pw_dump_sinks(&objects, &eld);
        assert_eq!(sinks.len(), 2);
        assert_eq!(sinks[0].description, "Monitor DELL SE2426HG (DisplayPort)");
        assert_eq!(sinks[1].description, "Monitor XG27AQDMES (HDMI)");
    }

    #[test]
    fn test_bt_codec_latency_and_internal_detection() {
        let objects = vec![
            json!({
                "id": 301,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "props": {
                        "media.class": "Audio/Sink",
                        "node.name": "bluez_output.AA_BB_CC.1",
                        "node.description": "Low LatencyBT",
                        "device.bus": "bluetooth",
                        "api.bluez5.codec": "aptx_ll"
                    }
                }
            }),
            json!({
                "id": 302,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "props": {
                        "media.class": "Audio/Sink",
                        "node.name": "alsa_output.pci-0000_00_1f.3.HiFi__Speaker__sink",
                        "node.description": "Speaker",
                        "device.form-factor": "internal"
                    }
                }
            }),
        ];
        let sinks = DeviceScanner::parse_pw_dump_sinks(&objects, &BTreeMap::new());
        assert_eq!(sinks[0].latency_ms, 65.0);
        assert_eq!(sinks[1].bus_type, "internal");
        assert!(sinks[1].is_internal);
    }

    #[test]
    fn test_parse_active_sink_inputs_filters_polifonia() {
        let raw = r#"Sink Input #501
	Sink: 10279
	Corked: no
	Mute: no
	Volume: front-left: 65536 / 100% / 0,00 dB
	Properties:
		application.name = "Google Chrome"
		application.icon_name = "google-chrome"
		media.name = "Playback"

Sink Input #601
	Sink: 58
	Corked: no
	Mute: no
	Properties:
		node.name = "polifonia_play_58"
"#;
        let inputs = DeviceScanner::parse_active_sink_inputs(raw);
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].app_name, "Google Chrome");
        assert!(inputs[0].is_active);
    }
}
