# QDP design rationale

This document explains why the Quick Desktop Protocol is designed the way it is.
It is non-normative: the [specification](spec/01-introduction.md) defines the
protocol, and this document records the goals, the evidence and the
alternatives behind it.

## 1. Summary

QDP is a single protocol for remote desktops and game streaming, built for
networks with round-trip times under 50 ms and hundreds of megabits per second
of capacity. It runs over QUIC, sends all screen content as AV1 video in two
layers (a fast motion layer and an optional lossless refinement layer), and
sends input as redundant state so that a lost packet never delays a keystroke.

## 2. Goals

### 2.1 Target conditions

Most remote desktop protocols were designed for slow or lossy wide-area
networks, and their defaults spend CPU time to save bandwidth. Today a large
share of users reach cloud machines in the same region: round-trip times under
50 ms, often under 10 ms, and links of 300 Mbit/s or more. Desktop clients
typically have at least four cores at 3 GHz or faster, and current phones and
tablets are as fast in short bursts, though heat and battery limit them over long
sessions. QDP is optimized for these conditions and still works, less well,
outside them.

### 2.2 Goals

1. **Latency**: add as little as possible to the network round trip. Every
   millisecond spent capturing, encoding, queuing, decoding or presenting is
   treated as a defect to remove.
2. **Smoothness**: consistent frame times. Variation in latency is noticed more
   than steady latency.
3. **Fidelity**: text and user interfaces are shown sharply, and bit-exact once
   they stop changing.
4. **Universality**: one protocol for office work, creative work and games,
   with any content and no modes. Clients run on Windows, macOS, Linux, Android
   and iOS; hosts run on Windows, macOS and Linux.
5. **Re-implementability**: the specification is complete enough to write an
   interoperable implementation in another language without reading ours.

### 2.3 Non-goals for version 1

