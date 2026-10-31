//! The host's display configuration: the monitor it streams, planned from
//! the client's display requests and the captured monitor's size, and
//! replanned as either changes (Sections 6.2, 6.3 and 6.3.1).

use std::collections::VecDeque;
use std::time::Duration;

use qdp::message::{
    Chroma, ClientMonitorFlags, ClientMonitorId, DisplayConfig, DisplayConfigId, DisplayRequest,
    DisplayRequestId, HostMonitor, HostMonitorFlags, MonitorId, StreamId, VideoCodec,
    VideoCodecCapability, VideoLayer, VideoStreamConfig, VideoStreamFlags,
};
use qdp::wire::{SignedVarInt, VarInt};
use qdp_codec::{EncoderSettings, TileCount, frame_rate_within, tile_count};
use tracing::info;

use crate::RuntimeError;
use crate::host::{BIT_DEPTH, EncoderTuning};
use crate::layout::VideoLayout;
use crate::media::DesktopScale;
use crate::stream_ids::StreamIds;

/// The Monitor ID of the host's one monitor.
const MONITOR_ID: MonitorId = MonitorId(VarInt::from_u32(1));

/// The H.273 colour primaries of standard dynamic range streams (BT.709).
const SDR_PRIMARIES: u8 = 1;

/// The H.273 transfer characteristics of standard dynamic range streams (sRGB).
const SDR_TRANSFER: u8 = 13;

/// The H.273 matrix coefficients of BT.709 YUV streams.
const BT709_MATRIX: u8 = 1;

/// The AV1 profile of the streams this host sends: profile 0, 4:2:0.
const AV1_PROFILE: u8 = 0;

/// The millihertz in one hertz.
const MILLIHERTZ_PER_HERTZ: u32 = 1000;

/// The Config ID of the first `DISPLAY_CONFIG`.
const FIRST_CONFIG_ID: DisplayConfigId = DisplayConfigId(VarInt::from_u32(1));

/// How many configurations' layouts the host keeps, to map positions the
/// client computed before it processed the newest one (Section 8.7, rule 6).
const KEPT_LAYOUTS: usize = 4;

/// What limits the host's video, whatever the client asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VideoLimits {
    /// The most frames per second the host sends.
    pub(crate) frame_rate: u32,
    /// What the client decodes, if the session negotiated it.
    pub(crate) client: Option<VideoCodecCapability>,
}

/// What a new configuration did to the motion stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum StreamChange {
    /// The stream continues.
    Kept,
    /// A new stream, with a new Stream ID, replaces it (Section 6.3.1).
    Replaced,
}

/// The host's display configuration.
#[derive(Debug)]
pub(crate) struct HostDisplay {
    /// What limits the video.
    limits: VideoLimits,
    /// The client's latest display request, if it sent one.
    request: Option<DisplayRequest>,
    /// The current plan.
    plan: VideoPlan,
    /// The current configuration.
    config: DisplayConfig,
    /// The captured monitor's interface scale.
    scale: DesktopScale,
    /// The layouts of the latest configurations, oldest first.
    layouts: VecDeque<(DisplayConfigId, VideoLayout)>,
}

impl HostDisplay {
    /// Plans the first configuration, for a `capture`-sized monitor at
    /// interface scale `scale` and the client's display request, if it sent
    /// one.
    pub(crate) fn new(
        capture: (usize, usize),
        scale: DesktopScale,
        request: Option<DisplayRequest>,
        limits: VideoLimits,
        stream_ids: &mut StreamIds,
    ) -> Result<Self, RuntimeError> {
        let plan = VideoPlan::new(
            capture,
            request.as_ref(),
            limits.frame_rate,
            limits.client.as_ref(),
        )?;
        let request_id = request
            .as_ref()
            .map_or(DisplayRequestId::NONE, |request| request.request_id);
        let config = display_config(&plan, scale, FIRST_CONFIG_ID, request_id, stream_ids.next());
        Ok(Self {
            limits,
            request,
            plan,
            layouts: VecDeque::from([(config.config_id, plan.layout)]),
            config,
            scale,
        })
    }

    /// Returns the current configuration.
    pub(crate) fn config(&self) -> &DisplayConfig {
        &self.config
    }

    /// Returns the current plan.
    pub(crate) fn plan(&self) -> &VideoPlan {
        &self.plan
    }

    /// Returns the motion stream's ID.
    pub(crate) fn stream_id(&self) -> StreamId {
        self.config.streams[0].stream_id
    }

    /// Returns the current layout.
    pub(crate) fn layout(&self) -> VideoLayout {
        self.plan.layout
    }

    /// Returns the layout of the configuration `config_id`, if the host
    /// still keeps it.
    pub(crate) fn layout_for(&self, config_id: DisplayConfigId) -> Option<VideoLayout> {
        self.layouts
            .iter()
            .find(|(id, _)| *id == config_id)
            .map(|(_, layout)| *layout)
    }

