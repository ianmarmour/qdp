//! A graphics card's displays as DRM shows them, through `ioctl` on
//! `/dev/dri/card*`: which connectors have a display, the CRTC driving each,
//! the planes it shows, the framebuffers on them, and when it refreshes.
//! The structures follow the kernel's `drm.h` and `drm_mode.h`.

use std::ffi::c_void;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};

/// The directory of the DRM devices.
const DRI_DIRECTORY: &str = "/dev/dri";

/// The start of every card's node name; render nodes, which cannot see
/// displays, are named otherwise.
const CARD_PREFIX: &str = "card";

/// `DRM_IOCTL_BASE`: the type of every DRM request.
const DRM_IOCTL_TYPE: u32 = b'd' as u32;

/// The `_IOC_WRITE` direction of a request: user space passes data.
const IOC_WRITE: u32 = 1;

/// The `_IOC_READ | _IOC_WRITE` direction of a request: data goes both ways.
const IOC_READ_WRITE: u32 = 3;

/// Returns DRM request `number`, passing `size` bytes in `direction`, as
/// `_IOC` builds it.
const fn request(direction: u32, number: u32, size: usize) -> libc::Ioctl {
    (direction << 30 | (size as u32) << 16 | DRM_IOCTL_TYPE << 8 | number) as libc::Ioctl
}

/// `DRM_IOCTL_SET_CLIENT_CAP`.
const SET_CLIENT_CAP: libc::Ioctl = request(IOC_WRITE, 0x0d, size_of::<ClientCap>());
/// `DRM_IOCTL_GEM_CLOSE`.
const GEM_CLOSE: libc::Ioctl = request(IOC_WRITE, 0x09, size_of::<GemClose>());
/// `DRM_IOCTL_PRIME_HANDLE_TO_FD`.
const PRIME_HANDLE_TO_FD: libc::Ioctl = request(IOC_READ_WRITE, 0x2d, size_of::<PrimeHandle>());
/// `DRM_IOCTL_WAIT_VBLANK`.
const WAIT_VBLANK: libc::Ioctl = request(IOC_READ_WRITE, 0x3a, size_of::<WaitVblank>());
/// `DRM_IOCTL_MODE_GETRESOURCES`.
const GET_RESOURCES: libc::Ioctl = request(IOC_READ_WRITE, 0xa0, size_of::<CardResources>());
/// `DRM_IOCTL_MODE_GETCRTC`.
const GET_CRTC: libc::Ioctl = request(IOC_READ_WRITE, 0xa1, size_of::<RawCrtc>());
/// `DRM_IOCTL_MODE_GETENCODER`.
const GET_ENCODER: libc::Ioctl = request(IOC_READ_WRITE, 0xa6, size_of::<RawEncoder>());
/// `DRM_IOCTL_MODE_GETCONNECTOR`.
const GET_CONNECTOR: libc::Ioctl = request(IOC_READ_WRITE, 0xa7, size_of::<RawConnector>());
/// `DRM_IOCTL_MODE_GETPROPERTY`.
const GET_PROPERTY: libc::Ioctl = request(IOC_READ_WRITE, 0xaa, size_of::<RawProperty>());
/// `DRM_IOCTL_MODE_GETPLANERESOURCES`.
const GET_PLANE_RESOURCES: libc::Ioctl = request(IOC_READ_WRITE, 0xb5, size_of::<PlaneResources>());
/// `DRM_IOCTL_MODE_GETPLANE`.
const GET_PLANE: libc::Ioctl = request(IOC_READ_WRITE, 0xb6, size_of::<RawPlane>());
/// `DRM_IOCTL_MODE_OBJ_GETPROPERTIES`.
const GET_OBJECT_PROPERTIES: libc::Ioctl =
    request(IOC_READ_WRITE, 0xb9, size_of::<ObjectProperties>());
/// `DRM_IOCTL_MODE_GETFB2`.
const GET_FRAMEBUFFER: libc::Ioctl = request(IOC_READ_WRITE, 0xce, size_of::<RawFramebuffer>());

/// `DRM_CLIENT_CAP_UNIVERSAL_PLANES`: show the primary and cursor planes
/// along with the overlays.
const CAP_UNIVERSAL_PLANES: u64 = 2;
/// `DRM_CLIENT_CAP_ATOMIC`: show the planes' atomic properties, such as
/// where a cursor plane is, and the universal planes with them.
const CAP_ATOMIC: u64 = 3;

/// `DRM_MODE_OBJECT_PLANE`: a plane, among the objects with properties.
const OBJECT_PLANE: u32 = 0xeeee_eeee;

