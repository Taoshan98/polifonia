mod engine;
mod icons;
mod ipc;
mod models;
mod scanner;
mod storage;
mod theme;
mod tray;
mod ui;

use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::engine::AudioEngineService;
use crate::ipc::{
    BranchSyncMode, CalibOutcomeDto, DaemonCalibState, DaemonStateSnapshot, IpcRequest, IpcResponse,
};
use crate::models::{AudioSink, SpeakerRole};
use crate::storage::StorageService;
use crate::tray::{spawn_tray, TrayCommand, TrayController};
use crate::ui::PolifoniaApp;

static SIGNAL_RECEIVED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_posix_signal(_sig: libc::c_int) {
    SIGNAL_RECEIVED.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    unsafe {
        libc::signal(
            libc::SIGINT,
            handle_posix_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            handle_posix_signal as *const () as libc::sighandler_t,
        );
    }
}

fn ensure_user_icons_installed() {
    const BRAND_SVG: &str = include_str!("../assets/io.github.taoshan98.Polifonia.svg");
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let apps_dir = home.join(".local/share/icons/hicolor/scalable/apps");
    if fs::create_dir_all(&apps_dir).is_err() {
        return;
    }

    for name in ["polifonia-studio-color.svg", "io.github.taoshan98.Polifonia.svg"] {
        let target = apps_dir.join(name);
        let up_to_date = fs::read_to_string(&target)
            .map(|existing| existing == BRAND_SVG)
            .unwrap_or(false);
        if !up_to_date {
            let _ = fs::write(&target, BRAND_SVG);
        }
    }

    let symbolic_tray = apps_dir.join("polifonia-studio-color-symbolic.svg");
    if symbolic_tray.exists() {
        let _ = fs::remove_file(symbolic_tray);
    }
}

/// Manages the lifecycle of the `--ui-window` child process so closing the window
/// tears down its Wayland/EGL surface cleanly while the audio & tray daemon stays alive.
#[derive(Clone)]
struct UiWindowProcessManager {
    child_pid: Arc<Mutex<Option<u32>>>,
    window_visible: Arc<AtomicBool>,
    tray: TrayController,
}

impl UiWindowProcessManager {
    fn new(window_visible: Arc<AtomicBool>, tray: TrayController) -> Self {
        Self {
            child_pid: Arc::new(Mutex::new(None)),
            window_visible,
            tray,
        }
    }

    fn show(&self) {
        let mut guard = self.child_pid.lock().unwrap();
        if let Some(pid) = *guard {
            let alive = unsafe { libc::kill(pid as i32, 0) == 0 };
            if alive {
                self.window_visible.store(true, Ordering::SeqCst);
                drop(guard);
                self.tray.refresh();
                return;
            }
            *guard = None;
        }

        let exe = match env::current_exe() {
            Ok(p) => p,
            Err(err) => {
                eprintln!("[DAEMON] Failed to locate executable for UI window: {err}");
                return;
            }
        };

        let mut child = match Command::new(exe)
            .arg("--ui-window")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(c) => c,
            Err(err) => {
                eprintln!("[DAEMON] Failed to spawn --ui-window child: {err}");
                return;
            }
        };

        let pid = child.id();
        *guard = Some(pid);
        self.window_visible.store(true, Ordering::SeqCst);
        drop(guard);
        self.tray.refresh();