- Optimizing for links below about 20 Mbit/s or round-trip times above 100 ms.
- Remoting drawing commands or window structure instead of pixels. That needs
  cooperation from the host's compositor, which closed operating systems do not
  offer ([Section 6](#6-alternatives-considered)).
- File transfer, USB, printer and smart card redirection, multi-party sessions,
  and host discovery ([Section 9](#9-future-work)).

## 3. Where latency comes from

On a 10 ms round trip the network contributes about 5 ms each way. The rest of
the delay between an input and the photons that show its result is spent at the
endpoints, and that is where existing protocols lose most of their time:

| Stage | Where time hides | What QDP does |
|---|---|---|
| Input | Event loops, batching | Send immediately, at up to 1 kHz ([§8](spec/08-input.md)) |
| Capture | Waiting for the compositor | Capture from virtual displays at the client's refresh rate ([§6](spec/06-video.md)) |
| Encoding | Whole-frame encoding, lookahead, B-frames | No reordering, one frame per temporal unit, fragments sent as encoded ([§6.4](spec/06-video.md#64-av1-bitstream-requirements)) |
| Transmission | Frame size ÷ link rate; queues | Per-frame byte budgets, delay-sensitive congestion control ([§5.5](spec/05-timing.md#55-rate-control)) |
| Jitter buffer | One to three frames of safety | None for video ([§5.4](spec/05-timing.md#54-presentation)) |
| Decoding | Frame-parallel decoders hold frames | Required tiles allow parallel decoding without holding frames ([§6.4.4](spec/06-video.md#644-tiles)) |
| Presentation | Compositor queues, vsync | Variable refresh, latest frame wins, phase alignment ([§5.3](spec/05-timing.md#53-display-timing)) |

Transmission time is easy to overlook: a 1 MB frame takes 27 ms to cross a
300 Mbit/s link. A fast link is headroom to keep frames small, not capacity to
fill.

## 4. Principles

The latency principles in [Section 2.5](spec/02-architecture.md#25-latency-principles)
of the specification follow from the analysis above: the newest frame wins;
never wait for quality; send nothing when nothing changes; recover forward;
treat input as state; draw the cursor locally.

## 5. Key decisions

### 5.1 One protocol for desktops and games

Game streaming systems (Parsec, Moonlight, GeForce NOW) send full-frame video,
take relative mouse input and favour latency. Remote desktop systems (RDP, DCV)
send text through lossless codecs, take absolute pointer input and favour
fidelity. Most of what they do is the same: capture, encode, transport, decode,
present, and inject input. QDP makes the differences fall out of the content,
not out of a mode switch:

- Content that changes every frame never stops changing, so it is never refined
  and stays on the low-latency motion layer. Text stops changing within a frame
  or two and is refined to lossless.
- The pointer mode follows the host: when an application captures and hides the
  pointer, the client switches to relative input ([§8.9](spec/08-input.md#89-pointer_mode-0x33)).
  This is the one place QDP relies on the host operating system's state, because
  it concerns what input means, not what pixels show.

### 5.2 QUIC as the transport

QUIC provides, in one standard package: TLS 1.3 encryption and authentication,
reliable streams without head-of-line blocking between them, unreliable
datagrams for time-critical media, connection migration between networks, and
a path to browser clients through WebTransport. Alternatives:

- **TCP** blocks all data behind any lost packet, which is fatal for
  interactive video.
- **A custom protocol over UDP** (as WireGuard and Parsec use) gives full
  control, but QDP would have to design and security-review its own handshake,
  encryption, reliability and migration. QUIC's congestion controller is
  pluggable in the major implementations, so we keep the control that matters.
- **WebRTC** brings session negotiation (SDP, ICE) and RTP machinery designed
  for conferencing between browsers, which a client-server desktop protocol does
  not need.

### 5.3 Video for everything, with AV1

We evaluated a custom tile codec (run-length encoding, palettes, move detection)
against standard video codecs on real screen content
([Section 8](#8-measurements)). Video won: motion compensation handled scrolling
better than our own move detection, and small changes such as typing cost under
a kilobyte per frame. Encoding everything as video also gives hardware encoding
and decoding on almost every device.

AV1 was chosen over H.264 and HEVC because it compresses better, especially on
scrolling (4.6 times fewer bits than H.264 in our test), it has screen content
tools, it is royalty-free, and hardware AV1 decoding is now widespread. Its
successor, AV2, can be added as a new codec code point once hardware supports it.

### 5.4 Two layers: motion and refinement

Video alone has two weaknesses that our measurements exposed:

1. **4:4:4 decoding**: chroma subsampling (4:2:0) blurs coloured text, but
   hardware decoders on most clients only handle 4:2:0. Software decoding of 4K
   4:4:4 managed 49 frames per second on game-like content in our test, short of
   60.
2. **Lossless output**: a standard codec cannot spread a lossless update over
   several frames. A lossless 4K window switch cost 575 KB to 1.2 MB in one
   frame, 15 to 32 ms on a 300 Mbit/s link.

The motion layer therefore uses whatever the client decodes in hardware,
usually 4:2:0, and carries every change. The refinement layer, in 4:4:4 or
lossless RGB, carries only blocks that have stopped changing, so its software
decoding cost is bounded by how often content goes still, not by the frame rate.
The client combines them block by block with a rule that needs no timing
assumptions ([§6.9](spec/06-video.md#69-composition)). Amazon DCV uses a similar
split, 4:2:0 video plus lossless updates of static regions, which suggests the
constraints are real.

### 5.5 Required tiles

AV1 decoders parallelize either across frames, which holds frames back and adds
latency, or across tiles within a frame. With frame parallelism disabled, a 4K
stream encoded as one tile decoded at 24 frames per second in our test; with
4 × 2 tiles it decoded at 73. The specification therefore requires a minimum
number of tiles for large frames ([§6.4.4](spec/06-video.md#644-tiles)), so that
software decoding works as a fallback.

### 5.6 Unreliable video with forward recovery

Video travels in QUIC datagrams, not streams, so that a lost packet delays only
its own frame. QDP adds the reliability it needs on top:

- **Retransmission** of individual fragments when the client asks, which at the
  target round-trip times usually completes within a frame interval.
- **Forward recovery** when retransmission would be too slow: the client reports
  the last frame it decoded, and the host sends a frame decodable from exactly
  that state ([§6.8](spec/06-video.md#68-loss-recovery)). Because the client
  decodes strictly in order and stops at the first gap, its decoder state is
  always known to the host, and recovery needs no key frame when the encoder
  keeps a reference the client has acknowledged. This relies on the decoder
  holding exactly that state; a client that feeds partial frames to its decoder
  must be able to undo them, or fall back to asking for a key frame.

Forward error correction is not part of version 1. At the round-trip times QDP
targets, retransmission is cheap, and every endpoint would otherwise have to
implement the same code. It is listed as future work.

### 5.7 Change tracking by the host, not the operating system

Operating systems report changed screen areas differently: Windows reports both
changed and moved areas, macOS and Linux report changed areas with different
accuracy, and some capture paths report nothing. QDP's composition rule relies
on knowing exactly which blocks changed, so the host compares captured images
itself ([§6.11](spec/06-video.md#611-encoding-guidance-non-normative)). Hashing a
4K frame in 64 × 64 blocks takes a fraction of a millisecond on a GPU. The
result is identical behaviour on every operating system.

### 5.8 The client draws the cursor

Drawing the cursor on the client removes all network latency from pointer
movement, which is the latency users notice first. Every major remote desktop
protocol does this; VNC calls it the Cursor pseudo-encoding.

### 5.9 Input modelled on HID

All input is described in terms of USB HID ([§8](spec/08-input.md)), in two forms:

- **Standard devices** (keyboard, absolute pointer, relative mouse, touchscreen,
  pen, touchpad, gamepad and text) have report layouts defined by QDP. The client
  builds them from its operating system's input events, because operating systems
  do not let applications read raw keyboard and mouse HID, many built-in keyboards
  and trackpads are not HID devices at all, phones have only touchscreens and
  on-screen keyboards, and a remote desktop needs absolute pointer positions that
  mouse hardware never reports.
- **Passthrough devices** forward a physical controller's HID descriptor and
  reports unchanged, so a game on the host sees the real wheel, flight stick or
  controller with all its features, with no per-device protocol work. HID is the
  same over USB and Bluetooth.

Each report states the device's complete state, and relative motion and scrolling
are running totals, so a later report repairs any lost one. Reports share one
sequence number, so the host applies Ctrl on the keyboard and a click on the mouse
in the order the user made them. Reports that change discrete state (a key, a
button, a touch, text) are repeated until acknowledged and are never dropped
because a newer report exists. A lost datagram therefore never loses a keystroke
and never waits a full round trip. Even a 1 kHz mouse and gamepad at a 50 ms round
trip need only a few megabits per second upstream, by our estimate.

Keys are identified by USB HID usage: a platform-neutral, standardized name for
each physical key that every operating system can map to its own codes. VNC uses
X11 keysyms, which describe characters rather than keys and need workarounds for
every layout. Gamepads use the W3C Standard Gamepad layout and pens use W3C Pointer
Events conventions, for the same reason.

### 5.10 Opus for audio

Opus has algorithmic delay as low as 5 ms with its 2.5 ms frames, in-band forward
error correction with 10 and 20 ms frames, loss concealment, multichannel support
and no royalties. Low delay and forward error correction do not combine, so
low-delay streams rely on loss concealment or on sending packets twice. Uncompressed PCM is
also offered because bandwidth is plentiful on the target networks. LC3 was
considered and rejected: its minimum delay is higher and its licensing outside
Bluetooth products is less clear.

### 5.11 Lazy clipboard

Endpoints announce clipboard formats and transfer data only on paste, which is
how operating system clipboards already work locally, so copying large content
costs nothing unless it is pasted remotely. Formats are media types rather than
platform format numbers, and content the platform marks as a password is never
announced.

### 5.12 Extensibility

The rules come from HTTP/3: ignore unknown message types, records and
capabilities; ignore extra trailing bytes; negotiate extensions before using
them; and grease reserved code points so that implementations that mishandle
unknown values are found early. The ALPN token carries the major version, and
everything else is negotiated by capability. VNC's fixed message layouts show
the alternative: two decades of incompatible extensions.

### 5.13 Security

QDP is secure by construction, unlike VNC, where encryption had to be added
later. TLS 1.3 is mandatory; passwords use SCRAM with TLS channel binding so
they cannot be relayed; 0-RTT is forbidden because replayed input is dangerous;
and nothing but the handshake is processed before authentication
([§11](spec/11-security.md)).

## 6. Alternatives considered

| Alternative | Why it was rejected |
|---|---|
| Streaming windows, layers or drawing commands instead of pixels | Needs the host compositor's cooperation; impossible to do generally on Windows and macOS. |
| A custom tile codec (RLE, palettes, QOI-style) | Two to three times larger than PNG-class codecs on screen content, and far worse than video on scrolling. |
| Diffing frames and sending the difference | Scrolling turns a diff into noise: 625 KB per frame in our test, worse than resending. |
| Classifying screen regions as text or video | Classification lags when content changes; per-block, per-frame decisions avoid it. |
| Using the operating system's damage reports | Inconsistent between platforms ([§5.7](#57-change-tracking-by-the-host-not-the-operating-system)). |
| H.264 or HEVC | Less efficient, and royalty-encumbered. |
| A single lossless or 4:4:4 video stream | Lossless updates cannot be spread over frames, and 4:4:4 lacks hardware decoding on most clients ([§5.4](#54-two-layers-motion-and-refinement)). |
| Video over reliable QUIC streams | One lost packet would delay every later frame on that stream. |
| Forward error correction in version 1 | Retransmission is cheap at the target round-trip times; deferred. |
| Raw USB redirection for keyboards, mice and controllers | Cannot express absolute pointer positions; takes the device away from the client; does not exist for built-in, Bluetooth or touch devices; needs kernel drivers on the host; exposes the host's USB stack. HID passthrough gives controllers their native features without these costs. |

## 7. Lessons from existing protocols

| Protocol | What QDP borrows | What QDP avoids |
|---|---|---|
| VNC / RFB [RFC6143] | Client-drawn cursor; simple, documented message layouts | Character-based key codes; weak built-in security; uncoordinated extensions |
| RDP | Nothing; QDP is designed independently of it | Layers of legacy codecs and virtual channels; TCP as the primary transport; a 30 fps default |
| SPICE | Separate channels for display, input, cursor and audio | One TCP connection per channel |
| Amazon DCV | 4:2:0 video plus lossless refinement of static areas; QUIC transport | Proprietary, undocumented wire format |
| Parsec, Moonlight | Low-latency pipeline: no buffering, hardware codecs, relative input | Gaming-first scope; 4:2:0 text by default |
| QUIC, HTTP/3 [RFC9000] [RFC9114] | Wire notation, variable-length integers, stream types, extensibility and greasing | — |
| AV1 RTP payload [AV1-RTP] | Dropping temporal delimiters; fragmenting temporal units | RTP framing, which QUIC makes unnecessary |

## 8. Measurements

These measurements guided the design. They were taken with software encoders
standing in for hardware ones, on a 4-core 2.1 GHz cloud CPU that is slower than
the target client, using screenshots of a code editor and a text-heavy web page
rendered by a browser, a scrolling sequence, and a pan-and-zoom across an
upscaled photograph standing in for game content. PSNR is measured in RGB.
x264's perceptual tuning lowers its PSNR somewhat, and the upscaled photograph is
easier to compress than real game footage. The tools used (ffmpeg, x264, SVT-AV1,
libaom, dav1d) were only measuring instruments; the implementation uses none of
them ([Section 10](#10-implementation)).

**Encoding at 4K (60 frames per second):**

| Content | H.264 4:4:4 (x264) | AV1 4:2:0 (SVT-AV1) | AV1 4:4:4 (libaom) |
|---|---|---|---|
| Scrolling code | 22.7 Mbit/s, 44.5 dB | 4.9 Mbit/s, 43.1 dB | 30.3 Mbit/s, 46.3 dB |
| Photo pan and zoom | 40.9 Mbit/s, 36.5 dB | 42.8 Mbit/s, 39.8 dB | 37.7 Mbit/s, 40.6 dB |
| One frame of dense text | 416 KB, 46.9 dB | 484 KB, 43.6 dB | 538 KB, 53.7 dB |

**Low-latency software decoding** (dav1d, 4 threads, no frame parallelism;
worst-case pan and zoom), in frames per second:

| Resolution | AV1 4:2:0, 1 tile | AV1 4:2:0, 4 × 2 tiles | AV1 4:4:4, 4 × 2 tiles |
|---|---|---|---|
| 1920 × 1080 | 60 | 156 | 134 |
| 2560 × 1440 | 36 | 108 | 91 |
| 3840 × 2160 | 24 | 73 | 49 |

**Frame-to-frame costs at 1080p** (H.264 4:4:4 unless stated):

| Update | Cost |
|---|---|
| Static screen | 0.5 to 2 KB per frame |
| Typing one character | 0.3 to 1.2 KB |
| Scrolling 24 px per frame | 3.1 KB per frame (370 KB resending changed tiles with RLE; 625 KB diffing frames; 5.8 KB with ideal move detection and RLE) |

**Window switch at 4K under a 150 KB per-frame budget**: dense text starts at
31 dB and reaches 45 dB after nine frames (150 ms) with lossy video alone.
Lossless H.264 ignores the budget and sends 575 KB to 1.2 MB in one frame. This
result motivated the refinement layer.

## 9. Future work

**Planned features:**

- Forward error correction, negotiated by capability, for lossy last-mile
  links such as Wi-Fi.
- A browser client over WebTransport and WebCodecs.
- File transfer, and USB, printer and smart card redirection.
- Sessions shared by several clients.
- Local prediction of scrolling and window dragging on the client.
- AV2 support once hardware decoders ship.
- Relays, NAT traversal and connection brokers.
- An AV1 encoder path that uses the host's changed-block map to skip unchanged
  areas, so that software encoding costs scale with what changed rather than with
  screen size ([Section 10](#10-implementation)).

**Open questions to resolve before version 1 is final:**

- Lossless AV1 encoding: the AV1 specification defines lossless coding, but an
  earlier test pipeline did not produce bit-exact output. Our own encoder path
  must be verified bit-exact with test vectors before the `LOSSLESS` refinement
  layer is relied on.
- Reference HID report descriptors for the standard devices, validated with HID
  tooling, for hosts that create virtual devices from the report layouts.
- Which hardware AV1 encoders support the reference control needed for
  key-frame-free recovery, and how hosts should fall back.
- Default values for loss detection and the redundancy window, to be tuned with
  real implementations on real networks.
- HDR metadata: whether to carry mastering display information beyond what the
  AV1 sequence header and metadata OBUs provide.

## 10. Implementation

The specification is language-neutral. Our implementation follows these
decisions:

- **Pure Rust**, with no bundled third-party C or C++ libraries such as ffmpeg.
  Rust crates that embed hand-written assembly for performance-critical code, such
  as cryptography or codec kernels, are acceptable.
- **Transport**: `quinn` for QUIC and `rustls` for TLS 1.3.
- **Software AV1 decoding**: rav1d.
- **Software AV1 encoding**: `rav1e`, behind an encoder interface so that it can be
  replaced later. It serves hosts without an AV1 hardware encoder and the refinement
  layer, which hardware encoders cannot produce.
- **Operating system interfaces** for everything hardware-specific:

| Platform | Role | Video | Capture | Audio | Input |
|---|---|---|---|---|---|
| Windows | Host and client | Media Foundation, Direct3D 12 video | Windows.Graphics.Capture, virtual display driver | WASAPI | SendInput, virtual HID driver |
| macOS | Host and client | VideoToolbox | ScreenCaptureKit | Core Audio | CGEvent, virtual HID |
| Linux | Host and client | VA-API or Vulkan Video | PipeWire, KMS | PipeWire | uinput |
| Android | Client | MediaCodec | — | AAudio | Android input framework |
| iOS | Client | VideoToolbox | — | Core Audio | UIKit, GameController framework |

- **Mobile clients** use the same Rust core through generated Swift and Kotlin
  bindings, so no protocol logic exists outside Rust.
- **AV1 only.** Hosts without an AV1 hardware encoder, including current Macs,
  encode in software. Real-time software AV1 encoding is expensive at high
  resolutions and frame rates, so such hosts start at reduced resolution or frame
  rate. Encoding guided by the changed-block map ([Section 9](#9-future-work)) is
  the main planned improvement, because for desktop work it makes encoding cost
  proportional to what changed.
