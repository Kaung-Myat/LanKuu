# LanKuu Mirror Protocol v3

LanKuu v3 extends the direct-LAN WebRTC protocol with concurrent source
sessions, device identity metadata, and a small control data channel. A desktop
receiver accepts up to four active sources. Each source has its own peer
connection, renderer process, stop flag, and lifecycle state.

## Signaling

- The desktop listens on TCP port `45456` and handles each accepted connection
  on an independent worker.
- Every signaling message remains a 4-byte unsigned big-endian length followed
  by UTF-8 JSON, limited to 1 MiB.
- The source request contains the complete non-trickle SDP offer plus:

```json
{
  "protocolVersion": 3,
  "sessionId": "54fa1f41-…",
  "deviceId": "stable-device-id",
  "deviceName": "Google Pixel 8",
  "platform": "android",
  "type": "offer",
  "sdp": "…"
}
```

- The response is the complete non-trickle SDP answer or an object containing
  an `error` string.
- v2 senders remain compatible. Missing metadata receives safe legacy values
  and a receiver-generated session ID.

## Session lifecycle

The receiver keeps a registry keyed by `sessionId`. States are `negotiating`,
`live`, `reconnecting`, `stopping`, and `failed`. Closing a player keeps its
peer connection available for the Play action. The session's Stop action ends
only that peer connection. Turning off the receiver stops every session and
prevents new connections.

Every stream is still H.264/AVC at up to 30 fps and a 1280-pixel longest-side
cap. Each session owns a native Rust `RTCPeerConnection`. Rust depacketizes RTP
into complete Annex-B H.264 access units and writes them directly to an
independent FFplay window per session. This avoids both Ubuntu WebKit's missing
`RTCPeerConnection` and its unreliable embedded H.264 `WebCodecs` path. The
LanKuu Mirror page remains the multi-device session/control list; closing a
device's player window keeps the session available. Its Play action starts a
fresh always-on-top player, primes it immediately with the latest complete H.264
key frame, and then requests a new key frame for the live stream. The request is
acknowledged only after the player process remains healthy through startup. The
UI runs this wait off the main thread, shows a per-session loading state, and
keeps the LanKuu window open underneath the player. The Stop action ends only
that device's session.

The receiver caches H.264 SPS/PPS parameter sets and prepends them when an IDR
frame arrives without in-band configuration. It also keeps the current GOP from
its latest prepared IDR through the newest dependent frame. A reopened player is
primed with that complete sequence before the live writer is swapped atomically,
so it neither waits for another key frame nor starts with missing references.

FFplay receives an explicit 30 fps raw-H.264 input configuration and a bounded
probe window. Probe packets remain buffered because discarding the first IDR
would leave the player black until Android produces another key frame.

Empty and invalid one- or two-byte RTP padding/keepalive payloads are ignored
before H.264 depacketization (valid two-byte AUD units are preserved), so they
cannot discard an in-progress fragmented frame. RTP timestamp changes also
prevent incomplete frames from being joined to the following frame.

## Control channel

The source creates a WebRTC data channel named `lankuu-control`. Before the
desktop closes a user-stopped session it sends:

```json
{ "type": "stop_session", "sessionId": "54fa1f41-…" }
```

Android responds by stopping MediaProjection, the encoder, its foreground
notification, and the peer connection. Remote start is deliberately not
supported: Android's system screen-capture consent remains mandatory.

## Security

DTLS-SRTP encrypts each WebRTC session, but the LAN signaling endpoint and
device metadata are not authenticated yet. Device pairing, approval, and a
trusted-device store are required before the stable release.
