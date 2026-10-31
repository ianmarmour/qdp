//! Screen capture on Wayland and X11 desktops through the ScreenCast portal
//! (`org.freedesktop.portal.ScreenCast`), which GNOME, KDE and wlroots
//! compositors implement, with frames delivered by PipeWire.
//!
//! The portal asks the user which monitor to share the first time. The
//! answer is remembered with a restore token, which the caller keeps and
//! passes back, so later sessions start without asking.
//!
//! Where the desktop supports it, the pointer comes as PipeWire metadata
//! beside the frames rather than drawn into them, so that the client can draw
//! it without latency (Section 9.1).

use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use ashpd::desktop::CreateSessionOptions;
use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{
    CursorMode, Screencast, SelectSourcesOptions, SourceType, StartCastOptions,
};
use pipewire as pw;
use pw::spa;
use pw::spa::buffer::meta::MetaCursor;
use qdp_codec::{Rgb32Order, Yuv420Frame};
use qdp_runtime::{CursorShape, CursorState, FrameSource, PointerPosition, RuntimeError};
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

use crate::latest_frame::LatestFrame;

/// The name of the thread that holds the portal session.
const PORTAL_THREAD: &str = "qdp-portal";

/// The name of the thread that runs PipeWire.
const PIPEWIRE_THREAD: &str = "qdp-pipewire";

/// How long to wait for the first frame.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

/// The largest frame dimension offered to PipeWire.
const MAX_DIMENSION: u32 = 8192;

/// The frame rate offered to PipeWire.
const PREFERRED_FRAME_RATE: u32 = 60;

/// The highest frame rate offered to PipeWire.
const MAX_FRAME_RATE: u32 = 360;

/// The bytes of one 32-bit pixel.
const PIXEL_BYTES: u32 = 4;

/// The pointer image size offered for cursor metadata, in pixels: the
/// preferred, smallest and largest side.
const CURSOR_SIDES: (i32, i32, i32) = (64, 1, 1024);

/// The largest value of an 8-bit channel.
const CHANNEL_MAX: u32 = 255;

/// The pointer as the PipeWire thread last saw it, once metadata arrived.
type SharedCursor = Arc<Mutex<Option<CursorState>>>;

/// What the portal returned for the chosen monitor.
struct PortalStream {
    /// The PipeWire remote to read from.
    remote: OwnedFd,
    /// The PipeWire node of the stream.
    node_id: u32,
    /// The token that restores this choice next time, if any.
    restore_token: Option<String>,
    /// Whether the pointer comes as metadata or drawn into the frames.
    cursor_mode: CursorMode,
}

/// Captures one monitor of the desktop.
pub struct ScreenCapture {
    /// The monitor's size.
    size: (usize, usize),
    /// The newest frame.
    shared: Arc<LatestFrame>,
    /// The pointer, once metadata about it arrived.
    cursor: SharedCursor,
    /// Stops the PipeWire thread.
    stop_pipewire: Option<pw::channel::Sender<()>>,
    /// The PipeWire thread.
    pipewire: Option<JoinHandle<()>>,
    /// Ends the portal session.
    stop_portal: Option<oneshot::Sender<()>>,
    /// The thread holding the portal session.
    portal: Option<JoinHandle<()>>,
    /// The token that restores the user's choice in a later session.
    restore_token: Option<String>,
}

