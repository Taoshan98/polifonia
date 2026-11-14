use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::engine::{AudioEngineService, RoutingStatus};
use crate::models::{AudioSink, SpeakerConfig, SpeakerRole, SystemConfig};
use crate::tray::TrayController;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveTab {
    Mixer,
    SignalFlow,
}

enum ScanResult {
    Sinks(Vec<AudioSink>),
    Routing(RoutingStatus),
}

enum AutoSyncEvent {
    Progress(String),
    Done(Result<(String, HashMap<String, f64>), String>),
}

enum SingleCalibEvent {
    Progress(String),
    Done(String, Result<f64, String>),
}

#[derive(Debug, Clone)]
struct PingRing {
    sink_name: String,
    started_at: Instant,
}

pub struct PolifoniaApp {
    engine: Arc<AudioEngineService>,
    tray: TrayController,
    window_visible: Arc<AtomicBool>,
    quit_requested: Arc<AtomicBool>,

    config: SystemConfig,
    live_sinks: HashMap<String, AudioSink>,
    routing_status: RoutingStatus,
    active_tab: ActiveTab,

    rename_target: Option<String>,
    rename_buffer: String,
    calib_modal_sink: Option<String>,

    auto_sync_running: bool,
    auto_sync_status: Option<String>,
    auto_sync_rx: Option<mpsc::Receiver<AutoSyncEvent>>,

    single_calib_running: bool,
    single_calib_status: Option<String>,
    single_calib_rx: Option<mpsc::Receiver<SingleCalibEvent>>,

    scan_tx: mpsc::Sender<ScanResult>,
    scan_rx: mpsc::Receiver<ScanResult>,
    scan_in_flight: bool,
    routing_in_flight: bool,

    hotplug_dirty: Arc<AtomicBool>,
    ui_sync_needed: Arc<AtomicBool>,
    last_master_sync: Instant,
    last_speaker_sync: HashMap<String, Instant>,
    last_routing_poll: Instant,

    toast: Option<(String, bool, Instant)>,
    flow_phase: f32,
    last_frame_time: Instant,
    pings: Vec<PingRing>,
}

