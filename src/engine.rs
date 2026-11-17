//! Audio Engine Service coordinating PipeWire virtual master routing, loopback supervisors,
//! real-time volume coalescing, watchdog self-healing, and Hilbert FFT acoustic calibration.

use crate::models::{AudioSink, SpeakerConfig, SpeakerRole, SystemConfig};
use crate::scanner::{run_cmd, run_quiet, DeviceScanner, SinkInputInfo};
use crate::storage::StorageService;
use rustfft::{num_complex::Complex, FftPlanner};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::f32::consts::PI as PI32;
use std::f64::consts::PI as PI64;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestinationStatus {
    pub sink_id: u32,
    pub sink_name: String,
    pub display_name: String,
    pub bus_type: String,
    pub role: SpeakerRole,
    pub volume_gain: f64,
    pub delay_ms: f64,
    pub phase_inverted: bool,
    pub hardware_latency_ms: f64,
    pub auto_comp_delay_ms: f64,
    pub total_delay_ms: f64,
    pub is_enabled: bool,
    pub is_streaming: bool,
    pub state: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoutingStatus {
    pub is_unison_active: bool,
    pub master_volume: f64,
    pub master_state: String,
    pub sources: Vec<SinkInputInfo>,
    pub destinations: Vec<DestinationStatus>,
    pub max_hw_latency: f64,
}

fn is_wireless_name(sink_name: &str) -> bool {
    sink_name.to_ascii_lowercase().contains("bluez")
}

fn is_wireless_speaker(spk: &SpeakerConfig) -> bool {
    let bus = spk.bus_type.to_ascii_lowercase();
    bus == "bluetooth" || bus == "network" || is_wireless_name(&spk.sink_name)
}

fn temp_file_path(prefix: &str, ext: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    env::temp_dir().join(format!("{prefix}_{}_{nanos}.{ext}", std::process::id()))
}

/// Writes a 16-bit PCM stereo WAV file without external encoder dependencies.
fn write_stereo_wav(path: &std::path::Path, sample_rate: u32, mono_pcm: &[i16]) -> std::io::Result<()> {
    let data_bytes = (mono_pcm.len() * 4) as u32;
    let byte_rate = sample_rate * 4;
    let mut buf = Vec::with_capacity(44 + data_bytes as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&2u16.to_le_bytes()); // Stereo
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&4u16.to_le_bytes()); // Block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // Bits per sample
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_bytes.to_le_bytes());
    for &sample in mono_pcm {
        let b = sample.to_le_bytes();
        buf.extend_from_slice(&b);
        buf.extend_from_slice(&b);
    }
    let mut f = File::create(path)?;
    f.write_all(&buf)?;
    Ok(())
}

/// Synthesizes a linear frequency-modulated chirp: phi(t) = 2*pi*(f0*t + (df/(2*T))*t^2) * Hann(t).
pub fn synthesize_linear_chirp(f0: f32, f1: f32, duration_s: f32, sample_rate: u32) -> Vec<f32> {
    let n_samples = ((sample_rate as f32) * duration_s).round().max(1.0) as usize;
    let df = f1 - f0;
    let denom = ((n_samples.saturating_sub(1)) as f32).max(1.0);
    (0..n_samples)
        .map(|i| {
            let t = duration_s * (i as f32) / denom;
            let phase = 2.0 * PI32 * (f0 * t + 0.5 * (df / duration_s) * t * t);
            let hann = 0.5 * (1.0 - (2.0 * PI32 * (i as f32) / denom).cos());
            phase.sin() * hann
        })
        .collect()
}

/// Computes phase-invariant Hilbert analytic matched-filter cross-correlation envelope in O(N log N).
pub fn compute_robust_correlation(rec: &[f32], template: &[f32]) -> Vec<f32> {
    let n_rec = rec.len();
    let n_tpl = template.len();
    if n_rec < n_tpl || n_tpl == 0 {
        return vec![0.0];
    }

    let valid_len = n_rec - n_tpl + 1;
    let n_fft = (n_rec + n_tpl - 1).next_power_of_two().max(2);

    let mean_rec = rec.iter().map(|&v| v as f64).sum::<f64>() / (n_rec as f64);
    let mean_tpl = template.iter().map(|&v| v as f64).sum::<f64>() / (n_tpl as f64);

    let mut rec_buf = vec![Complex::new(0.0f32, 0.0f32); n_fft];
    for (dst, &src) in rec_buf.iter_mut().zip(rec.iter()) {
        dst.re = src - mean_rec as f32;
    }

    let mut tpl_buf = vec![Complex::new(0.0f32, 0.0f32); n_fft];
    for (dst, &src) in tpl_buf.iter_mut().zip(template.iter()) {
        dst.re = src - mean_tpl as f32;
    }

    let mut planner = FftPlanner::<f32>::new();
    let fft_fwd = planner.plan_fft_forward(n_fft);
    let fft_inv = planner.plan_fft_inverse(n_fft);

    fft_fwd.process(&mut rec_buf);
    fft_fwd.process(&mut tpl_buf);

    let half = n_fft / 2;
    for k in 0..n_fft {
        let hilbert_w = if k == 0 || (n_fft % 2 == 0 && k == half) {
            1.0f32
        } else if k < half || (n_fft % 2 == 1 && k <= half) {
            2.0f32
        } else {
            0.0f32
        };
        if hilbert_w == 0.0 {
            rec_buf[k] = Complex::new(0.0, 0.0);
        } else {
            rec_buf[k] = (rec_buf[k] * tpl_buf[k].conj()) * hilbert_w;
        }
    }

    fft_inv.process(&mut rec_buf);
    let inv_norm = 1.0f32 / (n_fft as f32);
    rec_buf[..valid_len]
        .iter()
        .map(|c| c.norm() * inv_norm)
        .collect()
}

/// Detects the earliest direct-path arrival peak within `[win_start, win_end)` and refines to sub-sample accuracy.
pub fn detect_first_arrival_peak(
    corr: &[f32],
    win_start: usize,
    win_end: usize,
    sr: usize,
    noise_floor: f32,
) -> (f64, f32) {
    let ws = win_start.min(corr.len());
    let we = win_end.min(corr.len());
    if we <= ws {
        return (ws as f64, 0.0);
    }

    let (mut max_offset, mut max_val) = (0usize, f32::NEG_INFINITY);
    for (idx, &val) in corr[ws..we].iter().enumerate() {
        if val > max_val {
            max_val = val;
            max_offset = idx;
        }
    }
    let max_idx = ws + max_offset;
    let mut peak_idx = max_idx;

    let pre_start = ws.max(max_idx.saturating_sub((sr * 15) / 1000));
    let pre_end = ws.max(max_idx.saturating_sub((sr * 2) / 1000));
    if pre_end > pre_start + 2 && max_val > noise_floor * 3.0 {
        let (mut cand_off, mut cand_val) = (0usize, f32::NEG_INFINITY);
        for (idx, &val) in corr[pre_start..pre_end].iter().enumerate() {
            if val > cand_val {
                cand_val = val;
                cand_off = idx;
            }
        }
        let cand_idx = pre_start + cand_off;
        if cand_val >= (noise_floor * 3.5).max(max_val * 0.65) {
            let valley = corr[cand_idx..max_idx]
                .iter()
                .copied()
                .fold(f32::INFINITY, f32::min);
            if valley <= 0.55 * cand_val {
                peak_idx = cand_idx;
            }
        }
    }

    let peak_val = corr[peak_idx];
    let mut exact_idx = peak_idx as f64;
    if peak_idx > 0 && peak_idx + 1 < corr.len() {
        let y0 = corr[peak_idx - 1] as f64;
        let y1 = corr[peak_idx] as f64;
        let y2 = corr[peak_idx + 1] as f64;
        let denom = y0 - 2.0 * y1 + y2;
        if denom.abs() > 1e-6 {
            let delta = 0.5 * (y0 - y2) / denom;
            if (-0.5..=0.5).contains(&delta) {
                exact_idx += delta;
            }
        }
    }
    (exact_idx, peak_val)
}

#[derive(Default)]
struct EngineInner {
    config: SystemConfig,
    sync_processes: HashMap<String, Child>,
    running_sink_ids: HashMap<String, u32>,
    running_delays: HashMap<String, f64>,
    running_user_delays: HashMap<String, f64>,
    running_roles: HashMap<String, SpeakerRole>,
    running_phases: HashMap<String, bool>,
    running_muted: HashMap<String, bool>,
    running_user_min_delay: f64,
    is_active: bool,
    branch_seq: u64,
    master_module_id: Option<String>,
    cached_master_node_id: Option<u32>,
    prev_default_sink_id: Option<u32>,
    prev_default_sink_name: Option<String>,
}

