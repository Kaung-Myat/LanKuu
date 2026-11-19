# LanKuu connection QR v2

LanKuu Desktop exposes one QR code for file sharing and screen mirroring. The
code removes manual address entry and contains a high-entropy pairing token.

## Payload

The QR code contains a URI with this shape:

```text
lankuu://connect?v=2&host=192.168.1.56&transfer=45454&mirror=45456&name=LanKuu%20Desktop&pair=&lt;token&gt;
```

Fields:

- `v` is the connection-code version and must be `2`.
- `host` is the desktop address reachable on the current LAN.
- `transfer` is the TCP file and text receiver port.
- `mirror` is the TCP WebRTC signaling port.
- `name` is the percent-encoded desktop display name.
- `pair` is the pairing token required for transfers and mirror signaling.

Android rejects unknown schemes, unknown versions, blank or malformed hosts,
missing pairing tokens, and ports outside `1..65535`. A valid scan populates
both the Share destination and the Cast endpoint. Discovery can list nearby
devices but cannot authorize a transfer; scan the receiver QR code first.

The pairing token authenticates file/text transfer and the initial mirror
signal. Do not copy it into screenshots, logs, or chat messages.