impl PolifoniaApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        engine: Arc<AudioEngineService>,
        tray: TrayController,
        window_visible: Arc<AtomicBool>,
        quit_requested: Arc<AtomicBool>,
        egui_ctx_slot: Arc<Mutex<Option<egui::Context>>>,
        ui_sync_needed: Arc<AtomicBool>,
    ) -> Self {
        apply_studio_theme(&cc.egui_ctx);
        if let Ok(mut guard) = egui_ctx_slot.lock() {
            *guard = Some(cc.egui_ctx.clone());
        }

        let config = engine.config_snapshot();
        let (scan_tx, scan_rx) = mpsc::channel();
        let hotplug_dirty = Arc::new(AtomicBool::new(false));

        spawn_hotplug_watcher(
            cc.egui_ctx.clone(),
            Arc::clone(&hotplug_dirty),
        );

        // Initial asynchronous sink scan and startup Unison activation if previously active.
        let engine_init = Arc::clone(&engine);
        let tx_init = scan_tx.clone();
        let ctx_init = cc.egui_ctx.clone();
        let sync_flag_init = Arc::clone(&ui_sync_needed);
        let should_autostart = config.is_active;
        thread::Builder::new()
            .name("polifonia-startup".to_string())
            .spawn(move || {
                let sinks = engine_init.get_available_sinks(false);
                let _ = tx_init.send(ScanResult::Sinks(sinks));
                if should_autostart {
                    engine_init.activate_unison(None);
                    sync_flag_init.store(true, Ordering::Relaxed);
                }
                let routing = engine_init.get_routing_status();
                let _ = tx_init.send(ScanResult::Routing(routing));
                ctx_init.request_repaint();
            })
            .ok();

        let routing_status = RoutingStatus {
            is_unison_active: engine.is_running(),
            master_volume: config.master_volume,
            master_state: if engine.is_running() {
                "RUNNING".to_string()
            } else {
                "OFFLINE".to_string()
            },
            sources: Vec::new(),
            destinations: Vec::new(),
            max_hw_latency: 0.0,
        };

        Self {
            engine,
            tray,
            window_visible,
            quit_requested,
            config,
            live_sinks: HashMap::new(),
            routing_status,
            active_tab: ActiveTab::Mixer,
            rename_target: None,
            rename_buffer: String::new(),
            calib_modal_sink: None,
            auto_sync_running: false,
            auto_sync_status: None,
            auto_sync_rx: None,
            single_calib_running: false,
            single_calib_status: None,
            single_calib_rx: None,
            scan_tx,
            scan_rx,
            scan_in_flight: true,
            routing_in_flight: false,
            hotplug_dirty,
            ui_sync_needed,
            last_master_sync: Instant::now() - Duration::from_secs(10),
            last_speaker_sync: HashMap::new(),
            last_routing_poll: Instant::now(),
            toast: None,
            flow_phase: 0.0,
            last_frame_time: Instant::now(),
            pings: Vec::new(),
        }
    }

    fn set_toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toast = Some((msg.into(), is_error, Instant::now()));
    }

    fn trigger_sink_scan(&mut self, force: bool, ctx: &egui::Context) {
        if self.scan_in_flight {
            return;
        }
        self.scan_in_flight = true;
        let engine = Arc::clone(&self.engine);
        let tx = self.scan_tx.clone();
        let ctx = ctx.clone();
        thread::Builder::new()
            .name("polifonia-scan".to_string())
            .spawn(move || {
                let sinks = engine.get_available_sinks(force);
                let _ = tx.send(ScanResult::Sinks(sinks));
                let routing = engine.get_routing_status();
                let _ = tx.send(ScanResult::Routing(routing));
                ctx.request_repaint();
            })
            .ok();
    }

    fn trigger_routing_refresh(&mut self, ctx: &egui::Context) {
        if self.routing_in_flight {
            return;
        }
        self.routing_in_flight = true;
        self.last_routing_poll = Instant::now();
        let engine = Arc::clone(&self.engine);
        let tx = self.scan_tx.clone();
        let ctx = ctx.clone();
        thread::Builder::new()
            .name("polifonia-routing".to_string())
            .spawn(move || {
                let routing = engine.get_routing_status();
                let _ = tx.send(ScanResult::Routing(routing));
                ctx.request_repaint();
            })
            .ok();
    }

    fn reconcile_live_sinks(&mut self, sinks: Vec<AudioSink>) {
        self.live_sinks.clear();
        let mut changed = false;

        for sink in sinks {
            if sink.name.starts_with("polifonia_") {
                continue;
            }
            self.live_sinks.insert(sink.name.clone(), sink.clone());

            if let Some(existing) = self.config.speakers.get_mut(&sink.name) {
                if existing.sink_id != sink.id
                    || existing.display_name != sink.description
                    || (existing.hardware_latency_ms - sink.latency_ms).abs() > 0.5
                    || existing.bus_type != sink.bus_type
                    || existing.is_internal != sink.is_internal
                {
                    existing.sink_id = sink.id;
                    existing.display_name = sink.description.clone();
                    existing.hardware_latency_ms = sink.latency_ms;
                    existing.bus_type = sink.bus_type.clone();
                    existing.is_internal = sink.is_internal;
                    // Strictly preserve existing user role, gain, delay, polarity, and custom_name!
                    changed = true;
                }
            } else {
                let sp = SpeakerConfig::from_sink(&sink);
                self.config.speakers.insert(sink.name.clone(), sp);
                changed = true;
            }
        }

        if changed {
            self.engine.update_config(self.config.clone(), true);
            if self.engine.is_running() {
                let engine = Arc::clone(&self.engine);
                thread::spawn(move || engine.sync_active_branches(None));
            }
            self.tray.refresh();
        }
    }

    fn process_background_events(&mut self, ctx: &egui::Context) {
        if self.ui_sync_needed.swap(false, Ordering::Relaxed) {
            self.config = self.engine.config_snapshot();
            self.tray.refresh();
            self.trigger_routing_refresh(ctx);
        }

        while let Ok(res) = self.scan_rx.try_recv() {
            match res {
                ScanResult::Sinks(sinks) => {
                    self.scan_in_flight = false;
                    self.reconcile_live_sinks(sinks);
                }
                ScanResult::Routing(routing) => {
                    self.routing_in_flight = false;
                    self.routing_status = routing;
                }
            }
        }

        if self.hotplug_dirty.swap(false, Ordering::Relaxed) {
            self.trigger_sink_scan(true, ctx);
        }

        // Poll Auto-Sync results
        let mut auto_events = Vec::new();
        if let Some(rx) = &self.auto_sync_rx {
            while let Ok(ev) = rx.try_recv() {
                auto_events.push(ev);
            }
        }
        let mut clear_auto = false;
        for ev in auto_events {
            match ev {
                AutoSyncEvent::Progress(msg) => {
                    self.auto_sync_status = Some(msg);
                }
                AutoSyncEvent::Done(res) => {
                    self.auto_sync_running = false;
                    self.auto_sync_status = None;
                    clear_auto = true;
                    match res {
                        Ok((summary, _)) => {
                            self.config = self.engine.config_snapshot();
                            self.set_toast(summary, false);
                            self.tray.refresh();
                            self.trigger_routing_refresh(ctx);
                        }
                        Err(err) => {
                            self.set_toast(format!("Auto-Sync failed: {err}"), true);
                        }
                    }
                }
            }
        }
        if clear_auto {
            self.auto_sync_rx = None;
        }

        // Poll Single-Speaker Calibration results
        let mut single_events = Vec::new();
        if let Some(rx) = &self.single_calib_rx {
            while let Ok(ev) = rx.try_recv() {
                single_events.push(ev);
            }
        }
        let mut clear_single = false;
        for ev in single_events {
            match ev {
                SingleCalibEvent::Progress(msg) => {
                    self.single_calib_status = Some(msg);
                }
                SingleCalibEvent::Done(sink_name, res) => {
                    self.single_calib_running = false;
                    self.single_calib_status = None;
                    clear_single = true;
                    match res {
                        Ok(delay_ms) => {
                            if let Some(sp) = self.config.speakers.get_mut(&sink_name) {
                                sp.delay_ms = delay_ms;
                                self.commit_config_and_sync_branch(&sink_name);
                            }
                            self.set_toast(
                                format!("Measured acoustic offset: {delay_ms:+.1} ms"),
                                false,
                            );
                        }
                        Err(err) => {
                            self.set_toast(format!("Calibration error: {err}"), true);
                        }
                    }
                }
            }
        }
        if clear_single {
            self.single_calib_rx = None;
        }

        // Expire old toast after 4.5s
        if let Some((_, _, when)) = &self.toast {
            if when.elapsed() > Duration::from_millis(4500) {
                self.toast = None;
            }
        }

        // Prune completed test-tone pings
        self.pings
            .retain(|p| p.started_at.elapsed() < Duration::from_millis(950));
    }

    fn toggle_unison(&mut self, ctx: &egui::Context) {
        let engine = Arc::clone(&self.engine);
        let ctx = ctx.clone();
        let tray = self.tray.clone();
        let sync_flag = Arc::clone(&self.ui_sync_needed);

        if self.engine.is_running() || self.config.is_active {
            self.config.is_active = false;
            self.engine.update_config(self.config.clone(), true);
            thread::spawn(move || {
                engine.stop_unison_sink(false);
                sync_flag.store(true, Ordering::Relaxed);
                tray.refresh();
                ctx.request_repaint();
            });
            self.set_toast("Unison Engine moved to Standby", false);
        } else {
            self.config.is_active = true;
            let cfg_snap = self.config.clone();
            self.engine.update_config(cfg_snap.clone(), true);
            thread::spawn(move || {
                engine.activate_unison(Some(cfg_snap));
                sync_flag.store(true, Ordering::Relaxed);
                tray.refresh();
                ctx.request_repaint();
            });
            self.set_toast("Unison Engine starting...", false);
        }
        self.tray.refresh();
    }

    fn commit_config_and_sync_branch(&mut self, sink_name: &str) {
        self.engine.update_config(self.config.clone(), true);
        if self.engine.is_running() {
            let engine = Arc::clone(&self.engine);
            let sn = sink_name.to_string();
            let sync_flag = Arc::clone(&self.ui_sync_needed);
            thread::spawn(move || {
                engine.restart_single_branch(&sn);
                sync_flag.store(true, Ordering::Relaxed);
            });
        }
        self.tray.refresh();
    }

    fn commit_speaker_enable_change(&mut self) {
        self.engine.update_config(self.config.clone(), true);
        if self.engine.is_running() {
            let engine = Arc::clone(&self.engine);
            let sync_flag = Arc::clone(&self.ui_sync_needed);
            thread::spawn(move || {
                engine.sync_active_branches(None);
                sync_flag.store(true, Ordering::Relaxed);
            });
        }
        self.tray.refresh();
    }

    fn start_auto_sync(&mut self, ctx: &egui::Context) {
        if self.auto_sync_running {
            return;
        }
        self.auto_sync_running = true;
        self.auto_sync_status = Some("Initializing acoustic alignment...".to_string());
        let (tx, rx) = mpsc::channel();
        self.auto_sync_rx = Some(rx);

        let engine = Arc::clone(&self.engine);
        let ctx = ctx.clone();
        thread::Builder::new()
            .name("polifonia-autosync".to_string())
            .spawn(move || {
                let tx_prog = tx.clone();
                let ctx_prog = ctx.clone();
                let res = engine.auto_align_all_speakers(move |msg| {
                    let _ = tx_prog.send(AutoSyncEvent::Progress(msg.to_string()));
                    ctx_prog.request_repaint();
                });
                let _ = tx.send(AutoSyncEvent::Done(res));
                ctx.request_repaint();
            })
            .ok();
    }

    fn start_single_calibration(&mut self, sink_name: &str, ctx: &egui::Context) {
        if self.single_calib_running {
            return;
        }
        self.single_calib_running = true;
        self.single_calib_status = Some("Preparing chirp sweep...".to_string());
        let (tx, rx) = mpsc::channel();
        self.single_calib_rx = Some(rx);

        let engine = Arc::clone(&self.engine);
        let sn = sink_name.to_string();
        let ctx = ctx.clone();
        thread::Builder::new()
            .name("polifonia-single-cal".to_string())
            .spawn(move || {
                let tx_prog = tx.clone();
                let ctx_prog = ctx.clone();
                let res = engine.measure_acoustic_delay(&sn, move |msg| {
                    let _ = tx_prog.send(SingleCalibEvent::Progress(msg.to_string()));
                    ctx_prog.request_repaint();
                });
                let _ = tx.send(SingleCalibEvent::Done(sn, res));
                ctx.request_repaint();
            })
            .ok();
    }

    fn ordered_speakers(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .config
            .speakers
            .iter()
            .filter_map(|(k, sp)| {
                let is_live = self.live_sinks.contains_key(k);
                if is_live || sp.role.is_enabled() {
                    Some(k.clone())
                } else {
                    None
                }
            })
            .collect();

        names.sort_by(|a, b| {
            let sa = &self.config.speakers[a];
            let sb = &self.config.speakers[b];
            let live_a = self.live_sinks.contains_key(a);
            let live_b = self.live_sinks.contains_key(b);
            sb.role
                .is_enabled()
                .cmp(&sa.role.is_enabled())
                .then_with(|| live_b.cmp(&live_a))
                .then_with(|| sa.is_internal.cmp(&sb.is_internal))
                .then_with(|| sa.label().to_lowercase().cmp(&sb.label().to_lowercase()))
        });
        names
    }

    fn render_header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let unison_running = self.engine.is_running();
        let unison_desired = self.config.is_active;

        egui::Frame::none()
            .fill(COLOR_SURFACE)
            .inner_margin(egui::Margin::symmetric(18.0, 12.0))
            .stroke(egui::Stroke::new(1.0_f32, COLOR_BORDER))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Brand & Status
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("POLIFONIA")
                                    .size(19.0)
                                    .strong()
                                    .color(COLOR_TEXT),
                            );
                            ui.add_space(6.0);
                            let (badge_txt, badge_color) = if unison_running {
                                ("ACTIVE", COLOR_GREEN)
                            } else if unison_desired {
                                ("STARTING", COLOR_AMBER)
                            } else {
                                ("STANDBY", COLOR_MUTED)
                            };
                            status_pill(ui, badge_txt, badge_color);
                        });
                        ui.label(
                            egui::RichText::new(
                                "Multi-Output Acoustic Synchronization & DSP Matrix",
                            )
                            .size(11.5)
                            .color(COLOR_MUTED),
                        );
                    });

                    ui.add_space(20.0);

                    // View Switcher Tabs
                    ui.horizontal(|ui| {
                        let mixer_sel = self.active_tab == ActiveTab::Mixer;
                        if tab_button(ui, "Mixer Console", mixer_sel).clicked() {
                            self.active_tab = ActiveTab::Mixer;
                        }
                        let flow_sel = self.active_tab == ActiveTab::SignalFlow;
                        if tab_button(ui, "Signal Flow", flow_sel).clicked() {
                            self.active_tab = ActiveTab::SignalFlow;
                            self.trigger_routing_refresh(ctx);
                        }
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (btn_text, btn_fill, txt_col) = if unison_running || unison_desired {
                            ("DEACTIVATE", egui::Color32::from_rgb(185, 28, 28), COLOR_TEXT)
                        } else {
                            ("ACTIVATE UNISON", COLOR_CYAN, egui::Color32::from_rgb(9, 14, 24))
                        };
                        let toggle_btn = egui::Button::new(
                            egui::RichText::new(btn_text)
                                .size(13.0)
                                .strong()
                                .color(txt_col),
                        )
                        .fill(btn_fill)
                        .rounding(8.0)
                        .min_size(egui::vec2(148.0, 34.0));
                        if ui.add(toggle_btn).clicked() {
                            self.toggle_unison(ctx);
                        }

                        ui.add_space(8.0);
                        let rescan_label = if self.scan_in_flight {
                            "Scanning..."
                        } else {
                            "Rescan Devices"
                        };
                        if ui
                            .add_enabled(
                                !self.scan_in_flight,
                                egui::Button::new(rescan_label)
                                    .fill(COLOR_ELEVATED)
                                    .rounding(8.0)
                                    .min_size(egui::vec2(110.0, 32.0)),
                            )
                            .clicked()
                        {
                            self.trigger_sink_scan(true, ctx);
                            self.set_toast("Scanning PipeWire audio endpoints...", false);
                        }
                    });
                });
            });
    }

    fn render_master_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Frame::none()
            .fill(COLOR_ELEVATED)
            .rounding(12.0)
            .inner_margin(egui::Margin::symmetric(16.0, 12.0))
            .stroke(egui::Stroke::new(1.0_f32, COLOR_BORDER))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("MASTER GAIN")
                            .size(12.0)
                            .strong()
                            .color(COLOR_CYAN),
                    );
                    ui.add_space(6.0);

                    let mut master_pct = (self.config.master_volume * 100.0).round();
                    let slider = egui::Slider::new(&mut master_pct, 0.0..=150.0)
                        .show_value(false)
                        .step_by(1.0)
                        .trailing_fill(true);
                    let resp = ui.add_sized([230.0, 20.0], slider);
                    if resp.changed() {
                        let gain = (master_pct / 100.0).clamp(0.0, 1.5);
                        self.config.master_volume = gain;
                        if self.last_master_sync.elapsed() >= Duration::from_millis(55) {
                            self.last_master_sync = Instant::now();
                            let engine = Arc::clone(&self.engine);
                            thread::spawn(move || engine.set_master_gain(gain, false));
                        }
                    }
                    if resp.drag_stopped() {
                        let gain = (master_pct / 100.0).clamp(0.0, 1.5);
                        self.config.master_volume = gain;
                        let engine = Arc::clone(&self.engine);
                        let tray = self.tray.clone();
                        thread::spawn(move || {
                            engine.set_master_gain(gain, true);
                            tray.refresh();
                        });
                    }

                    ui.label(
                        egui::RichText::new(format!("{:.0}%", master_pct))
                            .size(13.5)
                            .strong()
                            .monospace()
                            .color(COLOR_TEXT),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !self.auto_sync_running,
                                egui::Button::new("Reset Delays")
                                    .fill(COLOR_SURFACE)
                                    .rounding(8.0)
                                    .min_size(egui::vec2(102.0, 30.0)),
                            )
                            .clicked()
                        {
                            for sp in self.config.speakers.values_mut() {
                                sp.delay_ms = 0.0;
                            }
                            let engine = Arc::clone(&self.engine);
                            let sync_flag = Arc::clone(&self.ui_sync_needed);
                            thread::spawn(move || {
                                engine.reset_all_delays();
                                sync_flag.store(true, Ordering::Relaxed);
                            });
                            self.set_toast("All speaker delays reset to 0.0 ms", false);
                        }

                        ui.add_space(8.0);
                        let sync_btn_txt = if self.auto_sync_running {
                            self.auto_sync_status
                                .as_deref()
                                .unwrap_or("Calibrating...")
                        } else {
                            "Auto-Sync Speakers"
                        };
                        if ui
                            .add_enabled(
                                !self.auto_sync_running,
                                egui::Button::new(
                                    egui::RichText::new(sync_btn_txt)
                                        .strong()
                                        .color(COLOR_CYAN),
                                )
                                .fill(egui::Color32::from_rgb(16, 40, 58))
                                .stroke(egui::Stroke::new(1.0_f32, COLOR_CYAN))
                                .rounding(8.0)
                                .min_size(egui::vec2(162.0, 30.0)),
                            )
                            .clicked()
                        {
                            self.start_auto_sync(ctx);
                        }
                    });
                });
            });
    }

    fn render_mixer_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.render_master_bar(ui, ctx);
        ui.add_space(12.0);

        let ordered_names = self.ordered_speakers();
        if ordered_names.is_empty() {
            egui::Frame::none()
                .fill(COLOR_ELEVATED)
                .rounding(12.0)
                .inner_margin(egui::Margin::same(32.0))
                .stroke(egui::Stroke::new(1.0_f32, COLOR_BORDER))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("No Audio Destinations Detected")
                                .size(16.0)
                                .strong()
                                .color(COLOR_TEXT),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(
                                "Connect an HDMI/DisplayPort display, Bluetooth speaker, or USB DAC and click Rescan Devices.",
                            )
                            .color(COLOR_MUTED),
                        );
                    });
                });
            return;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for sink_name in ordered_names {
                    self.render_speaker_card(ui, ctx, &sink_name);
                    ui.add_space(10.0);
                }
            });
    }

    fn render_speaker_card(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context, sink_name: &str) {
        let Some(sp) = self.config.speakers.get(sink_name).cloned() else {
            return;
        };
        let is_live = self.live_sinks.contains_key(sink_name);
        let enabled = sp.role.is_enabled();

        let border_col = if enabled && is_live {
            egui::Color32::from_rgb(38, 92, 130)
        } else if enabled {
            egui::Color32::from_rgb(140, 95, 25)
        } else {
            COLOR_BORDER
        };
        let card_bg = if enabled {
            COLOR_ELEVATED
        } else {
            egui::Color32::from_rgb(17, 22, 33)
        };

        egui::Frame::none()
            .fill(card_bg)
            .rounding(12.0)
            .inner_margin(egui::Margin::symmetric(16.0, 12.0))
            .stroke(egui::Stroke::new(1.0_f32, border_col))
            .show(ui, |ui| {
                // Header Row: Name + Bus Badge + Role Pills + Test + Enable Toggle
                ui.horizontal(|ui| {
                    // Status dot
                    let dot_col = if !is_live {
                        COLOR_AMBER
                    } else if enabled {
                        COLOR_GREEN
                    } else {
                        COLOR_MUTED
                    };
                    let (dot_rect, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter()
                        .circle_filled(dot_rect.center(), 4.5, dot_col);

                    // Name or Rename Editor
                    if self.rename_target.as_deref() == Some(sink_name) {
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.rename_buffer)
                                .desired_width(180.0),
                        );
                        let enter =
                            resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button("Save").clicked() || enter {
                            let trimmed = self.rename_buffer.trim().to_string();
                            if let Some(target) = self.config.speakers.get_mut(sink_name) {
                                target.custom_name = if trimmed.is_empty() {
                                    None
                                } else {
                                    Some(trimmed)
                                };
                            }
                            self.rename_target = None;
                            self.engine.update_config(self.config.clone(), true);
                            self.tray.refresh();
                        }
                        if ui.button("Cancel").clicked() {
                            self.rename_target = None;
                        }
                    } else {
                        ui.label(
                            egui::RichText::new(sp.label())
                                .size(15.0)
                                .strong()
                                .color(if enabled { COLOR_TEXT } else { COLOR_MUTED }),
                        );
                        if ui
                            .small_button("Edit")
                            .on_hover_text("Rename speaker")
                            .clicked()
                        {
                            self.rename_target = Some(sink_name.to_string());
                            self.rename_buffer = sp.label().to_string();
                        }
                    }

                    let bus_label = if sp.bus_type.is_empty() {
                        "OUT"
                    } else {
                        sp.bus_type.as_str()
                    };
                    badge_chip(ui, bus_label, COLOR_SURFACE, COLOR_CYAN);

                    if !is_live {
                        badge_chip(ui, "OFFLINE", COLOR_SURFACE, COLOR_AMBER);
                    }
                    if sp.is_internal {
                        badge_chip(ui, "INTERNAL", COLOR_SURFACE, COLOR_MUTED);
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Enable / Exclude Toggle
                        let enable_btn = egui::Button::new(
                            egui::RichText::new(if enabled { "ACTIVE" } else { "OFF" })
                                .size(11.5)
                                .strong()
                                .color(if enabled {
                                    egui::Color32::from_rgb(9, 14, 24)
                                } else {
                                    COLOR_MUTED
                                }),
                        )
                        .fill(if enabled { COLOR_GREEN } else { COLOR_SURFACE })
                        .rounding(8.0)
                        .min_size(egui::vec2(64.0, 26.0));
                        if ui.add(enable_btn).clicked() {
                            if let Some(target) = self.config.speakers.get_mut(sink_name) {
                                target.role = if enabled {
                                    SpeakerRole::Excluded
                                } else {
                                    target.effective_role()
                                };
                                target.normalize();
                            }
                            self.commit_speaker_enable_change();
                        }

                        ui.add_space(6.0);
                        // Test Tone button
                        if ui
                            .add(
                                egui::Button::new("Test")
                                    .fill(COLOR_SURFACE)
                                    .rounding(6.0)
                                    .min_size(egui::vec2(52.0, 26.0)),
                            )
                            .on_hover_text("Play 0.8s test tone")
                            .clicked()
                        {
                            let freq = if sp.role == SpeakerRole::Subwoofer {
                                80.0
                            } else {
                                440.0
                            };
                            let engine = Arc::clone(&self.engine);
                            let sn = sink_name.to_string();
                            thread::spawn(move || engine.play_test_tone(&sn, freq, 0.8));
                            self.pings.push(PingRing {
                                sink_name: sink_name.to_string(),
                                started_at: Instant::now(),
                            });
                        }

                        ui.add_space(8.0);
                        // Role Selector Pills: C, SUB, R, L, ST (right-to-left order!)
                        let role_pills = [
                            (SpeakerRole::Center, "C", "Center Channel"),
                            (SpeakerRole::Subwoofer, "SUB", "Subwoofer LFE"),
                            (SpeakerRole::Right, "R", "Right Channel Only"),
                            (SpeakerRole::Left, "L", "Left Channel Only"),
                            (SpeakerRole::Stereo, "ST", "Full Stereo (L+R)"),
                        ];
                        for (role_val, code, tip) in role_pills {
                            let is_sel = sp.role == role_val;
                            let pill = egui::Button::new(
                                egui::RichText::new(code)
                                    .size(11.0)
                                    .strong()
                                    .color(if is_sel {
                                        egui::Color32::from_rgb(9, 14, 24)
                                    } else {
                                        COLOR_MUTED
                                    }),
                            )
                            .fill(if is_sel { COLOR_CYAN } else { COLOR_SURFACE })
                            .rounding(6.0)
                            .min_size(egui::vec2(34.0, 24.0));
                            if ui.add(pill).on_hover_text(tip).clicked() {
                                if let Some(target) = self.config.speakers.get_mut(sink_name) {
                                    let was_enabled = target.role.is_enabled();
                                    target.role = role_val;
                                    target.normalize();
                                    if was_enabled {
                                        self.commit_config_and_sync_branch(sink_name);
                                    } else {
                                        self.commit_speaker_enable_change();
                                    }
                                }
                            }
                        }
                    });
                });

                // Subtitle row
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&sp.display_name)
                            .size(11.0)
                            .color(COLOR_MUTED),
                    );
                    if sp.hardware_latency_ms > 0.0 || sp.phase_inverted {
                        ui.label(
                            egui::RichText::new(format!(
                                "  |  HW Latency ~{:.1} ms{}",
                                sp.hardware_latency_ms,
                                if sp.phase_inverted { "  |  180 deg Inverted" } else { "" }
                            ))
                            .size(11.0)
                            .color(COLOR_MUTED),
                        );
                    }
                });

                ui.add_space(8.0);

                // Controls Row: Volume Slider + Mute + Delay Controls + Tune Modal
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Gain")
                            .size(12.0)
                            .color(COLOR_MUTED),
                    );
                    let mut vol_pct = (sp.volume_gain * 100.0).round();
                    let vol_slider = egui::Slider::new(&mut vol_pct, 0.0..=150.0)
                        .show_value(false)
                        .step_by(1.0)
                        .trailing_fill(true);
                    let resp = ui.add_sized([180.0, 18.0], vol_slider);
                    if resp.changed() {
                        let gain = (vol_pct / 100.0).clamp(0.0, 1.5);
                        if let Some(target) = self.config.speakers.get_mut(sink_name) {
                            target.volume_gain = gain;
                        }
                        let should_send = self
                            .last_speaker_sync
                            .get(sink_name)
                            .map(|t| t.elapsed() >= Duration::from_millis(55))
                            .unwrap_or(true);
                        if should_send {
                            self.last_speaker_sync
                                .insert(sink_name.to_string(), Instant::now());
                            let engine = Arc::clone(&self.engine);
                            let sn = sink_name.to_string();
                            thread::spawn(move || engine.set_speaker_gain(&sn, gain, false));
                        }
                    }
                    if resp.drag_stopped() {
                        let gain = (vol_pct / 100.0).clamp(0.0, 1.5);
                        if let Some(target) = self.config.speakers.get_mut(sink_name) {
                            target.volume_gain = gain;
                        }
                        let engine = Arc::clone(&self.engine);
                        let sn = sink_name.to_string();
                        let tray = self.tray.clone();
                        thread::spawn(move || {
                            engine.set_speaker_gain(&sn, gain, true);
                            tray.refresh();
                        });
                    }

                    ui.label(
                        egui::RichText::new(format!("{:.0}%", vol_pct))
                            .size(12.5)
                            .strong()
                            .monospace()
                            .color(COLOR_TEXT),
                    );

                    let mute_btn = egui::Button::new(
                        egui::RichText::new(if sp.mute { "MUTED" } else { "Mute" })
                            .size(11.0)
                            .color(if sp.mute { COLOR_TEXT } else { COLOR_MUTED }),
                    )
                    .fill(if sp.mute {
                        egui::Color32::from_rgb(185, 28, 28)
                    } else {
                        COLOR_SURFACE
                    })
                    .rounding(6.0);
                    if ui.add(mute_btn).clicked() {
                        let new_mute = !sp.mute;
                        if let Some(target) = self.config.speakers.get_mut(sink_name) {
                            target.mute = new_mute;
                        }
                        let engine = Arc::clone(&self.engine);
                        let sn = sink_name.to_string();
                        thread::spawn(move || engine.set_speaker_mute(&sn, new_mute));
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new("Tune / Calibrate")
                                    .fill(COLOR_SURFACE)
                                    .rounding(6.0),
                            )
                            .clicked()
                        {
                            self.calib_modal_sink = Some(sink_name.to_string());
                        }

                        ui.add_space(6.0);
                        if ui.small_button("0").on_hover_text("Reset delay to 0 ms").clicked() {
                            if let Some(target) = self.config.speakers.get_mut(sink_name) {
                                target.delay_ms = 0.0;
                            }
                            self.commit_config_and_sync_branch(sink_name);
                        }
                        for delta in [10.0f64, 2.0f64] {
                            if ui.small_button(format!("+{}", delta as i32)).clicked() {
                                if let Some(target) = self.config.speakers.get_mut(sink_name) {
                                    target.delay_ms = (target.delay_ms + delta).clamp(-300.0, 300.0);
                                }
                                self.commit_config_and_sync_branch(sink_name);
                            }
                        }

                        ui.label(
                            egui::RichText::new(format!("{:+.1} ms", sp.delay_ms))
                                .size(12.5)
                                .strong()
                                .monospace()
                                .color(if sp.delay_ms.abs() > 0.05 {
                                    COLOR_CYAN
                                } else {
                                    COLOR_MUTED
                                }),
                        );

                        for delta in [-2.0f64, -10.0f64] {
                            if ui.small_button(format!("{}", delta as i32)).clicked() {
                                if let Some(target) = self.config.speakers.get_mut(sink_name) {
                                    target.delay_ms = (target.delay_ms + delta).clamp(-300.0, 300.0);
                                }
                                self.commit_config_and_sync_branch(sink_name);
                            }
                        }
                        ui.label(
                            egui::RichText::new("Delay")
                                .size(12.0)
                                .color(COLOR_MUTED),
                        );
                    });
                });
            });
    }

    fn render_calibration_modal(&mut self, ctx: &egui::Context) {
        let Some(sink_name) = self.calib_modal_sink.clone() else {
            return;
        };
        let Some(sp) = self.config.speakers.get(&sink_name).cloned() else {
            self.calib_modal_sink = None;
            return;
        };

        let mut open = true;
        egui::Window::new(format!("Acoustic Tuning - {}", sp.label()))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(&sp.display_name)
                        .size(12.0)
                        .color(COLOR_MUTED),
                );
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    ui.label("Manual Delay Offset (ms):");
                    let mut delay = sp.delay_ms;
                    let slider = egui::Slider::new(&mut delay, -300.0..=300.0).step_by(0.5);
                    let resp = ui.add(slider);
                    if resp.changed() {
                        if let Some(target) = self.config.speakers.get_mut(&sink_name) {
                            target.delay_ms = (delay * 2.0).round() / 2.0;
                        }
                    }
                    if resp.drag_stopped() {
                        self.commit_config_and_sync_branch(&sink_name);
                    }
                });

                ui.add_space(6.0);
                let mut inverted = sp.phase_inverted;
                if ui
                    .checkbox(&mut inverted, "Invert Acoustic Polarity (180 degrees)")
                    .changed()
                {
                    if let Some(target) = self.config.speakers.get_mut(&sink_name) {
                        target.phase_inverted = inverted;
                    }
                    self.commit_config_and_sync_branch(&sink_name);
                }

                ui.add_space(10.0);
                let auto_comp = self
                    .routing_status
                    .destinations
                    .iter()
                    .find(|d| d.sink_name == sink_name)
                    .map(|d| (d.auto_comp_delay_ms, d.total_delay_ms));
                egui::Frame::none()
                    .fill(COLOR_SURFACE)
                    .rounding(8.0)
                    .inner_margin(egui::Margin::same(10.0))
                    .show(ui, |ui| {
                        ui.label(format!("Measured HW Buffer Latency: {:.1} ms", sp.hardware_latency_ms));
                        if let Some((auto_ms, total_ms)) = auto_comp {
                            ui.label(format!("Auto Unison Compensation: {:.1} ms", auto_ms));
                            ui.label(
                                egui::RichText::new(format!("Effective Branch Delay: {:.1} ms", total_ms))
                                    .strong()
                                    .color(COLOR_CYAN),
                            );
                        }
                    });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let btn_label = if self.single_calib_running {
                        self.single_calib_status
                            .as_deref()
                            .unwrap_or("Measuring...")
                    } else {
                        "Measure Relative Delay via Mic"
                    };
                    if ui
                        .add_enabled(
                            !self.single_calib_running,
                            egui::Button::new(btn_label).fill(egui::Color32::from_rgb(16, 40, 58)),
                        )
                        .clicked()
                    {
                        self.start_single_calibration(&sink_name, ctx);
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Apply & Close").clicked() {
                            self.commit_config_and_sync_branch(&sink_name);
                            self.calib_modal_sink = None;
                        }
                    });
                });
            });

        if !open {
            self.calib_modal_sink = None;
        }
    }

    fn render_signal_flow_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // Periodic background routing refresh while Signal Flow tab is visible (every 1.5s)
        if self.last_routing_poll.elapsed() >= Duration::from_millis(1500) {
            self.trigger_routing_refresh(ctx);
        }

        let status = self.routing_status.clone();

        // Telemetry Header Pills
        egui::Frame::none()
            .fill(COLOR_ELEVATED)
            .rounding(10.0)
            .inner_margin(egui::Margin::symmetric(14.0, 10.0))
            .stroke(egui::Stroke::new(1.0_f32, COLOR_BORDER))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let engine_col = if status.is_unison_active {
                        COLOR_GREEN
                    } else {
                        COLOR_MUTED
                    };
                    status_pill(
                        ui,
                        if status.is_unison_active {
                            "ENGINE: ACTIVE"
                        } else {
                            "ENGINE: STANDBY"
                        },
                        engine_col,
                    );
                    badge_chip(
                        ui,
                        &format!("MASTER {:.0}%", status.master_volume * 100.0),
                        COLOR_SURFACE,
                        COLOR_CYAN,
                    );
                    badge_chip(
                        ui,
                        &format!("SOURCES: {}", status.sources.len()),
                        COLOR_SURFACE,
                        COLOR_TEXT,
                    );
                    let enabled_cnt = status.destinations.iter().filter(|d| d.is_enabled).count();
                    badge_chip(
                        ui,
                        &format!("OUTPUTS: {}/{}", enabled_cnt, status.destinations.len()),
                        COLOR_SURFACE,
                        COLOR_TEXT,
                    );
                    badge_chip(
                        ui,
                        &format!("SYNC BASELINE: {:.1} ms", status.max_hw_latency),
                        COLOR_SURFACE,
                        COLOR_AMBER,
                    );
                });
            });

        ui.add_space(10.0);

        let avail = ui.available_size();
        let canvas_size = egui::vec2(avail.x.max(400.0), avail.y.max(260.0));
        let (rect, response) = ui.allocate_exact_size(canvas_size, egui::Sense::click());
        let painter = ui.painter_at(rect);

        // Dark grid canvas background
        painter.rect_filled(rect, 12.0, egui::Color32::from_rgb(10, 14, 22));
        painter.rect_stroke(rect, 12.0, egui::Stroke::new(1.0_f32, COLOR_BORDER));

        let step = 32.0;
        let grid_col = egui::Color32::from_rgba_unmultiplied(148, 163, 184, 16);
        let mut gx = rect.left() + step;
        while gx < rect.right() {
            painter.line_segment(
                [egui::pos2(gx, rect.top() + 8.0), egui::pos2(gx, rect.bottom() - 8.0)],
                egui::Stroke::new(1.0_f32, grid_col),
            );
            gx += step;
        }
        let mut gy = rect.top() + step;
        while gy < rect.bottom() {
            painter.line_segment(
                [egui::pos2(rect.left() + 8.0, gy), egui::pos2(rect.right() - 8.0, gy)],
                egui::Stroke::new(1.0_f32, grid_col),
            );
            gy += step;
        }

        // Column headers
        let col1_x = rect.left() + rect.width() * 0.16;
        let hub_x = rect.center().x;
        let col3_x = rect.left() + rect.width() * 0.84;

        painter.text(
            egui::pos2(col1_x, rect.top() + 20.0),
            egui::Align2::CENTER_CENTER,
            "AUDIO SOURCES",
            egui::FontId::proportional(11.5),
            COLOR_MUTED,
        );
        painter.text(
            egui::pos2(hub_x, rect.top() + 20.0),
            egui::Align2::CENTER_CENTER,
            "POLIFONIA DSP MATRIX",
            egui::FontId::proportional(11.5),
            COLOR_CYAN,
        );
        painter.text(
            egui::pos2(col3_x, rect.top() + 20.0),
            egui::Align2::CENTER_CENTER,
            "AUDIO DESTINATIONS",
            egui::FontId::proportional(11.5),
            COLOR_MUTED,
        );

        let body_top = rect.top() + 44.0;
        let body_bottom = rect.bottom() - 18.0;
        let body_h = (body_bottom - body_top).max(120.0);

        // Center Hub Node
        let hub_rect = egui::Rect::from_center_size(
            egui::pos2(hub_x, body_top + body_h * 0.5),
            egui::vec2(210.0, 96.0),
        );

        // Layout Destination nodes (right column)
        let mut live_dests = status.destinations.clone();
        live_dests.sort_by(|a, b| {
            b.is_enabled
                .cmp(&a.is_enabled)
                .then_with(|| a.display_name.cmp(&b.display_name))
        });

        let dest_count = live_dests.len().max(1);
        let dest_slot_h = (body_h / dest_count as f32).clamp(48.0, 82.0);
        let total_dest_h = dest_slot_h * live_dests.len() as f32;
        let dest_start_y = body_top + (body_h - total_dest_h) * 0.5 + dest_slot_h * 0.5;
        let dest_w = (rect.width() * 0.25).clamp(180.0, 260.0);
        let dest_h = (dest_slot_h - 10.0).clamp(42.0, 64.0);

        // Layout Source nodes (left column)
        let src_count = status.sources.len().max(1);
        let src_slot_h = (body_h / src_count as f32).clamp(50.0, 82.0);
        let total_src_h = src_slot_h * status.sources.len() as f32;
        let src_start_y = body_top + (body_h - total_src_h) * 0.5 + src_slot_h * 0.5;
        let src_w = (rect.width() * 0.23).clamp(165.0, 235.0);
        let src_h = (src_slot_h - 10.0).clamp(42.0, 62.0);

        let mut click_pos = if response.clicked() {
            response.interact_pointer_pos()
        } else {
            None
        };

        // Draw Source -> Hub bezier lines & Source cards
        if status.sources.is_empty() {
            let empty_rect = egui::Rect::from_center_size(
                egui::pos2(col1_x, body_top + body_h * 0.5),
                egui::vec2(src_w, 54.0),
            );
            draw_bezier(
                &painter,
                empty_rect.right_center(),
                hub_rect.left_center(),
                COLOR_BORDER,
                1.5,
                None,
            );
            painter.rect_filled(empty_rect, 9.0, COLOR_ELEVATED);
            painter.rect_stroke(empty_rect, 9.0, egui::Stroke::new(1.0_f32, COLOR_BORDER));
            painter.text(
                empty_rect.center(),
                egui::Align2::CENTER_CENTER,
                "No active audio streams",
                egui::FontId::proportional(12.0),
                COLOR_MUTED,
            );
        } else {
            for (idx, src) in status.sources.iter().enumerate() {
                let cy = src_start_y + idx as f32 * src_slot_h;
                let s_rect = egui::Rect::from_center_size(
                    egui::pos2(col1_x, cy),
                    egui::vec2(src_w, src_h),
                );
                let line_col = if src.is_active && status.is_unison_active {
                    COLOR_CYAN
                } else if src.is_active {
                    COLOR_GREEN
                } else {
                    COLOR_BORDER
                };
                let packet = if src.is_active && !src.muted && status.is_unison_active {
                    Some((self.flow_phase + idx as f32 * 0.23) % 1.0)
                } else {
                    None
                };
                draw_bezier(
                    &painter,
                    s_rect.right_center(),
                    hub_rect.left_center(),
                    line_col,
                    if src.is_active { 2.2 } else { 1.4 },
                    packet,
                );

                painter.rect_filled(s_rect, 9.0, COLOR_ELEVATED);
                painter.rect_stroke(s_rect, 9.0, egui::Stroke::new(1.0_f32, line_col));
                painter.circle_filled(
                    egui::pos2(s_rect.left() + 14.0, s_rect.center().y),
                    4.0,
                    if src.is_active { COLOR_GREEN } else { COLOR_MUTED },
                );
                painter.text(
                    egui::pos2(s_rect.left() + 26.0, s_rect.center().y - 7.0),
                    egui::Align2::LEFT_CENTER,
                    truncate_label(&src.app_name, 22),
                    egui::FontId::proportional(12.5),
                    COLOR_TEXT,
                );
                let sub = if src.muted {
                    "MUTED".to_string()
                } else {
                    format!("{}% • {}", src.volume_pct, truncate_label(&src.media_name, 16))
                };
                painter.text(
                    egui::pos2(s_rect.left() + 26.0, s_rect.center().y + 9.0),
                    egui::Align2::LEFT_CENTER,
                    sub,
                    egui::FontId::proportional(10.5),
                    COLOR_MUTED,
                );
            }
        }

        // Draw Hub -> Destination bezier lines & Destination cards
        let mut clicked_sink: Option<String> = None;
        for (idx, dest) in live_dests.iter().enumerate() {
            let cy = dest_start_y + idx as f32 * dest_slot_h;
            let d_rect = egui::Rect::from_center_size(
                egui::pos2(col3_x, cy),
                egui::vec2(dest_w, dest_h),
            );
            let line_col = if !dest.is_enabled {
                COLOR_BORDER
            } else if status.is_unison_active && dest.is_streaming {
                COLOR_CYAN
            } else {
                COLOR_GREEN
            };
            let packet = if status.is_unison_active && dest.is_enabled {
                Some((self.flow_phase + idx as f32 * 0.17) % 1.0)
            } else {
                None
            };
            draw_bezier(
                &painter,
                hub_rect.right_center(),
                d_rect.left_center(),
                line_col,
                if dest.is_enabled { 2.4 } else { 1.2 },
                packet,
            );

            // Ping ring animation if this node was recently tested
            for ping in &self.pings {
                if ping.sink_name == dest.sink_name {
                    let frac = (ping.started_at.elapsed().as_secs_f32() / 0.95).clamp(0.0, 1.0);
                    let r = 16.0 + frac * 42.0;
                    let alpha = ((1.0 - frac) * 200.0) as u8;
                    painter.circle_stroke(
                        d_rect.center(),
                        r,
                        egui::Stroke::new(2.0_f32, egui::Color32::from_rgba_unmultiplied(56, 189, 248, alpha)),
                    );
                }
            }

            let d_fill = if dest.is_enabled {
                COLOR_ELEVATED
            } else {
                egui::Color32::from_rgb(16, 21, 31)
            };
            painter.rect_filled(d_rect, 9.0, d_fill);
            painter.rect_stroke(d_rect, 9.0, egui::Stroke::new(1.2_f32, line_col));

            let led_col = if !dest.is_enabled {
                COLOR_MUTED
            } else if dest.is_streaming {
                COLOR_GREEN
            } else {
                COLOR_CYAN
            };
            painter.circle_filled(
                egui::pos2(d_rect.left() + 13.0, d_rect.center().y),
                4.5,
                led_col,
            );

            let label = self
                .config
                .speakers
                .get(&dest.sink_name)
                .map(|sp| sp.label().to_string())
                .unwrap_or_else(|| dest.display_name.clone());
            painter.text(
                egui::pos2(d_rect.left() + 25.0, d_rect.center().y - 8.0),
                egui::Align2::LEFT_CENTER,
                truncate_label(&label, 22),
                egui::FontId::proportional(12.5),
                if dest.is_enabled { COLOR_TEXT } else { COLOR_MUTED },
            );

            let meta = if dest.is_enabled {
                format!(
                    "{} | {:.0}% | {:+.1} ms{}",
                    dest.role.short_label(),
                    dest.volume_gain * 100.0,
                    dest.total_delay_ms,
                    if dest.phase_inverted { " | 180°" } else { "" }
                )
            } else {
                format!("{} | EXCLUDED", dest.bus_type)
            };
            painter.text(
                egui::pos2(d_rect.left() + 25.0, d_rect.center().y + 9.0),
                egui::Align2::LEFT_CENTER,
                meta,
                egui::FontId::proportional(10.5),
                COLOR_MUTED,
            );

            let tip = format!(
                "{}\nSink #{} ({})\nState: {}\nHW Buffer: {:.1} ms | User Delay: {:+.1} ms\nAuto Unison Comp: {:.1} ms | Effective: {:.1} ms{}\nClick to play test tone",
                label,
                dest.sink_id,
                dest.bus_type,
                dest.state,
                dest.hardware_latency_ms,
                dest.delay_ms,
                dest.auto_comp_delay_ms,
                dest.total_delay_ms,
                if dest.phase_inverted { "\nPolarity: 180 deg inverted" } else { "" }
            );
            let node_resp = ui.interact(
                d_rect,
                ui.id().with(("flow_dest", &dest.sink_name)),
                egui::Sense::click(),
            );
            let clicked_node = node_resp.clicked();
            node_resp.on_hover_text(tip);

            if clicked_node {
                clicked_sink = Some(dest.sink_name.clone());
                click_pos = None;
            } else if let Some(pos) = click_pos {
                if d_rect.contains(pos) {
                    clicked_sink = Some(dest.sink_name.clone());
                    click_pos = None;
                }
            }
        }

        // Draw Center Hub Node (click toggles Unison)
        let hub_border = if status.is_unison_active {
            COLOR_CYAN
        } else {
            COLOR_BORDER
        };
        painter.rect_filled(hub_rect, 14.0, COLOR_SURFACE);
        painter.rect_stroke(hub_rect, 14.0, egui::Stroke::new(2.0_f32, hub_border));
        painter.text(
            egui::pos2(hub_rect.center().x, hub_rect.center().y - 20.0),
            egui::Align2::CENTER_CENTER,
            "POLIFONIA UNISON",
            egui::FontId::proportional(14.0),
            COLOR_TEXT,
        );
        painter.text(
            egui::pos2(hub_rect.center().x, hub_rect.center().y + 2.0),
            egui::Align2::CENTER_CENTER,
            if status.is_unison_active {
                format!("{} • MASTER {:.0}%", status.master_state, status.master_volume * 100.0)
            } else {
                "STANDBY (Click to Activate)".to_string()
            },
            egui::FontId::proportional(11.5),
            if status.is_unison_active {
                COLOR_GREEN
            } else {
                COLOR_MUTED
            },
        );
        painter.text(
            egui::pos2(hub_rect.center().x, hub_rect.center().y + 24.0),
            egui::Align2::CENTER_CENTER,
            "Click Destination Node to Test",
            egui::FontId::proportional(10.0),
            COLOR_MUTED,
        );

        if let Some(pos) = click_pos {
            if hub_rect.contains(pos) {
                self.toggle_unison(ctx);
            }
        }
        if let Some(sn) = clicked_sink {
            let freq = self
                .config
                .speakers
                .get(&sn)
                .map(|sp| {
                    if sp.role == SpeakerRole::Subwoofer {
                        80.0
                    } else {
                        440.0
                    }
                })
                .unwrap_or(440.0);
            let engine = Arc::clone(&self.engine);
            let sn_bg = sn.clone();
            thread::spawn(move || engine.play_test_tone(&sn_bg, freq, 0.8));
            self.pings.push(PingRing {
                sink_name: sn,
                started_at: Instant::now(),
            });
        }
    }

    fn render_footer(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(COLOR_SURFACE)
            .inner_margin(egui::Margin::symmetric(16.0, 8.0))
            .stroke(egui::Stroke::new(1.0_f32, COLOR_BORDER))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let mut set_default = self.config.set_as_default;
                    if ui
                        .checkbox(&mut set_default, "Set Polifonia as default system output")
                        .changed()
                    {
                        self.config.set_as_default = set_default;
                        self.engine.update_config(self.config.clone(), true);
                        if set_default {
                            let engine = Arc::clone(&self.engine);
                            thread::spawn(move || engine.reassert_master_default());
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some((msg, is_err, _)) = &self.toast {
                            ui.label(
                                egui::RichText::new(msg)
                                    .size(12.0)
                                    .strong()
                                    .color(if *is_err { COLOR_RED } else { COLOR_GREEN }),
                            );
                        } else {
                            ui.label(
                                egui::RichText::new("Closing window minimizes to System Tray")
                                    .size(11.5)
                                    .color(COLOR_MUTED),
                            );
                        }
                    });
                });
            });
    }
}

