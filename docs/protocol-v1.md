# LanKuu wire protocol v1

LanKuu v1 uses one TCP connection per payload. Multi-byte integers are unsigned
and encoded in network byte order (big endian). File bytes are streamed and are
never wrapped in JSON or Base64.

Default ports:

- TCP `45454` for transfers
- UDP `45455` for discovery

## Transfer frame

| Field | Size | Value |
|---|---:|---|
| Magic | 8 bytes | ASCII `LANKUU01` |
| Kind | 1 byte | `1` text, `2` file |
| Name length | 2 bytes | UTF-8 name byte count |
| Payload length | 8 bytes | Payload byte count |
| Name | variable | UTF-8, maximum 1024 bytes |
| Payload | variable | Raw text or file bytes |

After consuming exactly the declared payload length, the receiver returns a
two-byte acknowledgment:

- `OK` — the payload was committed successfully
- `ER` — the payload was rejected or failed

The sender half-closes its write side after the payload and waits for the
acknowledgment. A receiver must treat an early EOF as a failed transfer and must
not publish a partial file.

### Multi-file batches

A multi-file selection is sent as an ordered sequence of independent v1 file
transfers. The sender waits for the `OK` or `ER` acknowledgment for the current
file before opening the next connection. A failed file does not cancel the
remaining selection, so the sender can report complete, partial, and failed
batch results without changing the wire format. Existing v1 Ubuntu and Android
receivers therefore remain compatible with multi-file sending.

Text payloads are UTF-8 and limited to 16 MiB. Receivers must discard directory
components and control characters from incoming file names.

## Discovery

A searching device broadcasts this exact UDP datagram to port `45455`:

```text
LANKUU_DISCOVER_V1
```

An active receiver responds by unicast to the source address and port:

```text
LANKUU_HERE_V1|<device-name>|<tcp-port>
```

The device name must replace `|` with `_`.

## Compatibility policy

The eight-byte magic includes the major wire version. Incompatible protocol
changes use a new magic value. Implementations must reject unknown magic and
payload kinds instead of guessing.

## Security status

Version 1 is an MVP interoperability protocol. It provides framing and TCP
delivery only; it does not provide identity, authorization, confidentiality,
or application-level integrity. These are required before a stable release.
