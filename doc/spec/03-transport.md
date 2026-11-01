# 3. Transport

## 3.1 QUIC and TLS

QDP runs over QUIC version 1 [RFC9000] [RFC9001] [RFC9002]. Endpoints MAY
negotiate a later QUIC version that provides the same stream, datagram and
security services. QUIC requires TLS 1.3 [RFC8446], so every QDP session is
encrypted and the host is always authenticated by the TLS handshake
([Section 11.1](11-security.md#111-host-authentication)).

The host listens on UDP port 4433 by default. Deployments MAY use any port.

## 3.2 ALPN

Endpoints identify QDP with the TLS Application-Layer Protocol Negotiation
extension [RFC7301]. The token for the final version 1 of this specification will
be `qdp/1`. Implementations of a draft MUST use the token `qdp-draft-NN`, where NN
is the two-digit draft number; this document is draft 00, so its token is
`qdp-draft-00`. A host that supports none of the tokens the client offers MUST
abort the handshake with the TLS `no_application_protocol` alert, which QUIC
carries as error code 0x0178.

The ALPN token identifies the major version. Compatible additions within a major
version are signalled by the minor version in the hello messages and by
capabilities ([Section 4.3](04-session.md#43-hello-messages)).

## 3.3 Transport parameters

- Both endpoints MUST send the `max_datagram_frame_size` transport parameter (0x20)
  [RFC9221] with a value of at least 1200, and SHOULD send 65535. The value limits
  a whole DATAGRAM frame, including its type and length fields. An endpoint checks
  the peer's value when the handshake completes and, if it is missing or too small,
  closes the connection with `QDP_TRANSPORT_REQUIREMENTS` in a 1-RTT packet, so that
  the QDP error code reaches the peer.
- The host MUST allow the client to open at least one bidirectional stream.
- Each endpoint MUST allow its peer to open at least 16 unidirectional streams, and
  SHOULD allow at least 64.
- Endpoints SHOULD set `max_idle_timeout` to 30 seconds and SHOULD send QUIC PING
  frames when no other packets have been sent for 10 seconds, so that an idle
  desktop does not end the session.

## 3.4 Early data and session resumption

Endpoints MUST NOT send QDP data in 0-RTT packets, and hosts MUST NOT accept 0-RTT
data. Replayed input could repeat keystrokes or clicks on the host.

Hosts MUST NOT issue TLS session tickets, and clients MUST NOT attempt TLS session
resumption. Without 0-RTT, resumption would save little, and it would prevent client
certificate authentication, which TLS does not offer on resumed connections. A
client that reconnects quickly, for example after the operating system suspended
it, uses a QDP resumption token instead
([Section 4.4.7](04-session.md#447-resumption_token-0x08)).

## 3.5 Streams

### 3.5.1 Control stream

The control stream is the first client-initiated bidirectional QUIC stream (stream
ID 0). The client opens it immediately after the QUIC handshake and sends
`CLIENT_HELLO` as its first message. Each direction of the control stream carries a
sequence of control messages ([Section 3.6](#36-control-messages)).

Neither endpoint may close or reset either direction of the control stream before
the session ends. An endpoint that sees the control stream closed or reset MUST
close the connection with `QDP_CLOSED_CRITICAL_STREAM`.

No other bidirectional streams are defined. A host that receives another
client-initiated bidirectional stream, or a client that receives a host-initiated
bidirectional stream, MUST close the connection with `QDP_STREAM_CREATION_ERROR`,
unless an extension negotiated by capability says otherwise.

### 3.5.2 Unidirectional streams

Unidirectional streams carry data that must arrive intact and in order: cursor
images, clipboard contents and the output reports of passthrough devices. Each
unidirectional stream begins with a stream type:

```
Unidirectional Stream {
  Stream Type (i),
  Stream Data (..),
}
```

| Stream Type | Name | Direction | Defined in |
|---|---|---|---|
| 0x01 | CURSOR_IMAGE | Host → client | [Section 9.2](09-cursor.md#92-cursor-images) |
| 0x02 | CLIPBOARD_DATA | Both | [Section 10.4](10-clipboard.md#104-clipboard-data) |
| 0x03 | HID_OUTPUT | Host → client | [Section 8.8.2](08-input.md#882-hid-output-streams) |

A receiver that does not recognize a stream type MUST either abort reading the
stream with a STOP_SENDING frame carrying `QDP_STREAM_TYPE_UNKNOWN`, or read and
discard its data. It MUST NOT close the connection for this reason. A receiver MUST
tolerate streams that are reset or end before their Stream Type is complete, and
ignores them. A receiver that gets a known stream type in the wrong direction MUST
close the connection with `QDP_STREAM_CREATION_ERROR`.

The host MUST NOT open unidirectional streams before it sends `SESSION_START`,
except `HID_OUTPUT` streams for devices it has accepted. The client MUST NOT open
unidirectional streams before it receives `SESSION_START`; a host that receives one
earlier MUST close the connection with `QDP_STREAM_CREATION_ERROR`. QUIC does not
order streams with the control stream, so a client can receive a host stream before
it has processed `SESSION_START` or the message that defines the stream's object.
It buffers such streams within the limits of [Section 3.7.3](#373-processing-rules).

## 3.6 Control messages

### 3.6.1 Framing

```
Control Message {
  Message Type (i),
  Message Length (i),
  Message Payload (..),
}
```

`Message Length` is the number of bytes in `Message Payload`. It MUST NOT exceed
1 MiB (1,048,576 bytes); a receiver that reads a larger length MUST close the
connection with `QDP_MESSAGE_TOO_LARGE` without reading the payload. Before
authentication succeeds, the host also applies the smaller budget of
[Section 11.7](11-security.md#117-resource-exhaustion). Data larger than this
belongs on a unidirectional stream.

The message types are listed in [Section 12.1](12-registries.md#121-control-message-types).
Payload layouts in later sections omit the `Message Type` and `Message Length`
fields.

### 3.6.2 Processing rules

- Control messages are processed in the order received.
- A receiver MUST ignore a control message of unknown type.
- A receiver MUST ignore bytes after the last field it understands in a known
  message ([Section 1.6](01-introduction.md#16-extensibility)).
- A message that is not allowed in the current session state, or not allowed in the
  direction it arrived, is a connection error of type `QDP_PROTOCOL_VIOLATION`.
- A message that refers to an identifier that is not live is handled as described in
  [Section 1.5.7](01-introduction.md#157-identifiers) and is not an error.

## 3.7 Datagrams

### 3.7.1 Framing

Each QUIC DATAGRAM frame carries exactly one QDP datagram:

```
Datagram {
  Datagram Type (i),
  Datagram Payload (..),
}
```

The payload runs to the end of the DATAGRAM frame. Datagram types are listed in
[Section 12.2](12-registries.md#122-datagram-types).

### 3.7.2 Size

QUIC datagrams cannot be fragmented [RFC9221]. A sender MUST NOT send a datagram
larger than the maximum its QUIC implementation reports for the current path, which
can be smaller than the peer's `max_datagram_frame_size` and can shrink when the
path changes. Video is split into fragments for this reason
([Section 6.6](06-video.md#66-video-fragments)), and audio and input are sized to fit
([Section 7.4](07-audio.md#74-audio_packet-datagram),
[Section 8.6](08-input.md#86-client-rules)).

### 3.7.3 Processing rules

- A receiver MUST silently drop a datagram of unknown type.
- Datagrams can be lost, duplicated or reordered. Every datagram type defines how a
  receiver detects and handles this.
- The host drops client datagrams that arrive before it has sent `SESSION_START`.
- A client can receive datagrams, and host streams, after its authentication has
  succeeded but before it has processed `SESSION_START`, or for a stream whose
  `DISPLAY_CONFIG` or `AUDIO_CONFIG` is still in flight
  ([Section 1.5.7](01-introduction.md#157-identifiers)). It MUST buffer at least
  2 MiB of such data for at least one second and process it once the defining
  message arrives. It discards data beyond its buffer limits, data older than two
  seconds, and data that arrives before its authentication has succeeded.

## 3.8 Prioritization

When an endpoint has more data than its congestion window allows, it SHOULD send
data in this order of priority, highest first:

1. control stream data and input datagrams;
2. audio datagrams;
3. motion-layer video fragments, including retransmissions;
4. cursor image streams and passthrough output streams;
5. refinement-layer video fragments;
6. clipboard data streams.

Feedback, clock synchronization and device output datagrams are small and SHOULD be
sent with the same priority as input.

## 3.9 Connection migration

Clients SHOULD support QUIC connection migration so that a session survives a change
of network path, for example from Wi-Fi to wired Ethernet or from Wi-Fi to a mobile
network. Mobile clients in particular SHOULD migrate the connection when their
network changes rather than reconnecting. Hosts MUST NOT disable active migration
unless deployment policy requires it.

## 3.10 Errors and closing

A session ends when either endpoint closes the QUIC connection. QDP errors are
signalled with a CONNECTION_CLOSE frame of the application type (0x1d), whose error
code is a QDP error code ([Section 12.6](12-registries.md#126-error-codes)) and
whose reason phrase SHOULD describe the cause for logs. A normal end of session uses
`QDP_NO_ERROR`. QUIC does not allow application error codes before the handshake is
complete, so QDP errors are sent only after it.

Stream-level errors use RESET_STREAM and STOP_SENDING frames with the stream error
codes in the same registry.

## 3.11 Round-trip time

Timers in this specification use the **smoothed RTT**: the `smoothed_rtt` that QUIC
maintains for the connection ([RFC9002], Section 5.3). An implementation whose QUIC
stack does not expose it uses the delay of the best recent clock synchronization
sample instead ([Section 5.2.2](05-timing.md#522-offset-and-round-trip-time)).
[Section 5.6](05-timing.md#56-timers-and-limits) lists every timer and its default.

## 3.12 Encoding examples (non-normative)

A `CLIENT_READY` message (type 0x06) has an empty payload and is encoded as two
bytes:

```
06 00          Message Type = 0x06, Message Length = 0
```

A `TIME_PING` datagram ([Section 5.2](05-timing.md#52-clock-synchronization)) with
Ping ID 7 and Origin Timestamp 1,500,000 µs:

```
07             Datagram Type = 0x07
07             Ping ID = 7
80 16 e3 60    Origin Timestamp = 1500000 (4-byte form: 0x80000000 | 1500000)
```

## 3.13 Local network discovery

Hosts MAY advertise themselves on a local network with DNS-Based Service Discovery
[RFC6763] over Multicast DNS [RFC6762], so that clients can list them without the
user typing an address:

- **Service type**: `_qdp._udp`, in the domain `local.`.
- **Instance name**: the host's name for users, as in `HOST_HELLO`, at most 63
  bytes.
- **SRV record**: the UDP port the host listens on.
- **TXT record** ([RFC6763], Section 6), with these keys:
  - `alpn`: the ALPN tokens ([Section 3.2](#32-alpn)) the host accepts, separated
    by commas, such as `qdp-draft-00`;
  - `pin`: the host's key pin, the SHA-256 hash of its certificate's
    SubjectPublicKeyInfo, as 64 lower-case hexadecimal digits;
  - `pair`: `1` while the host offers `PAIRING`
    ([Section 4.4.9](04-session.md#449-pairing-0x06)); absent otherwise.

  Clients ignore keys they do not know.

Discovery records are not authenticated: anyone on the network can advertise any
name, address and pin. A client MUST NOT treat a discovered pin as verified. It MAY
use it to recognize a host whose key it already pinned, which the TLS handshake then
verifies against the stored pin, and SHOULD warn the user when a host advertises the
name of a known host with another pin. A host whose key the client does not know is
authenticated by pairing, or by any other means of
[Section 11.1](11-security.md#111-host-authentication).

Advertising tells the local network that a QDP host is there and its name. Hosts
SHOULD let the administrator turn it off, and stop advertising when they stop
accepting connections.
