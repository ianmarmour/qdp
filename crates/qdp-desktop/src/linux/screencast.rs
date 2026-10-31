//! Screen capture on Wayland and X11 desktops through the ScreenCast portal
//! (`org.freedesktop.portal.ScreenCast`), which GNOME, KDE and wlroots
//! compositors implement, with frames delivered by PipeWire.
//!
//! The portal asks the user which monitor to share the first time. The
//! answer is remembered with a restore token, which the caller keeps and
//! passes back, so later sessions start without asking.

use std::os::fd::OwnedFd;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use ashpd::desktop::CreateSessionOptions;
use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{
    CursorMode, Screencast, SelectSourcesOptions, SourceType, StartCastOptions,
};
use pipewire as pw;
use pw::spa;
use qdp_codec::{Rgb32Order, Yuv420Frame};
use qdp_runtime::{FrameSource, RuntimeError};
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

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

/// The newest captured frame, shared between the PipeWire thread and the
/// encoder thread.
#[derive(Debug, Default)]
struct Shared {
    /// The newest frame, or why capture stopped.
    latest: Mutex<Latest>,
    /// Notified when the first frame or an error arrives.
    arrived: Condvar,
}

/// The state of [`Shared`].
#[derive(Debug, Default)]
struct Latest {
    /// The newest frame.
    frame: Option<Yuv420Frame>,
    /// Why capture stopped, if it did.
    error: Option<String>,
}

impl Shared {
    /// Stores a new frame.
    fn store(&self, frame: Yuv420Frame) {
        if let Ok(mut latest) = self.latest.lock() {
            latest.frame = Some(frame);
        }
        self.arrived.notify_all();
    }

    /// Records that capture stopped.
    fn fail(&self, error: String) {
        if let Ok(mut latest) = self.latest.lock() {
            latest.error.get_or_insert(error);
        }
        self.arrived.notify_all();
    }
}

/// What the portal returned for the chosen monitor.
struct PortalStream {
    /// The PipeWire remote to read from.
    remote: OwnedFd,
    /// The PipeWire node of the stream.
    node_id: u32,
    /// The token that restores this choice next time, if any.
    restore_token: Option<String>,
}

/// Captures one monitor of the desktop.
pub struct ScreenCapture {
    /// The monitor's size.
    size: (usize, usize),
    /// The newest frame.
    shared: Arc<Shared>,
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
        capture.wait_for_first_frame()?;
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
        let shared = Arc::new(Shared::default());
        let (stop_pipewire, stop_receiver) = pw::channel::channel();
        let thread_shared = Arc::clone(&shared);
        let pipewire = std::thread::Builder::new()
            .name(PIPEWIRE_THREAD.to_owned())
            .spawn(move || {
                if let Err(error) = run_pipewire(remote, node_id, &thread_shared, stop_receiver) {
                    thread_shared.fail(error);
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            size: (0, 0),
            shared,
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
        let latest = self
            .shared
            .latest
            .lock()
            .map_err(|_| "the capture thread panicked".to_owned())?;
        let (latest, _) = self
            .shared
            .arrived
            .wait_timeout_while(latest, timeout, |latest| {
                latest.frame.is_none() && latest.error.is_none()
            })
            .map_err(|_| "the capture thread panicked".to_owned())?;
        if let Some(error) = &latest.error {
            return Err(error.clone());
        }
        latest
            .frame
            .clone()
            .ok_or_else(|| "no frame arrived from the screen".to_owned())
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
        if (frame.width(), frame.height()) != self.size {
            return Err(RuntimeError::Capture(
                "the monitor's size changed".to_owned(),
            ));
        }
        Ok(frame)
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
    let options = SelectSourcesOptions::default()
        .set_cursor_mode(CursorMode::Embedded)
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

/// Runs a PipeWire stream reading `node_id` until `stop` receives, through
/// `remote` if given, or the user's PipeWire daemon otherwise.
fn run_pipewire(
    remote: Option<OwnedFd>,
    node_id: u32,
    shared: &Arc<Shared>,
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
        .param_changed(|_, format, id, param| {
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
        })
        .process(move |stream, format| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
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