impl std::fmt::Debug for ScreenCapture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScreenCapture")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl ScreenCapture {
    /// Starts capturing a monitor, asking the user which one unless
    /// `restore_token` restores an earlier choice. Blocks until the first
    /// frame arrives.
    pub fn open(restore_token: Option<String>) -> Result<Self, String> {
        let (stream, stop_portal, portal) = start_portal(restore_token)?;
        let mut capture = Self::start(Some(stream.remote), stream.node_id)?;
        capture.stop_portal = Some(stop_portal);
        capture.portal = Some(portal);
        capture.restore_token = stream.restore_token;
        if stream.cursor_mode == CursorMode::Hidden {
            if let Ok(mut cursor) = capture.cursor.lock() {
                *cursor = Some(CursorState {
                    position: PointerPosition::Unknown,
                    shape: Some(Arc::new(CursorShape::arrow())),
                });
            }
        }
        capture.wait_for_first_frame()?;
        info!(cursor = ?stream.cursor_mode, "screen cast started");
        Ok(capture)
    }

    /// Starts capturing a PipeWire video source of the user's PipeWire
    /// daemon directly, without the portal, such as a virtual display's
    /// stream. Blocks until the first frame arrives.
    pub fn from_pipewire_node(node_id: u32) -> Result<Self, String> {
        let mut capture = Self::start(None, node_id)?;
        capture.wait_for_first_frame()?;
        Ok(capture)
    }

    /// Starts the PipeWire thread reading `node_id`, through the portal's
    /// `remote` if given, or the user's PipeWire daemon otherwise.
    fn start(remote: Option<OwnedFd>, node_id: u32) -> Result<Self, String> {
        let shared = Arc::new(LatestFrame::default());
        let cursor = SharedCursor::default();
        let (stop_pipewire, stop_receiver) = pw::channel::channel();
        let thread_shared = Arc::clone(&shared);
        let thread_cursor = Arc::clone(&cursor);
        let pipewire = std::thread::Builder::new()
            .name(PIPEWIRE_THREAD.to_owned())
            .spawn(move || {
                let result = run_pipewire(
                    remote,
                    node_id,
                    &thread_shared,
                    thread_cursor,
                    stop_receiver,
                );
                if let Err(error) = result {
                    thread_shared.fail(error);
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            size: (0, 0),
            shared,
            cursor,
            stop_pipewire: Some(stop_pipewire),
            pipewire: Some(pipewire),
            stop_portal: None,
            portal: None,
            restore_token: None,
        })
    }

    /// Waits for the first frame and takes the monitor's size from it.
    fn wait_for_first_frame(&mut self) -> Result<(), String> {
        let first = self.wait_for_frame(FIRST_FRAME_TIMEOUT)?;
        self.size = (first.width(), first.height());
        info!(
            width = self.size.0,
            height = self.size.1,
            "capturing the screen"
        );
        Ok(())
    }

    /// Returns the token that restores the user's choice of monitor in a
    /// later session, if the portal gave one.
    pub fn restore_token(&self) -> Option<&str> {
        self.restore_token.as_deref()
    }

    /// Waits up to `timeout` for a frame and returns the newest.
    fn wait_for_frame(&self, timeout: Duration) -> Result<Yuv420Frame, String> {
        self.shared.wait(timeout)
    }
}

impl FrameSource for ScreenCapture {
    fn size(&self) -> (usize, usize) {
        self.size
    }

    fn capture(&mut self) -> Result<Yuv420Frame, RuntimeError> {
        // The compositor sends frames only when the screen changes, so the
        // newest frame is the current screen.
        let frame = self
            .wait_for_frame(FIRST_FRAME_TIMEOUT)
            .map_err(RuntimeError::Capture)?;
        // A monitor that changes size sends frames of its new size, which
        // the host follows with a new stream.
        self.size = (frame.width(), frame.height());
        Ok(frame)
    }

    fn cursor(&mut self) -> Option<CursorState> {
        self.cursor.lock().ok()?.clone()
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        if let Some(stop) = self.stop_pipewire.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.pipewire.take() {
            let _ = thread.join();
        }
        if let Some(stop) = self.stop_portal.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.portal.take() {
            let _ = thread.join();
        }
    }
}

/// The portal session's thread and the signal that ends it.
type PortalThread = (PortalStream, oneshot::Sender<()>, JoinHandle<()>);

/// Starts a ScreenCast portal session on a thread of its own, which keeps
/// the session open until told to stop.
fn start_portal(restore_token: Option<String>) -> Result<PortalThread, String> {
    let (started, started_receiver) = std::sync::mpsc::channel();
    let (stop, stop_receiver) = oneshot::channel::<()>();
    let thread = std::thread::Builder::new()
        .name(PORTAL_THREAD.to_owned())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = started.send(Err(error.to_string()));
                    return;
                }
            };
            runtime.block_on(async move {
                let proxy = match Screencast::new().await {
                    Ok(proxy) => proxy,
                    Err(error) => {
                        let _ = started.send(Err(format!("no ScreenCast portal: {error}")));
                        return;
                    }
                };
                let session = match proxy.create_session(CreateSessionOptions::default()).await {
                    Ok(session) => session,
                    Err(error) => {
                        let _ = started.send(Err(format!("cannot create a session: {error}")));
                        return;
                    }
                };
                let result = negotiate(&proxy, &session, restore_token.as_deref()).await;
                let failed = result.is_err();
                let _ = started.send(result);
                if !failed {
                    let _ = stop_receiver.await;
                }
                if let Err(error) = session.close().await {
                    debug!(%error, "closing the portal session failed");
                }
            });
        })
        .map_err(|error| error.to_string())?;
    let stream = started_receiver
        .recv()
        .map_err(|_| "the portal thread stopped".to_owned())??;
    Ok((stream, stop, thread))
}

