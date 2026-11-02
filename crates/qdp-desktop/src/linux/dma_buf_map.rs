//! Reading DMA-BUFs with the processor: mapping a buffer into memory,
//! bracketed by the synchronization that makes the GPU's writes visible.
//! Reading GPU memory is slow, so this is for small buffers, such as a
//! pointer's image, and for the odd frame that must reach memory.

use std::ffi::c_void;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::ptr::NonNull;

use qdp_codec::{DRM_FORMAT_MOD_LINEAR, DmaBufFrame, PixelDepth, Yuv420Frame};

/// The bytes of one 32-bit pixel.
pub(super) const PIXEL_BYTES: usize = 4;

/// `DMA_BUF_IOCTL_SYNC`: `_IOW('b', 0, struct dma_buf_sync)`, which brackets
/// the processor's access to a mapped DMA-BUF.
const DMA_BUF_IOCTL_SYNC: u64 = 0x4008_6200;

/// `DMA_BUF_SYNC_READ`, with `DMA_BUF_SYNC_START`, which is 0.
const DMA_BUF_SYNC_READ: u64 = 1;

/// `DMA_BUF_SYNC_END`.
const DMA_BUF_SYNC_END: u64 = 1 << 2;

/// A DMA-BUF mapped for reading. The GPU's writes before the mapping are
/// visible through it; the mapping is undone when it is dropped.
pub(super) struct MappedDmaBuf<'a> {
    /// Where the buffer is mapped.
    address: NonNull<c_void>,
    /// The mapped bytes.
    length: usize,
    /// The buffer.
    descriptor: BorrowedFd<'a>,
}

impl<'a> MappedDmaBuf<'a> {
    /// Maps the first `length` bytes of the buffer read-only, or returns
    /// `None` if its driver cannot map it.
    pub(super) fn map(descriptor: BorrowedFd<'a>, length: usize) -> Option<Self> {
        if length == 0 {
            return None;
        }
        // SAFETY: maps `length` bytes of the buffer read-only and shared;
        // the mapping is undone when the value is dropped, and nothing
        // refers to it after.
        let address = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ,
                libc::MAP_SHARED,
                descriptor.as_raw_fd(),
                0,
            )
        };
        if address == libc::MAP_FAILED {
            return None;
        }
        let mapped = Self {
            address: NonNull::new(address)?,
            length,
            descriptor,
        };
        mapped.sync(DMA_BUF_SYNC_READ);
        Some(mapped)
    }

    /// Returns the mapped bytes.
    pub(super) fn bytes(&self) -> &[u8] {
        // SAFETY: the mapping holds `length` readable bytes until it is
        // undone, which needs `self`, so not while the slice is borrowed.
        unsafe { std::slice::from_raw_parts(self.address.as_ptr().cast::<u8>(), self.length) }
    }

    /// Starts or ends the processor's reading, as `flags` say.
    fn sync(&self, flags: u64) {
        let mut sync = flags;
        // SAFETY: `DMA_BUF_IOCTL_SYNC` reads one `u64` of flags, which
        // lives for the call.
        unsafe {
            libc::ioctl(
                self.descriptor.as_raw_fd(),
                DMA_BUF_IOCTL_SYNC as _,
                &mut sync,
            )
        };
    }
}

impl Drop for MappedDmaBuf<'_> {
    fn drop(&mut self) {
        self.sync(DMA_BUF_SYNC_READ | DMA_BUF_SYNC_END);
        // SAFETY: the mapping was made by `map`, and no slice of it
        // outlives `self`.
        unsafe { libc::munmap(self.address.as_ptr(), self.length) };
    }
}

/// Reads a frame of 8-bit colours in a linear DMA-BUF into memory, its
/// picture scaled to its place in it, or returns `None` if it is not
/// linear, has deeper colours, or cannot be mapped.
pub(super) fn read_linear(frame: &DmaBufFrame) -> Option<Yuv420Frame> {
    let layout = frame.layout();
    if layout.modifier != DRM_FORMAT_MOD_LINEAR || frame.depth() != PixelDepth::Eight {
        return None;
    }
    let size = usize::try_from(frame.size()).ok()?;
    let region = frame.source();
    let pitch = layout.pitch as usize;
    let start = layout.offset as usize + region.y * pitch + region.x * PIXEL_BYTES;
    let end = start + pitch * (region.height - 1) + region.width * PIXEL_BYTES;
    if end > size {
        return None;
    }
    let mapped = MappedDmaBuf::map(frame.descriptor(), size)?;
    let picture = Yuv420Frame::from_rgb32(
        region.width,
        region.height,
        pitch,
        layout.order,
        &mapped.bytes()[start..],
    )?;
    let placed = frame.placed();
    Some(picture.scaled(placed.width, placed.height).placed_on_black(
        frame.width(),
        frame.height(),
        placed.x,
        placed.y,
    ))
}
