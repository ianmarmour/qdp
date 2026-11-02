//! Frames that stay in GPU memory, shared as DMA-BUFs: the way Linux
//! compositors share their screens. A hardware encoder reads them where they
//! are, converting their colours on the GPU, so that no pixel passes through
//! the processor.

use std::fs::File;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::fs::MetadataExt;

use crate::{CodecError, Rgb32Order};

/// `DRM_FORMAT_MOD_LINEAR`: rows of pixels one after another, a layout every
/// GPU can read and write.
pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;

/// The bytes of one 32-bit pixel.
const PIXEL_BYTES: usize = 4;

/// Where a frame's pixels are in its DMA-BUF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DmaBufLayout {
    /// The width in pixels.
    pub width: usize,
    /// The height in pixels.
    pub height: usize,
    /// The order of each pixel's colour bytes; the fourth byte, alpha or
    /// padding, is ignored.
    pub order: Rgb32Order,
    /// The DRM format modifier: how the pixels are tiled, if they are.
    pub modifier: u64,
    /// The offset of the first row, in bytes.
    pub offset: u32,
    /// The bytes from one row to the next.
    pub pitch: u32,
}

/// A further plane of a frame's buffer that its modifier adds to the
/// pixels', such as the metadata of a compressed layout: where it is in
/// the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DmaBufPlane {
    /// The offset of its first row, in bytes.
    pub offset: u32,
    /// The bytes from one of its rows to the next.
    pub pitch: u32,
}

/// A rectangle of pixels in a frame or its buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DmaBufRect {
    /// Its first column.
    pub x: usize,
    /// Its first row.
    pub y: usize,
    /// Its width.
    pub width: usize,
    /// Its height.
    pub height: usize,
}

impl DmaBufRect {
    /// Returns the rectangle of all of a `width × height` area.
    pub fn whole(width: usize, height: usize) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    /// Returns whether the rectangle is not empty and all inside a
    /// `width × height` area.
    fn is_inside(&self, width: usize, height: usize) -> bool {
        let fits = |start: usize, length: usize, limit: usize| {
            length > 0 && start.checked_add(length).is_some_and(|end| end <= limit)
        };
        fits(self.x, self.width, width) && fits(self.y, self.height, height)
    }
}

/// The most planes a modifier adds to the pixels': DRM buffers have four
/// planes at most.
pub const MAX_EXTRA_PLANES: usize = 3;

/// How many bits each colour of a frame's 32-bit pixels has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PixelDepth {
    /// Eight bits a colour, and eight of alpha or padding.
    #[default]
    Eight,
    /// Ten bits a colour, and two of alpha or padding, in a little-endian
    /// word whose lowest bits are the first colour of the order, as the
    /// displays of desktops that show deeper colour scan out.
    Ten,
}

/// Which buffer a DMA-BUF descriptor refers to: the same for every
/// descriptor of one buffer, and different for every other buffer that
/// exists at the same time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BufferIdentity {
    /// The device of the buffer's file.
    device: u64,
    /// The inode of the buffer's file.
    inode: u64,
}

/// One frame of 32-bit pixels in a DMA-BUF, in one plane.
///
/// The frame holds its own descriptor of the buffer, so the buffer lives
/// at least as long as the frame. The capture that made it may write the
/// buffer again later, once it has been handed back; the GPU orders that
/// write after the reads made before it.
#[derive(Debug)]
pub struct DmaBufFrame {
    /// The buffer.
    buffer: File,
    /// Where the frame is in it.
    layout: DmaBufLayout,
    /// The buffer's size in bytes.
    size: u64,
    /// Which buffer it is.
    identity: BufferIdentity,
    /// The planes the modifier adds to the pixels', in the same buffer.
    extra_planes: Vec<DmaBufPlane>,
    /// The bits each colour has.
    depth: PixelDepth,
    /// The part of the buffer that the picture is.
    source: DmaBufRect,
    /// The frame's width and height.
    frame_size: (usize, usize),
    /// Where in the frame the picture is, scaled from its source; the rest
    /// is black.
    placed: DmaBufRect,
}