impl EngineInner {
    fn pop_branch_state(&mut self, sink_name: &str) -> Option<Child> {
        self.running_sink_ids.remove(sink_name);
        self.running_delays.remove(sink_name);
        self.running_user_delays.remove(sink_name);
        self.running_roles.remove(sink_name);
        self.running_phases.remove(sink_name);
        self.running_muted.remove(sink_name);
        self.sync_processes.remove(sink_name)
    }

    fn clear_all_branch_states(&mut self) -> Vec<Child> {
        self.running_sink_ids.clear();
        self.running_delays.clear();
        self.running_user_delays.clear();
        self.running_roles.clear();
        self.running_phases.clear();
        self.running_muted.clear();
        self.running_user_min_delay = 0.0;
        self.sync_processes.drain().map(|(_, p)| p).collect()
    }
}

#[derive(Default)]
struct GainQueue {
    pending_gains: HashMap<String, f64>,
    pending_master_gain: Option<f64>,
    stop: bool,
}

pub struct AudioEngineService {
    pub storage: StorageService,
    inner: Mutex<EngineInner>,
    master_lock: Mutex<()>,
    gain_queue: Arc<(Mutex<GainQueue>, Condvar)>,
    watchdog_running: Arc<AtomicBool>,
    watchdog_stop: Arc<AtomicBool>,
}

impl AudioEngineService {
    pub fn new(storage: StorageService) -> Arc<Self> {
        let cfg = storage.load();
        let gain_queue = Arc::new((Mutex::new(GainQueue::default()), Condvar::new()));

        let svc = Arc::new(Self {
            storage,
            inner: Mutex::new(EngineInner {
                config: cfg,
                ..Default::default()
            }),
            master_lock: Mutex::new(()),
            gain_queue: Arc::clone(&gain_queue),
            watchdog_running: Arc::new(AtomicBool::new(false)),
            watchdog_stop: Arc::new(AtomicBool::new(false)),
        });

        Self::spawn_gain_worker(gain_queue);
        svc.cleanup_all_orphan_nodes(false);
        svc.heal_dangling_default_sink();
        svc
    }

    fn spawn_gain_worker(gq: Arc<(Mutex<GainQueue>, Condvar)>) {
        thread::spawn(move || {
            let (lock, cvar) = &*gq;
            loop {
                let (gains_batch, master_val, should_stop) = {
                    let mut guard = lock.lock().unwrap();
                    while guard.pending_gains.is_empty() && guard.pending_master_gain.is_none() && !guard.stop {
                        guard = cvar.wait(guard).unwrap();
                    }
                    let batch = std::mem::take(&mut guard.pending_gains);
                    let m = guard.pending_master_gain.take();
                    let stop = guard.stop;
                    (batch, m, stop)
                };

                for (s_name, g) in gains_batch {
                    if g <= 0.001 {
                        run_quiet("pactl", &["set-sink-mute", &s_name, "1"], 1000);
                    } else {
                        let vol_pct = format!("{}%", (g * 100.0) as u32);
                        run_quiet("pactl", &["set-sink-mute", &s_name, "0"], 1000);
                        run_quiet("pactl", &["set-sink-volume", &s_name, &vol_pct], 1000);
                    }
                }
                if let Some(m) = master_val {
                    let vol_pct = format!("{}%", (m * 100.0) as u32);
                    run_quiet("pactl", &["set-sink-volume", "polifonia_master", &vol_pct], 1000);
                }
                if should_stop {
                    break;
                }
            }
        });
    }

    fn async_kill(mut proc: Child) {
        let _ = proc.kill();
        let _ = proc.wait();
    }

    pub fn config_snapshot(&self) -> SystemConfig {
        self.inner.lock().unwrap().config.clone()
    }

    pub fn update_config(&self, mut config: SystemConfig, persist: bool) {
        if persist {
            let _ = self.storage.save(&mut config, None);
        }
        self.inner.lock().unwrap().config = config;
    }

    pub fn is_running(&self) -> bool {
        self.inner.lock().unwrap().is_active
    }

    pub fn get_available_sinks(&self, force: bool) -> Vec<AudioSink> {
        DeviceScanner::scan_sinks(force)
    }

    pub fn cleanup_all_orphan_nodes(&self, unload_master: bool) {
        let procs = self.inner.lock().unwrap().clear_all_branch_states();
        for p in procs {
            Self::async_kill(p);
        }
        run_quiet("pkill", &["-f", "pw-loopback.*polifonia"], 500);

        if unload_master {
            if let Some(out) = run_cmd("pactl", &["list", "modules", "short"], 2000) {
                for line in out.lines() {
                    if line.contains("polifonia") || (line.contains("null-sink") && line.contains("polifonia_master")) {
                        if let Some(mod_id) = line.split_whitespace().next() {
                            run_quiet("pactl", &["unload-module", mod_id], 4000);
                        }
                    }
                }
            }
            let mut g = self.inner.lock().unwrap();
            g.master_module_id = None;
            g.cached_master_node_id = None;
        }
    }

    fn find_polifonia_master_node_id(&self) -> Option<u32> {
        if let Some(id) = self.inner.lock().unwrap().cached_master_node_id {
            return Some(id);
        }
        let raw = run_cmd("pw-dump", &["Node"], 1000)?;
        let serde_json::Value::Array(nodes) = serde_json::from_str(&raw).ok()? else {
            return None;
        };
        for node in nodes {
            let name = node
                .get("info")
                .and_then(|i| i.get("props"))
                .and_then(|p| p.get("node.name"))
                .and_then(|v| v.as_str());
            if name == Some("polifonia_master") {
                if let Some(id) = node.get("id").and_then(|v| v.as_u64()) {
                    let nid = id as u32;
                    self.inner.lock().unwrap().cached_master_node_id = Some(nid);
                    return Some(nid);
                }
            }
        }
        None
    }

    fn is_master_sink_alive(&self) -> bool {
        let cached = self.inner.lock().unwrap().cached_master_node_id;
        if let Some(nid) = cached {
            let nid_str = nid.to_string();
            if let Some(out) = run_cmd("pw-cli", &["info", &nid_str], 400) {
                if out.contains("polifonia_master") {
                    return true;
                }
            }
            self.inner.lock().unwrap().cached_master_node_id = None;
        }
        if self.find_polifonia_master_node_id().is_some() {
            return true;
        }
        run_cmd("pactl", &["list", "sinks", "short"], 1500)
            .map(|s| s.contains("polifonia_master"))
            .unwrap_or(false)
    }

    fn route_active_streams_to_master(&self) {
        let live_sinks = DeviceScanner::scan_sinks(false);
        let master_id = live_sinks
            .iter()
            .find(|s| s.name == "polifonia_master")
            .map(|s| s.id);
        for inp in DeviceScanner::get_active_sink_inputs() {
            if master_id.is_none() || inp.sink_id != master_id {
                run_quiet("pactl", &["move-sink-input", &inp.id, "polifonia_master"], 1000);
            }
        }
    }

    fn route_active_streams_to_sink(&self, target_sink: &str) {
        if target_sink.is_empty() {
            return;
        }
        for inp in DeviceScanner::get_active_sink_inputs() {
            run_quiet("pactl", &["move-sink-input", &inp.id, target_sink], 1000);
        }
    }

    fn heal_dangling_default_sink(&self) {
        if let Some(def_name) = run_cmd("pactl", &["get-default-sink"], 1000) {
            if def_name.trim().to_ascii_lowercase().contains("polifonia") {
                let sinks = DeviceScanner::scan_sinks(false);
                if let Some(real) = sinks
                    .iter()
                    .find(|s| !s.is_internal && !s.name.to_ascii_lowercase().contains("polifonia"))
                    .or_else(|| sinks.first())
                {
                    run_quiet("pactl", &["set-default-sink", &real.name], 1000);
                    let id_str = real.id.to_string();
                    run_quiet("wpctl", &["set-default", &id_str], 1000);
                }
            }
        }
    }

