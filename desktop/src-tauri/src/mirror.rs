use serde_json::json;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex as StdMutex,
};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_H264};
use webrtc::api::APIBuilder;
use webrtc::interceptor::registry::Registry;
use webrtc::media::io::h264_writer::H264Writer;
use webrtc::media::io::Writer as MediaWriter;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtp_transceiver::rtp_codec::{
    RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType,
};

pub const SIGNAL_PORT: u16 = 45_456;
const MAX_SIGNAL_BYTES: usize = 1_048_576;
const SIGNAL_TIMEOUT: Duration = Duration::from_secs(20);

struct PlayerGuard(Arc<StdMutex<Child>>);

impl Drop for PlayerGuard {
    fn drop(&mut self) {
        if let Ok(mut child) = self.0.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn ensure_player_available() -> Result<(), String> {
    Command::new("ffplay")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| {
            "ffplay is required. Install FFmpeg and make ffplay available on PATH.".to_owned()
        })?
        .success()
        .then_some(())
        .ok_or_else(|| "ffplay is not working. Reinstall FFmpeg and check PATH.".to_owned())
}

pub fn run(listener: TcpListener, stop: Arc<AtomicBool>, app: AppHandle) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("could not start the WebRTC runtime: {error}"))?;

    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut stream, peer_address)) => {
                stream
                    .set_read_timeout(Some(SIGNAL_TIMEOUT))
                    .map_err(|error| error.to_string())?;
                stream
                    .set_write_timeout(Some(SIGNAL_TIMEOUT))
                    .map_err(|error| error.to_string())?;
                let offer = match read_offer(&mut stream) {
                    Ok(offer) => offer,
                    Err(error) => {
                        let _ = write_error(&mut stream, &error);
                        continue;
                    }
                };
                let _ = app.emit(
                    "mirror-peer-status",
                    format!("Negotiating WebRTC with {}", peer_address.ip()),
                );
                if let Err(error) = runtime.block_on(run_session(
                    &mut stream,
                    offer,
                    Arc::clone(&stop),
                    app.clone(),
                )) {
                    let _ = write_error(&mut stream, &error);
                    let _ = app.emit("app-error", format!("WebRTC session failed: {error}"));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(60));
            }
            Err(error) => return Err(format!("could not accept WebRTC signaling: {error}")),
        }
    }
    Ok(())
}

