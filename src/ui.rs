//! Polifonia Audio Studio — Matte Carbon & Studio Copper DAW Console UI.
//!
//! Runs inside an ephemeral `--ui-window` subprocess communicating with the
//! background Polifonia daemon over a local Unix domain socket.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use egui::epaint::RectShape;
use egui::{
    Align, Align2, Color32, CursorIcon, Frame, Id, Key, Layout, Margin, Pos2, Rect, ResizeDirection,
    RichText, Rounding, ScrollArea, Sense, Stroke, TextEdit, TextureHandle, Ui, Vec2,
    ViewportCommand,
};

use crate::engine::RoutingStatus;
use crate::icons::{self, UiIcon};
use crate::ipc::{
    self, BranchSyncMode, CalibOutcomeDto, DaemonStateSnapshot, IpcRequest, IpcResponse,
};
use crate::models::{AudioSink, SpeakerConfig, SpeakerRole, SystemConfig};
use crate::theme::{
    self, badge_chip, font_bold, font_mono, font_mono_bold, font_regular, gain_to_db_str,
    horizontal_studio_slider, paint_crisp_box, pill_switch, status_led_pill, studio_pad_button,
    tint_opaque, vertical_studio_fader, PadStyle, AMBER, BG_CONTROL, BG_CONTROL_HOVER, BG_HEADER,
    BG_MASTER_STRIP, BG_MODAL, BG_RACK, BG_STRIP, BG_STRIP_EXCLUDED, BG_WELL, BG_WINDOW,
    BORDER_STRONG, BORDER_SUBTLE, COPPER, COPPER_GLOW, EMERALD, ROSE, TEXT_MUTED, TEXT_PRIMARY,
    TEXT_SECONDARY,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveTab {
    Mixer,
    SignalFlow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RackFilter {
    All,
    ActiveOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StripDensity {
    Standard,
    Compact,
}

enum UiAsyncMsg {
    Sinks(Vec<AudioSink>),
    State(Box<DaemonStateSnapshot>),
    ConfigUpdated(SystemConfig),
    UnisonUpdated {
        is_running: bool,
        config: SystemConfig,
    },
    ErrorToast(String),
}

pub struct PolifoniaApp {
    brand_tex: TextureHandle,
    config: SystemConfig,
    is_running: bool,
    available_sinks: Vec<AudioSink>,
    initial_scan_complete: bool,
    routing_status: RoutingStatus,
    last_seen_hotplug_epoch: u64,
    last_calib_seq: u64,

    // Async channel fed by daemon IPC responses and background state poller
    async_tx: mpsc::Sender<UiAsyncMsg>,
    async_rx: mpsc::Receiver<UiAsyncMsg>,
    is_scanning: bool,

    // Acoustic calibration state mirrored from daemon
    calib_progress: Option<(f32, String)>,
    is_calibrating_all: bool,
    calibrating_single_sink: Option<String>,

    // Unison async toggle indicator
    unison_toggling: bool,

    // Debounced gain sync & remote-poll suppression during local interactions
    last_master_sync: Instant,
    last_speaker_sync: HashMap<String, Instant>,
    suppress_remote_config_until: Instant,

    // Transient UI interactions
    active_tab: ActiveTab,
    rack_filter: RackFilter,
    strip_density: StripDensity,
    tuning_modal_sink: Option<String>,
    editing_alias_sink: Option<String>,
    editing_alias_buf: String,
    remembered_roles: HashMap<String, SpeakerRole>,
    test_tone_until: HashMap<String, Instant>,
    status_toast: Option<(String, bool, Instant)>,

    // Pre-mute recall level for master quick mute
    saved_master_pre_mute: f64,
}

impl PolifoniaApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_studio_fonts(&cc.egui_ctx);
        theme::apply_studio_theme(&cc.egui_ctx);

        let brand_tex = icons::load_brand_texture(&cc.egui_ctx);

        let initial_snap = match ipc::send_request(
            &IpcRequest::GetState {
                include_routing: true,
            },
            Duration::from_millis(900),
        ) {
            Ok(IpcResponse::State(snap)) => Some(*snap),
            _ => None,
        };

        let (config, is_running, routing_status, last_seen_hotplug_epoch, last_calib_seq, calib) =
            match initial_snap {
                Some(snap) => {
                    let rt = snap.routing_status.unwrap_or_else(|| RoutingStatus {
                        is_unison_active: snap.is_running,
                        master_volume: snap.config.master_volume,
                        master_state: if snap.is_running {
                            "ACTIVE".to_string()
                        } else {
                            "STANDBY".to_string()
                        },
                        sources: Vec::new(),
                        destinations: Vec::new(),
                        max_hw_latency: 0.0,
                    });
                    (
                        snap.config,
                        snap.is_running,
                        rt,
                        snap.hotplug_epoch,
                        snap.calib.finished_seq,
                        snap.calib,
                    )
                }
                None => (
                    SystemConfig::default(),
                    false,
                    RoutingStatus {
                        is_unison_active: false,
                        master_volume: 1.0,
                        master_state: "STANDBY".to_string(),
                        sources: Vec::new(),
                        destinations: Vec::new(),
                        max_hw_latency: 0.0,
                    },
                    0,
                    0,
                    Default::default(),
                ),
            };

        let mut remembered_roles = HashMap::new();
        for (name, sp) in &config.speakers {
            if sp.role != SpeakerRole::Excluded {
                remembered_roles.insert(name.clone(), sp.role);
            }
        }

        let (async_tx, async_rx) = mpsc::channel();

        // Initial background hardware discovery for this window session
        {
            let tx = async_tx.clone();
            let ctx = cc.egui_ctx.clone();
            thread::Builder::new()
                .name("polifonia-ui-init-scan".into())
                .spawn(move || {
                    match ipc::send_request(&IpcRequest::ScanSinks { force: false }, Duration::from_secs(4)) {
                        Ok(IpcResponse::Sinks(sinks)) => {
                            let _ = tx.send(UiAsyncMsg::Sinks(sinks));
                        }
                        Ok(IpcResponse::Error(err)) => {
                            let _ = tx.send(UiAsyncMsg::ErrorToast(err));
                        }
                        _ => {}
                    }
                    ctx.request_repaint();
                })
                .ok();
        }

        // Background daemon state poller (exits window cleanly if daemon goes away)
        {
            let tx = async_tx.clone();
            let ctx = cc.egui_ctx.clone();
            thread::Builder::new()
                .name("polifonia-ui-poller".into())
                .spawn(move || {
                    let mut failures = 0u32;
                    let mut calibrating = false;
                    loop {
                        let interval_ms = if calibrating { 140 } else { 450 };
                        thread::sleep(Duration::from_millis(interval_ms));
                        match ipc::send_request(
                            &IpcRequest::GetState {
                                include_routing: true,
                            },
                            Duration::from_secs(2),
                        ) {
                            Ok(IpcResponse::State(snap)) => {
                                failures = 0;
                                calibrating = snap.calib.is_calibrating_all
                                    || snap.calib.calibrating_single_sink.is_some();
                                if tx.send(UiAsyncMsg::State(snap)).is_err() {
                                    break;
                                }
                                ctx.request_repaint();
                            }
                            Ok(_) => {
                                failures = 0;
                            }
                            Err(_) => {
                                failures += 1;
                                if failures >= 4 {
                                    std::process::exit(0);
                                }
                            }
                        }
                    }
                })
                .ok();
        }

        Self {
            brand_tex,
            config,
            is_running,
            available_sinks: Vec::new(),
            initial_scan_complete: false,
            routing_status,
            last_seen_hotplug_epoch,
            last_calib_seq,
            async_tx,
            async_rx,
            is_scanning: true,
            calib_progress: calib.progress,
            is_calibrating_all: calib.is_calibrating_all,
            calibrating_single_sink: calib.calibrating_single_sink,
            unison_toggling: false,
            last_master_sync: Instant::now() - Duration::from_secs(1),
            last_speaker_sync: HashMap::new(),
            suppress_remote_config_until: Instant::now(),
            active_tab: ActiveTab::Mixer,
            rack_filter: RackFilter::All,
            strip_density: StripDensity::Standard,
            tuning_modal_sink: None,
            editing_alias_sink: None,
            editing_alias_buf: String::new(),
            remembered_roles,
            test_tone_until: HashMap::new(),
            status_toast: None,
            saved_master_pre_mute: 1.0,
        }
    }

    fn dispatch_ipc(&self, req: IpcRequest, ctx: &egui::Context, timeout: Duration) {
        let tx = self.async_tx.clone();
        let ctx_bg = ctx.clone();
        thread::Builder::new()
            .name("polifonia-ui-ipc".into())
            .spawn(move || {
                match ipc::send_request(&req, timeout) {
                    Ok(IpcResponse::Ack) => {}
                    Ok(IpcResponse::State(snap)) => {
                        let _ = tx.send(UiAsyncMsg::State(snap));
                    }
                    Ok(IpcResponse::Sinks(sinks)) => {
                        let _ = tx.send(UiAsyncMsg::Sinks(sinks));
                    }
                    Ok(IpcResponse::Config(cfg)) => {
                        let _ = tx.send(UiAsyncMsg::ConfigUpdated(cfg));
                    }
                    Ok(IpcResponse::UnisonState { is_running, config }) => {
                        let _ = tx.send(UiAsyncMsg::UnisonUpdated { is_running, config });
                    }
                    Ok(IpcResponse::Error(err)) => {
                        let _ = tx.send(UiAsyncMsg::ErrorToast(err));
                    }
                    Err(err) => {
                        let _ = tx.send(UiAsyncMsg::ErrorToast(err));
                    }
                }
                ctx_bg.request_repaint();
            })
            .ok();
    }

    fn trigger_scan(&mut self, force: bool, ctx: &egui::Context) {
        if self.is_scanning {
            return;
        }
        self.is_scanning = true;
        self.dispatch_ipc(IpcRequest::ScanSinks { force }, ctx, Duration::from_secs(5));
    }

    fn close_to_tray(&self) {
        let _ = ipc::send_request(
            &IpcRequest::NotifyWindowClosing,
            Duration::from_millis(120),
        );
        std::process::exit(0);
    }

    fn note_local_edit(&mut self) {
        self.suppress_remote_config_until = Instant::now() + Duration::from_millis(750);
    }

    fn sync_remembered_roles_from_config(&mut self) {
        for (name, sp) in &self.config.speakers {
            if sp.role != SpeakerRole::Excluded {
                self.remembered_roles.insert(name.clone(), sp.role);
            }
        }
    }

    fn poll_background_events(&mut self, ctx: &egui::Context) {
        let mut msgs = Vec::new();
        while let Ok(m) = self.async_rx.try_recv() {
            msgs.push(m);
        }

        let is_dragging = ctx.dragged_id().is_some();

        for msg in msgs {
            match msg {
                UiAsyncMsg::Sinks(sinks) => {
                    self.is_scanning = false;
                    self.reconcile_sinks(sinks, ctx);
                }
                UiAsyncMsg::State(snap) => {
                    if !self.unison_toggling {
                        self.is_running = snap.is_running;
                    }
                    if let Some(rt) = snap.routing_status {
                        self.routing_status = rt;
                    }
                    if snap.hotplug_epoch > self.last_seen_hotplug_epoch {
                        self.last_seen_hotplug_epoch = snap.hotplug_epoch;
                        self.trigger_scan(true, ctx);
                    }

                    self.is_calibrating_all = snap.calib.is_calibrating_all;
                    self.calibrating_single_sink = snap.calib.calibrating_single_sink;
                    self.calib_progress = snap.calib.progress;

                    let calib_just_finished = snap.calib.finished_seq > self.last_calib_seq;
                    if calib_just_finished {
                        self.last_calib_seq = snap.calib.finished_seq;
                        self.config = snap.config;
                        self.sync_remembered_roles_from_config();
                        if let Some(outcome) = snap.calib.last_outcome {
                            match outcome {
                                CalibOutcomeDto::AutoAll(res) => match res {
                                    Ok(msg) => self.push_toast(msg, false),
                                    Err(err) => self.push_toast(err, true),
                                },
                                CalibOutcomeDto::Single { result, .. } => match result {
                                    Ok(delay_ms) => self.push_toast(
                                        format!("Measured acoustic offset: {delay_ms:+.1} ms"),
                                        false,
                                    ),
                                    Err(err) => {
                                        self.push_toast(format!("Calibration error: {err}"), true)
                                    }
                                },
                            }
                        }
                    } else if !self.unison_toggling
                        && self.editing_alias_sink.is_none()
                        && !is_dragging
                        && Instant::now() >= self.suppress_remote_config_until
                    {
                        self.config = snap.config;
                        self.sync_remembered_roles_from_config();
                    }
                }
                UiAsyncMsg::ConfigUpdated(cfg) => {
                    if self.editing_alias_sink.is_none()
                        && !is_dragging
                        && Instant::now() >= self.suppress_remote_config_until
                    {
                        self.config = cfg;
                        self.sync_remembered_roles_from_config();
                    }
                }
                UiAsyncMsg::UnisonUpdated { is_running, config } => {
                    self.unison_toggling = false;
                    self.is_running = is_running;
                    self.config = config;
                    self.sync_remembered_roles_from_config();
                }
                UiAsyncMsg::ErrorToast(err) => {
                    self.is_scanning = false;
                    self.unison_toggling = false;
                    self.push_toast(err, true);
                }
            }
        }

        if self.is_calibrating_all || self.calibrating_single_sink.is_some() {
            ctx.request_repaint_after(Duration::from_millis(80));
        }

        // Expire transient toast and test tone highlights cleanly
        let now = Instant::now();
        if let Some((_, _, exp)) = &self.status_toast {
            if now >= *exp {
                self.status_toast = None;
            }
        }
        self.test_tone_until.retain(|_, &mut until| until > now);
        if !self.test_tone_until.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }

    fn reconcile_sinks(&mut self, sinks: Vec<AudioSink>, ctx: &egui::Context) {
        self.initial_scan_complete = true;
        let mut changed = false;
        for sink in &sinks {
            if sink.name.starts_with("polifonia_") {
                continue;
            }
            let entry = self
                .config
                .speakers
                .entry(sink.name.clone())
                .or_insert_with(|| {
                    changed = true;
                    SpeakerConfig::from_sink(sink)
                });

            if entry.sink_id != sink.id
                || entry.display_name != sink.description
                || (entry.hardware_latency_ms - sink.latency_ms).abs() > 0.05
                || entry.bus_type != sink.bus_type
                || entry.is_internal != sink.is_internal
            {
                entry.sink_id = sink.id;
                entry.display_name = sink.description.clone();
                entry.hardware_latency_ms = sink.latency_ms;
                entry.bus_type = sink.bus_type.clone();
                entry.is_internal = sink.is_internal;
                changed = true;
            }
            entry.normalize();
        }

        if changed {
            self.note_local_edit();
            self.dispatch_ipc(
                IpcRequest::ReconcileConfig {
                    config: self.config.clone(),
                    sync_sinks: if self.is_running {
                        Some(sinks.clone())
                    } else {
                        None
                    },
                },
                ctx,
                Duration::from_secs(4),
            );
        }
        self.available_sinks = sinks;
    }

    fn push_toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.status_toast = Some((msg.into(), is_error, Instant::now() + Duration::from_secs(5)));
    }

    fn toggle_unison(&mut self, ctx: &egui::Context) {
        let currently_active = self.is_running || self.config.is_active;
        if !currently_active && !self.config.speakers.values().any(|s| s.role.is_enabled()) {
            self.push_toast(
                "Enable at least one output channel before starting Unison",
                true,
            );
            return;
        }
        if self.unison_toggling {
            return;
        }
        self.unison_toggling = true;
        let next_active = !currently_active;
        self.config.is_active = next_active;
        self.note_local_edit();

        self.dispatch_ipc(
            IpcRequest::SetUnison {
                active: next_active,
                config: self.config.clone(),
            },
            ctx,
            Duration::from_secs(12),
        );
    }

    fn start_auto_align_all(&mut self, ctx: &egui::Context) {
        if self.is_calibrating_all || self.calibrating_single_sink.is_some() {
            return;
        }
        self.is_calibrating_all = true;
        self.calib_progress = Some((0.02, "Initializing microphone calibration sweep…".to_string()));
        self.dispatch_ipc(IpcRequest::StartAutoAlignAll, ctx, Duration::from_secs(4));
    }

    fn start_single_calibration(&mut self, sink_name: String, ctx: &egui::Context) {
        if self.is_calibrating_all || self.calibrating_single_sink.is_some() {
            return;
        }
        self.calibrating_single_sink = Some(sink_name.clone());
        self.calib_progress = Some((
            0.05,
            format!("Measuring acoustic propagation for {sink_name}…"),
        ));
        self.dispatch_ipc(
            IpcRequest::StartSingleCalib { sink_name },
            ctx,
            Duration::from_secs(4),
        );
    }

    fn commit_speaker_config_change(
        &mut self,
        sink_name: &str,
        restart_branch: bool,
        ctx: &egui::Context,
    ) {
        self.note_local_edit();
        let sync_mode = if !self.is_running {
            BranchSyncMode::None
        } else if restart_branch {
            BranchSyncMode::RestartSingle
        } else {
            BranchSyncMode::SyncAll
        };
        self.dispatch_ipc(
            IpcRequest::SaveSpeakerChange {
                config: self.config.clone(),
                sink_name: sink_name.to_string(),
                sync_mode,
            },
            ctx,
            Duration::from_secs(5),
        );
    }

    fn toggle_speaker_enabled(&mut self, sink_name: &str, ctx: &egui::Context) {
        let current_role = self
            .config
            .speakers
            .get(sink_name)
            .map(|s| s.role)
            .unwrap_or(SpeakerRole::Excluded);

        if current_role == SpeakerRole::Excluded {
            let restore = self
                .remembered_roles
                .get(sink_name)
                .copied()
                .filter(|r| *r != SpeakerRole::Excluded)
                .unwrap_or_else(|| {
                    self.config
                        .speakers
                        .get(sink_name)
                        .map_or(SpeakerRole::Stereo, |s| s.effective_role())
                });
            if let Some(sp) = self.config.speakers.get_mut(sink_name) {
                sp.role = restore;
                sp.normalize();
            }
        } else {
            self.remembered_roles
                .insert(sink_name.to_string(), current_role);
            if let Some(sp) = self.config.speakers.get_mut(sink_name) {
                sp.role = SpeakerRole::Excluded;
                sp.normalize();
            }
        }
        self.commit_speaker_config_change(sink_name, false, ctx);
    }

    fn set_speaker_role(&mut self, sink_name: &str, role: SpeakerRole, ctx: &egui::Context) {
        if role != SpeakerRole::Excluded {
            self.remembered_roles.insert(sink_name.to_string(), role);
        }
        if let Some(sp) = self.config.speakers.get_mut(sink_name) {
            if sp.role == role {
                return;
            }
            sp.role = role;
            sp.normalize();
        }
        self.commit_speaker_config_change(sink_name, true, ctx);
    }

    fn sorted_speaker_keys(&self) -> Vec<String> {
        let online_set: HashSet<&str> = self
            .available_sinks
            .iter()
            .map(|s| s.name.as_str())
            .collect();

        let mut keys: Vec<String> = self
            .config
            .speakers
            .iter()
            .filter_map(|(k, sp)| {
                if sp.role.is_enabled()
                    || !self.initial_scan_complete
                    || online_set.contains(sp.sink_name.as_str())
                {
                    Some(k.clone())
                } else {
                    None
                }
            })
            .collect();

        keys.sort_by(|a, b| {
            let sa = &self.config.speakers[a];
            let sb = &self.config.speakers[b];
            let a_on = online_set.is_empty() || online_set.contains(sa.sink_name.as_str());
            let b_on = online_set.is_empty() || online_set.contains(sb.sink_name.as_str());
            let a_en = sa.role.is_enabled();
            let b_en = sb.role.is_enabled();

            b_en.cmp(&a_en)
                .then_with(|| b_on.cmp(&a_on))
                .then_with(|| role_sort_order(sa.role).cmp(&role_sort_order(sb.role)))
                .then_with(|| sa.label().to_lowercase().cmp(&sb.label().to_lowercase()))
        });
        keys
    }

    fn install_resize_grips(&self, ctx: &egui::Context, screen_rect: Rect) {
        let is_maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        if is_maximized {
            return;
        }

        let edge = 6.0;
        let corner = 12.0;
        let grips: [(Rect, ResizeDirection, CursorIcon); 8] = [
            (
                Rect::from_min_max(screen_rect.min, screen_rect.min + Vec2::splat(corner)),
                ResizeDirection::NorthWest,
                CursorIcon::ResizeNwSe,
            ),
            (
                Rect::from_min_max(
                    Pos2::new(screen_rect.max.x - corner, screen_rect.min.y),
                    Pos2::new(screen_rect.max.x, screen_rect.min.y + corner),
                ),
                ResizeDirection::NorthEast,
                CursorIcon::ResizeNeSw,
            ),
            (
                Rect::from_min_max(
                    Pos2::new(screen_rect.min.x, screen_rect.max.y - corner),
                    Pos2::new(screen_rect.min.x + corner, screen_rect.max.y),
                ),
                ResizeDirection::SouthWest,
                CursorIcon::ResizeNeSw,
            ),
            (
                Rect::from_min_max(screen_rect.max - Vec2::splat(corner), screen_rect.max),
                ResizeDirection::SouthEast,
                CursorIcon::ResizeNwSe,
            ),
            (
                Rect::from_min_max(
                    Pos2::new(screen_rect.min.x + corner, screen_rect.min.y),
                    Pos2::new(screen_rect.max.x - corner, screen_rect.min.y + edge),
                ),
                ResizeDirection::North,
                CursorIcon::ResizeVertical,
            ),
            (
                Rect::from_min_max(
                    Pos2::new(screen_rect.min.x + corner, screen_rect.max.y - edge),
                    Pos2::new(screen_rect.max.x - corner, screen_rect.max.y),
                ),
                ResizeDirection::South,
                CursorIcon::ResizeVertical,
            ),
            (
                Rect::from_min_max(
                    Pos2::new(screen_rect.min.x, screen_rect.min.y + corner),
                    Pos2::new(screen_rect.min.x + edge, screen_rect.max.y - corner),
                ),
                ResizeDirection::West,
                CursorIcon::ResizeHorizontal,
            ),
            (
                Rect::from_min_max(
                    Pos2::new(screen_rect.max.x - edge, screen_rect.min.y + corner),
                    Pos2::new(screen_rect.max.x, screen_rect.max.y - corner),
                ),
                ResizeDirection::East,
                CursorIcon::ResizeHorizontal,
            ),
        ];

        for (idx, (rect, dir, cursor)) in grips.into_iter().enumerate() {
            egui::Area::new(Id::new(("win_resize_grip", idx)))
                .order(egui::Order::Foreground)
                .fixed_pos(rect.min)
                .show(ctx, |ui| {
                    let (_, resp) = ui.allocate_exact_size(rect.size(), Sense::drag());
                    if resp.hovered() {
                        ctx.set_cursor_icon(cursor);
                    }
                    if resp.drag_started() {
                        ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
                    }
                });
        }
    }

    fn draw_header_bar(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let height = 56.0;
        let (bar_rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());

        // Bottom hairline divider
        ui.painter().line_segment(
            [
                Pos2::new(bar_rect.left(), bar_rect.bottom()),
                Pos2::new(bar_rect.right(), bar_rect.bottom()),
            ],
            Stroke::new(1.0_f32, BORDER_SUBTLE),
        );

        let is_running = self.is_running;
        let active_count = self
            .config
            .speakers
            .values()
            .filter(|s| s.role.is_enabled())
            .count();
        let is_max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));

        let inner = bar_rect.shrink2(Vec2::new(14.0, 8.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(
            Layout::left_to_right(Align::Center),
        ));

        // Full-color Polyphonic Fader Matrix emblem matching GNOME dock & tray 1:1
        let (logo_rect, _) = child.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
        icons::paint_brand_emblem(child.painter(), logo_rect, &self.brand_tex);

        child.add_space(2.0);
        child.vertical(|v| {
            v.spacing_mut().item_spacing.y = 0.0;
            v.label(RichText::new("POLIFONIA").font(font_bold(14.5)).color(TEXT_PRIMARY));
            v.label(
                RichText::new("AUDIO STUDIO")
                    .font(font_mono_bold(9.5))
                    .color(if is_running { COPPER_GLOW } else { TEXT_MUTED }),
            );
        });

        child.add_space(14.0);

        // Segmented view switcher inside dark capsule
        let tabs_rect = crisp_frame(
            &mut child,
            10.0,
            Margin::symmetric(3.0, 3.0),
            BG_WELL,
            Some(BORDER_SUBTLE),
            |seg| {
                seg.horizontal(|h| {
                    h.spacing_mut().item_spacing.x = 3.0;
                    if draw_header_tab(
                        h,
                        UiIcon::Mixer,
                        "Console Mixer",
                        self.active_tab == ActiveTab::Mixer,
                    )
                    .clicked()
                    {
                        self.active_tab = ActiveTab::Mixer;
                    }
                    if draw_header_tab(
                        h,
                        UiIcon::SignalFlow,
                        "Signal Flow",
                        self.active_tab == ActiveTab::SignalFlow,
                    )
                    .clicked()
                    {
                        self.active_tab = ActiveTab::SignalFlow;
                    }
                });
            },
        )
        .response
        .rect;

        // Right cluster
        let mut right_cluster_left_x = inner.right();
        let mut close_clicked = false;
        child.with_layout(Layout::right_to_left(Align::Center), |r| {
            r.spacing_mut().item_spacing.x = 7.0;

            // Window controls (right-to-left order -> Close first, then Maximize, then Minimize)
            if window_control_btn(r, UiIcon::WinClose, true).clicked() {
                close_clicked = true;
            }
            if window_control_btn(
                r,
                if is_max {
                    UiIcon::WinRestore
                } else {
                    UiIcon::WinMaximize
                },
                false,
            )
            .clicked()
            {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(!is_max));
            }
            if window_control_btn(r, UiIcon::WinMinimize, false).clicked() {
                ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
            }

            // Hairline separator
            let (sep_rect, _) = r.allocate_exact_size(Vec2::new(6.0, 22.0), Sense::hover());
            r.painter().line_segment(
                [
                    Pos2::new(sep_rect.center().x, sep_rect.top()),
                    Pos2::new(sep_rect.center().x, sep_rect.bottom()),
                ],
                Stroke::new(1.0_f32, BORDER_SUBTLE),
            );

            // Unison Engine CTA
            let unison_label = if self.unison_toggling {
                "SWITCHING…"
            } else if is_running {
                "UNISON LIVE"
            } else {
                "ENABLE UNISON"
            };
            let unison_style = if is_running {
                PadStyle::CopperPrimary
            } else {
                PadStyle::CopperTinted
            };
            if studio_pad_button(
                r,
                Some(UiIcon::UnisonPower),
                unison_label,
                Vec2::new(124.0, 31.0),
                unison_style,
            )
            .clicked()
            {
                self.toggle_unison(ctx);
            }

            // Auto-Sync Acoustic button
            let sync_label = if self.is_calibrating_all {
                "CALIBRATING…"
            } else {
                "Auto-Sync"
            };
            let sync_style = if self.is_calibrating_all {
                PadStyle::AmberWarn
            } else {
                PadStyle::Neutral
            };
            if studio_pad_button(
                r,
                Some(UiIcon::AutoSyncMic),
                sync_label,
                Vec2::new(102.0, 31.0),
                sync_style,
            )
            .on_hover_text("Measure acoustic arrival times with microphone and auto-align speakers")
            .clicked()
            {
                self.start_auto_align_all(ctx);
            }

            // Engine LED pill (100% opaque pre-blended fill + border -> zero edge halo)
            if is_running {
                status_led_pill(
                    r,
                    EMERALD,
                    &format!("LIVE · {active_count} OUT"),
                    TEXT_PRIMARY,
                    tint_opaque(BG_HEADER, EMERALD, 34),
                    tint_opaque(BG_HEADER, EMERALD, 115),
                );
            } else {
                status_led_pill(
                    r,
                    TEXT_MUTED,
                    &format!("STANDBY · {active_count} ARMED"),
                    TEXT_SECONDARY,
                    BG_WELL,
                    BORDER_SUBTLE,
                );
            }
            right_cluster_left_x = r.min_rect().left();
        });

        if close_clicked {
            self.close_to_tray();
        }

        // Non-overlapping drag / double-click zones over brand area and middle empty header space
        let drag_left = Rect::from_min_max(
            bar_rect.min,
            Pos2::new((tabs_rect.left() - 4.0).max(bar_rect.left()), bar_rect.bottom()),
        );
        let drag_mid = Rect::from_min_max(
            Pos2::new(tabs_rect.right() + 4.0, bar_rect.top()),
            Pos2::new(
                (right_cluster_left_x - 4.0).max(tabs_rect.right() + 4.0),
                bar_rect.bottom(),
            ),
        );
        for (id_salt, drag_rect) in [("header_drag_left", drag_left), ("header_drag_mid", drag_mid)]
        {
            if drag_rect.width() > 2.0 {
                let drag_resp = ui.interact(drag_rect, Id::new(id_salt), Sense::click_and_drag());
                if drag_resp.drag_started() {
                    ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                }
                if drag_resp.double_clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Maximized(!is_max));
                }
            }
        }
    }

    fn draw_mixer_view(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        // Optional calibration progress strip banner
        if let Some((frac, msg)) = self.calib_progress.clone() {
            crisp_frame(
                ui,
                10.0,
                Margin::symmetric(14.0, 9.0),
                Color32::from_rgb(32, 24, 18),
                Some(tint_opaque(BG_WINDOW, COPPER, 130)),
                |bar| {
                    bar.horizontal(|h| {
                        h.label(
                            RichText::new("ACOUSTIC ALIGNMENT")
                                .font(font_mono_bold(10.5))
                                .color(COPPER_GLOW),
                        );
                        h.label(RichText::new(msg).font(font_regular(12.5)).color(TEXT_PRIMARY));
                        h.with_layout(Layout::right_to_left(Align::Center), |r| {
                            r.label(
                                RichText::new(format!("{:.0}%", frac * 100.0))
                                    .font(font_mono_bold(11.5))
                                    .color(COPPER_GLOW),
                            );
                        });
                    });
                    bar.add_space(4.0);
                    let (prog_rect, _) =
                        bar.allocate_exact_size(Vec2::new(bar.available_width(), 5.0), Sense::hover());
                    bar.painter()
                        .rect_filled(prog_rect, Rounding::same(2.5), BG_WELL);
                    let fill_rect = Rect::from_min_max(
                        prog_rect.min,
                        Pos2::new(
                            prog_rect.left() + prog_rect.width() * frac.clamp(0.0, 1.0),
                            prog_rect.bottom(),
                        ),
                    );
                    bar.painter()
                        .rect_filled(fill_rect, Rounding::same(2.5), COPPER);
                },
            );
            ui.add_space(6.0);
        }

        // Compute counts and ordered device keys
        let ordered_keys = self.sorted_speaker_keys();
        let total_devices = ordered_keys.len();
        let active_devices = ordered_keys
            .iter()
            .filter(|k| self.config.speakers[*k].role.is_enabled())
            .count();

        // Rack sub-toolbar
        crisp_frame(
            ui,
            10.0,
            Margin::symmetric(10.0, 6.0),
            Color32::from_rgb(21, 21, 24),
            Some(BORDER_SUBTLE),
            |tb| {
                tb.horizontal(|h| {
                    h.spacing_mut().item_spacing.x = 6.0;

                    let all_style = if self.rack_filter == RackFilter::All {
                        PadStyle::CopperTinted
                    } else {
                        PadStyle::Neutral
                    };
                    if studio_pad_button(
                        h,
                        None,
                        &format!("All Channels ({total_devices})"),
                        Vec2::new(112.0, 26.0),
                        all_style,
                    )
                    .clicked()
                    {
                        self.rack_filter = RackFilter::All;
                    }

                    let act_style = if self.rack_filter == RackFilter::ActiveOnly {
                        PadStyle::CopperTinted
                    } else {
                        PadStyle::Neutral
                    };
                    if studio_pad_button(
                        h,
                        None,
                        &format!("Active Only ({active_devices})"),
                        Vec2::new(108.0, 26.0),
                        act_style,
                    )
                    .clicked()
                    {
                        self.rack_filter = RackFilter::ActiveOnly;
                    }

                    let density_label = match self.strip_density {
                        StripDensity::Standard => "Standard",
                        StripDensity::Compact => "Compact",
                    };
                    if studio_pad_button(
                        h,
                        Some(UiIcon::StripDensity),
                        density_label,
                        Vec2::new(88.0, 26.0),
                        PadStyle::Neutral,
                    )
                    .on_hover_text("Toggle between full fader bays and high-density multi-device strips")
                    .clicked()
                    {
                        self.strip_density = match self.strip_density {
                            StripDensity::Standard => StripDensity::Compact,
                            StripDensity::Compact => StripDensity::Standard,
                        };
                    }

                    if studio_pad_button(
                        h,
                        Some(UiIcon::Refresh),
                        if self.is_scanning { "Scanning…" } else { "Scan" },
                        Vec2::new(72.0, 26.0),
                        PadStyle::Neutral,
                    )
                    .clicked()
                    {
                        self.trigger_scan(true, ctx);
                    }

                    h.with_layout(Layout::right_to_left(Align::Center), |r| {
                        let hw = self.routing_status.max_hw_latency;
                        if hw > 0.05 {
                            badge_chip(
                                r,
                                &format!("BASELINE HW {hw:.1} ms"),
                                BG_WELL,
                                TEXT_SECONDARY,
                                BORDER_SUBTLE,
                            );
                        }
                        let stream_count = self
                            .routing_status
                            .sources
                            .iter()
                            .filter(|s| s.is_active)
                            .count();
                        if stream_count > 0 {
                            badge_chip(
                                r,
                                &format!(
                                    "{stream_count} ACTIVE STREAM{}",
                                    if stream_count == 1 { "" } else { "S" }
                                ),
                                tint_opaque(BG_HEADER, EMERALD, 34),
                                EMERALD,
                                tint_opaque(BG_HEADER, EMERALD, 110),
                            );
                        }
                    });
                });
            },
        );

        ui.add_space(8.0);

        let visible_keys: Vec<String> = ordered_keys
            .iter()
            .filter(|k| {
                self.rack_filter == RackFilter::All || self.config.speakers[*k].role.is_enabled()
            })
            .cloned()
            .collect();
        let hidden_excluded: Vec<String> = ordered_keys
            .iter()
            .filter(|k| {
                self.rack_filter == RackFilter::ActiveOnly
                    && !self.config.speakers[*k].role.is_enabled()
            })
            .cloned()
            .collect();

        // Main DAW console body: scrollable left channel rack + pinned right Master Bus strip
        let total_avail = ui.available_size();
        let master_w = match self.strip_density {
            StripDensity::Standard => 182.0,
            StripDensity::Compact => 164.0,
        };
        let rack_w = (total_avail.x - master_w - 10.0).max(260.0);
        let console_h = (total_avail.y - if hidden_excluded.is_empty() { 0.0 } else { 42.0 }).max(260.0);

        let mut any_fader_hovered = false;

        ui.horizontal_top(|row| {
            row.spacing_mut().item_spacing.x = 10.0;

            // Channel strip rack well
            let (rack_outer_rect, rack_resp) =
                row.allocate_exact_size(Vec2::new(rack_w, console_h), Sense::hover());

            paint_crisp_box(row.painter(), rack_outer_rect, 12.0, BG_RACK, Some(BORDER_SUBTLE));

            let rack_inner = rack_outer_rect.shrink2(Vec2::new(10.0, 10.0));
            let mut rack_ui = row.new_child(egui::UiBuilder::new().max_rect(rack_inner));

            // Wheel-to-horizontal scroll when cursor is inside rack but NOT hovering a vertical fader
            let scroll_delta_y = ctx.input(|i| i.raw_scroll_delta.y);
            let scroll_delta_x = ctx.input(|i| i.raw_scroll_delta.x);

            let strip_gap = 10.0;
            let count = visible_keys.len().max(1) as f32;
            let base_w = match self.strip_density {
                StripDensity::Standard => 210.0,
                StripDensity::Compact => 148.0,
            };
            let max_w = match self.strip_density {
                StripDensity::Standard => 246.0,
                StripDensity::Compact => 176.0,
            };
            let avail_for_cards = rack_inner.width();
            let elastic_w = ((avail_for_cards - strip_gap * (count - 1.0)) / count)
                .clamp(base_w, max_w);
            let card_h = (rack_inner.height() - 12.0).max(240.0);

            ScrollArea::horizontal()
                .id_salt("polifonia_channel_rack_scroll")
                .auto_shrink([false, false])
                .show(&mut rack_ui, |scroll_ui| {
                    if !ctx.memory(|m| {
                        m.data
                            .get_temp::<bool>(Id::new("fader_hovered_last_frame"))
                            .unwrap_or(false)
                    }) && rack_resp.hovered()
                        && scroll_delta_y.abs() > 0.5
                        && scroll_delta_x.abs() < 0.5
                    {
                        scroll_ui.scroll_with_delta(Vec2::new(scroll_delta_y * 1.4, 0.0));
                    }

                    if visible_keys.is_empty() {
                        scroll_ui.allocate_ui(Vec2::new(rack_inner.width() - 8.0, card_h), |empty| {
                            empty.centered_and_justified(|c| {
                                c.vertical_centered(|v| {
                                    v.add_space(card_h * 0.30);
                                    v.label(
                                        RichText::new("No Active Output Channels")
                                            .font(font_bold(15.0))
                                            .color(TEXT_PRIMARY),
                                    );
                                    v.add_space(6.0);
                                    v.label(
                                        RichText::new(
                                            "Enable any discovered speaker below or switch filter to 'All Channels' to patch devices into the Unison mix.",
                                        )
                                        .font(font_regular(12.5))
                                        .color(TEXT_SECONDARY),
                                    );
                                    v.add_space(12.0);
                                    if studio_pad_button(
                                        v,
                                        None,
                                        "Show All Channels",
                                        Vec2::new(146.0, 32.0),
                                        PadStyle::CopperTinted,
                                    )
                                    .clicked()
                                    {
                                        self.rack_filter = RackFilter::All;
                                    }
                                });
                            });
                        });
                    } else {
                        scroll_ui.horizontal_top(|cards| {
                            cards.spacing_mut().item_spacing.x = strip_gap;
                            for key in &visible_keys {
                                if self.draw_channel_strip(cards, ctx, key, elastic_w, card_h) {
                                    any_fader_hovered = true;
                                }
                            }
                        });
                    }
                });

            // Anchored right Master Bus Strip
            if self.draw_master_strip(row, ctx, master_w, console_h) {
                any_fader_hovered = true;
            }
        });

        ctx.memory_mut(|m| {
            m.data
                .insert_temp(Id::new("fader_hovered_last_frame"), any_fader_hovered);
        });

        // Quick-add dock for inactive devices when Active Only filter is selected
        if !hidden_excluded.is_empty() {
            ui.add_space(6.0);
            crisp_frame(
                ui,
                9.0,
                Margin::symmetric(10.0, 5.0),
                BG_HEADER,
                Some(BORDER_SUBTLE),
                |dock| {
                    ScrollArea::horizontal()
                        .id_salt("quick_add_excluded_dock")
                        .show(dock, |dh| {
                            dh.horizontal(|h| {
                                h.label(
                                    RichText::new("STANDBY OUTPUTS:")
                                        .font(font_mono_bold(10.0))
                                        .color(TEXT_MUTED),
                                );
                                for key in &hidden_excluded {
                                    if let Some(sp) = self.config.speakers.get(key).cloned() {
                                        let label = format!("+ {}", truncate_name(sp.label(), 22));
                                        if studio_pad_button(
                                            h,
                                            Some(icons::icon_for_bus(&sp.bus_type, sp.is_internal)),
                                            &label,
                                            Vec2::new(90.0, 24.0),
                                            PadStyle::Neutral,
                                        )
                                        .on_hover_text("Click to patch this speaker into the active console rack")
                                        .clicked()
                                        {
                                            self.toggle_speaker_enabled(key, ctx);
                                        }
                                    }
                                }
                            });
                        });
                },
            );
        }
    }

    /// Renders an individual vertical console channel strip. Returns `true` if its fader is hovered.
    fn draw_channel_strip(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        sink_key: &str,
        width: f32,
        height: f32,
    ) -> bool {
        let Some(sp_snapshot) = self.config.speakers.get(sink_key).cloned() else {
            return false;
        };

        let is_enabled = sp_snapshot.role.is_enabled();
        let compact = self.strip_density == StripDensity::Compact;
        let dest_status = self
            .routing_status
            .destinations
            .iter()
            .find(|d| d.sink_name == sp_snapshot.sink_name)
            .cloned();
        let is_online = self.available_sinks.is_empty()
            || self
                .available_sinks
                .iter()
                .any(|s| s.name == sp_snapshot.sink_name);
        let is_streaming = dest_status.as_ref().is_some_and(|d| d.is_streaming);

        let role_color = role_accent_color(sp_snapshot.role);
        let accent = if !is_enabled {
            TEXT_MUTED
        } else {
            role_color
        };

        let bg = if is_enabled {
            BG_STRIP
        } else {
            BG_STRIP_EXCLUDED
        };
        let border = if is_streaming && is_enabled {
            tint_opaque(BG_RACK, accent, 130)
        } else if is_enabled {
            tint_opaque(BG_RACK, accent, 64)
        } else {
            BORDER_SUBTLE
        };

        let (card_rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        let painter = ui.painter();

        paint_crisp_box(painter, card_rect, 12.0, bg, Some(border));

        // Subtle bottom micro-accent pill for active channels
        if is_enabled {
            let pill_w = (card_rect.width() * 0.28).clamp(32.0, 52.0);
            let bottom_pill = Rect::from_center_size(
                Pos2::new(card_rect.center().x, card_rect.bottom() - 3.0),
                Vec2::new(pill_w, 2.0),
            );
            let pill_col = tint_opaque(bg, accent, if is_streaming { 220 } else { 130 });
            painter.rect_filled(bottom_pill, Rounding::same(1.0), pill_col);
        }

        let inner_rect = card_rect.shrink2(Vec2::new(10.0, 10.0));
        let mut s = ui.new_child(egui::UiBuilder::new().max_rect(inner_rect).layout(
            Layout::top_down(Align::LEFT),
        ));
        s.spacing_mut().item_spacing.y = 6.0;

        // 1. Bus tag + enable pill switch
        s.horizontal(|top| {
            let ic_rect = top
                .allocate_exact_size(Vec2::splat(16.0), Sense::hover())
                .0;
            icons::paint_icon(
                top.painter(),
                ic_rect,
                icons::icon_for_bus(&sp_snapshot.bus_type, sp_snapshot.is_internal),
                if is_enabled { accent } else { TEXT_MUTED },
            );

            let bus_tag = format_bus_badge(
                &sp_snapshot.bus_type,
                sp_snapshot.is_internal,
                sp_snapshot.hardware_latency_ms,
                compact,
            );
            badge_chip(top, &bus_tag, BG_WELL, TEXT_SECONDARY, BORDER_SUBTLE);

            top.with_layout(Layout::right_to_left(Align::Center), |r| {
                if pill_switch(r, is_enabled, role_color)
                    .on_hover_text(if is_enabled {
                        "Exclude channel from Unison mix"
                    } else {
                        "Include channel in Unison mix"
                    })
                    .clicked()
                {
                    self.toggle_speaker_enabled(sink_key, ctx);
                }
            });
        });

        // 2. Title + Inline Rename
        let editing_this = self.editing_alias_sink.as_deref() == Some(sink_key);
        if editing_this {
            s.horizontal(|eh| {
                let edit_w = (eh.available_width() - 36.0).max(60.0);
                let resp = eh.add_sized(
                    Vec2::new(edit_w, 24.0),
                    TextEdit::singleline(&mut self.editing_alias_buf)
                        .font(font_regular(12.0))
                        .hint_text("Speaker alias…"),
                );
                if (resp.lost_focus() && eh.input(|i| i.key_pressed(Key::Enter)))
                    || studio_pad_button(eh, None, "OK", Vec2::new(30.0, 24.0), PadStyle::CopperPrimary)
                        .clicked()
                {
                    let trimmed = self.editing_alias_buf.trim().to_string();
                    if let Some(sp) = self.config.speakers.get_mut(sink_key) {
                        sp.custom_name = if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed)
                        };
                    }
                    self.editing_alias_sink = None;
                    self.commit_speaker_config_change(sink_key, false, ctx);
                }
            });
        } else {
            s.horizontal(|nh| {
                let max_chars = if compact { 13 } else { 19 };
                let label_text = truncate_name(sp_snapshot.label(), max_chars);
                let name_lbl = nh.add(
                    egui::Label::new(
                        RichText::new(label_text)
                            .font(font_bold(if compact { 12.0 } else { 13.0 }))
                            .color(if is_enabled { TEXT_PRIMARY } else { TEXT_SECONDARY }),
                    )
                    .truncate(),
                );
                name_lbl.on_hover_text(sp_snapshot.label());

                nh.with_layout(Layout::right_to_left(Align::Center), |r| {
                    let (btn_rect, btn_resp) =
                        r.allocate_exact_size(Vec2::splat(20.0), Sense::click());
                    let ic_col = if btn_resp.hovered() {
                        COPPER_GLOW
                    } else {
                        TEXT_MUTED
                    };
                    icons::paint_icon(r.painter(), btn_rect.shrink(3.0), UiIcon::EditPen, ic_col);
                    if btn_resp.on_hover_text("Rename speaker alias").clicked() {
                        self.editing_alias_sink = Some(sink_key.to_string());
                        self.editing_alias_buf = sp_snapshot
                            .custom_name
                            .clone()
                            .unwrap_or_else(|| sp_snapshot.display_name.clone());
                    }
                });
            });
        }

        // Connection / Stream state strip line
        s.horizontal(|st| {
            let (dot_col, state_txt) = if !is_online {
                (AMBER, "OFFLINE")
            } else if !is_enabled {
                (TEXT_MUTED, "EXCLUDED")
            } else if sp_snapshot.mute {
                (ROSE, "MUTED")
            } else if is_streaming {
                (EMERALD, "STREAMING")
            } else if self.is_running {
                (COPPER, "ARMED")
            } else {
                (TEXT_SECONDARY, "STANDBY")
            };
            let (drect, _) = st.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            st.painter().circle_filled(drect.center(), 3.2, dot_col);
            st.label(
                RichText::new(state_txt)
                    .font(font_mono_bold(9.8))
                    .color(dot_col),
            );
            st.with_layout(Layout::right_to_left(Align::Center), |r| {
                r.label(
                    RichText::new(sp_snapshot.role.short_label())
                        .font(font_mono_bold(10.5))
                        .color(accent),
                );
            });
        });

        // 3. Role Matrix Selector (all 7 roles cleanly reachable)
        if compact {
            let roles = [
                SpeakerRole::Stereo,
                SpeakerRole::Left,
                SpeakerRole::Right,
                SpeakerRole::Center,
                SpeakerRole::Subwoofer,
                SpeakerRole::SurroundLeft,
                SpeakerRole::SurroundRight,
            ];
            let next_role = {
                let idx = roles
                    .iter()
                    .position(|r| *r == sp_snapshot.role)
                    .unwrap_or(0);
                roles[(idx + 1) % roles.len()]
            };
            let role_btn_w = s.available_width();
            if studio_pad_button(
                &mut s,
                None,
                &format!("Role: {}", sp_snapshot.role.short_label()),
                Vec2::new(role_btn_w, 24.0),
                if is_enabled {
                    PadStyle::CopperTinted
                } else {
                    PadStyle::Neutral
                },
            )
            .on_hover_text("Click to cycle speaker role (or open Tune for all roles)")
            .clicked()
            {
                self.set_speaker_role(sink_key, next_role, ctx);
            }
        } else {
            let row1 = [
                SpeakerRole::Stereo,
                SpeakerRole::Left,
                SpeakerRole::Right,
                SpeakerRole::Center,
            ];
            let row2 = [
                SpeakerRole::Subwoofer,
                SpeakerRole::SurroundLeft,
                SpeakerRole::SurroundRight,
            ];
            let pad_w1 = ((s.available_width() - 9.0) / 4.0).max(34.0);
            s.horizontal(|rh| {
                rh.spacing_mut().item_spacing.x = 3.0;
                for r in row1 {
                    let selected = sp_snapshot.role == r;
                    if role_pill_btn(rh, r.short_label(), pad_w1, selected, role_accent_color(r))
                        .clicked()
                    {
                        self.set_speaker_role(sink_key, r, ctx);
                    }
                }
            });
            let pad_w2 = ((s.available_width() - 6.0) / 3.0).max(40.0);
            s.horizontal(|rh| {
                rh.spacing_mut().item_spacing.x = 3.0;
                for r in row2 {
                    let selected = sp_snapshot.role == r;
                    if role_pill_btn(rh, r.short_label(), pad_w2, selected, role_accent_color(r))
                        .clicked()
                    {
                        self.set_speaker_role(sink_key, r, ctx);
                    }
                }
            });
        }

        // 4. Vertical Fader Stage
        let reserved_bottom = if compact { 126.0 } else { 130.0 };
        let fader_h = (s.available_height() - reserved_bottom).clamp(110.0, 260.0);
        let fader_w = if compact { 66.0 } else { 80.0 };

        let mut gain_val = sp_snapshot.volume_gain;
        let mut fader_hovered = false;
        let mut gain_ipc_payload: Option<(f64, bool)> = None;

        s.vertical_centered(|fc| {
            let resp = vertical_studio_fader(
                fc,
                Id::new(("speaker_fader", sink_key)),
                &mut gain_val,
                fader_w,
                fader_h,
                accent,
                !is_enabled || sp_snapshot.mute,
                is_streaming,
            );
            fader_hovered = resp.hovered;

            if resp.changed {
                let due = self
                    .last_speaker_sync
                    .get(sink_key)
                    .map_or(true, |t| t.elapsed() >= Duration::from_millis(45));
                if due || resp.released {
                    self.last_speaker_sync
                        .insert(sink_key.to_string(), Instant::now());
                    gain_ipc_payload = Some((gain_val, resp.released));
                }
            } else if resp.released {
                gain_ipc_payload = Some((gain_val, true));
            }
        });

        if let Some((g_val, persist)) = gain_ipc_payload {
            if let Some(sp) = self.config.speakers.get_mut(sink_key) {
                sp.volume_gain = g_val;
            }
            self.note_local_edit();
            self.dispatch_ipc(
                IpcRequest::SetSpeakerGain {
                    sink_name: sp_snapshot.sink_name.clone(),
                    gain: g_val,
                    persist,
                },
                ctx,
                Duration::from_secs(2),
            );
        }

        // 5. Numeric Gain Readout (dB + %)
        s.vertical_centered(|vr| {
            vr.spacing_mut().item_spacing.y = 1.0;
            vr.label(
                RichText::new(gain_to_db_str(gain_val))
                    .font(font_mono_bold(13.0))
                    .color(if !is_enabled || sp_snapshot.mute {
                        TEXT_MUTED
                    } else if gain_val > 1.02 {
                        AMBER
                    } else {
                        TEXT_PRIMARY
                    }),
            );
            vr.label(
                RichText::new(format!("{:.0}%", gain_val * 100.0))
                    .font(font_mono(10.0))
                    .color(TEXT_MUTED),
            );
        });

        // 6. Tactile Transport Row: Mute | Test | 180° Polarity
        let btn_w = ((s.available_width() - 8.0) / 3.0).max(34.0);
        let test_active = self
            .test_tone_until
            .get(&sp_snapshot.sink_name)
            .is_some_and(|t| *t > Instant::now());

        s.horizontal(|th| {
            th.spacing_mut().item_spacing.x = 4.0;
            if studio_pad_button(
                th,
                Some(if sp_snapshot.mute {
                    UiIcon::Mute
                } else {
                    UiIcon::SpeakerOn
                }),
                "",
                Vec2::new(btn_w, 28.0),
                if sp_snapshot.mute {
                    PadStyle::RoseMute
                } else {
                    PadStyle::Neutral
                },
            )
            .on_hover_text(if sp_snapshot.mute {
                "Unmute speaker"
            } else {
                "Mute speaker"
            })
            .clicked()
            {
                let next_mute = !sp_snapshot.mute;
                if let Some(sp) = self.config.speakers.get_mut(sink_key) {
                    sp.mute = next_mute;
                }
                self.note_local_edit();
                self.dispatch_ipc(
                    IpcRequest::SetSpeakerMute {
                        sink_name: sp_snapshot.sink_name.clone(),
                        mute: next_mute,
                    },
                    ctx,
                    Duration::from_secs(2),
                );
            }

            let tone_freq = if sp_snapshot.role == SpeakerRole::Subwoofer {
                80.0
            } else {
                440.0
            };
            let tone_hint = if sp_snapshot.role == SpeakerRole::Subwoofer {
                "Play 80 Hz subwoofer test tone"
            } else {
                "Play 440 Hz test tone on this speaker"
            };
            if studio_pad_button(
                th,
                Some(UiIcon::TestTone),
                "",
                Vec2::new(btn_w, 28.0),
                if test_active {
                    PadStyle::AmberWarn
                } else {
                    PadStyle::Neutral
                },
            )
            .on_hover_text(tone_hint)
            .clicked()
            {
                self.test_tone_until.insert(
                    sp_snapshot.sink_name.clone(),
                    Instant::now() + Duration::from_millis(1400),
                );
                self.dispatch_ipc(
                    IpcRequest::PlayTestTone {
                        sink_name: sp_snapshot.sink_name.clone(),
                        freq: tone_freq,
                        duration: 1.2,
                    },
                    ctx,
                    Duration::from_secs(2),
                );
            }

            if studio_pad_button(
                th,
                Some(UiIcon::Polarity180),
                "",
                Vec2::new(btn_w, 28.0),
                if sp_snapshot.phase_inverted {
                    PadStyle::CopperTinted
                } else {
                    PadStyle::Neutral
                },
            )
            .on_hover_text("Invert acoustic polarity (180° phase)")
            .clicked()
            {
                if let Some(sp) = self.config.speakers.get_mut(sink_key) {
                    sp.phase_inverted = !sp.phase_inverted;
                }
                self.commit_speaker_config_change(sink_key, true, ctx);
            }
        });

        // 7. Delay Readout + Tune Modal Trigger
        let total_delay_ms = dest_status
            .as_ref()
            .map_or(sp_snapshot.delay_ms, |d| d.total_delay_ms);

        crisp_frame(
            &mut s,
            8.0,
            Margin::symmetric(8.0, 5.0),
            BG_WELL,
            Some(BORDER_SUBTLE),
            |df| {
                df.horizontal(|h| {
                    h.vertical(|v| {
                        v.spacing_mut().item_spacing.y = 0.0;
                        v.label(
                            RichText::new("DELAY")
                                .font(font_mono_bold(8.8))
                                .color(TEXT_MUTED),
                        );
                        let delay_lbl = v.label(
                            RichText::new(format!("{total_delay_ms:.1} ms"))
                                .font(font_mono_bold(11.0))
                                .color(if total_delay_ms > 0.1 {
                                    COPPER_GLOW
                                } else {
                                    TEXT_SECONDARY
                                }),
                        );
                        if let Some(d) = &dest_status {
                            delay_lbl.on_hover_text(format!(
                                "#{} {} [{} · {}]\nEnabled: {} · Gain: {:.0}% · Phase: {}\nHW {:.1} ms + Auto {:.1} ms + Manual {:.1} ms",
                                d.sink_id,
                                d.display_name,
                                d.bus_type.to_uppercase(),
                                d.state,
                                if d.is_enabled { d.role.as_str() } else { "excluded" },
                                d.volume_gain * 100.0,
                                if d.phase_inverted { "180°" } else { "0°" },
                                d.hardware_latency_ms,
                                d.auto_comp_delay_ms,
                                d.delay_ms,
                            ));
                        }
                    });

                    h.with_layout(Layout::right_to_left(Align::Center), |r| {
                        let single_busy =
                            self.calibrating_single_sink.as_deref() == Some(sink_key);
                        if studio_pad_button(
                            r,
                            Some(UiIcon::AcousticTune),
                            if compact { "" } else { "Tune" },
                            Vec2::new(if compact { 34.0 } else { 64.0 }, 26.0),
                            if single_busy {
                                PadStyle::AmberWarn
                            } else {
                                PadStyle::Neutral
                            },
                        )
                        .on_hover_text("Open Acoustic Delay, Polarity & Mic Calibration modal")
                        .clicked()
                        {
                            self.tuning_modal_sink = Some(sink_key.to_string());
                        }
                    });
                });
            },
        );

        fader_hovered
    }

    /// Renders the pinned Master Bus console strip on the right side. Returns `true` if hovered.
    fn draw_master_strip(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        width: f32,
        height: f32,
    ) -> bool {
        let is_running = self.is_running;
        let any_streaming = self
            .routing_status
            .destinations
            .iter()
            .any(|d| d.is_streaming);

        let (card_rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        let painter = ui.painter();

        paint_crisp_box(
            painter,
            card_rect,
            12.0,
            BG_MASTER_STRIP,
            Some(if is_running {
                tint_opaque(BG_WINDOW, COPPER, 135)
            } else {
                BORDER_STRONG
            }),
        );

        // Subtle bottom micro-pill matching active channel strips
        if is_running {
            let bottom_pill = Rect::from_center_size(
                Pos2::new(card_rect.center().x, card_rect.bottom() - 3.0),
                Vec2::new(48.0, 2.0),
            );
            painter.rect_filled(
                bottom_pill,
                Rounding::same(1.0),
                if any_streaming { COPPER_GLOW } else { COPPER },
            );
        }

        let inner = card_rect.shrink2(Vec2::new(11.0, 11.0));
        let mut m = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(
            Layout::top_down(Align::Center),
        ));
        m.spacing_mut().item_spacing.y = 6.0;

        badge_chip(
            &mut m,
            "MASTER BUS",
            if is_running {
                tint_opaque(BG_MASTER_STRIP, COPPER, 36)
            } else {
                BG_WELL
            },
            if is_running { COPPER_GLOW } else { TEXT_SECONDARY },
            if is_running {
                tint_opaque(BG_MASTER_STRIP, COPPER, 135)
            } else {
                BORDER_SUBTLE
            },
        );

        m.label(
            RichText::new("Polifonia Master")
                .font(font_bold(13.0))
                .color(TEXT_PRIMARY),
        );

        let state_text = if !is_running {
            "STANDBY"
        } else if any_streaming {
            "LOCKED · LIVE"
        } else {
            &self.routing_status.master_state
        };
        m.label(
            RichText::new(state_text)
                .font(font_mono_bold(10.0))
                .color(if !is_running {
                    TEXT_MUTED
                } else if any_streaming {
                    EMERALD
                } else {
                    COPPER_GLOW
                }),
        )
        .on_hover_text(format!(
            "Unison Bus: {} · Master Gain: {:.0}%",
            if self.routing_status.is_unison_active {
                "ONLINE"
            } else {
                "STANDBY"
            },
            self.routing_status.master_volume * 100.0
        ));

        // Master Fader + Dual Stereo Meter Bars
        let reserved_bottom = 148.0;
        let fader_h = (m.available_height() - reserved_bottom).clamp(110.0, 275.0);
        let mut master_gain = self.config.master_volume;
        let mut fader_hovered = false;
        let mut master_ipc_payload: Option<(f64, bool)> = None;

        m.horizontal(|fh| {
            fh.add_space(((fh.available_width() - 108.0) * 0.5).max(0.0));
            draw_master_level_bar(
                fh,
                fader_h,
                master_gain,
                any_streaming && is_running,
                ctx.input(|i| i.time),
            );
            let resp = vertical_studio_fader(
                fh,
                Id::new("polifonia_master_fader"),
                &mut master_gain,
                78.0,
                fader_h,
                COPPER,
                !is_running,
                any_streaming && is_running,
            );
            fader_hovered = resp.hovered;
            draw_master_level_bar(
                fh,
                fader_h,
                master_gain,
                any_streaming && is_running,
                ctx.input(|i| i.time) + 0.37,
            );

            if resp.changed {
                let due = self.last_master_sync.elapsed() >= Duration::from_millis(45);
                if due || resp.released {
                    self.last_master_sync = Instant::now();
                    master_ipc_payload = Some((master_gain, resp.released));
                }
            } else if resp.released {
                master_ipc_payload = Some((master_gain, true));
            }
        });

        if let Some((mg, persist)) = master_ipc_payload {
            self.config.master_volume = mg;
            self.note_local_edit();
            self.dispatch_ipc(
                IpcRequest::SetMasterGain { gain: mg, persist },
                ctx,
                Duration::from_secs(2),
            );
        }

        // Calibrated Master Readout
        m.label(
            RichText::new(gain_to_db_str(master_gain))
                .font(font_mono_bold(15.0))
                .color(if master_gain > 1.02 {
                    AMBER
                } else if is_running {
                    COPPER_GLOW
                } else {
                    TEXT_PRIMARY
                }),
        );
        m.label(
            RichText::new(format!("{:.0}%", master_gain * 100.0))
                .font(font_mono(10.5))
                .color(TEXT_MUTED),
        );

        let full_btn_w = m.available_width();
        let is_master_muted = master_gain <= 0.001;
        if studio_pad_button(
            &mut m,
            Some(if is_master_muted {
                UiIcon::Mute
            } else {
                UiIcon::SpeakerOn
            }),
            if is_master_muted {
                "Restore Bus"
            } else {
                "Mute Master"
            },
            Vec2::new(full_btn_w, 28.0),
            if is_master_muted {
                PadStyle::RoseMute
            } else {
                PadStyle::Neutral
            },
        )
        .clicked()
        {
            let new_master = if is_master_muted {
                if self.saved_master_pre_mute > 0.05 {
                    self.saved_master_pre_mute
                } else {
                    1.0
                }
            } else {
                self.saved_master_pre_mute = master_gain;
                0.0
            };
            self.config.master_volume = new_master;
            self.note_local_edit();
            self.dispatch_ipc(
                IpcRequest::SetMasterGain {
                    gain: new_master,
                    persist: true,
                },
                ctx,
                Duration::from_secs(2),
            );
        }

        let mut snap_unity = false;
        let mut reset_delays = false;
        m.horizontal(|bh| {
            bh.spacing_mut().item_spacing.x = 5.0;
            let half_w = ((full_btn_w - 5.0) * 0.5).max(52.0);
            if studio_pad_button(
                bh,
                None,
                "0.0 dB",
                Vec2::new(half_w, 26.0),
                PadStyle::Neutral,
            )
            .on_hover_text("Snap Master fader to unity gain (+0.0 dB)")
            .clicked()
            {
                snap_unity = true;
            }

            if studio_pad_button(
                bh,
                None,
                "0 ms",
                Vec2::new(half_w, 26.0),
                PadStyle::Neutral,
            )
            .on_hover_text("Reset all manual speaker delays to 0.0 ms")
            .clicked()
            {
                reset_delays = true;
            }
        });

        if snap_unity {
            self.config.master_volume = 1.0;
            self.note_local_edit();
            self.dispatch_ipc(
                IpcRequest::SetMasterGain {
                    gain: 1.0,
                    persist: true,
                },
                ctx,
                Duration::from_secs(2),
            );
        }
        if reset_delays {
            for sp in self.config.speakers.values_mut() {
                sp.delay_ms = 0.0;
            }
            self.note_local_edit();
            self.dispatch_ipc(IpcRequest::ResetAllDelays, ctx, Duration::from_secs(5));
            self.push_toast("All speaker acoustic delays reset to 0.0 ms", false);
        }

        if studio_pad_button(
            &mut m,
            None,
            "Make Default",
            Vec2::new(full_btn_w, 25.0),
            PadStyle::Neutral,
        )
        .on_hover_text("Reassert Polifonia Master as system default PipeWire output")
        .clicked()
        {
            self.dispatch_ipc(IpcRequest::ReassertDefault, ctx, Duration::from_secs(2));
            self.push_toast("Polifonia Master asserted as system default output", false);
        }

        if any_streaming && is_running {
            ctx.request_repaint_after(Duration::from_millis(65));
        }

        fader_hovered
    }

    /// Studio Copper Bezier PipeWire signal flow diagram inside a vertical ScrollArea.
    fn draw_routing_view(&mut self, ui: &mut Ui) {
        let sources = &self.routing_status.sources;
        let ordered_keys = self.sorted_speaker_keys();
        let row_count = sources.len().max(ordered_keys.len()).max(2);
        let canvas_h = ((row_count as f32) * 86.0 + 40.0).max(460.0);

        crisp_frame(
            ui,
            12.0,
            Margin::same(12.0),
            BG_RACK,
            Some(BORDER_SUBTLE),
            |outer| {
                ScrollArea::vertical()
                    .id_salt("polifonia_signal_flow_scroll")
                    .auto_shrink([false, false])
                    .show(outer, |sv| {
                        let canvas_w = sv.available_width().max(760.0);
                        let (rect, _) =
                            sv.allocate_exact_size(Vec2::new(canvas_w, canvas_h), Sense::hover());
                        let p = sv.painter();

                        // Subtle technical engineering grid dots
                        let step = 28.0;
                        let mut gx = rect.left() + 14.0;
                        while gx < rect.right() {
                            let mut gy = rect.top() + 14.0;
                            while gy < rect.bottom() {
                                p.circle_filled(
                                    Pos2::new(gx, gy),
                                    0.9,
                                    Color32::from_rgb(36, 36, 42),
                                );
                                gy += step;
                            }
                            gx += step;
                        }

                        // Column headers
                        let src_x = rect.left() + 18.0;
                        let src_w = (canvas_w * 0.25).clamp(180.0, 240.0);
                        let hub_w = (canvas_w * 0.26).clamp(200.0, 260.0);
                        let hub_x = rect.center().x - hub_w * 0.5;
                        let dst_w = (canvas_w * 0.29).clamp(220.0, 290.0);
                        let dst_x = rect.right() - dst_w - 18.0;

                        p.text(
                            Pos2::new(src_x, rect.top() + 10.0),
                            Align2::LEFT_TOP,
                            "AUDIO SOURCES (STREAMS)",
                            font_mono_bold(10.5),
                            TEXT_MUTED,
                        );
                        p.text(
                            Pos2::new(rect.center().x, rect.top() + 10.0),
                            Align2::CENTER_TOP,
                            "UNISON MATRIX COMBINER",
                            font_mono_bold(10.5),
                            COPPER_GLOW,
                        );
                        p.text(
                            Pos2::new(dst_x, rect.top() + 10.0),
                            Align2::LEFT_TOP,
                            "OUTPUT CHANNELS",
                            font_mono_bold(10.5),
                            TEXT_MUTED,
                        );

                        // Central Hub card
                        let hub_rect = Rect::from_center_size(
                            Pos2::new(hub_x + hub_w * 0.5, rect.center().y),
                            Vec2::new(hub_w, 152.0),
                        );
                        let is_running = self.is_running;

                        // Left side source cards + copper curves to Hub
                        let top_y = rect.top() + 38.0;
                        let src_count = sources.len().max(1);
                        let src_span = ((src_count as f32) * 82.0).min(canvas_h - 60.0);
                        let src_start_y = rect.center().y - src_span * 0.5;

                        if sources.is_empty() {
                            let empty_src = Rect::from_min_size(
                                Pos2::new(src_x, rect.center().y - 34.0),
                                Vec2::new(src_w, 68.0),
                            );
                            paint_crisp_box(
                                p,
                                empty_src,
                                10.0,
                                BG_STRIP_EXCLUDED,
                                Some(BORDER_SUBTLE),
                            );
                            p.text(
                                empty_src.center(),
                                Align2::CENTER_CENTER,
                                "No Active Streams",
                                font_regular(12.5),
                                TEXT_MUTED,
                            );
                            paint_cubic_cable(
                                p,
                                Pos2::new(empty_src.right(), empty_src.center().y),
                                Pos2::new(hub_rect.left(), hub_rect.center().y),
                                BORDER_STRONG,
                                false,
                            );
                        } else {
                            let slot_h = src_span / src_count as f32;
                            for (i, src) in sources.iter().enumerate() {
                                let cy = src_start_y + (i as f32 + 0.5) * slot_h;
                                let srect = Rect::from_center_size(
                                    Pos2::new(src_x + src_w * 0.5, cy),
                                    Vec2::new(src_w, 64.0),
                                );
                                let cable_col = if src.is_active && is_running {
                                    COPPER
                                } else {
                                    BORDER_STRONG
                                };
                                paint_cubic_cable(
                                    p,
                                    Pos2::new(srect.right(), srect.center().y),
                                    Pos2::new(hub_rect.left(), hub_rect.center().y),
                                    cable_col,
                                    src.is_active && is_running,
                                );

                                paint_crisp_box(p, srect, 10.0, BG_STRIP, Some(cable_col));
                                p.text(
                                    Pos2::new(srect.left() + 12.0, srect.top() + 14.0),
                                    Align2::LEFT_TOP,
                                    truncate_name(&src.app_name, 22),
                                    font_bold(12.5),
                                    TEXT_PRIMARY,
                                );
                                let sub = if src.media_name.is_empty() {
                                    format!("{}% Stream Gain", src.volume_pct)
                                } else {
                                    truncate_name(&src.media_name, 26)
                                };
                                p.text(
                                    Pos2::new(srect.left() + 12.0, srect.top() + 36.0),
                                    Align2::LEFT_TOP,
                                    sub,
                                    font_mono(10.5),
                                    if src.is_active { COPPER_GLOW } else { TEXT_MUTED },
                                );
                            }
                        }

                        // Output channel nodes + cables from Hub
                        let dst_count = ordered_keys.len().max(1);
                        let dst_span = (dst_count as f32) * 82.0;
                        let dst_start_y = top_y.max(rect.center().y - dst_span * 0.5);
                        let dst_slot_h = dst_span / dst_count as f32;

                        for (i, key) in ordered_keys.iter().enumerate() {
                            let sp = &self.config.speakers[key];
                            let dest = self
                                .routing_status
                                .destinations
                                .iter()
                                .find(|d| d.sink_name == sp.sink_name);
                            let enabled = sp.role.is_enabled();
                            let streaming = dest.is_some_and(|d| d.is_streaming);
                            let total_delay = dest.map_or(sp.delay_ms, |d| d.total_delay_ms);

                            let cy = dst_start_y + (i as f32 + 0.5) * dst_slot_h;
                            let drect = Rect::from_center_size(
                                Pos2::new(dst_x + dst_w * 0.5, cy),
                                Vec2::new(dst_w, 68.0),
                            );
                            let cable_col = if !enabled {
                                Color32::from_rgb(50, 50, 58)
                            } else if streaming {
                                COPPER
                            } else {
                                role_accent_color(sp.role)
                            };

                            paint_cubic_cable(
                                p,
                                Pos2::new(hub_rect.right(), hub_rect.center().y),
                                Pos2::new(drect.left(), drect.center().y),
                                cable_col,
                                streaming && enabled,
                            );

                            paint_crisp_box(
                                p,
                                drect,
                                10.0,
                                if enabled { BG_STRIP } else { BG_STRIP_EXCLUDED },
                                Some(if enabled { cable_col } else { BORDER_SUBTLE }),
                            );

                            let ic_rect = Rect::from_min_size(
                                Pos2::new(drect.left() + 10.0, drect.top() + 11.0),
                                Vec2::splat(16.0),
                            );
                            icons::paint_icon(
                                p,
                                ic_rect,
                                icons::icon_for_bus(&sp.bus_type, sp.is_internal),
                                cable_col,
                            );

                            p.text(
                                Pos2::new(drect.left() + 32.0, drect.top() + 11.0),
                                Align2::LEFT_TOP,
                                truncate_name(sp.label(), 22),
                                font_bold(12.5),
                                if enabled { TEXT_PRIMARY } else { TEXT_MUTED },
                            );
                            p.text(
                                Pos2::new(drect.right() - 10.0, drect.top() + 12.0),
                                Align2::RIGHT_TOP,
                                sp.role.short_label(),
                                font_mono_bold(10.5),
                                cable_col,
                            );

                            let meta = if !enabled {
                                "EXCLUDED".to_string()
                            } else {
                                format!(
                                    "{} · {:.1} ms{}",
                                    gain_to_db_str(sp.volume_gain),
                                    total_delay,
                                    if sp.phase_inverted { " · Ø180°" } else { "" }
                                )
                            };
                            p.text(
                                Pos2::new(drect.left() + 12.0, drect.top() + 38.0),
                                Align2::LEFT_TOP,
                                meta,
                                font_mono(10.5),
                                if enabled { TEXT_SECONDARY } else { TEXT_MUTED },
                            );
                        }

                        // Paint Central Unison Combiner Hub above cable endpoints
                        paint_crisp_box(
                            p,
                            hub_rect,
                            14.0,
                            BG_MASTER_STRIP,
                            Some(if is_running { COPPER } else { BORDER_STRONG }),
                        );

                        let emblem_r = Rect::from_center_size(
                            Pos2::new(hub_rect.center().x, hub_rect.top() + 32.0),
                            Vec2::splat(30.0),
                        );
                        icons::paint_brand_emblem(p, emblem_r, &self.brand_tex);

                        p.text(
                            Pos2::new(hub_rect.center().x, hub_rect.top() + 62.0),
                            Align2::CENTER_TOP,
                            "POLIFONIA MASTER",
                            font_bold(13.0),
                            TEXT_PRIMARY,
                        );
                        p.text(
                            Pos2::new(hub_rect.center().x, hub_rect.top() + 86.0),
                            Align2::CENTER_TOP,
                            if is_running {
                                "UNISON ENGINE ACTIVE"
                            } else {
                                "UNISON ENGINE STANDBY"
                            },
                            font_mono_bold(10.0),
                            if is_running { EMERALD } else { TEXT_MUTED },
                        );
                        p.text(
                            Pos2::new(hub_rect.center().x, hub_rect.top() + 112.0),
                            Align2::CENTER_TOP,
                            format!(
                                "Gain {}  ·  Max HW {:.1} ms",
                                gain_to_db_str(self.config.master_volume),
                                self.routing_status.max_hw_latency
                            ),
                            font_mono(10.5),
                            COPPER_GLOW,
                        );
                    });
            },
        );
    }

    /// Renders custom-chrome Acoustic Tuning modal overlay when a speaker's Tune button is active.
    fn draw_tuning_modal(&mut self, ctx: &egui::Context, shell_rect: Rect, outer_rounding: f32) {
        let Some(sink_key) = self.tuning_modal_sink.clone() else {
            return;
        };
        let Some(sp) = self.config.speakers.get(&sink_key).cloned() else {
            self.tuning_modal_sink = None;
            return;
        };

        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.tuning_modal_sink = None;
            return;
        }

        egui::Area::new(Id::new("acoustic_tuning_backdrop"))
            .order(egui::Order::Middle)
            .fixed_pos(shell_rect.min)
            .show(ctx, |ui| {
                let (_, bg_resp) = ui.allocate_exact_size(shell_rect.size(), Sense::click());
                ui.painter().rect_filled(
                    shell_rect,
                    Rounding::same(outer_rounding),
                    Color32::from_black_alpha(168),
                );
                if bg_resp.clicked() {
                    self.tuning_modal_sink = None;
                }
            });

        if self.tuning_modal_sink.is_none() {
            return;
        }

        let dest = self
            .routing_status
            .destinations
            .iter()
            .find(|d| d.sink_name == sp.sink_name)
            .cloned();

        egui::Window::new("acoustic_tuning_dialog")
            .id(Id::new("acoustic_tuning_window"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .fixed_size(Vec2::new(470.0, 440.0))
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .frame(Frame::none())
            .show(ctx, |win_ui| {
                crisp_frame(
                    win_ui,
                    14.0,
                    Margin::same(18.0),
                    BG_MODAL,
                    Some(BORDER_STRONG),
                    |ui| {
                        // Header
                        ui.horizontal(|h| {
                            let (ic_r, _) = h.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                            icons::paint_icon(h.painter(), ic_r, UiIcon::AcousticTune, COPPER);
                            h.vertical(|v| {
                                v.spacing_mut().item_spacing.y = 1.0;
                                v.label(
                                    RichText::new("Acoustic Tuning & Alignment")
                                        .font(font_bold(15.0))
                                        .color(TEXT_PRIMARY),
                                );
                                v.label(
                                    RichText::new(sp.label())
                                        .font(font_mono(11.0))
                                        .color(COPPER_GLOW),
                                );
                            });
                            h.with_layout(Layout::right_to_left(Align::Center), |r| {
                                if window_control_btn(r, UiIcon::WinClose, false).clicked() {
                                    self.tuning_modal_sink = None;
                                }
                            });
                        });

                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(10.0);

                        // Section 1: Channel Role Assignment (all 7 roles + Excluded)
                        ui.label(
                            RichText::new("CHANNEL MATRIX ASSIGNMENT")
                                .font(font_mono_bold(10.5))
                                .color(TEXT_MUTED),
                        );
                        ui.add_space(5.0);

                        let all_roles: [(SpeakerRole, &str); 8] = [
                            (SpeakerRole::Stereo, "Stereo (L+R)"),
                            (SpeakerRole::Left, "Front L"),
                            (SpeakerRole::Right, "Front R"),
                            (SpeakerRole::Center, "Center"),
                            (SpeakerRole::SurroundLeft, "Surround L"),
                            (SpeakerRole::SurroundRight, "Surround R"),
                            (SpeakerRole::Subwoofer, "Sub LFE"),
                            (SpeakerRole::Excluded, "Excluded"),
                        ];
                        let chip_w = ((ui.available_width() - 18.0) / 4.0).max(96.0);
                        for chunk in all_roles.chunks(4) {
                            ui.horizontal(|rh| {
                                rh.spacing_mut().item_spacing.x = 6.0;
                                for &(role, label) in chunk {
                                    let selected = sp.role == role;
                                    if role_pill_btn(
                                        rh,
                                        label,
                                        chip_w,
                                        selected,
                                        role_accent_color(role),
                                    )
                                    .clicked()
                                    {
                                        self.set_speaker_role(&sink_key, role, ctx);
                                    }
                                }
                            });
                        }

                        ui.add_space(12.0);

                        // Section 2: Latency & Precision Delay
                        let hw_ms = sp.hardware_latency_ms;
                        let auto_comp = dest.as_ref().map_or(0.0, |d| d.auto_comp_delay_ms);
                        let total_ms = dest.as_ref().map_or(sp.delay_ms, |d| d.total_delay_ms);

                        let mut pending_delay_commit = false;
                        crisp_frame(
                            ui,
                            10.0,
                            Margin::same(12.0),
                            BG_WELL,
                            Some(BORDER_SUBTLE),
                            |box_ui| {
                                box_ui.horizontal(|lh| {
                                    lh.label(
                                        RichText::new("MANUAL DELAY OFFSET")
                                            .font(font_mono_bold(10.5))
                                            .color(TEXT_MUTED),
                                    );
                                    lh.with_layout(Layout::right_to_left(Align::Center), |r| {
                                        r.label(
                                            RichText::new(format!("{:.1} ms", sp.delay_ms))
                                                .font(font_mono_bold(14.0))
                                                .color(COPPER_GLOW),
                                        );
                                    });
                                });

                                let mut delay_val = sp.delay_ms;
                                let sl_w = box_ui.available_width();
                                let d_resp = horizontal_studio_slider(
                                    box_ui,
                                    Id::new(("modal_delay_slider", &sink_key)),
                                    &mut delay_val,
                                    0.0..=300.0,
                                    sl_w,
                                    COPPER,
                                );
                                if d_resp.changed {
                                    if let Some(entry) = self.config.speakers.get_mut(&sink_key) {
                                        entry.delay_ms = delay_val;
                                    }
                                    self.note_local_edit();
                                    if d_resp.released {
                                        pending_delay_commit = true;
                                    }
                                } else if d_resp.released {
                                    pending_delay_commit = true;
                                }

                                box_ui.add_space(4.0);
                                box_ui.horizontal(|nh| {
                                    nh.spacing_mut().item_spacing.x = 5.0;
                                    for (lbl, delta) in [
                                        ("-1.0 ms", -1.0_f64),
                                        ("-0.1 ms", -0.1),
                                        ("+0.1 ms", 0.1),
                                        ("+1.0 ms", 1.0),
                                    ] {
                                        if studio_pad_button(
                                            nh,
                                            None,
                                            lbl,
                                            Vec2::new(62.0, 25.0),
                                            PadStyle::Neutral,
                                        )
                                        .clicked()
                                        {
                                            if let Some(entry) =
                                                self.config.speakers.get_mut(&sink_key)
                                            {
                                                entry.delay_ms =
                                                    ((entry.delay_ms + delta).clamp(0.0, 300.0)
                                                        * 10.0)
                                                        .round()
                                                        / 10.0;
                                            }
                                            pending_delay_commit = true;
                                        }
                                    }
                                    nh.with_layout(Layout::right_to_left(Align::Center), |r| {
                                        if studio_pad_button(
                                            r,
                                            None,
                                            "Reset 0",
                                            Vec2::new(66.0, 25.0),
                                            PadStyle::Neutral,
                                        )
                                        .clicked()
                                        {
                                            if let Some(entry) =
                                                self.config.speakers.get_mut(&sink_key)
                                            {
                                                entry.delay_ms = 0.0;
                                            }
                                            pending_delay_commit = true;
                                        }
                                    });
                                });

                                box_ui.add_space(6.0);
                                box_ui.label(
                                    RichText::new(format!(
                                        "HW Buffer: {hw_ms:.1} ms   ·   Auto-Baseline Comp: {auto_comp:.1} ms   ·   Total Effective: {total_ms:.1} ms"
                                    ))
                                    .font(font_mono(10.5))
                                    .color(TEXT_SECONDARY),
                                );
                            },
                        );

                        if pending_delay_commit {
                            self.commit_speaker_config_change(&sink_key, true, ctx);
                        }

                        ui.add_space(10.0);

                        // Section 3: Polarity & Acoustic Mic Measurement CTA
                        let mut polarity_toggled = false;
                        ui.horizontal(|ph| {
                            ph.label(
                                RichText::new("180° Acoustic Polarity Invert")
                                    .font(font_bold(12.5))
                                    .color(TEXT_PRIMARY),
                            );
                            ph.with_layout(Layout::right_to_left(Align::Center), |r| {
                                if pill_switch(r, sp.phase_inverted, COPPER).clicked() {
                                    polarity_toggled = true;
                                }
                            });
                        });
                        if polarity_toggled {
                            if let Some(entry) = self.config.speakers.get_mut(&sink_key) {
                                entry.phase_inverted = !entry.phase_inverted;
                            }
                            self.commit_speaker_config_change(&sink_key, true, ctx);
                        }

                        ui.add_space(12.0);
                        ui.horizontal(|act| {
                            let single_busy =
                                self.calibrating_single_sink.is_some() || self.is_calibrating_all;
                            if studio_pad_button(
                                act,
                                Some(UiIcon::AutoSyncMic),
                                if single_busy {
                                    "Measuring Sweep…"
                                } else {
                                    "Measure Acoustic Delay (Mic)"
                                },
                                Vec2::new(226.0, 32.0),
                                if single_busy {
                                    PadStyle::AmberWarn
                                } else {
                                    PadStyle::CopperTinted
                                },
                            )
                            .clicked()
                            {
                                self.start_single_calibration(sp.sink_name.clone(), ctx);
                            }

                            let modal_freq = if sp.role == SpeakerRole::Subwoofer {
                                80.0
                            } else {
                                440.0
                            };
                            let modal_tone_label = if sp.role == SpeakerRole::Subwoofer {
                                "Test 80Hz"
                            } else {
                                "Test 440Hz"
                            };
                            if studio_pad_button(
                                act,
                                Some(UiIcon::TestTone),
                                modal_tone_label,
                                Vec2::new(112.0, 32.0),
                                PadStyle::Neutral,
                            )
                            .clicked()
                            {
                                self.test_tone_until.insert(
                                    sp.sink_name.clone(),
                                    Instant::now() + Duration::from_millis(1400),
                                );
                                self.dispatch_ipc(
                                    IpcRequest::PlayTestTone {
                                        sink_name: sp.sink_name.clone(),
                                        freq: modal_freq,
                                        duration: 1.2,
                                    },
                                    ctx,
                                    Duration::from_secs(2),
                                );
                            }

                            act.with_layout(Layout::right_to_left(Align::Center), |r| {
                                if studio_pad_button(
                                    r,
                                    None,
                                    "Done",
                                    Vec2::new(72.0, 32.0),
                                    PadStyle::CopperPrimary,
                                )
                                .clicked()
                                {
                                    self.tuning_modal_sink = None;
                                }
                            });
                        });
                    },
                );
            });
    }

    fn draw_footer_bar(&mut self, ui: &mut Ui) {
        let height = 28.0;
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());

        ui.painter().line_segment(
            [
                Pos2::new(rect.left(), rect.top()),
                Pos2::new(rect.right(), rect.top()),
            ],
            Stroke::new(1.0_f32, BORDER_SUBTLE),
        );

        let inner = rect.shrink2(Vec2::new(14.0, 4.0));
        let mut f = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::left_to_right(Align::Center)),
        );

        if let Some((msg, is_err, _)) = &self.status_toast {
            f.label(
                RichText::new(msg)
                    .font(font_bold(11.5))
                    .color(if *is_err { ROSE } else { EMERALD }),
            );
        } else {
            let active_count = self
                .config
                .speakers
                .values()
                .filter(|s| s.role.is_enabled())
                .count();
            let sink_count = self.available_sinks.len();
            f.label(
                RichText::new(format!(
                    "PipeWire Unison · {active_count} active / {sink_count} detected sinks · Double-click fader for 0.0 dB unity"
                ))
                .font(font_mono(10.5))
                .color(TEXT_MUTED),
            );
        }

        f.with_layout(Layout::right_to_left(Align::Center), |r| {
            r.label(
                RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                    .font(font_mono_bold(10.5))
                    .color(TEXT_MUTED),
            );
        });
    }
}

