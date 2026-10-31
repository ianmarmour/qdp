# 4. Session

## 4.1 Overview

After the QUIC handshake, a session passes through four phases on the control
stream:

1. **Hello**: the endpoints exchange versions and capabilities.
2. **Authentication**: the client proves its identity to the host.
3. **Configuration**: the client describes its monitors, audio and input
   devices; the host answers with the streams it will send.
4. **Active**: media, input, cursor and clipboard data flow until the
   connection closes.

## 4.2 Session states

| Client state | Entered when | Host state | Entered when |
|---|---|---|---|
| `AWAIT_HOST_HELLO` | it sends `CLIENT_HELLO` | `AWAIT_CLIENT_HELLO` | the QUIC handshake completes |
| `AUTHENTICATING` | it receives `HOST_HELLO` | `AUTHENTICATING` | it sends `HOST_HELLO` |
| `CONFIGURING` | it receives a successful `AUTH_RESULT` | `CONFIGURING` | it sends a successful `AUTH_RESULT` |
| `ACTIVE` | it receives `SESSION_START` | `ACTIVE` | it sends `SESSION_START` |

The table below lists when each control message may be sent. A message received
outside these rules is a connection error of type `QDP_PROTOCOL_VIOLATION`,
except for pipelined messages ([Section 4.4.6](#446-pipelining)).

| Message | Sender | Allowed when |
|---|---|---|
| `CLIENT_HELLO` | Client | First message on the control stream; exactly once |
| `HOST_HELLO` | Host | First message on the control stream; exactly once |
| `AUTH_START` | Client | `AUTHENTICATING` |
| `AUTH_CONTINUE` | Both | `AUTHENTICATING`, during a method that uses it |
| `AUTH_RESULT` | Host | `AUTHENTICATING` |
| `DISPLAY_REQUEST`, `AUDIO_REQUEST`, `KEYBOARD_INFO`, `DEVICE_ATTACH`, `DEVICE_DETACH` | Client | `CONFIGURING` or `ACTIVE` |
| `CLIENT_READY` | Client | `CONFIGURING`; exactly once |
| `DISPLAY_CONFIG`, `AUDIO_CONFIG`, `POINTER_MODE`, `CURSOR_SET`, `CURSOR_WARP` | Host | `CONFIGURING` after receiving `CLIENT_READY`, or `ACTIVE` |
| `SESSION_START` | Host | `CONFIGURING` after receiving `CLIENT_READY`; exactly once |
| `KEYFRAME_REQUEST` | Client | `ACTIVE` |
| `CLIPBOARD_OFFER`, `CLIPBOARD_REQUEST`, `CLIPBOARD_REJECT` | Both | `ACTIVE` |

Datagrams and unidirectional streams are used only in the `ACTIVE` state.

The host MUST close the connection with `QDP_HANDSHAKE_TIMEOUT` if it has not
received `CLIENT_READY` within 30 seconds of the QUIC handshake completing. The
client SHOULD close the connection with the same code if it has not received
`SESSION_START` within 30 seconds.

## 4.3 Hello messages

### 4.3.1 CLIENT_HELLO (0x01)

```
CLIENT_HELLO Payload {
  Minor Version (i),
  Client Name (String),
  Capability Count (i),
  Capability (..) ...,
}
```

- **Minor Version**: the highest minor version of this specification the client
  implements. This document defines minor version 0.
- **Client Name**: a human-readable product name and version, for logs and
  display, for example `qdp-client/0.1.0`. At most 255 bytes.
- **Capability Count** and **Capability**: the client's capabilities
  ([Section 4.6](#46-capabilities)). At most 64 capabilities.

### 4.3.2 HOST_HELLO (0x02)

```
HOST_HELLO Payload {
  Minor Version (i),
  Host Name (String),
  Auth Method Count (i),
  Auth Method (i) ...,
  Capability Count (i),
  Capability (..) ...,
}
```

- **Minor Version**: the highest minor version the host implements.
- **Host Name**: a human-readable name for the host, for example its computer
  name. At most 255 bytes. A host MAY send an empty name to avoid revealing it
  before authentication.
- **Auth Method**: the authentication methods the host accepts for this client,
  in the host's order of preference ([Section 4.4](#44-authentication)). There
  MUST be at least one.
- **Capability**: the host's capabilities. At most 64.

Both endpoints use the lower of the two minor versions. Because later minor
versions only add fields at the end of structures and add new negotiated
features, an endpoint never needs to reject a peer for having a different minor
version.

If the client and host share no video codec, the host MUST close the connection
with `QDP_NO_COMMON_CAPABILITY` after sending `HOST_HELLO`.

## 4.4 Authentication

### 4.4.1 Messages

```
AUTH_START Payload {
  Auth Method (i),
  Auth Data (Octets),
}

AUTH_CONTINUE Payload {
  Auth Data (Octets),
}

AUTH_RESULT Payload {
  Status (i),
  Auth Data (Octets),
  Reason (String),
}
```

The client begins an attempt with `AUTH_START` naming one of the offered
methods. The method defines any `AUTH_CONTINUE` exchange. The host ends the
attempt with `AUTH_RESULT`:

| Status | Name | Meaning |
|---|---|---|
| 0x00 | `SUCCESS` | The client is authenticated; both endpoints enter `CONFIGURING`. |
| 0x01 | `FAILED_RETRY` | The attempt failed; the client may start another attempt. |
| 0x02 | `FAILED_FINAL` | The attempt failed; the host will close the connection. |

`Reason` is a human-readable explanation of at most 255 bytes. It SHOULD NOT
reveal whether a user name exists. After three failed attempts on one
connection, a host MUST send `FAILED_FINAL` and close the connection with
`QDP_AUTH_FAILED`. Hosts SHOULD also limit the rate of failed attempts per
source address and per user name.

A host that receives `AUTH_START` for a method it did not offer MUST answer
`FAILED_RETRY`; the attempt counts as failed.

### 4.4.2 CLIENT_CERTIFICATE (0x01)

The client presented a certificate in the TLS handshake, and the host
authorizes it, typically by comparing the SHA-256 hash of its
SubjectPublicKeyInfo with a list of allowed keys. `AUTH_START` carries empty
Auth Data and the host answers with `AUTH_RESULT` directly. A host MUST NOT
offer this method on a connection where the client presented no certificate.

### 4.4.3 TOKEN (0x02)

`AUTH_START` carries an opaque token of at most 4096 bytes that the client
obtained out of band, for example from a connection broker. The host answers
with `AUTH_RESULT` directly. Tokens are bearer credentials; hosts SHOULD accept
only short-lived tokens and SHOULD reject a token that has been used before.

### 4.4.4 SCRAM_SHA_256_PLUS (0x03)

Password authentication uses SCRAM-SHA-256-PLUS [RFC5802] [RFC7677] with the
`tls-exporter` channel binding [RFC9266]. The channel binding data is the 32-byte
TLS exporter output with label `EXPORTER-Channel-Binding` and an empty context,
taken from the QUIC connection's TLS session, so a password exchange cannot be
relayed through another connection. The GS2 header is therefore
`p=tls-exporter,,`.

SCRAM messages are carried as UTF-8 bytes in Auth Data without SASL framing:

| Step | Message | Auth Data |
|---|---|---|
| 1 | `AUTH_START` | client-first-message |
| 2 | `AUTH_CONTINUE` (host) | server-first-message |
| 3 | `AUTH_CONTINUE` (client) | client-final-message |
| 4 | `AUTH_RESULT` | server-final-message on success; empty on failure |

On `SUCCESS`, the client MUST verify the server signature in the
server-final-message and MUST close the connection with `QDP_AUTH_FAILED` if
it does not match.

### 4.4.5 ANONYMOUS (0x04)

No authentication. A host MUST NOT offer this method unless an administrator has
explicitly enabled it, and clients SHOULD warn the user before using it.
`AUTH_START` carries empty Auth Data.

### 4.4.6 Pipelining

To shorten session setup, a client MAY:

- send `AUTH_START` immediately after `CLIENT_HELLO`, before receiving
  `HOST_HELLO`, if it knows from earlier sessions that the host accepts the
  method; and
- send its configuration messages and `CLIENT_READY`
  ([Section 4.5](#45-configuration)) immediately after its final authentication
  message.

The host processes pipelined messages only once the state they require is
reached. It MUST buffer up to 64 KiB of pipelined messages and MUST close the
connection with `QDP_RESOURCE_LIMIT` if a client sends more before
authentication succeeds. If the attempt fails, the host discards the pipelined
configuration messages and the client MUST send them again after a later
successful attempt. With a token or client certificate and pipelining, a session
becomes active one round trip after the QUIC handshake.

## 4.5 Configuration

In the `CONFIGURING` state the client MUST send `DISPLAY_REQUEST`
([Section 6.2](06-video.md#62-display_request-0x10)). It SHOULD send
`KEYBOARD_INFO` ([Section 8.2.1](08-input.md#821-keyboard_info-0x30)), MAY send
`AUDIO_REQUEST` ([Section 7.2](07-audio.md#72-audio_request-0x18)) and one
`DEVICE_ATTACH` per extra input device
([Section 8.2.2](08-input.md#822-device_attach-0x31)), and then sends:

```
CLIENT_READY Payload {
}
```

After receiving `CLIENT_READY`, the host MUST send, in this order:

1. `DISPLAY_CONFIG` ([Section 6.3](06-video.md#63-display_config-0x11));
2. `AUDIO_CONFIG`, if the client sent `AUDIO_REQUEST`;
3. `POINTER_MODE` ([Section 8.7](08-input.md#87-pointer_mode-0x33));
4. `CURSOR_SET` ([Section 9.3](09-cursor.md#93-cursor_set-0x38));
5. `SESSION_START`.

```
SESSION_START Payload {
}
```

The host may start sending media immediately after `SESSION_START`. The client
MUST NOT send datagrams or open unidirectional streams until it has received
`SESSION_START`.

## 4.6 Capabilities

### 4.6.1 Format

```
Capability {
  Capability ID (i),
  Capability Length (i),
  Capability Value (..),
}
```

A capability states that the sender implements a feature, and with what limits.
A feature is used only if both endpoints advertise it. Unless a capability is
marked *repeatable*, an endpoint MUST NOT send it more than once. A
`Capability Length` above 4096 is a connection error of type
`QDP_PROTOCOL_VIOLATION`.

### 4.6.2 VIDEO_CODEC (0x01), repeatable

```
VIDEO_CODEC Value {
  Codec (i),
  Profiles (8),
  Max Bit Depth (8),
  Max Width (i),
  Max Height (i),
  Max Pixel Rate (i),
  Flags (8),
}
```

The host lists what it can encode and the client what it can decode, one
capability per codec.

- **Codec**: 0x01 for AV1. Entries with an unknown codec are ignored.
- **Profiles**: AV1 profiles supported: 0x01 profile 0 (Main), 0x02 profile 1
  (High), 0x04 profile 2 (Professional).
- **Max Bit Depth**: 8, 10 or 12.
- **Max Width**, **Max Height**: largest frame dimensions in pixels.
- **Max Pixel Rate**: luma samples per second the endpoint can sustain across all
  video streams in low-latency mode, or 0 if not stated. Hosts SHOULD keep the
  total within the client's value.
- **Flags**:
  - 0x01 `HARDWARE_PROFILE_0`: profile 0 is hardware-accelerated.
  - 0x02 `HARDWARE_PROFILE_1`: profile 1 is hardware-accelerated.
  - 0x04 `LOSSLESS`: the endpoint can encode or decode lossless frames
    ([Section 6.4.3](06-video.md#643-refinement-layer)).

The host uses the flags to choose stream formats. For example, it can send 4:4:4
on the motion layer only to a client that decodes profile 1 in hardware.

### 4.6.3 REFINEMENT (0x02)

Empty value. The endpoint implements the refinement layer and layer composition
([Section 6.9](06-video.md#69-composition)).

### 4.6.4 AUDIO_CODEC (0x03), repeatable

```
AUDIO_CODEC Value {
  Codec (i),
  Max Channels (i),
  Directions (8),
}
```

- **Codec**: 0x01 Opus, 0x02 PCM L16, 0x03 PCM L24 ([Section 7.5](07-audio.md#75-codecs)).
- **Max Channels**: 1 to 8.
- **Directions**: 0x01 `SEND` (the endpoint can encode and send this codec),
  0x02 `RECEIVE` (it can receive and decode it).

### 4.6.5 INPUT (0x04)

```
INPUT Value {
  Input Classes (i),
}
```

For the client, the classes it can capture; for the host, the classes it can
inject. A class is used only if both set it.

| Mask | Class |
|---|---|
| 0x01 | `KEYBOARD` |
| 0x02 | `POINTER_ABSOLUTE` |
| 0x04 | `POINTER_RELATIVE` |
| 0x08 | `GAMEPAD` |
| 0x10 | `TOUCH` |
| 0x20 | `PEN` |
| 0x40 | `TEXT` (Unicode text events) |
| 0x80 | `DEVICE_OUTPUT` (rumble and LEDs) |

### 4.6.6 CLIPBOARD (0x05)

```
CLIPBOARD Value {
  Directions (8),
  Max Size (i),
}
```

- **Directions**: 0x01 `SEND` (the endpoint offers its clipboard), 0x02
  `RECEIVE` (it accepts the peer's clipboard). Hosts set these from
  administrative policy.
- **Max Size**: the largest clipboard data, in bytes, the endpoint accepts.

### 4.6.7 MONITORS (0x06)

```
MONITORS Value {
  Max Monitors (i),
}
```

The number of monitors the endpoint can stream (host) or display (client). An
endpoint that does not send this capability supports one monitor.

### 4.6.8 HDR (0x07)

```
HDR Value {
  Colorimetry (8),
}
```

High dynamic range formats the endpoint can encode (host) or present (client):
0x01 BT.2020 primaries with the PQ transfer function, 0x02 BT.2020 primaries
with the HLG transfer function.

## 4.7 Ending a session

Either endpoint ends the session by closing the QUIC connection
([Section 3.10](03-transport.md#310-errors-and-closing)). A host that accepts a
new session for the same user while one is active MAY close the old session
with `QDP_SESSION_REPLACED`. A host that is shutting down or logging the user
out SHOULD close sessions with `QDP_HOST_SHUTDOWN`. When a session ends, the host
MUST release every key and button it pressed on the client's behalf
([Section 8.6](08-input.md#86-host-processing)).