    fn ensure_master_sink(&self) -> bool {
        let _lock = self.master_lock.lock().unwrap();
        let set_as_default = self.inner.lock().unwrap().config.set_as_default;

        if self.is_master_sink_alive() {
            if set_as_default {
                if let Some(wp_id) = self.find_polifonia_master_node_id() {
                    let s = wp_id.to_string();
                    run_quiet("wpctl", &["set-default", &s], 300);
                }
                run_quiet("pactl", &["set-default-sink", "polifonia_master"], 300);
                run_quiet("pactl", &["set-sink-mute", "polifonia_master", "0"], 300);
                self.route_active_streams_to_master();
            }
            return true;
        }

        let need_prev = {
            let g = self.inner.lock().unwrap();
            g.prev_default_sink_id.is_none()
                || g.prev_default_sink_name
                    .as_deref()
                    .map(|n| n.is_empty() || n.to_ascii_lowercase().contains("polifonia"))
                    .unwrap_or(true)
        };
        if need_prev {
            let cur_id = DeviceScanner::get_default_sink_id();
            let cur_name = run_cmd("pactl", &["get-default-sink"], 1000)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            if !cur_name.is_empty() && !cur_name.to_ascii_lowercase().contains("polifonia") {
                let mut g = self.inner.lock().unwrap();
                g.prev_default_sink_id = cur_id;
                g.prev_default_sink_name = Some(cur_name);
            } else {
                let sinks = DeviceScanner::scan_sinks(false);
                if let Some(real) = sinks
                    .iter()
                    .find(|s| !s.is_internal && !s.name.to_ascii_lowercase().contains("polifonia"))
                    .or_else(|| sinks.first())
                {
                    let mut g = self.inner.lock().unwrap();
                    g.prev_default_sink_id = Some(real.id);
                    g.prev_default_sink_name = Some(real.name.clone());
                }
            }
        }

        let mut loaded_id: Option<String> = None;
        if let Some(out) = run_cmd("pactl", &["list", "modules", "short"], 1500) {
            for line in out.lines() {
                if line.contains("polifonia_master") && line.contains("null-sink") {
                    loaded_id = line.split_whitespace().next().map(|s| s.to_string());
                    break;
                }
            }
        }

        if loaded_id.is_none() {
            if let Some(out) = run_cmd(
                "pactl",
                &[
                    "load-module",
                    "module-null-sink",
                    "sink_name=polifonia_master",
                    "rate=48000",
                    "channels=2",
                    "channel_map=front-left,front-right",
                    "sink_properties=device.description=Polifonia media.class=Audio/Sink node.always-process=true priority.driver=30000 priority.session=30000",
                ],
                6000,
            ) {
                loaded_id = Some(out.trim().to_string());
                thread::sleep(Duration::from_millis(50));
            } else if self.is_master_sink_alive() {
                return true;
            } else {
                return false;
            }
        }

        let master_vol = {
            let mut g = self.inner.lock().unwrap();
            g.master_module_id = loaded_id;
            g.cached_master_node_id = None;
            g.config.master_volume
        };

        let wp_id = self.find_polifonia_master_node_id();
        let vol_pct = format!("{}%", (master_vol * 100.0) as u32);
        run_quiet("pactl", &["set-sink-volume", "polifonia_master", &vol_pct], 500);
        run_quiet("pactl", &["set-sink-mute", "polifonia_master", "0"], 500);

        if set_as_default {
            run_quiet("pactl", &["set-default-sink", "polifonia_master"], 500);
            if let Some(id) = wp_id {
                let s = id.to_string();
                run_quiet("wpctl", &["set-default", &s], 500);
            }
            self.route_active_streams_to_master();
        }
        true
    }

    pub fn reassert_master_default(&self) {
        let (is_active, set_as_default) = {
            let g = self.inner.lock().unwrap();
            (g.is_active, g.config.set_as_default)
        };
        if !is_active || !set_as_default {
            return;
        }
        run_quiet("pactl", &["set-default-sink", "polifonia_master"], 500);
        if let Some(id) = self.find_polifonia_master_node_id() {
            let s = id.to_string();
            run_quiet("wpctl", &["set-default", &s], 500);
        }
        self.route_active_streams_to_master();
    }

    pub fn set_master_gain(&self, gain: f64, persist: bool) {
        let clamped = gain.clamp(0.0, 1.5);
        let is_active = {
            let mut g = self.inner.lock().unwrap();
            g.config.master_volume = clamped;
            if persist {
                let _ = self.storage.save(&mut g.config, None);
            }
            g.is_active
        };
        if is_active {
            let (lock, cvar) = &*self.gain_queue;
            lock.lock().unwrap().pending_master_gain = Some(clamped);
            cvar.notify_one();
        }
    }

    pub fn set_speaker_gain(&self, sink_name: &str, gain: f64, persist: bool) {
        let clamped = gain.clamp(0.0, 1.5);
        let is_active = {
            let mut g = self.inner.lock().unwrap();
            if let Some(spk) = g.config.speakers.get_mut(sink_name) {
                spk.volume_gain = clamped;
            }
            if persist {
                let _ = self.storage.save(&mut g.config, None);
            }
            g.is_active
        };
        if is_active {
            let (lock, cvar) = &*self.gain_queue;
            lock.lock().unwrap().pending_gains.insert(sink_name.to_string(), clamped);
            cvar.notify_one();
        }
    }

    pub fn set_speaker_mute(&self, sink_name: &str, mute: bool) {
        let (pid_opt, gain_val, is_bt) = {
            let mut g = self.inner.lock().unwrap();
            let mut gain = 1.0;
            let mut bt = is_wireless_name(sink_name);
            if let Some(spk) = g.config.speakers.get_mut(sink_name) {
                spk.mute = mute;
                gain = spk.volume_gain;
                bt = bt || is_wireless_speaker(spk);
            }
            g.running_muted.insert(sink_name.to_string(), mute);
            let pid = g.sync_processes.get(sink_name).map(|p| p.id());
            (pid, gain, bt)
        };

        let s_name = sink_name.to_string();
        thread::spawn(move || {
            if let Some(pid) = pid_opt {
                let pid_str = pid.to_string();
                run_quiet("wpctl", &["set-mute", "-p", &pid_str, if mute { "1" } else { "0" }], 150);
                run_quiet("wpctl", &["set-volume", "-p", &pid_str, if mute { "0.0" } else { "1.0" }], 150);
            }
            if !mute {
                let vol_pct = format!("{}%", (gain_val * 100.0) as u32);
                run_quiet("pactl", &["set-sink-mute", &s_name, "0"], 200);
                run_quiet("pactl", &["set-sink-volume", &s_name, &vol_pct], 200);
                if let Some(pid) = pid_opt {
                    let pid_str = pid.to_string();
                    run_quiet("wpctl", &["set-mute", "-p", &pid_str, "0"], 150);
                }
                if is_bt {
                    thread::sleep(Duration::from_millis(80));
                    run_quiet("pactl", &["set-sink-mute", &s_name, "0"], 300);
                    run_quiet("pactl", &["set-sink-volume", &s_name, &vol_pct], 300);
                }
            }
        });
    }

    pub fn play_test_tone(&self, target: &str, freq: f64, duration_s: f64) {
        let target_owned = target.to_string();
        thread::spawn(move || {
            let sr = 44100u32;
            let n_samples = ((sr as f64) * duration_s).round().max(1.0) as usize;
            let fade = (n_samples / 4).clamp(1, 441);
            let mut pcm = Vec::with_capacity(n_samples);
            for i in 0..n_samples {
                let env = if i < fade {
                    i as f64 / fade as f64
                } else if i > n_samples - fade {
                    (n_samples - i) as f64 / fade as f64
                } else {
                    1.0
                };
                let s = (32767.0 * 0.12 * env * (2.0 * PI64 * freq * (i as f64) / (sr as f64)).sin()) as i16;
                pcm.push(s);
            }
            let wav = temp_file_path("polifonia_tone", "wav");
            if write_stereo_wav(&wav, sr, &pcm).is_ok() {
                let target_arg = format!("--target={target_owned}");
                let wav_str = wav.to_string_lossy().to_string();
                let timeout = ((duration_s + 2.0) * 1000.0) as u64;
                run_quiet("pw-play", &[&target_arg, "--volume=0.22", &wav_str], timeout);
            }
            let _ = fs::remove_file(&wav);
        });
    }

