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
//!
//! When the host is asked to, the captured monitor takes the client's
//! display: its mode and interface scale change for the session.
//!
//! When the encoder reads frames the GPU holds, the stream asks first for
//! DMA-BUFs in the linear layout every GPU reads, and the frames stay where
//! the compositor drew them; a desktop that cannot share them so sends
//! frames in memory instead. If the encoder later cannot read them, the
//! stream asks for frames in memory from then on, and the last DMA-BUF frame
//! is read into memory to stand in until the first one arrives.

use std::os::fd::{BorrowedFd, OwnedFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ashpd::desktop::CreateSessionOptions;
use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{
    CursorMode, Screencast, SelectSourcesOptions, SourceType, StartCastOptions,
};
use pipewire as pw;
use pw::spa;
use pw::spa::buffer::meta::MetaCursor;
use qdp_codec::{DRM_FORMAT_MOD_LINEAR, DmaBufFrame, DmaBufLayout, Rgb32Order, Yuv420Frame};
use qdp_runtime::{
    CapturedFrame, CursorShape, CursorState, DesktopScale, FrameArrivals, FrameSource, LayoutRect,
    MonitorPlacement, PointerPosition, RuntimeError, WantedDisplay,
};
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

use super::desktop_layout;
use super::dma_buf_map::read_linear;
use super::monitor_matching::MonitorMatching;
use crate::FrameMemory;
use crate::latest_frame::LatestFrame;

/// The name of the thread that holds the portal session.
const PORTAL_THREAD: &str = "qdp-portal";

/// The name of the portal runtime's threads.
const PORTAL_RUNTIME_THREAD: &str = "qdp-portal-runtime";

/// The portal runtime's worker threads: its requests are few and small.
const PORTAL_WORKERS: usize = 1;

/// How long the portal takes to share a monitor before the host says it
/// may be asking the user on the host's screen.
const PORTAL_ANSWER_HINT: Duration = Duration::from_secs(5);

/// The name of the thread that runs PipeWire.
const PIPEWIRE_THREAD: &str = "qdp-pipewire";

/// How long to wait for the first frame.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the monitor may take to send frames of its new size after its
/// mode changes.
const MODE_CHANGE_TIMEOUT: Duration = Duration::from_secs(3);

/// How often the newest frame is checked for the new size.
const MODE_CHANGE_POLL: Duration = Duration::from_millis(20);

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

/// The least distance, in pixels, from the monitor's edges within which a
/// pointer the compositor stops reporting is taken to have left for another
/// monitor.
const MIN_EDGE_MARGIN: i32 = 64;

/// The part of the monitor's smaller side, as its divisor, within which a
/// pointer the compositor stops reporting is taken to have left: a quick
/// move crosses that much between two frames.
const EDGE_MARGIN_DIVISOR: i32 = 10;

/// The pointer the stream's metadata describes.
#[derive(Debug, Default)]
struct StreamCursor {
    /// The pointer as the host sees it, once metadata arrived.
    state: Option<CursorState>,
    /// The image put aside while an application hides the pointer, shown
    /// again if it comes back without a new one.
    hidden_shape: Option<Arc<CursorShape>>,
}

/// One buffer's report of the pointer.
#[derive(Debug, Clone)]
struct CursorReport {
    /// The image the buffer carries, if it carries one: `Some(None)` for an
    /// image that shows nothing.
    shape: Option<Option<Arc<CursorShape>>>,
    /// Where the pointer's hotspot is in the frame, in pixels.
    position: (i32, i32),
}

/// The pointer as the PipeWire thread last saw it.
type SharedCursor = Arc<Mutex<StreamCursor>>;

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
    /// The monitor's size in the desktop's own coordinates, if the portal
    /// told it.
    desktop_size: Option<(i32, i32)>,
    /// The monitor's top-left corner in the desktop's coordinates, if the
    /// portal told it.
    desktop_position: Option<(i32, i32)>,
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
    /// The monitor's size in the desktop's own coordinates, if the portal
    /// told it, from which its interface scale follows.
    desktop_size: Option<(i32, i32)>,
    /// Where the monitor is in the desktop, if known.
    placement: Option<MonitorPlacement>,
    /// The monitor's name, as the compositor knows it, if known.
    monitor_name: Option<String>,
    /// Brings the monitor to the client's display, if the host was asked to.
    matching: Option<MonitorMatching>,
    /// Where the stream's frames are delivered.
    frames: FrameMemory,
    /// Has the PipeWire thread ask for frames in memory.
    to_memory: Option<pw::channel::Sender<()>>,
    /// Keeps the session the stream belongs to, other than a portal's, for
    /// as long as the capture, ending it after the stream.
    session: Option<Box<dyn std::any::Any + Send>>,
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
    /// `restore_token` restores an earlier choice, with frames delivered as
    /// `frames` asks. Blocks until the first frame arrives.
    pub fn open(restore_token: Option<String>, frames: FrameMemory) -> Result<Self, String> {
        let (stream, stop_portal, portal) = start_portal(restore_token)?;
        let mut capture = Self::start(Some(stream.remote), stream.node_id, frames)?;
        capture.stop_portal = Some(stop_portal);
        capture.portal = Some(portal);
        capture.restore_token = stream.restore_token;
        capture.desktop_size = stream.desktop_size;
        (capture.monitor_name, capture.placement) =
            portal_monitor(stream.desktop_position, stream.desktop_size);
        if stream.cursor_mode == CursorMode::Hidden {
            if let Ok(mut cursor) = capture.cursor.lock() {
                cursor.state = Some(CursorState {
                    position: PointerPosition::Unknown,
                    shape: Some(Arc::new(CursorShape::arrow())),
                });
            }
        }
        capture.wait_for_first_frame()?;
        info!(
            cursor = ?stream.cursor_mode,
            scale_percent = capture.desktop_scale().map(DesktopScale::percent),
            "screen cast started"
        );
        Ok(capture)
    }

    /// Starts capturing a PipeWire video source of the user's PipeWire
    /// daemon directly, without the portal, such as a virtual display's
    /// stream, with frames delivered as `frames` asks. Blocks until the
    /// first frame arrives.
    pub fn from_pipewire_node(node_id: u32, frames: FrameMemory) -> Result<Self, String> {
        let mut capture = Self::start(None, node_id, frames)?;
        capture.wait_for_first_frame()?;
        Ok(capture)
    }

    /// Starts the PipeWire thread reading `node_id`, through the portal's
    /// `remote` if given, or the user's PipeWire daemon otherwise, with
    /// frames delivered as `frames` asks.
    fn start(remote: Option<OwnedFd>, node_id: u32, frames: FrameMemory) -> Result<Self, String> {
        let shared = Arc::new(LatestFrame::default());
        let cursor = SharedCursor::default();
        let (stop_pipewire, stop_receiver) = pw::channel::channel();
        let (to_memory, to_memory_receiver) = pw::channel::channel();
        let thread_shared = Arc::clone(&shared);
        let thread_cursor = Arc::clone(&cursor);
        let pipewire = std::thread::Builder::new()
            .name(PIPEWIRE_THREAD.to_owned())
            .spawn(move || {
                let channels = PipeWireChannels {
                    stop: stop_receiver,
                    to_memory: to_memory_receiver,
                };
                let result = run_pipewire(
                    remote,
                    node_id,
                    frames,
                    &thread_shared,
                    thread_cursor,
                    channels,
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
            desktop_size: None,
            placement: None,
            monitor_name: None,
            matching: None,
            frames,
            to_memory: Some(to_memory),
            session: None,
        })
    }

    /// Keeps `session`, which the stream belongs to, for as long as the
    /// capture, ending it after the stream.
    pub(super) fn keep_alive(&mut self, session: Box<dyn std::any::Any + Send>) {
        self.session = Some(session);
    }

    /// Has the monitor take the client's display when the host asks it to,
    /// saving its own settings at `saved_path` while they are changed.
    /// Fails if the monitor's name is not known, as the compositor's display
    /// settings need it.
    pub fn match_displays(&mut self, saved_path: PathBuf) -> Result<(), String> {
        let name = self
            .monitor_name
            .clone()
            .ok_or_else(|| "the captured monitor's name is not known".to_owned())?;
        info!(monitor = name, "the monitor will take the client's display");
        self.matching = Some(MonitorMatching::new(name, saved_path)?);
        Ok(())
    }

    /// Waits until the monitor sends frames of `size`, in either orientation,
    /// or [`MODE_CHANGE_TIMEOUT`] passes, and returns whether it did.
    fn wait_for_size(&mut self, size: (usize, usize)) -> bool {
        let deadline = Instant::now() + MODE_CHANGE_TIMEOUT;
        loop {
            if let Ok(frame) = self.wait_for_frame(MODE_CHANGE_POLL) {
                self.size = frame.size();
            }
            if self.size == size || self.size == (size.1, size.0) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(MODE_CHANGE_POLL);
        }
    }

    /// Reads the monitor's size and place in the desktop again, which its
    /// mode and scale change.
    fn refresh_layout(&mut self) {
        let Some(name) = &self.monitor_name else {
            return;
        };
        let outputs = match desktop_layout::outputs() {
            Ok(outputs) => outputs,
            Err(error) => {
                debug!(%error, "the desktop's monitors cannot be listed");
                return;
            }
        };
        let Some(output) = outputs.iter().find(|output| output.name == *name) else {
            return;
        };
        let size = (
            i32::try_from(output.rect.width).unwrap_or(i32::MAX),
            i32::try_from(output.rect.height).unwrap_or(i32::MAX),
        );
        self.desktop_size = Some(size);
        self.placement = desktop_layout::placement(output.rect, &outputs);
    }

    /// Waits for the first frame and takes the monitor's size from it.
    fn wait_for_first_frame(&mut self) -> Result<(), String> {
        let first = self.wait_for_frame(FIRST_FRAME_TIMEOUT)?;
        self.size = first.size();
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
    fn wait_for_frame(&self, timeout: Duration) -> Result<CapturedFrame, String> {
        self.shared.wait(timeout)
    }
}

impl FrameSource for ScreenCapture {
    fn size(&self) -> (usize, usize) {
        self.size
    }

    fn capture(&mut self) -> Result<CapturedFrame, RuntimeError> {
        // The compositor sends frames only when the screen changes, so the
        // newest frame is the current screen.
        let frame = self
            .wait_for_frame(FIRST_FRAME_TIMEOUT)
            .map_err(RuntimeError::Capture)?;
        // A monitor that changes size sends frames of its new size, which
        // the host follows with a new stream.
        self.size = frame.size();
        Ok(frame)
    }

    fn cursor(&mut self) -> Option<CursorState> {
        self.cursor.lock().ok()?.state.clone()
    }

    /// The monitor's pixel width over its width in the desktop's
    /// coordinates, which the portal reports for monitor streams.
    fn desktop_scale(&self) -> Option<DesktopScale> {
        let (width, _) = self.desktop_size?;
        DesktopScale::from_widths(self.size.0, usize::try_from(width).ok()?)
    }

    fn monitor_placement(&self) -> Option<MonitorPlacement> {
        self.placement
    }

    /// PipeWire delivers a frame each time the compositor draws the
    /// monitor, so each is told as it arrives.
    fn use_memory_frames(&mut self) {
        if self.frames == FrameMemory::System {
            return;
        }
        self.frames = FrameMemory::System;
        info!("asking the desktop for the screen in memory instead of in GPU buffers");
        if let Some(to_memory) = &self.to_memory {
            let _ = to_memory.send(());
        }
        // The desktop sends a frame in memory only once the screen changes,
        // so the last frame stands in, read out of its buffer.
        let stand_in = self
            .shared
            .refuse_gpu_frames()
            .and_then(|frame| match frame {
                CapturedFrame::DmaBuf(frame) => read_linear(&frame),
                CapturedFrame::Memory(_) => None,
            });
        match stand_in {
            Some(frame) => self.shared.fill(frame.into()),
            None => debug!("the last GPU frame cannot be read; waiting for one in memory"),
        }
    }

    fn watch_frames(&mut self, arrivals: Arc<FrameArrivals>) -> bool {
        self.shared.watch(arrivals)
    }

    /// Changes the monitor's mode and interface scale to come as close to
    /// `display` as it can, if the host was asked to, and waits for frames
    /// of its new size. Returns whether its size is now the client's.
    fn match_display(&mut self, display: &WantedDisplay) -> bool {
        let Some(matching) = self.matching.as_mut() else {
            return false;
        };
        let settings = match matching.apply(display) {
            Ok(settings) => settings,
            Err(error) => {
                warn!(%error, "the monitor cannot take the client's display");
                return false;
            }
        };
        let size = (settings.mode.width as usize, settings.mode.height as usize);
        if !self.wait_for_size(size) {
            warn!(
                width = size.0,
                height = size.1,
                "the monitor did not send frames of its new size in time"
            );
        }
        self.refresh_layout();
        (display.width as usize, display.height as usize) == size
    }
}

/// Returns the name of the monitor the portal described, at `position` and
/// of `size` in the desktop's coordinates as far as it said, and where it is
/// among the desktop's monitors, as far as they are known. Without its
/// position, the monitor is the one of its size, or the desktop's only one.
fn portal_monitor(
    position: Option<(i32, i32)>,
    size: Option<(i32, i32)>,
) -> (Option<String>, Option<MonitorPlacement>) {
    let outputs = match desktop_layout::outputs() {
        Ok(outputs) => outputs,
        Err(error) => {
            debug!(%error, "the desktop's monitors cannot be listed");
            return (None, None);
        }
    };
    let described = position.zip(size).and_then(|((x, y), (width, height))| {
        Some(LayoutRect {
            x,
            y,
            width: u32::try_from(width).ok()?,
            height: u32::try_from(height).ok()?,
        })
    });
    let of_size = |output: &&desktop_layout::OutputLayout| {
        size.is_some_and(|(width, height)| {
            (i64::from(output.rect.width), i64::from(output.rect.height))
                == (i64::from(width), i64::from(height))
        })
    };
    let output = match described {
        Some(rect) => outputs
            .iter()
            .find(|output| (output.rect.x, output.rect.y) == (rect.x, rect.y)),
        None => {
            let sized: Vec<_> = outputs.iter().filter(of_size).collect();
            match (sized.as_slice(), outputs.as_slice()) {
                ([one], _) => Some(*one),
                (_, [only]) => Some(only),
                _ => None,
            }
        }
    };
    let name = output.map(|output| output.name.clone());
    let placement = described
        .or(output.map(|output| output.rect))
        .and_then(|rect| desktop_layout::placement(rect, &outputs));
    info!(monitor = ?name, ?placement, "the captured monitor's place in the desktop");
    (name, placement)
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

/// Returns the runtime the portal's requests run on, for the life of the
/// process.
///
/// ashpd keeps one D-Bus connection for the whole process, and zbus runs the
/// task that reads its replies on the runtime that opened it. A runtime of a
/// session's own ended that task with the session, so that every later
/// session's requests waited for replies forever: a client that reconnected
/// was never shown the desktop.
fn portal_runtime() -> Result<&'static tokio::runtime::Runtime, String> {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    if let Some(runtime) = RUNTIME.get() {
        return Ok(runtime);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(PORTAL_WORKERS)
        .thread_name(PORTAL_RUNTIME_THREAD)
        .enable_all()
        .build()
        .map_err(|error| format!("cannot start the portal's runtime: {error}"))?;
    // Another thread may have made one meanwhile; this one is then dropped.
    Ok(RUNTIME.get_or_init(|| runtime))
}

/// Starts a ScreenCast portal session on a thread of its own, which keeps
/// the session open until told to stop.
fn start_portal(restore_token: Option<String>) -> Result<PortalThread, String> {
    let (started, started_receiver) = std::sync::mpsc::channel();
    let (stop, stop_receiver) = oneshot::channel::<()>();
    let thread = std::thread::Builder::new()
        .name(PORTAL_THREAD.to_owned())
        .spawn(move || {
            let runtime = match portal_runtime() {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = started.send(Err(error));
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
    let stream = match started_receiver.recv_timeout(PORTAL_ANSWER_HINT) {
        Ok(stream) => stream,
        Err(RecvTimeoutError::Timeout) => {
            warn!(
                "the desktop has not shared a monitor yet; it may be asking on the host's \
                 screen which one to share"
            );
            started_receiver
                .recv()
                .map_err(|_| "the portal thread stopped".to_owned())?
        }
        Err(RecvTimeoutError::Disconnected) => return Err("the portal thread stopped".to_owned()),
    }?;
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
        desktop_size: stream.size(),
        desktop_position: stream.position(),
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
    /// The DMA-BUFs' format modifier, if frames come in DMA-BUFs rather
    /// than in memory.
    modifier: Option<u64>,
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

/// Records the pointer a buffer's cursor metadata describes, for a
/// `frame`-sized monitor.
fn update_cursor(shared: &SharedCursor, meta: &MetaCursor, frame: (u32, u32)) {
    // KWin and Mutter mark the metadata invalid while the pointer is not in
    // the stream.
    let report = meta.is_valid().then(|| CursorReport {
        shape: meta.bitmap().map(|bitmap| {
            let hotspot = (meta.hotspot().x, meta.hotspot().y);
            let size = bitmap.size();
            BitmapOrder::of(bitmap.format())
                .and_then(|order| {
                    let data = bitmap.bitmap_data()?;
                    let stride = bitmap.stride().unsigned_abs() as usize;
                    cursor_shape(order, (size.width, size.height), stride, data, hotspot)
                })
                .map(Arc::new)
        }),
        position: (meta.position().x, meta.position().y),
    });
    if let Ok(mut cursor) = shared.lock() {
        note_report(&mut cursor, report, frame);
    }
}

/// Notes one buffer's report of the pointer, or that it reported none, for a
/// `frame`-sized monitor.
fn note_report(cursor: &mut StreamCursor, report: Option<CursorReport>, frame: (u32, u32)) {
    let Some(report) = report else {
        // No pointer in the stream: it left the monitor, or an application
        // hid it, which KWin reports the same way, since a pointer without
        // an image covers none of the monitor. Only one last seen near an
        // edge can have left.
        let Some(state) = cursor.state.as_mut() else {
            return;
        };
        if let PointerPosition::At(x, y) = state.position {
            if near_edge((x, y), frame) {
                state.position = PointerPosition::Hidden;
            } else if let Some(shape) = state.shape.take() {
                cursor.hidden_shape = Some(shape);
            }
        }
        return;
    };
    let hidden_shape = cursor.hidden_shape.take();
    let state = cursor.state.get_or_insert_with(CursorState::default);
    match report.shape {
        Some(shape) => state.shape = shape,
        None => {
            if state.shape.is_none() {
                state.shape = hidden_shape;
            }
        }
    }
    state.position = PointerPosition::At(report.position.0, report.position.1);
}

/// Returns whether `position` is near enough an edge of a `frame`-sized
/// monitor for a pointer last seen there to have left it.
fn near_edge((x, y): (i32, i32), (width, height): (u32, u32)) -> bool {
    let width = i32::try_from(width).unwrap_or(i32::MAX);
    let height = i32::try_from(height).unwrap_or(i32::MAX);
    let margin = (width.min(height) / EDGE_MARGIN_DIVISOR).max(MIN_EDGE_MARGIN);
    x < margin || y < margin || x >= width - margin || y >= height - margin
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

/// The channels by which the capture tells the PipeWire thread what to do.
struct PipeWireChannels {
    /// Stops the thread.
    stop: pw::channel::Receiver<()>,
    /// Asks for frames in memory from then on.
    to_memory: pw::channel::Receiver<()>,
}

/// Runs a PipeWire stream reading `node_id` until told to stop, through
/// `remote` if given, or the user's PipeWire daemon otherwise, asking for
/// frames as `frames` says.
fn run_pipewire(
    remote: Option<OwnedFd>,
    node_id: u32,
    frames: FrameMemory,
    shared: &Arc<LatestFrame>,
    cursor: SharedCursor,
    channels: PipeWireChannels,
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
    let stream = pw::stream::StreamRc::new(
        core,
        "qdp-screen",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|error| error.to_string())?;

    let frame_shared = Arc::clone(shared);
    // Whether the stream's buffers are DMA-BUFs, for the request for frames
    // in memory, which changes nothing otherwise.
    let in_dma_bufs = Arc::new(AtomicBool::new(false));
    let format_in_dma_bufs = Arc::clone(&in_dma_bufs);
    let _listener = stream
        .add_local_listener_with_user_data(StreamFormat::default())
        .param_changed(move |stream, format, id, param| {
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
            // A format with a modifier was negotiated for DMA-BUFs.
            let dma_buf = param.as_object().is_ok_and(|object| {
                object
                    .find_prop(spa::utils::Id(spa::sys::SPA_FORMAT_VIDEO_modifier))
                    .is_some()
            });
            *format = StreamFormat {
                width: info.size().width,
                height: info.size().height,
                order,
                modifier: dma_buf.then(|| info.modifier()),
            };
            format_in_dma_bufs.store(dma_buf, Ordering::Release);
            match format.modifier {
                Some(modifier) => info!(
                    width = format.width,
                    height = format.height,
                    modifier = format_args!("{modifier:#x}"),
                    "the desktop shares the screen in GPU buffers"
                ),
                None => debug!(?format, "screen stream format"),
            }
            // Ask for buffers of the negotiated kind, and for the pointer as
            // metadata, which sources that draw it into their frames ignore.
            let params: Vec<Vec<u8>> = [buffers_param(dma_buf), cursor_meta_param()]
                .into_iter()
                .flatten()
                .collect();
            let mut pods: Vec<&spa::pod::Pod> = params
                .iter()
                .filter_map(|bytes| spa::pod::Pod::from_bytes(bytes))
                .collect();
            if let Err(error) = stream.update_params(&mut pods) {
                debug!(%error, "cannot ask for buffers and cursor metadata");
            }
        })
        .process(move |stream, format| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            if let Some(meta) = buffer.find_meta::<MetaCursor>() {
                update_cursor(&cursor, meta, (format.width, format.height));
            }
            let Some(order) = format.order else {
                return;
            };
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else {
                return;
            };
            let chunk = data.chunk();
            // Buffers that only move the pointer are marked corrupted, or
            // left empty: their contents are not a frame.
            if chunk.flags().contains(spa::buffer::ChunkFlags::CORRUPTED) || chunk.size() == 0 {
                return;
            }
            if data.type_() == spa::buffer::DataType::DmaBuf {
                match dma_buf_frame(data, format) {
                    Some(frame) => frame_shared.store(CapturedFrame::DmaBuf(Arc::new(frame))),
                    None => debug!("unusable screen DMA-BUF skipped"),
                }
                return;
            }
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
                Some(frame) => frame_shared.store(frame.into()),
                None => debug!(size, stride, "incomplete screen buffer skipped"),
            }
        })
        .register()
        .map_err(|error| error.to_string())?;

    let formats = match frames {
        FrameMemory::Gpu => vec![
            video_format(BufferKind::DmaBuf)?,
            video_format(BufferKind::Memory)?,
        ],
        FrameMemory::System => vec![video_format(BufferKind::Memory)?],
    };
    let mut pods = formats
        .iter()
        .map(|bytes| {
            spa::pod::Pod::from_bytes(bytes)
                .ok_or_else(|| "cannot describe the stream format".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut pods,
        )
        .map_err(|error| format!("cannot connect to the screen stream: {error}"))?;

    let quit_loop = main_loop.clone();
    let _stop = channels
        .stop
        .attach(main_loop.loop_(), move |()| quit_loop.quit());
    let weak_stream = stream.downgrade();
    let _to_memory = channels.to_memory.attach(main_loop.loop_(), move |()| {
        let Some(stream) = weak_stream.upgrade() else {
            return;
        };
        if !in_dma_bufs.load(Ordering::Acquire) {
            // The desktop shares the screen in memory already, and would do
            // so again whenever it renegotiates, as it could not otherwise.
            return;
        }
        // Offering only formats in memory has the desktop renegotiate.
        let format = match video_format(BufferKind::Memory) {
            Ok(format) => format,
            Err(error) => {
                warn!(%error, "cannot ask for the screen in memory");
                return;
            }
        };
        let Some(pod) = spa::pod::Pod::from_bytes(&format) else {
            return;
        };
        if let Err(error) = stream.update_params(&mut [pod]) {
            warn!(%error, "cannot ask for the screen in memory");
        }
    });
    main_loop.run();
    Ok(())
}

/// The kind of buffers a stream format is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BufferKind {
    /// DMA-BUFs in the linear layout.
    DmaBuf,
    /// Memory the processor reads.
    Memory,
}

/// Returns the stream format offered for `kind` of buffers: 32-bit RGB of
/// any size and rate, and for DMA-BUFs the linear layout, which every GPU
/// can read. Compositors choose the size and modifier themselves.
fn video_format(kind: BufferKind) -> Result<Vec<u8>, String> {
    let mut format = spa::pod::object!(
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
    if kind == BufferKind::DmaBuf {
        // The allocating compositor chooses among the modifiers, so the
        // property is not fixated here (PipeWire's DMA-BUF negotiation).
        let linear = DRM_FORMAT_MOD_LINEAR as i64;
        format.properties.push(spa::pod::Property {
            key: spa::sys::SPA_FORMAT_VIDEO_modifier,
            flags: spa::pod::PropertyFlags::MANDATORY | spa::pod::PropertyFlags::DONT_FIXATE,
            value: spa::pod::Value::Choice(spa::pod::ChoiceValue::Long(spa::utils::Choice(
                spa::utils::ChoiceFlags::empty(),
                spa::utils::ChoiceEnum::Enum {
                    default: linear,
                    alternatives: vec![linear],
                },
            ))),
        });
    }
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(format),
    )
    .map(|(bytes, _)| bytes.into_inner())
    .map_err(|error| format!("cannot describe the stream format: {error:?}"))
}

/// Returns the parameter asking for buffers of the kind a format was
/// negotiated for: DMA-BUFs if `dma_buf`, memory otherwise.
fn buffers_param(dma_buf: bool) -> Option<Vec<u8>> {
    let data_types = if dma_buf {
        1 << spa::sys::SPA_DATA_DmaBuf
    } else {
        (1 << spa::sys::SPA_DATA_MemFd) | (1 << spa::sys::SPA_DATA_MemPtr)
    };
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![spa::pod::Property {
            key: spa::sys::SPA_PARAM_BUFFERS_dataType,
            flags: spa::pod::PropertyFlags::empty(),
            value: spa::pod::Value::Choice(spa::pod::ChoiceValue::Int(spa::utils::Choice(
                spa::utils::ChoiceFlags::empty(),
                spa::utils::ChoiceEnum::Flags {
                    default: data_types,
                    flags: Vec::new(),
                },
            ))),
        }],
    };
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .ok()
    .map(|(bytes, _)| bytes.into_inner())
}

/// Returns the frame a dequeued DMA-BUF buffer holds, with a descriptor of
/// its own, or `None` if the buffer is unusable.
fn dma_buf_frame(data: &spa::buffer::Data, format: &StreamFormat) -> Option<DmaBufFrame> {
    let order = format.order?;
    let modifier = format.modifier?;
    let chunk = data.chunk();
    let pitch = u32::try_from(chunk.stride())
        .ok()
        .filter(|&stride| stride > 0)
        .unwrap_or(format.width * PIXEL_BYTES);
    let descriptor = data.fd();
    if descriptor < 0 {
        return None;
    }
    // SAFETY: a dequeued buffer's DMA-BUF descriptor stays open until the
    // buffer is queued again, after this call; it is only duplicated here.
    let descriptor = unsafe { BorrowedFd::borrow_raw(descriptor) }
        .try_clone_to_owned()
        .ok()?;
    let layout = DmaBufLayout {
        width: format.width as usize,
        height: format.height as usize,
        order,
        modifier,
        offset: chunk.offset(),
        pitch,
    };
    DmaBufFrame::new(descriptor, layout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A task a session's portal work spawns, as zbus spawns the one that
    /// reads its shared connection's replies, keeps running for later
    /// sessions, on the one runtime they all share.
    #[test]
    fn keeps_tasks_spawned_by_a_session() {
        let runtime = portal_runtime().unwrap();
        let (requests, mut incoming) = tokio::sync::mpsc::unbounded_channel::<u32>();
        let (replies, answered) = std::sync::mpsc::channel();
        runtime.block_on(async move {
            tokio::spawn(async move {
                while let Some(request) = incoming.recv().await {
                    let _ = replies.send(request);
                }
            });
        });
        // The first session's work is done; a later one is still answered.
        requests.send(7).unwrap();
        assert_eq!(answered.recv_timeout(Duration::from_secs(1)), Ok(7));
        assert!(std::ptr::eq(runtime, portal_runtime().unwrap()));
    }

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

    /// Returns a report of a pointer at `position` without a new image.
    fn at(position: (i32, i32)) -> CursorReport {
        CursorReport {
            shape: None,
            position,
        }
    }

    /// A pointer the compositor stops reporting inside the monitor was hidden
    /// by an application, and comes back with its image; one last seen at an
    /// edge left the monitor.
    #[test]
    fn tells_hidden_pointers_from_departed_ones() {
        let frame = (5120, 2160);
        let arrow = Arc::new(CursorShape::arrow());
        let mut cursor = StreamCursor::default();
        note_report(
            &mut cursor,
            Some(CursorReport {
                shape: Some(Some(Arc::clone(&arrow))),
                position: (2000, 1000),
            }),
            frame,
        );
        // Hidden in the middle: on the monitor, with nothing to show.
        note_report(&mut cursor, None, frame);
        let state = cursor.state.clone().unwrap();
        assert_eq!(state.position, PointerPosition::At(2000, 1000));
        assert!(state.shape.is_none());
        // Shown again without a new image: the old one returns.
        note_report(&mut cursor, Some(at((2010, 1000))), frame);
        let state = cursor.state.clone().unwrap();
        assert_eq!(state.shape.as_deref(), Some(&*arrow));
        // Gone past the right edge: off the monitor, its image kept.
        note_report(&mut cursor, Some(at((5100, 1000))), frame);
        note_report(&mut cursor, None, frame);
        let state = cursor.state.clone().unwrap();
        assert_eq!(state.position, PointerPosition::Hidden);
        assert!(state.shape.is_some());
        // Before any report, nothing is known.
        let mut unknown = StreamCursor::default();
        note_report(&mut unknown, None, frame);
        assert!(unknown.state.is_none());
    }

    /// The edge margin grows with the monitor, from a floor.
    #[test]
    fn measures_edge_margins() {
        assert!(near_edge((100, 1000), (5120, 2160)));
        assert!(!near_edge((300, 1000), (5120, 2160)));
        assert!(near_edge((5000, 1000), (5120, 2160)));
        assert!(near_edge((40, 300), (640, 480)));
        assert!(!near_edge((100, 300), (640, 480)));
        // An unknown size counts as all edge.
        assert!(near_edge((10, 10), (0, 0)));
    }

    /// The metadata parameter serializes.
    #[test]
    fn describes_cursor_metadata() {
        let bytes = cursor_meta_param().unwrap();
        assert!(spa::pod::Pod::from_bytes(&bytes).is_some());
    }
}
