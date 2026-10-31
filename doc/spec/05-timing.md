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
the sample with the smallest delay. Endpoints MAY estimate drift between the two
clocks by linear regression over a longer window and correct the offset between
samples.

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
fragment arrived. Converted to the host clock with the clock offset and compared
with when the host sent that fragment, they give a one-way transit time. Its
absolute value is uncertain by the offset error, but its trend is not: a steady
rise of a few milliseconds means a queue is building somewhere on the path, and
the host should lower its rate before packets are lost.