    fn spawn_loopback_locked(
        g: &mut EngineInner,
        spk: &SpeakerConfig,
        total_delay_ms: f64,
        is_bt: bool,
        start_muted: bool,
    ) {
        g.branch_seq += 1;
        let sink_name = &spk.sink_name;
        let seq_tag = format!("{}_{}", spk.sink_id, g.branch_seq);
        let target = if sink_name.is_empty() {
            spk.sink_id.to_string()
        } else {
            sink_name.clone()
        };

        let mut cap_props = format!(
            "target.object=polifonia_master stream.capture.sink=true node.name=polifonia_cap_{seq_tag} node.dont-reconnect=true state.restore-props=false audio.rate=48000 resample.quality=10 channelmix.normalize=true"
        );
        let mut play_props = format!(
            "target.object={target} node.name=polifonia_play_{seq_tag} node.passive=false node.dont-reconnect=true state.restore-props=false resample.quality=10 channelmix.normalize=true channelmix.upmix=true"
        );

        let role_to_use = spk.effective_role();
        let phase_inverted = spk.phase_inverted;

        match role_to_use {
            SpeakerRole::Left | SpeakerRole::SurroundLeft => {
                cap_props.push_str(" audio.position=[ FL ]");
                play_props.push_str(" audio.position=[ FL, FR ]");
            }
            SpeakerRole::Right | SpeakerRole::SurroundRight => {
                cap_props.push_str(" audio.position=[ FR ]");
                play_props.push_str(" audio.position=[ FL, FR ]");
            }
            SpeakerRole::Subwoofer | SpeakerRole::Center => {
                cap_props.push_str(" audio.position=[ FL, FR ]");
                play_props.push_str(" audio.position=[ FL, FR ]");
            }
            _ if phase_inverted => {
                cap_props.push_str(" audio.position=[ FL, FR ]");
                play_props.push_str(" audio.position=[ FR, FL ]");
            }
            _ => {}
        }

        if sink_name.to_ascii_lowercase().contains("pro-output") && spk.sink_id > 0 {
            let sid = spk.sink_id.to_string();
            run_quiet(
                "pw-cli",
                &["s", &sid, "Props", r#"{ params = [ "api.alsa.period-size" 512 "api.alsa.headroom" 1024 ] }"#],
                300,
            );
        }

        let mut cmd = Command::new("pw-loopback");
        cmd.arg(format!("--capture-props={cap_props}"))
            .arg(format!("--playback-props={play_props}"))
            .arg("-n")
            .arg(format!("polifonia_branch_{seq_tag}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        if !is_bt {
            cmd.args(["-l", "15"]);
        }
        if total_delay_ms > 0.05 {
            cmd.args(["--delay", &format!("{:.4}", total_delay_ms / 1000.0)]);
        }

        if let Ok(proc) = cmd.spawn() {
            let pid = proc.id();
            g.sync_processes.insert(sink_name.clone(), proc);
            g.running_sink_ids.insert(sink_name.clone(), spk.sink_id);
            g.running_delays.insert(sink_name.clone(), total_delay_ms);
            g.running_user_delays.insert(sink_name.clone(), spk.delay_ms);
            g.running_roles.insert(sink_name.clone(), role_to_use);
            g.running_phases.insert(sink_name.clone(), phase_inverted);
            g.running_muted.insert(sink_name.clone(), start_muted);

            let pid_str = pid.to_string();
            run_quiet("wpctl", &["set-mute", "-p", &pid_str, if start_muted { "1" } else { "0" }], 100);
            if !start_muted {
                let s_name = sink_name.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(80));
                    run_quiet("wpctl", &["set-mute", "-p", &pid_str, "0"], 100);
                    run_quiet("wpctl", &["set-volume", "-p", &pid_str, "1.0"], 100);
                    run_quiet("pactl", &["set-sink-mute", &s_name, "0"], 100);
                });
            }
        }
    }

    pub fn sync_active_branches(&self, live_sinks_opt: Option<&[AudioSink]>) {
        if !self.is_running() {
            return;
        }
        self.ensure_master_sink();

        let owned_sinks;
        let live_sinks = match live_sinks_opt {
            Some(s) => s,
            None => {
                owned_sinks = DeviceScanner::scan_sinks(false);
                &owned_sinks
            }
        };

        let live_sink_map: HashMap<&str, &AudioSink> = live_sinks.iter().map(|s| (s.name.as_str(), s)).collect();
        let live_sink_names: HashSet<String> = live_sinks.iter().map(|s| s.name.clone()).collect();

        let mut kills: Vec<Child> = Vec::new();
        let mut mute_updates: Vec<(String, bool)> = Vec::new();
        let mut to_launch: Vec<(SpeakerConfig, f64, bool)> = Vec::new();

        {
            let mut g = self.inner.lock().unwrap();
            if !g.is_active {
                return;
            }

            if !live_sink_map.is_empty() {
                for ch in g.config.speakers.values_mut() {
                    if let Some(live_obj) = live_sink_map.get(ch.sink_name.as_str()) {
                        ch.is_internal = live_obj.is_internal;
                        ch.sink_id = live_obj.id;
                        if live_obj.latency_ms > 0.0 {
                            ch.hardware_latency_ms = live_obj.latency_ms;
                        }
                    }
                }
            }

            // Compute baselines across ACTIVE non-internal speakers only (fixes Bug #1C:
            // disabled speakers with stale negative delays must never delay active speakers).
            let active_profile_speakers: Vec<SpeakerConfig> = g
                .config
                .speakers
                .values()
                .filter(|s| {
                    s.role.is_enabled()
                        && !s.is_internal
                        && (live_sink_names.is_empty() || live_sink_names.contains(&s.sink_name))
                })
                .cloned()
                .collect();

            let baseline_pool = if !active_profile_speakers.is_empty() {
                active_profile_speakers
            } else {
                g.config
                    .speakers
                    .values()
                    .filter(|s| s.role.is_enabled())
                    .cloned()
                    .collect()
            };

            let max_hw_latency = baseline_pool
                .iter()
                .map(|s| s.hardware_latency_ms)
                .fold(0.0f64, f64::max);
            let user_min_delay = baseline_pool
                .iter()
                .map(|s| s.delay_ms)
                .fold(0.0f64, f64::min)
                .min(0.0);

            let configured_keys: Vec<String> = g
                .config
                .speakers
                .keys()
                .filter(|k| live_sink_names.is_empty() || live_sink_names.contains(*k))
                .cloned()
                .collect();

            let to_terminate: Vec<String> = g
                .sync_processes
                .keys()
                .filter(|k| !live_sink_names.is_empty() && !live_sink_names.contains(*k))
                .cloned()
                .collect();

            for s_name in to_terminate {
                if let Some(p) = g.pop_branch_state(&s_name) {
                    kills.push(p);
                }
            }

            for sink_name in configured_keys {
                let mut spk = match g.config.speakers.get(&sink_name) {
                    Some(s) => s.clone(),
                    None => continue,
                };
                let is_enabled = spk.role.is_enabled();
                if let Some(orig) = g.config.speakers.get_mut(&sink_name) {
                    orig.mute = !is_enabled;
                }
                spk.mute = !is_enabled;

                let target_role = spk.effective_role();
                let target_phase = spk.phase_inverted;
                let total_delay_ms =
                    (spk.delay_ms - user_min_delay) + (max_hw_latency - spk.hardware_latency_ms).max(0.0);
                let is_bt = is_wireless_speaker(&spk);

                let proc_dead = match g.sync_processes.get_mut(&sink_name) {
                    None => true,
                    Some(p) => p.try_wait().ok().flatten().is_some(),
                };
                let prev_sink_id = g.running_sink_ids.get(&sink_name).copied();
                let sink_id_changed = live_sink_map
                    .get(sink_name.as_str())
                    .map(|l| prev_sink_id.is_some() && prev_sink_id != Some(l.id))
                    .unwrap_or(false);

                let last_user_delay = g.running_user_delays.get(&sink_name).copied();
                let running_total = g.running_delays.get(&sink_name).copied();

                let needs_restart = g.running_roles.get(&sink_name).copied() != Some(target_role)
                    || g.running_phases.get(&sink_name).copied().unwrap_or(false) != target_phase
                    || last_user_delay.map(|d| (d - spk.delay_ms).abs() > 0.05).unwrap_or(true)
                    || (g.running_user_min_delay - user_min_delay).abs() > 0.05
                    || running_total.map(|d| (d - total_delay_ms).abs() > 1.0).unwrap_or(false)
                    || sink_id_changed;

                if !proc_dead && !needs_restart {
                    let should_be_muted = !is_enabled;
                    if g.running_muted.get(&sink_name).copied().unwrap_or(false) != should_be_muted {
                        mute_updates.push((sink_name.clone(), should_be_muted));
                    }
                    continue;
                }

                if !is_enabled {
                    if let Some(p) = g.pop_branch_state(&sink_name) {
                        kills.push(p);
                    }
                    continue;
                }

                if let Some(p) = g.pop_branch_state(&sink_name) {
                    kills.push(p);
                }
                to_launch.push((spk, total_delay_ms, is_bt));
            }

            // Kill obsolete or restarting branches first
            for p in kills.drain(..) {
                Self::async_kill(p);
            }

            // Launch wireless endpoints (0) before wired endpoints (1)
            to_launch.sort_by_key(|(_, _, is_bt)| if *is_bt { 0 } else { 1 });
            for (spk, total_delay_ms, is_bt) in to_launch {
                let vol_pct = format!("{}%", (spk.volume_gain * 100.0) as u32);
                run_quiet("pactl", &["set-sink-mute", &spk.sink_name, "0"], 500);
                run_quiet("pactl", &["set-sink-volume", &spk.sink_name, &vol_pct], 500);
                thread::sleep(Duration::from_millis(40));
                Self::spawn_loopback_locked(&mut g, &spk, total_delay_ms, is_bt, false);
            }

            g.running_user_min_delay = user_min_delay;
        }

        for (s_name, m) in mute_updates {
            self.set_speaker_mute(&s_name, m);
        }
        run_quiet("pactl", &["set-sink-mute", "polifonia_master", "0"], 500);
        self.reassert_master_default();
    }

    pub fn restart_single_branch(&self, sink_name: &str) {
        let old = self.inner.lock().unwrap().pop_branch_state(sink_name);
        if let Some(p) = old {
            Self::async_kill(p);
        }
        self.sync_active_branches(None);
    }

    pub fn start_unison_sink(self: &Arc<Self>) -> bool {
        let has_active = {
            let g = self.inner.lock().unwrap();
            g.config.speakers.values().any(|s| s.role.is_enabled())
        };
        if !has_active || !self.ensure_master_sink() {
            return false;
        }
        self.inner.lock().unwrap().is_active = true;
        self.sync_active_branches(None);
        self.start_watchdog();
        true
    }

    pub fn activate_unison(self: &Arc<Self>, config: Option<SystemConfig>) -> bool {
        if let Some(cfg) = config {
            self.inner.lock().unwrap().config = cfg;
        }
        self.start_unison_sink()
    }

    pub fn stop_unison_sink(&self, unload_master: bool) {
        self.watchdog_stop.store(true, Ordering::Relaxed);
        let (procs, sink_names, prev_name, prev_id) = {
            let mut g = self.inner.lock().unwrap();
            g.is_active = false;
            let procs = g.clear_all_branch_states();
            let names: Vec<String> = g.config.speakers.keys().cloned().collect();
            (procs, names, g.prev_default_sink_name.clone(), g.prev_default_sink_id)
        };

        for p in procs {
            Self::async_kill(p);
        }
        run_quiet("pkill", &["-f", "pw-loopback.*polifonia"], 300);

        for name in sink_names {
            run_quiet("pactl", &["set-sink-mute", &name, "0"], 200);
        }

        let mut target_name = prev_name;
        let mut target_id = prev_id;
        if target_name
            .as_deref()
            .map(|n| n.is_empty() || n.to_ascii_lowercase().contains("polifonia"))
            .unwrap_or(true)
        {
            let live = DeviceScanner::scan_sinks(false);
            if let Some(valid) = live
                .iter()
                .find(|s| !s.is_internal && !s.name.to_ascii_lowercase().contains("polifonia"))
                .or_else(|| live.first())
            {
                target_name = Some(valid.name.clone());
                target_id = Some(valid.id);
            }
        }

        if let Some(ref tname) = target_name {
            if !tname.to_ascii_lowercase().contains("polifonia") {
                self.route_active_streams_to_sink(tname);
                run_quiet("pactl", &["set-default-sink", tname], 1000);
                run_quiet("pactl", &["set-sink-mute", tname, "0"], 500);
            }
        }
        if let Some(tid) = target_id {
            let s = tid.to_string();
            run_quiet("wpctl", &["set-default", &s], 1000);
        }

        if unload_master {
            {
                let mut g = self.inner.lock().unwrap();
                g.prev_default_sink_id = None;
                g.prev_default_sink_name = None;
            }
            self.cleanup_all_orphan_nodes(true);
            self.heal_dangling_default_sink();
        }
    }

    fn start_watchdog(self: &Arc<Self>) {
        self.watchdog_stop.store(false, Ordering::Relaxed);
        if self.watchdog_running.swap(true, Ordering::SeqCst) {
            return;
        }

        let engine = Arc::clone(self);
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(3));
                if engine.watchdog_stop.load(Ordering::Relaxed) || !engine.is_running() {
                    if engine.watchdog_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    continue;
                }
                if !engine.is_master_sink_alive() && !engine.ensure_master_sink() {
                    continue;
                }
                let live_sinks = DeviceScanner::scan_sinks(false);
                let live_names: HashSet<String> = live_sinks.iter().map(|s| s.name.clone()).collect();
                let live_map: HashMap<String, AudioSink> =
                    live_sinks.iter().map(|s| (s.name.clone(), s.clone())).collect();

                let mut need_resync = false;
                {
                    let mut g = engine.inner.lock().unwrap();
                    let active_names: Vec<String> = g
                        .config
                        .speakers
                        .values()
                        .filter(|s| {
                            s.role.is_enabled() && (live_names.is_empty() || live_names.contains(&s.sink_name))
                        })
                        .map(|s| s.sink_name.clone())
                        .collect();

                    for s_name in active_names {
                        let proc_dead = match g.sync_processes.get_mut(&s_name) {
                            None => true,
                            Some(p) => p.try_wait().ok().flatten().is_some(),
                        };
                        if proc_dead {
                            if let Some(p) = g.pop_branch_state(&s_name) {
                                Self::async_kill(p);
                            }
                            need_resync = true;
                            continue;
                        }
                        if let Some(live_obj) = live_map.get(&s_name) {
                            let id_changed = g
                                .running_sink_ids
                                .get(&s_name)
                                .map(|&id| id != live_obj.id)
                                .unwrap_or(false);
                            let lat_drifted = g
                                .config
                                .speakers
                                .get(&s_name)
                                .map(|c| {
                                    live_obj.latency_ms > 0.0
                                        && (live_obj.latency_ms - c.hardware_latency_ms).abs() > 25.0
                                })
                                .unwrap_or(false);

                            if id_changed || lat_drifted {
                                if let Some(c) = g.config.speakers.get_mut(&s_name) {
                                    c.sink_id = live_obj.id;
                                    if live_obj.latency_ms > 0.0 {
                                        c.hardware_latency_ms = live_obj.latency_ms;
                                    }
                                }
                                if let Some(p) = g.pop_branch_state(&s_name) {
                                    Self::async_kill(p);
                                }
                                need_resync = true;
                            }
                        }
                    }
                }
                if need_resync {
                    engine.sync_active_branches(Some(&live_sinks));
                }
            }
            engine.watchdog_running.store(false, Ordering::SeqCst);
        });
    }

    pub fn get_routing_status(&self) -> RoutingStatus {
        let sink_states = DeviceScanner::get_sink_states();
        let sink_inputs = DeviceScanner::get_active_sink_inputs();
        let live_sinks = DeviceScanner::scan_sinks(false);
        let live_names: HashSet<String> = live_sinks.iter().map(|s| s.name.clone()).collect();

        let mut g = self.inner.lock().unwrap();
        let is_active = g.is_active;
        let master_volume = g.config.master_volume;
        let master_state = sink_states
            .get("polifonia_master")
            .cloned()
            .unwrap_or_else(|| if is_active { "IDLE" } else { "STOPPED" }.to_string());

        let active_pool: Vec<SpeakerConfig> = g
            .config
            .speakers
            .values()
            .filter(|s| {
                s.role.is_enabled() && !s.is_internal && (live_names.is_empty() || live_names.contains(&s.sink_name))
            })
            .cloned()
            .collect();

        let max_hw_latency = active_pool
            .iter()
            .map(|s| s.hardware_latency_ms)
            .fold(0.0f64, f64::max);
        let user_min_delay = active_pool
            .iter()
            .map(|s| s.delay_ms)
            .fold(0.0f64, f64::min)
            .min(0.0);

        let keys: Vec<String> = g
            .config
            .speakers
            .keys()
            .filter(|k| live_names.is_empty() || live_names.contains(*k))
            .cloned()
            .collect();

        let mut destinations = Vec::new();
        for key in keys {
            let ch = g.config.speakers.get(&key).cloned().unwrap();
            let is_enabled = ch.role.is_enabled();
            let auto_comp = if is_enabled {
                (max_hw_latency - ch.hardware_latency_ms).max(0.0)
            } else {
                0.0
            };
            let user_offset = if is_enabled { ch.delay_ms - user_min_delay } else { 0.0 };
            let total_delay_ms = if is_enabled { user_offset + auto_comp } else { 0.0 };
            let state = sink_states
                .get(&ch.sink_name)
                .or_else(|| sink_states.get(&ch.sink_id.to_string()))
                .cloned()
                .unwrap_or_else(|| "SUSPENDED".to_string());

            let proc_running = match g.sync_processes.get_mut(&ch.sink_name) {
                Some(p) => p.try_wait().ok().flatten().is_none(),
                None => false,
            };

            destinations.push(DestinationStatus {
                sink_id: ch.sink_id,
                sink_name: ch.sink_name.clone(),
                display_name: ch.label().to_string(),
                bus_type: ch.bus_type.clone(),
                role: ch.role,
                volume_gain: ch.volume_gain,
                delay_ms: ch.delay_ms,
                phase_inverted: ch.phase_inverted,
                hardware_latency_ms: ch.hardware_latency_ms,
                auto_comp_delay_ms: auto_comp,
                total_delay_ms,
                is_enabled,
                is_streaming: is_active && is_enabled && (proc_running || state == "RUNNING"),
                state,
            });
        }

        RoutingStatus {
            is_unison_active: is_active,
            master_volume,
            master_state,
            sources: sink_inputs,
            destinations,
            max_hw_latency,
        }
    }

    pub fn reset_all_delays(&self) {
        {
            let mut g = self.inner.lock().unwrap();
            for spk in g.config.speakers.values_mut() {
                spk.delay_ms = 0.0;
            }
            let _ = self.storage.save(&mut g.config, None);
        }
        if self.is_running() {
            self.sync_active_branches(None);
        }
    }

    /// Closed-loop single-channel acoustic calibration via Hilbert analytic cross-correlation.
    pub fn measure_acoustic_delay<F>(&self, target_sink: &str, mut progress_cb: F) -> Result<f64, String>
    where
        F: FnMut(&str),
    {
        let (ref_sink, ref_user_delay, ref_hw_lat, target_hw_lat, is_target_bt) = {
            let g = self.inner.lock().unwrap();
            let active_others: Vec<&SpeakerConfig> = g
                .config
                .speakers
                .values()
                .filter(|s| s.sink_name != target_sink && s.role.is_enabled())
                .collect();
            let chosen = active_others
                .iter()
                .find(|s| !is_wireless_speaker(s))
                .or_else(|| active_others.first())
                .copied();
            let Some(ref_spk) = chosen else {
                return Err("Activate at least one other speaker as calibration reference.".to_string());
            };
            let target_hw = g
                .config
                .speakers
                .get(target_sink)
                .map(|s| s.hardware_latency_ms)
                .unwrap_or(0.0);
            let bt = is_wireless_name(target_sink)
                || g.config
                    .speakers
                    .get(target_sink)
                    .map(is_wireless_speaker)
                    .unwrap_or(false);
            (
                ref_spk.sink_name.clone(),
                ref_spk.delay_ms,
                ref_spk.hardware_latency_ms,
                target_hw,
                bt,
            )
        };

        progress_cb("Generating calibration signals...");
        let sr = 48000u32;
        let (chirp_signal, pcm) = build_probe_chirp_pcm(sr);

        let chirp_wav = temp_file_path("polifonia_chirp_single", "wav");
        let raw_rec = temp_file_path("polifonia_rec_single", "raw");
        let _guard = TempCleanupGuard::new(vec![chirp_wav.clone(), raw_rec.clone()]);
        let _mic_guard = MicStateGuard::new(None);

        write_stereo_wav(&chirp_wav, sr, &pcm).map_err(|e| format!("WAV write error: {e}"))?;
        progress_cb("Starting microphone capture...");

        let raw_str = raw_rec.to_string_lossy().to_string();
        run_quiet("pactl", &["set-sink-mute", "polifonia_master", "1"], 500);
        let mut proc_rec = Command::new("pw-record")
            .args(["--format=s16", "--rate=48000", "--channels=1", &raw_str])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to start pw-record: {e}"))?;

        let (t_start, lead_in_s) = wait_for_recorder_start(&mut proc_rec, &raw_rec, sr)?;
        let wav_str = chirp_wav.to_string_lossy().to_string();

        progress_cb("Playing reference chirp...");
        let ref_rel = lead_in_s + t_start.elapsed().as_secs_f64();
        run_quiet(
            "pw-play",
            &[&format!("--target={ref_sink}"), "--volume=0.95", &wav_str],
            2000,
        );
        thread::sleep(Duration::from_millis(900));

        progress_cb("Playing target chirp...");
        let target_rel = lead_in_s + t_start.elapsed().as_secs_f64();
        let t_target_offset_s = target_rel - ref_rel;
        run_quiet(
            "pw-play",
            &[&format!("--target={target_sink}"), "--volume=0.95", &wav_str],
            2000,
        );

        progress_cb("Listening for audio echoes...");
        thread::sleep(Duration::from_millis(1400));

        stop_recorder_cleanly(proc_rec);
        run_quiet("pactl", &["set-sink-mute", "polifonia_master", "0"], 500);

        let rec_arr = read_pcm_s16_le(&raw_rec)?;
        if rec_arr.len() < sr as usize {
            return Err("Insufficient microphone data or microphone inactive.".to_string());
        }

        progress_cb("Analyzing acoustic cross-correlation...");
        let corr = compute_robust_correlation(&rec_arr, &chirp_signal);
        let noise_floor = compute_trimmed_noise_floor(&corr);

        let sr_usize = sr as usize;
        let sr_f = sr as f64;
        let pre_pad_s = 0.05;
        let win1_start = ((ref_rel + pre_pad_s + 0.01) * sr_f).max(0.0) as usize;
        let win1_end = corr
            .len()
            .min(((ref_rel + pre_pad_s + 0.85) * sr_f) as usize);
        let (p1, val1) =
            detect_first_arrival_peak(&corr, win1_start, win1_end, sr_usize, noise_floor);

        let win2_start = (((target_rel + pre_pad_s + if is_target_bt { 0.08 } else { 0.01 })
            * sr_f)
            .max(0.0) as usize)
            .max((p1 as usize) + (sr_usize / 2))
            .min(corr.len().saturating_sub(1));
        let win2_end = corr.len().min(
            ((target_rel + pre_pad_s + if is_target_bt { 0.85 } else { 0.45 }) * sr_f) as usize,
        );
        if win2_end <= win2_start {
            return Err("Incomplete recording.".to_string());
        }
        let (p2, val2) =
            detect_first_arrival_peak(&corr, win2_start, win2_end, sr_usize, noise_floor);

        let snr1 = val1 / noise_floor;
        let snr2 = val2 / noise_floor;
        eprintln!(
            "[CALIB-SINGLE] ref_snr={snr1:.2} target_snr={snr2:.2} p1={:.3}s p2={:.3}s",
            p1 / sr_f,
            p2 / sr_f
        );
        if snr1 < 4.5 || snr2 < 4.5 {
            return Err(format!(
                "Acoustic signal too weak relative to room noise (SNR {:.1} / {:.1}, need >= 4.5).",
                snr1, snr2
            ));
        }

        let meas_diff_ms = (((p2 - p1) / sr_f) - t_target_offset_s) * 1000.0;
        let hw_comp_delta = target_hw_lat - ref_hw_lat;
        let residual = meas_diff_ms - hw_comp_delta;
        let max_res = if is_target_bt { 350.0 } else { 150.0 };
        if residual.abs() > max_res {
            return Err(format!(
                "Measured offset ({residual:+.1} ms) outside physical bounds; check microphone placement."
            ));
        }

        let target_delay_ms = ((ref_user_delay - residual) * 10.0).round() / 10.0;
        Ok(target_delay_ms.clamp(-350.0, 350.0))
    }

    /// Closed-loop system identification aligning all active external speakers automatically.
    pub fn auto_align_all_speakers<F>(
        self: &Arc<Self>,
        mut progress_cb: F,
    ) -> Result<(String, HashMap<String, f64>), String>
    where
        F: FnMut(&str),
    {
        let (mut active_speakers, master_vol): (Vec<SpeakerConfig>, f64) = {
            let g = self.inner.lock().unwrap();
            (
                g.config
                    .speakers
                    .values()
                    .filter(|s| s.role.is_enabled() && !s.is_internal)
                    .cloned()
                    .collect(),
                g.config.master_volume,
            )
        };

        if active_speakers.len() < 2 {
            return Err(
                "Activate at least 2 output channels to perform automatic synchronization."
                    .to_string(),
            );
        }

        if !self.is_running() {
            progress_cb("Starting Unison graph...");
            if self.start_unison_sink() {
                thread::sleep(Duration::from_millis(450));
            }
        }

        let sr = 48000u32;
        let pre_pad_s = 0.05f64;
        let (chirp_signal, pcm) = build_probe_chirp_pcm(sr);

        let chirp_wav = temp_file_path("polifonia_chirp_auto", "wav");
        let raw_rec = temp_file_path("polifonia_rec_auto", "raw");
        let _file_guard = TempCleanupGuard::new(vec![chirp_wav.clone(), raw_rec.clone()]);
        let _mic_guard = MicStateGuard::new(Some(master_vol));
        write_stereo_wav(&chirp_wav, sr, &pcm).map_err(|e| format!("WAV write error: {e}"))?;

        // Prepare muted background streams, speaker order, and pipeline state BEFORE starting pw-record
        // so setup delays never shift rec_arr[0] away from t_start.
        let mut muted_inputs = Vec::new();
        for inp in DeviceScanner::get_active_sink_inputs() {
            if inp.id.chars().all(|c| c.is_ascii_digit()) {
                run_quiet("pactl", &["set-sink-input-mute", &inp.id, "1"], 200);
                muted_inputs.push(inp.id);
            }
        }

        active_speakers.sort_by(|a, b| {
            let aw = if is_wireless_speaker(a) { 1 } else { 0 };
            let bw = if is_wireless_speaker(b) { 1 } else { 0 };
            aw.cmp(&bw).then_with(|| {
                a.hardware_latency_ms
                    .partial_cmp(&b.hardware_latency_ms)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });

        // Keep existing pw-loopback branches alive so wireless sinks remain RUNNING in A2DP mode
        // without cold-start reconnection delay, while muting polifonia_master and playing the
        // probe directly to each physical sink so Bluetooth sinks never get muted mid-session.
        let wav_str = chirp_wav.to_string_lossy().to_string();
        run_quiet("pactl", &["set-sink-mute", "polifonia_master", "1"], 300);
        for spk in &active_speakers {
            run_quiet("pactl", &["set-sink-mute", &spk.sink_name, "0"], 150);
        }

        progress_cb("Starting microphone capture...");
        let raw_str = raw_rec.to_string_lossy().to_string();
        let mut proc_rec = Command::new("pw-record")
            .args(["--format=s16", "--rate=48000", "--channels=1", &raw_str])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to start pw-record: {e}"))?;

        let (t_start, lead_in_s) = wait_for_recorder_start(&mut proc_rec, &raw_rec, sr)?;

        struct Emission {
            spk: SpeakerConfig,
            t_emit_rel_s: f64,
            applied_loop_delay_ms: f64,
        }
        let mut emissions = Vec::new();

        for spk in &active_speakers {
            progress_cb(&format!("Measuring: {}...", spk.label()));
            let is_bt = is_wireless_speaker(spk);
            let rel = lead_in_s + t_start.elapsed().as_secs_f64();
            run_quiet(
                "pw-play",
                &[&format!("--target={}", spk.sink_name), "--volume=0.95", &wav_str],
                2500,
            );
            let drain_ms = if is_bt { 850 } else { 450 };
            emissions.push(Emission {
                spk: spk.clone(),
                t_emit_rel_s: rel + pre_pad_s,
                applied_loop_delay_ms: 0.0,
            });
            thread::sleep(Duration::from_millis(drain_ms));
        }

        progress_cb("Listening to acoustic echoes...");
        thread::sleep(Duration::from_millis(1000));

        stop_recorder_cleanly(proc_rec);

        // Always restore all active speaker & client stream states immediately.
        run_quiet("pactl", &["set-sink-mute", "polifonia_master", "0"], 500);
        for spk in &active_speakers {
            if !spk.mute {
                run_quiet("pactl", &["set-sink-mute", &spk.sink_name, "0"], 150);
            }
        }
        for inp_id in muted_inputs {
            run_quiet("pactl", &["set-sink-input-mute", &inp_id, "0"], 200);
        }

        let rec_arr = read_pcm_s16_le(&raw_rec)?;
        if rec_arr.len() < sr as usize {
            return Err("Insufficient microphone data for acoustic analysis.".to_string());
        }

        progress_cb("Computing Hilbert phase alignment...");
        let corr = compute_robust_correlation(&rec_arr, &chirp_signal);
        let noise_floor = compute_trimmed_noise_floor(&corr);

        let sr_usize = sr as usize;
        let sr_f = sr as f64;
        let mut latencies: HashMap<String, f64> = HashMap::new();
        let mut last_peak_idx = 0usize;

        for (idx, em) in emissions.iter().enumerate() {
            let is_bt = is_wireless_speaker(&em.spk);
            let min_after_prev = if last_peak_idx > 0 {
                last_peak_idx + (sr_usize * 8) / 100
            } else {
                0
            };
            let emit_offset_s = em.t_emit_rel_s + (em.applied_loop_delay_ms / 1000.0);

            let (win_start, mut win_end) = if !is_bt {
                let ws = min_after_prev.max((sr_f * (emit_offset_s + 0.01)).max(0.0) as usize);
                let we = corr.len().min((sr_f * (emit_offset_s + 0.45)) as usize);
                (ws, we)
            } else {
                let ws = min_after_prev.max((sr_f * (emit_offset_s + 0.08)).max(0.0) as usize);
                let we = corr.len().min((sr_f * (emit_offset_s + 0.85)) as usize);
                (ws, we)
            };

            if let Some(next_em) = emissions.get(idx + 1) {
                let next_emit_s = next_em.t_emit_rel_s + (next_em.applied_loop_delay_ms / 1000.0);
                if next_emit_s > emit_offset_s + 0.08 {
                    let bound = (win_start + (sr_usize * 5) / 100)
                        .max((sr_f * (next_emit_s - 0.01)) as usize);
                    win_end = win_end.min(bound);
                }
            }

            if win_end <= win_start {
                continue;
            }
            let (exact_peak_idx, peak_val) =
                detect_first_arrival_peak(&corr, win_start, win_end, sr_usize, noise_floor);
            let snr = peak_val / noise_floor;
            eprintln!(
                "[AUTO-SYNC] speaker='{}' win=[{:.2}s..{:.2}s] peak_at={:.3}s snr={:.2} applied_delay={:.1}ms",
                em.spk.label(),
                win_start as f64 / sr_f,
                win_end as f64 / sr_f,
                exact_peak_idx / sr_f,
                snr,
                em.applied_loop_delay_ms
            );
            if snr >= 4.5 {
                last_peak_idx = exact_peak_idx as usize;
                let measured_latency_ms = ((exact_peak_idx / sr_f) - em.t_emit_rel_s) * 1000.0;
                latencies.insert(
                    em.spk.sink_name.clone(),
                    (measured_latency_ms - em.applied_loop_delay_ms).max(0.0),
                );
            }
        }

        let ref_speaker = &active_speakers[0];
        let Some(&ref_lat) = latencies.get(&ref_speaker.sink_name) else {
            return Err(format!(
                "Microphone signal too weak (detected {}/{} speakers, need SNR >= 4.5). Existing delays preserved.",
                latencies.len(),
                active_speakers.len()
            ));
        };
        if latencies.len() < 2 {
            return Err(format!(
                "Microphone signal too weak (detected {}/{} speakers, need SNR >= 4.5). Existing delays preserved.",
                latencies.len(),
                active_speakers.len()
            ));
        }

        let ref_hw_lat = ref_speaker.hardware_latency_ms;
        let mut calibrated_delays = HashMap::new();
        let mut adjusted_count = 0usize;

        {
            let mut g = self.inner.lock().unwrap();
            for spk in &active_speakers {
                let val = if spk.sink_name == ref_speaker.sink_name {
                    0.0
                } else if let Some(&spk_lat) = latencies.get(&spk.sink_name) {
                    let is_bt = is_wireless_speaker(spk);
                    let phys_diff = spk_lat - ref_lat;
                    let hw_comp_delta = spk.hardware_latency_ms - ref_hw_lat;
                    let residual = phys_diff - hw_comp_delta;
                    let max_residual = if is_bt { 350.0 } else { 150.0 };

                    if residual.abs() > max_residual {
                        spk.delay_ms
                    } else if residual.abs() <= 0.3 {
                        0.0
                    } else {
                        ((-residual * 10.0).round() / 10.0).clamp(-350.0, 350.0)
                    }
                } else {
                    spk.delay_ms
                };

                if (val - spk.delay_ms).abs() > 0.05 || val.abs() > 0.05 {
                    adjusted_count += 1;
                }
                if let Some(cfg_spk) = g.config.speakers.get_mut(&spk.sink_name) {
                    cfg_spk.delay_ms = val;
                }
                calibrated_delays.insert(spk.sink_name.clone(), val);
            }
            let _ = self.storage.save(&mut g.config, None);
        }

        if self.is_running() {
            self.sync_active_branches(None);
        }

        let summary = if adjusted_count == 0 {
            "All speakers are already in perfect natural alignment (0.0 ms).".to_string()
        } else {
            format!("Calibrated {adjusted_count} speakers with precision fine delays.")
        };
        Ok((summary, calibrated_delays))
    }
}

impl Drop for AudioEngineService {
    fn drop(&mut self) {
        self.watchdog_stop.store(true, Ordering::Relaxed);
        let (lock, cvar) = &*self.gain_queue;
        lock.lock().unwrap().stop = true;
        cvar.notify_all();
    }
}

struct TempCleanupGuard {
    paths: Vec<PathBuf>,
}

impl TempCleanupGuard {
    fn new(paths: Vec<PathBuf>) -> Self {
        Self { paths }
    }
}

impl Drop for TempCleanupGuard {
    fn drop(&mut self) {
        for p in &self.paths {
            let _ = fs::remove_file(p);
        }
    }
}

fn build_probe_chirp_pcm(sr: u32) -> (Vec<f32>, Vec<i16>) {
    let chirp_signal = synthesize_linear_chirp(1200.0, 3500.0, 0.06, sr);
    let pre_pad = (sr as f64 * 0.05) as usize;
    let post_pad = (sr as f64 * 0.25) as usize;

    let mut pcm = vec![0i16; pre_pad];
    for &v in &chirp_signal {
        pcm.push((v * 32767.0 * 0.80).round().clamp(-32767.0, 32767.0) as i16);
    }
    pcm.extend(std::iter::repeat_n(0i16, post_pad));
    (chirp_signal, pcm)
}

/// Computes a robust 75% trimmed mean noise floor so bright chirps and reverb tails
/// do not artificially depress the signal-to-noise ratio estimate.
fn compute_trimmed_noise_floor(corr: &[f32]) -> f32 {
    if corr.is_empty() {
        return 1e-6;
    }
    let mut samples: Vec<f32> = corr.iter().step_by(4).copied().collect();
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let keep = ((samples.len() * 3) / 4).max(1);
    let sum: f64 = samples[..keep].iter().map(|&v| v as f64).sum();
    (sum / keep as f64).max(1e-6) as f32
}

/// Waits until `pw-record` has flushed its first PCM chunk to disk so `t_start`
/// aligns with the exact sample timeline stored in `raw_rec`.
fn wait_for_recorder_start(
    proc_rec: &mut Child,
    raw_rec: &std::path::Path,
    sr: u32,
) -> Result<(Instant, f64), String> {
    for _ in 0..120 {
        if proc_rec.try_wait().ok().flatten().is_some() {
            return Err("Unable to start microphone recording.".to_string());
        }
        let bytes = fs::metadata(raw_rec).map(|m| m.len()).unwrap_or(0);
        if bytes > 0 {
            let lead_in_s = (bytes / 2) as f64 / (sr as f64);
            return Ok((Instant::now(), lead_in_s));
        }
        thread::sleep(Duration::from_millis(5));
    }
    Ok((Instant::now(), 0.0))
}

/// Sends `SIGINT` to flush `pw-record`'s buffered capture tail cleanly before waiting.
fn stop_recorder_cleanly(mut proc_rec: Child) {
    unsafe {
        libc::kill(proc_rec.id() as libc::pid_t, libc::SIGINT);
    }
    for _ in 0..15 {
        if proc_rec.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = proc_rec.kill();
    let _ = proc_rec.wait();
}

/// Temporarily unmutes/boosts the default microphone (and optionally `polifonia_master`)
/// during acoustic calibration and restores original levels automatically on drop.
struct MicStateGuard {
    orig_muted: bool,
    orig_vol_pct: Option<u32>,
    boosted_mic: bool,
    master_vol_restore: Option<f64>,
}

impl MicStateGuard {
    fn new(master_vol_restore: Option<f64>) -> Self {
        let orig_muted = run_cmd("pactl", &["get-source-mute", "@DEFAULT_SOURCE@"], 300)
            .map(|s| s.to_ascii_lowercase().contains("yes"))
            .unwrap_or(false);
        let orig_vol_pct = run_cmd("pactl", &["get-source-volume", "@DEFAULT_SOURCE@"], 300)
            .and_then(|s| {
                s.split_whitespace().find_map(|tok| {
                    tok.strip_suffix('%').and_then(|num| num.parse::<u32>().ok())
                })
            });

        if orig_muted {
            run_quiet("pactl", &["set-source-mute", "@DEFAULT_SOURCE@", "0"], 300);
        }
        let boosted_mic = orig_vol_pct.map(|v| v < 95).unwrap_or(false);
        if boosted_mic {
            run_quiet("pactl", &["set-source-volume", "@DEFAULT_SOURCE@", "100%"], 300);
        }
        if master_vol_restore.is_some() {
            run_quiet("pactl", &["set-sink-volume", "polifonia_master", "90%"], 300);
        }

        Self {
            orig_muted,
            orig_vol_pct,
            boosted_mic,
            master_vol_restore,
        }
    }
}

impl Drop for MicStateGuard {
    fn drop(&mut self) {
        if self.boosted_mic {
            if let Some(pct) = self.orig_vol_pct {
                run_quiet(
                    "pactl",
                    &["set-source-volume", "@DEFAULT_SOURCE@", &format!("{pct}%")],
                    300,
                );
            }
        }
        if self.orig_muted {
            run_quiet("pactl", &["set-source-mute", "@DEFAULT_SOURCE@", "1"], 300);
        }
        if let Some(m) = self.master_vol_restore {
            let pct = (m * 100.0).round() as i32;
            run_quiet(
                "pactl",
                &["set-sink-volume", "polifonia_master", &format!("{pct}%")],
                300,
            );
        }
    }
}

fn read_pcm_s16_le(path: &std::path::Path) -> Result<Vec<f32>, String> {
    let mut f = File::open(path).map_err(|e| format!("Failed to read microphone capture: {e}"))?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)
        .map_err(|e| format!("Failed reading capture bytes: {e}"))?;
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        out.push(i16::from_le_bytes([chunk[0], chunk[1]]) as f32);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hilbert_correlation_phase_invariance_and_chirp_sweep() {
        let sr = 48000u32;
        let tpl = synthesize_linear_chirp(1200.0, 3500.0, 0.06, sr);
        assert_eq!(tpl.len(), 2880);

        // Place delayed template inside a 1.0s recording at exact sample offset 12000 (250.0 ms).
        let true_offset = 12000usize;
        let mut rec = vec![0.0f32; 48000];
        for (i, &s) in tpl.iter().enumerate() {
            rec[true_offset + i] = s;
        }
        // Test 180-degree polarity inversion invariance via envelope detection.
        let mut rec_inv = vec![0.0f32; 48000];
        for (i, &s) in tpl.iter().enumerate() {
            rec_inv[true_offset + i] = -s;
        }

        let corr1 = compute_robust_correlation(&rec, &tpl);
        let corr2 = compute_robust_correlation(&rec_inv, &tpl);
        let (p1, _) = detect_first_arrival_peak(&corr1, 0, corr1.len(), sr as usize, 1.0);
        let (p2, _) = detect_first_arrival_peak(&corr2, 0, corr2.len(), sr as usize, 1.0);

        assert!((p1 - true_offset as f64).abs() < 1.5, "p1={p1}");
        assert!((p2 - true_offset as f64).abs() < 1.5, "p2={p2}");
    }

    #[test]
    fn test_detect_first_arrival_prefers_direct_wavefront_over_reflection() {
        let sr = 48000usize;
        let mut corr = vec![1.0f32; 48000];
        // Direct path arrival at sample 10000 (smaller peak = 80), strong wall reflection 8ms later at 10384 (peak = 110).
        corr[9999] = 55.0;
        corr[10000] = 80.0;
        corr[10001] = 55.0;
        corr[10383] = 75.0;
        corr[10384] = 110.0;
        corr[10385] = 75.0;

        let (peak, _) = detect_first_arrival_peak(&corr, 9500, 11000, sr, 1.0);
        assert!((peak - 10000.0).abs() < 0.5, "Expected direct path ~10000, got {peak}");
    }

    #[test]
    fn test_snr_gate_rejects_ambient_noise_and_accepts_chirp() {
        let sr = 48000u32;
        let tpl = synthesize_linear_chirp(1200.0, 3500.0, 0.06, sr);

        // 2 seconds (96,000 samples) of deterministic LCG white ambient noise.
        let mut state: u64 = 0x1234_5678_9abc_def0;
        let mut noise = Vec::with_capacity(96_000);
        for _ in 0..96_000 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let sample = (((state >> 33) as f32) / (u32::MAX as f32 / 2.0)) * 2.0 - 1.0;
            noise.push(sample * 0.05);
        }

        let corr_noise = compute_robust_correlation(&noise, &tpl);
        let floor_noise = compute_trimmed_noise_floor(&corr_noise);
        let (_, peak_noise) =
            detect_first_arrival_peak(&corr_noise, 0, corr_noise.len(), sr as usize, floor_noise);
        let snr_noise = peak_noise / floor_noise;
        assert!(
            snr_noise < 4.5,
            "Rayleigh noise SNR {snr_noise} unexpectedly exceeded 4.5 gate"
        );

        // Superimpose chirp at sample 24,000 (500 ms) with amplitude 0.30.
        let true_idx = 24_000usize;
        let mut sig = noise;
        for (i, &v) in tpl.iter().enumerate() {
            sig[true_idx + i] += v * 0.30;
        }
        let corr_sig = compute_robust_correlation(&sig, &tpl);
        let floor_sig = compute_trimmed_noise_floor(&corr_sig);
        let (detected_idx, peak_sig) =
            detect_first_arrival_peak(&corr_sig, 0, corr_sig.len(), sr as usize, floor_sig);
        let snr_sig = peak_sig / floor_sig;
        assert!(snr_sig >= 4.5, "Signal SNR {snr_sig} failed 4.5 gate");
        assert!(
            (detected_idx - true_idx as f64).abs() < 2.0,
            "Expected peak near {true_idx}, got {detected_idx}"
        );
    }
}