/// `DRM_MODE_FB_MODIFIERS`: the framebuffer's modifiers are set.
const FRAMEBUFFER_MODIFIERS: u32 = 1 << 1;

/// `DRM_FORMAT_MOD_INVALID`: a framebuffer made without modifiers, whose
/// layout the driver keeps to itself.
pub(super) const MODIFIER_IMPLICIT: u64 = 0x00ff_ffff_ffff_ffff;

/// `DRM_MODE_CONNECTED`: a display is connected.
const CONNECTED: u32 = 1;

/// `_DRM_VBLANK_RELATIVE`: wait for a number of refreshes from now.
const VBLANK_RELATIVE: u32 = 0x1;
/// `_DRM_VBLANK_SECONDARY`: the second CRTC, as the oldest interface names it.
const VBLANK_SECONDARY: u32 = 0x2000_0000;
/// `_DRM_VBLANK_HIGH_CRTC_SHIFT`: where the index of a third or later CRTC goes.
const VBLANK_HIGH_CRTC_SHIFT: u32 = 1;
/// `_DRM_VBLANK_HIGH_CRTC_MASK`.
const VBLANK_HIGH_CRTC_MASK: u32 = 0x0000_003e;

/// The number of connector modes passed when reading a connector: one,
/// which keeps the kernel from probing the display again, as asking for
/// none would, which is slow and can make the display flicker.
const CONNECTOR_MODE_SLOTS: usize = 1;

/// The value of a plane's `type` property for a primary plane.
const PLANE_TYPE_PRIMARY: u64 = 1;
/// The value of a plane's `type` property for a cursor plane.
const PLANE_TYPE_CURSOR: u64 = 2;

/// The names the kernel gives connector types, by `DRM_MODE_CONNECTOR_*`.
const CONNECTOR_TYPE_NAMES: [&str; 21] = [
    "Unknown",
    "VGA",
    "DVI-I",
    "DVI-D",
    "DVI-A",
    "Composite",
    "SVIDEO",
    "LVDS",
    "Component",
    "DIN",
    "DP",
    "HDMI-A",
    "HDMI-B",
    "TV",
    "eDP",
    "Virtual",
    "DSI",
    "DPI",
    "Writeback",
    "SPI",
    "USB",
];

/// `struct drm_set_client_cap`.
#[repr(C)]
#[derive(Debug, Default)]
struct ClientCap {
    /// `DRM_CLIENT_CAP_*`.
    capability: u64,
    /// 1 to turn it on.
    value: u64,
}

/// `struct drm_gem_close`.
#[repr(C)]
#[derive(Debug, Default)]
struct GemClose {
    /// The handle to close.
    handle: u32,
    /// Unused.
    pad: u32,
}

/// `struct drm_prime_handle`.
#[repr(C)]
#[derive(Debug, Default)]
struct PrimeHandle {
    /// The buffer's handle.
    handle: u32,
    /// The new descriptor's flags.
    flags: u32,
    /// The new descriptor, which the kernel returns.
    fd: i32,
}

/// `union drm_wait_vblank`: the request's type, sequence and signal, which
/// the reply overwrites with the refresh's sequence and time.
#[repr(C)]
#[derive(Debug, Default)]
struct WaitVblank {
    /// `_DRM_VBLANK_*` flags and the CRTC.
    kind: u32,
    /// The refreshes to wait for; the reply's refresh count.
    sequence: u32,
    /// The request's signal, unused; the reply's seconds.
    first: libc::c_long,
    /// The reply's microseconds.
    second: libc::c_long,
}

/// `struct drm_mode_card_res`.
#[repr(C)]
#[derive(Debug, Default)]
struct CardResources {
    /// Where to write the framebuffers.
    fb_id_ptr: u64,
    /// Where to write the CRTCs.
    crtc_id_ptr: u64,
    /// Where to write the connectors.
    connector_id_ptr: u64,
    /// Where to write the encoders.
    encoder_id_ptr: u64,
    /// The framebuffers.
    count_fbs: u32,
    /// The CRTCs.
    count_crtcs: u32,
    /// The connectors.
    count_connectors: u32,
    /// The encoders.
    count_encoders: u32,
    /// The narrowest framebuffer.
    min_width: u32,
    /// The widest framebuffer.
    max_width: u32,
    /// The shortest framebuffer.
    min_height: u32,
    /// The tallest framebuffer.
    max_height: u32,
}

