//! The desktop image Desktop Duplication delivers, turned upright, and the
//! pointer drawn over it: Desktop Duplication delivers the pointer apart from
//! the desktop.

/// Bytes per pixel of a 32-bit BGRA image.
const PIXEL: usize = 4;

/// A fully opaque alpha value.
const OPAQUE: u8 = 0xff;

/// Every bit of a colour channel.
const ALL_BITS: u8 = 0xff;

/// How far the display is rotated from its native orientation, which is the
/// orientation the duplicated image is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Rotation {
    /// Not rotated.
    #[default]
    Identity,
    /// Rotated 90 degrees clockwise: portrait.
    Clockwise90,
    /// Rotated 180 degrees: landscape, flipped.
    Clockwise180,
    /// Rotated 270 degrees clockwise: portrait, flipped.
    Clockwise270,
}

impl Rotation {
    /// Returns the size of the upright desktop of a `width × height` native
    /// image.
    pub(super) const fn desktop_size(self, width: usize, height: usize) -> (usize, usize) {
        match self {
            Self::Identity | Self::Clockwise180 => (width, height),
            Self::Clockwise90 | Self::Clockwise270 => (height, width),
        }
    }

    /// Returns the native image position showing desktop position `(x, y)`,
    /// for a native image `width × height` pixels.
    const fn native_position(
        self,
        (x, y): (usize, usize),
        (width, height): (usize, usize),
    ) -> (usize, usize) {
        match self {
            Self::Identity => (x, y),
            Self::Clockwise90 => (width - 1 - y, x),
            Self::Clockwise180 => (width - 1 - x, height - 1 - y),
            Self::Clockwise270 => (y, height - 1 - x),
        }
    }
}

/// A 32-bit BGRA image with packed rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Bgra {
    /// The width in pixels.
    pub(super) width: usize,
    /// The height in pixels.
    pub(super) height: usize,
    /// The pixels, row by row.
    pub(super) data: Vec<u8>,
}

impl Bgra {
    /// Returns the bytes per row.
    pub(super) const fn stride(&self) -> usize {
        self.width * PIXEL
    }

    /// Replaces the image with the upright desktop of a native image, `width ×
    /// height` pixels with `stride` bytes per row. Returns `None` if `source`
    /// is too short.
    pub(super) fn copy_upright(
        &mut self,
        source: &[u8],
        (width, height): (usize, usize),
        stride: usize,
        rotation: Rotation,
    ) -> Option<()> {
        if width == 0 || height == 0 || stride < width * PIXEL {
            return None;
        }
        if source.len() < stride * (height - 1) + width * PIXEL {
            return None;
        }
        (self.width, self.height) = rotation.desktop_size(width, height);
        self.data.resize(self.width * self.height * PIXEL, 0);
        if rotation == Rotation::Identity {
            for (row, line) in self.data.chunks_exact_mut(width * PIXEL).enumerate() {
                line.copy_from_slice(&source[row * stride..row * stride + width * PIXEL]);
            }
            return Some(());
        }
        let desktop_stride = self.stride();
        for y in 0..self.height {
            for x in 0..self.width {
                let (native_x, native_y) = rotation.native_position((x, y), (width, height));
                let from = native_y * stride + native_x * PIXEL;
                let to = y * desktop_stride + x * PIXEL;
                self.data[to..to + PIXEL].copy_from_slice(&source[from..from + PIXEL]);
            }
        }
        Some(())
    }

    /// Returns the pixel at `(x, y)`, which must be inside the image.
    fn pixel_mut(&mut self, x: usize, y: usize) -> &mut [u8] {
        let start = y * self.stride() + x * PIXEL;
        &mut self.data[start..start + PIXEL]
    }
}

/// How a pointer shape's pixels combine with the desktop under them
/// (`DXGI_OUTDUPL_POINTER_SHAPE_TYPE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShapeKind {
    /// One bit per pixel: an AND mask, then an XOR mask, each `height` rows.
    Monochrome,
    /// 32-bit BGRA, blended by its alpha.
    Color,
    /// 32-bit BGRA whose alpha is a mask: 0 replaces the desktop with the
    /// colour, 0xFF XORs the desktop with it.
    MaskedColor,
}

impl ShapeKind {
    /// The `DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME` value.
    const MONOCHROME: u32 = 1;
    /// The `DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR` value.
    const COLOR: u32 = 2;
    /// The `DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR` value.
    const MASKED_COLOR: u32 = 4;

    /// Returns the kind of a `DXGI_OUTDUPL_POINTER_SHAPE_TYPE` value.
    pub(super) const fn from_dxgi(value: u32) -> Option<Self> {
        match value {
            Self::MONOCHROME => Some(Self::Monochrome),
            Self::COLOR => Some(Self::Color),
            Self::MASKED_COLOR => Some(Self::MaskedColor),
            _ => None,
        }
    }
}

