//! What the core tells the app: the session's progress, the video's size,
//! the host's cursor and pointer mode, gamepad rumble and statistics.
//!
//! The app reads events one at a time as lines of text, each a word naming
//! the event followed by its values, separated by spaces:
//!
//! | Line | Meaning |
//! |---|---|
//! | `connecting` | Connecting to the host. |
//! | `connected` | Connected; the session is starting. |
//! | `reconnecting <ms> <error>` | The connection was lost; reconnecting in `<ms>`. |
//! | `video <width> <height>` | The video's size, in pixels, changed. |
//! | `cursor` | The host's cursor changed; `Native.cursorImage` returns it. |
//! | `warp <x> <y> move\|follow` | The host's pointer moved, in video pixels. |
//! | `pointer absolute\|relative` | The host's pointer mode changed. |
//! | `rumble <slot> <low> <high>` | A gamepad's rumble, each motor from 0 to 65535. |
//! | `stats <fps> <latency_ms\|-> <decode_ms> <worst>` | The last second's video. |
//! | `pair first\|retry` | Pairing needs the code the host shows; `retry` after a code that did not work. |
//! | `paired <pin>` | Pairing succeeded with the host whose key pin is `<pin>`, in hexadecimal. |
//! | `ended` | The session ended normally. |
//! | `failed <error>` | The session failed. |
//!
//! A `stats` line's `<worst>` is the slowest frame's latency, then the stages
//! it went through: capture to its first fragment, transfer, waiting for the
//! decoder and decoding, each in milliseconds; or five `-` when no frame's
//! latency is known. The app answers a `pair` line with `Native.pairingCode`.

use std::fmt;
use std::time::Duration;

use qdp::message::{PointerKind, PointerMode};
use qdp_quic::KeyPin;
use qdp_runtime::{LatencyStages, WarpKind};

use crate::settings::format_pin;

/// Something the app is told.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Connecting to the host.
    Connecting,
    /// Connected; the session is starting.
    Connected,
    /// The connection was lost, and is made again after `delay`.
    Reconnecting {
        /// How long until the next attempt.
        delay: Duration,
        /// Why the connection was lost.
        error: String,
    },
    /// The video's size changed.
    Video {
        /// Its width in pixels.
        width: usize,
        /// Its height in pixels.
        height: usize,
    },
    /// The host's cursor changed, to an image or to none.
    Cursor,
    /// The host's pointer moved without the user's input.
    Warp {
        /// Its horizontal position, in video pixels.
        x: f64,
        /// Its vertical position, in video pixels.
        y: f64,
        /// Whether the app moves its pointer, or only draws the cursor there.
        kind: WarpKind,
    },
    /// The host's pointer mode changed: in relative mode the app captures
    /// the pointer, and in absolute mode keeps it in the area the host
    /// confines it to, if any (Section 8.9). Its line names the mode only.
    PointerMode(PointerMode),
    /// A gamepad's rumble changed.
    Rumble {
        /// The gamepad's slot.
        slot: u32,
        /// The low-frequency motor, from 0 off to 65535 full.
        low: u16,
        /// The high-frequency motor, likewise.
        high: u16,
    },
    /// The last second's video.
    Stats(VideoStats),
    /// Pairing needs the code the host shows (Section 4.4.9).
    PairingCode(CodeRequest),
    /// Pairing succeeded with the host whose key this is, which the app
    /// keeps to connect with from then on.
    Paired(KeyPin),
    /// The session ended.
    Ended(Ending),
}

/// Why the app is asked for a pairing code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodeRequest {
    /// The host shows a code for the first time.
    First,
    /// The last code did not work, and the host shows a new one.
    Retry,
}

/// How a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    /// The user stopped it, or the host closed it.
    Normal,
    /// It failed, for this reason.
    Failed(String),
}

/// What the video did over the last second.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoStats {
    /// The frames shown.
    pub frames: u32,
    /// The mean time from capture on the host to decoding, once the clocks
    /// are synchronized.
    pub latency: Option<Duration>,
    /// The mean time decoding took.
    pub decode: Duration,
    /// The slowest frame, of those whose latency is known.
    pub worst: Option<SlowestFrame>,
}

/// The frame of a second whose latency was highest, and where it went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlowestFrame {
    /// Its time from capture to decoding.
    pub latency: Duration,
    /// Where that time went.
    pub stages: LatencyStages,
}

/// The values of a `stats` line's `<worst>` when no frame's latency is known.
const UNKNOWN_WORST: &str = "- - - - -";

