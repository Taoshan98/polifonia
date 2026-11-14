use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{Handle, Icon, MenuItem, Tray, TrayService};

use crate::engine::AudioEngineService;
use crate::models::{SpeakerConfig, SpeakerRole};

#[derive(Debug, Clone)]
pub enum TrayCommand {
    ToggleWindow,
    ToggleUnison,
    SetMasterVolume(f64),
    ToggleSpeaker(String),
    SetSpeakerRole(String, SpeakerRole),
    SetSpeakerVolume(String, f64),
    TestSpeaker(String),
    Quit,
}

#[derive(Clone)]
pub struct TrayController {
    handle: Arc<Mutex<Option<Handle<PolifoniaTray>>>>,
}

impl TrayController {
    pub fn refresh(&self) {
        if let Ok(guard) = self.handle.lock() {
            if let Some(handle) = guard.as_ref() {
                handle.update(|_| {});
            }
        }
    }
}

pub struct PolifoniaTray {
    engine: Arc<AudioEngineService>,
    cmd_tx: mpsc::Sender<TrayCommand>,
    window_visible: Arc<AtomicBool>,
    icon_pixmaps: Vec<Icon>,
}

impl PolifoniaTray {
    fn send(&self, cmd: TrayCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    fn sorted_destinations(&self) -> Vec<SpeakerConfig> {
        let cfg = self.engine.config_snapshot();
        let mut items: Vec<SpeakerConfig> = cfg.speakers.into_values().collect();
        items.sort_by(|a, b| {
            let ea = a.role.is_enabled();
            let eb = b.role.is_enabled();
            eb.cmp(&ea)
                .then_with(|| a.label().to_lowercase().cmp(&b.label().to_lowercase()))
        });
        items
    }
}

impl Tray for PolifoniaTray {
    fn id(&self) -> String {
        "io.github.taoshan98.Polifonia".to_string()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::ApplicationStatus
    }

    fn title(&self) -> String {
        "Polifonia Audio Studio".to_string()
    }

