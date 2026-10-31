# 5. Timing

## 5.1 Session clocks

Each endpoint keeps a **session clock**: a monotonic clock counting microseconds
from an epoch the endpoint chooses when the session starts. The clock MUST NOT
jump or run backwards during the session, and its epoch MUST NOT change.
Wall-clock time is never sent.

Every Timestamp field ([Section 1.5.5](01-introduction.md#155-timestamps-and-durations))
is on the sender's session clock. To relate the two clocks, each endpoint
estimates the offset between them ([Section 5.2](#52-clock-synchronization)).

## 5.2 Clock synchronization

### 5.2.1 Datagrams

```
TIME_PING Datagram {
  Datagram Type (i) = 0x07,
  Ping ID (i),
  Origin Timestamp (i),
}

TIME_PONG Datagram {
  Datagram Type (i) = 0x08,
  Ping ID (i),
  Origin Timestamp (i),
  Receive Timestamp (i),
  Transmit Timestamp (i),
}
```

An endpoint that receives `TIME_PING` MUST answer with `TIME_PONG` without
deliberate delay, echoing `Ping ID` and `Origin Timestamp`, and setting
`Receive Timestamp` to when the ping arrived and `Transmit Timestamp` to when the
pong is sent, both on its own clock.

The client MUST send `TIME_PING` at least once per second during the first five
seconds of the `ACTIVE` state and at least once every ten seconds after that. The
host MAY send `TIME_PING` as well.

### 5.2.2 Offset and round-trip time

With T1 = Origin Timestamp, T2 = Receive Timestamp, T3 = Transmit Timestamp and
T4 = the time the pong arrived on the pinging endpoint's clock, the pinging
endpoint computes, as in NTP [RFC5905]:

```
offset = ((T2 − T1) + (T3 − T4)) / 2      peer clock ≈ local clock + offset
delay  = (T4 − T1) − (T3 − T2)            round-trip time excluding the peer's processing
```

Network delay is rarely symmetric, so a single sample can be wrong by up to half
its delay. Endpoints SHOULD keep the last eight samples and use the offset from
the sample with the smallest delay. The two clocks also drift apart slowly. An
endpoint that uses the offset to schedule anything, such as phase alignment,
SHOULD estimate the drift, for example by linear regression over a window of a
minute or more, or SHOULD use only samples less than 20 seconds old.

## 5.3 Display timing

To let the host time its frames to the client's display, the client reports each
monitor's refresh timing in `DISPLAY_TIMING` records of the `VIDEO_FEEDBACK`
datagram ([Section 6.7](06-video.md#67-video-feedback)):

```
DISPLAY_TIMING Record Value {
  Monitor ID (i),
  Refresh Period (i),
  Vsync Timestamp (i),
  Flags (8),
}
```

- **Monitor ID**: the host monitor whose content the client shows on this
  display.
- **Refresh Period**: the display's refresh period in **nanoseconds**, or 0 if
  unknown. Nanoseconds are used because microsecond rounding accumulates phase
  error quickly.
- **Vsync Timestamp**: the client-clock time of a recent start of vertical
  blanking on that display.
- **Flags**: 0x01 `VRR_ACTIVE` means the display is in variable refresh mode and
  will show a frame as soon as it is presented, within its refresh range.

The client SHOULD send `DISPLAY_TIMING` when the session becomes active, when
the timing changes, and at least once per second.

### 5.3.1 Phase alignment (non-normative)

Without coordination, a frame finishes decoding at a random point in the
client's refresh cycle and waits on average half a refresh period before it is
shown. A host can remove that wait by adjusting when it captures frames, so that
each frame finishes decoding shortly before the client's next vertical blank. A
simple controller:

1. From `FRAME_TIMING` records, compute for each frame the client-clock time it
   was decoded, and its phase within the client's refresh cycle using
   `DISPLAY_TIMING`.
2. Choose a target phase that leaves a safety margin before vertical blank, for
   example 1.5 ms plus the observed decode-time jitter.
3. Nudge the capture schedule by a fraction of the phase error each frame, as a
   phase-locked loop does, so that it converges without oscillating.

When `VRR_ACTIVE` is set, no alignment is needed: the client presents each frame
as soon as it is decoded, and the host simply sends frames as the source
produces them.

## 5.4 Presentation

The client SHOULD present each newly composited image at the earliest
opportunity: immediately when the display is in variable refresh mode, otherwise
at the next vertical blank. When several images become ready before one vertical
blank, the client SHOULD present only the newest. The client SHOULD NOT queue
images ahead of the display to smooth out network jitter unless the user has
asked it to trade latency for smoothness.

## 5.5 Rate control

### 5.5.1 Requirements

The QUIC congestion controller governs datagrams as well as stream data
[RFC9221], so a host can never send faster than the path allows. Within that
limit:

- The host MUST adapt its encoding rate so that video does not persistently
  exceed what the congestion controller allows. Fragments dropped because the
  congestion window is full are lost frames.
- When it must reduce its rate, the host MUST reduce refinement-layer traffic
  before motion-layer traffic.
- The host SHOULD use a congestion controller that limits queuing delay, rather
  than one that fills queues until packets are lost, and SHOULD support ECN and
  L4S [RFC9330] where the QUIC implementation allows.
- The host SHOULD also lower its rate while a large share of its packets is lost.
  A congestion controller that does not treat loss as congestion, as BBR does not,
  can leave the window wide while a wireless link loses much of what is sent, and
  each loss costs a recovery frame larger than the frames it replaces. Under
  heavy loss, the host can lower the frame rate as well.

### 5.5.2 Frame budgets (non-normative)

The time a frame takes to cross the network is at least its size divided by the
available rate: a 1 MB frame takes 27 ms at 300 Mbit/s, while a 100 KB frame
takes 2.7 ms. Large frames therefore add latency even on fast links. A host can
bound this by giving every motion-layer frame a **byte budget**:

```
budget = estimated available rate × target transmission time
```

A target transmission time of 3 to 4 ms is a good starting point. At 300 Mbit/s,
4 ms gives a budget of 150 KB per frame. Content that does not fit at full
quality is sent at lower quality and improved afterwards by the refinement
layer.

### 5.5.3 Queue detection (non-normative)

`FRAME_TIMING` records give the client-clock time at which each frame's last
fragment arrived. The raw difference between that time and the host-clock time at
which the host sent the fragment contains the unknown clock offset, which is
constant, plus the one-way transit time. Comparing each difference with the
smallest difference seen over a sliding window of a few seconds cancels the offset
and leaves the queuing delay. A rise of a few milliseconds above that minimum means
a queue is building somewhere on the path, and the host should lower its rate
before packets are lost. Clock drift moves the minimum slowly, which the sliding
window absorbs.

## 5.6 Timers and limits

This table collects the timers and limits defined throughout the specification.
Values marked *recommended* are defaults that implementations may tune; the others
are requirements of the section named.

| Timer or limit | Value | Section |
|---|---|---|
| `CLIENT_HELLO` after the QUIC handshake | 10 s | [4.2](04-session.md#42-session-states) |
| Authentication complete | 120 s | [4.2](04-session.md#42-session-states) |
| `CLIENT_READY` after authentication; `SESSION_START` after `CLIENT_READY` | 30 s | [4.2](04-session.md#42-session-states) |
| QUIC idle timeout; keep-alive PING | 30 s; after 10 s idle (recommended) | [3.3](03-transport.md#33-transport-parameters) |
| Clock synchronization pings | 1/s for the first 5 s, then at least every 10 s | [5.2.1](#521-datagrams) |
| `DISPLAY_TIMING` | at least 1/s | [5.3](#53-display-timing) |
| Frame transmission time target | 3–4 ms (recommended) | [5.5.2](#552-frame-budgets-non-normative) |
| Buffering of data that overtakes its definition | at least 2 MiB for 1 s; discarded after 2 s | [3.7.3](03-transport.md#373-processing-rules) |
| Incomplete frames kept per stream | 16 (recommended) | [6.6.3](06-video.md#663-reassembly) |
| `NACK` when idle | after max(1 ms, smoothed RTT / 8) without progress | [6.8.1](06-video.md#681-retransmission) |
| Declaring a frame lost | smoothed RTT + 4 ms after a `NACK` with no progress (recommended) | [6.8.2](06-video.md#682-declaring-loss) |
| Host keeps sent fragments | at least 2 × smoothed RTT | [6.8.1](06-video.md#681-retransmission) |
| `DECODED` records | at least every 100 ms while frames arrive, every 1 s otherwise | [6.7](06-video.md#67-video-feedback) |
| `LOSS` feedback in the loss state | at least every 20 ms | [6.8.3](06-video.md#683-client-rules) |
| Host tail-loss repair | 2 × smoothed RTT + 10 ms after a frame's last fragment | [6.8.4](06-video.md#684-host-rules) |
| Repeated recovery frame | 2 × smoothed RTT + 20 ms after the last, doubling with each that failed, up to 1 s | [6.8.4](06-video.md#684-host-rules) |
| Input coalescing; motion merging | 1 ms; up to 8 ms | [8.6](08-input.md#86-client-rules) |
| Input repeat interval | max(2 ms, smoothed RTT / 4) | [8.6](08-input.md#86-client-rules) |
| Input retention | 1 s | [8.6](08-input.md#86-client-rules) |
| Host wait for a missing input report | max(20 ms, smoothed RTT) | [8.7](08-input.md#87-host-rules) |
| Input heartbeat while anything is held; host release | 100 ms; after 1 s silence | [8.6](08-input.md#86-client-rules), [8.7](08-input.md#87-host-rules) |
| `INPUT_ACK` rate | at most 1 per ms | [8.5](08-input.md#85-input_ack-datagram) |
| `DEVICE_OUTPUT` repeats; client rumble stop | 1/s, 100 ms while rumbling; after 250 ms silence | [8.8.1](08-input.md#881-device_output-datagram) |
| HID feature request timeout | 1 s (recommended) | [8.8.3](08-input.md#883-hid_feature_request-0x35-and-hid_feature_response-0x36) |
| Cursor warps applied to the client's pointer | at most 60/s | [9.4](09-cursor.md#94-cursor_warp-0x39) |
| Clipboard answer | 5 s | [10.3](10-clipboard.md#103-clipboard_request-0x49-and-clipboard_reject-0x4a) |
| Audio jitter buffer | 5–20 ms (recommended) | [7.6](07-audio.md#76-receiving-audio) |
| Control message; pre-authentication budget | 1 MiB; 64 KiB | [3.6.1](03-transport.md#361-framing), [11.7](11-security.md#117-resource-exhaustion) |
| Capabilities; Auth Data | 64 of at most 4096 bytes; 8192 bytes | [4.6.1](04-session.md#461-format), [4.4.1](04-session.md#441-messages) |
| Frame payload; fragments per frame | 64 MiB; 65535 | [6.5](06-video.md#65-frame-payload), [6.6.2](06-video.md#662-sending-rules) |
| Report descriptor; passthrough input report; attached devices | 16384 bytes; 1024 bytes; at least 16 | [8.2](08-input.md#82-devices) |
| Cursor images | 128, each at most 512 × 512 | [9.2](09-cursor.md#92-cursor-images) |
