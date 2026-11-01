//! Capturing the console: whatever the graphics card scans out to a
//! display, read from its framebuffers through DRM (see [`device`]).
//!
//! The ScreenCast portal exists only inside a desktop session. This sees
//! what a person at the machine sees, whatever shows it: the login screen,
//! the lock screen, any desktop, the text console. It follows the display
//! as one hands it to the next, as the login screen does to the desktop
//! someone logs into. The kernel shows another program's framebuffers only
//! to root, so this is for a host that runs as a system service.
//!
//! Frames stay in GPU memory, for a hardware encoder that reads them there,
//! and the pointer, which the card shows on a plane of its own, is read
//! apart from them (Section 9.1).

mod device;

use std::fs::File;
use std::io;
use std::os::fd::AsFd;
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use qdp_codec::{
    DRM_FORMAT_MOD_LINEAR, DmaBufFrame, DmaBufLayout, DmaBufPlane, PixelDepth, Rgb32Order,
};
use qdp_runtime::{
    CapturedFrame, CursorShape, CursorState, FrameArrivals, FrameSource, PointerPosition,
    RuntimeError,
};
use tracing::{debug, info, warn};

use self::device::{
    Card, ConnectorName, ExportedFramebuffer, MODIFIER_IMPLICIT, PlaneAccess, PlaneKind,
};
use super::dma_buf_map::{MappedDmaBuf, PIXEL_BYTES, read_linear};
use crate::FrameMemory;

/// Returns the DRM four-character code `code`, as `fourcc_code` builds it.
const fn fourcc(code: [u8; 4]) -> u32 {
    u32::from_le_bytes(code)
}

/// `DRM_FORMAT_XRGB8888`: blue, green, red and padding bytes.
const FORMAT_XRGB8888: u32 = fourcc(*b"XR24");
/// `DRM_FORMAT_ARGB8888`: blue, green, red and alpha bytes.
const FORMAT_ARGB8888: u32 = fourcc(*b"AR24");
/// `DRM_FORMAT_XBGR8888`: red, green, blue and padding bytes.
const FORMAT_XBGR8888: u32 = fourcc(*b"XB24");
/// `DRM_FORMAT_ABGR8888`: red, green, blue and alpha bytes.
const FORMAT_ABGR8888: u32 = fourcc(*b"AB24");
/// `DRM_FORMAT_XRGB2101010`: blue, green and red in ten bits each, then
/// padding.
const FORMAT_XRGB2101010: u32 = fourcc(*b"XR30");
/// `DRM_FORMAT_ARGB2101010`: blue, green and red in ten bits each, then
/// alpha.
const FORMAT_ARGB2101010: u32 = fourcc(*b"AR30");
/// `DRM_FORMAT_XBGR2101010`: red, green and blue in ten bits each, then
/// padding.
const FORMAT_XBGR2101010: u32 = fourcc(*b"XB30");
/// `DRM_FORMAT_ABGR2101010`: red, green and blue in ten bits each, then
/// alpha, as desktops showing deeper colour scan out.
const FORMAT_ABGR2101010: u32 = fourcc(*b"AB30");

/// The name of the thread that watches the display refresh.
const WATCHER_THREAD: &str = "qdp-console";

/// How long the watcher waits before looking again at a display that does
/// not refresh, as one that is off or changing hands does not.
const IDLE_RETRY: Duration = Duration::from_millis(100);

/// How long a capture waits for a display to show a picture, when there is
/// no earlier frame to stand in, before giving up.
const DISPLAY_WAIT: Duration = Duration::from_secs(10);

/// How often a capture that waits for a display looks again.
const DISPLAY_POLL: Duration = Duration::from_millis(50);

/// How long a pointer image read from its buffer is kept before it is read
/// again, for compositors that draw a new image into the same buffer.
const POINTER_REFRESH: Duration = Duration::from_secs(1);

/// The largest value of a byte.
const BYTE_MAX: u32 = 255;

/// Why frames in memory cannot be had from the console.
const NEEDS_GPU_ENCODER: &str = "the display's picture is in a tiled GPU buffer, which only a \
hardware encoder can read; console capture needs one (VA-API)";

