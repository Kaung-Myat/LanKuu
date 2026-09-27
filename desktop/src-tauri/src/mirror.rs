use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex as AsyncMutex;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_H264};
use webrtc::api::APIBuilder;
use webrtc::data_channel::RTCDataChannel;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtp::codecs::h264::H264Packet;
use webrtc::rtp::packetizer::Depacketizer;
use webrtc::rtp_transceiver::rtp_codec::{
    RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType,
};

pub const SIGNAL_PORT: u16 = 45_456;
pub const MAX_CONCURRENT_SESSIONS: usize = 4;
const MAX_SIGNAL_BYTES: usize = 1_048_576;
const SIGNAL_TIMEOUT: Duration = Duration::from_secs(25);
const MAX_BOOTSTRAP_FRAMES: usize = 180;
const MAX_BOOTSTRAP_BYTES: usize = 8 * 1024 * 1024;
static NEXT_FALLBACK_SESSION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Deserialize, Serialize)]
pub struct MirrorDescription {
    #[serde(rename = "type")]
    pub sdp_type: String,
    pub sdp: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MirrorSessionView {
    pub id: String,
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub address: String,
    pub status: String,
    pub started_at_ms: u64,
}

#[derive(Default)]
struct H264ParameterSets {
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
}

struct PreparedH264Frame {
    data: Vec<u8>,
    key_frame: bool,
    codec: String,
    nalu_types: Vec<u8>,
}

struct PlayerFeed {
    input: ChildStdin,
    bootstrap_frames: Vec<Vec<u8>>,
    bootstrap_bytes: usize,
}

impl PlayerFeed {
    fn new(input: ChildStdin) -> Self {
        Self {
            input,
            bootstrap_frames: Vec::new(),
            bootstrap_bytes: 0,
        }
    }

    fn cache(&mut self, frame: &PreparedH264Frame) {
        if frame.key_frame {
            self.bootstrap_frames.clear();
            self.bootstrap_bytes = 0;
        } else if self.bootstrap_frames.is_empty() {
            return;
        }

        if self.bootstrap_frames.len() >= MAX_BOOTSTRAP_FRAMES
            || self.bootstrap_bytes + frame.data.len() > MAX_BOOTSTRAP_BYTES
        {
            self.bootstrap_frames.clear();
            self.bootstrap_bytes = 0;
            return;
        }
        self.bootstrap_bytes += frame.data.len();
        self.bootstrap_frames.push(frame.data.clone());
    }

    fn has_bootstrap(&self) -> bool {
        !self.bootstrap_frames.is_empty()
    }

    fn install(&mut self, mut input: ChildStdin) -> Result<(), String> {
        if self.bootstrap_frames.is_empty() {
            return Err("video key frame is not ready yet; try again in a moment".to_owned());
        }
        for frame in &self.bootstrap_frames {
            input
                .write_all(frame)
                .map_err(|error| format!("could not start mirror video: {error}"))?;
        }
        input
            .flush()
            .map_err(|error| format!("could not start mirror video: {error}"))?;
        self.input = input;
        Ok(())
    }
}

struct MirrorSessionControl {
    view: MirrorSessionView,
    stop: Arc<AtomicBool>,
    reopen_player: Arc<AtomicBool>,
    request_key_frame: Arc<AtomicBool>,
    reopen_result: Arc<Mutex<Option<mpsc::SyncSender<Result<(), String>>>>>,
}

#[derive(Clone, Default)]
pub struct SessionRegistry {
    inner: Arc<Mutex<HashMap<String, MirrorSessionControl>>>,
}

struct IncomingOffer {
    description: MirrorDescription,
    requested_session_id: String,
    device_id: String,
    device_name: String,
    platform: String,
}

struct RegisteredSession {
    id: String,
    view: MirrorSessionView,
    stop: Arc<AtomicBool>,
    reopen_player: Arc<AtomicBool>,
    request_key_frame: Arc<AtomicBool>,
    reopen_result: Arc<Mutex<Option<mpsc::SyncSender<Result<(), String>>>>>,
}

impl SessionRegistry {
    fn register(
        &self,
        offer: &IncomingOffer,
        address: String,
    ) -> Result<RegisteredSession, String> {
        let mut sessions = self
            .inner
            .lock()
            .map_err(|_| "mirror session registry is unavailable".to_owned())?;
        if sessions.len() >= MAX_CONCURRENT_SESSIONS {
            return Err(format!(
                "mirror receiver is full (maximum {MAX_CONCURRENT_SESSIONS} devices)"
            ));
        }

        let requested = safe_identifier(&offer.requested_session_id);
        let base = if requested.is_empty() {
            fallback_session_id()
        } else {
            requested
        };
        let mut id = base.clone();
        while sessions.contains_key(&id) {
            id = format!(
                "{}-{}",
                base,
                NEXT_FALLBACK_SESSION_ID.fetch_add(1, Ordering::Relaxed)
            );
        }

        let stop = Arc::new(AtomicBool::new(false));
        let reopen_player = Arc::new(AtomicBool::new(false));
        let request_key_frame = Arc::new(AtomicBool::new(false));
        let reopen_result = Arc::new(Mutex::new(None));
        let view = MirrorSessionView {
            id: id.clone(),
            device_id: limited_text(&offer.device_id, "unknown-device", 96),
            device_name: limited_text(&offer.device_name, "Nearby device", 80),
            platform: limited_text(&offer.platform, "unknown", 24).to_lowercase(),
            address,
            status: "negotiating".to_owned(),
            started_at_ms: now_millis(),
        };
        sessions.insert(
            id.clone(),
            MirrorSessionControl {
                view: view.clone(),
                stop: Arc::clone(&stop),
                reopen_player: Arc::clone(&reopen_player),
                request_key_frame: Arc::clone(&request_key_frame),
                reopen_result: Arc::clone(&reopen_result),
            },
        );
        Ok(RegisteredSession {
            id,
            view,
            stop,
            reopen_player,
            request_key_frame,
            reopen_result,
        })
    }