/// Asks the portal for one monitor and opens its PipeWire remote.
async fn negotiate(
    proxy: &Screencast,
    session: &ashpd::desktop::Session<Screencast>,
    restore_token: Option<&str>,
) -> Result<PortalStream, String> {
    // The host never draws the pointer into its video (Section 9.1): without
    // metadata the pointer is left out, and the client shows a stock arrow.
    let cursor_mode = match proxy.available_cursor_modes().await {
        Ok(modes) if modes.contains(CursorMode::Metadata) => CursorMode::Metadata,
        Ok(modes) if modes.contains(CursorMode::Hidden) => CursorMode::Hidden,
        _ => {
            warn!("the desktop can only draw the pointer into the screen cast");
            CursorMode::Embedded
        }
    };
    let options = SelectSourcesOptions::default()
        .set_cursor_mode(cursor_mode)
        .set_sources(Some(SourceType::Monitor.into()))
        .set_multiple(false)
        .set_persist_mode(PersistMode::ExplicitlyRevoked)
        .set_restore_token(restore_token);
    proxy
        .select_sources(session, options)
        .await
        .and_then(|request| request.response())
        .map_err(|error| format!("selecting a monitor failed: {error}"))?;
    let streams = proxy
        .start(session, None, StartCastOptions::default())
        .await
        .and_then(|request| request.response())
        .map_err(|error| format!("the screen cast was refused: {error}"))?;
    let stream = streams
        .streams()
        .first()
        .ok_or_else(|| "the portal returned no stream".to_owned())?;
    let remote = proxy
        .open_pipe_wire_remote(session, Default::default())
        .await
        .map_err(|error| format!("cannot open the PipeWire remote: {error}"))?;
    Ok(PortalStream {
        remote,
        node_id: stream.pipe_wire_node_id(),
        restore_token: streams.restore_token().map(str::to_owned),
        cursor_mode,
    })
}

/// The negotiated format of the PipeWire stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct StreamFormat {
    /// The frame width.
    width: u32,
    /// The frame height.
    height: u32,
    /// The byte order of the pixels, once a supported format is negotiated.
    order: Option<Rgb32Order>,
}

/// The byte order of a cursor bitmap's pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BitmapOrder {
    /// Red, green, blue, alpha.
    Rgba,
    /// Blue, green, red, alpha.
    Bgra,
    /// Alpha, red, green, blue.
    Argb,
    /// Alpha, blue, green, red.
    Abgr,
}

impl BitmapOrder {
    /// Returns the order of a bitmap format, if it has alpha.
    fn of(format: spa::param::video::VideoFormat) -> Option<Self> {
        use spa::param::video::VideoFormat;
        match format {
            VideoFormat::RGBA => Some(Self::Rgba),
            VideoFormat::BGRA => Some(Self::Bgra),
            VideoFormat::ARGB => Some(Self::Argb),
            VideoFormat::ABGR => Some(Self::Abgr),
            _ => None,
        }
    }

    /// Returns the red, green, blue and alpha of one pixel.
    fn rgba(self, [first, second, third, fourth]: [u8; 4]) -> [u8; 4] {
        match self {
            Self::Rgba => [first, second, third, fourth],
            Self::Bgra => [third, second, first, fourth],
            Self::Argb => [second, third, fourth, first],
            Self::Abgr => [fourth, third, second, first],
        }
    }
}

/// Converts a cursor bitmap, with premultiplied alpha as GNOME and KDE send
/// it, to straight-alpha RGBA. Returns `None` for empty or short bitmaps,
/// which mean the pointer is not shown.
fn cursor_shape(
    order: BitmapOrder,
    (width, height): (u32, u32),
    stride: usize,
    data: &[u8],
    hotspot: (i32, i32),
) -> Option<CursorShape> {
    let row_bytes = width as usize * PIXEL_BYTES as usize;
    if width == 0 || height == 0 || stride < row_bytes {
        return None;
    }
    if data.len() < stride * (height as usize - 1) + row_bytes {
        return None;
    }
    let mut pixels = Vec::with_capacity(row_bytes * height as usize);
    for row in data.chunks(stride).take(height as usize) {
        for pixel in row[..row_bytes].chunks_exact(PIXEL_BYTES as usize) {
            let [red, green, blue, alpha] = order.rgba([pixel[0], pixel[1], pixel[2], pixel[3]]);
            let straight = |channel: u8| {
                if alpha == 0 {
                    0
                } else {
                    (u32::from(channel) * CHANNEL_MAX / u32::from(alpha)).min(CHANNEL_MAX) as u8
                }
            };
            pixels.extend_from_slice(&[straight(red), straight(green), straight(blue), alpha]);
        }
    }
    let clamp = |value: i32, size: u32| u32::try_from(value).unwrap_or(0).min(size - 1);
    Some(CursorShape {
        width,
        height,
        hotspot: (clamp(hotspot.0, width), clamp(hotspot.1, height)),
        pixels,
    })
}