impl eframe::App for PolifoniaApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_background_events(ctx);

        // Instant close-to-tray by exiting this ephemeral UI subprocess cleanly
        if ctx.input(|i| i.viewport().close_requested()) {
            self.close_to_tray();
        }

        let is_maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let outer_rounding = if is_maximized { 0.0 } else { 13.0 };
        let screen_rect = ctx.screen_rect();
        let shell_rect = if is_maximized {
            screen_rect
        } else {
            screen_rect.shrink(2.0)
        };

        egui::CentralPanel::default()
            .frame(Frame::none().fill(Color32::TRANSPARENT))
            .show(ctx, |ui| {
                let painter = ui.painter();
                paint_crisp_box(
                    painter,
                    shell_rect,
                    outer_rounding,
                    BG_HEADER,
                    if is_maximized { None } else { Some(BORDER_STRONG) },
                );

                let content_rect = shell_rect.shrink(if is_maximized { 0.0 } else { 1.0 });
                let mid_top = content_rect.top() + 56.0;
                let mid_bot = (content_rect.bottom() - 28.0).max(mid_top);
                let edge_inset = if is_maximized { 0.0 } else { 1.0 };
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(shell_rect.left() + edge_inset, mid_top),
                        Pos2::new(shell_rect.right() - edge_inset, mid_bot),
                    ),
                    Rounding::ZERO,
                    BG_WINDOW,
                );

                let mut shell = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(content_rect)
                        .layout(Layout::top_down(Align::LEFT)),
                );
                shell.spacing_mut().item_spacing.y = 0.0;

                self.draw_header_bar(&mut shell, ctx);

                // Middle workspace region
                let body_h = (shell.available_height() - 28.0).max(160.0);
                let body_rect = shell
                    .allocate_exact_size(Vec2::new(shell.available_width(), body_h), Sense::hover())
                    .0
                    .shrink2(Vec2::new(12.0, 10.0));
                let mut body = shell.new_child(
                    egui::UiBuilder::new()
                        .max_rect(body_rect)
                        .layout(Layout::top_down(Align::LEFT)),
                );

                match self.active_tab {
                    ActiveTab::Mixer => self.draw_mixer_view(&mut body, ctx),
                    ActiveTab::SignalFlow => self.draw_routing_view(&mut body),
                }

                self.draw_footer_bar(&mut shell);
            });

        self.draw_tuning_modal(ctx, shell_rect, outer_rounding);
        self.install_resize_grips(ctx, screen_rect);
    }
}