    fn update_status(&self, id: &str, status: &str) -> Result<(), String> {
        if !matches!(
            status,
            "negotiating" | "connecting" | "live" | "reconnecting" | "stopping" | "failed"
        ) {
            return Err("invalid mirror session status".to_owned());
        }
        let mut sessions = self
            .inner
            .lock()
            .map_err(|_| "mirror session registry is unavailable".to_owned())?;
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| "mirror session is no longer active".to_owned())?;
        session.view.status = status.to_owned();
        Ok(())
    }

    fn remove(&self, id: &str) {
        if let Ok(mut sessions) = self.inner.lock() {
            if let Some(session) = sessions.remove(id) {
                session.stop.store(true, Ordering::Relaxed);
            }
        }
    }

    fn views(&self) -> Result<Vec<MirrorSessionView>, String> {
        let sessions = self
            .inner
            .lock()
            .map_err(|_| "mirror session registry is unavailable".to_owned())?;
        let mut views = sessions
            .values()
            .map(|session| session.view.clone())
            .collect::<Vec<_>>();
        views.sort_by_key(|session| session.started_at_ms);
        Ok(views)
    }

    fn stop_one(&self, id: &str) -> Result<(), String> {
        let mut sessions = self
            .inner
            .lock()
            .map_err(|_| "mirror session registry is unavailable".to_owned())?;
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| "mirror session is no longer active".to_owned())?;
        session.view.status = "stopping".to_owned();
        session.stop.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn show_one(&self, id: &str) -> Result<(), String> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let result_handle = {
            let sessions = self
                .inner
                .lock()
                .map_err(|_| "mirror session registry is unavailable".to_owned())?;
            let session = sessions
                .get(id)
                .ok_or_else(|| "mirror session is no longer active".to_owned())?;
            if session.view.status != "live" && session.view.status != "reconnecting" {
                return Err("mirror session is not ready to play".to_owned());
            }
            let result_handle = Arc::clone(&session.reopen_result);
            let mut result = result_handle
                .lock()
                .map_err(|_| "mirror player request is unavailable".to_owned())?;
            if result.is_some() {
                return Err("mirror player is already opening".to_owned());
            }
            *result = Some(sender);
            session.reopen_player.store(true, Ordering::Relaxed);
            session.request_key_frame.store(true, Ordering::Relaxed);
            drop(result);
            result_handle
        };
        match receiver.recv_timeout(Duration::from_secs(4)) {
            Ok(result) => result,
            Err(_) => {
                if let Ok(mut result) = result_handle.lock() {
                    result.take();
                }
                Err("timed out while opening the mirror player".to_owned())
            }
        }
    }

    fn stop_all(&self) {
        if let Ok(mut sessions) = self.inner.lock() {
            for session in sessions.values_mut() {
                session.view.status = "stopping".to_owned();
                session.stop.store(true, Ordering::Relaxed);
            }
        }
    }
}

pub fn session_views(registry: &SessionRegistry) -> Result<Vec<MirrorSessionView>, String> {
    registry.views()
}

