# LanKuu

LanKuu sends files and text directly between devices on the same local network.
The MVP includes a native Ubuntu/Linux CLI, a cross-platform Tauri desktop app,
and an Android APK that speak the same small streaming protocol.

> **MVP security notice:** File/text transfer traffic is not yet encrypted or
> authenticated. Mirroring media is encrypted by WebRTC, but device pairing is
> still planned before the first stable release.

## What works

- Stream files without loading them into memory, including multi-gigabyte files
- Send UTF-8 text
- Discover receiving devices with a UDP broadcast
- Save Linux downloads without overwriting an existing file
- Save Android downloads in `Downloads/LanKuu`
- Select and send multiple Android files as an ordered transfer batch
- Use a polished desktop interface with file picking, text sharing, live
  progress, receiver controls, and transfer activity
- Mirror up to four Android screens concurrently to one desktop over the LAN
  with hardware H.264 encoding and per-device Stop controls
- Transfer between CLI ↔ CLI, CLI ↔ Android, and Android ↔ Android

## Ubuntu CLI

Build and test:

```bash
cargo test
cargo build --release -p lankuu
```

Start a receiver:

```bash
./target/release/lankuu receive
```

Find receivers and send content:

```bash
./target/release/lankuu discover
./target/release/lankuu send 192.168.1.42 ./large-file.iso
./target/release/lankuu send-text 192.168.1.42 "mingalaba"
```

Received files default to `~/Downloads/LanKuu`. Use `lankuu help` for all
options.

## Desktop app

The desktop app uses Tauri 2 with a dependency-free HTML/CSS/JavaScript
frontend and the shared Rust transfer core. Tagged GitHub releases build
installers for Ubuntu, Windows, and both Intel and Apple Silicon Macs.

Install the official Tauri Linux prerequisites on Ubuntu:

```bash
sudo apt install libwebkit2gtk-4.1-dev libsoup-3.0-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev \
  ffmpeg
```

The release `.deb` declares FFmpeg as a dependency. Install it separately before
using mirroring from an `.AppImage`. The manual command above is only needed
when running or building LanKuu from source.
LanKuu terminates WebRTC in native Rust because Ubuntu's WebKit build does not
expose `RTCPeerConnection`, and its embedded H.264 WebCodecs decoder is not
reliable across Ubuntu builds. Each connected stream therefore opens in its own
native low-latency FFplay window. The LanKuu page remains the multi-device
session and stop-control panel.

Run a development build:

```bash
cargo run -p lankuu-desktop
```

Build the optimized desktop binary:

```bash
cargo build --release -p lankuu-desktop
```

Inside the app, choose a discovered device (or enter its IP), then drop or pick
files. Turn on **Ready to receive** to make the desktop discoverable. Received
files are stored in `~/Downloads/LanKuu`.

### Native screen mirroring

1. Put the Android phones and desktop computer on the same trusted network.
2. Open **Mirror** in the desktop app and select **Start mirror receiver**.
3. Open **Cast** in the Android app, enter the Ubuntu IP shown on desktop, and
   select **Start mirroring**.
4. Accept Android's system screen-sharing prompt. Each device appears as a
   low-latency video tile inside LanKuu and in its own player window. Repeat on
   up to four phones.
5. Stop one phone from its list item without interrupting the other sessions,
   or turn off the receiver to stop every session.

Each device has its own native player window. The Play action opens that
device's player again after it was closed. LanKuu shows an in-card loading state
while the native player starts, reports a real process failure, and keeps the
main window in place while the always-on-top player appears above it. Use the
Stop action beside a device to end only that device's mirroring session.
The desktop preserves the current H.264 key-frame sequence when reopening a
player, preventing a black wait for the next encoder key frame.

Mirroring uses MediaProjection, hardware H.264 encoding, and a direct WebRTC
peer connection. TCP port `45456` exchanges the one-time SDP offer/answer;
WebRTC then chooses a direct dynamic UDP media path, encrypts video with
DTLS-SRTP, and adapts bitrate using network feedback. The desktop's native Rust
peer depacketizes complete H.264 frames and sends them to each native FFplay
window. A WebRTC control channel lets the desktop stop an individual Android
capture cleanly. The current MVP is view-only: it does not transmit audio or
support desktop touch/keyboard control.

## Android APK

The Android MVP supports Android 10 and newer.

```bash
cd android
./gradlew assembleDebug
```

Install `app/build/outputs/apk/debug/app-debug.apk`, keep LanKuu open, and tap
**Start receiving** before sending to the phone. To send, discover a receiving
device or enter its local IP address.

## Releases

Pushing a version tag runs `.github/workflows/release.yml`. The workflow first
checks Rust formatting, core tests, and Android lint. It then creates native
Ubuntu (`.deb` and `.AppImage`), Windows (`.exe`), macOS (`.dmg`), and Android
(`.apk`) downloads. A draft release is published only after every platform
build succeeds.

The current Android CI artifact is debug-signed so it can be installed for MVP
testing. A stable production signing key is required before Play Store or
production distribution.

## Repository layout

```text
crates/lankuu-core/  Rust wire protocol and streaming helpers
crates/lankuu-cli/   Ubuntu/Linux command-line app
desktop/             Tauri desktop app and macOS-inspired interface
android/             Kotlin Android application
docs/protocol-v1.md  Cross-platform wire protocol
docs/mirror-v3.md    Multi-session direct-LAN WebRTC mirroring protocol
```

## Current MVP limitations

- No encryption, device pairing, or receiver confirmation
- No interrupted-transfer resume or content checksum yet
- Mirroring is currently video-only and view-only
- Mirroring signaling is LAN-only and is not authenticated until pairing lands
- Android receives only while the app screen/process remains active
- UDP discovery may be blocked by guest Wi-Fi or access-point isolation
- The Android implementation mirrors the shared protocol in Kotlin; moving the
  transfer engine into the Rust core through JNI is a later hardening step

## Next milestones

1. Pair devices with a short code and encrypt every transfer
2. Add BLAKE3 verification, chunking, pause, and resume
3. Move Android receiving to a user-visible foreground transfer service
4. Add transfer progress, history, and Android share-sheet integration
5. Harden Windows and macOS support, then add an iOS client