/// `struct drm_mode_modeinfo`.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct ModeInfo {
    /// The pixel clock in kilohertz.
    clock: u32,
    /// The visible width.
    hdisplay: u16,
    /// Where the horizontal sync starts.
    hsync_start: u16,
    /// Where the horizontal sync ends.
    hsync_end: u16,
    /// The whole line.
    htotal: u16,
    /// The horizontal skew.
    hskew: u16,
    /// The visible height.
    vdisplay: u16,
    /// Where the vertical sync starts.
    vsync_start: u16,
    /// Where the vertical sync ends.
    vsync_end: u16,
    /// The whole frame.
    vtotal: u16,
    /// The times each line is scanned.
    vscan: u16,
    /// The refresh rate in hertz.
    vrefresh: u32,
    /// `DRM_MODE_FLAG_*`.
    flags: u32,
    /// `DRM_MODE_TYPE_*`.
    kind: u32,
    /// The mode's name.
    name: [u8; 32],
}

/// `struct drm_mode_crtc`.
#[repr(C)]
#[derive(Debug, Default)]
struct RawCrtc {
    /// Where the connectors to set are, unused when reading.
    set_connectors_ptr: u64,
    /// The connectors to set.
    count_connectors: u32,
    /// The CRTC.
    crtc_id: u32,
    /// The framebuffer it scans out.
    fb_id: u32,
    /// Where its picture starts in the framebuffer, across.
    x: u32,
    /// Where its picture starts in the framebuffer, down.
    y: u32,
    /// The size of its gamma table.
    gamma_size: u32,
    /// Whether it has a mode.
    mode_valid: u32,
    /// Its mode.
    mode: ModeInfo,
}

/// `struct drm_mode_get_encoder`.
#[repr(C)]
#[derive(Debug, Default)]
struct RawEncoder {
    /// The encoder.
    encoder_id: u32,
    /// `DRM_MODE_ENCODER_*`.
    encoder_type: u32,
    /// The CRTC it feeds from.
    crtc_id: u32,
    /// The CRTCs it can feed from.
    possible_crtcs: u32,
    /// The encoders it can clone.
    possible_clones: u32,
}

/// `struct drm_mode_get_connector`.
#[repr(C)]
#[derive(Debug, Default)]
struct RawConnector {
    /// Where to write its encoders.
    encoders_ptr: u64,
    /// Where to write its modes.
    modes_ptr: u64,
    /// Where to write its properties.
    props_ptr: u64,
    /// Where to write its properties' values.
    prop_values_ptr: u64,
    /// Its modes.
    count_modes: u32,
    /// Its properties.
    count_props: u32,
    /// Its encoders.
    count_encoders: u32,
    /// The encoder driving it.
    encoder_id: u32,
    /// The connector.
    connector_id: u32,
    /// `DRM_MODE_CONNECTOR_*`.
    connector_type: u32,
    /// Its number among the card's connectors of its type.
    connector_type_id: u32,
    /// Whether a display is connected.
    connection: u32,
    /// The display's width in millimetres.
    mm_width: u32,
    /// The display's height in millimetres.
    mm_height: u32,
    /// The display's subpixel order.
    subpixel: u32,
    /// Unused.
    pad: u32,
}

/// `struct drm_mode_get_plane_res`.
#[repr(C)]
#[derive(Debug, Default)]
struct PlaneResources {
    /// Where to write the planes.
    plane_id_ptr: u64,
    /// The planes.
    count_planes: u32,
}

/// `struct drm_mode_get_plane`.
#[repr(C)]
#[derive(Debug, Default)]
struct RawPlane {
    /// The plane.
    plane_id: u32,
    /// The CRTC showing it.
    crtc_id: u32,
    /// The framebuffer it shows.
    fb_id: u32,
    /// The CRTCs it can be shown on.
    possible_crtcs: u32,
    /// The size of its gamma table.
    gamma_size: u32,
    /// Its pixel formats.
    count_format_types: u32,
    /// Where to write its pixel formats.
    format_type_ptr: u64,
}

/// `struct drm_mode_obj_get_properties`.
#[repr(C)]
#[derive(Debug, Default)]
struct ObjectProperties {
    /// Where to write the properties.
    props_ptr: u64,
    /// Where to write their values.
    prop_values_ptr: u64,
    /// The properties.
    count_props: u32,
    /// The object.
    obj_id: u32,
    /// `DRM_MODE_OBJECT_*`.
    obj_type: u32,
}

/// `struct drm_mode_get_property`.
#[repr(C)]
#[derive(Debug, Default)]
struct RawProperty {
    /// Where to write its values.
    values_ptr: u64,
    /// Where to write its enumeration's names.
    enum_blob_ptr: u64,
    /// The property.
    prop_id: u32,
    /// `DRM_MODE_PROP_*`.
    flags: u32,
    /// Its name.
    name: [u8; 32],
    /// Its values.
    count_values: u32,
    /// Its enumeration's names.
    count_enum_blobs: u32,
}