impl eframe::App for PolifoniaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame_time).as_secs_f32().min(0.1);
        self.last_frame_time = now;

        // Close-to-tray behavior unless an explicit Quit came from tray or signal.
        if ctx.input(|i| i.viewport().close_requested())
            && !self.quit_requested.load(Ordering::Relaxed)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.window_visible.store(false, Ordering::Relaxed);
            self.tray.refresh();
            return;
        }

        if self.quit_requested.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        self.process_background_events(ctx);

        // Throttled 30 FPS time-based animation ONLY while SignalFlow is visible & active.
        let animate_flow = self.active_tab == ActiveTab::SignalFlow
            && (self.routing_status.is_unison_active || !self.pings.is_empty());
        if animate_flow {
            self.flow_phase = (self.flow_phase + dt * 0.45) % 1.0;
            ctx.request_repaint_after(Duration::from_millis(33));
        }

        egui::TopBottomPanel::top("polifonia_header")
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                self.render_header(ui, ctx);
            });

        egui::TopBottomPanel::bottom("polifonia_footer")
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                self.render_footer(ui);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(COLOR_BG)
                    .inner_margin(egui::Margin::symmetric(16.0, 14.0)),
            )
            .show(ctx, |ui| match self.active_tab {
                ActiveTab::Mixer => self.render_mixer_tab(ui, ctx),
                ActiveTab::SignalFlow => self.render_signal_flow_tab(ui, ctx),
            });

        self.render_calibration_modal(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.engine.stop_unison_sink(true);
    }
}

