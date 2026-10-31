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
extension. The token for the final version 1 of this specification will be
`qdp/1`. Implementations of a draft MUST use the token `qdp-draft-NN`, where NN
is the two-digit draft number; this document is draft 00, so its token is
`qdp-draft-00`. A host MUST abort the handshake if the client offers no token
the host supports.

The ALPN token identifies the major version. Compatible additions within a major
version are signalled by the minor version in the hello messages and by
capabilities ([Section 4.3](04-session.md#43-hello-messages)).

## 3.3 Transport parameters

- Both endpoints MUST send the `max_datagram_frame_size` transport parameter
  (0x20) [RFC9221] with a value of at least 1200, and SHOULD send 65535. If the
  peer does not, the endpoint MUST close the connection with
  `QDP_TRANSPORT_REQUIREMENTS`.
- The host MUST allow the client to open at least one bidirectional stream.
- Each endpoint MUST allow its peer to open at least 16 unidirectional streams,
  and SHOULD allow at least 64.
- Endpoints SHOULD set `max_idle_timeout` to 30 seconds and SHOULD send QUIC
  PING frames when no other packets have been sent for 10 seconds, so that an
  idle desktop does not end the session.

## 3.4 Early data

Endpoints MUST NOT send QDP data in 0-RTT packets, and hosts MUST NOT accept
0-RTT data. Replayed input could repeat keystrokes or clicks on the host.

## 3.5 Streams

### 3.5.1 Control stream

The control stream is the first client-initiated bidirectional QUIC stream
(stream ID 0). The client opens it immediately after the QUIC handshake and
sends `CLIENT_HELLO` as its first message. Each direction of the control stream
carries a sequence of control messages ([Section 3.6](#36-control-messages)).

Neither endpoint may close or reset either direction of the control stream
before the session ends. An endpoint that sees the control stream closed or
reset MUST close the connection with `QDP_CLOSED_CRITICAL_STREAM`.

No other bidirectional streams are defined. A host that receives another
client-initiated bidirectional stream, or a client that receives a
host-initiated bidirectional stream, MUST close the connection with
`QDP_STREAM_CREATION_ERROR`, unless an extension negotiated by capability says
otherwise.

### 3.5.2 Unidirectional streams

Unidirectional streams carry bulk data that must arrive intact: cursor images
and clipboard contents. Each unidirectional stream begins with a stream type:

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

A receiver that does not recognize a stream type MUST either abort reading the
stream with a STOP_SENDING frame carrying `QDP_STREAM_TYPE_UNKNOWN`, or read and
discard its data. It MUST NOT close the connection for this reason. A receiver
that gets a known stream type in the wrong direction MUST close the connection
with `QDP_STREAM_CREATION_ERROR`.

Unidirectional streams MUST NOT be opened before the session is active
([Section 4.2](04-session.md#42-session-states)); a receiver MUST reset any that
arrive earlier.

## 3.6 Control messages

### 3.6.1 Framing

```
Control Message {
  Message Type (i),
  Message Length (i),
  Message Payload (..),
}
```

`Message Length` is the number of bytes in `Message Payload`. It MUST NOT
exceed 1 MiB (1,048,576 bytes); a receiver that reads a larger length MUST
close the connection with `QDP_MESSAGE_TOO_LARGE`. Data larger than this
belongs on a unidirectional stream.

The message types are listed in [Section 12.1](12-registries.md#121-control-message-types).
Payload layouts in later sections omit the `Message Type` and `Message Length`
fields.

### 3.6.2 Processing rules

- Control messages are processed in the order received.
- A receiver MUST ignore a control message of unknown type.
- A receiver MUST ignore bytes after the last field it understands in a known
  message ([Section 1.6](01-introduction.md#16-extensibility)).
- A message that is not allowed in the current session state, or not allowed
  in the direction it arrived, is a connection error of type
  `QDP_PROTOCOL_VIOLATION`.

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
larger than the maximum its QUIC implementation reports for the current path,
which can be smaller than the peer's `max_datagram_frame_size`. Video, the only
large data carried in datagrams, is split into fragments for this reason
([Section 6.6](06-video.md#66-video-fragments)).

### 3.7.3 Processing rules

- A receiver MUST silently drop a datagram of unknown type.
- Datagrams that arrive before the session is active MUST be dropped.
- Datagrams can be lost, duplicated or reordered. Every datagram type defines
  how a receiver detects and handles this.

## 3.8 Prioritization

When an endpoint has more data than its congestion window allows, it SHOULD send
data in this order of priority, highest first:

1. control stream data and input datagrams;
2. audio datagrams;
3. motion-layer video fragments, including retransmissions;
4. cursor image streams;
5. refinement-layer video fragments;
6. clipboard data streams.

Feedback, clock synchronization and device output datagrams are small and
SHOULD be sent with the same priority as input.

## 3.9 Connection migration

Clients SHOULD support QUIC connection migration so that a session survives a
change of network path, for example from Wi-Fi to wired Ethernet. Hosts MUST
NOT disable active migration unless deployment policy requires it.

## 3.10 Errors and closing

A session ends when either endpoint closes the QUIC connection. QDP errors are
signalled with a CONNECTION_CLOSE frame of the application type (0x1d), whose
error code is a QDP error code ([Section 12.6](12-registries.md#126-error-codes))
and whose reason phrase SHOULD describe the cause for logs. A normal end of
session uses `QDP_NO_ERROR`.

Stream-level errors use RESET_STREAM and STOP_SENDING frames with the stream
error codes in the same registry.

## 3.11 Encoding examples (non-normative)

A `CLIENT_READY` message (type 0x06) has an empty payload and is encoded as two
bytes:

```
06 00          Message Type = 0x06, Message Length = 0
```

A `TIME_PING` datagram ([Section 5.2](05-timing.md#52-clock-synchronization))
with Ping ID 7 and Origin Timestamp 1,500,000 µs:

```
07             Datagram Type = 0x07
07             Ping ID = 7
80 16 e3 60    Origin Timestamp = 1500000 (4-byte form: 0x80000000 | 1500000)
```