/// `struct drm_mode_fb_cmd2`.
#[repr(C)]
#[derive(Debug, Default)]
struct RawFramebuffer {
    /// The framebuffer.
    fb_id: u32,
    /// Its width in pixels.
    width: u32,
    /// Its height in pixels.
    height: u32,
    /// Its pixel format's four-character code.
    pixel_format: u32,
    /// `DRM_MODE_FB_*`.
    flags: u32,
    /// Its planes' buffers.
    handles: [u32; 4],
    /// Its planes' bytes from one row to the next.
    pitches: [u32; 4],
    /// Its planes' first rows.
    offsets: [u32; 4],
    /// Its planes' layouts.
    modifier: [u64; 4],
}

/// How much of the planes a card shows this program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum PlaneAccess {
    /// Every plane and its atomic properties, such as where the cursor is.
    Atomic,
    /// Every plane, but not where a cursor plane is.
    Universal,
    /// Only the overlays: the CRTC's own framebuffer stands for the primary
    /// plane, and the cursor cannot be seen.
    Legacy,
}

/// A connector's name as the kernel gives it, such as `DP-1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ConnectorName {
    /// `DRM_MODE_CONNECTOR_*`.
    kind: u32,
    /// Its number among the card's connectors of its kind.
    index: u32,
}

impl fmt::Display for ConnectorName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = CONNECTOR_TYPE_NAMES
            .get(self.kind as usize)
            .copied()
            .unwrap_or(CONNECTOR_TYPE_NAMES[0]);
        write!(formatter, "{kind}-{}", self.index)
    }
}

/// A connector, as it is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Connector {
    /// Its name.
    pub name: ConnectorName,
    /// Whether a display is connected.
    pub connected: bool,
    /// The encoder driving it, or 0 if none does.
    pub encoder: u32,
}

/// A CRTC, which scans a picture out to its connectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Crtc {
    /// The framebuffer it scans out, as the oldest interface sees it, or 0.
    pub framebuffer: u32,
    /// Where its picture starts in the framebuffer.
    pub origin: (u32, u32),
    /// Its mode's width and height, if it has a mode: if it is on.
    pub size: Option<(u32, u32)>,
}

/// A plane, which a CRTC composes its picture from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Plane {
    /// The CRTC showing it, or 0 if it is off.
    pub crtc: u32,
    /// The framebuffer it shows, or 0.
    pub framebuffer: u32,
    /// The CRTCs it can be shown on, a bit for each by its index.
    pub possible_crtcs: u32,
}

/// What a plane is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum PlaneKind {
    /// The picture under everything, as a rule.
    Primary,
    /// The pointer.
    Cursor,
    /// Anything else: a window a desktop shows straight from its buffer, or
    /// the desktop itself, over a window on the primary plane.
    Overlay,
}

/// The atomic properties that say where a plane is: the part of its
/// framebuffer it shows, where on its CRTC, and its place among the planes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GeometryProperties {
    /// `SRC_X`, `SRC_Y`, `SRC_W` and `SRC_H`.
    source: [u32; 4],
    /// `CRTC_X`, `CRTC_Y`, `CRTC_W` and `CRTC_H`.
    on_crtc: [u32; 4],
    /// `zpos`, if the card has it.
    zpos: Option<u32>,
}

/// The names of [`GeometryProperties`]' properties, in their order, `zpos`
/// last.
const GEOMETRY_PROPERTIES: [&str; 9] = [
    "SRC_X", "SRC_Y", "SRC_W", "SRC_H", "CRTC_X", "CRTC_Y", "CRTC_W", "CRTC_H", "zpos",
];

/// Where a plane is, as its atomic properties say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct PlaneGeometry {
    /// The part of its framebuffer it shows: x, y, width and height, in
    /// 16.16 fixed-point pixels.
    pub source: [u64; 4],
    /// Where it shows on its CRTC: x and y, which may be before the
    /// display's edges, and width and height, in pixels.
    pub on_crtc: (i64, i64, u64, u64),
    /// Its place among the CRTC's planes, higher over lower, if the card
    /// tells it.
    pub zpos: Option<u64>,
}

/// A framebuffer exported as a DMA-BUF.
#[derive(Debug)]
pub(super) struct ExportedFramebuffer {
    /// The buffer.
    pub descriptor: OwnedFd,
    /// The width and height in pixels.
    pub size: (u32, u32),
    /// Its pixel format's four-character code.
    pub format: u32,
    /// How its pixels are laid out: a modifier, or [`MODIFIER_IMPLICIT`].
    pub modifier: u64,
    /// The bytes from one row to the next.
    pub pitch: u32,
    /// The offset of the first row.
    pub offset: u32,
    /// The planes its modifier adds to the pixels', in the same buffer,
    /// such as a compressed layout's metadata: each one's offset and pitch.
    pub extra_planes: Vec<(u32, u32)>,
}