impl DmaBufFrame {
    /// Takes a DMA-BUF descriptor and where the frame is in it. Fails if the
    /// frame is empty, its rows do not fit its pitch, or the descriptor
    /// cannot be examined.
    pub fn new(descriptor: OwnedFd, layout: DmaBufLayout) -> Result<Self, CodecError> {
        if layout.width == 0 || layout.height == 0 {
            return Err(CodecError::FrameSize);
        }
        let row_bytes = layout
            .width
            .checked_mul(PIXEL_BYTES)
            .ok_or(CodecError::FrameSize)?;
        if layout.modifier == DRM_FORMAT_MOD_LINEAR && (layout.pitch as usize) < row_bytes {
            return Err(CodecError::FrameSize);
        }
        let buffer = File::from(descriptor);
        let metadata = buffer
            .metadata()
            .map_err(|error| CodecError::Encoder(format!("cannot examine a DMA-BUF: {error}")))?;
        // A DMA-BUF's file has the buffer's size; the frame's own extent
        // stands in where it does not say.
        let extent = u64::from(layout.offset) + u64::from(layout.pitch) * layout.height as u64;
        let size = if metadata.len() > 0 {
            metadata.len()
        } else {
            extent
        };
        Ok(Self {
            buffer,
            layout,
            size,
            identity: BufferIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
            },
            extra_planes: Vec::new(),
            depth: PixelDepth::Eight,
            source: DmaBufRect::whole(layout.width, layout.height),
            frame_size: (layout.width, layout.height),
            placed: DmaBufRect::whole(layout.width, layout.height),
        })
    }

    /// Returns a `size` frame showing the `source` part of the buffer at
    /// `placed` in it, scaled to fit, and black elsewhere: as a display
    /// shows a plane whose buffer is larger or smaller than the screen, or
    /// that covers only part of it. Fails if `source` is not all in the
    /// buffer, `placed` not all in the frame, or either is empty.
    pub fn with_picture(
        mut self,
        source: DmaBufRect,
        size: (usize, usize),
        placed: DmaBufRect,
    ) -> Result<Self, CodecError> {
        if !source.is_inside(self.layout.width, self.layout.height)
            || !placed.is_inside(size.0, size.1)
        {
            return Err(CodecError::FrameSize);
        }
        self.source = source;
        self.frame_size = size;
        self.placed = placed;
        Ok(self)
    }

    /// Returns the part of the buffer that the picture is.
    pub fn source(&self) -> DmaBufRect {
        self.source
    }

    /// Returns where in the frame the picture is.
    pub fn placed(&self) -> DmaBufRect {
        self.placed
    }

    /// Returns whether the frame is all of its buffer, as it is.
    pub fn is_whole_buffer(&self) -> bool {
        let whole = DmaBufRect::whole(self.layout.width, self.layout.height);
        self.source == whole && self.placed == whole
    }

    /// Returns the frame with colours of `depth` bits.
    pub fn with_depth(mut self, depth: PixelDepth) -> Self {
        self.depth = depth;
        self
    }

    /// Returns how many bits each colour has.
    pub fn depth(&self) -> PixelDepth {
        self.depth
    }

    /// Returns the frame with the planes its modifier adds to the pixels',
    /// in the same buffer, such as a compressed layout's metadata, as a
    /// display's buffers have; [`MAX_EXTRA_PLANES`] at most are kept.
    pub fn with_extra_planes(mut self, planes: &[DmaBufPlane]) -> Self {
        self.extra_planes = planes.iter().copied().take(MAX_EXTRA_PLANES).collect();
        self
    }

    /// Returns the planes the modifier adds to the pixels'.
    pub fn extra_planes(&self) -> &[DmaBufPlane] {
        &self.extra_planes
    }

    /// Returns the frame's width in pixels: the buffer's, unless
    /// [`DmaBufFrame::with_picture`] says otherwise.
    pub fn width(&self) -> usize {
        self.frame_size.0
    }

    /// Returns the frame's height in pixels.
    pub fn height(&self) -> usize {
        self.frame_size.1
    }

    /// Returns where the frame is in its buffer: the whole buffer's layout,
    /// of which the picture is [`DmaBufFrame::source`].
    pub fn layout(&self) -> DmaBufLayout {
        self.layout
    }

    /// Returns the buffer's size in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Returns which buffer the frame is in.
    pub fn identity(&self) -> BufferIdentity {
        self.identity
    }

    /// Returns the buffer's descriptor.
    pub fn descriptor(&self) -> BorrowedFd<'_> {
        self.buffer.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A file standing in for a DMA-BUF, of `size` bytes.
    fn buffer(size: usize) -> OwnedFd {
        let mut file = tempfile();
        file.write_all(&vec![0; size]).unwrap();
        OwnedFd::from(file)
    }

    /// Returns an unnamed temporary file.
    fn tempfile() -> File {
        let path = std::env::temp_dir().join(format!(
            "qdp-dma-buf-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        file
    }

    /// A linear layout of `width × height` pixels.
    fn layout(width: usize, height: usize, pitch: u32) -> DmaBufLayout {
        DmaBufLayout {
            width,
            height,
            order: Rgb32Order::Bgr,
            modifier: DRM_FORMAT_MOD_LINEAR,
            offset: 0,
            pitch,
        }
    }

    /// A frame knows its buffer's size and identity, which another
    /// descriptor of the same buffer shares.
    #[test]
    fn identifies_its_buffer() {
        let descriptor = buffer(64 * 4 * 2);
        let again = descriptor.try_clone().unwrap();
        let frame = DmaBufFrame::new(descriptor, layout(64, 2, 256)).unwrap();
        assert_eq!((frame.width(), frame.height()), (64, 2));
        assert_eq!(frame.size(), 512);
        let same = DmaBufFrame::new(again, layout(64, 2, 256)).unwrap();
        assert_eq!(frame.identity(), same.identity());
        let other = DmaBufFrame::new(buffer(512), layout(64, 2, 256)).unwrap();
        assert_ne!(frame.identity(), other.identity());
    }

    /// A frame is all of its buffer until given a picture, whose source
    /// must be in the buffer and place in the frame; its size is then the
    /// frame's.
    #[test]
    fn shows_part_of_its_buffer() {
        let frame = || DmaBufFrame::new(buffer(64 * 4 * 8), layout(64, 8, 256)).unwrap();
        assert!(frame().is_whole_buffer());
        let source = DmaBufRect {
            x: 8,
            y: 2,
            width: 32,
            height: 4,
        };
        let placed = DmaBufRect {
            x: 100,
            y: 10,
            width: 320,
            height: 40,
        };
        let shown = frame().with_picture(source, (640, 80), placed).unwrap();
        assert!(!shown.is_whole_buffer());
        assert_eq!((shown.source(), shown.placed()), (source, placed));
        assert_eq!((shown.width(), shown.height()), (640, 80));
        assert_eq!(shown.layout().width, 64);
        let outside = DmaBufRect { x: 40, ..source };
        assert!(frame().with_picture(outside, (640, 80), placed).is_err());
        let empty = DmaBufRect {
            height: 0,
            ..source
        };
        assert!(frame().with_picture(empty, (640, 80), placed).is_err());
        assert!(frame().with_picture(source, (400, 80), placed).is_err());
    }

    /// Empty frames, and linear rows longer than their pitch, are refused.
    #[test]
    fn refuses_impossible_layouts() {
        assert!(DmaBufFrame::new(buffer(16), layout(0, 2, 256)).is_err());
        assert!(DmaBufFrame::new(buffer(512), layout(64, 2, 255)).is_err());
        let tiled = DmaBufLayout {
            modifier: 1,
            ..layout(64, 2, 128)
        };
        assert!(DmaBufFrame::new(buffer(512), tiled).is_ok());
    }
}