/// The display being captured: its connector, the CRTC driving it, and the
/// planes it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Target {
    /// The connector the display is on.
    connector: u32,
    /// The connector's name, for the log.
    name: ConnectorName,
    /// The CRTC driving it.
    crtc: u32,
    /// The CRTC's index among the card's, which names it to vertical blank
    /// requests and in planes' possible CRTCs.
    crtc_index: u32,
    /// Where the CRTC's picture starts in its framebuffer.
    origin: (u32, u32),
    /// Its primary plane, or `None` if the card shows only the CRTC's
    /// framebuffer.
    primary: Option<u32>,
    /// Its cursor plane, if the card shows it.
    cursor: Option<CursorPlane>,
}

/// A CRTC's cursor plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CursorPlane {
    /// The plane.
    id: u32,
    /// Its `CRTC_X` and `CRTC_Y` properties, which say where it is, if the
    /// card shows atomic properties.
    position: Option<(u32, u32)>,
}

/// What a display shows at a moment: what the watcher compares between
/// refreshes to tell whether anything changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shown {
    /// The framebuffer of its picture, or 0.
    picture: u32,
    /// The pointer's framebuffer and position, if it shows.
    pointer: Option<(u32, (i64, i64))>,
}

/// The pointer's image, as last read from its buffer.
#[derive(Debug)]
struct PointerImage {
    /// The buffer it was read from: its device and inode.
    buffer: (u64, u64),
    /// When it was read.
    read_at: Instant,
    /// The image, cut down to its visible pixels.
    shape: Arc<CursorShape>,
    /// Where the visible pixels start in the buffer.
    offset: (i64, i64),
}

/// Captures the console: the picture the graphics card shows on a display.
pub struct ConsoleCapture {
    /// The card.
    card: Arc<Card>,
    /// How much of the planes it shows.
    access: PlaneAccess,
    /// The display captured, which the watcher may move to another CRTC.
    target: Arc<Mutex<Target>>,
    /// Where frames are delivered.
    frames: FrameMemory,
    /// The last frame, and the buffer it came from.
    last: Option<((u64, u64), CapturedFrame)>,
    /// The size of the last frame.
    size: (usize, usize),
    /// The pointer's image.
    pointer: Option<PointerImage>,
    /// The thread that tells the host about new frames, once asked.
    watcher: Option<Watcher>,
}

impl ConsoleCapture {
    /// Starts capturing the first display that shows a picture, on the
    /// first card that has one, delivering frames as `frames` says. Blocks
    /// until the first frame is read.
    pub fn open(frames: FrameMemory) -> Result<Self, String> {
        let cards = Card::open_all();
        if cards.is_empty() {
            return Err("no graphics card can be opened in /dev/dri".to_owned());
        }
        let mut reasons = Vec::new();
        for card in cards {
            let access = card.show_planes();
            let target = match find_target(&card, access, None) {
                Ok(Some(target)) => target,
                Ok(None) => {
                    reasons.push(format!("{}: no display is on", card.path().display()));
                    continue;
                }
                Err(error) => {
                    reasons.push(format!("{}: {error}", card.path().display()));
                    continue;
                }
            };
            info!(
                card = %card.path().display(),
                display = %target.name,
                ?access,
                "capturing the console"
            );
            let mut capture = Self {
                card: Arc::new(card),
                access,
                target: Arc::new(Mutex::new(target)),
                frames,
                last: None,
                size: (0, 0),
                pointer: None,
                watcher: None,
            };
            capture.capture().map_err(|error| error.to_string())?;
            return Ok(capture);
        }
        Err(format!(
            "no display shows a picture ({}); console capture streams what a monitor shows, so \
             one must be connected and on",
            reasons.join("; ")
        ))
    }

    /// Returns the display captured now.
    fn target(&self) -> Target {
        *lock(&self.target)
    }

    /// Reads framebuffer `id` as a frame, or returns the last frame again if
    /// it is the same buffer.
    fn read(&mut self, id: u32) -> Result<CapturedFrame, RuntimeError> {
        let exported = self
            .card
            .export_framebuffer(id)
            .map_err(|error| RuntimeError::Capture(error.to_string()))?;
        let ReadFrame { frame, buffer } = dma_buf_frame(exported)?;
        if let Some((last_buffer, last)) = &self.last {
            if *last_buffer == buffer {
                return Ok(last.clone());
            }
        }
        let captured = match self.frames {
            FrameMemory::Gpu => CapturedFrame::DmaBuf(Arc::new(frame)),
            FrameMemory::System => {
                let frame = read_linear(&frame)
                    .ok_or_else(|| RuntimeError::Capture(NEEDS_GPU_ENCODER.to_owned()))?;
                CapturedFrame::Memory(Arc::new(frame))
            }
        };
        self.size = captured.size();
        self.last = Some((buffer, captured.clone()));
        Ok(captured)
    }

