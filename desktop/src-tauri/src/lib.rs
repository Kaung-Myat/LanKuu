mod mirror;

use lankuu_core::network::{
    discover_devices as discover_on_lan, receive_one, send_file as send_file_over_lan,
    send_text as send_text_over_lan, spawn_discovery_responder, ReceivedPayload,
};
use lankuu_core::{Header, PayloadKind, DEFAULT_TRANSFER_PORT};
use serde::Serialize;
use std::fs;
use std::net::{TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

struct ReceiverTask {
    stop: Arc<AtomicBool>,
    thread: thread::JoinHandle<()>,
}

struct MirrorTask {
    stop: Arc<AtomicBool>,
    thread: thread::JoinHandle<()>,
}

struct DesktopState {
    receiver: Mutex<Option<ReceiverTask>>,
    mirror: Mutex<Option<MirrorTask>>,
    mirror_sessions: mirror::SessionRegistry,
    next_transfer_id: AtomicU64,
}

impl Default for DesktopState {
    fn default() -> Self {
        Self {
            receiver: Mutex::new(None),
            mirror: Mutex::new(None),
            mirror_sessions: mirror::SessionRegistry::default(),
            next_transfer_id: AtomicU64::new(1),
        }
    }
}

impl DesktopState {
    fn transfer_id(&self) -> u64 {
        self.next_transfer_id.fetch_add(1, Ordering::Relaxed)
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceView {
    name: String,
    address: String,
    port: u16,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiverView {
    running: bool,
    directory: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MirrorView {
    running: bool,
    address: String,
    port: u16,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TransferEvent {
    id: u64,
    direction: &'static str,
    kind: &'static str,
    name: String,
    transferred: u64,
    total: u64,
    status: &'static str,
    detail: Option<String>,
}

#[tauri::command]
async fn discover_devices() -> Result<Vec<DeviceView>, String> {
    tauri::async_runtime::spawn_blocking(|| discover_on_lan(Duration::from_secs(2)))
        .await
        .map_err(|error| format!("discovery task failed: {error}"))?
        .map(|devices| {
            devices
                .into_iter()
                .map(|device| DeviceView {
                    name: device.name,
                    address: device.address.to_string(),
                    port: device.port,
                })
                .collect()
        })
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn send_text(app: AppHandle, host: String, port: u16, text: String) -> Result<(), String> {
    let id = app.state::<DesktopState>().transfer_id();
    let total = text.len() as u64;
    emit_transfer(
        &app,
        TransferEvent {
            id,
            direction: "send",
            kind: "text",
            name: "Text message".to_owned(),
            transferred: 0,
            total,
            status: "transferring",
            detail: Some(host.clone()),
        },
    );

    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut emitter = ProgressEmitter::new(
            worker_app.clone(),
            id,
            "send",
            "text",
            "Text message".to_owned(),
        );
        send_text_over_lan(&host, port, &text, |sent, size| emitter.update(sent, size))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("text transfer task failed: {error}"))?;

    finish_transfer(&app, id, "send", "text", "Text message", total, result)
}

#[tauri::command]
async fn send_files(
    app: AppHandle,
    host: String,
    port: u16,
    paths: Vec<String>,
) -> Result<usize, String> {
    if paths.is_empty() {
        return Err("choose at least one file".to_owned());
    }

    let worker_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut delivered = 0_usize;
        for raw_path in paths {
            let path = PathBuf::from(&raw_path);
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("File")
                .to_owned();
            let total = fs::metadata(&path).map(|value| value.len()).unwrap_or(0);
            let id = worker_app.state::<DesktopState>().transfer_id();
            emit_transfer(
                &worker_app,
                TransferEvent {
                    id,
                    direction: "send",
                    kind: "file",
                    name: name.clone(),
                    transferred: 0,
                    total,
                    status: "transferring",
                    detail: Some(host.clone()),
                },
            );

            let mut emitter =
                ProgressEmitter::new(worker_app.clone(), id, "send", "file", name.clone());
            let result =
                send_file_over_lan(&host, port, &path, |sent, size| emitter.update(sent, size))
                    .map(|_| ())
                    .map_err(|error| error.to_string());

            let succeeded = result.is_ok();
            let completion = finish_transfer(&worker_app, id, "send", "file", &name, total, result);
            if succeeded {
                delivered += 1;
            } else {
                completion?;
            }
        }
        Ok(delivered)
    })
    .await
    .map_err(|error| format!("file transfer task failed: {error}"))?
}

#[tauri::command]
fn start_receiver(app: AppHandle, state: State<'_, DesktopState>) -> Result<ReceiverView, String> {
    let mut receiver = state
        .receiver
        .lock()
        .map_err(|_| "receiver state is unavailable".to_owned())?;
    if receiver.is_some() {
        return Ok(receiver_view(true));
    }

    let output = download_directory();
    fs::create_dir_all(&output).map_err(|error| error.to_string())?;
    let listener = TcpListener::bind(("0.0.0.0", DEFAULT_TRANSFER_PORT))
        .map_err(|error| format!("could not start receiver: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;

    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let responder_stop = Arc::clone(&stop);
    let device_name = device_name();
    let thread_app = app.clone();
    let thread_output = output.clone();
    let task = thread::spawn(move || {
        let responder =
            spawn_discovery_responder(device_name, DEFAULT_TRANSFER_PORT, responder_stop);
        emit_receiver_status(&thread_app, true);

        while !thread_stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, peer)) => {
                    handle_incoming(&thread_app, stream, peer.ip().to_string(), &thread_output);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(120));
                }
                Err(error) => {
                    let _ = thread_app.emit("app-error", format!("Receiver stopped: {error}"));
                    break;
                }
            }
        }

        thread_stop.store(true, Ordering::Relaxed);
        let _ = responder.join();
        emit_receiver_status(&thread_app, false);
    });

    *receiver = Some(ReceiverTask { stop, thread: task });
    Ok(receiver_view(true))
}

#[tauri::command]
fn stop_receiver(state: State<'_, DesktopState>) -> Result<ReceiverView, String> {
    let mut receiver = state
        .receiver
        .lock()
        .map_err(|_| "receiver state is unavailable".to_owned())?;
    if let Some(task) = receiver.take() {
        task.stop.store(true, Ordering::Relaxed);
        thread::spawn(move || {
            let _ = task.thread.join();
        });
    }
    Ok(receiver_view(false))
}

#[tauri::command]
fn receiver_status(state: State<'_, DesktopState>) -> Result<ReceiverView, String> {
    let running = state
        .receiver
        .lock()
        .map_err(|_| "receiver state is unavailable".to_owned())?
        .is_some();
    Ok(receiver_view(running))
}

#[tauri::command]
fn start_mirror_receiver(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<MirrorView, String> {
    ensure_mirror_runtime()?;

    let mut mirror = state
        .mirror
        .lock()
        .map_err(|_| "mirror state is unavailable".to_owned())?;
    if mirror
        .as_ref()
        .is_some_and(|task| task.thread.is_finished())
    {
        mirror.take();
    }
    if mirror.is_some() {
        return Ok(mirror_view(true));
    }

    let listener = TcpListener::bind(("0.0.0.0", mirror::SIGNAL_PORT)).map_err(|error| {
        format!(
            "could not start WebRTC signaling on port {}: {error}",
            mirror::SIGNAL_PORT
        )
    })?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;

    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let thread_app = app.clone();
    let thread_sessions = state.mirror_sessions.clone();
    let task = thread::spawn(move || {
        emit_mirror_status(&thread_app, true);
        if let Err(error) = mirror::run(
            listener,
            Arc::clone(&thread_stop),
            thread_sessions.clone(),
            thread_app.clone(),
        ) {
            if !thread_stop.load(Ordering::Relaxed) {
                let _ = thread_app.emit("app-error", format!("WebRTC receiver stopped: {error}"));
            }
        }
        mirror::emit_sessions(&thread_app, &thread_sessions);
        emit_mirror_status(&thread_app, false);
    });

    *mirror = Some(MirrorTask { stop, thread: task });
    Ok(mirror_view(true))
}

#[tauri::command]
fn stop_mirror_receiver(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<MirrorView, String> {
    let mut mirror = state
        .mirror
        .lock()
        .map_err(|_| "mirror state is unavailable".to_owned())?;
    if let Some(task) = mirror.take() {
        mirror::stop_all_sessions(&state.mirror_sessions);
        let _ = app.emit("mirror-stop-all", ());
        task.stop.store(true, Ordering::Relaxed);
        thread::spawn(move || {
            let _ = task.thread.join();
        });
    }
    Ok(mirror_view(false))
}

#[tauri::command]
fn mirror_sessions(
    state: State<'_, DesktopState>,
) -> Result<Vec<mirror::MirrorSessionView>, String> {
    mirror::session_views(&state.mirror_sessions)
}

#[tauri::command]
fn stop_mirror_session(
    state: State<'_, DesktopState>,
    session_id: String,
) -> Result<Vec<mirror::MirrorSessionView>, String> {
    mirror::stop_session(&state.mirror_sessions, &session_id)
}

#[tauri::command]
async fn show_mirror_session(
    state: State<'_, DesktopState>,
    session_id: String,
) -> Result<(), String> {
    let sessions = state.mirror_sessions.clone();
    tauri::async_runtime::spawn_blocking(move || mirror::show_session(&sessions, &session_id))
        .await
        .map_err(|error| format!("could not open mirror player: {error}"))?
}

#[tauri::command]
fn mirror_status(state: State<'_, DesktopState>) -> Result<MirrorView, String> {
    let mut mirror = state
        .mirror
        .lock()
        .map_err(|_| "mirror state is unavailable".to_owned())?;
    if mirror
        .as_ref()
        .is_some_and(|task| task.thread.is_finished())
    {
        mirror.take();
    }
    Ok(mirror_view(mirror.is_some()))
}

#[tauri::command]
fn open_downloads() -> Result<(), String> {
    let path = download_directory();
    fs::create_dir_all(&path).map_err(|error| error.to_string())?;

    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer");
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = Command::new("xdg-open");

    command
        .arg(path)
        .spawn()
        .map_err(|error| format!("could not open Downloads: {error}"))?;
    Ok(())
}

#[tauri::command]
fn minimize_window(window: WebviewWindow) -> Result<(), String> {
    window.minimize().map_err(|error| error.to_string())
}

#[tauri::command]
fn toggle_maximize_window(window: WebviewWindow) -> Result<(), String> {
    if window.is_maximized().map_err(|error| error.to_string())? {
        window.unmaximize().map_err(|error| error.to_string())
    } else {
        window.maximize().map_err(|error| error.to_string())
    }
}

#[tauri::command]
fn close_window(window: WebviewWindow) -> Result<(), String> {
    window.close().map_err(|error| error.to_string())
}

fn handle_incoming(app: &AppHandle, stream: std::net::TcpStream, peer: String, output: &Path) {
    let id = app.state::<DesktopState>().transfer_id();
    let name = Arc::new(Mutex::new("Incoming transfer".to_owned()));
    let kind = Arc::new(Mutex::new("file"));
    let progress_name = Arc::clone(&name);
    let progress_kind = Arc::clone(&kind);
    let progress_app = app.clone();
    let started_app = app.clone();
    let started_name = Arc::clone(&name);
    let started_kind = Arc::clone(&kind);
    let mut last_emit = Instant::now() - Duration::from_secs(1);

    let result = receive_one(
        stream,
        output,
        move |header: &Header| {
            if let Ok(mut value) = started_name.lock() {
                *value = if header.kind == PayloadKind::Text {
                    format!("Text from {peer}")
                } else {
                    header.name.clone()
                };
            }
            if let Ok(mut value) = started_kind.lock() {
                *value = if header.kind == PayloadKind::Text {
                    "text"
                } else {
                    "file"
                };
            }
            emit_transfer(
                &started_app,
                TransferEvent {
                    id,
                    direction: "receive",
                    kind: if header.kind == PayloadKind::Text {
                        "text"
                    } else {
                        "file"
                    },
                    name: if header.kind == PayloadKind::Text {
                        format!("Text from {peer}")
                    } else {
                        header.name.clone()
                    },
                    transferred: 0,
                    total: header.payload_len,
                    status: "transferring",
                    detail: Some(peer.clone()),
                },
            );
        },
        move |received, total| {
            if received == total || last_emit.elapsed() >= Duration::from_millis(100) {
                last_emit = Instant::now();
                let current_name = progress_name
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_else(|_| "Incoming transfer".to_owned());
                let current_kind = progress_kind.lock().map(|value| *value).unwrap_or("file");
                emit_transfer(
                    &progress_app,
                    TransferEvent {
                        id,
                        direction: "receive",
                        kind: current_kind,
                        name: current_name,
                        transferred: received,
                        total,
                        status: "transferring",
                        detail: None,
                    },
                );
            }
        },
    );

    match result {
        Ok(ReceivedPayload::Text { text, .. }) => emit_transfer(
            app,
            TransferEvent {
                id,
                direction: "receive",
                kind: "text",
                name: name
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_else(|_| "Text message".to_owned()),
                transferred: text.len() as u64,
                total: text.len() as u64,
                status: "complete",
                detail: Some(text),
            },
        ),
        Ok(ReceivedPayload::File { path, size, .. }) => emit_transfer(
            app,
            TransferEvent {
                id,
                direction: "receive",
                kind: "file",
                name: path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("Received file")
                    .to_owned(),
                transferred: size,
                total: size,
                status: "complete",
                detail: Some(path.display().to_string()),
            },
        ),
        Err(error) => emit_transfer(
            app,
            TransferEvent {
                id,
                direction: "receive",
                kind: "file",
                name: name
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_else(|_| "Incoming transfer".to_owned()),
                transferred: 0,
                total: 0,
                status: "failed",
                detail: Some(error.to_string()),
            },
        ),
    }
}

struct ProgressEmitter {
    app: AppHandle,
    id: u64,
    direction: &'static str,
    kind: &'static str,
    name: String,
    last_emit: Instant,
}

impl ProgressEmitter {
    fn new(
        app: AppHandle,
        id: u64,
        direction: &'static str,
        kind: &'static str,
        name: String,
    ) -> Self {
        Self {
            app,
            id,
            direction,
            kind,
            name,
            last_emit: Instant::now() - Duration::from_secs(1),
        }
    }

    fn update(&mut self, transferred: u64, total: u64) {
        if transferred == total || self.last_emit.elapsed() >= Duration::from_millis(100) {
            self.last_emit = Instant::now();
            emit_transfer(
                &self.app,
                TransferEvent {
                    id: self.id,
                    direction: self.direction,
                    kind: self.kind,
                    name: self.name.clone(),
                    transferred,
                    total,
                    status: "transferring",
                    detail: None,
                },
            );
        }
    }
}

fn finish_transfer(
    app: &AppHandle,
    id: u64,
    direction: &'static str,
    kind: &'static str,
    name: &str,
    total: u64,
    result: Result<(), String>,
) -> Result<(), String> {
    match result {
        Ok(()) => {
            emit_transfer(
                app,
                TransferEvent {
                    id,
                    direction,
                    kind,
                    name: name.to_owned(),
                    transferred: total,
                    total,
                    status: "complete",
                    detail: None,
                },
            );
            Ok(())
        }
        Err(error) => {
            emit_transfer(
                app,
                TransferEvent {
                    id,
                    direction,
                    kind,
                    name: name.to_owned(),
                    transferred: 0,
                    total,
                    status: "failed",
                    detail: Some(error.clone()),
                },
            );
            Err(error)
        }
    }
}

fn emit_transfer(app: &AppHandle, event: TransferEvent) {
    let _ = app.emit("transfer-event", event);
}

fn emit_receiver_status(app: &AppHandle, running: bool) {
    let _ = app.emit("receiver-status", receiver_view(running));
}

fn emit_mirror_status(app: &AppHandle, running: bool) {
    let _ = app.emit("mirror-status", mirror_view(running));
}

fn receiver_view(running: bool) -> ReceiverView {
    ReceiverView {
        running,
        directory: download_directory().display().to_string(),
    }
}

fn mirror_view(running: bool) -> MirrorView {
    MirrorView {
        running,
        address: local_network_address(),
        port: mirror::SIGNAL_PORT,
    }
}

fn local_network_address() -> String {
    UdpSocket::bind(("0.0.0.0", 0))
        .and_then(|socket| {
            socket.connect(("1.1.1.1", 80))?;
            socket.local_addr()
        })
        .map(|address| address.ip().to_string())
        .unwrap_or_else(|_| "Your desktop IP".to_owned())
}

fn download_directory() -> PathBuf {
    #[cfg(target_os = "windows")]
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    #[cfg(not(target_os = "windows"))]
    let home = std::env::var_os("HOME");

    home.map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads")
        .join("LanKuu")
}

fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "LanKuu Desktop".to_owned())
}

#[cfg(target_os = "linux")]
fn ensure_mirror_runtime() -> Result<(), String> {
    let player_available = Command::new("ffplay")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !player_available {
        return Err(
            "Ubuntu mirror player is missing. Install ffmpeg, then restart LanKuu.".to_owned(),
        );
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn ensure_mirror_runtime() -> Result<(), String> {
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(DesktopState::default())
        .plugin(tauri_plugin_dialog::init())
        .on_page_load(|webview, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Finished {
                if let Err(error) = webview.eval(include_str!("../../ui/app.js")) {
                    eprintln!("could not initialize LanKuu UI: {error}");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            discover_devices,
            send_text,
            send_files,
            start_receiver,
            stop_receiver,
            receiver_status,
            start_mirror_receiver,
            stop_mirror_receiver,
            mirror_status,
            mirror_sessions,
            stop_mirror_session,
            show_mirror_session,
            open_downloads,
            minimize_window,
            toggle_maximize_window,
            close_window,
        ])
        .run(tauri::generate_context!())
        .expect("error while running LanKuu desktop");
}
