# 6. Display and video

## 6.1 Model

The host streams one or more **monitors**. Each monitor has:

- exactly one **motion layer** video stream, which carries every change with the
  lowest possible latency and is normally decoded in hardware; and
- optionally one **refinement layer** video stream, which re-sends composition
  blocks that have stopped changing at higher fidelity, up to bit-exact.

The client decodes both and combines them block by block
([Section 6.9](#69-composition)). Both layers cover the whole monitor at its full
resolution.

## 6.2 DISPLAY_REQUEST (0x10)

The client describes the displays, or windows, it will use to show the session.

```
DISPLAY_REQUEST Payload {
  Request ID (i),
  Monitor Count (i),
  Client Monitor (..) ...,
}

Client Monitor {
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

- **Request ID**: increases with each `DISPLAY_REQUEST`; echoed in the answer.
- **Monitor Count**: from 1 to the host's Max Monitors.
- **Client Monitor ID**: the client's identifier for this display area.
- **Flags**: 0x01 `PRIMARY`; 0x02 `VRR_CAPABLE`; 0x04 `HDR_PQ` and 0x08 `HDR_HLG`,
  set when the display can currently present that format.
- **X**, **Y**: position in pixels in the client's desktop, relative to the
  top-left of the primary display.
- **Width**, **Height**: the size in pixels of the area that will show this
  monitor: the display resolution when full screen, or the window's content area.
- **Refresh Rate**: in millihertz (59940 for 59.94 Hz), or 0 if unknown. For a
  variable refresh display, the maximum rate.
- **Scale**: the client's interface scale for this display in percent (100 means
  no scaling). The host SHOULD apply the same scale to its monitor so that text
  appears at the size the user expects.
- **Physical Width**, **Physical Height**: in millimetres, or 0 if unknown.
- **Peak Luminance**: in candela per square metre, or 0 if unknown.

The client sends `DISPLAY_REQUEST` during configuration and again whenever its
displays change, for example when a window is resized or a display is added.

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

- **Config ID**: increases with each `DISPLAY_CONFIG`.
- **Request ID**: the `DISPLAY_REQUEST` this answers, or 0 when the host changed
  the configuration on its own, for example because a local user changed the
  resolution.

**Host Monitor fields:**

- **Client Monitor ID**: the client display area this monitor is meant for, or 0
  if none.
- **Flags**: 0x01 `PRIMARY`.
- **X**, **Y**: position in the host's desktop.
- **Width**, **Height**: in pixels, each from 1 to 16384.
- **Refresh Rate**: the host monitor's refresh rate in millihertz.
- **Scale**: the host monitor's interface scale in percent.

The host SHOULD create or configure monitors that match the request in size,
refresh rate and scale, for example by using virtual displays. When it cannot,
it reports what it actually uses, and the client scales the image to fit.

**Video Stream Config fields:**

- **Layer**: 0x00 `MOTION` or 0x01 `REFINEMENT`.
- **Codec**: 0x01 for AV1.
- **Profile**, **Bit Depth**: the AV1 `seq_profile` and bit depth of the stream.
- **Chroma**: 0x00 for 4:2:0, 0x01 for 4:4:4.
- **Flags**: 0x01 `FULL_RANGE` (full color range); 0x02 `LOSSLESS` (refinement
  layer only, [Section 6.4.3](#643-refinement-layer)).
- **Color Primaries**, **Transfer Characteristics**, **Matrix Coefficients**:
  code points from [H273]. They MUST match the color configuration in the
  stream's AV1 sequence header.

A stream's frame size is always its monitor's Width and Height. Video and audio
streams share one space of Stream IDs.

The host MUST answer each `DISPLAY_REQUEST` with a `DISPLAY_CONFIG` carrying its
Request ID, even when the configuration does not change.

Each monitor MUST have exactly one motion stream. It MAY have one refinement
stream, only if both endpoints advertised the `REFINEMENT` capability.

### 6.3.1 Reconfiguration

Each `DISPLAY_CONFIG` replaces the previous one completely.

- A stream whose ID appears in the old configuration but not the new one has
  ended. The client releases its decoder and discards its fragments.
- A stream ID that appears in both MUST have an identical Video Stream Config.
  Any change, including a change of monitor size, requires a new Stream ID.
- A new stream starts with Frame Number 1, which is a key frame.
- The client SHOULD keep showing a monitor's last image until the first frame of
  its new stream is decoded, so that a resize does not flash black.

## 6.4 AV1 bitstream requirements

### 6.4.1 All layers

Every video stream MUST conform to [AV1], and in addition:

1. Each frame payload carries one temporal unit in the low overhead bitstream
   format of [AV1]. Every OBU MUST have `obu_has_size_field` equal to 1.
2. Temporal units MUST NOT contain temporal delimiter OBUs or tile list OBUs.
   A receiver whose decoder needs a temporal delimiter adds one. Senders SHOULD
   NOT send padding OBUs.
3. Each temporal unit MUST contain exactly one frame with `show_frame` equal to 1
   and no frames with `show_frame` equal to 0, and MUST NOT use
   `show_existing_frame`. Frames are therefore never reordered and never held
   back by the decoder.
4. OBU extension headers MUST NOT be used (`obu_extension_flag` is 0): there is
   one operating point with no spatial or temporal scalability.
5. Every frame's size MUST equal the monitor size, super-resolution MUST NOT be
   enabled (`enable_superres` is 0), and the render size MUST equal the frame
   size.
6. Film grain synthesis MUST NOT be used (`film_grain_params_present` is 0).
7. Frame Number 1, and every frame flagged `KEY_FRAME`
   ([Section 6.6.1](#661-video_fragment-datagram)), MUST begin with a sequence
   header OBU followed by a key frame with `show_frame` equal to 1. The sequence
   header MUST NOT change within a stream.

### 6.4.2 Motion layer

The motion layer uses AV1 profile 0 (4:2:0) or profile 1 (4:4:4), at 8 or 10
bits, within the client's `VIDEO_CODEC` capability. The host SHOULD use 4:4:4 on
the motion layer only when the client decodes profile 1 in hardware or when the
stream's pixel rate fits the client's stated decoding throughput.

### 6.4.3 Refinement layer

The refinement layer uses AV1 profile 1 with 4:4:4 chroma, at the motion layer's
bit depth. When the stream's `LOSSLESS` flag is set:

- every frame MUST be coded losslessly as defined by [AV1]: `base_q_idx` is 0
  and all quantizer deltas are 0, so that `CodedLossless` is 1; and
- `matrix_coefficients` MUST be 0 (identity), so that the decoded G, B and R
  planes equal the captured pixel values exactly.

A host MUST set `LOSSLESS` only if both endpoints advertised the `LOSSLESS` flag
for AV1. Without it, refinement frames are lossy at a quality of the host's
choosing.

### 6.4.4 Tiles

To let clients decode in parallel without buffering frames, every frame MUST be
divided into at least the following number of AV1 tiles:

| Frame size in luma samples | Minimum tiles |
|---|---|
| up to 2,073,600 (1920 × 1080) | 1 (2 recommended) |
| up to 4,096,000 (2560 × 1600) | 4 |
| above 4,096,000 | 8 |

Tiles SHOULD have nearly equal areas. The limits of [AV1] still apply: a tile is
at most 4096 samples wide and at most 4096 × 2304 samples in area, and there are
at most 64 tile columns and 64 tile rows.

## 6.5 Frame payload

The data carried by a frame's fragments is its **frame payload**:

```
Frame Payload {
  Frame Header (..),
  Temporal Unit (..),
}

Frame Header {
  Capture Timestamp (i),
  [Recovery Base (i)],
  [Base Frame Number (i)],
  Blocks (..),
}
```

- **Capture Timestamp**: the host-clock time at which the source image was
  captured.
- **Recovery Base**: present only when the frame's fragments carry the
  `RECOVERY` flag and not the `KEY_FRAME` flag. It is the Frame Number of the
  last frame the client is assumed to have decoded
  ([Section 6.8](#68-loss-recovery)).
- **Base Frame Number**: present only in refinement streams. It is the Frame
  Number of the motion-layer frame, on the same monitor, whose source image this
  refinement frame reproduces.
- **Blocks**: a Block Map ([Section 6.5.1](#651-block-maps)). In a motion
  stream it lists the **changed blocks**: composition blocks whose source content
  differs from the source of the previous motion frame or, in a frame with a
  Recovery Base, from the source of frame Recovery Base. Measuring a recovery
  frame's changes from its Recovery Base accounts for the frames the client
  discarded while recovering. In a refinement stream it lists the **covered
  blocks**: blocks whose decoded pixels reproduce the source image of Base Frame
  Number, exactly when the stream is `LOSSLESS`.
- **Temporal Unit**: the AV1 temporal unit, running to the end of the payload.

A frame payload MUST NOT exceed 64 MiB.

### 6.5.1 Block maps

A monitor of W × H pixels is divided into composition blocks of 64 × 64 pixels,
with Bw = ⌈W / 64⌉ columns and Bh = ⌈H / 64⌉ rows; blocks in the last column or
row may be smaller. Block (c, r) covers x from 64c to min(64c + 64, W) − 1 and y
from 64r to min(64r + 64, H) − 1. Blocks are numbered in raster order,
i = r × Bw + c, from 0 to N − 1 where N = Bw × Bh.

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

- The first frame of a stream has Frame Number 1. Each new frame has the next
  number; the host never skips a number.
- The frame payload is split into consecutive pieces carried in fragments with
  indices 0, 1, 2 and so on. Every fragment except the last carries the same
  number of bytes, S ≥ 1; the last carries between 1 and S bytes. The data of
  fragment i therefore starts at byte i × S of the frame payload.
- `KEY_FRAME` and `RECOVERY` MUST be identical on every fragment of a frame.
  `LAST` MUST be set on exactly the final fragment. A frame has at most 65535
  fragments.
- The host SHOULD start sending fragments as soon as the corresponding encoded
  data is available, rather than waiting for the whole temporal unit. Fixing S
  in advance makes this possible: the number of fragments is only known when the
  `LAST` fragment is sent.
- A retransmission repeats a fragment exactly, with `RETRANSMISSION` added.

### 6.6.3 Reassembly

The client keeps the fragments of each incomplete frame. It learns S from the
length of any fragment without `LAST`, and places fragment i at offset i × S. A
frame is complete when its `LAST` fragment and every lower index have arrived.
Duplicate fragments are ignored. The client MAY pass the contiguous beginning of
a frame to a decoder that accepts partial temporal units.

The client MUST limit the memory it uses for incomplete frames. It SHOULD keep at
most 16 incomplete frames per stream; when it must drop one, it drops the oldest
and treats it as lost. Fragments of frames numbered at or below the stream's last
decoded frame are dropped.

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
| 0x03 | `LOSS` | Stream ID (i), Last Decoded Frame (i) |
| 0x04 | `FRAME_TIMING` | Stream ID (i), Frame Number (i), First Fragment Time (i), Last Fragment Time (i), Decoded Time (i), Presented Time (i) |
| 0x05 | `DISPLAY_TIMING` | See [Section 5.3](05-timing.md#53-display-timing) |

```
Missing Range {
  First Index (i),
  Count (i),
}
```

- **DECODED**: the highest Frame Number of the stream the client has decoded. The
  client SHOULD send it after each decoded frame and at least every 100 ms while
  frames are arriving.
- **NACK**: fragments of one frame that the client is missing. A Missing Range
  with Count 0 means "First Index and every later fragment", for when the `LAST`
  fragment has not arrived and the frame's length is unknown.
- **LOSS**: the client has given up on a frame and stopped decoding the stream
  after Last Decoded Frame ([Section 6.8](#68-loss-recovery)).
- **FRAME_TIMING**: client-clock times at which the frame's first and last
  fragments arrived, at which it finished decoding, and at which it was
  presented. Presented Time is 0 if a newer frame replaced it before
  presentation. The client SHOULD report every motion-layer frame; several
  records may share one datagram.

## 6.8 Loss recovery

### 6.8.1 Retransmission

The client SHOULD send a `NACK` for a frame's missing fragments when a fragment
of a later frame of the same stream arrives, or when the frame has been
incomplete with no new fragment for max(1 ms, RTT / 8). It SHOULD NOT request the
same fragment again within one smoothed round-trip time.

The host SHOULD retransmit requested fragments it still holds, unless sending a
recovery frame would reach the client sooner. It SHOULD keep each frame's
fragments for at least two smoothed round-trip times after sending them. At the
round-trip times QDP targets, a retransmission usually arrives within a frame
interval.

### 6.8.2 Declaring loss

The client declares a frame lost when it cannot be completed in time. It SHOULD
do so when the frame is still incomplete one smoothed round-trip time plus 4 ms
after its first `NACK`, or when it drops the frame to save memory.

### 6.8.3 Client rules

The client decodes the frames of a stream strictly in Frame Number order. When it
declares a frame lost, the stream enters the **loss state** with
L = the stream's Last Decoded Frame (0 if nothing has been decoded). In the loss
state the client:

1. MUST NOT pass any frame to the decoder except a complete frame that has the
   `KEY_FRAME` flag, or a complete frame that has the `RECOVERY` flag and a
   Recovery Base equal to L;
2. discards every other frame;
3. MUST include a `LOSS` record for the stream in every `VIDEO_FEEDBACK` datagram,
   and MUST send `VIDEO_FEEDBACK` at least every 20 ms.

After decoding an acceptable frame, the stream leaves the loss state and
decoding continues in order from that frame.

### 6.8.4 Host rules

When the host receives `LOSS` for stream S with Last Decoded Frame L, it MUST, as
soon as possible, send a frame with the `RECOVERY` flag that is either:

- a key frame (`KEY_FRAME` also set, no Recovery Base); or
- a frame with Recovery Base L that a decoder can decode correctly after decoding
  the frames of S in order up to and including L, and no later frames.

If L is 0, the recovery frame MUST be a key frame. Frames sent after a recovery
frame MUST NOT depend on frames numbered between L and the recovery frame. After
sending a recovery frame for L, the host SHOULD wait one smoothed round-trip time
before sending another for the same L, because `LOSS` records sent before the
recovery frame arrived will still be in flight.

Recovery from an earlier reference is much smaller than a key frame. Encoders
that support reference invalidation or long-term references can provide it
([Section 6.11](#611-encoding-guidance-non-normative)).

## 6.9 Composition

For each monitor, the client keeps:

- the latest decoded motion-layer frame;
- a **refined image** the size of the monitor; and
- for each composition block b, `changed[b]`, the Frame Number of the most
  recent decoded motion frame whose changed blocks include b, and `refined[b]`,
  the Base Frame Number of the most recent decoded refinement frame whose
  covered blocks include b, or "none".

When a motion frame is decoded, the client sets `changed[b]` to its Frame Number
for each changed block. Frame Number 1, and a key frame that ends a loss state
([Section 6.8.3](#683-client-rules)), mark every block changed, whatever their
maps say, because the client cannot know what changed in the frames it did not
decode.

When a refinement frame is decoded, the client copies each covered block's pixels
from the decoded frame into the refined image and sets `refined[b]` to the frame's
Base Frame Number. Pixels of blocks that are not covered MUST NOT be used.

The client displays block b from the refined image if `refined[b]` is not "none"
and `refined[b] ≥ changed[b]`, and from the latest motion frame otherwise. Both
layers are converted to a common color space before they are combined. When a
monitor gets new streams, every `refined[b]` is reset to "none".

This rule needs no timing assumptions. A refinement frame describes the source as
it was at motion frame Base Frame Number; a block that changed in a later motion
frame shows the motion layer until a newer refinement covers it.

## 6.10 KEYFRAME_REQUEST (0x12)

```
KEYFRAME_REQUEST Payload {
  Stream ID (i),
}
```

The client sends `KEYFRAME_REQUEST` when its decoder for the stream has failed or
been reset and cannot continue from any earlier frame. The stream enters the loss
state, accepting only a key frame. The host MUST answer with a key frame flagged
`KEY_FRAME` and `RECOVERY`. Ordinary packet loss is handled with `LOSS` records,
not with this message.

## 6.11 Encoding guidance (non-normative)

- **Change tracking.** Compare each captured image with the previous one block by
  block, for example by hashing each 64 × 64 block on the GPU. This produces the
  changed-blocks map and tells the host whether to encode a frame at all. It works
  identically on every operating system and does not depend on the OS reporting
  changed areas correctly.
- **Motion encoder settings.** No B-frames, no lookahead, one shown frame per
  temporal unit, intra refresh instead of periodic key frames, the tile counts of
  [Section 6.4.4](#644-tiles), and a per-frame byte budget
  ([Section 5.5.2](05-timing.md#552-frame-budgets-non-normative)).
- **Recovery.** Keep at least one reference slot holding a frame the client has
  acknowledged as decoded. On `LOSS`, encode the next frame referencing only that
  slot. Many hardware encoders expose this as reference picture invalidation.
- **Refinement scheduling.** Queue blocks for refinement once they are unchanged
  in a motion frame. Send refinement frames with spare capacity only, sized so
  they never delay the next motion frame, and refine blocks near the pointer
  first. Regions that change every frame, such as video or games, are never
  queued, so they stay on the motion layer automatically.