async fn run_session(
    signal: &mut TcpStream,
    offer: RTCSessionDescription,
    stop: Arc<AtomicBool>,
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
    let peer_connection = Arc::new(
        api.new_peer_connection(RTCConfiguration::default())
            .await
            .map_err(|error| format!("could not create WebRTC peer: {error}"))?,
    );
    peer_connection
        .add_transceiver_from_kind(RTPCodecType::Video, None)
        .await
        .map_err(|error| format!("could not create video receiver: {error}"))?;

    let session_done = Arc::new(AtomicBool::new(false));
    let state_done = Arc::clone(&session_done);
    let state_app = app.clone();
    peer_connection.on_peer_connection_state_change(Box::new(move |state| {
        let state_done = Arc::clone(&state_done);
        let state_app = state_app.clone();
        Box::pin(async move {
            match state {
                RTCPeerConnectionState::Connected => {
                    let _ = state_app.emit("mirror-peer-status", "Android connected securely");
                }
                RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => {
                    state_done.store(true, Ordering::Relaxed);
                }
                _ => {}
            }
        })
    }));

    let (player, player_input) = start_player()?;
    let player = Arc::new(StdMutex::new(player));
    let _player_guard = PlayerGuard(Arc::clone(&player));
    let writer = Arc::new(Mutex::new(H264Writer::new(player_input)));
    let track_writer = Arc::clone(&writer);
    let track_done = Arc::clone(&session_done);
    let weak_peer = Arc::downgrade(&peer_connection);
    let track_app = app.clone();
    peer_connection.on_track(Box::new(move |track, _, _| {
        let track_writer = Arc::clone(&track_writer);
        let track_done = Arc::clone(&track_done);
        let weak_peer = weak_peer.clone();
        let track_app = track_app.clone();
        Box::pin(async move {
            if !track
                .codec()
                .capability
                .mime_type
                .eq_ignore_ascii_case(MIME_TYPE_H264)
            {
                let _ = track_app.emit("app-error", "Desktop received a non-H.264 mirror track");
                track_done.store(true, Ordering::Relaxed);
                return;
            }

            let media_ssrc = track.ssrc();
            tokio::spawn(async move {
                loop {
                    match track.read_rtp().await {
                        Ok((packet, _)) => {
                            if track_writer.lock().await.write_rtp(&packet).is_err() {
                                track_done.store(true, Ordering::Relaxed);
                                break;
                            }
                        }
                        Err(_) => {
                            track_done.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                }
            });

            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(2));
                loop {
                    interval.tick().await;
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

    peer_connection
        .set_remote_description(offer)
        .await
        .map_err(|error| format!("Android offer was rejected: {error}"))?;
    let answer = peer_connection
        .create_answer(None)
        .await
        .map_err(|error| format!("could not create the WebRTC answer: {error}"))?;
    let mut gathering_complete = peer_connection.gathering_complete_promise().await;
    peer_connection
        .set_local_description(answer)
        .await
        .map_err(|error| format!("could not activate the WebRTC answer: {error}"))?;
    let _ = tokio::time::timeout(SIGNAL_TIMEOUT, gathering_complete.recv())
        .await
        .map_err(|_| "timed out while finding the desktop LAN route".to_owned())?;
    let local_description = peer_connection
        .local_description()
        .await
        .ok_or_else(|| "WebRTC did not produce an answer".to_owned())?;
    write_description(signal, &local_description)?;

    while !stop.load(Ordering::Relaxed) && !session_done.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(mut child) = player.lock() {
            if child
                .try_wait()
                .map_err(|error| format!("could not inspect mirror player: {error}"))?
                .is_some()
            {
                session_done.store(true, Ordering::Relaxed);
            }
        }
    }

    let _ = peer_connection.close().await;
    drop(writer);
    let _ = app.emit("mirror-peer-status", "Waiting for Android");
    Ok(())
}

fn start_player() -> Result<(Child, std::process::ChildStdin), String> {
    let mut player = Command::new("ffplay")
        .args([
            "-loglevel",
            "warning",
            "-fflags",
            "nobuffer",
            "-flags",
            "low_delay",
            "-framedrop",
            "-autoexit",
            "-probesize",
            "32",
            "-analyzeduration",
            "0",
            "-sync",
            "ext",
            "-f",
            "h264",
            "-i",
            "pipe:0",
            "-window_title",
            "LanKuu Mirror · WebRTC",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not start ffplay: {error}"))?;
    let input = player
        .stdin
        .take()
        .ok_or_else(|| "could not open the mirror video pipe".to_owned())?;
    Ok((player, input))
}

fn read_offer(stream: &mut TcpStream) -> Result<RTCSessionDescription, String> {
    let mut length_bytes = [0_u8; 4];
    stream
        .read_exact(&mut length_bytes)
        .map_err(|error| format!("could not read WebRTC offer length: {error}"))?;
    let length = validate_signal_length(length_bytes)?;
    let mut body = vec![0_u8; length];
    stream
        .read_exact(&mut body)
        .map_err(|error| format!("could not read WebRTC offer: {error}"))?;
    serde_json::from_slice(&body).map_err(|error| format!("invalid WebRTC offer: {error}"))
}

fn validate_signal_length(length_bytes: [u8; 4]) -> Result<usize, String> {
    let length = u32::from_be_bytes(length_bytes) as usize;
    if length == 0 || length > MAX_SIGNAL_BYTES {
        return Err("invalid WebRTC offer size".to_owned());
    }
    Ok(length)
}

fn write_description(
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

    #[test]
    fn rejects_oversized_signal_length() {
        assert!(validate_signal_length(0_u32.to_be_bytes()).is_err());
        assert!(validate_signal_length(((MAX_SIGNAL_BYTES as u32) + 1).to_be_bytes()).is_err());
        assert_eq!(validate_signal_length(512_u32.to_be_bytes()).unwrap(), 512);
    }
}