// ---------------------------------------------------------------------------
// UI Micro-Components & Geometry Helpers
// ---------------------------------------------------------------------------

/// Lays out children inside `margin` and paints concentric rounded background/border
/// boxes behind them, avoiding `epaint`'s `PathStroke::outside` outline extrusion.
fn crisp_frame<R>(
    ui: &mut Ui,
    radius: f32,
    margin: Margin,
    fill: Color32,
    border: Option<Color32>,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    let outer_idx = ui.painter().add(egui::Shape::Noop);
    let inner_idx = ui.painter().add(egui::Shape::Noop);
    let res = Frame::none().inner_margin(margin).show(ui, add_contents);
    let rect = res.response.rect;
    match border {
        Some(b_col) => {
            ui.painter().set(
                outer_idx,
                RectShape::filled(rect, Rounding::same(radius), b_col),
            );
            let inner = rect.shrink(1.0);
            if inner.is_positive() {
                ui.painter().set(
                    inner_idx,
                    RectShape::filled(inner, Rounding::same((radius - 1.0).max(0.0)), fill),
                );
            }
        }
        None => {
            ui.painter().set(
                outer_idx,
                RectShape::filled(rect, Rounding::same(radius), fill),
            );
        }
    }
    res
}

fn draw_header_tab(ui: &mut Ui, icon: UiIcon, label: &str, active: bool) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font_bold(12.0), TEXT_PRIMARY);
    let w = 14.0 + 6.0 + galley.size().x + 20.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(w, 26.0), Sense::click());
    let p = ui.painter();

    let fg = if active { TEXT_PRIMARY } else { TEXT_SECONDARY };
    if active {
        paint_crisp_box(
            p,
            rect,
            7.0,
            BG_CONTROL,
            Some(Color32::from_rgb(58, 58, 66)),
        );
    } else if response.hovered() {
        paint_crisp_box(p, rect, 7.0, Color32::from_rgb(26, 26, 30), None);
    }

    let content_w = 14.0 + 6.0 + galley.size().x;
    let sx = rect.center().x - content_w * 0.5;
    let ic_rect = Rect::from_center_size(Pos2::new(sx + 7.0, rect.center().y), Vec2::splat(13.0));
    icons::paint_icon(p, ic_rect, icon, if active { COPPER_GLOW } else { fg });

    let g = ui
        .painter()
        .layout_no_wrap(label.to_string(), font_bold(12.0), fg);
    p.galley(
        Pos2::new(sx + 20.0, rect.center().y - g.size().y * 0.5),
        g,
        fg,
    );

    response
}