pub fn stop_session(
    registry: &SessionRegistry,
    session_id: &str,
) -> Result<Vec<MirrorSessionView>, String> {
    registry.stop_one(session_id)?;
    registry.views()
}

pub fn show_session(registry: &SessionRegistry, session_id: &str) -> Result<(), String> {
    registry.show_one(session_id)
}

pub fn stop_all_sessions(registry: &SessionRegistry) {
    registry.stop_all();
}

pub fn emit_sessions(app: &AppHandle, registry: &SessionRegistry) {
    if let Ok(sessions) = registry.views() {
        let _ = app.emit("mirror-sessions", sessions);
    }
}

pub fn run(
    listener: TcpListener,
    receiver_stop: Arc<AtomicBool>,
    sessions: SessionRegistry,
    app: AppHandle,
) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("could not start the native WebRTC runtime: {error}"))?;
    let mut workers = Vec::new();

    while !receiver_stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut stream, peer_address)) => {
                let worker_stop = Arc::clone(&receiver_stop);
                let worker_sessions = sessions.clone();
                let worker_app = app.clone();
                let runtime_handle = runtime.handle().clone();
                workers.push(thread::spawn(move || {
                    if let Err(error) = configure_signal_stream(&stream) {
                        let _ = write_error(&mut stream, &error);
                        return;
                    }
                    let offer = match read_offer(&mut stream) {
                        Ok(offer) => offer,
                        Err(error) => {
                            let _ = write_error(&mut stream, &error);
                            return;
                        }
                    };
                    let registered =
                        match worker_sessions.register(&offer, peer_address.ip().to_string()) {
                            Ok(session) => session,
                            Err(error) => {
                                let _ = write_error(&mut stream, &error);
                                return;
                            }
                        };
                    emit_sessions(&worker_app, &worker_sessions);

                    let result = runtime_handle.block_on(run_native_session(
                        &mut stream,
                        offer.description,
                        &registered,
                        Arc::clone(&worker_stop),
                        worker_sessions.clone(),
                        worker_app.clone(),
                    ));
                    if let Err(error) = result {
                        let _ = write_error(&mut stream, &error);
                        let _ = worker_sessions.update_status(&registered.id, "failed");
                        emit_sessions(&worker_app, &worker_sessions);
                        let _ = worker_app.emit(
                            "app-error",
                            format!("{} mirror failed: {error}", registered.view.device_name),
                        );
                        thread::sleep(Duration::from_millis(600));
                    }
                    worker_sessions.remove(&registered.id);
                    emit_sessions(&worker_app, &worker_sessions);
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(60));
            }
            Err(error) => return Err(format!("could not accept WebRTC signaling: {error}")),
        }
    }

    sessions.stop_all();
    let _ = app.emit("mirror-stop-all", ());
    emit_sessions(&app, &sessions);
    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