fn spawn_hotplug_watcher(ctx: egui::Context, hotplug_dirty: Arc<AtomicBool>) {
    thread::Builder::new()
        .name("polifonia-hotplug".to_string())
        .spawn(move || loop {
            let child = Command::new("pactl")
                .args(["subscribe"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();

            let Ok(mut proc) = child else {
                thread::sleep(Duration::from_secs(3));
                continue;
            };

            if let Some(stdout) = proc.stdout.take() {
                let reader = BufReader::new(stdout);
                let mut last_trigger = Instant::now() - Duration::from_secs(5);
                for line in reader.lines().map_while(Result::ok) {
                    if line.contains("on sink") || line.contains("on card") || line.contains("on server") {
                        if last_trigger.elapsed() >= Duration::from_millis(450) {
                            last_trigger = Instant::now();
                            hotplug_dirty.store(true, Ordering::Relaxed);
                            ctx.request_repaint();
                        }
                    }
                }
            }

            let _ = proc.wait();
            thread::sleep(Duration::from_secs(2));
        })
        .ok();
}

const COLOR_BG: egui::Color32 = egui::Color32::from_rgb(11, 15, 23);
const COLOR_SURFACE: egui::Color32 = egui::Color32::from_rgb(20, 27, 41);
const COLOR_ELEVATED: egui::Color32 = egui::Color32::from_rgb(26, 35, 52);
const COLOR_BORDER: egui::Color32 = egui::Color32::from_rgb(39, 52, 74);
const COLOR_CYAN: egui::Color32 = egui::Color32::from_rgb(56, 189, 248);
const COLOR_GREEN: egui::Color32 = egui::Color32::from_rgb(34, 197, 94);
const COLOR_AMBER: egui::Color32 = egui::Color32::from_rgb(245, 158, 11);
const COLOR_RED: egui::Color32 = egui::Color32::from_rgb(239, 68, 68);
const COLOR_TEXT: egui::Color32 = egui::Color32::from_rgb(248, 250, 252);
const COLOR_MUTED: egui::Color32 = egui::Color32::from_rgb(148, 163, 184);

fn apply_studio_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.window_fill = COLOR_BG;
    style.visuals.panel_fill = COLOR_BG;
    style.visuals.extreme_bg_color = egui::Color32::from_rgb(8, 11, 18);
    style.visuals.widgets.noninteractive.bg_fill = COLOR_SURFACE;
    style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, COLOR_TEXT);
    style.visuals.widgets.inactive.bg_fill = COLOR_SURFACE;
    style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, COLOR_TEXT);
    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(36, 48, 70);
    style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(44, 60, 88);
    style.visuals.selection.bg_fill = COLOR_CYAN;
    style.visuals.selection.stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(9, 14, 24));
    ctx.set_style(style);
}

