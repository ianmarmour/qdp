//! Negotiation of the features both endpoints support (Sections 4.3 and 4.6).

use std::collections::HashSet;

use crate::message::{
    AudioCodec, Capability, ClientHello, Directions, HdrFormats, HostHello, InputClasses,
    MinorVersion, VideoCodec, VideoCodecCapability, VideoCodecFlags,
};
use crate::registry::ErrorCode;
use crate::session::SessionError;
use crate::wire::VarInt;

/// The number of monitors an endpoint supports when it sends no `MONITORS`
/// capability (Section 4.6.7).
const DEFAULT_MAX_MONITORS: VarInt = VarInt::from_u32(1);

/// The largest clipboard data one direction accepts, or `None` when that direction
/// is not used.
pub type ClipboardLimit = Option<VarInt>;

/// The features a session can use, derived from both hello messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Negotiated {
    /// The lower of the two minor versions.
    pub minor_version: MinorVersion,
    /// What the host can encode in AV1.
    pub host_video: VideoCodecCapability,
    /// What the client can decode in AV1.
    pub client_video: VideoCodecCapability,
    /// Whether both endpoints implement the refinement layer.
    pub refinement: bool,
    /// Whether both endpoints can encode or decode lossless AV1 frames.
    pub lossless: bool,
    /// Codecs the host can send and the client can receive, for playback.
    pub playback_codecs: Vec<AudioCodec>,
    /// Codecs the client can send and the host can receive, for the microphone.
    pub capture_codecs: Vec<AudioCodec>,
    /// The input classes both endpoints set.
    pub input_classes: InputClasses,
    /// The client's Max Size when the host may offer its clipboard to the client.
    pub clipboard_to_client: ClipboardLimit,
    /// The host's Max Size when the client may offer its clipboard to the host.
    pub clipboard_to_host: ClipboardLimit,
    /// The number of monitors the client can display and the host can stream.
    pub max_monitors: VarInt,
    /// The HDR formats the host can encode and the client can present.
    pub hdr_formats: HdrFormats,
}

/// The capabilities of one endpoint, indexed by kind.
#[derive(Debug, Clone, Default)]
struct CapabilitySet {
    /// `VIDEO_CODEC` entries for AV1; entries for unknown codecs are ignored.
    av1: Vec<VideoCodecCapability>,
    /// Whether `REFINEMENT` is present.
    refinement: bool,
    /// `AUDIO_CODEC` entries, as codec and directions.
    audio: Vec<(AudioCodec, Directions)>,
    /// The `INPUT` classes, or none.
    input: InputClasses,
    /// The `CLIPBOARD` directions and Max Size, if present.
    clipboard: Option<(Directions, VarInt)>,
    /// The `MONITORS` value, if present.
    monitors: Option<VarInt>,
    /// The `HDR` formats, or none.
    hdr: HdrFormats,
}

impl CapabilitySet {
    /// Indexes `capabilities`, failing if a non-repeatable capability appears more
    /// than once.
    fn new(capabilities: &[Capability]) -> Result<Self, SessionError> {
        let mut seen = HashSet::new();
        let mut set = Self::default();
        for capability in capabilities {
            let id = capability.id();
            let is_unique =
                !id.is_repeatable() && !matches!(capability, Capability::Unknown { .. });
            if is_unique && !seen.insert(id) {
                return Err(SessionError::new(
                    ErrorCode::PROTOCOL_VIOLATION,
                    format!("capability {id} sent more than once"),
                ));
            }
            match capability {
                Capability::VideoCodec(video) if video.codec == VideoCodec::AV1 => {
                    set.av1.push(*video);
                }
                Capability::VideoCodec(_) | Capability::Unknown { .. } => {}
                Capability::Refinement => set.refinement = true,
                Capability::AudioCodec(audio) => set.audio.push((audio.codec, audio.directions)),
                Capability::Input(classes) => set.input = *classes,
                Capability::Clipboard(clipboard) => {
                    set.clipboard = Some((clipboard.directions, clipboard.max_size));
                }
                Capability::Monitors(max) => set.monitors = Some(*max),
                Capability::Hdr(formats) => set.hdr = *formats,
            }
        }
        Ok(set)
    }

    /// Returns whether the endpoint handles `codec` in `direction`.
    fn audio_supports(&self, codec: AudioCodec, direction: Directions) -> bool {
        self.audio
            .iter()
            .any(|&(candidate, directions)| candidate == codec && directions.contains(direction))
    }

