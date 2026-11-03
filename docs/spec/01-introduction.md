# 1. Introduction

## 1.1 Scope

The Quick Desktop Protocol (QDP) carries an interactive graphical session
between two computers: a **host**, whose displays, audio and input devices are
shared, and a **client**, which shows the host's displays, plays its audio and
sends user input back. QDP is designed for networks with round-trip times under
50 ms and hundreds of megabits per second of capacity. The same protocol serves
remote desktop work and game streaming; there are no separate modes.

This specification defines:

- how QDP uses QUIC [RFC9000] and QUIC datagrams [RFC9221];
- session establishment, authentication and capability negotiation;
- clock synchronization and display timing;
- video streams encoded with AV1 [AV1], and how a client composites them;
- audio streams encoded with Opus [RFC6716] or PCM;
- input from standard devices (keyboard, pointer, mouse, touchscreen, pen,
  touchpad, gamepad and text) and the passthrough of HID input devices
  [HID-CLASS];
- the remote cursor and the clipboard.

It does not define file transfer, redirection of devices other than HID input
devices (for example raw USB devices, printers and smart cards), sessions with
more than one client, or how clients discover hosts. These are listed as future
work in the [design rationale](../design-rationale.md#9-future-work).

## 1.2 Requirements language

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT", "SHOULD",
"SHOULD NOT", "RECOMMENDED", "NOT RECOMMENDED", "MAY", and "OPTIONAL" in this
document are to be interpreted as described in BCP 14 [RFC2119] [RFC8174] when,
and only when, they appear in all capitals, as shown here.

Sections and paragraphs marked *non-normative* give guidance and examples; they
place no requirements on implementations.

## 1.3 Terminology

- **Host**: the endpoint whose displays are captured and whose input is
  controlled. The host is the QUIC server.
- **Client**: the endpoint that displays the session and captures user input.
  The client is the QUIC client.
- **Endpoint**: either the host or the client. **Peer**: the other endpoint.
- **Session**: one QUIC connection carrying QDP, from the QUIC handshake until
  the connection closes. Ending a session does not end the user's desktop on the
  host ([Section 4.7](04-session.md#47-ending-a-session)).
- **Monitor**: a display surface on the host, real or virtual, that is streamed
  to the client.
- **Video stream**: a sequence of AV1 temporal units carrying one layer of one
  monitor. Not to be confused with a QUIC stream.
- **Layer**: the role of a video stream. The **motion layer** carries every
  change to a monitor with the lowest possible latency. The optional
  **refinement layer** re-sends content that has stopped changing at higher
  fidelity, up to bit-exact.
- **Temporal unit (TU)**: all AV1 OBUs for one time instant, as defined by
  [AV1]. In QDP each temporal unit contains exactly one displayed frame.
- **Frame**: one temporal unit together with its QDP Frame Header
  ([Section 6.5](06-video.md#65-frame-payload)), identified within its video
  stream by a Frame Number.
- **Fragment**: the part of a frame carried in one QUIC datagram.
- **Composition block**: a 64 × 64 pixel square of a monitor. Composition
  blocks are the unit in which the client combines layers.
- **Host desktop**: the pixel space in which the host lays out its monitors
  ([Section 1.5.6](#156-positions)).
- **Input device**: a source of input that the client attaches to the session,
  such as a keyboard or a game controller. Each input device produces **input
  reports** ([Section 8](08-input.md)).
- **Session clock**: an endpoint's monotonic microsecond clock
  ([Section 5.1](05-timing.md#51-session-clocks)).
- **Smoothed RTT**: the connection's smoothed round-trip time as QUIC measures it
  ([Section 3.11](03-transport.md#311-round-trip-time)).

## 1.4 Notation

Wire formats are described with the notation of [RFC9000], Section 1.3:

- `x (A)`: x is A bits long.
- `x (i)`: x is a variable-length integer ([Section 1.5.1](#151-variable-length-integers)).
- `x (A..B)`: x can be any length from A to B bits; `x (..)` has no upper bound.
- `x (L) = C`: x has the fixed value C, with length L.
- `x (L) = C..D`: x has a value in the range C to D inclusive.
- `[x (L)]`: x is optional; the text states when it is present.
- `x (L) ...`: x is repeated zero or more times.

Complex fields are named and followed by a list of fields in braces. Fields are
transmitted in the order listed. QDP adds one form:

- `x (si)`: x is a signed variable-length integer
  ([Section 1.5.2](#152-signed-variable-length-integers)).

Flag fields are described by the hexadecimal mask of each flag, for example
`0x01`. A mask applies to the field's value as an unsigned integer; this avoids
any ambiguity about bit numbering.

## 1.5 Common data types

### 1.5.1 Variable-length integers

Variable-length integers use the encoding of [RFC9000], Section 16. The two
most significant bits of the first byte give the length of the encoding; the
remaining bits hold the value in network byte order:

| 2 MSB | Length | Usable bits | Range |
|---|---|---|---|
| 00 | 1 byte | 6 | 0 to 63 |
| 01 | 2 bytes | 14 | 0 to 16383 |
| 10 | 4 bytes | 30 | 0 to 1073741823 |
| 11 | 8 bytes | 62 | 0 to 4611686018427387903 |

Senders SHOULD use the shortest encoding. Receivers MUST accept any valid
encoding of a value.

### 1.5.2 Signed variable-length integers

A signed value n is mapped to an unsigned value with zigzag encoding, then
encoded as a variable-length integer: n ≥ 0 maps to 2n, and n < 0 maps to
−2n − 1. So 0, −1, 1, −2, 2 encode as 0, 1, 2, 3, 4.

### 1.5.3 Fixed-width integers

Fixed-width integers, for example `x (16)`, are unsigned and in network byte
order (big-endian) unless the field description says they are signed. Signed
fixed-width integers use two's complement.

### 1.5.4 Strings and octet sequences

```
String {
  Length (i),
  Value (..),
}

Octets {
  Length (i),
  Value (..),
}
```

`Length` is the number of bytes in `Value`. A String's value MUST be valid
UTF-8 without a byte order mark or NUL characters; a receiver that finds invalid
UTF-8 MUST treat it as a connection error of type `QDP_PROTOCOL_VIOLATION`. An
Octets value is arbitrary binary data. Field descriptions give maximum lengths.

### 1.5.5 Timestamps and durations

A **Timestamp** is a variable-length integer counting microseconds on the
sender's session clock ([Section 5.1](05-timing.md#51-session-clocks)). A
**duration** is a variable-length integer counting microseconds unless the field
description says otherwise.

### 1.5.6 Positions

The **host desktop** is a pixel space that the host defines. Each monitor of a
`DISPLAY_CONFIG` ([Section 6.3](06-video.md#63-display_config-0x11)) occupies
the rectangle of pixels from (X, Y) to (X + Width − 1, Y + Height − 1), where X
and Y are its Host Monitor X and Y fields and Width and Height its size. All four
count pixels of the monitor's video, whatever unit the host's operating system
uses for its own desktop. The rectangles of different monitors MUST NOT overlap;
a host that mirrors one display onto another reports it as one monitor. Gaps
between monitors are allowed. The host converts between the host desktop and its
operating system's coordinates itself.

For example, a host whose built-in display is 2880 × 1800 pixels at 200% scale,
with a 1920 × 1080 display to its right, reports the built-in display at X = 0
and the external display at X = 2880, even if its operating system places the
external display at 1440 points.

A **position** is a point of the host desktop in units of 1/256 pixel: the
top-left corner of desktop pixel (x, y) is position (256x, 256y). X increases to
the right and Y increases downward. Positions are signed, because a monitor can
lie above or to the left of the origin. They are sent as signed variable-length
integers (`(si)`) in control messages and as signed 32-bit integers in input
reports. A host that receives a position outside every monitor applies it at the
nearest point of the nearest monitor.

A position always refers to the monitor layout of one `DISPLAY_CONFIG`. A
position in a control message from the host refers to the latest
`DISPLAY_CONFIG` the host sent before that message. Input reports name the
layout they were computed with ([Section 8.3](08-input.md#83-input-datagram)).

### 1.5.7 Identifiers

Identifiers (Stream ID, Monitor ID, Device ID, Cursor ID, Request ID, Offer ID
and so on) are variable-length integers chosen by the endpoint that defines the
object. The value 0 is reserved to mean "none" and never identifies an object.
Unless the section that defines an identifier says otherwise, an endpoint
assigns the identifiers of each kind in strictly increasing order and never
reuses one within a session. Video and audio Stream IDs are one kind.

An identifier is **live** from the control message that defines its object until
the message that ends it, for example a `DISPLAY_CONFIG` that no longer lists a
stream, or a `DEVICE_DETACH`. Datagrams and unidirectional streams are not
ordered with the control stream, and messages in the two directions cross, so an
endpoint routinely receives references to identifiers that are not live. Typical
cases are a fragment of a stream whose `DISPLAY_CONFIG` is still in flight, and
a `KEYFRAME_REQUEST`, `NACK` or `LOSS` for a stream that a `DISPLAY_CONFIG` has
just ended. A receiver handles a reference to an identifier that is not live as
follows, and none of these cases is an error:

- If the peer defines identifiers of that kind and the identifier is higher than
  any the peer has defined so far, its defining message is still in flight. The
  receiver buffers the data and processes it when the definition arrives, within
  the limits of [Section 3.5.2](03-transport.md#352-unidirectional-streams) and
  [Section 3.7.3](03-transport.md#373-processing-rules), unless the section that
  defines the data says otherwise.
- Otherwise the object has ended, or never existed, and the receiver ignores the
  reference. A request that names it is refused in the way the request's section
  defines, for example with status `FAILED`.

## 1.6 Extensibility

Like HTTP/3 ([RFC9114], Section 9), QDP requires implementations to ignore
unknown or unsupported values in all extensible protocol elements. The rules
below are complete; [RFC9114] is cited for background only.

1. A receiver MUST ignore control messages, datagrams, records and capabilities
   of unknown type, skipping each by its length. A host MUST treat a device whose
   Device Kind it does not know as a device it does not support
   ([Section 8.2.1](08-input.md#821-device_attach-0x31)).
2. A receiver MUST either abort reading a unidirectional QUIC stream of unknown
   type or read and discard its data
   ([Section 3.5.2](03-transport.md#352-unidirectional-streams)), and MUST NOT
   treat it as a connection error.
3. The following structures are **extensible**, because the receiver knows
   where each one ends: control messages, records, capabilities, input reports
   of standard devices other than text, output reports of standard devices,
   datagrams whose last field is not data running to the end of the datagram
   (such as `TIME_PONG`), and any other structure whose size is given by a
   length field. When an extensible structure is longer than the fields this
   document defines for it, the receiver MUST ignore the extra bytes. Later
   minor versions add fields to extensible structures only at the end.
4. Every other structure is **frozen**: its layout never changes within a major
   version, because bytes added to it would be read as the data or entries that
   follow it. Frozen structures include the fields of every datagram that ends
   in data running to the end of the datagram (such as `VIDEO_FRAGMENT`,
   `AUDIO_PACKET` and `INPUT`), the headers of unidirectional streams, the data
   of text and passthrough reports, and repeated entries that have no length of
   their own. A later version changes a frozen structure only by defining a new
   type code, or a new layout that is used only when a capability enables it.
5. A receiver MUST treat a structure that is shorter than its defined fields as
   a connection error of type `QDP_PROTOCOL_VIOLATION`.
6. Flag bits not defined by this document are reserved. Senders MUST set them
   to zero and receivers MUST ignore them.
7. An endpoint MUST NOT use an extension until the peer has advertised support
   for it with a capability ([Section 4.6](04-session.md#46-capabilities)).

Values of the form `0x1f × N + 0x21` for integer N ≥ 0 (0x21, 0x40, 0x5f,
0x7e, ...) are reserved in QDP's type and identifier spaces: control message
types, datagram types, unidirectional stream types, capability IDs and record
types ([Sections 12.1 to 12.5](12-registries.md)). They never carry meaning and
are never assigned. They are not reserved among flag masks, enumerated field
values or error codes, which the defining sections list in full. Endpoints
SHOULD occasionally send a control message, datagram, record, capability or
unidirectional stream with a reserved type and arbitrary content, so that peers
that mishandle unknown values are found early. This practice is called
greasing, as in HTTP/3.

## 1.7 References

### 1.7.1 Normative references

- **[AV1]** Alliance for Open Media, *AV1 Bitstream & Decoding Process
  Specification*, version 1.0.0 with Errata 1.
- **[H273]** ITU-T H.273, *Coding-independent code points for video signal type
  identification*.
- **[HID]** USB Implementers Forum, *HID Usage Tables for Universal Serial Bus
  (USB)*, version 1.5.
- **[HID-CLASS]** USB Implementers Forum, *Device Class Definition for Human
  Interface Devices (HID)*, version 1.11 (report descriptors, report IDs, and
  input, output and feature reports).
- **[RFC2104]** HMAC: Keyed-Hashing for Message Authentication.
- **[RFC2119]** Key words for use in RFCs to Indicate Requirement Levels.
- **[RFC3190]** RTP Payload Format for 12-bit DAT Audio and 20- and 24-bit
  Linear Sampled Audio (L24).
- **[RFC3551]** RTP Profile for Audio and Video Conferences (L16).
- **[RFC4226]** HOTP: An HMAC-Based One-Time Password Algorithm.
- **[RFC4511]** Lightweight Directory Access Protocol (LDAP): The Protocol.
- **[RFC4648]** The Base16, Base32, and Base64 Data Encodings.
- **[RFC5280]** Internet X.509 Public Key Infrastructure Certificate and
  Certificate Revocation List (CRL) Profile (`SubjectPublicKeyInfo`).
- **[RFC5646]** Tags for Identifying Languages (BCP 47).
- **[RFC5802]** Salted Challenge Response Authentication Mechanism (SCRAM).
- **[RFC5869]** HMAC-based Extract-and-Expand Key Derivation Function (HKDF).
- **[RFC5905]** Network Time Protocol Version 4.
- **[RFC6238]** TOTP: Time-Based One-Time Password Algorithm.
- **[RFC6716]** Definition of the Opus Audio Codec.
- **[RFC6762]** Multicast DNS.
- **[RFC6763]** DNS-Based Service Discovery.
- **[RFC6838]** Media Type Specifications and Registration Procedures.
- **[RFC7301]** Transport Layer Security (TLS) Application-Layer Protocol
  Negotiation Extension (ALPN).
- **[RFC7677]** SCRAM-SHA-256 and SCRAM-SHA-256-PLUS.
- **[RFC7845]** Ogg Encapsulation for the Opus Audio Codec (channel mappings
  and multistream packing).
- **[RFC8032]** Edwards-Curve Digital Signature Algorithm (EdDSA) (Ed25519).
- **[RFC8174]** Ambiguity of Uppercase vs Lowercase in RFC 2119 Key Words.
- **[RFC8265]** Preparation, Enforcement, and Comparison of Internationalized
  Strings Representing Usernames and Passwords (PRECIS).
- **[RFC8446]** The Transport Layer Security (TLS) Protocol Version 1.3.
- **[RFC9000]** QUIC: A UDP-Based Multiplexed and Secure Transport.
- **[RFC9001]** Using TLS to Secure QUIC.
- **[RFC9002]** QUIC Loss Detection and Congestion Control.
- **[RFC9221]** An Unreliable Datagram Extension to QUIC.
- **[RFC9266]** Channel Bindings for TLS 1.3 (`tls-exporter`).
- **[RFC9382]** SPAKE2, a Password-Authenticated Key Exchange (the
  `SPAKE2-P256-SHA256-HKDF-HMAC` ciphersuite, with its M and N for P-256).
- **[RFC9525]** Service Identity in TLS.
- **[USB2]** USB Implementers Forum, *Universal Serial Bus Specification*,
  revision 2.0 (the `idVendor`, `idProduct` and `bcdDevice` fields of the device
  descriptor).
- **[GAMEPAD]** W3C, *Gamepad*, Working Draft, Section "Remapping": the standard
  gamepad layout of buttons 0 to 16 and axes 0 to 3, which QDP uses unchanged.
- **[POINTER]** W3C, *Pointer Events Level 2*, Recommendation, 4 April 2019 (pen
  pressure, tilt and twist).

### 1.7.2 Informative references

- **[RFC6143]** The Remote Framebuffer Protocol (VNC).
- **[RFC9114]** HTTP/3 (stream types, extensibility and greasing patterns).
- **[RFC9298]** Proxying UDP in HTTP (the tunnelling that gateways resemble).
- **[RFC9330]** Low Latency, Low Loss, and Scalable Throughput (L4S).
- **[AV1-RTP]** Alliance for Open Media, *RTP Payload Format for AV1*.
- **[BT1359]** ITU-R BT.1359, *Relative timing of sound and vision for
  broadcasting*.