fn tab_button(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let btn = egui::Button::new(
        egui::RichText::new(label)
            .size(12.5)
            .strong()
            .color(if selected {
                egui::Color32::from_rgb(9, 14, 24)
            } else {
                COLOR_TEXT
            }),
    )
    .fill(if selected { COLOR_CYAN } else { COLOR_ELEVATED })
    .stroke(egui::Stroke::new(1.0_f32, if selected { COLOR_CYAN } else { COLOR_BORDER }))
    .rounding(8.0)
    .min_size(egui::vec2(108.0, 30.0));
    ui.add(btn)
}

fn status_pill(ui: &mut egui::Ui, label: &str, color: egui::Color32) {
    egui::Frame::none()
        .fill(egui::Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            28,
        ))
        .rounding(99.0)
        .stroke(egui::Stroke::new(1.0_f32, color))
        .inner_margin(egui::Margin::symmetric(8.0, 2.0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(label).size(10.5).strong().color(color));
        });
}

fn badge_chip(
    ui: &mut egui::Ui,
    text: &str,
    bg: egui::Color32,
    fg: egui::Color32,
) {
    egui::Frame::none()
        .fill(bg)
        .rounding(6.0)
        .stroke(egui::Stroke::new(1.0_f32, COLOR_BORDER))
        .inner_margin(egui::Margin::symmetric(7.0, 2.0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(10.5).strong().color(fg));
        });
}