/// A pointer's image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PointerShape {
    /// How the pixels combine with the desktop.
    kind: ShapeKind,
    /// The width in pixels.
    width: usize,
    /// The height in pixels; a monochrome shape has twice as many rows.
    height: usize,
    /// The bytes per row.
    pitch: usize,
    /// The rows.
    data: Vec<u8>,
}

impl PointerShape {
    /// Returns a shape from Desktop Duplication's description of it, or
    /// `None` if `data` is too short.
    pub(super) fn new(
        kind: ShapeKind,
        (width, height): (usize, usize),
        pitch: usize,
        data: Vec<u8>,
    ) -> Option<Self> {
        let height = match kind {
            ShapeKind::Monochrome => height / 2,
            ShapeKind::Color | ShapeKind::MaskedColor => height,
        };
        let rows = match kind {
            ShapeKind::Monochrome => height * 2,
            ShapeKind::Color | ShapeKind::MaskedColor => height,
        };
        let row_bytes = match kind {
            ShapeKind::Monochrome => width.div_ceil(8),
            ShapeKind::Color | ShapeKind::MaskedColor => width * PIXEL,
        };
        if pitch < row_bytes || (rows > 0 && data.len() < pitch * (rows - 1) + row_bytes) {
            return None;
        }
        Some(Self {
            kind,
            width,
            height,
            pitch,
            data,
        })
    }

    /// Returns bit `x` of a monochrome mask row.
    fn mask_bit(&self, row: usize, x: usize) -> bool {
        self.data[row * self.pitch + x / 8] & (0x80 >> (x % 8)) != 0
    }

    /// Returns the BGRA pixel at `(x, y)` of a colour shape.
    fn colour(&self, x: usize, y: usize) -> [u8; 4] {
        let start = y * self.pitch + x * PIXEL;
        let mut pixel = [0; 4];
        pixel.copy_from_slice(&self.data[start..start + PIXEL]);
        pixel
    }

    /// Draws the shape over `image` with its top-left corner at `(left,
    /// top)`, clipped to the image.
    pub(super) fn draw(&self, image: &mut Bgra, (left, top): (i64, i64)) {
        for y in 0..self.height {
            let Some(image_y) = offset(top, y, image.height) else {
                continue;
            };
            for x in 0..self.width {
                let Some(image_x) = offset(left, x, image.width) else {
                    continue;
                };
                let pixel = image.pixel_mut(image_x, image_y);
                match self.kind {
                    ShapeKind::Monochrome => {
                        let keep = if self.mask_bit(y, x) { ALL_BITS } else { 0 };
                        let flip = if self.mask_bit(self.height + y, x) {
                            ALL_BITS
                        } else {
                            0
                        };
                        for channel in &mut pixel[..3] {
                            *channel = (*channel & keep) ^ flip;
                        }
                    }
                    ShapeKind::Color => {
                        let [blue, green, red, alpha] = self.colour(x, y);
                        for (channel, source) in pixel[..3].iter_mut().zip([blue, green, red]) {
                            *channel = blend(source, *channel, alpha);
                        }
                    }
                    ShapeKind::MaskedColor => {
                        let [blue, green, red, mask] = self.colour(x, y);
                        let invert = if mask == OPAQUE {
                            [pixel[0], pixel[1], pixel[2]]
                        } else {
                            [0; 3]
                        };
                        for ((channel, source), under) in
                            pixel[..3].iter_mut().zip([blue, green, red]).zip(invert)
                        {
                            *channel = source ^ under;
                        }
                    }
                }
                pixel[3] = OPAQUE;
            }
        }
    }
}

/// Returns `start + index` if it is inside `0..limit`.
fn offset(start: i64, index: usize, limit: usize) -> Option<usize> {
    usize::try_from(start + i64::try_from(index).ok()?)
        .ok()
        .filter(|&position| position < limit)
}