fn window_control_btn(ui: &mut Ui, icon: UiIcon, is_close: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
    let p = ui.painter();

    if response.hovered() {
        if is_close {
            paint_crisp_box(p, rect, 7.0, Color32::from_rgb(185, 44, 44), None);
        } else {
            paint_crisp_box(p, rect, 7.0, BG_CONTROL_HOVER, None);
        }
    }

    let fg = if response.hovered() {
        TEXT_PRIMARY
    } else {
        TEXT_SECONDARY
    };
    icons::paint_icon(p, rect.shrink(6.0), icon, fg);
    response
}

fn role_pill_btn(
    ui: &mut Ui,
    label: &str,
    width: f32,
    selected: bool,
    accent: Color32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 22.0), Sense::click());
    let p = ui.painter();
    let bg = if selected {
        tint_opaque(BG_STRIP, accent, 48)
    } else if response.hovered() {
        BG_CONTROL
    } else {
        BG_WELL
    };
    let border = if selected {
        tint_opaque(BG_STRIP, accent, 130)
    } else if response.hovered() {
        BORDER_STRONG
    } else {
        BORDER_SUBTLE
    };
    let fg = if selected || response.hovered() {
        TEXT_PRIMARY
    } else {
        TEXT_MUTED
    };

    paint_crisp_box(p, rect, 6.0, bg, Some(border));
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        font_mono_bold(10.0),
        fg,
    );
    response
}

