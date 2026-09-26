# LanKuu Mirror Protocol v1

LanKuu's native mirroring MVP sends an Annex-B H.264 elementary stream from
Android to an Ubuntu receiver over UDP. Both devices must be on the same
trusted local network.

## Transport

- UDP destination port: `45456`
- Maximum encoded payload per datagram: 1,150 bytes
- Video: H.264/AVC, up to 1280 pixels on the longest side, 30 fps, 5 Mbit/s
- Multi-byte integers use network byte order (big endian)

## Datagram header

Every datagram starts with this 22-byte header:

| Offset | Size | Field |
| --- | ---: | --- |
| 0 | 4 | ASCII magic `LKSC` |
| 4 | 1 | Protocol version (`1`) |
| 5 | 1 | Flags: bit 0 codec config, bit 1 key frame |
| 6 | 4 | Frame ID |
| 10 | 2 | Zero-based chunk index |
| 12 | 2 | Chunk count |
| 14 | 8 | Presentation timestamp in microseconds |
| 22 | n | Encoded H.264 bytes |

The receiver groups datagrams by frame ID, orders them by chunk index, and
writes only complete frames to the decoder. Incomplete frames expire after 500
milliseconds so packet loss cannot grow memory indefinitely.

## MVP security and reliability

The stream is not encrypted or authenticated. UDP is intentionally used to
avoid retransmission stalls and keep latency low, so lost packets can produce a
brief visual artifact. Pairing, encryption, adaptive bitrate, audio, and input
control are future protocol revisions.
