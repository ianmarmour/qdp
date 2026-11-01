# 4. Session

## 4.1 Overview

After the QUIC handshake, a session passes through four phases on the control
stream:

1. **Hello**: the endpoints exchange versions and capabilities.
2. **Authentication**: the client proves its identity to the host.
3. **Configuration**: the client describes its monitors, audio and input devices;
   the host answers with the streams it will send and the devices it accepts.
4. **Active**: media, input, cursor and clipboard data flow until the connection
   closes.

## 4.2 Session states

| Client state | Entered when | Host state | Entered when |
|---|---|---|---|
| `AWAIT_HOST_HELLO` | it sends `CLIENT_HELLO` | `AWAIT_CLIENT_HELLO` | the QUIC handshake completes |
| `AUTHENTICATING` | it receives `HOST_HELLO` | `AUTHENTICATING` | it sends `HOST_HELLO` |
| `CONFIGURING` | it receives a successful `AUTH_RESULT` | `CONFIGURING` | it sends a successful `AUTH_RESULT` |
| `ACTIVE` | it receives `SESSION_START` | `ACTIVE` | it sends `SESSION_START` |

The table below lists when each control message may be sent. A message received
outside these rules is a connection error of type `QDP_PROTOCOL_VIOLATION`, except
for pipelined messages ([Section 4.4.6](#446-pipelining)).

| Message | Sender | Allowed when |
|---|---|---|
| `CLIENT_HELLO` | Client | First message on the control stream; exactly once |
| `HOST_HELLO` | Host | First message on the control stream; exactly once |
| `AUTH_START` | Client | `AUTHENTICATING` |
| `AUTH_CONTINUE` | Both | `AUTHENTICATING`, during a method that uses it |
| `AUTH_RESULT` | Host | `AUTHENTICATING` |
| `DISPLAY_REQUEST`, `AUDIO_REQUEST`, `KEYBOARD_INFO`, `DEVICE_ATTACH`, `DEVICE_DETACH` | Client | `CONFIGURING` or `ACTIVE` |
| `CLIENT_READY` | Client | `CONFIGURING`; exactly once per successful authentication |
| `DEVICE_STATUS` | Host | `CONFIGURING` or `ACTIVE`, answering a `DEVICE_ATTACH` |
| `HID_FEATURE_REQUEST` | Host | `CONFIGURING` or `ACTIVE`, for a device it has accepted |
| `HID_FEATURE_RESPONSE` | Client | `CONFIGURING` or `ACTIVE`, answering a request |
| `DISPLAY_CONFIG`, `AUDIO_CONFIG`, `POINTER_MODE`, `CURSOR_SET`, `CURSOR_WARP` | Host | `CONFIGURING` after receiving `CLIENT_READY`, or `ACTIVE` |
| `SESSION_START` | Host | `CONFIGURING` after receiving `CLIENT_READY`; exactly once |
| `RESUMPTION_TOKEN` | Host | `ACTIVE` |
| `KEYFRAME_REQUEST` | Client | `ACTIVE` |
| `CLIPBOARD_OFFER`, `CLIPBOARD_REQUEST`, `CLIPBOARD_REJECT` | Both | `ACTIVE` |

Datagrams are used only in the `ACTIVE` state, and unidirectional streams only as
[Section 3.5.2](03-transport.md#352-unidirectional-streams) allows.

The host MUST close the connection with `QDP_HANDSHAKE_TIMEOUT` if it has not
received `CLIENT_HELLO` within 10 seconds of the QUIC handshake completing, if
authentication has not succeeded within 120 seconds, which leaves time for a user
to type a password, or if it has not received `CLIENT_READY` within 30 seconds of a
successful `AUTH_RESULT`. The client SHOULD close the connection with the same code
if it has not received `SESSION_START` within 30 seconds of sending `CLIENT_READY`.

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
- **Client Name**: a human-readable product name and version, for logs and display,
  for example `qdp-client/0.1.0`. At most 255 bytes.
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
- **Host Name**: a human-readable name for the host, for example its computer name.
  At most 255 bytes. A host MAY send an empty name to avoid revealing it before
  authentication.
- **Auth Method**: the authentication methods the host accepts for this client, in
  the host's order of preference ([Section 4.4](#44-authentication)). There MUST be
  at least one.
- **Capability**: the host's capabilities. At most 64.

Both endpoints use the lower of the two minor versions. Later minor versions only
add fields at the end of extensible structures
([Section 1.6](01-introduction.md#16-extensibility)) and add features that are used
only when negotiated, so an endpoint never needs to reject a peer for having a
different minor version.

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

The client begins an attempt with `AUTH_START` naming one of the offered methods.
The method defines any `AUTH_CONTINUE` exchange; Auth Data is at most 8192 bytes in
every message. The host ends the attempt with `AUTH_RESULT`:

| Status | Name | Meaning |
|---|---|---|
| 0x00 | `SUCCESS` | The client is authenticated; both endpoints enter `CONFIGURING`. |
| 0x01 | `FAILED_RETRY` | The attempt failed; the client may start another attempt. |
| 0x02 | `FAILED_FINAL` | The attempt failed; the host will close the connection. |
| 0x03 | `NEXT_FACTOR` | The attempt succeeded, and the user must also authenticate with another method. |

A host MAY require more than one factor, for example a password or the client's
key, then a one-time password ([Section 4.4.10](#4410-totp-0x09)). It then answers
an attempt that succeeded, but is not the last factor, with `NEXT_FACTOR`, whose
Auth Data lists the methods the next attempt may use:

```
NEXT_FACTOR Auth Data {
  Method Count (i),
  Auth Method (i) ...,
  [Enrollment (String)],
}
```

The client starts the next attempt with one of them, on the same connection, and
the session state stays `AUTHENTICATING` until an attempt ends with `SUCCESS`.
Enrollment is present only to enroll a one-time password secret
([Section 4.4.10](#4410-totp-0x09)). A failed attempt at a later factor starts the
user over at the first, and counts as one failed attempt toward the limits below.
Pipelined configuration is discarded after `NEXT_FACTOR` as after a failure
([Section 4.4.6](#446-pipelining)).

`Reason` is a human-readable explanation of at most 255 bytes, and SHOULD be the
same for every kind of failure, so that it does not reveal whether a user name
exists. After three failed attempts on one connection, a host MUST send
`FAILED_FINAL` and close the connection with `QDP_AUTH_FAILED`. Hosts SHOULD also
limit the rate of failed attempts per source address and per user name.

A host that receives `AUTH_START` for a method it did not offer MUST answer
`FAILED_RETRY`; the attempt counts as failed.

### 4.4.2 CLIENT_CERTIFICATE (0x01)

The client presented a certificate in the TLS handshake, and the host authorizes it,
typically by comparing the SHA-256 hash of its SubjectPublicKeyInfo [RFC5280] with a
list of allowed keys. A host that accepts this method MUST request a client
certificate in the TLS handshake, because QUIC does not allow client authentication
after the handshake. `AUTH_START` carries empty Auth Data and the host answers with
`AUTH_RESULT` directly. A host MUST NOT offer this method on a connection where the
client presented no certificate.

### 4.4.3 TOKEN (0x02)

`AUTH_START` carries an opaque token of at most 4096 bytes that the client obtained
out of band, for example from a connection broker, or a resumption token from an
earlier session ([Section 4.4.7](#447-resumption_token-0x08)). The host answers with
`AUTH_RESULT` directly. Tokens are bearer credentials:

- hosts MUST reject tokens that were not issued for them, for example by checking
  that the token names the host's key;
- hosts SHOULD accept only short-lived tokens and SHOULD reject a token that has
  been used before;
- clients MUST NOT send a token to a host whose key they have not verified by
  pinning or by public key infrastructure
  ([Section 11.1](11-security.md#111-host-authentication)).

### 4.4.4 SCRAM_SHA_256_PLUS (0x03)

Password authentication against credentials stored by the QDP host uses
SCRAM-SHA-256-PLUS [RFC5802] [RFC7677] with the `tls-exporter` channel binding
[RFC9266]. The channel binding data is the 32-byte TLS exporter output with label
`EXPORTER-Channel-Binding` and an empty context, taken from the QUIC connection's TLS
session, so a password exchange cannot be relayed through another connection. The
GS2 header is therefore `p=tls-exporter,,`.

SCRAM messages are carried as UTF-8 bytes in Auth Data without SASL framing:

| Step | Message | Auth Data |
|---|---|---|
| 1 | `AUTH_START` | client-first-message |
| 2 | `AUTH_CONTINUE` (host) | server-first-message |
| 3 | `AUTH_CONTINUE` (client) | client-final-message |
| 4 | `AUTH_RESULT` | server-final-message on success; empty on failure |

In addition to [RFC5802]:

- User names are prepared with the UsernameCaseMapped profile and passwords with the
  OpaqueString profile of [RFC8265].
- The host MUST reject any GS2 header other than `p=tls-exporter,,`, and MUST check
  that the channel binding attribute `c=` equals the base64 encoding of the GS2
  header followed by the 32-byte exporter output.
- For a user name it does not know, the host MUST still answer step 1 with a
  server-first-message, using a salt derived deterministically from the user name
  and a host secret and its normal iteration count, and fail only at step 4, taking
  the same time as for a known user.
- Hosts MUST use an iteration count of at least 4096, and SHOULD use a much higher
  one. Clients MUST reject iteration counts below 4096 or above 10,000,000 by closing
  the connection with `QDP_AUTH_FAILED`.
- On `SUCCESS`, the client MUST verify the server signature in the
  server-final-message and MUST close the connection with `QDP_AUTH_FAILED` if it
  does not match.

### 4.4.5 ANONYMOUS (0x04)

No authentication. A host MUST NOT offer this method unless an administrator has
explicitly enabled it, and clients SHOULD warn the user before using it. `AUTH_START`
carries empty Auth Data.

### 4.4.6 Pipelining

To shorten session setup, a client MAY:

- send `AUTH_START` immediately after `CLIENT_HELLO`, before receiving `HOST_HELLO`,
  if it knows from an earlier session that the host accepts the method; and
- send its configuration messages and `CLIENT_READY`
  ([Section 4.5](#45-configuration)) immediately after its final authentication
  message, using the host capabilities it remembers from an earlier session.

The host processes pipelined messages only once the state they require is reached.
The client MUST NOT pipeline more than 64 KiB of messages before a successful
`AUTH_RESULT`, and MUST NOT pipeline passthrough device attachments; a host that
receives more closes the connection as
[Section 11.7](11-security.md#117-resource-exhaustion) describes. Because the
client's memory of the host's capabilities can be out of date, the host tolerates
mismatches in pipelined configuration: it configures at most as many monitors as it
supports, and refuses devices it does not support with `DEVICE_STATUS`.

If an attempt fails, the host discards every message other than authentication
messages that it received before the client's next `AUTH_START`. Discarded messages
do not count toward the 64 KiB limit or toward the rule that `CLIENT_READY` is sent
once, and the client MUST send its configuration again after a later successful
attempt. With a token or client certificate and pipelining, a session becomes active
one round trip after the QUIC handshake.

### 4.4.7 RESUMPTION_TOKEN (0x08)

```
RESUMPTION_TOKEN Payload {
  Token (Octets),
  Lifetime (i),
}
```

Ending a QDP connection does not end the user's desktop on the host
([Section 4.7](#47-ending-a-session)). So that a client can reconnect without asking
the user for credentials again, for example after a phone's operating system
suspended it and the connection timed out, the host MAY send `RESUMPTION_TOKEN` after
`SESSION_START`, and again whenever it rotates the token. Token is opaque and at most
256 bytes; Lifetime is the number of seconds it remains valid after the connection
ends.

A client reconnecting within the lifetime authenticates the new connection with the
`TOKEN` method, using this token, and sees the desktop as it is now. The host MUST
accept each resumption token at most once, MUST bind it to the identity that
authenticated the original session, and SHOULD issue a fresh token after each
successful resumption. Clients store resumption tokens as described in
[Section 11.2](11-security.md#112-client-authentication).

### 4.4.8 PASSWORD (0x05)

Some hosts must check a user's operating system account password, for example a
domain account, which SCRAM cannot do because the host stores no SCRAM verifier for
it. With the `PASSWORD` method, `AUTH_START` carries:

```
PASSWORD Auth Data {
  User Name (String),
  Password (String),
}
```

encoded inside the Auth Data Octets. The host verifies them with its operating
system and answers with `AUTH_RESULT`. The password is protected by TLS but is
revealed to the host, so a client MUST send it only to a host whose key it has
verified by pinning or by public key infrastructure, never on a connection trusted
only on first use. Hosts MUST rate-limit failures as for every method. How an
authenticated identity maps to an operating system account and desktop is decided by
the host.

### 4.4.9 PAIRING (0x06)

Pairing lets a user authorize a new client by typing, once, a short code that the
host shows, with no key pin, token or password to copy between devices. It
authenticates both endpoints: the host learns that the client is the one whose user
can see the host's display, and the client learns that it reached the host showing
the code, so a client that has not verified the host's key may use it
([Section 11.1](11-security.md#111-host-authentication)). On success the host
authorizes the client's certificate key for `CLIENT_CERTIFICATE` in later sessions,
and the client pins the host's key.

The client MUST present a certificate in the TLS handshake, and a host MUST NOT offer
`PAIRING` on a connection where the client presented none. A host offers `PAIRING`
only while pairing is enabled, which an administrator MUST be able to turn off.

**Code.** A pairing code is four decimal digits, chosen uniformly from 0000 to 9999
with a cryptographically secure random number generator, for example by rejection
sampling. The host chooses a new code for every exchange; a code is used for one
exchange only and expires 120 seconds after the host shows it. A guess succeeds with
probability one in 10,000, which the exchange and the limits below keep from adding
up: each exchange allows one guess, and the host allows few exchanges.

**Exchange.** The endpoints run SPAKE2 [RFC9382] with the ciphersuite
`SPAKE2-P256-SHA256-HKDF-HMAC`. The client is party A and the host is party B:

| Step | Message | Auth Data |
|---|---|---|
| 1 | `AUTH_START` | Device Name |
| 2 | `AUTH_CONTINUE` (host) | pB |
| 3 | `AUTH_CONTINUE` (client) | pA, followed by the client's confirmation cA |
| 4 | `AUTH_RESULT` | The host's confirmation cB on `SUCCESS`; empty on failure |

```
PAIRING Start Auth Data {
  Device Name (String),
}
```

- **Device Name**: the client device's name for users, such as "Living room
  tablet", at most 64 bytes. It is not authenticated and serves only to tell the
  user which device is asking.
- **pA**, **pB**: SEC1 uncompressed points of 65 bytes. **cA**, **cB**: HMAC-SHA256
  outputs of 32 bytes.

On step 1 the host chooses a code and shows it to the user at the host together with
the Device Name and the client's address, for example in a system notification,
then chooses y and sends pB. On step 2 the client asks its user for the code shown
on the host, chooses x and sends pA and cA. The parameters of [RFC9382] are:

- **Identities**: A is the 32-byte key pin of the client's certificate and B the
  32-byte key pin of the host's certificate (the SHA-256 hash of the
  SubjectPublicKeyInfo), each as the endpoint observed them in the TLS handshake. A
  host or client in the middle of the connection holds other keys, so the two
  endpoints' transcripts differ and the exchange fails.
- **w**: `OS2IP(HKDF-SHA256(IKM = code, salt = nil, info = "QDP pairing code",
  L = 40)) mod p`, where the code is its four ASCII digits and p is the order of
  P-256. No memory-hard function is used: the code is never stored and
  is used for one exchange, so there is no verifier to guess against offline.
- **AAD**: the 32-byte `tls-exporter` channel binding of
  [Section 4.4.4](#444-scram_sha_256_plus-0x03), which binds the confirmations to
  the connection.

Endpoints MUST reject a pA or pB that is not a point of P-256 or is the point at
infinity, and MUST abort if K is the point at infinity. Confirmations are compared
in constant time.

The host:

- verifies cA before sending anything that depends on the code, and sends cB only
  on success. If cA does not match, the code has expired, or the client sent
  something else, it answers `FAILED_RETRY`; the code is used up, and another
  attempt starts again at step 1 with a new code;
- allows at most one pairing exchange at a time, answering `FAILED_RETRY` to a
  pairing attempt while another is in progress;
- after three consecutive failed exchanges, MUST refuse pairing for at least one
  minute; after a fourth for three minutes, and after a fifth for five; and after
  each further failure for twice the time before, until an exchange succeeds or the
  host's user resets it. A user who mistypes waits a minute or so, while an
  attacker who cannot see the host's display gets about twenty guesses in a year,
  each of which the host shows to its user: a chance of about 0.2%;
- on success, records the client's key pin, with its Device Name, as authorized for
  `CLIENT_CERTIFICATE`, and SHOULD let the user list paired clients and remove
  them.

The client:

- MAY accept the host's certificate without verifying it, since the exchange
  authenticates the host, but MUST still verify the TLS handshake's signatures;
- MUST NOT send any other credential, and MUST NOT pipeline messages
  ([Section 4.4.6](#446-pipelining)) after step 3, until it has verified cB, and
  MUST close the connection with `QDP_AUTH_FAILED` if cB does not match;
- on success, pins the host's key ([Section 11.1](11-security.md#111-host-authentication))
  and SHOULD authenticate later connections to the host with `CLIENT_CERTIFICATE`;
- keeps its private key in the platform's protected storage where it can
  ([Section 11.2](11-security.md#112-client-authentication)).

### 4.4.10 TOTP (0x09)

A time-based one-time password [RFC6238] from an authenticator app on the user's
phone, as a second factor after one that identifies the user, for example with
`PASSWORD`, `CLIENT_CERTIFICATE` or a gateway's directory password
([Section 13.4](13-gateway.md#134-authentication)). A host or gateway offers
`TOTP` only in `NEXT_FACTOR`, never as the first factor. `AUTH_START` carries the
code's ASCII digits as Auth Data, and the host answers with `AUTH_RESULT`
directly.

Inside a private network a second factor is optional: a host MAY require `TOTP`,
as its administrator chooses. At the network's edge it is not: gateways always
require it ([Section 13.4](13-gateway.md#134-authentication)), and a host that
accepts connections from the internet, for example through a forwarded port,
SHOULD require `TOTP` of clients connecting from outside its local networks,
while it MAY let clients on them authenticate with one factor.

Codes follow the parameters every common authenticator app supports: 6 digits,
computed with HMAC-SHA-1 [RFC4226] over 30-second steps counted from the Unix
epoch, from a secret of at least 20 random bytes. The verifier:

- accepts the code of the current step and of the step before and after, to allow
  for clock skew, and compares codes in constant time;
- MUST NOT accept a code of a step at or before the last step it accepted for the
  same user, so that a code someone else saw cannot be used again;
- keeps the secrets where only it can read them, as they let anyone make the
  user's codes, and lets administrators reset a user's secret.

**Enrollment.** A verifier MAY let a user without a secret enroll one as they
authenticate. Its `NEXT_FACTOR` result then names `TOTP` and carries Enrollment:
a `otpauth://totp/` URI naming the issuer and the user, with the new secret in
base32 [RFC4648] and the algorithm, digits and period above, which the client shows
as a QR code and as text for the user's authenticator app. The next attempt's code
proves that the app holds the secret; the verifier keeps the secret only once a
code is accepted, and offers no enrollment to a user who has one. Because
enrollment lets whoever passed the first factor choose the second, verifiers
SHOULD offer it only where that is acceptable, for example while an organization
rolls out second factors, or once after an administrator reset the user's secret.

## 4.5 Configuration

In the `CONFIGURING` state the client MUST send `DISPLAY_REQUEST`
([Section 6.2](06-video.md#62-display_request-0x10)) and one `DEVICE_ATTACH` for each
required input device, at least `KEYBOARD`, `POINTER` and `MOUSE`
([Section 8.2.1](08-input.md#821-device_attach-0x31)). It SHOULD send
`KEYBOARD_INFO` ([Section 8.2.3](08-input.md#823-keyboard_info-0x30)), MAY send
`AUDIO_REQUEST` ([Section 7.2](07-audio.md#72-audio_request-0x18)) and
`DEVICE_ATTACH` messages for other devices, and then sends:

```
CLIENT_READY Payload {
}
```

The host answers each `DEVICE_ATTACH` with `DEVICE_STATUS` as it processes it. After
receiving `CLIENT_READY`, the host MUST send, in this order:

1. `DEVICE_STATUS` for any device attached during configuration that it has not yet
   answered;
2. `DISPLAY_CONFIG` ([Section 6.3](06-video.md#63-display_config-0x11));
3. `AUDIO_CONFIG`, if the client sent `AUDIO_REQUEST`;
4. `POINTER_MODE` ([Section 8.9](08-input.md#89-pointer_mode-0x33));
5. `CURSOR_SET` ([Section 9.3](09-cursor.md#93-cursor_set-0x38));
6. `SESSION_START`.

```
SESSION_START Payload {
}
```

The host may start sending media immediately after `SESSION_START`. The client MUST
NOT send datagrams or open unidirectional streams until it has received
`SESSION_START`. Datagrams and streams from the host can overtake `SESSION_START`;
the client buffers them as [Section 3.7.3](03-transport.md#373-processing-rules)
describes.

## 4.6 Capabilities

### 4.6.1 Format

```
Capability {
  Capability ID (i),
  Capability Length (i),
  Capability Value (..),
}
```

A capability states that the sender implements a feature, and with what limits. A
feature is used only if both endpoints advertise it. Unless a capability is marked
*repeatable*, an endpoint MUST NOT send it more than once. A `Capability Length`
above 4096 is a connection error of type `QDP_PROTOCOL_VIOLATION`.

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
  Max Level (8),
}
```

The host lists what it can encode and the client what it can decode, one capability
per codec.

- **Codec**: 0x01 for AV1. Entries with an unknown codec are ignored.
- **Profiles**: AV1 profiles supported: 0x01 profile 0 (Main), 0x02 profile 1 (High).
  The value 0x04, profile 2, is reserved and MUST NOT be used in this version.
- **Max Bit Depth**: 8 or 10. The value 12 is reserved for later versions.
- **Max Width**, **Max Height**: largest frame dimensions in pixels. The host MUST
  NOT configure a monitor larger than the client's limits.
- **Max Pixel Rate**: luma samples per second the endpoint can sustain across all
  video streams in low-latency mode, or 0 if not stated. Hosts SHOULD keep the total
  within the client's value.
- **Flags**:
  - 0x01 `HARDWARE_PROFILE_0`: profile 0 is hardware-accelerated.
  - 0x02 `HARDWARE_PROFILE_1`: profile 1 is hardware-accelerated.
  - 0x04 `LOSSLESS`: the endpoint can encode or decode lossless frames
    ([Section 6.4.3](06-video.md#643-refinement-layer)).
- **Max Level**: the highest AV1 level (`seq_level_idx`) the endpoint supports, or 31
  for no limit. Every stream's sequence header MUST signal a level the stream meets
  and that does not exceed the client's Max Level.

The host uses the flags to choose stream formats. For example, it can send 4:4:4 on
the motion layer only to a client that decodes profile 1 in hardware.

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
- **Directions**: 0x01 `SEND` (the endpoint can encode and send this codec), 0x02
  `RECEIVE` (it can receive and decode it).

### 4.6.5 INPUT (0x04)

```
INPUT Value {
  Input Classes (i),
}
```

For the client, the kinds of input device it can attach; for the host, the kinds it
can inject. A class is used only if both set it.

| Mask | Class |
|---|---|
| 0x01 | `KEYBOARD` |
| 0x02 | `POINTER` (absolute) |
| 0x04 | `MOUSE` (relative) |
| 0x08 | `TOUCHSCREEN` |
| 0x10 | `PEN` |
| 0x20 | `TOUCHPAD` |
| 0x40 | `GAMEPAD` |
| 0x80 | `TEXT` |
| 0x100 | `PASSTHROUGH` |

### 4.6.6 CLIPBOARD (0x05)

```
CLIPBOARD Value {
  Directions (8),
  Max Size (i),
}
```

- **Directions**: 0x01 `SEND` (the endpoint offers its clipboard), 0x02 `RECEIVE`
  (it accepts the peer's clipboard). Hosts set these from administrative policy.
- **Max Size**: the largest clipboard data, in bytes, the endpoint accepts.

### 4.6.7 MONITORS (0x06)

```
MONITORS Value {
  Max Monitors (i),
}
```

The number of monitors the endpoint can stream (host) or display (client). An
endpoint that does not send this capability supports one monitor. The host MUST NOT
configure more monitors than the client supports.

### 4.6.8 HDR (0x07)

```
HDR Value {
  Formats (8),
}
```

A mask of the high dynamic range formats the endpoint can encode (host) or present
(client), in any combination: 0x01 BT.2020 primaries with the PQ transfer function,
0x02 BT.2020 primaries with the HLG transfer function.

## 4.7 Ending a session

Either endpoint ends the session by closing the QUIC connection
([Section 3.10](03-transport.md#310-errors-and-closing)). Ending a QDP connection
does not end the user's desktop on the host: a client that reconnects, with fresh
credentials or a resumption token ([Section 4.4.7](#447-resumption_token-0x08)), sees
the desktop as it is now. Hosts MAY end an abandoned desktop after an administrative
timeout, and SHOULD lock it while no client is connected
([Section 11.4](11-security.md#114-local-control-and-privacy)).

A host that accepts a new session for the same user while one is active MAY close the
old session with `QDP_SESSION_REPLACED`. A host that is shutting down or logging the
user out SHOULD close sessions with `QDP_HOST_SHUTDOWN`. When a session ends, the host
MUST return every input device to rest ([Section 8.7](08-input.md#87-host-rules)).