fn draw_master_level_bar(
    ui: &mut Ui,
    height: f32,
    master_gain: f64,
    active_audio: bool,
    time_s: f64,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(8.0, height), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, Rounding::same(3.0), BG_WELL);

    let level = if !active_audio || master_gain <= 0.001 {
        0.05
    } else {
        let wave = 0.5 + 0.35 * (time_s * 5.2).sin() + 0.15 * (time_s * 11.7).cos();
        (wave as f32 * (master_gain as f32 / 1.5).clamp(0.1, 1.0)).clamp(0.08, 0.96)
    };

    let segs = 18usize;
    let gap = 2.0;
    let seg_h = ((rect.height() - (segs as f32 - 1.0) * gap) / segs as f32).max(2.0);
    let lit = ((segs as f32) * level).round() as usize;

    for i in 0..segs {
        let idx_from_bottom = segs - 1 - i;
        let y0 = rect.top() + i as f32 * (seg_h + gap);
        let seg_r = Rect::from_min_size(Pos2::new(rect.left() + 1.5, y0), Vec2::new(5.0, seg_h));
        let col = if idx_from_bottom >= lit {
            Color32::from_rgb(24, 24, 28)
        } else if idx_from_bottom > 15 {
            ROSE
        } else if idx_from_bottom > 12 {
            AMBER
        } else {
            COPPER
        };
        p.rect_filled(seg_r, Rounding::same(1.5), col);
    }
}