/// Why a framebuffer cannot be exported.
#[derive(Debug)]
pub(super) enum ExportError {
    /// The kernel shows another program's buffers only to the display's
    /// master and to root.
    NotPermitted,
    /// Its planes are in more than one buffer, as no desktop's screen's
    /// are.
    Planar,
    /// The kernel refused.
    Io(io::Error),
}

impl fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPermitted => formatter.write_str(
                "reading the display's buffers needs root (CAP_SYS_ADMIN); run qdp as root",
            ),
            Self::Planar => {
                formatter.write_str("the display shows a picture spread over several buffers")
            }
            Self::Io(error) => write!(formatter, "cannot read the display's buffer: {error}"),
        }
    }
}

/// An open graphics card.
#[derive(Debug)]
pub(super) struct Card {
    /// Its node.
    file: File,
    /// The node's path.
    path: PathBuf,
}

impl Card {
    /// Opens every graphics card, in the order of their nodes' names.
    pub(super) fn open_all() -> Vec<Self> {
        let Ok(entries) = std::fs::read_dir(DRI_DIRECTORY) else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(CARD_PREFIX))
            .map(|entry| entry.path())
            .collect();
        paths.sort();
        paths
            .into_iter()
            .filter_map(|path| {
                let file = OpenOptions::new().read(true).write(true).open(&path).ok()?;
                Some(Self { file, path })
            })
            .collect()
    }

    /// Returns the card's node.
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// Asks the card to show every plane, with as many of their properties
    /// as it can, and returns how much it shows.
    pub(super) fn show_planes(&self) -> PlaneAccess {
        if self.set_capability(CAP_ATOMIC).is_ok() {
            PlaneAccess::Atomic
        } else if self.set_capability(CAP_UNIVERSAL_PLANES).is_ok() {
            PlaneAccess::Universal
        } else {
            PlaneAccess::Legacy
        }
    }

    /// Turns a client capability on.
    fn set_capability(&self, capability: u64) -> io::Result<()> {
        let mut argument = ClientCap {
            capability,
            value: 1,
        };
        self.ioctl(SET_CLIENT_CAP, &mut argument)
    }

    /// Returns the card's CRTCs, in index order, and its connectors.
    pub(super) fn resources(&self) -> io::Result<(Vec<u32>, Vec<u32>)> {
        loop {
            let mut counts = CardResources::default();
            self.ioctl(GET_RESOURCES, &mut counts)?;
            let mut crtcs = vec![0u32; counts.count_crtcs as usize];
            let mut connectors = vec![0u32; counts.count_connectors as usize];
            let mut resources = CardResources {
                crtc_id_ptr: crtcs.as_mut_ptr() as u64,
                connector_id_ptr: connectors.as_mut_ptr() as u64,
                count_crtcs: counts.count_crtcs,
                count_connectors: counts.count_connectors,
                ..CardResources::default()
            };
            self.ioctl(GET_RESOURCES, &mut resources)?;
            // A display plugged in between the calls adds a connector; the
            // counts are asked again then.
            if resources.count_crtcs <= counts.count_crtcs
                && resources.count_connectors <= counts.count_connectors
            {
                crtcs.truncate(resources.count_crtcs as usize);
                connectors.truncate(resources.count_connectors as usize);
                return Ok((crtcs, connectors));
            }
        }
    }

    /// Returns a connector as it is now, without probing its display again.
    pub(super) fn connector(&self, id: u32) -> io::Result<Connector> {
        let mut modes = [ModeInfo::default(); CONNECTOR_MODE_SLOTS];
        let mut connector = RawConnector {
            connector_id: id,
            modes_ptr: modes.as_mut_ptr() as u64,
            count_modes: CONNECTOR_MODE_SLOTS as u32,
            ..RawConnector::default()
        };
        self.ioctl(GET_CONNECTOR, &mut connector)?;
        Ok(Connector {
            name: ConnectorName {
                kind: connector.connector_type,
                index: connector.connector_type_id,
            },
            connected: connector.connection == CONNECTED,
            encoder: connector.encoder_id,
        })
    }

    /// Returns the CRTC an encoder feeds from, or 0 if none.
    pub(super) fn encoder_crtc(&self, id: u32) -> io::Result<u32> {
        let mut encoder = RawEncoder {
            encoder_id: id,
            ..RawEncoder::default()
        };
        self.ioctl(GET_ENCODER, &mut encoder)?;
        Ok(encoder.crtc_id)
    }

    /// Returns a CRTC as it is now.
    pub(super) fn crtc(&self, id: u32) -> io::Result<Crtc> {
        let mut crtc = RawCrtc {
            crtc_id: id,
            ..RawCrtc::default()
        };
        self.ioctl(GET_CRTC, &mut crtc)?;
        let size = (crtc.mode_valid != 0)
            .then(|| (u32::from(crtc.mode.hdisplay), u32::from(crtc.mode.vdisplay)));
        Ok(Crtc {
            framebuffer: crtc.fb_id,
            origin: (crtc.x, crtc.y),
            size,
        })
    }

    /// Returns the card's planes.
    pub(super) fn planes(&self) -> io::Result<Vec<u32>> {
        loop {
            let mut counts = PlaneResources::default();
            self.ioctl(GET_PLANE_RESOURCES, &mut counts)?;
            let mut planes = vec![0u32; counts.count_planes as usize];
            let mut resources = PlaneResources {
                plane_id_ptr: planes.as_mut_ptr() as u64,
                count_planes: counts.count_planes,
            };
            self.ioctl(GET_PLANE_RESOURCES, &mut resources)?;
            if resources.count_planes <= counts.count_planes {
                planes.truncate(resources.count_planes as usize);
                return Ok(planes);
            }
        }
    }

    /// Returns a plane as it is now.
    pub(super) fn plane(&self, id: u32) -> io::Result<Plane> {
        let mut plane = RawPlane {
            plane_id: id,
            ..RawPlane::default()
        };
        self.ioctl(GET_PLANE, &mut plane)?;
        Ok(Plane {
            crtc: plane.crtc_id,
            framebuffer: plane.fb_id,
            possible_crtcs: plane.possible_crtcs,
        })
    }

    /// Returns what a plane is for, from its `type` property, or `None` if
    /// it has none, as planes have only once universal planes are shown.
    pub(super) fn plane_kind(&self, id: u32) -> io::Result<Option<PlaneKind>> {
        let Some(property) = self.find_property(id, "type")? else {
            return Ok(None);
        };
        let value = self
            .plane_properties(id)?
            .into_iter()
            .find(|(prop, _)| *prop == property);
        Ok(value.map(|(_, value)| match value {
            PLANE_TYPE_PRIMARY => PlaneKind::Primary,
            PLANE_TYPE_CURSOR => PlaneKind::Cursor,
            _ => PlaneKind::Overlay,
        }))
    }

    /// Returns the properties that say where a plane is, or `None` if this
    /// program does not see them, as it sees them only through the atomic
    /// interface.
    pub(super) fn geometry_properties(&self, plane: u32) -> io::Result<Option<GeometryProperties>> {
        let mut found = [None; GEOMETRY_PROPERTIES.len()];
        for (property, _) in self.plane_properties(plane)? {
            let name = self.property_name(property)?;
            if let Some(index) = GEOMETRY_PROPERTIES
                .iter()
                .position(|&wanted| wanted == name)
            {
                found[index] = Some(property);
            }
        }
        let required = |range: std::ops::Range<usize>| -> Option<[u32; 4]> {
            let mut properties = [0; 4];
            for (slot, index) in properties.iter_mut().zip(range) {
                *slot = found[index]?;
            }
            Some(properties)
        };
        Ok(required(0..4)
            .zip(required(4..8))
            .map(|(source, on_crtc)| GeometryProperties {
                source,
                on_crtc,
                zpos: found[8],
            }))
    }

    /// Returns where a plane is now, from its `properties`, or `None` if
    /// they are gone.
    pub(super) fn plane_geometry(
        &self,
        plane: u32,
        properties: &GeometryProperties,
    ) -> io::Result<Option<PlaneGeometry>> {
        let values = self.plane_properties(plane)?;
        let value = |wanted: u32| {
            values
                .iter()
                .find(|(property, _)| *property == wanted)
                .map(|&(_, value)| value)
        };
        let four = |properties: [u32; 4]| -> Option<[u64; 4]> {
            Some([
                value(properties[0])?,
                value(properties[1])?,
                value(properties[2])?,
                value(properties[3])?,
            ])
        };
        let (Some(source), Some(on_crtc)) = (four(properties.source), four(properties.on_crtc))
        else {
            return Ok(None);
        };
        Ok(Some(PlaneGeometry {
            source,
            // The position is signed: a plane may start before the
            // display's edges.
            on_crtc: (on_crtc[0] as i64, on_crtc[1] as i64, on_crtc[2], on_crtc[3]),
            zpos: properties.zpos.and_then(value),
        }))
    }

    /// Returns the identifier of the plane's property named `name`, if it
    /// has one.
    pub(super) fn find_property(&self, plane: u32, name: &str) -> io::Result<Option<u32>> {
        for (property, _) in self.plane_properties(plane)? {
            if self.property_name(property)? == name {
                return Ok(Some(property));
            }
        }
        Ok(None)
    }

    /// Returns a plane's properties and their values.
    pub(super) fn plane_properties(&self, plane: u32) -> io::Result<Vec<(u32, u64)>> {
        loop {
            let mut counts = ObjectProperties {
                obj_id: plane,
                obj_type: OBJECT_PLANE,
                ..ObjectProperties::default()
            };
            self.ioctl(GET_OBJECT_PROPERTIES, &mut counts)?;
            let mut properties = vec![0u32; counts.count_props as usize];
            let mut values = vec![0u64; counts.count_props as usize];
            let mut listed = ObjectProperties {
                props_ptr: properties.as_mut_ptr() as u64,
                prop_values_ptr: values.as_mut_ptr() as u64,
                count_props: counts.count_props,
                obj_id: plane,
                obj_type: OBJECT_PLANE,
            };
            self.ioctl(GET_OBJECT_PROPERTIES, &mut listed)?;
            if listed.count_props <= counts.count_props {
                let count = listed.count_props as usize;
                return Ok(properties.into_iter().zip(values).take(count).collect());
            }
        }
    }

    /// Returns a property's name.
    fn property_name(&self, id: u32) -> io::Result<String> {
        let mut property = RawProperty {
            prop_id: id,
            ..RawProperty::default()
        };
        self.ioctl(GET_PROPERTY, &mut property)?;
        let end = property
            .name
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(property.name.len());
        Ok(String::from_utf8_lossy(&property.name[..end]).into_owned())
    }

    /// Exports a framebuffer as a DMA-BUF.
    pub(super) fn export_framebuffer(&self, id: u32) -> Result<ExportedFramebuffer, ExportError> {
        let mut framebuffer = RawFramebuffer {
            fb_id: id,
            ..RawFramebuffer::default()
        };
        self.ioctl(GET_FRAMEBUFFER, &mut framebuffer)
            .map_err(ExportError::Io)?;
        let handle = framebuffer.handles[0];
        let exported = if handle == 0 {
            Err(ExportError::NotPermitted)
        } else if framebuffer.handles[1..]
            .iter()
            .any(|&other| other != 0 && other != handle)
        {
            Err(ExportError::Planar)
        } else {
            self.export_handle(handle).map_err(ExportError::Io)
        };
        // Reading the framebuffer gave this file handles to its buffers,
        // which are closed once exported, so that they do not pile up.
        let mut handles = framebuffer.handles;
        handles.sort_unstable();
        let mut previous = 0;
        for handle in handles {
            if handle != 0 && handle != previous {
                self.close_handle(handle);
            }
            previous = handle;
        }
        let modifier = if framebuffer.flags & FRAMEBUFFER_MODIFIERS != 0 {
            framebuffer.modifier[0]
        } else {
            MODIFIER_IMPLICIT
        };
        Ok(ExportedFramebuffer {
            descriptor: exported?,
            size: (framebuffer.width, framebuffer.height),
            format: framebuffer.pixel_format,
            modifier,
            pitch: framebuffer.pitches[0],
            offset: framebuffer.offsets[0],
            extra_planes: framebuffer.pitches[1..]
                .iter()
                .zip(&framebuffer.offsets[1..])
                .take_while(|(pitch, _)| **pitch != 0)
                .map(|(&pitch, &offset)| (offset, pitch))
                .collect(),
        })
    }

    /// Exports a buffer this file holds a handle to as a DMA-BUF.
    fn export_handle(&self, handle: u32) -> io::Result<OwnedFd> {
        let mut prime = PrimeHandle {
            handle,
            flags: libc::O_CLOEXEC as u32,
            fd: -1,
        };
        self.ioctl(PRIME_HANDLE_TO_FD, &mut prime)?;
        // SAFETY: the kernel returned a new descriptor of the DMA-BUF,
        // which nothing else owns.
        Ok(unsafe { OwnedFd::from_raw_fd(prime.fd) })
    }

    /// Closes this file's handle to a buffer; the buffer lives on while
    /// the display or a DMA-BUF holds it.
    fn close_handle(&self, handle: u32) {
        let mut close = GemClose { handle, pad: 0 };
        let _ = self.ioctl(GEM_CLOSE, &mut close);
    }

    /// Waits for the next refresh of the CRTC at `index`.
    pub(super) fn wait_for_refresh(&self, index: u32) -> io::Result<()> {
        let mut wait = WaitVblank {
            kind: VBLANK_RELATIVE | vblank_crtc_bits(index),
            sequence: 1,
            ..WaitVblank::default()
        };
        self.ioctl(WAIT_VBLANK, &mut wait)
    }

    /// Makes request `request` with `argument`, again if a signal
    /// interrupted it.
    fn ioctl<T>(&self, request: libc::Ioctl, argument: &mut T) -> io::Result<()> {
        loop {
            // SAFETY: every request is made with the `repr(C)` structure
            // the kernel takes for it, which `T` is at each call, alive and
            // borrowed for the call; the arrays its pointers name outlive
            // it, with room for the counts they are passed with.
            let result = unsafe {
                libc::ioctl(
                    self.file.as_raw_fd(),
                    request,
                    std::ptr::from_mut(argument).cast::<c_void>(),
                )
            };
            if result == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

/// Returns the bits of a vertical blank request that name the CRTC at
/// `index`, as `drmWaitVBlank` sets them.
fn vblank_crtc_bits(index: u32) -> u32 {
    match index {
        0 => 0,
        1 => VBLANK_SECONDARY,
        _ => (index << VBLANK_HIGH_CRTC_SHIFT) & VBLANK_HIGH_CRTC_MASK,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The structures have the kernel's sizes, so that each request
    /// carries the number the kernel knows it by.
    #[test]
    fn structures_have_the_kernels_sizes() {
        assert_eq!(size_of::<ClientCap>(), 16);
        assert_eq!(size_of::<GemClose>(), 8);
        assert_eq!(size_of::<PrimeHandle>(), 12);
        assert_eq!(size_of::<CardResources>(), 64);
        assert_eq!(size_of::<ModeInfo>(), 68);
        assert_eq!(size_of::<RawCrtc>(), 104);
        assert_eq!(size_of::<RawEncoder>(), 20);
        assert_eq!(size_of::<RawConnector>(), 80);
        assert_eq!(size_of::<PlaneResources>(), 16);
        assert_eq!(size_of::<RawPlane>(), 32);
        assert_eq!(size_of::<ObjectProperties>(), 32);
        assert_eq!(size_of::<RawProperty>(), 64);
        assert_eq!(size_of::<RawFramebuffer>(), 104);
    }

    /// Returns a request's number, whatever type the C library gives it.
    #[allow(clippy::unnecessary_cast, reason = "`libc::Ioctl` is `c_int` on musl")]
    fn number(request: libc::Ioctl) -> u64 {
        request as u64
    }

    /// The requests are the numbers the kernel's headers give them on
    /// 64-bit systems.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn requests_are_the_kernels() {
        assert_eq!(number(SET_CLIENT_CAP), 0x4010_640d);
        assert_eq!(number(GEM_CLOSE), 0x4008_6409);
        assert_eq!(number(PRIME_HANDLE_TO_FD), 0xc00c_642d);
        assert_eq!(number(WAIT_VBLANK), 0xc018_643a);
        assert_eq!(number(GET_RESOURCES), 0xc040_64a0);
        assert_eq!(number(GET_CRTC), 0xc068_64a1);
        assert_eq!(number(GET_ENCODER), 0xc014_64a6);
        assert_eq!(number(GET_CONNECTOR), 0xc050_64a7);
        assert_eq!(number(GET_PROPERTY), 0xc040_64aa);
        assert_eq!(number(GET_PLANE_RESOURCES), 0xc010_64b5);
        assert_eq!(number(GET_PLANE), 0xc020_64b6);
        assert_eq!(number(GET_OBJECT_PROPERTIES), 0xc020_64b9);
        assert_eq!(number(GET_FRAMEBUFFER), 0xc068_64ce);
    }

    /// The first two CRTCs are named as the oldest interface named them,
    /// and later ones by their index.
    #[test]
    fn names_crtcs_for_vertical_blanks() {
        assert_eq!(vblank_crtc_bits(0), 0);
        assert_eq!(vblank_crtc_bits(1), VBLANK_SECONDARY);
        assert_eq!(vblank_crtc_bits(2), 0x4);
        assert_eq!(vblank_crtc_bits(5), 0xa);
    }

    /// Connectors are named as the kernel names them.
    #[test]
    fn names_connectors() {
        let name = |kind, index| ConnectorName { kind, index }.to_string();
        assert_eq!(name(10, 1), "DP-1");
        assert_eq!(name(11, 2), "HDMI-A-2");
        assert_eq!(name(14, 1), "eDP-1");
        assert_eq!(name(99, 1), "Unknown-1");
    }
}