    /// Answers a `DISPLAY_REQUEST` with a new configuration, which
    /// [`HostDisplay::config`] then returns, even if nothing changed
    /// (Section 6.3).
    pub(crate) fn handle_request(
        &mut self,
        request: DisplayRequest,
        stream_ids: &mut StreamIds,
    ) -> Result<StreamChange, RuntimeError> {
        let request_id = request.request_id;
        self.request = Some(request);
        self.replan(self.plan.layout.capture, request_id, stream_ids)
    }

    /// Follows the captured monitor to a new size with a configuration the
    /// host makes on its own, answering no request.
    pub(crate) fn capture_resized(
        &mut self,
        capture: (usize, usize),
        stream_ids: &mut StreamIds,
    ) -> Result<StreamChange, RuntimeError> {
        info!(
            width = capture.0,
            height = capture.1,
            "the captured monitor changed size"
        );
        self.replan(capture, DisplayRequestId::NONE, stream_ids)
    }

    /// Plans the next configuration.
    fn replan(
        &mut self,
        capture: (usize, usize),
        request_id: DisplayRequestId,
        stream_ids: &mut StreamIds,
    ) -> Result<StreamChange, RuntimeError> {
        let plan = VideoPlan::new(
            capture,
            self.request.as_ref(),
            self.limits.frame_rate,
            self.limits.client.as_ref(),
        )?;
        let (change, stream_id) = if plan.needs_new_stream(&self.plan) {
            (StreamChange::Replaced, stream_ids.next())
        } else {
            (StreamChange::Kept, self.stream_id())
        };
        let next_id = self.config.config_id.0.into_inner() + 1;
        let config_id = DisplayConfigId(VarInt::new(next_id).unwrap_or(VarInt::MAX));
        self.config = display_config(&plan, self.scale, config_id, request_id, stream_id);
        self.plan = plan;
        self.layouts.push_back((config_id, plan.layout));
        while self.layouts.len() > KEPT_LAYOUTS {
            self.layouts.pop_front();
        }
        if change == StreamChange::Replaced {
            info!(
                stream = stream_id.0.into_inner(),
                width = plan.layout.video.0,
                height = plan.layout.video.1,
                refresh_millihertz = plan.refresh_millihertz,
                "new video stream"
            );
        }
        Ok(change)
    }
}

/// What the host streams of one monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VideoPlan {
    /// The sizes of the capture and of its video.
    pub(crate) layout: VideoLayout,
    /// The most motion frames per second, in millihertz.
    pub(crate) refresh_millihertz: u32,
    /// The client display area the monitor is meant for, or 0 if none.
    client_monitor_id: ClientMonitorId,
}

impl VideoPlan {
    /// Plans the video of a `capture`-sized monitor for the client's
    /// display request: no larger than the client's display area or the
    /// largest frame it decodes, no more frames per second than the area's
    /// refresh rate or `frame_rate` (Sections 6.2 and 6.3), and within the
    /// client's Max Level (Section 6.4.1, rule 8).
    fn new(
        capture: (usize, usize),
        request: Option<&DisplayRequest>,
        frame_rate: u32,
        client: Option<&VideoCodecCapability>,
    ) -> Result<Self, RuntimeError> {
        let area = request.and_then(|request| {
            request
                .monitors
                .iter()
                .find(|monitor| monitor.flags.contains(ClientMonitorFlags::PRIMARY))
                .or(request.monitors.first())
        });
        let side = |value: VarInt| usize::try_from(value.into_inner()).unwrap_or(usize::MAX);
        let bounds: Vec<(usize, usize)> = client
            .map(|client| (side(client.max_width), side(client.max_height)))
            .into_iter()
            .chain(area.map(|area| (side(area.width), side(area.height))))
            .collect();
        let layout = VideoLayout::fit(capture, &bounds);
        let mut refresh_millihertz = frame_rate.max(1).saturating_mul(MILLIHERTZ_PER_HERTZ);
        if let Some(requested) = area
            .and_then(|area| u32::try_from(area.refresh_rate.into_inner()).ok())
            .filter(|&requested| requested > 0)
        {
            refresh_millihertz = refresh_millihertz.min(requested);
        }
        let mut plan = Self {
            layout,
            refresh_millihertz,
            client_monitor_id: area.map(|area| area.id).unwrap_or_default(),
        };
        if let Some(client) = client {
            plan.fit_level(client.max_level)?;
        }
        Ok(plan)
    }

