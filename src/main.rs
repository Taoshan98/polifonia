mod engine;
mod models;
mod scanner;
mod storage;
mod tray;
mod ui;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use eframe::egui;

use crate::engine::AudioEngineService;
use crate::models::SpeakerRole;
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

fn spawn_signal_monitor(
    engine: Arc<AudioEngineService>,
    quit_requested: Arc<AtomicBool>,
    egui_ctx_slot: Arc<Mutex<Option<egui::Context>>>,
) {
    thread::Builder::new()
        .name("polifonia-signals".to_string())
        .spawn(move || loop {
            if SIGNAL_RECEIVED.load(Ordering::Relaxed) {
                quit_requested.store(true, Ordering::Relaxed);
                engine.stop_unison_sink(true);
                if let Ok(guard) = egui_ctx_slot.lock() {
                    if let Some(ctx) = guard.as_ref() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        ctx.request_repaint();
                    }
                }
                thread::sleep(Duration::from_millis(350));
                std::process::exit(0);
            }
            thread::sleep(Duration::from_millis(200));
        })
        .ok();
}

fn spawn_tray_command_dispatcher(
    rx: mpsc::Receiver<TrayCommand>,
    engine: Arc<AudioEngineService>,
    tray: TrayController,
    window_visible: Arc<AtomicBool>,
    quit_requested: Arc<AtomicBool>,
    egui_ctx_slot: Arc<Mutex<Option<egui::Context>>>,
    ui_sync_needed: Arc<AtomicBool>,
) {
    let wake_ui = move |ctx_slot: &Arc<Mutex<Option<egui::Context>>>| {
        if let Ok(guard) = ctx_slot.lock() {
            if let Some(ctx) = guard.as_ref() {
                ctx.request_repaint();
            }
        }
    };

    thread::Builder::new()
        .name("polifonia-tray-cmd".to_string())
        .spawn(move || {
            while let Ok(cmd) = rx.recv() {
                match cmd {
                    TrayCommand::ToggleWindow => {
                        let next = !window_visible.load(Ordering::Relaxed);
                        window_visible.store(next, Ordering::Relaxed);
                        if let Ok(guard) = egui_ctx_slot.lock() {
                            if let Some(ctx) = guard.as_ref() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(next));
                                if next {
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                                }
                                ctx.request_repaint();
                            }
                        }
                        tray.refresh();
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
                        ui_sync_needed.store(true, Ordering::Relaxed);
                        wake_ui(&egui_ctx_slot);
                        tray.refresh();
                    }
                    TrayCommand::SetMasterVolume(gain) => {
                        engine.set_master_gain(gain, true);
                        ui_sync_needed.store(true, Ordering::Relaxed);
                        wake_ui(&egui_ctx_slot);
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
                        ui_sync_needed.store(true, Ordering::Relaxed);
                        wake_ui(&egui_ctx_slot);
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
                        ui_sync_needed.store(true, Ordering::Relaxed);
                        wake_ui(&egui_ctx_slot);
                        tray.refresh();
                    }
                    TrayCommand::SetSpeakerVolume(sink_name, gain) => {
                        engine.set_speaker_gain(&sink_name, gain, true);
                        ui_sync_needed.store(true, Ordering::Relaxed);
                        wake_ui(&egui_ctx_slot);
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
                        quit_requested.store(true, Ordering::Relaxed);
                        engine.stop_unison_sink(true);
                        if let Ok(guard) = egui_ctx_slot.lock() {
                            if let Some(ctx) = guard.as_ref() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                ctx.request_repaint();
                            }
                        }
                        thread::sleep(Duration::from_millis(350));
                        std::process::exit(0);
                    }
                }
            }
        })
        .ok();
}

fn build_window_icon() -> Arc<egui::IconData> {
    let size = 32usize;
    let mut rgba = Vec::with_capacity(size * size * 4);
    let cx = 15.5f32;
    let cy = 15.5f32;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let mut r = 0u8;
            let mut g = 0u8;
            let mut b = 0u8;
            let mut a = 0u8;

            if dist <= 14.8 {
                let edge = (14.8 - dist).clamp(0.0, 1.0);
                a = (edge * 250.0) as u8;
                r = 16;
                g = 23;
                b = 36;
            }
            if (dist - 12.2).abs() <= 1.1 {
                let k = (1.1 - (dist - 12.2).abs()).clamp(0.0, 1.0);
                a = a.max((k * 255.0) as u8);
                r = ((1.0 - k) * r as f32 + k * 56.0) as u8;
                g = ((1.0 - k) * g as f32 + k * 189.0) as u8;
                b = ((1.0 - k) * b as f32 + k * 248.0) as u8;
            }
            if (dist - 7.6).abs() <= 1.05 {
                let k = (1.05 - (dist - 7.6).abs()).clamp(0.0, 1.0);
                a = a.max((k * 255.0) as u8);
                r = ((1.0 - k) * r as f32 + k * 125.0) as u8;
                g = ((1.0 - k) * g as f32 + k * 211.0) as u8;
                b = ((1.0 - k) * b as f32 + k * 252.0) as u8;
            }
            if dist <= 3.2 {
                let k = ((3.2 - dist) / 1.0).clamp(0.0, 1.0);
                a = 255;
                r = ((1.0 - k) * r as f32 + k * 56.0) as u8;
                g = ((1.0 - k) * g as f32 + k * 224.0) as u8;
                b = ((1.0 - k) * b as f32 + k * 248.0) as u8;
            }
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }

    Arc::new(egui::IconData {
        rgba,
        width: size as u32,
        height: size as u32,
    })
}

fn main() -> eframe::Result<()> {
    install_signal_handlers();

    let storage = StorageService::default_store();
    let engine = AudioEngineService::new(storage);

    let window_visible = Arc::new(AtomicBool::new(true));
    let quit_requested = Arc::new(AtomicBool::new(false));
    let ui_sync_needed = Arc::new(AtomicBool::new(false));
    let egui_ctx_slot: Arc<Mutex<Option<egui::Context>>> = Arc::new(Mutex::new(None));

    let (cmd_tx, cmd_rx) = mpsc::channel::<TrayCommand>();
    let tray = spawn_tray(Arc::clone(&engine), cmd_tx, Arc::clone(&window_visible));

    spawn_tray_command_dispatcher(
        cmd_rx,
        Arc::clone(&engine),
        tray.clone(),
        Arc::clone(&window_visible),
        Arc::clone(&quit_requested),
        Arc::clone(&egui_ctx_slot),
        Arc::clone(&ui_sync_needed),
    );
    spawn_signal_monitor(
        Arc::clone(&engine),
        Arc::clone(&quit_requested),
        Arc::clone(&egui_ctx_slot),
    );

    let viewport = egui::ViewportBuilder::default()
        .with_title("Polifonia Audio Studio")
        .with_app_id("io.github.taoshan98.Polifonia")
        .with_inner_size([1080.0, 680.0])
        .with_min_inner_size([820.0, 520.0])
        .with_icon(build_window_icon());

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Polifonia Audio Studio",
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(PolifoniaApp::new(
                cc,
                Arc::clone(&engine),
                tray,
                window_visible,
                quit_requested,
                egui_ctx_slot,
                ui_sync_needed,
            )))
        }),
    )
}