/// Blends a channel of `source` with `alpha` over `destination`.
fn blend(source: u8, destination: u8, alpha: u8) -> u8 {
    let alpha = u32::from(alpha);
    let mixed = u32::from(source) * alpha + u32::from(destination) * (u32::from(OPAQUE) - alpha);
    ((mixed + u32::from(OPAQUE) / 2) / u32::from(OPAQUE)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a `width × height` image whose pixels are numbered.
    fn numbered(width: usize, height: usize) -> Vec<u8> {
        (0..width * height)
            .flat_map(|index| [index as u8, 0, 0, OPAQUE])
            .collect()
    }

    /// Returns the numbers of an image's pixels, row by row.
    fn numbers(image: &Bgra) -> Vec<u8> {
        image
            .data
            .chunks_exact(PIXEL)
            .map(|pixel| pixel[0])
            .collect()
    }

    /// The native image is copied upright for each rotation.
    #[test]
    fn turns_the_desktop_upright() {
        // The native image is 3 × 2:
        //   0 1 2
        //   3 4 5
        let source = numbered(3, 2);
        let mut image = Bgra::default();
        image
            .copy_upright(&source, (3, 2), 12, Rotation::Identity)
            .unwrap();
        assert_eq!((image.width, image.height), (3, 2));
        assert_eq!(numbers(&image), [0, 1, 2, 3, 4, 5]);
        // Turning the 90° image back counter-clockwise puts its top-right
        // corner at the top-left.
        image
            .copy_upright(&source, (3, 2), 12, Rotation::Clockwise90)
            .unwrap();
        assert_eq!((image.width, image.height), (2, 3));
        assert_eq!(numbers(&image), [2, 5, 1, 4, 0, 3]);
        image
            .copy_upright(&source, (3, 2), 12, Rotation::Clockwise180)
            .unwrap();
        assert_eq!(numbers(&image), [5, 4, 3, 2, 1, 0]);
        image
            .copy_upright(&source, (3, 2), 12, Rotation::Clockwise270)
            .unwrap();
        assert_eq!((image.width, image.height), (2, 3));
        assert_eq!(numbers(&image), [3, 0, 4, 1, 5, 2]);
    }

    /// Row padding is skipped, and short sources are refused.
    #[test]
    fn honours_the_stride() {
        let mut source = numbered(2, 2);
        source.splice(8..8, [9; 8]);
        let mut image = Bgra::default();
        image
            .copy_upright(&source, (2, 2), 16, Rotation::Identity)
            .unwrap();
        assert_eq!(numbers(&image), [0, 1, 2, 3]);
        assert!(
            image
                .copy_upright(&source[..20], (2, 2), 16, Rotation::Identity)
                .is_none()
        );
    }

    /// Returns a `width × height` grey image.
    fn grey(width: usize, height: usize) -> Bgra {
        Bgra {
            width,
            height,
            data: [0x80, 0x80, 0x80, OPAQUE].repeat(width * height),
        }
    }

    /// Monochrome masks make black, white, the desktop, or its inverse.
    #[test]
    fn draws_monochrome_pointers() {
        // A 4 × 1 shape: AND mask 0011, XOR mask 0101.
        let shape = PointerShape::new(ShapeKind::Monochrome, (4, 2), 1, vec![0x30, 0x50]).unwrap();
        let mut image = grey(4, 1);
        shape.draw(&mut image, (0, 0));
        let colours: Vec<u8> = image.data.chunks_exact(PIXEL).map(|p| p[0]).collect();
        assert_eq!(colours, [0x00, 0xff, 0x80, 0x7f]);
    }

    /// Colour pointers blend by alpha; masked colour pointers replace or XOR.
    #[test]
    fn draws_colour_pointers() {
        let pixels = [
            [0xff, 0xff, 0xff, 0xff],
            [0xff, 0xff, 0xff, 0x00],
            [0x00, 0x00, 0x00, 0x80],
        ];
        let shape = PointerShape::new(ShapeKind::Color, (3, 1), 12, pixels.concat()).unwrap();
        let mut image = grey(3, 1);
        shape.draw(&mut image, (0, 0));
        let colours: Vec<u8> = image.data.chunks_exact(PIXEL).map(|p| p[0]).collect();
        assert_eq!(colours, [0xff, 0x80, 0x40]);

        let pixels = [[0x0f, 0x0f, 0x0f, 0x00], [0x0f, 0x0f, 0x0f, 0xff]];
        let shape = PointerShape::new(ShapeKind::MaskedColor, (2, 1), 8, pixels.concat()).unwrap();
        let mut image = grey(2, 1);
        shape.draw(&mut image, (0, 0));
        let colours: Vec<u8> = image.data.chunks_exact(PIXEL).map(|p| p[0]).collect();
        assert_eq!(colours, [0x0f, 0x8f]);
    }

    /// Pointers partly off the desktop are clipped.
    #[test]
    fn clips_pointers() {
        let shape = PointerShape::new(ShapeKind::Color, (2, 2), 8, vec![0xff; 16]).unwrap();
        let mut image = grey(2, 2);
        shape.draw(&mut image, (-1, 1));
        let colours: Vec<u8> = image.data.chunks_exact(PIXEL).map(|p| p[0]).collect();
        assert_eq!(colours, [0x80, 0x80, 0xff, 0x80]);
        shape.draw(&mut image, (5, 5));
        assert!(PointerShape::new(ShapeKind::Color, (2, 2), 8, vec![0; 15]).is_none());
    }
}
