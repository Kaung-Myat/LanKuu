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
- Mirror an Android screen to Ubuntu over the LAN with hardware H.264 encoding
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
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev ffmpeg
```

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

1. Put the Android phone and Ubuntu computer on the same trusted network.
2. Open **Mirror** in the desktop app and select **Start mirror receiver**.
3. Open **Cast** in the Android app, enter the Ubuntu IP shown on desktop, and
   select **Start mirroring**.
4. Accept Android's system screen-sharing prompt. Video opens in a low-latency
   `ffplay` window. Stop from either app when finished.

Mirroring uses MediaProjection, hardware H.264 encoding, and a direct WebRTC
peer connection. TCP port `45456` exchanges the one-time SDP offer/answer;
WebRTC then chooses a direct dynamic UDP media path, encrypts video with
DTLS-SRTP, and adapts bitrate using network feedback. The current MVP is
view-only: it does not transmit audio or support desktop touch/keyboard control.

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
docs/mirror-v2.md    Direct-LAN WebRTC mirroring protocol
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