fn paint_cubic_cable(
    painter: &egui::Painter,
    from: Pos2,
    to: Pos2,
    color: Color32,
    glow: bool,
) {
    let dx = (to.x - from.x).abs().max(40.0) * 0.48;
    let c1 = Pos2::new(from.x + dx, from.y);
    let c2 = Pos2::new(to.x - dx, to.y);

    if glow {
        let glow_col = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 44);
        painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
            [from, c1, c2, to],
            false,
            Color32::TRANSPARENT,
            Stroke::new(6.0_f32, glow_col),
        ));
    }

    painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
        [from, c1, c2, to],
        false,
        Color32::TRANSPARENT,
        Stroke::new(if glow { 2.1_f32 } else { 1.4_f32 }, color),
    ));

    painter.circle_filled(from, 3.2, color);
    painter.circle_filled(to, 3.2, color);
}

fn role_accent_color(role: SpeakerRole) -> Color32 {
    match role {
        SpeakerRole::Stereo => COPPER,
        SpeakerRole::Left => Color32::from_rgb(245, 158, 11),
        SpeakerRole::Right => Color32::from_rgb(251, 146, 60),
        SpeakerRole::Center => Color32::from_rgb(228, 228, 231),
        SpeakerRole::Subwoofer => Color32::from_rgb(234, 88, 12),
        SpeakerRole::SurroundLeft | SpeakerRole::SurroundRight => Color32::from_rgb(20, 184, 166),
        SpeakerRole::Excluded => TEXT_MUTED,
    }
}

fn role_sort_order(role: SpeakerRole) -> u8 {
    match role {
        SpeakerRole::Stereo => 0,
        SpeakerRole::Left => 1,
        SpeakerRole::Right => 2,
        SpeakerRole::Center => 3,
        SpeakerRole::SurroundLeft => 4,
        SpeakerRole::SurroundRight => 5,
        SpeakerRole::Subwoofer => 6,
        SpeakerRole::Excluded => 7,
    }
}

fn format_bus_badge(
    bus_type: &str,
    is_internal: bool,
    hw_latency_ms: f64,
    compact: bool,
) -> String {
    let prefix = match bus_type {
        "hdmi" => "HDMI",
        "bluetooth" => "BT",
        "usb" => "USB",
        "spdif" => "OPT",
        "jack" => "AUX",
        "network" => "NET",
        _ if is_internal => "INT",
        _ => "OUT",
    };
    if compact || hw_latency_ms <= 0.1 {
        prefix.to_string()
    } else {
        format!("{prefix} {:.0}ms", hw_latency_ms)
    }
}

fn truncate_name(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else if max_chars <= 1 {
        "…".to_string()
    } else {
        let mut out: String = chars[..max_chars - 1].iter().collect();
        out.push('…');
        out
    }
}