    /// Returns the pointer's image in framebuffer `id`, reading it again if
    /// the buffer changed or the image is old, or the stock arrow if it
    /// cannot be read.
    fn pointer_image(&mut self, id: u32) -> (Arc<CursorShape>, (i64, i64)) {
        let exported = match self.card.export_framebuffer(id) {
            Ok(exported) => exported,
            Err(error) => {
                debug!(%error, "cannot read the pointer's image");
                return (Arc::new(CursorShape::arrow()), (0, 0));
            }
        };
        let buffer = buffer_identity(&exported).unwrap_or_default();
        if let Some(image) = &self.pointer {
            if image.buffer == buffer && image.read_at.elapsed() < POINTER_REFRESH {
                return (Arc::clone(&image.shape), image.offset);
            }
        }
        let (shape, offset) = read_pointer(&exported)
            .map(|(shape, (x, y))| (shape, (i64::from(x), i64::from(y))))
            .unwrap_or_else(|| (CursorShape::arrow(), (0, 0)));
        // A shape equal to the last is shared, so that the host does not
        // send it again.
        let shape = match &self.pointer {
            Some(image) if *image.shape == shape => Arc::clone(&image.shape),
            _ => Arc::new(shape),
        };
        self.pointer = Some(PointerImage {
            buffer,
            read_at: Instant::now(),
            shape: Arc::clone(&shape),
            offset,
        });
        (shape, offset)
    }
}

impl FrameSource for ConsoleCapture {
    fn size(&self) -> (usize, usize) {
        self.size
    }

    /// Reads the picture the display shows now. While it shows none, as
    /// while the login screen hands it to a desktop, the last frame stands
    /// in, and the display is looked for again, on whatever CRTC now
    /// drives it.
    fn capture(&mut self) -> Result<CapturedFrame, RuntimeError> {
        let deadline = Instant::now() + DISPLAY_WAIT;
        loop {
            let target = self.target();
            let picture = picture(&self.card, &target).unwrap_or(0);
            if picture != 0 {
                return self.read(picture);
            }
            find_again(&self.card, self.access, &self.target);
            if let Some((_, last)) = &self.last {
                return Ok(last.clone());
            }
            if Instant::now() >= deadline {
                return Err(RuntimeError::Capture(
                    "the display shows no picture".to_owned(),
                ));
            }
            std::thread::sleep(DISPLAY_POLL);
        }
    }

    /// Frames from the console that are not in linear buffers can reach
    /// memory only through a hardware encoder, so captures fail from now on
    /// unless they are.
    fn use_memory_frames(&mut self) {
        if self.frames == FrameMemory::System {
            return;
        }
        self.frames = FrameMemory::System;
        warn!("the encoder cannot read the console's GPU buffers; reading them in memory");
        self.last = None;
    }

    /// Reads the cursor plane: where it is and the image it shows. Without
    /// a cursor plane, the pointer is in the frames, if the desktop draws
    /// it.
    fn cursor(&mut self) -> Option<CursorState> {
        let target = self.target();
        let plane = target.cursor?;
        let Some((framebuffer, position)) = pointer_on(&self.card, &target, plane) else {
            return Some(CursorState::default());
        };
        let (shape, (offset_x, offset_y)) = self.pointer_image(framebuffer);
        let position = match position {
            Some((x, y)) => {
                let x = i64::from(target.origin.0) + x + offset_x;
                let y = i64::from(target.origin.1) + y + offset_y;
                PointerPosition::At(
                    i32::try_from(x).unwrap_or(i32::MAX),
                    i32::try_from(y).unwrap_or(i32::MAX),
                )
            }
            None => PointerPosition::Unknown,
        };
        Some(CursorState {
            position,
            shape: Some(shape),
        })
    }

