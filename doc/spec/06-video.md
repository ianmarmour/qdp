# 6. Display and video

## 6.1 Model

The host streams one or more **monitors**. Each monitor has:

- exactly one **motion layer** video stream, which carries every change with the
  lowest possible latency and is normally decoded in hardware; and
- optionally one **refinement layer** video stream, which re-sends composition
  blocks that have stopped changing at higher fidelity, up to bit-exact.

The client decodes both and combines them block by block
([Section 6.9](#69-composition)). Both layers cover the whole monitor at its full
resolution. Monitors are laid out in the host desktop
([Section 1.5.6](01-introduction.md#156-positions)).

## 6.2 DISPLAY_REQUEST (0x10)

The client describes the displays, or windows, it will use to show the session.

```
DISPLAY_REQUEST Payload {
  Request ID (i),
  Monitor Count (i),
  Client Monitor (..) ...,
}

Client Monitor {
  Entry Length (i),
  Client Monitor ID (i),
  Flags (8),
  X (si),
  Y (si),
  Width (i),
  Height (i),
  Refresh Rate (i),
  Scale (i),
  Physical Width (i),
  Physical Height (i),
  Peak Luminance (i),
}
```

- **Request ID**: starts at 1 and increases with each `DISPLAY_REQUEST`.
- **Monitor Count**: from 1 to the host's Max Monitors.
- **Entry Length**: the number of bytes in the rest of this entry. A receiver skips
  any bytes after the fields it knows.
- **Client Monitor ID**: the client's identifier for this display area.
- **Flags**: 0x01 `PRIMARY`; 0x02 `VRR_CAPABLE`; 0x04 `HDR_PQ` and 0x08 `HDR_HLG`,
  set when the display can currently present that format.
- **X**, **Y**: position in physical pixels in the client's desktop, relative to the
  top-left of the primary display.
- **Width**, **Height**: the size in physical pixels of the area that will show this
  monitor: the display resolution when full screen, or the window's content area.
- **Refresh Rate**: in millihertz (59940 for 59.94 Hz), or 0 if unknown. For a
  variable refresh display, the maximum rate. A client MAY request a rate below the
  display's, for example to save battery.
- **Scale**: the client's interface scale for this display in percent (100 means no
  scaling). The host SHOULD apply the same scale to its monitor so that text appears
  at the size the user expects.
- **Physical Width**, **Physical Height**: in millimetres, or 0 if unknown.
- **Peak Luminance**: in candela per square metre, or 0 if unknown.

The client sends `DISPLAY_REQUEST` during configuration and again whenever its
displays change. While the user is resizing a window, the client SHOULD send at most
one `DISPLAY_REQUEST` every 250 ms and one when the resize ends, and scales the last
image in between.

## 6.3 DISPLAY_CONFIG (0x11)

The host announces the monitors it will stream and the video streams for each.

```
DISPLAY_CONFIG Payload {
  Config ID (i),
  Request ID (i),
  Monitor Count (i),
  Host Monitor (..) ...,
  Stream Count (i),
  Video Stream Config (..) ...,
}

Host Monitor {
  Entry Length (i),
  Monitor ID (i),
  Client Monitor ID (i),
  Flags (8),
  X (si),
  Y (si),
  Width (i),
  Height (i),
  Refresh Rate (i),
  Scale (i),
}

Video Stream Config {
  Entry Length (i),
  Stream ID (i),
  Monitor ID (i),
  Layer (8),
  Codec (i),
  Profile (8),
  Bit Depth (8),
  Chroma (8),
  Flags (8),
  Color Primaries (8),
  Transfer Characteristics (8),
  Matrix Coefficients (8),
}
```

**DISPLAY_CONFIG fields:**

- **Config ID**: starts at 1 and increases with each `DISPLAY_CONFIG`. Input reports
  name the Config ID whose layout they use
  ([Section 8.3](08-input.md#83-input-datagram)).
- **Request ID**: the highest `DISPLAY_REQUEST` this configuration answers, or 0 when
  the host changed the configuration on its own, for example because a local user
  changed the resolution. A `DISPLAY_CONFIG` answers every request up to and
  including its Request ID; the host MAY act only on the newest pending request and
  MAY limit how often it reconfigures.

**Host Monitor fields:**

- **Entry Length**: as for Client Monitor.
- **Client Monitor ID**: the client display area this monitor is meant for, or 0 if
  none.
- **Flags**: 0x01 `PRIMARY`.
- **X**, **Y**, **Width**, **Height**: the monitor's rectangle in the host desktop, in
  pixels of the monitor's video ([Section 1.5.6](01-introduction.md#156-positions)).
  Width and Height are each from 16 to 16384, MUST NOT exceed the client's Max Width
  and Max Height, and rectangles MUST NOT overlap.
- **Refresh Rate**: the host monitor's refresh rate in millihertz.
- **Scale**: the host monitor's interface scale in percent.

The host SHOULD create or configure monitors that match the request in size, refresh
rate and scale, for example by using virtual displays. When it cannot, it reports
what it actually uses, and the client scales the image to fit. The host SHOULD NOT
send motion frames for a monitor more often than the Refresh Rate the client
requested for the corresponding display area.

**Video Stream Config fields:**

- **Entry Length**: as for Client Monitor.
- **Layer**: 0x00 `MOTION` or 0x01 `REFINEMENT`.
- **Codec**: 0x01 for AV1.
- **Profile**, **Bit Depth**: the AV1 `seq_profile` and bit depth of the stream.
- **Chroma**: 0x00 for 4:2:0, 0x01 for 4:4:4.
- **Flags**: 0x01 `FULL_RANGE` (full color range); 0x02 `LOSSLESS` (refinement layer
  only, [Section 6.4.3](#643-refinement-layer)).
- **Color Primaries**, **Transfer Characteristics**, **Matrix Coefficients**: code
  points from [H273]. The value 2 ("unspecified") MUST NOT be used. Standard dynamic
  range streams use primaries 1 and transfer characteristics 13 (sRGB); PQ streams
  use 9 and 16; HLG streams use 9 and 18. The matrix is 1 (BT.709) or 9 (BT.2020)
  for YUV streams and 0 (identity) for RGB streams.

Every field of a Video Stream Config MUST match the stream's AV1 sequence header,
which MUST have `color_description_present_flag` set. A client that finds a mismatch
MUST close the connection with `QDP_PROTOCOL_VIOLATION`. A stream's frame size is
always its monitor's Width and Height. Video and audio streams share one space of
Stream IDs.

Each monitor MUST have exactly one motion stream. It MAY have one refinement stream,
only if both endpoints advertised the `REFINEMENT` capability. The refinement stream
uses the color primaries, transfer characteristics and bit depth of its motion
stream.

The host MUST answer each `DISPLAY_REQUEST` with a `DISPLAY_CONFIG` whose Request ID
is at least that request's, even when the configuration does not change.

### 6.3.1 Reconfiguration

Each `DISPLAY_CONFIG` replaces the previous one completely.

- A stream whose ID appears in the old configuration but not the new one has ended.
  The client releases its decoder and discards its fragments.
- A stream ID that appears in both MUST have an identical Video Stream Config. Any
  change, including a change of monitor size, requires a new Stream ID.
- When a monitor's motion stream is replaced, its refinement stream, if any, MUST be
  replaced too, so that Base Frame Numbers always refer to the current motion stream.
- A new stream starts with Frame Number 1, which is a key frame.
- The client SHOULD keep showing a monitor's last image until the first frame of its
  new streams is decoded, so that a resize does not flash black.

## 6.4 AV1 bitstream requirements

### 6.4.1 All layers

Every video stream MUST conform to [AV1], and in addition:

1. Each frame payload carries one temporal unit in the low overhead bitstream format
   of [AV1]. Every OBU MUST have `obu_has_size_field` equal to 1.
2. Temporal units MUST NOT contain temporal delimiter OBUs or tile list OBUs. A
   receiver whose decoder needs a temporal delimiter adds one. Senders SHOULD NOT
   send padding OBUs.
3. Each temporal unit MUST contain exactly one frame with `show_frame` equal to 1 and
   no frames with `show_frame` equal to 0, and MUST NOT use `show_existing_frame`.
   Frames are therefore never reordered and never held back by the decoder.
4. OBU extension headers MUST NOT be used (`obu_extension_flag` is 0): there is one
   operating point with no spatial or temporal scalability.
5. Every frame's render size MUST equal the monitor size, and super-resolution MUST
   NOT be enabled (`enable_superres` is 0). The frame size SHOULD equal the monitor
   size, but a host whose encoder can only code aligned sizes MAY pad frames on the
   right by fewer than 64 samples and at the bottom by fewer than 16. Receivers show
   only the top-left region of the monitor size.
6. Film grain synthesis MUST NOT be used (`film_grain_params_present` is 0).
7. Frame Number 1, and every frame flagged `KEY_FRAME`
   ([Section 6.6.1](#661-video_fragment-datagram)), MUST begin with a sequence header
   OBU, optionally followed by metadata OBUs, followed by a key frame with
   `show_frame` equal to 1. Frame Number 1 MUST be flagged `KEY_FRAME`. The sequence
   header MUST NOT change within a stream.
8. The sequence header MUST signal a level (`seq_level_idx`) that the stream meets and
   that does not exceed the client's Max Level ([Section 4.6.2](04-session.md#462-video_codec-0x01-repeatable)).

### 6.4.2 Motion layer

The motion layer uses AV1 profile 0 (4:2:0) or profile 1 (4:4:4), at 8 or 10 bits,
within the client's `VIDEO_CODEC` capability. The host SHOULD use 4:4:4 on the motion
layer only when the client decodes profile 1 in hardware or when the stream's pixel
rate fits the client's stated decoding throughput.

### 6.4.3 Refinement layer

The refinement layer uses AV1 profile 1 with 4:4:4 chroma, at the motion layer's bit
depth. When the stream's `LOSSLESS` flag is set:

- every frame MUST be coded losslessly as defined by [AV1]: `base_q_idx` is 0 and all
  quantizer deltas are 0, so that `CodedLossless` is 1;
- `matrix_coefficients` MUST be 0 (identity), so that the decoded G, B and R planes
  are the captured pixel values; and
- the stream MUST use full color range (`color_range` 1, and the `FULL_RANGE` flag),
  so that the decoded values are used without range conversion.

A host MUST set `LOSSLESS` only if both endpoints advertised the `LOSSLESS` flag for
AV1. Without it, refinement frames are lossy at a quality of the host's choosing.

### 6.4.4 Tiles

To let clients decode in parallel without buffering frames, every frame MUST be
divided into at least the following number of AV1 tiles:

| Frame size in luma samples | Minimum tiles |
|---|---|
| up to 2,073,600 (1920 × 1080) | 1 (2 recommended) |
| up to 4,096,000 (2560 × 1600) | 4 |
| above 4,096,000 | 8 |

Tiles SHOULD have nearly equal areas. The limits of [AV1] still apply: a tile is at
most 4096 samples wide and at most 4096 × 2304 samples in area, and there are at most
64 tile columns and 64 tile rows.

## 6.5 Frame payload

The data carried by a frame's fragments is its **frame payload**:

```
Frame Payload {
  Frame Header (..),
  Temporal Unit (..),
}

Frame Header {
  Header Length (i),
  Capture Timestamp (i),
  [Recovery Base (i)],
  [Base Frame Number (i)],
  Blocks (..),
}
```

- **Header Length**: the number of bytes in the rest of the Frame Header. The temporal
  unit starts after them; a receiver skips any header bytes after the fields it knows.
- **Capture Timestamp**: the host-clock time at which the source image was captured.
- **Recovery Base**: present only when the frame's fragments carry the `RECOVERY` flag
  and not the `KEY_FRAME` flag. It is the Frame Number of the last frame the client is
  assumed to have decoded ([Section 6.8](#68-loss-recovery)).
- **Base Frame Number**: present only in refinement streams. It is the Frame Number of
  the motion-layer frame, on the same monitor, whose source image this refinement
  frame reproduces.
- **Blocks**: a Block Map ([Section 6.5.1](#651-block-maps)). In a motion stream it
  lists the **changed blocks**: composition blocks whose source content differs from
  the source of the previous motion frame. In a frame with a Recovery Base, it is the
  union of the changed blocks of every motion frame after Recovery Base up to and
  including this one, so that changes in the frames the client discarded are not
  missed. In a refinement stream it lists the **covered blocks**: blocks whose decoded
  pixels reproduce the source image of Base Frame Number, exactly when the stream is
  `LOSSLESS`.
- **Temporal Unit**: the AV1 temporal unit, running to the end of the payload.

A frame payload MUST NOT exceed 64 MiB.

### 6.5.1 Block maps

A monitor of W × H pixels is divided into composition blocks of 64 × 64 pixels, with
Bw = ⌈W / 64⌉ columns and Bh = ⌈H / 64⌉ rows; blocks in the last column or row may be
smaller. Block (c, r) covers x from 64c to min(64c + 64, W) − 1 and y from 64r to
min(64r + 64, H) − 1. Blocks are numbered in raster order, i = r × Bw + c, from 0 to
N − 1 where N = Bw × Bh.

```
Block Map {
  Encoding (8),
  [Bitmap (..)],
  [Run Count (i), Run Length (i) ...],
}
```

| Encoding | Name | Contents |
|---|---|---|
| 0x00 | `NONE` | No blocks are set. No further fields. |
| 0x01 | `ALL` | All blocks are set. No further fields. |
| 0x02 | `BITMAP` | ⌈N / 8⌉ bytes. Block i is set when byte ⌊i / 8⌋ has the bit with mask `1 << (i mod 8)` set. Unused bits in the last byte MUST be zero. |
| 0x03 | `RUNS` | Run Count, then that many run lengths. Runs alternate unset, set, unset, and so on, starting with an unset run, which may have length 0. Blocks after the last run are unset. The run lengths MUST NOT sum to more than N. |

Senders SHOULD use whichever encoding is shortest.

## 6.6 Video fragments

### 6.6.1 VIDEO_FRAGMENT datagram

```
VIDEO_FRAGMENT Datagram {
  Datagram Type (i) = 0x01,
  Stream ID (i),
  Frame Number (i),
  Fragment Index (i),
  Flags (8),
  Fragment Data (..),
}
```

| Flag | Name | Meaning |
|---|---|---|
| 0x01 | `KEY_FRAME` | The temporal unit begins with a sequence header and a key frame. |
| 0x02 | `RECOVERY` | The frame ends a loss state ([Section 6.8](#68-loss-recovery)). |
| 0x04 | `RETRANSMISSION` | This datagram repeats an earlier one. |
| 0x08 | `LAST` | This is the frame's last fragment. |

### 6.6.2 Sending rules

- The first frame of a stream has Frame Number 1. Each new frame has the next number;
  the host never skips a number.
- The frame payload is split into consecutive pieces carried in fragments with
  indices 0, 1, 2 and so on. Every fragment except the last carries the same number of
  bytes, S ≥ 1; the last carries between 1 and S bytes. The data of fragment i
  therefore starts at byte i × S of the frame payload. S may differ between frames.
- The host chooses S so that every fragment of the frame fits the maximum datagram
  size, allowing for the largest Fragment Index the frame may need. If the maximum
  datagram size shrinks so that a frame's remaining or retransmitted fragments no
  longer fit, the host abandons the frame, and the client recovers from its loss.
- `KEY_FRAME` and `RECOVERY` MUST be identical on every fragment of a frame. `LAST`
  MUST be set on exactly the final fragment. A frame has at most 65535 fragments.
- The host SHOULD start sending fragments as soon as the corresponding encoded data is
  available, rather than waiting for the whole temporal unit. Fixing S in advance
  makes this possible: the number of fragments is only known when the `LAST` fragment
  is sent.
- A retransmission repeats a fragment exactly, with `RETRANSMISSION` added.

### 6.6.3 Reassembly

The client keeps the fragments of each incomplete frame. It learns S from the length
of any fragment without `LAST`, and places fragment i at offset i × S. A frame is
complete when its `LAST` fragment and every lower index have arrived. Duplicate
fragments are ignored. A frame whose fragments are inconsistent (non-`LAST` fragments
of different lengths, a `LAST` fragment longer than S, a fragment above the `LAST`
fragment's index, or differing flags) is treated as lost. Fragments for a Stream ID
whose `DISPLAY_CONFIG` has not yet arrived are buffered as
[Section 3.7.3](03-transport.md#373-processing-rules) describes.

The client MAY pass the contiguous beginning of a frame to a decoder that accepts
partial temporal units, but only if it can abandon a partially decoded frame and
return the decoder to its state after its Last Decoded Frame. A client that cannot do
so and abandons a partially fed frame MUST treat its decoder as reset
([Section 6.10](#610-keyframe_request-0x12)).

The client MUST limit the memory it uses for incomplete frames. It SHOULD keep at most
16 incomplete frames per stream; when it must drop one, it drops the oldest and treats
it as lost. Fragments of frames numbered at or below the stream's Last Decoded Frame
are dropped.

## 6.7 Video feedback

```
VIDEO_FEEDBACK Datagram {
  Datagram Type (i) = 0x02,
  Record (..) ...,
}

Record {
  Record Type (i),
  Record Length (i),
  Record Value (..),
}
```

A `VIDEO_FEEDBACK` datagram carries one or more records, for any streams.

| Record Type | Name | Value |
|---|---|---|
| 0x01 | `DECODED` | Stream ID (i), Last Decoded Frame (i) |
| 0x02 | `NACK` | Stream ID (i), Frame Number (i), Range Count (i), Missing Range (..) ... |
| 0x03 | `LOSS` | Stream ID (i), Last Decoded Frame (i), Highest Frame Seen (i) |
| 0x04 | `FRAME_TIMING` | Stream ID (i), Frame Number (i), First Fragment Time (i), Last Fragment Time (i), Decoded Time (i), Presented Time (i) |
| 0x05 | `DISPLAY_TIMING` | See [Section 5.3](05-timing.md#53-display-timing) |

```
Missing Range {
  First Index (i),
  Count (i),
}
```

- **DECODED**: the highest Frame Number of the stream the client has decoded. The
  client SHOULD send it after each decoded frame, and MUST send it at least every
  100 ms while frames of the stream are arriving and at least once per second
  otherwise, so that the host can detect a lost final frame. A retransmitted
  fragment of a frame the client has decoded means the host has not seen it
  decoded, and the client SHOULD send `DECODED` at once.
- **NACK**: fragments of one frame that the client is missing. A Missing Range with
  Count 0 means "First Index and every later fragment", for when the `LAST` fragment
  has not arrived and the frame's length is unknown. Missing Range is a frozen
  structure ([Section 1.6](01-introduction.md#16-extensibility)).
- **LOSS**: the client has given up on a frame and stopped decoding the stream after
  Last Decoded Frame ([Section 6.8](#68-loss-recovery)). Highest Frame Seen is the
  highest Frame Number of the stream of which the client has received any fragment.
- **FRAME_TIMING**: client-clock times at which the frame's first and last fragments
  arrived, at which it finished decoding, and at which it was presented. Presented
  Time is 0 if a newer frame replaced it before presentation. The client SHOULD report
  every motion-layer frame; several records may share one datagram.

## 6.8 Loss recovery

### 6.8.1 Retransmission

The client sends a `NACK` for a frame's missing fragments when it has evidence that
they are missing: a fragment with a higher index of the same frame, or a fragment of a
later frame of the same stream, has arrived. A frame none of whose fragments has
arrived is missing entirely once a fragment of a later frame arrives, and its `NACK`
names all of its fragments. The client MAY also send a `NACK` when a frame has been
incomplete with no new fragment for max(1 ms, smoothed RTT / 8). It SHOULD NOT
request the same fragment again within one smoothed RTT, and ignores fragments of a
frame it is decoding, or has abandoned for it.

The host SHOULD retransmit requested fragments it still holds, unless sending a
recovery frame would reach the client sooner. It ignores requests for fragments it
has not sent yet, and does not resend a fragment it sent less than one smoothed RTT
ago. A fragment is sent when it leaves the host for the network, not when it is
queued: one waiting behind the congestion window will still arrive, and queuing it
again would only lengthen the wait for everything behind it. The Missing Ranges of a
`NACK` are whole runs of missing fragments, so the fragment after a range with a
nonzero Count that ends inside the frame has arrived; fragments arrive in the order
they were sent, so those sent before it are lost rather than late, and the host MAY
resend them at once however recently they were sent. The host MUST keep each frame's
fragments for at least two smoothed RTTs after sending
them, and SHOULD keep them for the longer of 100 ms and four smoothed RTTs, so that a
large frame can recover over several rounds of `NACK`s. At the round-trip times QDP targets, a retransmission usually arrives
within a frame interval.

### 6.8.2 Declaring loss

The client declares a frame lost when it stops making progress while the stream does
not: when its fragments are still missing one smoothed RTT plus 4 ms after its first
`NACK`, no new fragment of the frame has arrived in that time, and some fragment of the
stream has arrived since the timer started. Each newly arriving fragment of the frame
restarts this timer; repeating a `NACK` does not, or a frame whose fragments never come
would wait forever. Fragments arrive in the order they were sent, so while nothing
arrives at all, as when a Wi-Fi client's radio briefly leaves its channel and then
delivers what it held, the missing fragments are late rather than lost, and declaring
the frame lost would only cost a recovery frame. A frame dropped to save memory, or found inconsistent, is
also lost.

### 6.8.3 Client rules

The client decodes the frames of a stream strictly in Frame Number order. Each stream
is in one of three states:

| State | Entered when | Frames the client decodes |
|---|---|---|
| `KEY_ONLY` | the stream starts, or its decoder fails or is reset (L = 0) | only a complete frame flagged `KEY_FRAME` |
| `DECODING` | it decodes an acceptable frame | the next Frame Number in order; or at once, abandoning lower frames, a complete `KEY_FRAME` frame or a complete `RECOVERY` frame whose Recovery Base equals its Last Decoded Frame |
| `LOSS` | it declares a frame lost (L = its Last Decoded Frame) | a complete `KEY_FRAME` frame, or a complete `RECOVERY` frame with Recovery Base L, then the frames after it in order |

In the `LOSS` and `KEY_ONLY` states the client:

1. keeps, within its memory limits, frames numbered above the newest frame that would
   be acceptable, so that it can decode them in order once that frame is decoded, and
   discards other frames;
2. sends `NACK` only for acceptable frames and the frames after them;
3. MUST include a `LOSS` record for the stream, with Last Decoded Frame L, in every
   `VIDEO_FEEDBACK` datagram, and MUST send `VIDEO_FEEDBACK` at least every 20 ms,
   until it holds a complete frame it would decode. In the `KEY_ONLY` state, L is 0,
   which forces the host to send a key frame. A frame that arrived whole is not lost
   while the decoder works on it: a `LOSS` record then, its Highest Frame Seen at or
   above that frame, would tell the host that its recovery frame failed
   ([Section 6.8.4](#684-host-rules)), and cost another.

### 6.8.4 Host rules

When the host receives `LOSS` for stream S with Last Decoded Frame L, it MUST, as soon
as possible, send a frame with the `RECOVERY` flag that is either:

- a key frame (`KEY_FRAME` also set, no Recovery Base); or
- a frame with Recovery Base L that a decoder can decode correctly after decoding the
  frames of S in order up to and including L, and no later frames.

If L is 0, the recovery frame MUST be a key frame. Frames sent after a recovery frame
MUST NOT depend on frames numbered between L and the recovery frame.

`LOSS` records keep arriving until the recovery frame has reached the client. After
sending recovery frame R for L, the host does not send another for the same L while
the records' Highest Frame Seen is below R, unless 2 × smoothed RTT + 20 ms have
passed since it sent R's last fragment. A `LOSS` for L with Highest Frame Seen at or
above R means R did not arrive complete, and the host sends a new recovery frame.

Recovery frames that keep failing usually mean the host sends more than the path
carries, and a key frame, many times larger than the frames around it, adds most
to the overload. So the host backs off: the k-th recovery frame in a row that fails
delays the next until 2^(k−1) × (2 × smoothed RTT + 20 ms) after the failed one was
sent, but no more than 1 s after it. Until the next is sent, the host sends no
other frames of the stream, which the client could not decode. Once the client
reports a recovery frame decoded, the count starts again and nothing is held back.

If the client's `DECODED` records have not reached the newest frame N of a stream
within 2 × smoothed RTT + 10 ms of the host sending N's last fragment, and the host
has sent no later frame, N or its last fragments may have been lost while the screen
stayed still. The host MUST then resend N's last fragment or send a new frame of the
current content. The last fragment is enough: a client that lacks others of N's
fragments learns from it how many N has, and asks for them
([Section 6.8.1](#681-retransmission)); one that decoded N answers with `DECODED`.
Resending all of N, which may be hundreds of datagrams, would add most to the very
queue that may be what delays the `DECODED`. The timer starts again when the
resent fragment is sent.

Recovery from an earlier reference is much smaller than a key frame. Encoders that
support reference invalidation or long-term references can provide it
([Section 6.11](#611-encoding-guidance-non-normative)).

## 6.9 Composition

For each monitor, the client keeps:

- the latest decoded motion-layer frame;
- a **refined image** the size of the monitor; and
- for each composition block b, `changed[b]`, the Frame Number of the most recent
  decoded motion frame whose changed blocks include b, and `refined[b]`, the Base
  Frame Number of the most recent decoded refinement frame whose covered blocks
  include b, or "none".

When a motion frame is decoded, the client sets `changed[b]` to its Frame Number for
each changed block. Frame Number 1, and a key frame that ends the `LOSS` or `KEY_ONLY`
state, mark every block changed, whatever their maps say.

When a refinement frame is decoded, the client copies each covered block's pixels from
the decoded frame into the refined image and sets `refined[b]` to the frame's Base
Frame Number. Pixels of blocks that are not covered MUST NOT be used.

The client displays block b from the refined image if `refined[b]` is not "none" and
`refined[b] ≥ changed[b]`, and from the latest motion frame otherwise. Each layer is
converted to non-linear RGB with its own matrix and range, and the two are combined in
the motion stream's color space. When a monitor's streams are replaced, every
`refined[b]` is reset to "none", and the client keeps showing the last composited
image until the new motion stream's first frame is decoded.

This rule needs no timing assumptions. A refinement frame describes the source as it
was at motion frame Base Frame Number; a block that changed in a later motion frame
shows the motion layer until a newer refinement covers it.

## 6.10 KEYFRAME_REQUEST (0x12)

```
KEYFRAME_REQUEST Payload {
  Stream ID (i),
}
```

The client sends `KEYFRAME_REQUEST` when its decoder for the stream has failed or been
reset and cannot continue from any earlier frame. The stream enters the `KEY_ONLY`
state, in which the client reports `LOSS` with Last Decoded Frame 0
([Section 6.8.3](#683-client-rules)), so a lost key frame is replaced. The host MUST
answer with a key frame flagged `KEY_FRAME` and `RECOVERY`. Ordinary packet loss is
handled with `LOSS` records, not with this message.

## 6.11 Encoding guidance (non-normative)

- **Change tracking.** Compare each captured image with the previous one block by
  block, for example by hashing each 64 × 64 block on the GPU. This produces the
  changed-blocks map and tells the host whether to encode a frame at all. It works
  identically on every operating system and does not depend on the OS reporting
  changed areas correctly. Software encoders can also use the map to skip analysing
  unchanged areas.
- **Motion encoder settings.** No B-frames, no lookahead, one shown frame per temporal
  unit, intra refresh instead of periodic key frames, the tile counts of
  [Section 6.4.4](#644-tiles), and a per-frame byte budget
  ([Section 5.5.2](05-timing.md#552-frame-budgets-non-normative)).
- **Recovery.** Keep at least one reference slot holding a frame the client has
  acknowledged as decoded. On `LOSS`, encode the next frame referencing only that
  slot. Many hardware encoders expose this as reference picture invalidation.
- **Decoder backlog.** Keep the frames sent beyond the client's latest `DECODED` to
  about one frame interval plus one smoothed RTT, and lower the frame rate or
  resolution when `FRAME_TIMING` shows decoding falling behind arrival. Do not send a
  refinement frame while an earlier one is still undecoded.
- **Refinement scheduling.** Queue blocks for refinement once they are unchanged in a
  motion frame. Send refinement frames with spare capacity only, sized so they never
  delay the next motion frame, and refine blocks near the pointer first. Regions that
  change every frame, such as video or games, are never queued, so they stay on the
  motion layer automatically.