fn truncate_label(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= max_chars.saturating_sub(1) {
            out.push_str("..");
            return out;
        }
        out.push(ch);
    }
    out
}

fn draw_bezier(
    painter: &egui::Painter,
    p0: egui::Pos2,
    p3: egui::Pos2,
    color: egui::Color32,
    width: f32,
    packet_t: Option<f32>,
) {
    let dx = ((p3.x - p0.x).abs() * 0.48).max(32.0);
    let p1 = egui::pos2(p0.x + dx, p0.y);
    let p2 = egui::pos2(p3.x - dx, p3.y);

    let bezier = egui::epaint::CubicBezierShape::from_points_stroke(
        [p0, p1, p2, p3],
        false,
        egui::Color32::TRANSPARENT,
        egui::Stroke::new(width, color),
    );
    painter.add(bezier);

    if let Some(t) = packet_t {
        let u = 1.0 - t;
        let pos = egui::pos2(
            u * u * u * p0.x + 3.0 * u * u * t * p1.x + 3.0 * u * t * t * p2.x + t * t * t * p3.x,
            u * u * u * p0.y + 3.0 * u * u * t * p1.y + 3.0 * u * t * t * p2.y + t * t * t * p3.y,
        );
        painter.circle_filled(
            pos,
            6.5,
            egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 70),
        );
        painter.circle_filled(pos, 3.2, COLOR_TEXT);
    }
}
