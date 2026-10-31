# 2. Architecture

## 2.1 Roles

A QDP session has exactly one host and one client. The client opens a QUIC
connection to the host. Once the session is active:

- the host captures each shared monitor, encodes it as AV1 video and sends it to
  the client;
- the host captures its audio output and sends it to the client;
- the client sends keyboard, mouse, gamepad, touch and pen input, which the host
  injects into its operating system;
- the client may send microphone audio, which the host exposes as an audio
  input device;
- both sides may exchange clipboard contents;
- the host tells the client which cursor image to draw, and the client draws it
  locally.

A host MAY serve several sessions at once, for example on a multi-user server,
but each session is independent. Sharing one desktop with several clients is
outside the scope of this version.

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
 │                              │── haptics ────▶│ touch, pen                   │
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
| Input | Client → host | Datagrams | Unreliable; redundant until acknowledged |
| Input acknowledgement | Host → client | Datagrams | Unreliable; cumulative |
| Device output (rumble, LEDs) | Host → client | Datagrams | Unreliable; state refreshed |
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
  │ ◀── DISPLAY_CONFIG, AUDIO_CONFIG, POINTER_MODE,         │
  │     CURSOR_SET, SESSION_START ──────────────────────── │
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
5. **Input is state.** Input datagrams carry the current input state and every
   unacknowledged event, so a lost datagram never loses a keystroke and never
   waits for a retransmission.
6. **Draw the cursor locally.** The client draws the pointer itself, so pointer
   movement has no network latency.

## 2.6 Conformance

An implementation conforms to this specification if it implements all features
marked *required* for its role and follows the rules of every optional feature it
advertises.

| Feature | Host | Client |
|---|---|---|
| QUIC transport, control stream, datagrams ([§3](03-transport.md)) | Required | Required |
| Session handshake and capabilities ([§4](04-session.md)) | Required | Required |
| Authentication: client certificate, token, SCRAM-SHA-256-PLUS | At least one | All three |
| Clock synchronization ([§5](05-timing.md)) | Required | Required |
| Single monitor, motion layer, AV1 profile 0, 8-bit ([§6](06-video.md)) | Required | Required |
| Multiple monitors | Optional | Optional |
| 10-bit video, HDR | Optional | Optional |
| Refinement layer | Optional | Optional |
| Audio output, Opus stereo ([§7](07-audio.md)) | Required | Required |
| Surround audio, PCM, microphone | Optional | Optional |
| Keyboard and mouse, absolute and relative ([§8](08-input.md)) | Required | Required |
| Gamepads, touch, pen, text input | Optional | Optional |
| Remote cursor ([§9](09-cursor.md)) | Required | Required |
| Clipboard ([§10](10-clipboard.md)) | Optional | Optional |