/// Records the pointer a buffer's cursor metadata describes.
fn update_cursor(shared: &SharedCursor, meta: &MetaCursor) {
    if !meta.is_valid() {
        // The buffer carries no new pointer information.
        return;
    }
    let Ok(mut cursor) = shared.lock() else {
        return;
    };
    let state = cursor.get_or_insert_with(CursorState::default);
    if let Some(bitmap) = meta.bitmap() {
        let hotspot = (meta.hotspot().x, meta.hotspot().y);
        let size = bitmap.size();
        let shape = BitmapOrder::of(bitmap.format()).and_then(|order| {
            let data = bitmap.bitmap_data()?;
            let stride = bitmap.stride().unsigned_abs() as usize;
            cursor_shape(order, (size.width, size.height), stride, data, hotspot)
        });
        state.shape = shape.map(Arc::new);
    }
    let position = meta.position();
    state.position = PointerPosition::At(position.x, position.y);
}

/// Returns the parameter asking for cursor metadata on each buffer.
fn cursor_meta_param() -> Option<Vec<u8>> {
    let meta_size = |side: i32| {
        let header =
            size_of::<spa::sys::spa_meta_cursor>() + size_of::<spa::sys::spa_meta_bitmap>();
        header as i32 + side * side * PIXEL_BYTES as i32
    };
    let (preferred, smallest, largest) = CURSOR_SIDES;
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamMeta.as_raw(),
        id: spa::param::ParamType::Meta.as_raw(),
        properties: vec![
            spa::pod::Property {
                key: spa::sys::SPA_PARAM_META_type,
                flags: spa::pod::PropertyFlags::empty(),
                value: spa::pod::Value::Id(spa::utils::Id(spa::sys::SPA_META_Cursor)),
            },
            spa::pod::Property {
                key: spa::sys::SPA_PARAM_META_size,
                flags: spa::pod::PropertyFlags::empty(),
                value: spa::pod::Value::Choice(spa::pod::ChoiceValue::Int(spa::utils::Choice(
                    spa::utils::ChoiceFlags::empty(),
                    spa::utils::ChoiceEnum::Range {
                        default: meta_size(preferred),
                        min: meta_size(smallest),
                        max: meta_size(largest),
                    },
                ))),
            },
        ],
    };
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .ok()
    .map(|(bytes, _)| bytes.into_inner())
}