        let slot = Arc::clone(&self.child_pid);
        let vis = Arc::clone(&self.window_visible);
        let tray = self.tray.clone();
        thread::Builder::new()
            .name("polifonia-ui-waiter".to_string())
            .spawn(move || {
                let _ = child.wait();
                let mut cleared = false;
                if let Ok(mut g) = slot.lock() {
                    if *g == Some(pid) {
                        *g = None;
                        cleared = true;
                    }
                }
                if cleared {
                    vis.store(false, Ordering::SeqCst);
                    tray.refresh();
                }
            })
            .ok();
    }

    fn hide(&self) {
        self.window_visible.store(false, Ordering::SeqCst);
        if let Ok(mut guard) = self.child_pid.lock() {
            if let Some(pid) = guard.take() {
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
            }
        }
        self.tray.refresh();
    }

    fn mark_closing(&self) {
        self.window_visible.store(false, Ordering::SeqCst);
        if let Ok(mut guard) = self.child_pid.lock() {
            *guard = None;
        }
        self.tray.refresh();
    }

    fn toggle(&self) {
        if self.window_visible.load(Ordering::SeqCst) {
            self.hide();
        } else {
            self.show();
        }
    }
}

fn spawn_hotplug_watcher(hotplug_epoch: Arc<AtomicU64>) {
    thread::Builder::new()
        .name("polifonia-hotplug".to_string())
        .spawn(move || loop {
            let child = Command::new("pactl")
                .args(["subscribe"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();

            match child {
                Ok(mut proc_handle) => {
                    if let Some(stdout) = proc_handle.stdout.take() {
                        let reader = BufReader::new(stdout);
                        let mut last_trigger = Instant::now() - Duration::from_secs(1);
                        for line in reader.lines().map_while(Result::ok) {
                            let is_relevant = line.contains("on sink ")
                                && (line.contains("'new'") || line.contains("'remove'"))
                                && !line.contains("Polifonia_");
                            if is_relevant && last_trigger.elapsed() >= Duration::from_millis(250) {
                                last_trigger = Instant::now();
                                hotplug_epoch.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                    }
                    let _ = proc_handle.wait();
                }
                Err(_) => {
                    thread::sleep(Duration::from_secs(3));
                }
            }
            thread::sleep(Duration::from_millis(750));
        })
        .ok();
}

fn spawn_signal_monitor(
    engine: Arc<AudioEngineService>,
    window_mgr: UiWindowProcessManager,
    sock_path: PathBuf,
) {
    thread::Builder::new()
        .name("polifonia-signals".to_string())
        .spawn(move || loop {
            if SIGNAL_RECEIVED.load(Ordering::Relaxed) {
                window_mgr.hide();
                engine.stop_unison_sink(true);
                let _ = fs::remove_file(&sock_path);
                thread::sleep(Duration::from_millis(120));
                std::process::exit(0);
            }
            thread::sleep(Duration::from_millis(120));
        })
        .ok();
}

fn spawn_tray_command_dispatcher(
    rx: mpsc::Receiver<TrayCommand>,
    engine: Arc<AudioEngineService>,
    tray: TrayController,
    window_mgr: UiWindowProcessManager,
    sock_path: PathBuf,
) {
    thread::Builder::new()
        .name("polifonia-tray-cmd".to_string())
        .spawn(move || {
            while let Ok(cmd) = rx.recv() {
                match cmd {
                    TrayCommand::ToggleWindow => {
                        window_mgr.toggle();
                    }
                    TrayCommand::ToggleUnison => {
                        let mut cfg = engine.config_snapshot();
                        let active_now = engine.is_running() || cfg.is_active;
                        if active_now {
                            cfg.is_active = false;
                            engine.update_config(cfg, true);
                            engine.stop_unison_sink(false);
                        } else {
                            cfg.is_active = true;
                            engine.update_config(cfg.clone(), true);
                            engine.activate_unison(Some(cfg));
                        }
                        tray.refresh();
                    }
                    TrayCommand::SetMasterVolume(gain) => {
                        engine.set_master_gain(gain, true);
                        tray.refresh();
                    }
                    TrayCommand::ToggleSpeaker(sink_name) => {
                        let mut cfg = engine.config_snapshot();
                        if let Some(sp) = cfg.speakers.get_mut(&sink_name) {
                            sp.role = if sp.role.is_enabled() {
                                SpeakerRole::Excluded
                            } else {
                                sp.effective_role()
                            };
                            sp.normalize();
                            engine.update_config(cfg, true);
                            if engine.is_running() {
                                engine.sync_active_branches(None);
                            }
                        }
                        tray.refresh();
                    }
                    TrayCommand::SetSpeakerRole(sink_name, role) => {
                        let mut cfg = engine.config_snapshot();
                        if let Some(sp) = cfg.speakers.get_mut(&sink_name) {
                            let was_enabled = sp.role.is_enabled();
                            sp.role = role;
                            sp.normalize();
                            engine.update_config(cfg, true);
                            if engine.is_running() {
                                if was_enabled && role.is_enabled() {
                                    engine.restart_single_branch(&sink_name);
                                } else {
                                    engine.sync_active_branches(None);
                                }
                            }
                        }
                        tray.refresh();
                    }
                    TrayCommand::SetSpeakerVolume(sink_name, gain) => {
                        engine.set_speaker_gain(&sink_name, gain, true);
                        tray.refresh();
                    }
                    TrayCommand::TestSpeaker(sink_name) => {
                        let cfg = engine.config_snapshot();
                        let freq = cfg
                            .speakers
                            .get(&sink_name)
                            .map(|sp| {
                                if sp.role == SpeakerRole::Subwoofer {
                                    80.0
                                } else {
                                    440.0
                                }
                            })
                            .unwrap_or(440.0);
                        let eng = Arc::clone(&engine);
                        thread::spawn(move || eng.play_test_tone(&sink_name, freq, 0.8));
                    }
                    TrayCommand::Quit => {
                        window_mgr.hide();
                        engine.stop_unison_sink(true);
                        let _ = fs::remove_file(&sock_path);
                        thread::sleep(Duration::from_millis(120));
                        std::process::exit(0);
                    }
                }
            }
        })
        .ok();
}

fn handle_ipc_request(
    req: IpcRequest,
    engine: &Arc<AudioEngineService>,
    tray: &TrayController,
    window_mgr: &UiWindowProcessManager,
    hotplug_epoch: &Arc<AtomicU64>,
    calib_state: &Arc<Mutex<DaemonCalibState>>,
) -> IpcResponse {
    match req {
        IpcRequest::Ping => IpcResponse::Ack,
        IpcRequest::ShowWindow => {
            window_mgr.show();
            IpcResponse::Ack
        }
        IpcRequest::NotifyWindowClosing => {
            window_mgr.mark_closing();
            IpcResponse::Ack
        }
        IpcRequest::GetState { include_routing } => {
            let routing_status = if include_routing {
                Some(engine.get_routing_status())
            } else {
                None
            };
            let calib = calib_state
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default();
            IpcResponse::State(Box::new(DaemonStateSnapshot {
                config: engine.config_snapshot(),
                is_running: engine.is_running(),
                routing_status,
                hotplug_epoch: hotplug_epoch.load(Ordering::Relaxed),
                calib,
            }))
        }
        IpcRequest::ScanSinks { force } => {
            let sinks = engine.get_available_sinks(force);
            IpcResponse::Sinks(sinks)
        }
        IpcRequest::ReconcileConfig { config, sync_sinks } => {
            let mut merged = engine.config_snapshot();
            for (k, sp) in config.speakers {
                let entry = merged.speakers.entry(k).or_insert_with(|| sp.clone());
                entry.sink_id = sp.sink_id;
                entry.display_name = sp.display_name;
                entry.hardware_latency_ms = sp.hardware_latency_ms;
                entry.bus_type = sp.bus_type;
                entry.is_internal = sp.is_internal;
                entry.normalize();
            }
            engine.update_config(merged.clone(), true);
            if let Some(sinks) = sync_sinks {
                if engine.is_running() {
                    let eng = Arc::clone(engine);
                    let sinks_bg: Vec<AudioSink> = sinks;
                    thread::spawn(move || eng.sync_active_branches(Some(&sinks_bg)));
                }
            }
            tray.refresh();
            IpcResponse::Config(merged)
        }
        IpcRequest::SaveSpeakerChange {
            config,
            sink_name,
            sync_mode,
        } => {
            let mut updated = engine.config_snapshot();
            if let Some(sp) = config.speakers.get(&sink_name) {
                let mut sp_norm = sp.clone();
                sp_norm.normalize();
                updated.speakers.insert(sink_name.clone(), sp_norm);
            } else {
                updated = config;
            }
            engine.update_config(updated.clone(), true);
            if engine.is_running() {
                match sync_mode {
                    BranchSyncMode::None => {}
                    BranchSyncMode::SyncAll => {
                        let eng = Arc::clone(engine);
                        thread::Builder::new()
                            .name("polifonia-sync-branches".into())
                            .spawn(move || eng.sync_active_branches(None))
                            .ok();
                    }
                    BranchSyncMode::RestartSingle => {
                        let eng = Arc::clone(engine);
                        let sn = sink_name;
                        thread::Builder::new()
                            .name("polifonia-restart-branch".into())
                            .spawn(move || eng.restart_single_branch(&sn))
                            .ok();
                    }
                }
            }
            tray.refresh();
            IpcResponse::Config(updated)
        }
        IpcRequest::SetUnison { active, config } => {
            let mut merged = engine.config_snapshot();
            for (k, sp) in config.speakers {
                merged.speakers.entry(k).or_insert(sp);
            }
            merged.is_active = active;
            engine.update_config(merged, true);
            if active {
                let _ = engine.activate_unison(None);
            } else {
                engine.stop_unison_sink(false);
            }
            tray.refresh();
            IpcResponse::UnisonState {
                is_running: engine.is_running(),
                config: engine.config_snapshot(),
            }
        }
        IpcRequest::SetMasterGain { gain, persist } => {
            engine.set_master_gain(gain, persist);
            if persist {
                tray.refresh();
            }
            IpcResponse::Ack
        }
        IpcRequest::SetSpeakerGain {
            sink_name,
            gain,
            persist,
        } => {
            engine.set_speaker_gain(&sink_name, gain, persist);
            if persist {
                tray.refresh();
            }
            IpcResponse::Ack
        }
        IpcRequest::SetSpeakerMute { sink_name, mute } => {
            engine.set_speaker_mute(&sink_name, mute);
            tray.refresh();
            IpcResponse::Ack
        }
        IpcRequest::PlayTestTone {
            sink_name,
            freq,
            duration,
        } => {
            let eng = Arc::clone(engine);
            thread::Builder::new()
                .name("polifonia-test-tone".into())
                .spawn(move || eng.play_test_tone(&sink_name, freq, duration))
                .ok();
            IpcResponse::Ack
        }
        IpcRequest::ResetAllDelays => {
            engine.reset_all_delays();
            tray.refresh();
            IpcResponse::Config(engine.config_snapshot())
        }
        IpcRequest::ReassertDefault => {
            let eng = Arc::clone(engine);
            thread::Builder::new()
                .name("polifonia-reassert-default".into())
                .spawn(move || eng.reassert_master_default())
                .ok();
            IpcResponse::Ack
        }
        IpcRequest::StartAutoAlignAll => {
            {
                let mut g = calib_state.lock().unwrap();
                if g.is_calibrating_all || g.calibrating_single_sink.is_some() {
                    return IpcResponse::Ack;
                }
                g.is_calibrating_all = true;
                g.progress = Some((0.02, "Initializing microphone calibration sweep…".to_string()));
            }
            let eng = Arc::clone(engine);
            let slot = Arc::clone(calib_state);
            let tr = tray.clone();
            thread::Builder::new()
                .name("polifonia-auto-align".into())
                .spawn(move || {
                    let mut step = 0usize;
                    let res = eng.auto_align_all_speakers(|msg| {
                        step += 1;
                        let frac = (step as f32 * 0.14).clamp(0.08, 0.94);
                        if let Ok(mut g) = slot.lock() {
                            g.progress = Some((frac, msg.to_string()));
                        }
                    });
                    if let Ok(mut g) = slot.lock() {
                        g.is_calibrating_all = false;
                        g.progress = None;
                        g.finished_seq = g.finished_seq.saturating_add(1);
                        g.last_outcome =
                            Some(CalibOutcomeDto::AutoAll(res.map(|(summary, _)| summary)));
                    }
                    tr.refresh();
                })
                .ok();
            IpcResponse::Ack
        }
        IpcRequest::StartSingleCalib { sink_name } => {
            {
                let mut g = calib_state.lock().unwrap();
                if g.is_calibrating_all || g.calibrating_single_sink.is_some() {
                    return IpcResponse::Ack;
                }
                g.calibrating_single_sink = Some(sink_name.clone());
                g.progress = Some((
                    0.05,
                    format!("Measuring acoustic propagation for {sink_name}…"),
                ));
            }
            let eng = Arc::clone(engine);
            let slot = Arc::clone(calib_state);
            let tr = tray.clone();
            thread::Builder::new()
                .name("polifonia-single-calib".into())
                .spawn(move || {
                    let mut step = 0usize;
                    let res = eng.measure_acoustic_delay(&sink_name, |msg| {
                        step += 1;
                        let frac = (step as f32 * 0.22).clamp(0.10, 0.92);
                        if let Ok(mut g) = slot.lock() {
                            g.progress = Some((frac, msg.to_string()));
                        }
                    });
                    if let Ok(delay_ms) = &res {
                        let mut cfg = eng.config_snapshot();
                        if let Some(sp) = cfg.speakers.get_mut(&sink_name) {
                            sp.delay_ms = *delay_ms;
                        }
                        eng.update_config(cfg, true);
                        if eng.is_running() {
                            eng.restart_single_branch(&sink_name);
                        }
                    }
                    if let Ok(mut g) = slot.lock() {
                        g.calibrating_single_sink = None;
                        g.progress = None;
                        g.finished_seq = g.finished_seq.saturating_add(1);
                        g.last_outcome = Some(CalibOutcomeDto::Single {
                            sink_name,
                            result: res,
                        });
                    }
                    tr.refresh();
                })
                .ok();
            IpcResponse::Ack
        }
    }
}

fn handle_ipc_client(
    mut stream: UnixStream,
    engine: Arc<AudioEngineService>,
    tray: TrayController,
    window_mgr: UiWindowProcessManager,
    hotplug_epoch: Arc<AtomicU64>,
    calib_state: Arc<Mutex<DaemonCalibState>>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let mut line = String::new();
    let read_res = {
        let mut reader = BufReader::new(&stream);
        reader.read_line(&mut line)
    };

    let response = match read_res {
        Ok(0) => return,
        Ok(_) => match serde_json::from_str::<IpcRequest>(line.trim()) {
            Ok(req) => handle_ipc_request(
                req,
                &engine,
                &tray,
                &window_mgr,
                &hotplug_epoch,
                &calib_state,
            ),
            Err(err) => IpcResponse::Error(format!("Invalid IPC payload: {err}")),
        },
        Err(err) => IpcResponse::Error(format!("IPC read error: {err}")),
    };

    if let Ok(mut payload) = serde_json::to_string(&response) {
        payload.push('\n');
        let _ = stream.write_all(payload.as_bytes());
        let _ = stream.flush();
    }
}

fn build_window_icon() -> Arc<egui::IconData> {
    let size = 128usize;
    let rgba = icons::render_fader_matrix_icon(size, true, false);
    Arc::new(egui::IconData {
        rgba,
        width: size as u32,
        height: size as u32,
    })
}

fn run_ui_window_process() -> eframe::Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title("Polifonia Audio Studio")
        .with_app_id("io.github.taoshan98.Polifonia")
        .with_decorations(false)
        .with_transparent(true)
        .with_inner_size([1180.0, 720.0])
        .with_min_inner_size([880.0, 560.0])
        .with_icon(build_window_icon());

    let native_options = eframe::NativeOptions {
        viewport,
        run_and_return: false,
        ..Default::default()
    };

    eframe::run_native(
        "Polifonia Audio Studio",
        native_options,
        Box::new(move |cc| Ok(Box::new(PolifoniaApp::new(cc)))),
    )
}

fn bind_daemon_socket() -> Option<(UnixListener, PathBuf)> {
    let primary = ipc::socket_path();
    if let Some(parent) = primary.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::remove_file(&primary);
    if let Ok(listener) = UnixListener::bind(&primary) {
        return Some((listener, primary));
    }

    let uid = unsafe { libc::getuid() };
    let fallback = PathBuf::from(format!("/tmp/polifonia-studio-{uid}.sock"));
    let _ = fs::remove_file(&fallback);
    if let Ok(listener) = UnixListener::bind(&fallback) {
        env::set_var("POLIFONIA_IPC_SOCKET", &fallback);
        return Some((listener, fallback));
    }
    None
}

fn run_daemon_process(listener: UnixListener, sock_path: PathBuf) -> eframe::Result<()> {
    install_signal_handlers();
    ensure_user_icons_installed();

    let storage = StorageService::default_store();
    let engine = AudioEngineService::new(storage);

    let window_visible = Arc::new(AtomicBool::new(true));
    let hotplug_epoch = Arc::new(AtomicU64::new(1));
    let calib_state = Arc::new(Mutex::new(DaemonCalibState::default()));

    let (cmd_tx, cmd_rx) = mpsc::channel::<TrayCommand>();
    let tray = spawn_tray(Arc::clone(&engine), cmd_tx, Arc::clone(&window_visible));
    let window_mgr = UiWindowProcessManager::new(window_visible, tray.clone());

    spawn_hotplug_watcher(Arc::clone(&hotplug_epoch));

    if engine.config_snapshot().is_active {
        let eng = Arc::clone(&engine);
        let tr = tray.clone();
        thread::Builder::new()
            .name("polifonia-autostart".to_string())
            .spawn(move || {
                if !eng.is_running() {
                    let _ = eng.activate_unison(None);
                    tr.refresh();
                }
            })
            .ok();
    }

    spawn_tray_command_dispatcher(
        cmd_rx,
        Arc::clone(&engine),
        tray.clone(),
        window_mgr.clone(),
        sock_path.clone(),
    );
    spawn_signal_monitor(Arc::clone(&engine), window_mgr.clone(), sock_path);

    // Open initial console window
    window_mgr.show();

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let eng = Arc::clone(&engine);
                let tr = tray.clone();
                let wm = window_mgr.clone();
                let hp = Arc::clone(&hotplug_epoch);
                let cs = Arc::clone(&calib_state);
                thread::Builder::new()
                    .name("polifonia-ipc-req".to_string())
                    .spawn(move || handle_ipc_client(stream, eng, tr, wm, hp, cs))
                    .ok();
            }
            Err(err) => {
                eprintln!("[DAEMON] Socket accept error: {err}");
                thread::sleep(Duration::from_millis(50));
            }
        }
    }

    Ok(())
}

fn main() -> eframe::Result<()> {
    if env::args().any(|a| a == "--ui-window") {
        return run_ui_window_process();
    }

    // Single-instance launcher check: raise/reopen existing daemon's window if already running.
    if let Ok(IpcResponse::Ack) = ipc::send_request(&IpcRequest::ShowWindow, Duration::from_millis(600)) {
        return Ok(());
    }

    if let Some((listener, sock_path)) = bind_daemon_socket() {
        run_daemon_process(listener, sock_path)
    } else {
        // Last-resort fallback if Unix sockets are completely restricted
        run_ui_window_process()
    }
}
