# LanKuu Mirror Protocol v2

LanKuu v2 mirrors one Android screen to one Ubuntu desktop with a direct WebRTC
peer connection. Both devices must be on the same local network. No cloud,
STUN, TURN, or media server is used in this phase.

## Signaling

- The Ubuntu receiver listens on TCP port `45456`.
- Every message is a 4-byte unsigned big-endian length followed by UTF-8 JSON.
- The Android request is a complete, non-trickle WebRTC SDP offer.
- The Ubuntu response is a complete, non-trickle WebRTC SDP answer, or an
  object containing an `error` string.
- Signal bodies are limited to 1 MiB.

The signaling transport is deliberately small and LAN-only. It is not yet
authenticated; the pairing milestone will bind SDP exchange to a trusted
device identity.

## Media

- Android captures with `ScreenCapturerAndroid`/MediaProjection.
- Video is H.264/AVC, 30 fps, with a 1280-pixel longest-side cap.
- Initial bitrate is 3.5 Mbit/s with a 0.6–6 Mbit/s adaptive range.
- WebRTC provides ICE connectivity checks, congestion feedback, NACK/RTCP,
  jitter handling, keyframe requests, and DTLS-SRTP media encryption.
- Ubuntu depacketizes H.264 RTP and feeds Annex-B data to a low-buffer ffplay
  renderer.

## Session model

The v2 receiver accepts one active Android peer at a time. A stopped or failed
session returns the listener to its waiting state. Session ownership is kept
separate from the signaling listener so a later version can map multiple peer
IDs to independent renderer sessions without changing the Android capture
contract.