/// Runs a PipeWire stream reading `node_id` until `stop` receives, through
/// `remote` if given, or the user's PipeWire daemon otherwise.
fn run_pipewire(
    remote: Option<OwnedFd>,
    node_id: u32,
    shared: &Arc<LatestFrame>,
    cursor: SharedCursor,
    stop: pw::channel::Receiver<()>,
) -> Result<(), String> {
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None).map_err(|error| error.to_string())?;
    let context =
        pw::context::ContextRc::new(&main_loop, None).map_err(|error| error.to_string())?;
    let core = match remote {
        Some(remote) => context.connect_fd_rc(remote, None),
        None => context.connect_rc(None),
    }
    .map_err(|error| format!("cannot connect to PipeWire: {error}"))?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "qdp-screen",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|error| error.to_string())?;

    let frame_shared = Arc::clone(shared);
    let _listener = stream
        .add_local_listener_with_user_data(StreamFormat::default())
        .param_changed(|stream, format, id, param| {
            let Some(param) = param else {
                return;
            };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let mut info = spa::param::video::VideoInfoRaw::default();
            if info.parse(param).is_err() {
                return;
            }
            let order = match info.format() {
                spa::param::video::VideoFormat::BGRx | spa::param::video::VideoFormat::BGRA => {
                    Some(Rgb32Order::Bgr)
                }
                spa::param::video::VideoFormat::RGBx | spa::param::video::VideoFormat::RGBA => {
                    Some(Rgb32Order::Rgb)
                }
                other => {
                    warn!(format = ?other, "unsupported screen pixel format");
                    None
                }
            };
            *format = StreamFormat {
                width: info.size().width,
                height: info.size().height,
                order,
            };
            debug!(?format, "screen stream format");
            // Ask for the pointer as metadata, which sources that draw it into
            // their frames ignore.
            let meta = cursor_meta_param();
            match meta.as_deref().and_then(spa::pod::Pod::from_bytes) {
                Some(pod) => {
                    if let Err(error) = stream.update_params(&mut [pod]) {
                        debug!(%error, "cannot ask for cursor metadata");
                    }
                }
                None => debug!("cannot describe the cursor metadata"),
            }
        })
        .process(move |stream, format| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            if let Some(meta) = buffer.find_meta::<MetaCursor>() {
                update_cursor(&cursor, meta);
            }
            let Some(order) = format.order else {
                return;
            };
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else {
                return;
            };
            let chunk = data.chunk();
            let offset = chunk.offset() as usize;
            let size = chunk.size() as usize;
            let stride = match chunk.stride() {
                stride if stride > 0 => stride as usize,
                _ => (format.width * PIXEL_BYTES) as usize,
            };
            let Some(bytes) = data.data() else {
                return;
            };
            let Some(image) = bytes.get(offset..offset + size) else {
                return;
            };
            match Yuv420Frame::from_rgb32(
                format.width as usize,
                format.height as usize,
                stride,
                order,
                image,
            ) {
                Some(frame) => frame_shared.store(frame),
                None => debug!(size, stride, "incomplete screen buffer skipped"),
            }
        })
        .register()
        .map_err(|error| error.to_string())?;

    let format = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::BGRA,
            spa::param::video::VideoFormat::RGBx,
            spa::param::video::VideoFormat::RGBA
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle {
                width: 1920,
                height: 1080
            },
            spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            spa::utils::Rectangle {
                width: MAX_DIMENSION,
                height: MAX_DIMENSION
            }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction {
                num: PREFERRED_FRAME_RATE,
                denom: 1
            },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction {
                num: MAX_FRAME_RATE,
                denom: 1
            }
        ),
    );
    let bytes = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(format),
    )
    .map_err(|error| format!("cannot describe the stream format: {error:?}"))?
    .0
    .into_inner();
    let pod = spa::pod::Pod::from_bytes(&bytes)
        .ok_or_else(|| "cannot describe the stream format".to_owned())?;
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut [pod],
        )
        .map_err(|error| format!("cannot connect to the screen stream: {error}"))?;

    let quit_loop = main_loop.clone();
    let _stop = stop.attach(main_loop.loop_(), move |()| quit_loop.quit());
    main_loop.run();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Premultiplied bitmaps in each byte order become straight RGBA.
    #[test]
    fn converts_cursor_bitmaps() {
        // One opaque red pixel and one half-transparent white one, padded to
        // a 12-byte stride.
        let rgba = [255, 0, 0, 255, 128, 128, 128, 128, 9, 9, 9, 9];
        let shape = cursor_shape(BitmapOrder::Rgba, (2, 1), 12, &rgba, (1, 5)).unwrap();
        assert_eq!(shape.pixels, [255, 0, 0, 255, 255, 255, 255, 128]);
        assert_eq!(shape.hotspot, (1, 0));
        let bgra = [0, 0, 255, 255];
        let shape = cursor_shape(BitmapOrder::Bgra, (1, 1), 4, &bgra, (0, 0)).unwrap();
        assert_eq!(shape.pixels, [255, 0, 0, 255]);
        let argb = [255, 255, 0, 0];
        let shape = cursor_shape(BitmapOrder::Argb, (1, 1), 4, &argb, (0, 0)).unwrap();
        assert_eq!(shape.pixels, [255, 0, 0, 255]);
        let abgr = [255, 0, 0, 255];
        let shape = cursor_shape(BitmapOrder::Abgr, (1, 1), 4, &abgr, (0, 0)).unwrap();
        assert_eq!(shape.pixels, [255, 0, 0, 255]);
    }

    /// Empty and short bitmaps mean no visible pointer.
    #[test]
    fn refuses_empty_bitmaps() {
        assert!(cursor_shape(BitmapOrder::Rgba, (0, 0), 0, &[], (0, 0)).is_none());
        assert!(cursor_shape(BitmapOrder::Rgba, (2, 2), 8, &[0; 12], (0, 0)).is_none());
    }

    /// The metadata parameter serializes.
    #[test]
    fn describes_cursor_metadata() {
        let bytes = cursor_meta_param().unwrap();
        assert!(spa::pod::Pod::from_bytes(&bytes).is_some());
    }
}
