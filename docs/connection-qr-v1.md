# LanKuu connection QR v1

LanKuu Desktop exposes one QR code for file sharing and screen mirroring. The
code removes manual address entry. It does not authenticate either device.

## Payload

The QR code contains a URI with this shape:

```text
lankuu://connect?v=1&host=192.168.1.56&transfer=45454&mirror=45456&name=LanKuu%20Desktop
```

Fields:

- `v` is the connection-code version and must be `1`.
- `host` is the desktop address reachable on the current LAN.
- `transfer` is the TCP file and text receiver port.
- `mirror` is the TCP WebRTC signaling port.
- `name` is the percent-encoded desktop display name.

Android rejects unknown schemes, unknown versions, blank or malformed hosts,
and ports outside `1..65535`. A valid scan populates both the Share destination
and the Cast endpoint. Manual address entry and UDP discovery remain available
as fallback paths.

The QR payload is connection metadata only. File-transfer authentication and
mirror-signaling authentication require a separate protocol revision.
