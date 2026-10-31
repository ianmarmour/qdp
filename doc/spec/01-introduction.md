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
- keyboard, mouse, gamepad, touch and pen input;
- the remote cursor and the clipboard.

It does not define file transfer, device redirection (USB, printers, smart
cards), sessions with more than one client, or how clients discover hosts. These
are listed as future work in the
[design rationale](../design-rationale.md#9-future-work).

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
  the connection closes.
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
- **Session clock**: an endpoint's monotonic microsecond clock
  ([Section 5.1](05-timing.md#51-session-clocks)).

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

A **Position** is a variable-length integer counting 1/256ths of a pixel from
the top-left corner of a monitor. X increases to the right and Y increases
downward. A position on a monitor of width W is in the range 0 to 256 × W − 1.

### 1.5.7 Identifiers

Identifiers (Stream ID, Monitor ID, Device ID, Cursor ID and so on) are
variable-length integers chosen by the endpoint that defines the object. The
value 0 is reserved to mean "none" and never identifies an object. An endpoint
MUST NOT reuse an identifier for a different object within a session unless the
section that defines the identifier allows it.

## 1.6 Extensibility

QDP follows the rule of [RFC9114], Section 9: implementations MUST ignore
unknown or unsupported values in all extensible protocol elements. Concretely:

1. A receiver MUST ignore control messages, datagrams, records, capabilities and
   input events of unknown type, skipping them by their length.
2. A receiver MUST abort reading a unidirectional QUIC stream of unknown type
   ([Section 3.5.2](03-transport.md#352-unidirectional-streams)) and MUST NOT
   treat it as a connection error.
3. When a control message, record, capability or event is longer than the
   fields this document defines for it, the receiver MUST ignore the extra
   bytes. Later minor versions only ever add fields at the end.
4. A receiver MUST treat a structure that is shorter than its defined fields as
   a connection error of type `QDP_PROTOCOL_VIOLATION`.
5. Flag bits not defined by this document are reserved. Senders MUST set them
   to zero and receivers MUST ignore them.
6. An endpoint MUST NOT use an extension until the peer has advertised support
   for it with a capability ([Section 4.6](04-session.md#46-capabilities)).

Values of the form `0x1f × N + 0x21` for integer N ≥ 0 (0x21, 0x40, 0x5f,
0x7e, ...) are reserved in every QDP registry ([Section 12](12-registries.md)).
They never carry meaning. Endpoints SHOULD occasionally send a message, record,
capability or datagram with a reserved type and arbitrary content, so that
peers that mishandle unknown values are found early. This practice, called
greasing, is borrowed from HTTP/3.

## 1.7 References

### 1.7.1 Normative references

- **[AV1]** Alliance for Open Media, *AV1 Bitstream & Decoding Process
  Specification*, version 1.0.0 with Errata 1.
- **[H273]** ITU-T H.273, *Coding-independent code points for video signal type
  identification*.
- **[HID]** USB Implementers Forum, *HID Usage Tables for Universal Serial Bus*.
- **[RFC2119]** Key words for use in RFCs to Indicate Requirement Levels.
- **[RFC3190]** RTP Payload Format for 12-bit DAT Audio and 20- and 24-bit
  Linear Sampled Audio (L24).
- **[RFC3551]** RTP Profile for Audio and Video Conferences (L16).
- **[RFC5646]** Tags for Identifying Languages (BCP 47).
- **[RFC5802]** Salted Challenge Response Authentication Mechanism (SCRAM).
- **[RFC5905]** Network Time Protocol Version 4.
- **[RFC6716]** Definition of the Opus Audio Codec.
- **[RFC6838]** Media Type Specifications and Registration Procedures.
- **[RFC7677]** SCRAM-SHA-256 and SCRAM-SHA-256-PLUS.
- **[RFC7845]** Ogg Encapsulation for the Opus Audio Codec (channel mappings
  and multistream packing).
- **[RFC8174]** Ambiguity of Uppercase vs Lowercase in RFC 2119 Key Words.
- **[RFC8446]** The Transport Layer Security (TLS) Protocol Version 1.3.
- **[RFC9000]** QUIC: A UDP-Based Multiplexed and Secure Transport.
- **[RFC9001]** Using TLS to Secure QUIC.
- **[RFC9002]** QUIC Loss Detection and Congestion Control.
- **[RFC9221]** An Unreliable Datagram Extension to QUIC.
- **[RFC9266]** Channel Bindings for TLS 1.3 (`tls-exporter`).
- **[GAMEPAD]** W3C, *Gamepad*, Section "Standard Gamepad".
- **[POINTER]** W3C, *Pointer Events* (pen pressure, tilt and twist).

### 1.7.2 Informative references

- **[RFC6143]** The Remote Framebuffer Protocol (VNC).
- **[MS-RDPBCGR]**, **[MS-RDPEGFX]**, **[MS-RDPEDISP]**, **[MS-RDPECLIP]**
  Microsoft Remote Desktop Protocol specifications: core, graphics pipeline,
  display control and clipboard.
- **[RFC9114]** HTTP/3 (stream types, extensibility and greasing patterns).
- **[RFC9330]** Low Latency, Low Loss, and Scalable Throughput (L4S).
- **[AV1-RTP]** Alliance for Open Media, *RTP Payload Format for AV1*.
- **[BT1359]** ITU-R BT.1359, *Relative timing of sound and vision for
  broadcasting*.
