//! Identifier newtypes (Section 1.5.7).
//!
//! Identifiers are variable-length integers chosen by the endpoint that defines the
//! object. The value 0 is reserved to mean "none" and never identifies an object.

use crate::wire::VarInt;

/// A video or audio stream. Video and audio Stream IDs share one space.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StreamId(pub VarInt);

/// A host monitor, defined by `DISPLAY_CONFIG` (Section 6.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonitorId(pub VarInt);

/// A client display area, defined by `DISPLAY_REQUEST` (Section 6.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClientMonitorId(pub VarInt);

impl ClientMonitorId {
    /// No client display area.
    pub const NONE: Self = Self(VarInt::ZERO);
}

/// A `DISPLAY_REQUEST`, numbered from 1 (Section 6.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DisplayRequestId(pub VarInt);

impl DisplayRequestId {
    /// A configuration that answers no request.
    pub const NONE: Self = Self(VarInt::ZERO);
}

/// A `DISPLAY_CONFIG`, numbered from 1 (Section 6.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DisplayConfigId(pub VarInt);

/// An `AUDIO_CONFIG`, numbered from 1 (Section 7.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AudioConfigId(pub VarInt);

/// The number of a frame within a video stream, from 1 (Section 6.6.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameNumber(pub VarInt);

impl FrameNumber {
    /// No frame: the Last Decoded Frame of a stream that has decoded none.
    pub const NONE: Self = Self(VarInt::ZERO);

    /// The first frame of every stream.
    pub const FIRST: Self = Self(VarInt::from_u32(1));
}

/// An input device attached by the client, numbered from 1 (Section 8.2.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(pub VarInt);

/// The sequence number of an input report, shared by all devices and numbered from
/// 1 (Section 8.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InputSequence(pub VarInt);

impl InputSequence {
    /// No report: the acknowledgement before any report has been applied.
    pub const NONE: Self = Self(VarInt::ZERO);
}

/// A `CURSOR_WARP`, numbered from 1 (Section 9.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WarpId(pub VarInt);

impl WarpId {
    /// No warp processed yet.
    pub const NONE: Self = Self(VarInt::ZERO);
}

/// A `HID_FEATURE_REQUEST`, numbered from 1 (Section 8.8.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FeatureRequestId(pub VarInt);