    /// Lowers the frame rate until the stream stays within `max_level`.
    fn fit_level(&mut self, max_level: u8) -> Result<(), RuntimeError> {
        let (width, height) = self.layout.video;
        let tiles = TileCount::at_most_columns(tile_count(width, height) as u32);
        let frame_rate = self.frame_rate();
        let fitted = frame_rate_within(width as u32, height as u32, frame_rate, tiles, max_level)
            .ok_or(RuntimeError::Config(
            "the video is too large for the client's AV1 level",
        ))?;
        if fitted < frame_rate {
            info!(
                frame_rate = fitted,
                max_level, "frame rate lowered to the client's AV1 level"
            );
            self.refresh_millihertz = fitted * MILLIHERTZ_PER_HERTZ;
        }
        Ok(())
    }

    /// Returns whether a stream of this plan cannot continue as one of
    /// `other`'s: its frames have another size or rate, so its encoder and
    /// sequence header differ (Sections 6.3.1 and 6.4.1).
    fn needs_new_stream(&self, other: &Self) -> bool {
        self.layout.video != other.layout.video || self.frame_rate() != other.frame_rate()
    }

    /// Returns the frames per second, rounded up, as encoders and levels
    /// count them.
    fn frame_rate(&self) -> u32 {
        self.refresh_millihertz.div_ceil(MILLIHERTZ_PER_HERTZ)
    }

    /// Returns the time between frames.
    pub(crate) fn frame_interval(&self) -> Duration {
        Duration::from_secs(MILLIHERTZ_PER_HERTZ.into()) / self.refresh_millihertz.max(1)
    }

    /// Returns the encoder settings of the video.
    pub(crate) fn encoder_settings(&self, tuning: &EncoderTuning) -> EncoderSettings {
        EncoderSettings {
            width: self.layout.video.0,
            height: self.layout.video.1,
            frame_rate: self.frame_rate(),
            speed: tuning.speed,
            quantizer: tuning.quantizer,
            threads: tuning.threads,
        }
    }
}

/// Returns the configuration `config_id` of one monitor at interface scale
/// `scale` with one AV1 motion stream, `stream_id`, as `plan` describes it,
/// answering the requests up to `request_id`.
fn display_config(
    plan: &VideoPlan,
    scale: DesktopScale,
    config_id: DisplayConfigId,
    request_id: DisplayRequestId,
    stream_id: StreamId,
) -> DisplayConfig {
    let (width, height) = plan.layout.video;
    DisplayConfig {
        config_id,
        request_id,
        monitors: vec![HostMonitor {
            id: MONITOR_ID,
            client_monitor_id: plan.client_monitor_id,
            flags: HostMonitorFlags::PRIMARY,
            x: SignedVarInt::ZERO,
            y: SignedVarInt::ZERO,
            width: varint(width),
            height: varint(height),
            refresh_rate: VarInt::from_u32(plan.refresh_millihertz),
            scale: VarInt::from_u32(scale.percent()),
        }],
        streams: vec![VideoStreamConfig {
            stream_id,
            monitor_id: MONITOR_ID,
            layer: VideoLayer::Motion,
            codec: VideoCodec::AV1,
            profile: AV1_PROFILE,
            bit_depth: BIT_DEPTH,
            chroma: Chroma::Yuv420,
            flags: VideoStreamFlags::default(),
            color_primaries: SDR_PRIMARIES,
            transfer_characteristics: SDR_TRANSFER,
            matrix_coefficients: BT709_MATRIX,
        }],
    }
}