    /// Returns the single AV1 capability, merging repeated entries by taking the
    /// first.
    fn av1(&self) -> Option<VideoCodecCapability> {
        self.av1.first().copied()
    }

    /// Returns the endpoint's monitor limit.
    fn max_monitors(&self) -> VarInt {
        self.monitors.unwrap_or(DEFAULT_MAX_MONITORS)
    }
}

/// Returns the codecs that `sender` can send and `receiver` can receive, in the
/// sender's order.
fn audio_codecs(sender: &CapabilitySet, receiver: &CapabilitySet) -> Vec<AudioCodec> {
    let mut codecs = Vec::new();
    for &(codec, directions) in &sender.audio {
        if directions.contains(Directions::SEND)
            && receiver.audio_supports(codec, Directions::RECEIVE)
            && !codecs.contains(&codec)
        {
            codecs.push(codec);
        }
    }
    codecs
}

/// Returns the Max Size of `receiver` when `sender` may offer its clipboard to it.
fn clipboard_limit(sender: &CapabilitySet, receiver: &CapabilitySet) -> ClipboardLimit {
    let (sender_directions, _) = sender.clipboard?;
    let (receiver_directions, max_size) = receiver.clipboard?;
    (sender_directions.contains(Directions::SEND)
        && receiver_directions.contains(Directions::RECEIVE))
    .then_some(max_size)
}

impl Negotiated {
    /// Negotiates the features of a session from both hello messages.
    ///
    /// Fails with `QDP_PROTOCOL_VIOLATION` if either endpoint repeats a
    /// non-repeatable capability, and with `QDP_NO_COMMON_CAPABILITY` if the
    /// endpoints share no video codec.
    pub fn new(client: &ClientHello, host: &HostHello) -> Result<Self, SessionError> {
        let client_set = CapabilitySet::new(&client.capabilities)?;
        let host_set = CapabilitySet::new(&host.capabilities)?;

        let (Some(host_video), Some(client_video)) = (host_set.av1(), client_set.av1()) else {
            return Err(SessionError::new(
                ErrorCode::NO_COMMON_CAPABILITY,
                "no video codec in common",
            ));
        };
        let refinement = client_set.refinement && host_set.refinement;
        let lossless = host_video.flags.contains(VideoCodecFlags::LOSSLESS)
            && client_video.flags.contains(VideoCodecFlags::LOSSLESS);

        Ok(Self {
            minor_version: client.minor_version.min(host.minor_version),
            host_video,
            client_video,
            refinement,
            lossless,
            playback_codecs: audio_codecs(&host_set, &client_set),
            capture_codecs: audio_codecs(&client_set, &host_set),
            input_classes: client_set.input.intersection(host_set.input),
            clipboard_to_client: clipboard_limit(&host_set, &client_set),
            clipboard_to_host: clipboard_limit(&client_set, &host_set),
            max_monitors: client_set.max_monitors().min(host_set.max_monitors()),
            hdr_formats: HdrFormats(client_set.hdr.0 & host_set.hdr.0),
        })
    }

