//! Lightweight local Unix-domain-socket JSON IPC between the persistent Polifonia
//! audio/tray daemon and the ephemeral `--ui-window` console process.

use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::engine::RoutingStatus;
use crate::models::{AudioSink, SystemConfig};

/// Determines the per-user Unix domain socket path inside `$XDG_RUNTIME_DIR` (with `/tmp` fallback).
pub fn socket_path() -> PathBuf {
    if let Some(explicit) = env::var_os("POLIFONIA_IPC_SOCKET") {
        let p = PathBuf::from(explicit);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    if let Some(dir) = env::var_os("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(dir);
        if !p.as_os_str().is_empty() {
            return p.join("polifonia-studio.sock");
        }
    }
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/polifonia-studio-{uid}.sock"))
}

/// Strategy for syncing PipeWire loopback branches after a speaker configuration change.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BranchSyncMode {
    None,
    SyncAll,
    RestartSingle,
}

/// Serializable calibration completion outcome reported back to the UI client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CalibOutcomeDto {
    AutoAll(Result<String, String>),
    Single {
        sink_name: String,
        result: Result<f64, String>,
    },
}

/// Live acoustic calibration status tracked in the daemon.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DaemonCalibState {
    pub is_calibrating_all: bool,
    pub calibrating_single_sink: Option<String>,
    pub progress: Option<(f32, String)>,
    pub finished_seq: u64,
    pub last_outcome: Option<CalibOutcomeDto>,
}

/// Complete state snapshot fetched by the UI window poller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStateSnapshot {
    pub config: SystemConfig,
    pub is_running: bool,
    pub routing_status: Option<RoutingStatus>,
    pub hotplug_epoch: u64,
    pub calib: DaemonCalibState,
}

/// Requests sent over the Unix domain socket from UI / secondary launcher instances to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcRequest {
    Ping,
    ShowWindow,
    NotifyWindowClosing,
    GetState {
        include_routing: bool,
    },
    ScanSinks {
        force: bool,
    },
    ReconcileConfig {
        config: SystemConfig,
        sync_sinks: Option<Vec<AudioSink>>,
    },
    SaveSpeakerChange {
        config: SystemConfig,
        sink_name: String,
        sync_mode: BranchSyncMode,
    },
    SetUnison {
        active: bool,
        config: SystemConfig,
    },
    SetMasterGain {
        gain: f64,
        persist: bool,
    },
    SetSpeakerGain {
        sink_name: String,
        gain: f64,
        persist: bool,
    },
    SetSpeakerMute {
        sink_name: String,
        mute: bool,
    },
    PlayTestTone {
        sink_name: String,
        freq: f64,
        duration: f64,
    },
    ResetAllDelays,
    ReassertDefault,
    StartAutoAlignAll,
    StartSingleCalib {
        sink_name: String,
    },
}

/// Responses returned synchronously by the daemon for each [`IpcRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcResponse {
    Ack,
    State(Box<DaemonStateSnapshot>),
    Sinks(Vec<AudioSink>),
    Config(SystemConfig),
    UnisonState {
        is_running: bool,
        config: SystemConfig,
    },
    Error(String),
}

/// Sends a single JSON-line request to an explicit socket path and waits for its response.
pub fn send_request_to(
    path: &Path,
    req: &IpcRequest,
    timeout: Duration,
) -> Result<IpcResponse, String> {
    let mut stream = UnixStream::connect(path).map_err(|e| format!("IPC connect error: {e}"))?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let payload = serde_json::to_string(req).map_err(|e| format!("IPC serialize error: {e}"))?;
    stream
        .write_all(payload.as_bytes())
        .map_err(|e| format!("IPC write error: {e}"))?;
    stream
        .write_all(b"\n")
        .map_err(|e| format!("IPC write newline error: {e}"))?;
    stream
        .flush()
        .map_err(|e| format!("IPC flush error: {e}"))?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let n = reader
        .read_line(&mut line)
        .map_err(|e| format!("IPC read error: {e}"))?;
    if n == 0 {
        return Err("IPC connection closed before response".to_string());
    }

    serde_json::from_str::<IpcResponse>(line.trim())
        .map_err(|e| format!("IPC deserialize error: {e}"))
}

/// Sends a single JSON-line request to the active daemon socket returned by [`socket_path`].
pub fn send_request(req: &IpcRequest, timeout: Duration) -> Result<IpcResponse, String> {
    send_request_to(&socket_path(), req, timeout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::net::UnixListener;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn test_ipc_unix_socket_roundtrip() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = PathBuf::from(format!("/tmp/polifonia_ipc_test_{nanos}.sock"));
        let _ = fs::remove_file(&path);

        let listener = UnixListener::bind(&path).expect("bind test unix socket");
        let server = thread::spawn(move || {
            for stream in listener.incoming().take(2).flatten() {
                let mut reader = BufReader::new(&stream);
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let req: IpcRequest = serde_json::from_str(line.trim()).unwrap();
                let resp = match req {
                    IpcRequest::Ping => IpcResponse::Ack,
                    IpcRequest::GetState { .. } => {
                        IpcResponse::State(Box::new(DaemonStateSnapshot {
                            config: SystemConfig::default(),
                            is_running: true,
                            routing_status: None,
                            hotplug_epoch: 7,
                            calib: DaemonCalibState::default(),
                        }))
                    }
                    _ => IpcResponse::Error("unexpected".to_string()),
                };
                let mut out = serde_json::to_string(&resp).unwrap();
                out.push('\n');
                let mut writer = stream;
                let _ = writer.write_all(out.as_bytes());
                let _ = writer.flush();
            }
        });

        let r1 = send_request_to(&path, &IpcRequest::Ping, Duration::from_secs(2)).unwrap();
        assert!(matches!(r1, IpcResponse::Ack));

        let r2 = send_request_to(
            &path,
            &IpcRequest::GetState {
                include_routing: false,
            },
            Duration::from_secs(2),
        )
        .unwrap();
        match r2 {
            IpcResponse::State(snap) => {
                assert!(snap.is_running);
                assert_eq!(snap.hotplug_epoch, 7);
            }
            other => panic!("Unexpected IPC response: {other:?}"),
        }

        let _ = server.join();
        let _ = fs::remove_file(&path);
    }
}
