# 2. Architecture

## 2.1 Roles

A QDP session has exactly one host and one client. The client opens a QUIC
connection to the host. Once the session is active:

- the host captures each shared monitor, encodes it as AV1 video and sends it to
  the client;
- the host captures its audio output and sends it to the client;
- the client attaches input devices, either standard devices (keyboard,
  pointer, mouse, touchscreen, pen, touchpad, gamepad and text) or passthrough
  HID devices, and sends their input reports, which the host injects into its
  operating system; the host sends output reports, such as keyboard LEDs and
  rumble, back to the devices;
- the client may send microphone audio, which the host exposes as an audio
  input device;
- both sides may exchange clipboard contents;
- the host tells the client which cursor image to draw, and the client draws it
  locally.

A host MAY serve several sessions at once, for example on a multi-user server,
but each session is independent: the input, devices and audio of one session
MUST reach only that session's desktop. Sharing one desktop with several clients
is outside the scope of this version.

Clients run on Windows, macOS, Linux, Android and iOS, including phones and
tablets that have no physical keyboard or mouse. Hosts run on Windows, macOS and
Linux; this version does not define hosts on Android or iOS
([Section 2.6](#26-conformance)).

## 2.2 Components (non-normative)

```
               HOST                                           CLIENT
 ┌──────────────────────────────┐                ┌──────────────────────────────┐
 │ Virtual or real monitors     │                │ Display(s)                   │
 │   └─ capture (no cursor)     │                │   ▲                          │
 │        └─ change tracking    │                │   │ present (VRR / vsync)    │
 │             └─ AV1 encoders  │── video ──────▶│ AV1 decoders ─▶ compositor   │
 │                motion layer  │  (datagrams)   │   motion + refinement layers │
 │                refinement    │◀─ feedback ────│                              │
 │ Audio capture ─▶ Opus        │── audio ──────▶│ Opus ─▶ jitter buffer ─▶ out │
 │ Audio input   ◀─ Opus        │◀─ microphone ──│ mic ─▶ echo cancel ─▶ Opus   │
 │ Input injection              │◀─ input ───────│ keyboard, mouse, gamepads,   │
 │                              │── output ─────▶│ touch, pen, HID devices      │
 │ Cursor shape tracking        │── cursor ─────▶│ local cursor rendering       │
 │ Clipboard                    │◀─ clipboard ──▶│ Clipboard                    │
 └──────────────────────────────┘                └──────────────────────────────┘
```

## 2.3 Channels

QDP multiplexes everything over one QUIC connection
([Section 3](03-transport.md)):

| Channel | Direction | Carried on | Reliability |
|---|---|---|---|
| Session control | Both | Control stream | Reliable, ordered |
| Display and stream configuration | Both | Control stream | Reliable, ordered |
| Video fragments | Host → client | Datagrams | Unreliable; QDP-level retransmission and recovery |
| Video feedback | Client → host | Datagrams | Unreliable; state repeated until acted on |
| Audio | Both | Datagrams | Unreliable; codec loss concealment |
| Input devices (attach, detach, keyboard layout) | Both | Control stream | Reliable, ordered |
| Input reports | Client → host | Datagrams | Unreliable; repeated until acknowledged |
| Input acknowledgement | Host → client | Datagrams | Unreliable; cumulative |
| Output reports of standard devices (LEDs, rumble) | Host → client | Datagrams | Unreliable; state refreshed |
| Output reports of passthrough devices | Host → client | Unidirectional streams | Reliable, ordered |
| Feature reports of passthrough devices | Both | Control stream | Reliable, ordered |
| Clock synchronization | Both | Datagrams | Unreliable |
| Cursor shape changes and warps | Host → client | Control stream | Reliable, ordered |
| Cursor images | Host → client | Unidirectional streams | Reliable |
| Clipboard announcements | Both | Control stream | Reliable, ordered |
| Clipboard data | Both | Unidirectional streams | Reliable |

Time-critical data travels in datagrams so that a lost packet never delays
newer data behind a retransmission. Data that must arrive intact and in order
travels on QUIC streams.

## 2.4 Session lifecycle

```
Client                                                   Host
  │ ── QUIC handshake, ALPN "qdp-draft-00" ─────────────▶ │
  │ ── CLIENT_HELLO ────────────────────────────────────▶ │
  │ ◀──────────────────────────────────────── HOST_HELLO ── │
  │ ── AUTH_START ──────────────────────────────────────▶ │
  │ ◀──────────────────── AUTH_CONTINUE (method-specific) ── │
  │ ◀─────────────────────────────────────── AUTH_RESULT ── │
  │ ── DISPLAY_REQUEST, AUDIO_REQUEST, KEYBOARD_INFO,       │
  │    DEVICE_ATTACH ..., CLIENT_READY ─────────────────▶ │
  │ ◀── DEVICE_STATUS ..., DISPLAY_CONFIG, AUDIO_CONFIG,    │
  │     POINTER_MODE, CURSOR_SET, SESSION_START ────────── │
  │ ◀═══════════ video, audio, cursor, clipboard ════════▶ │
  │ ═══════════ input, feedback, microphone ════════════▶ │
  │ ── CONNECTION_CLOSE (application error code) ───────▶ │
```

The client may pipeline messages so that, after the QUIC handshake, a session
becomes active in one round trip when the authentication method needs no
challenge ([Section 4.4.6](04-session.md#446-pipelining)).

## 2.5 Latency principles

These principles shape the rest of the specification. The design rationale
explains where each comes from.

1. **The newest frame wins.** Neither endpoint queues video to smooth playback.
   A client presents the most recent decoded frame as soon as the display can
   show it.
2. **Never wait for quality.** The host sends what fits in the current frame's
   byte budget and improves static content afterwards with the refinement layer.
3. **Send nothing when nothing changes.** Frames are produced only when a
   monitor's content changes. A static desktop costs no video bandwidth.
4. **Recover forward.** A lost fragment is retransmitted once if that is fastest.
   Otherwise the host sends a recovery frame; it never resends stale content.
5. **Input is state.** Input reports describe the state of each input device,
   as USB HID reports do, and the client keeps sending each report until the
   host acknowledges it. The host repairs a lost datagram from the datagrams
   that follow it instead of waiting for a retransmission.
6. **Draw the cursor locally.** The client draws the pointer itself, so pointer
   movement has no network latency.

## 2.6 Conformance

An implementation conforms to this specification if it implements all features
marked *required* for its role and follows the rules of every optional feature it
advertises.

This specification places no requirement on a client that cannot be met on
Windows, macOS, Linux, Android and iOS, and no requirement on a host that cannot
be met on Windows, macOS and Linux. No requirement on a client depends on a
physical keyboard or mouse, or on any other facility that only desktop computers
have. A client without them, such as a phone or tablet, still attaches the
required `KEYBOARD`, `POINTER` and `MOUSE` devices, and a `TEXT` device where
that class is negotiated ([Section 8.2.1](08-input.md#821-device_attach-0x31)),
and produces their reports from touch gestures and an on-screen keyboard.

| Feature | Host | Client |
|---|---|---|
| QUIC transport, control stream, datagrams ([§3](03-transport.md)) | Required | Required |
| Session handshake and capabilities ([§4](04-session.md)) | Required | Required |
| Authentication: client certificate, token, SCRAM-SHA-256-PLUS, password ([§4.4](04-session.md#44-authentication)) | At least one | All four |
| Clock synchronization ([§5](05-timing.md)) | Required | Required |
| Single monitor, motion layer, AV1 profile 0, 8-bit ([§6](06-video.md)) | Required | Required |
| Multiple monitors | Optional | Optional |
| 10-bit video, HDR | Optional | Optional |
| Refinement layer | Optional | Optional |
| Audio output, Opus stereo ([§7](07-audio.md)) | Required | Required |
| Surround audio, PCM, microphone | Optional | Optional |
| Keyboard, pointer and mouse devices ([§8](08-input.md)) | Required | Required |
| Touchscreen, pen, touchpad, gamepad and text devices | Optional | Optional |
| HID passthrough | Optional | Optional |
| Remote cursor ([§9](09-cursor.md)) | Required | Required |
| Clipboard ([§10](10-clipboard.md)) | Optional | Optional |
| Resumption tokens ([§4.4.7](04-session.md#447-resumption_token-0x08)) | Optional | Optional |
