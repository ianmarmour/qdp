# 7. Audio

## 7.1 Model

Audio flows in **audio streams**. A playback stream carries the host's audio
output to the client; a capture stream carries the client's microphone to the
host, which presents it as an audio input device. The host defines all audio
streams in `AUDIO_CONFIG`, including the client's capture stream, so that both
directions use parameters the host can handle.

## 7.2 AUDIO_REQUEST (0x18)

```
AUDIO_REQUEST Payload {
  Playback Channels (i),
  Capture Channels (i),
}
```

- **Playback Channels**: the number of output channels the client wants, from 1
  to 8, or 0 for no playback.
- **Capture Channels**: the number of microphone channels the client will send,
  usually 1, or 0 for no microphone.

The client sends `AUDIO_REQUEST` during configuration, and again when its audio
devices change or the user turns the microphone on or off.

## 7.3 AUDIO_CONFIG (0x19)

```
AUDIO_CONFIG Payload {
  Config ID (i),
  Stream Count (i),
  Audio Stream Config (..) ...,
}

Audio Stream Config {
  Stream ID (i),
  Direction (8),
  Codec (i),
  Sample Rate (i),
  Channel Count (i),
  Frame Duration (i),
  [Opus Mapping (..)],
}

Opus Mapping {
  Stream Count (8),
  Coupled Count (8),
  Channel Mapping (8) ...,
}
```

The host MUST answer each `AUDIO_REQUEST` with an `AUDIO_CONFIG`.

- **Config ID**: increases with each `AUDIO_CONFIG`. As with displays, each
  configuration replaces the previous one; streams that disappear have ended,
  and any change of parameters requires a new Stream ID. Audio and video Stream
  IDs share one space and MUST NOT collide.
- **Direction**: 0x00 `PLAYBACK` (host to client) or 0x01 `CAPTURE` (client to
  host).
- **Codec**: 0x01 Opus, 0x02 PCM L16, 0x03 PCM L24.
- **Sample Rate**: in hertz. It MUST be 48000 for Opus.
- **Channel Count**: 1 to 8. The order of channels is that of channel mapping
  family 1 of [RFC7845]: mono; left, right; left, center, right; and so on up to
  7.1 (front left, front center, front right, side left, side right, rear left,
  rear right, LFE).
- **Frame Duration**: the audio duration of each packet in microseconds. For
  Opus it MUST be 2500, 5000, 10000 or 20000. For PCM it MUST be between 1000
  and 20000 and a whole number of samples.
- **Opus Mapping**: present only for Opus streams with more than two channels.
  It gives the multistream layout exactly as the channel mapping table of
  [RFC7845], Section 5.1.1: the number of Opus streams, how many of them are
  coupled stereo streams, and, for each output channel, the index of the decoded
  channel that feeds it.

## 7.4 AUDIO_PACKET datagram

```
AUDIO_PACKET Datagram {
  Datagram Type (i) = 0x03,
  Stream ID (i),
  Sequence Number (i),
  Sample Position (i),
  Capture Timestamp (i),
  Payload (..),
}
```

- **Sequence Number**: starts at 1 and increases by 1 for each packet of the
  stream. A packet sent twice keeps its number.
- **Sample Position**: the index, per channel, of the packet's first sample since
  the start of the stream. Gaps in Sample Position mean the sender sent nothing
  for that time, for example during silence.
- **Capture Timestamp**: the sender-clock time at which the first sample was
  captured.
- **Payload**: one encoded frame ([Section 7.5](#75-codecs)).

## 7.5 Codecs

### 7.5.1 Opus

For one or two channels the payload is one Opus packet [RFC6716]. For more
channels it is an Opus multistream packet: the packets of all streams but the
last use the self-delimiting framing of [RFC6716], Appendix B, and the last uses
the normal framing, as in [RFC7845], Section 5.1.1. Senders SHOULD enable Opus
in-band forward error correction and set its expected loss rate from observed
loss.

### 7.5.2 PCM

PCM payloads carry interleaved samples, signed and in network byte order: 16-bit
samples as in L16 [RFC3551], or 24-bit samples as in L24 [RFC3190]. PCM has no
loss concealment of its own; senders MAY send each packet twice, and receivers
drop duplicates by Sequence Number.

## 7.6 Receiving audio

- **Jitter buffer.** Audio cannot stall the way video can, so the receiver keeps
  a small adaptive buffer. A target of 5 to 20 ms suits the networks QDP is
  designed for. The buffer SHOULD shrink when jitter is low.
- **Loss.** A missing packet is concealed with the codec's loss concealment, or
  recovered from in-band forward error correction in the next Opus packet.
- **Clock drift.** The sender's and receiver's audio clocks run at slightly
  different rates. The receiver SHOULD resample gently to keep its buffer near
  the target, rather than dropping or inserting samples audibly.
- **Synchronization with video.** Using the clock offset
  ([Section 5.2](05-timing.md#52-clock-synchronization)), the client can map
  Capture Timestamps of audio and video to its own clock. Video is shown as soon
  as possible and audio after its jitter buffer, so audio normally trails video
  by the buffer depth. A lag of up to about 20 ms is well inside the range viewers
  cannot detect [BT1359]; a client MAY delay video slightly if the lag grows
  larger.

## 7.7 Microphone

The client SHOULD apply acoustic echo cancellation to the microphone signal
before encoding it, using the decoded playback audio as the reference signal.
Only the client has both signals; the host cannot cancel echo picked up in the
client's room.