async fn run_native_session(
    signal: &mut TcpStream,
    offer: MirrorDescription,
    session: &RegisteredSession,
    receiver_stop: Arc<AtomicBool>,
    sessions: SessionRegistry,
    app: AppHandle,
) -> Result<(), String> {
    let mut media_engine = MediaEngine::default();
    media_engine
        .register_codec(
            RTCRtpCodecParameters {
                capability: RTCRtpCodecCapability {
                    mime_type: MIME_TYPE_H264.to_owned(),
                    clock_rate: 90_000,
                    channels: 0,
                    sdp_fmtp_line:
                        "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                            .to_owned(),
                    rtcp_feedback: vec![],
                },
                payload_type: 102,
                ..Default::default()
            },
            RTPCodecType::Video,
        )
        .map_err(|error| format!("could not register H.264: {error}"))?;
    let registry = register_default_interceptors(Registry::new(), &mut media_engine)
        .map_err(|error| format!("could not configure WebRTC feedback: {error}"))?;
    let api = APIBuilder::new()
        .with_media_engine(media_engine)
        .with_interceptor_registry(registry)
        .build();
    let peer = Arc::new(
        api.new_peer_connection(RTCConfiguration::default())
            .await
            .map_err(|error| format!("could not create native WebRTC peer: {error}"))?,
    );
    peer.add_transceiver_from_kind(RTPCodecType::Video, None)
        .await
        .map_err(|error| format!("could not create the H.264 receiver: {error}"))?;

    let (mut player, player_input) = start_player(&session.view.device_name)?;
    let player_feed = Arc::new(Mutex::new(PlayerFeed::new(player_input)));
    let player_available = Arc::new(AtomicBool::new(true));

    let session_done = Arc::new(AtomicBool::new(false));
    let state_done = Arc::clone(&session_done);
    let state_sessions = sessions.clone();
    let state_app = app.clone();
    let state_id = session.id.clone();
    peer.on_peer_connection_state_change(Box::new(move |state| {
        let state_done = Arc::clone(&state_done);
        let state_sessions = state_sessions.clone();
        let state_app = state_app.clone();
        let state_id = state_id.clone();
        Box::pin(async move {
            let status = match state {
                RTCPeerConnectionState::Connected => Some("live"),
                RTCPeerConnectionState::Disconnected => Some("reconnecting"),
                RTCPeerConnectionState::Failed => Some("failed"),
                _ => None,
            };
            if let Some(status) = status {
                let _ = state_sessions.update_status(&state_id, status);
                emit_sessions(&state_app, &state_sessions);
            }
            if matches!(
                state,
                RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed
            ) {
                state_done.store(true, Ordering::Relaxed);
            }
        })
    }));

    let control_channel: Arc<AsyncMutex<Option<Arc<RTCDataChannel>>>> =
        Arc::new(AsyncMutex::new(None));
    let received_control = Arc::clone(&control_channel);
    peer.on_data_channel(Box::new(move |channel| {
        let received_control = Arc::clone(&received_control);
        Box::pin(async move {
            if channel.label() == "lankuu-control" {
                *received_control.lock().await = Some(channel);
            }
        })
    }));

    let frame_app = app.clone();
    let frame_session_id = session.id.clone();
    let track_done = Arc::clone(&session_done);
    let track_player_feed = Arc::clone(&player_feed);
    let track_player_available = Arc::clone(&player_available);
    let track_request_key_frame = Arc::clone(&session.request_key_frame);
    let weak_peer = Arc::downgrade(&peer);
    peer.on_track(Box::new(move |track, _, _| {
        let frame_app = frame_app.clone();
        let frame_session_id = frame_session_id.clone();
        let track_done = Arc::clone(&track_done);
        let track_player_feed = Arc::clone(&track_player_feed);
        let track_player_available = Arc::clone(&track_player_available);
        let track_request_key_frame = Arc::clone(&track_request_key_frame);
        let weak_peer = weak_peer.clone();
        Box::pin(async move {
            if !track
                .codec()
                .capability
                .mime_type
                .eq_ignore_ascii_case(MIME_TYPE_H264)
            {
                let _ = frame_app.emit("app-error", "Desktop received a non-H.264 mirror track");
                track_done.store(true, Ordering::Relaxed);
                return;
            }

            let media_ssrc = track.ssrc();
            let reader_done = Arc::clone(&track_done);
            tokio::spawn(async move {
                let mut depacketizer = H264Packet::default();
                let mut frame = Vec::with_capacity(128 * 1024);
                let mut frame_timestamp = None;
                let mut parameter_sets = H264ParameterSets::default();
                let mut reported_frames = 0_u8;
                let mut reported_errors = 0_u8;
                loop {
                    let (packet, _) = match track.read_rtp().await {
                        Ok(packet) => packet,
                        Err(_) => {
                            reader_done.store(true, Ordering::Relaxed);
                            break;
                        }
                    };
                    if frame_timestamp.is_some_and(|timestamp| timestamp != packet.header.timestamp)
                    {
                        frame.clear();
                        depacketizer = H264Packet::default();
                    }
                    frame_timestamp = Some(packet.header.timestamp);
                    if !is_ignorable_h264_payload(&packet.payload) {
                        match depacketizer.depacketize(&packet.payload) {
                            Ok(payload) => frame.extend_from_slice(&payload),
                            Err(error) => {
                                if reported_errors < 3 {
                                    eprintln!("could not depacketize mirror frame: {error}");
                                    reported_errors += 1;
                                }
                                depacketizer = H264Packet::default();
                                frame.clear();
                                continue;
                            }
                        }
                    }
                    if !packet.header.marker {
                        continue;
                    }
                    if !frame.is_empty() {
                        let prepared = parameter_sets.prepare(&frame);
                        if reported_frames < 4 {
                            eprintln!(
                                "mirror {} H.264 frame: {} bytes, key={}, codec={}, NAL units={:?}",
                                frame_session_id,
                                prepared.data.len(),
                                prepared.key_frame,
                                prepared.codec,
                                prepared.nalu_types
                            );
                            reported_frames += 1;
                        }
                        let player_result = track_player_feed
                            .lock()
                            .map_err(|_| "mirror player input is unavailable")
                            .and_then(|mut feed| {
                                feed.cache(&prepared);
                                if track_player_available.load(Ordering::Relaxed) {
                                    feed.input
                                        .write_all(&prepared.data)
                                        .map_err(|_| "mirror player window was closed")
                                } else {
                                    Ok(())
                                }
                            });
                        if player_result.is_err() {
                            track_player_available.store(false, Ordering::Relaxed);
                        }
                    }
                    frame.clear();
                    frame_timestamp = None;
                }
            });

            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(250));
                let mut ticks = 0_u8;
                loop {
                    interval.tick().await;
                    let requested = track_request_key_frame.swap(false, Ordering::Relaxed);
                    ticks = ticks.wrapping_add(1);
                    if !requested && ticks % 8 != 1 {
                        continue;
                    }
                    let Some(peer) = weak_peer.upgrade() else {
                        break;
                    };
                    if peer
                        .write_rtcp(&[Box::new(PictureLossIndication {
                            sender_ssrc: 0,
                            media_ssrc,
                        })])
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
        })
    }));

    let remote = RTCSessionDescription::offer(offer.sdp)
        .map_err(|error| format!("Android offer is invalid: {error}"))?;
    peer.set_remote_description(remote)
        .await
        .map_err(|error| format!("Android offer was rejected: {error}"))?;
    let answer = peer
        .create_answer(None)
        .await
        .map_err(|error| format!("could not create the native WebRTC answer: {error}"))?;
    let mut gathering_complete = peer.gathering_complete_promise().await;
    peer.set_local_description(answer)
        .await
        .map_err(|error| format!("could not activate the WebRTC answer: {error}"))?;
    let _ = tokio::time::timeout(SIGNAL_TIMEOUT, gathering_complete.recv())
        .await
        .map_err(|_| "timed out while finding the desktop LAN route".to_owned())?;
    let local = peer
        .local_description()
        .await
        .ok_or_else(|| "native WebRTC did not produce an answer".to_owned())?;
    write_rtc_description(signal, &local)?;
    let _ = sessions.update_status(&session.id, "connecting");
    emit_sessions(&app, &sessions);

    while !receiver_stop.load(Ordering::Relaxed)
        && !session.stop.load(Ordering::Relaxed)
        && !session_done.load(Ordering::Relaxed)
    {
        if session.reopen_player.swap(false, Ordering::Relaxed) {
            session.request_key_frame.store(true, Ordering::Relaxed);
            let mut bootstrap_ready = player_feed.lock().is_ok_and(|feed| feed.has_bootstrap());
            for _ in 0..20 {
                if bootstrap_ready {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
                bootstrap_ready = player_feed.lock().is_ok_and(|feed| feed.has_bootstrap());
            }
            if !bootstrap_ready {
                complete_player_request(
                    &session.reopen_result,
                    Err("video key frame is not ready yet; try again in a moment".to_owned()),
                );
                continue;
            }

            match start_player(&session.view.device_name) {
                Ok((mut new_player, new_input)) => {
                    let install_result = player_feed
                        .lock()
                        .map_err(|_| "mirror player input is unavailable".to_owned())
                        .and_then(|mut feed| feed.install(new_input));
                    if let Err(error) = install_result {
                        let _ = new_player.kill();
                        let _ = new_player.wait();
                        complete_player_request(&session.reopen_result, Err(error));
                        continue;
                    }
                    let _ = player.kill();
                    let _ = player.wait();
                    player = new_player;
                    player_available.store(true, Ordering::Relaxed);
                    session.request_key_frame.store(true, Ordering::Relaxed);
                    tokio::time::sleep(Duration::from_millis(180)).await;
                    match player.try_wait() {
                        Ok(None) => complete_player_request(&session.reopen_result, Ok(())),
                        Ok(Some(status)) => {
                            player_available.store(false, Ordering::Relaxed);
                            complete_player_request(
                                &session.reopen_result,
                                Err(format!("mirror player exited before opening ({status})")),
                            );
                        }
                        Err(error) => complete_player_request(
                            &session.reopen_result,
                            Err(format!("could not verify mirror player process: {error}")),
                        ),
                    }
                }
                Err(error) => {
                    complete_player_request(&session.reopen_result, Err(error.clone()));
                    let _ = app.emit(
                        "app-error",
                        format!(
                            "Could not open {} player: {error}",
                            session.view.device_name
                        ),
                    );
                }
            }
        } else if player_available.load(Ordering::Relaxed)
            && player.try_wait().is_ok_and(|status| status.is_some())
        {
            player_available.store(false, Ordering::Relaxed);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    complete_player_request(
        &session.reopen_result,
        Err("mirror session ended before the player could open".to_owned()),
    );

    if session.stop.load(Ordering::Relaxed) || receiver_stop.load(Ordering::Relaxed) {
        if let Some(channel) = control_channel.lock().await.as_ref() {
            let _ = channel
                .send_text(json!({ "type": "stop_session", "sessionId": session.id }).to_string())
                .await;
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
    }
    let _ = peer.close().await;
    drop(player_feed);
    let _ = player.kill();
    let _ = player.wait();
    Ok(())
}

fn start_player(device_name: &str) -> Result<(Child, ChildStdin), String> {
    let title = player_window_title(device_name);
    let mut command = Command::new("ffplay");
    // Do not use `-fflags nobuffer` here: for a raw H.264 pipe it can discard
    // the first IDR during stream probing, leaving a black window until the
    // encoder produces another key frame.
    command.args([
        "-loglevel",
        "warning",
        "-flags",
        "low_delay",
        "-probesize",
        "32768",
        "-analyzeduration",
        "100000",
        "-fpsprobesize",
        "2",
        "-framerate",
        "30",
        "-framedrop",
        "-alwaysontop",
        "-sync",
        "ext",
        "-window_title",
        &title,
        "-f",
        "h264",
        "-i",
        "pipe:0",
    ]);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("could not open the separate mirror window: {error}"))?;
    let input = child
        .stdin
        .take()
        .ok_or_else(|| "could not connect video to the mirror window".to_owned())?;
    Ok((child, input))
}

fn player_window_title(device_name: &str) -> String {
    format!("LanKuu Mirror · {device_name}")
}

fn complete_player_request(
    result: &Arc<Mutex<Option<mpsc::SyncSender<Result<(), String>>>>>,
    value: Result<(), String>,
) {
    if let Ok(mut result) = result.lock() {
        if let Some(sender) = result.take() {
            let _ = sender.send(value);
        }
    }
}

fn is_ignorable_h264_payload(payload: &[u8]) -> bool {
    payload.len() <= 1 || (payload.len() == 2 && payload[0] & 0x1f != 9)
}

impl H264ParameterSets {
    fn prepare(&mut self, frame: &[u8]) -> PreparedH264Frame {
        let nalus = annex_b_nalus(frame);
        let mut nalu_types = Vec::with_capacity(nalus.len());
        let mut has_sps = false;
        let mut has_pps = false;
        let mut key_frame = false;

        for nalu in nalus {
            let Some(header) = nalu.first() else {
                continue;
            };
            let nalu_type = header & 0x1f;
            nalu_types.push(nalu_type);
            match nalu_type {
                5 => key_frame = true,
                7 => {
                    has_sps = true;
                    self.sps = Some(nalu.to_vec());
                }
                8 => {
                    has_pps = true;
                    self.pps = Some(nalu.to_vec());
                }
                _ => {}
            }
        }

        let codec = self
            .sps
            .as_deref()
            .and_then(h264_codec_from_sps)
            .unwrap_or_else(|| "avc1.42E01F".to_owned());
        let mut data = Vec::with_capacity(frame.len() + 128);
        if key_frame {
            if !has_sps {
                append_annex_b_nalu(&mut data, self.sps.as_deref());
            }
            if !has_pps {
                append_annex_b_nalu(&mut data, self.pps.as_deref());
            }
        }
        data.extend_from_slice(frame);

        PreparedH264Frame {
            data,
            key_frame,
            codec,
            nalu_types,
        }
    }
}

fn append_annex_b_nalu(output: &mut Vec<u8>, nalu: Option<&[u8]>) {
    if let Some(nalu) = nalu {
        output.extend_from_slice(&[0, 0, 0, 1]);
        output.extend_from_slice(nalu);
    }
}

fn h264_codec_from_sps(sps: &[u8]) -> Option<String> {
    (sps.len() >= 4).then(|| format!("avc1.{:02X}{:02X}{:02X}", sps[1], sps[2], sps[3]))
}

fn annex_b_nalus(data: &[u8]) -> Vec<&[u8]> {
    let mut nalus = Vec::new();
    let Some((mut start, mut prefix_len)) = find_annex_b_start(data, 0) else {
        return nalus;
    };
    loop {
        let nalu_start = start + prefix_len;
        if let Some((next_start, next_prefix_len)) = find_annex_b_start(data, nalu_start) {
            if nalu_start < next_start {
                nalus.push(&data[nalu_start..next_start]);
            }
            start = next_start;
            prefix_len = next_prefix_len;
        } else {
            if nalu_start < data.len() {
                nalus.push(&data[nalu_start..]);
            }
            break;
        }
    }
    nalus
}

fn find_annex_b_start(data: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut index = from;
    while index + 3 <= data.len() {
        if data[index..].starts_with(&[0, 0, 0, 1]) {
            return Some((index, 4));
        }
        if data[index..].starts_with(&[0, 0, 1]) {
            return Some((index, 3));
        }
        index += 1;
    }
    None
}

#[cfg(test)]
fn contains_idr_nalu(frame: &[u8]) -> bool {
    let mut index = 0;
    while index + 4 < frame.len() {
        let (start, prefix) = if frame[index..].starts_with(&[0, 0, 0, 1]) {
            (index, 4)
        } else if frame[index..].starts_with(&[0, 0, 1]) {
            (index, 3)
        } else {
            index += 1;
            continue;
        };
        if frame
            .get(start + prefix)
            .is_some_and(|byte| byte & 0x1f == 5)
        {
            return true;
        }
        index = start + prefix + 1;
    }
    false
}

fn configure_signal_stream(stream: &TcpStream) -> Result<(), String> {
    stream
        .set_read_timeout(Some(SIGNAL_TIMEOUT))
        .and_then(|_| stream.set_write_timeout(Some(SIGNAL_TIMEOUT)))
        .map_err(|error| format!("could not configure signaling connection: {error}"))
}

fn read_offer(stream: &mut TcpStream) -> Result<IncomingOffer, String> {
    let mut length_bytes = [0_u8; 4];
    stream
        .read_exact(&mut length_bytes)
        .map_err(|error| format!("could not read WebRTC offer length: {error}"))?;
    let length = validate_signal_length(length_bytes)?;
    let mut body = vec![0_u8; length];
    stream
        .read_exact(&mut body)
        .map_err(|error| format!("could not read WebRTC offer: {error}"))?;
    parse_offer(&body)
}

fn parse_offer(body: &[u8]) -> Result<IncomingOffer, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|error| format!("invalid WebRTC offer: {error}"))?;
    let description: MirrorDescription = serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid WebRTC description: {error}"))?;
    validate_description(&description, "offer")?;
    Ok(IncomingOffer {
        description,
        requested_session_id: json_string(&value, "sessionId", ""),
        device_id: json_string(&value, "deviceId", "legacy-device"),
        device_name: json_string(&value, "deviceName", "Android device"),
        platform: json_string(&value, "platform", "android"),
    })
}

fn validate_description(
    description: &MirrorDescription,
    expected_type: &str,
) -> Result<(), String> {
    if !description.sdp_type.eq_ignore_ascii_case(expected_type)
        || description.sdp.trim().is_empty()
    {
        return Err(format!("invalid WebRTC {expected_type}"));
    }
    Ok(())
}

fn json_string(value: &Value, key: &str, fallback: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

fn fallback_session_id() -> String {
    format!(
        "mirror-{}",
        NEXT_FALLBACK_SESSION_ID.fetch_add(1, Ordering::Relaxed)
    )
}

fn safe_identifier(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(96)
        .collect()
}

fn limited_text(value: &str, fallback: &str, limit: usize) -> String {
    let value = value.trim();
    let value = if value.is_empty() { fallback } else { value };
    value.chars().take(limit).collect()
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn validate_signal_length(length_bytes: [u8; 4]) -> Result<usize, String> {
    let length = u32::from_be_bytes(length_bytes) as usize;
    if length == 0 || length > MAX_SIGNAL_BYTES {
        return Err("invalid WebRTC offer size".to_owned());
    }
    Ok(length)
}

fn write_rtc_description(
    stream: &mut TcpStream,
    description: &RTCSessionDescription,
) -> Result<(), String> {
    let body = serde_json::to_vec(description)
        .map_err(|error| format!("could not serialize WebRTC answer: {error}"))?;
    write_signal(stream, &body)
}

fn write_error(stream: &mut TcpStream, error: &str) -> Result<(), String> {
    let body = serde_json::to_vec(&json!({ "error": error }))
        .map_err(|serialize_error| serialize_error.to_string())?;
    write_signal(stream, &body)
}

fn write_signal(stream: &mut TcpStream, body: &[u8]) -> Result<(), String> {
    let length =
        u32::try_from(body.len()).map_err(|_| "signal response is too large".to_owned())?;
    stream
        .write_all(&length.to_be_bytes())
        .and_then(|_| stream.write_all(body))
        .and_then(|_| stream.flush())
        .map_err(|error| format!("could not send WebRTC answer: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_offer(id: &str) -> IncomingOffer {
        parse_offer(
            json!({
                "type": "offer",
                "sdp": "v=0\r\n",
                "sessionId": id,
                "deviceId": "device-1",
                "deviceName": "Test phone",
                "platform": "android"
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn rejects_oversized_signal_length() {
        assert!(validate_signal_length(0_u32.to_be_bytes()).is_err());
        assert!(validate_signal_length(((MAX_SIGNAL_BYTES as u32) + 1).to_be_bytes()).is_err());
        assert_eq!(validate_signal_length(512_u32.to_be_bytes()).unwrap(), 512);
    }

    #[test]
    fn offer_metadata_is_parsed_without_changing_the_sdp() {
        let offer = test_offer("session-123");
        assert_eq!(offer.requested_session_id, "session-123");
        assert_eq!(offer.device_name, "Test phone");
        assert_eq!(offer.platform, "android");
        assert_eq!(offer.description.sdp, "v=0\r\n");
    }

    #[test]
    fn registry_assigns_unique_ids_and_enforces_the_limit() {
        let registry = SessionRegistry::default();
        for index in 0..MAX_CONCURRENT_SESSIONS {
            let registered = registry
                .register(&test_offer("duplicate"), format!("192.168.1.{}", index + 2))
                .unwrap();
            if index == 0 {
                assert_eq!(registered.id, "duplicate");
            } else {
                assert_ne!(registered.id, "duplicate");
            }
        }
        assert!(registry
            .register(&test_offer("one-too-many"), "192.168.1.99".to_owned())
            .is_err());
    }

    #[test]
    fn play_request_reopens_only_a_live_session_and_requests_a_key_frame() {
        let registry = SessionRegistry::default();
        let session = registry
            .register(&test_offer("play-session"), "192.168.1.20".to_owned())
            .unwrap();
        assert!(registry.show_one(&session.id).is_err());
        registry.update_status(&session.id, "live").unwrap();
        let request_registry = registry.clone();
        let request_id = session.id.clone();
        let request = thread::spawn(move || request_registry.show_one(&request_id));
        for _ in 0..50 {
            if session.reopen_player.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert!(session.reopen_player.load(Ordering::Relaxed));
        assert!(session.request_key_frame.load(Ordering::Relaxed));
        complete_player_request(&session.reopen_result, Ok(()));
        request.join().unwrap().unwrap();
    }

    #[test]
    fn detects_idr_frames_in_annex_b_data() {
        assert!(contains_idr_nalu(&[0, 0, 0, 1, 0x65, 1, 2, 3]));
        assert!(!contains_idr_nalu(&[0, 0, 0, 1, 0x41, 1, 2, 3]));
    }

    #[test]
    fn ignores_empty_and_short_rtp_payloads_before_h264_depacketizing() {
        assert!(is_ignorable_h264_payload(&[]));
        assert!(is_ignorable_h264_payload(&[0x00]));
        assert!(is_ignorable_h264_payload(&[0x1c, 0x80]));
        assert!(!is_ignorable_h264_payload(&[0x09, 0x10]));
        assert!(!is_ignorable_h264_payload(&[0x1c, 0x80, 0x65]));
    }

    #[test]
    fn key_frames_receive_cached_parameter_sets_and_actual_codec() {
        let mut sets = H264ParameterSets::default();
        let config = [
            0, 0, 0, 1, 0x67, 0x64, 0x00, 0x28, 1, 2, 0, 0, 0, 1, 0x68, 3, 4,
        ];
        let prepared_config = sets.prepare(&config);
        assert!(!prepared_config.key_frame);
        assert_eq!(prepared_config.codec, "avc1.640028");

        let idr = [0, 0, 0, 1, 0x65, 9, 8, 7];
        let prepared_idr = sets.prepare(&idr);
        assert!(prepared_idr.key_frame);
        assert_eq!(prepared_idr.codec, "avc1.640028");
        assert_eq!(prepared_idr.nalu_types, vec![5]);
        assert_eq!(
            annex_b_nalus(&prepared_idr.data)
                .iter()
                .filter_map(|nalu| nalu.first().map(|byte| byte & 0x1f))
                .collect::<Vec<_>>(),
            vec![7, 8, 5]
        );
    }
}