/// Returns `duration` in milliseconds, to a tenth.
fn milliseconds(duration: Duration) -> String {
    format!("{:.1}", duration.as_secs_f64() * 1000.0)
}

impl fmt::Display for Event {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connecting => formatter.write_str("connecting"),
            Self::Connected => formatter.write_str("connected"),
            Self::Reconnecting { delay, error } => {
                write!(formatter, "reconnecting {} {error}", delay.as_millis())
            }
            Self::Video { width, height } => write!(formatter, "video {width} {height}"),
            Self::Cursor => formatter.write_str("cursor"),
            Self::Warp { x, y, kind } => {
                let kind = match kind {
                    WarpKind::Move => "move",
                    WarpKind::Follow => "follow",
                };
                write!(formatter, "warp {x} {y} {kind}")
            }
            Self::PointerMode(mode) => formatter.write_str(match mode.mode {
                PointerKind::Absolute => "pointer absolute",
                PointerKind::Relative => "pointer relative",
            }),
            Self::Rumble { slot, low, high } => write!(formatter, "rumble {slot} {low} {high}"),
            Self::Stats(stats) => {
                let worst = stats.worst.map_or_else(
                    || UNKNOWN_WORST.to_owned(),
                    |worst| {
                        let stages = worst.stages;
                        [
                            worst.latency,
                            stages.to_first_fragment,
                            stages.transfer,
                            stages.queued,
                            stages.decode,
                        ]
                        .map(milliseconds)
                        .join(" ")
                    },
                );
                write!(
                    formatter,
                    "stats {} {} {} {worst}",
                    stats.frames,
                    stats.latency.map_or_else(|| "-".to_owned(), milliseconds),
                    milliseconds(stats.decode)
                )
            }
            Self::PairingCode(request) => formatter.write_str(match request {
                CodeRequest::First => "pair first",
                CodeRequest::Retry => "pair retry",
            }),
            Self::Paired(pin) => write!(formatter, "paired {}", format_pin(*pin)),
            Self::Ended(Ending::Normal) => formatter.write_str("ended"),
            Self::Ended(Ending::Failed(error)) => write!(formatter, "failed {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each event is a word followed by its values.
    #[test]
    fn formats_events() {
        let lines = [
            (Event::Connecting, "connecting"),
            (
                Event::Reconnecting {
                    delay: Duration::from_millis(500),
                    error: "timed out".to_owned(),
                },
                "reconnecting 500 timed out",
            ),
            (
                Event::Video {
                    width: 1920,
                    height: 1080,
                },
                "video 1920 1080",
            ),
            (
                Event::Warp {
                    x: 10.5,
                    y: 20.0,
                    kind: WarpKind::Follow,
                },
                "warp 10.5 20 follow",
            ),
            (
                Event::PointerMode(PointerMode {
                    mode: PointerKind::Relative,
                    confinement: None,
                }),
                "pointer relative",
            ),
            (
                Event::Rumble {
                    slot: 1,
                    low: 65535,
                    high: 0,
                },
                "rumble 1 65535 0",
            ),
            (
                Event::Stats(VideoStats {
                    frames: 120,
                    latency: Some(Duration::from_micros(4_250)),
                    decode: Duration::from_micros(1_300),
                    worst: Some(SlowestFrame {
                        latency: Duration::from_micros(18_400),
                        stages: LatencyStages {
                            to_first_fragment: Duration::from_micros(14_100),
                            transfer: Duration::from_micros(1_000),
                            queued: Duration::from_micros(300),
                            decode: Duration::from_micros(3_000),
                        },
                    }),
                }),
                "stats 120 4.2 1.3 18.4 14.1 1.0 0.3 3.0",
            ),
            (
                Event::Stats(VideoStats {
                    frames: 0,
                    latency: None,
                    decode: Duration::ZERO,
                    worst: None,
                }),
                "stats 0 - 0.0 - - - - -",
            ),
            (Event::PairingCode(CodeRequest::First), "pair first"),
            (Event::PairingCode(CodeRequest::Retry), "pair retry"),
            (Event::Ended(Ending::Normal), "ended"),
            (
                Event::Ended(Ending::Failed("closed by peer".to_owned())),
                "failed closed by peer",
            ),
        ];
        for (event, line) in lines {
            assert_eq!(event.to_string(), line);
        }
        assert_eq!(
            Event::Paired(KeyPin([0xab; 32])).to_string(),
            format!("paired {}", "ab".repeat(32))
        );
    }
}