    /// Watches the display refresh, telling the host each time its picture
    /// or pointer changed.
    fn watch_frames(&mut self, arrivals: Arc<FrameArrivals>) -> bool {
        let stop = Arc::new(AtomicBool::new(false));
        let watching = Watching {
            card: Arc::clone(&self.card),
            access: self.access,
            target: Arc::clone(&self.target),
            arrivals,
            stop: Arc::clone(&stop),
        };
        let thread = std::thread::Builder::new()
            .name(WATCHER_THREAD.to_owned())
            .spawn(move || watching.run());
        match thread {
            Ok(thread) => {
                self.watcher = Some(Watcher {
                    stop,
                    thread: Some(thread),
                });
                true
            }
            Err(error) => {
                warn!(%error, "cannot watch the display; capturing on a timer");
                false
            }
        }
    }
}

/// The watcher's thread, stopped and joined when the capture ends.
#[derive(Debug)]
struct Watcher {
    /// Asks the thread to stop.
    stop: Arc<AtomicBool>,
    /// The thread.
    thread: Option<JoinHandle<()>>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the watcher's thread needs.
struct Watching {
    /// The card.
    card: Arc<Card>,
    /// How much of the planes it shows.
    access: PlaneAccess,
    /// The display captured.
    target: Arc<Mutex<Target>>,
    /// Where new frames are told.
    arrivals: Arc<FrameArrivals>,
    /// Set when the capture ends.
    stop: Arc<AtomicBool>,
}

impl Watching {
    /// Waits for each refresh of the display, and tells of a new frame when
    /// what it shows changed. A display that stops refreshing is looked for
    /// again, as it is when another CRTC takes it over.
    fn run(self) {
        let mut last = None;
        while !self.stop.load(Ordering::Relaxed) {
            let target = *lock(&self.target);
            if self.card.wait_for_refresh(target.crtc_index).is_err() {
                if find_again(&self.card, self.access, &self.target) {
                    self.arrivals.arrived();
                }
                std::thread::sleep(IDLE_RETRY);
                continue;
            }
            let shown = shown(&self.card, &target).ok();
            if shown != last {
                last = shown;
                self.arrivals.arrived();
            }
        }
    }
}

/// Returns the lock's value, even if a thread panicked holding it.
fn lock(target: &Mutex<Target>) -> MutexGuard<'_, Target> {
    target.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Looks for the captured display again, as when it stops showing a
/// picture, on whatever CRTC drives it now, or another display if it is
/// gone. Returns whether the target changed.
fn find_again(card: &Card, access: PlaneAccess, target: &Mutex<Target>) -> bool {
    let current = *lock(target);
    match find_target(card, access, Some(current.connector)) {
        Ok(Some(found)) if found != current => {
            info!(display = %found.name, "the console moved; following it");
            *lock(target) = found;
            true
        }
        Ok(_) => false,
        Err(error) => {
            debug!(%error, "cannot look for the display again");
            false
        }
    }
}

/// Finds a display that shows a picture: the one on connector `preferred`
/// if it still does, otherwise the first.
fn find_target(
    card: &Card,
    access: PlaneAccess,
    preferred: Option<u32>,
) -> io::Result<Option<Target>> {
    let (crtcs, connectors) = card.resources()?;
    let mut found = None;
    for id in connectors {
        let Ok(connector) = card.connector(id) else {
            continue;
        };
        if !connector.connected || connector.encoder == 0 {
            continue;
        }
        let Ok(crtc_id) = card.encoder_crtc(connector.encoder) else {
            continue;
        };
        let Some(index) = crtcs.iter().position(|&crtc| crtc == crtc_id) else {
            continue;
        };
        let Ok(crtc) = card.crtc(crtc_id) else {
            continue;
        };
        if crtc.size.is_none() {
            continue;
        }
        let candidate = (id, connector.name, crtc_id, index as u32, crtc.origin);
        if Some(id) == preferred {
            found = Some(candidate);
            break;
        }
        found.get_or_insert(candidate);
    }
    let Some((connector, name, crtc, crtc_index, origin)) = found else {
        return Ok(None);
    };
    let (primary, cursor) = match access {
        PlaneAccess::Legacy => (None, None),
        PlaneAccess::Atomic | PlaneAccess::Universal => planes_of(card, access, crtc, crtc_index)?,
    };
    Ok(Some(Target {
        connector,
        name,
        crtc,
        crtc_index,
        origin,
        primary,
        cursor,
    }))
}

/// Returns the primary and cursor planes of the CRTC `crtc` at `index`:
/// those that can be shown on it, preferring those shown on it now.
fn planes_of(
    card: &Card,
    access: PlaneAccess,
    crtc: u32,
    index: u32,
) -> io::Result<(Option<u32>, Option<CursorPlane>)> {
    let bit = 1u32 << index;
    let mut primary = None;
    let mut cursor = None;
    for id in card.planes()? {
        let Ok(plane) = card.plane(id) else {
            continue;
        };
        if plane.possible_crtcs & bit == 0 {
            continue;
        }
        let shown_here = plane.crtc == crtc;
        match card.plane_kind(id)? {
            Some(PlaneKind::Primary) if primary.is_none() || shown_here => primary = Some(id),
            Some(PlaneKind::Cursor) if cursor.is_none() || shown_here => {
                let position = if access == PlaneAccess::Atomic {
                    card.find_property(id, "CRTC_X")?
                        .zip(card.find_property(id, "CRTC_Y")?)
                } else {
                    None
                };
                cursor = Some(CursorPlane { id, position });
            }
            _ => {}
        }
    }
    Ok((primary, cursor))
}

/// Returns the framebuffer of the display's picture, or 0 if it shows none
/// on the CRTC captured.
fn picture(card: &Card, target: &Target) -> io::Result<u32> {
    match target.primary {
        Some(id) => {
            let plane = card.plane(id)?;
            Ok(if plane.crtc == target.crtc {
                plane.framebuffer
            } else {
                0
            })
        }
        None => Ok(card.crtc(target.crtc)?.framebuffer),
    }
}

/// Returns the pointer's framebuffer and, if the card tells it, where the
/// plane is on the CRTC, or `None` if the pointer does not show there.
fn pointer_on(
    card: &Card,
    target: &Target,
    plane: CursorPlane,
) -> Option<(u32, Option<(i64, i64)>)> {
    let state = card.plane(plane.id).ok()?;
    if state.crtc != target.crtc || state.framebuffer == 0 {
        return None;
    }
    let position = plane.position.and_then(|(x_property, y_property)| {
        let properties = card.plane_properties(plane.id).ok()?;
        let value = |wanted| {
            properties
                .iter()
                .find(|(property, _)| *property == wanted)
                // The positions are signed: a pointer at the left or top
                // edge starts before it.
                .map(|&(_, value)| value as i64)
        };
        Some((value(x_property)?, value(y_property)?))
    });
    Some((state.framebuffer, position))
}

/// Returns what the display shows now.
fn shown(card: &Card, target: &Target) -> io::Result<Shown> {
    let pointer = target.cursor.and_then(|plane| {
        let (framebuffer, position) = pointer_on(card, target, plane)?;
        Some((framebuffer, position.unwrap_or_default()))
    });
    Ok(Shown {
        picture: picture(card, target)?,
        pointer,
    })
}

/// A frame read from a framebuffer, with the buffer it came from.
struct ReadFrame {
    /// The frame.
    frame: DmaBufFrame,
    /// The buffer's device and inode.
    buffer: (u64, u64),
}

/// Makes a frame of an exported framebuffer, if its pixels are a kind the
/// encoders take.
fn dma_buf_frame(exported: ExportedFramebuffer) -> Result<ReadFrame, RuntimeError> {
    let (order, depth) = pixel_format(exported.format).ok_or_else(|| {
        RuntimeError::Capture(format!(
            "the display's pixel format {} cannot be streamed yet",
            format_name(exported.format)
        ))
    })?;
    let buffer = buffer_identity(&exported).unwrap_or_default();
    let layout = DmaBufLayout {
        width: exported.size.0 as usize,
        height: exported.size.1 as usize,
        order,
        modifier: exported.modifier,
        offset: exported.offset,
        pitch: exported.pitch,
    };
    let extra_planes: Vec<DmaBufPlane> = exported
        .extra_planes
        .iter()
        .map(|&(offset, pitch)| DmaBufPlane { offset, pitch })
        .collect();
    let frame = DmaBufFrame::new(exported.descriptor, layout)
        .map_err(|error| RuntimeError::Capture(error.to_string()))?
        .with_extra_planes(&extra_planes)
        .with_depth(depth);
    Ok(ReadFrame { frame, buffer })
}

/// Returns the device and inode of an exported framebuffer's buffer: the
/// same for every export of one buffer.
fn buffer_identity(exported: &ExportedFramebuffer) -> Option<(u64, u64)> {
    let file = File::from(exported.descriptor.as_fd().try_clone_to_owned().ok()?);
    let metadata = file.metadata().ok()?;
    Some((metadata.dev(), metadata.ino()))
}

/// Returns the order of the colours of a 32-bit pixel format and their
/// depth, if the encoders take it.
fn pixel_format(format: u32) -> Option<(Rgb32Order, PixelDepth)> {
    match format {
        FORMAT_XRGB8888 | FORMAT_ARGB8888 => Some((Rgb32Order::Bgr, PixelDepth::Eight)),
        FORMAT_XBGR8888 | FORMAT_ABGR8888 => Some((Rgb32Order::Rgb, PixelDepth::Eight)),
        FORMAT_XRGB2101010 | FORMAT_ARGB2101010 => Some((Rgb32Order::Bgr, PixelDepth::Ten)),
        FORMAT_XBGR2101010 | FORMAT_ABGR2101010 => Some((Rgb32Order::Rgb, PixelDepth::Ten)),
        _ => None,
    }
}

/// Returns the order of the colour bytes of an 8-bit 32-bit pixel format,
/// as pointer images have, if it is one.
fn pixel_order(format: u32) -> Option<Rgb32Order> {
    match pixel_format(format)? {
        (order, PixelDepth::Eight) => Some(order),
        (_, PixelDepth::Ten) => None,
    }
}

/// Returns a pixel format's four-character code as text.
fn format_name(format: u32) -> String {
    format
        .to_le_bytes()
        .iter()
        .map(|&byte| {
            if byte.is_ascii_graphic() {
                byte as char
            } else {
                '?'
            }
        })
        .collect()
}

/// Reads the pointer's image from its exported framebuffer, if its pixels
/// can be read: cut down to its visible pixels, with where those start.
fn read_pointer(exported: &ExportedFramebuffer) -> Option<(CursorShape, (u32, u32))> {
    let order = pixel_order(exported.format)?;
    if exported.modifier != DRM_FORMAT_MOD_LINEAR && exported.modifier != MODIFIER_IMPLICIT {
        return None;
    }
    let (width, height) = exported.size;
    let length = exported.offset as usize + exported.pitch as usize * height as usize;
    let mapped = MappedDmaBuf::map(exported.descriptor.as_fd(), length)?;
    let pixels = &mapped.bytes()[exported.offset as usize..];
    pointer_shape(pixels, width, height, exported.pitch as usize, order)
}

/// Makes the pointer's image from `pixels`, rows of 32-bit pixels `pitch`
/// bytes apart with premultiplied alpha, as cursor planes show them, cut
/// down to the pixels that show; returns it with where those start.
fn pointer_shape(
    pixels: &[u8],
    width: u32,
    height: u32,
    pitch: usize,
    order: Rgb32Order,
) -> Option<(CursorShape, (u32, u32))> {
    let row_bytes = width as usize * PIXEL_BYTES;
    if width == 0 || height == 0 || pitch < row_bytes {
        return None;
    }
    let rows: Vec<&[u8]> = (0..height as usize)
        .map(|row| pixels.get(row * pitch..row * pitch + row_bytes))
        .collect::<Option<_>>()?;
    let alpha = |x: usize, y: usize| rows[y][x * PIXEL_BYTES + 3];
    let showing = |x: usize, y: usize| alpha(x, y) != 0;
    let columns = 0..width as usize;
    let lines = 0..height as usize;
    let top = lines
        .clone()
        .find(|&y| columns.clone().any(|x| showing(x, y)));
    let Some(top) = top else {
        // Nothing shows: a hidden pointer, as one transparent pixel.
        let shape = CursorShape {
            width: 1,
            height: 1,
            hotspot: (0, 0),
            pixels: vec![0; PIXEL_BYTES],
        };
        return Some((shape, (0, 0)));
    };
    let bottom = lines
        .clone()
        .rev()
        .find(|&y| columns.clone().any(|x| showing(x, y)))?;
    let left = columns
        .clone()
        .find(|&x| lines.clone().any(|y| showing(x, y)))?;
    let right = columns
        .clone()
        .rev()
        .find(|&x| lines.clone().any(|y| showing(x, y)))?;
    let mut rgba = Vec::with_capacity((right - left + 1) * (bottom - top + 1) * PIXEL_BYTES);
    for row in &rows[top..=bottom] {
        for pixel in row[left * PIXEL_BYTES..(right + 1) * PIXEL_BYTES].chunks_exact(PIXEL_BYTES) {
            let (red, green, blue) = match order {
                Rgb32Order::Bgr => (pixel[2], pixel[1], pixel[0]),
                Rgb32Order::Rgb => (pixel[0], pixel[1], pixel[2]),
            };
            let alpha = pixel[3];
            rgba.extend_from_slice(&[
                unpremultiply(red, alpha),
                unpremultiply(green, alpha),
                unpremultiply(blue, alpha),
                alpha,
            ]);
        }
    }
    let shape = CursorShape {
        width: (right - left + 1) as u32,
        height: (bottom - top + 1) as u32,
        hotspot: (0, 0),
        pixels: rgba,
    };
    Some((shape, (left as u32, top as u32)))
}

/// Returns a colour byte premultiplied by `alpha` with the alpha taken out.
fn unpremultiply(colour: u8, alpha: u8) -> u8 {
    if alpha == 0 {
        return 0;
    }
    let alpha = u32::from(alpha);
    ((u32::from(colour) * BYTE_MAX + alpha / 2) / alpha).min(BYTE_MAX) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four-character codes are the kernel's.
    #[test]
    fn formats_are_the_kernels() {
        assert_eq!(FORMAT_XRGB8888, 0x3432_5258);
        assert_eq!(FORMAT_ARGB8888, 0x3432_5241);
        assert_eq!(FORMAT_XBGR8888, 0x3432_4258);
        assert_eq!(FORMAT_ABGR8888, 0x3432_4241);
        assert_eq!(format_name(fourcc(*b"XR30")), "XR30");
        assert_eq!(
            pixel_format(FORMAT_ABGR2101010),
            Some((Rgb32Order::Rgb, PixelDepth::Ten))
        );
        assert_eq!(
            pixel_format(FORMAT_XRGB2101010),
            Some((Rgb32Order::Bgr, PixelDepth::Ten))
        );
        assert_eq!(pixel_order(FORMAT_ABGR2101010), None);
        assert_eq!(pixel_format(fourcc(*b"NV12")), None);
    }

    /// A pointer's image is cut down to the pixels that show, with where
    /// they start, and its colours lose their premultiplied alpha.
    #[test]
    fn cuts_the_pointer_to_what_shows() {
        let (width, height, pitch) = (4u32, 3u32, 20usize);
        let mut pixels = vec![0u8; pitch * height as usize];
        // Blue, green, red, alpha: half-transparent white at (1, 1), and
        // opaque red at (2, 2).
        pixels[pitch + 4..pitch + 8].copy_from_slice(&[128, 128, 128, 128]);
        pixels[2 * pitch + 8..2 * pitch + 12].copy_from_slice(&[0, 0, 255, 255]);
        let (shape, offset) =
            pointer_shape(&pixels, width, height, pitch, Rgb32Order::Bgr).unwrap();
        assert_eq!(offset, (1, 1));
        assert_eq!((shape.width, shape.height), (2, 2));
        assert_eq!(&shape.pixels[0..4], &[255, 255, 255, 128]);
        assert_eq!(&shape.pixels[4..8], &[0, 0, 0, 0]);
        assert_eq!(&shape.pixels[12..16], &[255, 0, 0, 255]);
    }

    /// A pointer that shows nothing is one transparent pixel.
    #[test]
    fn hidden_pointer_is_one_transparent_pixel() {
        let pixels = vec![0u8; 64 * 64 * 4];
        let (shape, offset) = pointer_shape(&pixels, 64, 64, 256, Rgb32Order::Bgr).unwrap();
        assert_eq!((shape.width, shape.height, offset), (1, 1, (0, 0)));
        assert!(shape.is_invisible());
    }

    /// Rows shorter than the image are refused.
    #[test]
    fn refuses_short_rows() {
        assert!(pointer_shape(&[0; 16], 4, 1, 8, Rgb32Order::Bgr).is_none());
        assert!(pointer_shape(&[0; 15], 4, 1, 16, Rgb32Order::Bgr).is_none());
    }

    /// Unpremultiplying restores the colour, and transparent pixels are
    /// black.
    #[test]
    fn unpremultiplies() {
        assert_eq!(unpremultiply(128, 128), 255);
        assert_eq!(unpremultiply(64, 128), 128);
        assert_eq!(unpremultiply(10, 0), 0);
        assert_eq!(unpremultiply(255, 255), 255);
    }
}