    /// Returns whether the session can use input `class`.
    pub fn supports_input(&self, class: InputClasses) -> bool {
        self.input_classes.contains(class)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{AudioCodecCapability, Av1Profiles, ClipboardCapability, NO_LEVEL_LIMIT};

    /// Returns an AV1 capability with `flags`.
    fn av1(flags: VideoCodecFlags) -> Capability {
        Capability::VideoCodec(VideoCodecCapability {
            codec: VideoCodec::AV1,
            profiles: Av1Profiles::MAIN,
            max_bit_depth: 10,
            max_width: VarInt::from_u32(3840),
            max_height: VarInt::from_u32(2160),
            max_pixel_rate: VarInt::ZERO,
            flags,
            max_level: NO_LEVEL_LIMIT,
        })
    }

    /// Returns an audio codec capability.
    fn audio(codec: AudioCodec, directions: u8) -> Capability {
        Capability::AudioCodec(AudioCodecCapability {
            codec,
            max_channels: VarInt::from_u32(2),
            directions: Directions(directions),
        })
    }

    /// Returns a client hello with `capabilities`.
    fn client(capabilities: Vec<Capability>) -> ClientHello {
        ClientHello {
            minor_version: MinorVersion(VarInt::from_u32(2)),
            client_name: String::new(),
            capabilities,
        }
    }

    /// Returns a host hello with `capabilities`.
    fn host(capabilities: Vec<Capability>) -> HostHello {
        HostHello {
            minor_version: MinorVersion::CURRENT,
            host_name: String::new(),
            auth_methods: vec![crate::message::AuthMethod::TOKEN],
            capabilities,
        }
    }

    /// Features are used only when both endpoints advertise them.
    #[test]
    fn negotiates_common_features() {
        let send_receive = Directions::SEND.0 | Directions::RECEIVE.0;
        let client = client(vec![
            av1(VideoCodecFlags::LOSSLESS),
            Capability::Refinement,
            audio(AudioCodec::OPUS, send_receive),
            audio(AudioCodec::PCM_L16, Directions::RECEIVE.0),
            Capability::Input(InputClasses::KEYBOARD.union(InputClasses::GAMEPAD)),
            Capability::Clipboard(ClipboardCapability {
                directions: Directions(send_receive),
                max_size: VarInt::from_u32(1 << 20),
            }),
            Capability::Monitors(VarInt::from_u32(3)),
            Capability::Hdr(HdrFormats::BT2020_PQ),
        ]);
        let host = host(vec![
            av1(VideoCodecFlags::HARDWARE_PROFILE_0),
            Capability::Refinement,
            audio(AudioCodec::OPUS, send_receive),
            audio(AudioCodec::PCM_L16, Directions::SEND.0),
            Capability::Input(InputClasses::KEYBOARD.union(InputClasses::MOUSE)),
            Capability::Clipboard(ClipboardCapability {
                directions: Directions::SEND,
                max_size: VarInt::from_u32(8 << 20),
            }),
            Capability::Monitors(VarInt::from_u32(2)),
        ]);

        let negotiated = Negotiated::new(&client, &host).unwrap();
        assert_eq!(negotiated.minor_version, MinorVersion::CURRENT);
        assert!(negotiated.refinement);
        assert!(!negotiated.lossless);
        assert_eq!(
            negotiated.playback_codecs,
            [AudioCodec::OPUS, AudioCodec::PCM_L16]
        );
        assert_eq!(negotiated.capture_codecs, [AudioCodec::OPUS]);
        assert_eq!(negotiated.input_classes, InputClasses::KEYBOARD);
        assert_eq!(
            negotiated.clipboard_to_client,
            Some(VarInt::from_u32(1 << 20))
        );
        assert_eq!(negotiated.clipboard_to_host, None);
        assert_eq!(negotiated.max_monitors, VarInt::from_u32(2));
        assert_eq!(negotiated.hdr_formats, HdrFormats::default());
    }

    /// Without `MONITORS`, an endpoint supports one monitor.
    #[test]
    fn default_monitor_limit() {
        let negotiated = Negotiated::new(
            &client(vec![av1(VideoCodecFlags::default())]),
            &host(vec![
                av1(VideoCodecFlags::default()),
                Capability::Monitors(VarInt::from_u32(4)),
            ]),
        )
        .unwrap();
        assert_eq!(negotiated.max_monitors, VarInt::from_u32(1));
    }

    /// Endpoints without a common video codec cannot form a session.
    #[test]
    fn no_common_video_codec() {
        let error = Negotiated::new(
            &client(Vec::new()),
            &host(vec![av1(VideoCodecFlags::default())]),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::NO_COMMON_CAPABILITY);
    }

    /// A repeated non-repeatable capability is a protocol violation, while
    /// repeated codec entries are allowed.
    #[test]
    fn repeated_capabilities() {
        let error = Negotiated::new(
            &client(vec![
                av1(VideoCodecFlags::default()),
                Capability::Refinement,
                Capability::Refinement,
            ]),
            &host(vec![av1(VideoCodecFlags::default())]),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::PROTOCOL_VIOLATION);

        let repeated_codecs = Negotiated::new(
            &client(vec![
                av1(VideoCodecFlags::default()),
                audio(AudioCodec::OPUS, Directions::RECEIVE.0),
                audio(AudioCodec::PCM_L24, Directions::RECEIVE.0),
            ]),
            &host(vec![av1(VideoCodecFlags::default())]),
        );
        assert!(repeated_codecs.is_ok());
    }
}