    fn icon_name(&self) -> String {
        "audio-speakers".to_string()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        self.icon_pixmaps.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let cfg = self.engine.config_snapshot();
        let running = self.engine.is_running();
        let active_count = cfg
            .speakers
            .values()
            .filter(|sp| sp.role.is_enabled())
            .count();
        let status = if running { "ACTIVE" } else { "STANDBY" };
        ksni::ToolTip {
            icon_name: "audio-speakers".to_string(),
            icon_pixmap: Vec::new(),
            title: "Polifonia Audio Studio".to_string(),
            description: format!(
                "Unison {} • {} destination(s) • Master {}%",
                status,
                active_count,
                (cfg.master_volume * 100.0).round() as i32
            ),
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(TrayCommand::ToggleWindow);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let cfg = self.engine.config_snapshot();
        let is_active = self.engine.is_running() || cfg.is_active;
        let visible = self.window_visible.load(Ordering::Relaxed);
        let master_pct = (cfg.master_volume * 100.0).round() as i32;

        let mut items: Vec<MenuItem<Self>> = Vec::new();

        items.push(
            StandardItem {
                label: if visible {
                    "Hide Studio Console".to_string()
                } else {
                    "Show Studio Console".to_string()
                },
                icon_name: if visible {
                    "window-minimize".to_string()
                } else {
                    "window-restore".to_string()
                },
                activate: Box::new(|this: &mut Self| {
                    this.send(TrayCommand::ToggleWindow);
                }),
                ..Default::default()
            }
            .into(),
        );

        items.push(MenuItem::Separator);

        items.push(
            CheckmarkItem {
                label: if is_active {
                    "Unison Engine: ACTIVE".to_string()
                } else {
                    "Unison Engine: Standby".to_string()
                },
                checked: is_active,
                activate: Box::new(|this: &mut Self| {
                    this.send(TrayCommand::ToggleUnison);
                }),
                ..Default::default()
            }
            .into(),
        );

        let presets: &[(i32, f64)] = &[
            (150, 1.5),
            (100, 1.0),
            (80, 0.8),
            (60, 0.6),
            (40, 0.4),
            (20, 0.2),
            (0, 0.0),
        ];
        let mut selected_preset = presets.len();
        let mut min_diff = f64::MAX;
        for (idx, &(_, val)) in presets.iter().enumerate() {
            let diff = (cfg.master_volume - val).abs();
            if diff < min_diff && diff <= 0.05 {
                min_diff = diff;
                selected_preset = idx;
            }
        }

        let master_sub: Vec<MenuItem<Self>> = vec![RadioGroup {
            selected: selected_preset,
            select: Box::new(move |this: &mut Self, idx: usize| {
                let values = [1.5, 1.0, 0.8, 0.6, 0.4, 0.2, 0.0];
                if let Some(&gain) = values.get(idx) {
                    this.send(TrayCommand::SetMasterVolume(gain));
                }
            }),
            options: presets
                .iter()
                .map(|&(pct, _)| RadioItem {
                    label: format!("{}%", pct),
                    ..Default::default()
                })
                .collect(),
        }
        .into()];

        items.push(
            SubMenu {
                label: format!("Master Volume ({}%)", master_pct),
                icon_name: "audio-volume-high".to_string(),
                submenu: master_sub,
                ..Default::default()
            }
            .into(),
        );

        items.push(MenuItem::Separator);

        let destinations = self.sorted_destinations();
        let mut dest_sub: Vec<MenuItem<Self>> = Vec::new();
        if destinations.is_empty() {
            dest_sub.push(
                StandardItem {
                    label: "No audio destinations detected".to_string(),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
        } else {
            for sp in destinations {
                let sink_name = sp.sink_name.clone();
                let enabled = sp.role.is_enabled();
                let vol_pct = (sp.volume_gain * 100.0).round() as i32;
                let bus = if sp.bus_type.is_empty() {
                    "OUT"
                } else {
                    sp.bus_type.as_str()
                };
                let title = if enabled {
                    format!("{} [{} • {}%]", sp.label(), sp.role.short_label(), vol_pct)
                } else {
                    format!("{} [{} • OFF]", sp.label(), bus)
                };

                let sn_toggle = sink_name.clone();
                let sn_test = sink_name.clone();

                let mut sub_entries: Vec<MenuItem<Self>> = vec![
                    CheckmarkItem {
                        label: "Active in Unison".to_string(),
                        checked: enabled,
                        activate: Box::new(move |this: &mut Self| {
                            this.send(TrayCommand::ToggleSpeaker(sn_toggle.clone()));
                        }),
                        ..Default::default()
                    }
                    .into(),
                    MenuItem::Separator,
                ];

                let roles = [
                    (SpeakerRole::Stereo, "Stereo (L+R)"),
                    (SpeakerRole::Left, "Left Front (L)"),
                    (SpeakerRole::Right, "Right Front (R)"),
                    (SpeakerRole::Subwoofer, "Subwoofer (LFE)"),
                    (SpeakerRole::Center, "Center (C)"),
                ];
                let selected_role = roles
                    .iter()
                    .position(|(r, _)| *r == sp.role)
                    .unwrap_or(usize::MAX);
                let sn_role = sink_name.clone();
                sub_entries.push(
                    RadioGroup {
                        selected: selected_role,
                        select: Box::new(move |this: &mut Self, idx: usize| {
                            let all_roles = [
                                SpeakerRole::Stereo,
                                SpeakerRole::Left,
                                SpeakerRole::Right,
                                SpeakerRole::Subwoofer,
                                SpeakerRole::Center,
                            ];
                            if let Some(&role) = all_roles.get(idx) {
                                this.send(TrayCommand::SetSpeakerRole(sn_role.clone(), role));
                            }
                        }),
                        options: roles
                            .iter()
                            .map(|(_, label)| RadioItem {
                                label: (*label).to_string(),
                                ..Default::default()
                            })
                            .collect(),
                    }
                    .into(),
                );

                sub_entries.push(MenuItem::Separator);

                let vol_presets: &[(i32, f64)] =
                    &[(150, 1.5), (100, 1.0), (80, 0.8), (60, 0.6), (40, 0.4), (0, 0.0)];
                let sn_vol = sink_name.clone();
                let vol_items: Vec<MenuItem<Self>> = vol_presets
                    .iter()
                    .map(|&(pct, gain)| {
                        let sn = sn_vol.clone();
                        StandardItem {
                            label: format!("Set Volume {}%", pct),
                            activate: Box::new(move |this: &mut Self| {
                                this.send(TrayCommand::SetSpeakerVolume(sn.clone(), gain));
                            }),
                            ..Default::default()
                        }
                        .into()
                    })
                    .collect();

                sub_entries.push(
                    SubMenu {
                        label: format!("Channel Gain ({}%)", vol_pct),
                        submenu: vol_items,
                        ..Default::default()
                    }
                    .into(),
                );

                sub_entries.push(
                    StandardItem {
                        label: "Play Test Tone".to_string(),
                        icon_name: "media-playback-start".to_string(),
                        activate: Box::new(move |this: &mut Self| {
                            this.send(TrayCommand::TestSpeaker(sn_test.clone()));
                        }),
                        ..Default::default()
                    }
                    .into(),
                );

                dest_sub.push(
                    SubMenu {
                        label: title,
                        submenu: sub_entries,
                        ..Default::default()
                    }
                    .into(),
                );
            }
        }

        items.push(
            SubMenu {
                label: "Destinations".to_string(),
                icon_name: "audio-card".to_string(),
                submenu: dest_sub,
                ..Default::default()
            }
            .into(),
        );

        items.push(MenuItem::Separator);

        items.push(
            StandardItem {
                label: "Quit Polifonia".to_string(),
                icon_name: "application-exit".to_string(),
                activate: Box::new(|this: &mut Self| {
                    this.send(TrayCommand::Quit);
                }),
                ..Default::default()
            }
            .into(),
        );

        items
    }
}

/// Generate a crisp 32x32 ARGB32 icon programmatically so the binary requires no external runtime assets.
fn build_tray_icon_pixmap() -> Vec<Icon> {
    let size: i32 = 32;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    let cx = 15.5f32;
    let cy = 15.5f32;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();

            let mut a = 0u8;
            let mut r = 0u8;
            let mut g = 0u8;
            let mut b = 0u8;

            // Dark circular badge background
            if dist <= 14.5 {
                let edge = (14.5 - dist).clamp(0.0, 1.0);
                a = (edge * 245.0) as u8;
                r = 19;
                g = 26;
                b = 38;
            }

            // Outer cyan ring
            if (dist - 12.2).abs() <= 1.1 {
                let intensity = (1.1 - (dist - 12.2).abs()).clamp(0.0, 1.0);
                a = a.max((intensity * 255.0) as u8);
                r = ((1.0 - intensity) * r as f32 + intensity * 56.0) as u8;
                g = ((1.0 - intensity) * g as f32 + intensity * 189.0) as u8;
                b = ((1.0 - intensity) * b as f32 + intensity * 248.0) as u8;
            }

            // Mid concentric acoustic wave
            if (dist - 7.6).abs() <= 1.05 {
                let intensity = (1.05 - (dist - 7.6).abs()).clamp(0.0, 1.0);
                a = a.max((intensity * 255.0) as u8);
                r = ((1.0 - intensity) * r as f32 + intensity * 125.0) as u8;
                g = ((1.0 - intensity) * g as f32 + intensity * 211.0) as u8;
                b = ((1.0 - intensity) * b as f32 + intensity * 252.0) as u8;
            }

            // Center glowing core dot
            if dist <= 3.2 {
                let intensity = ((3.2 - dist) / 1.0).clamp(0.0, 1.0);
                a = 255;
                r = ((1.0 - intensity) * r as f32 + intensity * 56.0) as u8;
                g = ((1.0 - intensity) * g as f32 + intensity * 224.0) as u8;
                b = ((1.0 - intensity) * b as f32 + intensity * 248.0) as u8;
            }

            // Network byte order ARGB32
            data.push(a);
            data.push(r);
            data.push(g);
            data.push(b);
        }
    }

    vec![Icon {
        width: size,
        height: size,
        data,
    }]
}

pub fn spawn_tray(
    engine: Arc<AudioEngineService>,
    cmd_tx: mpsc::Sender<TrayCommand>,
    window_visible: Arc<AtomicBool>,
) -> TrayController {
    let handle_slot: Arc<Mutex<Option<Handle<PolifoniaTray>>>> = Arc::new(Mutex::new(None));
    let handle_slot_bg = Arc::clone(&handle_slot);

    let tray = PolifoniaTray {
        engine,
        cmd_tx,
        window_visible,
        icon_pixmaps: build_tray_icon_pixmap(),
    };

    thread::Builder::new()
        .name("polifonia-tray".to_string())
        .spawn(move || {
            let service = TrayService::new(tray);
            let handle = service.handle();
            if let Ok(mut guard) = handle_slot_bg.lock() {
                *guard = Some(handle);
            }
            if let Err(err) = service.run() {
                eprintln!("[TRAY] StatusNotifier service exited: {err}");
            }
        })
        .ok();

    TrayController {
        handle: handle_slot,
    }
}