/// Returns `value` as a variable-length integer, saturating.
fn varint(value: usize) -> VarInt {
    VarInt::try_from(value).unwrap_or(VarInt::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::SCALE_PERCENT;
    use qdp::message::{Av1Profiles, ClientMonitor, NO_LEVEL_LIMIT, VideoCodecFlags};

    /// Returns a request for one display area.
    fn request(width: u32, height: u32, refresh_millihertz: u32) -> DisplayRequest {
        DisplayRequest {
            request_id: DisplayRequestId(VarInt::from_u32(3)),
            monitors: vec![ClientMonitor {
                id: ClientMonitorId(VarInt::from_u32(7)),
                flags: ClientMonitorFlags::PRIMARY,
                x: SignedVarInt::ZERO,
                y: SignedVarInt::ZERO,
                width: VarInt::from_u32(width),
                height: VarInt::from_u32(height),
                refresh_rate: VarInt::from_u32(refresh_millihertz),
                scale: VarInt::from_u32(SCALE_PERCENT),
                physical_width: VarInt::ZERO,
                physical_height: VarInt::ZERO,
                peak_luminance: VarInt::ZERO,
            }],
        }
    }

    /// Returns a client that decodes frames up to `max_side` at `max_level`.
    fn client(max_side: u32, max_level: u8) -> VideoCodecCapability {
        VideoCodecCapability {
            codec: VideoCodec::AV1,
            profiles: Av1Profiles::MAIN,
            max_bit_depth: BIT_DEPTH,
            max_width: VarInt::from_u32(max_side),
            max_height: VarInt::from_u32(max_side),
            max_pixel_rate: VarInt::ZERO,
            flags: VideoCodecFlags::default(),
            max_level,
        }
    }

    /// The video fits the client's display area and decoder, at no more
    /// frames than the area refreshes, for the area's Client Monitor ID.
    #[test]
    fn plans_video_for_the_request() {
        let uhd = (3840, 2160);
        let area = request(1920, 1200, 59_940);
        let plan =
            VideoPlan::new(uhd, Some(&area), 60, Some(&client(8192, NO_LEVEL_LIMIT))).unwrap();
        assert_eq!(plan.layout.video, (1920, 1080));
        assert_eq!(plan.refresh_millihertz, 59_940);
        assert_eq!(plan.frame_rate(), 60);
        assert_eq!(plan.frame_interval(), Duration::from_nanos(16_683_350));
        assert_eq!(plan.client_monitor_id, ClientMonitorId(VarInt::from_u32(7)));
        let config = display_config(
            &plan,
            DesktopScale::from_widths(3840, 2560).unwrap(),
            FIRST_CONFIG_ID,
            area.request_id,
            StreamId(VarInt::from_u32(1)),
        );
        assert_eq!(config.monitors[0].width, VarInt::from_u32(1920));
        assert_eq!(config.monitors[0].scale, VarInt::from_u32(150));
        assert_eq!(config.monitors[0].client_monitor_id, plan.client_monitor_id);

        let small_decoder =
            VideoPlan::new(uhd, Some(&area), 30, Some(&client(1280, NO_LEVEL_LIMIT))).unwrap();
        assert_eq!(small_decoder.layout.video, (1280, 720));
        assert_eq!(small_decoder.refresh_millihertz, 30_000);

        let unknown_rate = VideoPlan::new(uhd, Some(&request(3840, 2160, 0)), 60, None).unwrap();
        assert_eq!(unknown_rate.layout.video, uhd);
        assert_eq!(unknown_rate.refresh_millihertz, 60_000);
    }

    /// The frame rate drops to fit the client's Max Level, and video too
    /// large for it at any rate is refused.
    #[test]
    fn plans_within_the_level() {
        let hd = (1920, 1080);
        let plan = VideoPlan::new(hd, None, 60, Some(&client(8192, 8))).unwrap();
        assert_eq!(plan.refresh_millihertz, 34_000);
        assert!(VideoPlan::new(hd, None, 60, Some(&client(8192, 5))).is_err());
    }

    /// Requests that change the video's size or rate replace the stream;
    /// others keep it. Every configuration has a new Config ID, and recent
    /// layouts stay known.
    #[test]
    fn replans_on_requests_and_captures() {
        let mut ids = StreamIds::default();
        let limits = VideoLimits {
            frame_rate: 60,
            client: Some(client(8192, NO_LEVEL_LIMIT)),
        };
        let mut display = HostDisplay::new(
            (3840, 2160),
            DesktopScale::UNSCALED,
            Some(request(1920, 1080, 60_000)),
            limits,
            &mut ids,
        )
        .unwrap();
        let first = display.stream_id();
        assert_eq!(display.config().config_id, FIRST_CONFIG_ID);
        assert_eq!(display.layout().video, (1920, 1080));

        let same = request(1920, 1080, 60_000);
        assert_eq!(
            display.handle_request(same, &mut ids).unwrap(),
            StreamChange::Kept
        );
        assert_eq!(display.stream_id(), first);
        assert_eq!(display.config().config_id.0.into_inner(), 2);
        assert_eq!(display.config().request_id.0.into_inner(), 3);

        let smaller = request(1280, 720, 60_000);
        assert_eq!(
            display.handle_request(smaller, &mut ids).unwrap(),
            StreamChange::Replaced
        );
        assert_ne!(display.stream_id(), first);
        assert_eq!(display.layout().video, (1280, 720));
        let old = display.layout_for(FIRST_CONFIG_ID).unwrap();
        assert_eq!(old.video, (1920, 1080));

        // The monitor shrinks: the video keeps its size, the layout changes.
        let stream = display.stream_id();
        assert_eq!(
            display.capture_resized((2560, 1440), &mut ids).unwrap(),
            StreamChange::Kept
        );
        assert_eq!(display.stream_id(), stream);
        assert_eq!(display.layout().capture, (2560, 1440));
        assert_eq!(display.config().request_id, DisplayRequestId::NONE);

        for _ in 0..KEPT_LAYOUTS {
            display.capture_resized((2560, 1440), &mut ids).unwrap();
        }
        assert!(display.layout_for(FIRST_CONFIG_ID).is_none());
    }
}
