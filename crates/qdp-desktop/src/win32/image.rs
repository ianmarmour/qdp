//! The desktop image Desktop Duplication delivers, turned upright, and the
//! pointer's image, which Desktop Duplication delivers apart from the desktop,
//! as RGBA for the client to draw.

/// Bytes per pixel of a 32-bit BGRA image.
const PIXEL: usize = 4;

/// A fully opaque alpha value.
const OPAQUE: u8 = 0xff;

/// Every bit of a colour channel: white, in each channel.
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

/// What one pointer pixel does to the desktop under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    /// Covers the desktop with this RGBA colour, blended by its alpha.
    Paint([u8; 4]),
    /// Inverts the desktop, which an RGBA image cannot show.
    Invert,
    /// Leaves the desktop alone.
    Transparent,
}

/// The colour inverting pixels are shown in.
const INVERT_COLOUR: [u8; 4] = [0, 0, 0, OPAQUE];

/// The colour of the outline around inverting pixels, which keeps them
/// visible on dark backgrounds.
const OUTLINE_COLOUR: [u8; 4] = [ALL_BITS, ALL_BITS, ALL_BITS, OPAQUE];

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
    /// `None` if it is empty or `data` is too short.
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
        if width == 0
            || height == 0
            || pitch < row_bytes
            || data.len() < pitch * (rows - 1) + row_bytes
        {
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

    /// Returns the width in pixels.
    pub(super) const fn width(&self) -> usize {
        self.width
    }

    /// Returns the height in pixels.
    pub(super) const fn height(&self) -> usize {
        self.height
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

    /// Returns what the pixel at `(x, y)` does to the desktop.
    fn effect(&self, x: usize, y: usize) -> Effect {
        match self.kind {
            ShapeKind::Monochrome => match (self.mask_bit(y, x), self.mask_bit(self.height + y, x))
            {
                (false, false) => Effect::Paint([0, 0, 0, OPAQUE]),
                (false, true) => Effect::Paint([ALL_BITS, ALL_BITS, ALL_BITS, OPAQUE]),
                (true, false) => Effect::Transparent,
                (true, true) => Effect::Invert,
            },
            ShapeKind::Color => {
                let [blue, green, red, alpha] = self.colour(x, y);
                Effect::Paint([red, green, blue, alpha])
            }
            ShapeKind::MaskedColor => match self.colour(x, y) {
                [blue, green, red, 0] => Effect::Paint([red, green, blue, OPAQUE]),
                [0, 0, 0, _] => Effect::Transparent,
                _ => Effect::Invert,
            },
        }
    }

    /// Returns the shape as RGBA pixels with straight alpha, row by row, as
    /// QDP sends pointer images (Section 9.2). Pixels that invert the
    /// desktop become black with a white outline.
    pub(super) fn to_rgba(&self) -> Vec<u8> {
        let effects: Vec<Effect> = (0..self.height)
            .flat_map(|y| (0..self.width).map(move |x| (x, y)))
            .map(|(x, y)| self.effect(x, y))
            .collect();
        let inverts_near = |x: usize, y: usize| {
            (y.saturating_sub(1)..=(y + 1).min(self.height - 1)).any(|near_y| {
                (x.saturating_sub(1)..=(x + 1).min(self.width - 1))
                    .any(|near_x| effects[near_y * self.width + near_x] == Effect::Invert)
            })
        };
        let mut rgba = Vec::with_capacity(effects.len() * PIXEL);
        for (index, effect) in effects.iter().enumerate() {
            let (x, y) = (index % self.width, index / self.width);
            let colour = match effect {
                Effect::Paint(colour) => *colour,
                Effect::Invert => INVERT_COLOUR,
                Effect::Transparent if inverts_near(x, y) => OUTLINE_COLOUR,
                Effect::Transparent => [0; 4],
            };
            rgba.extend_from_slice(&colour);
        }
        rgba
    }
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

    /// Returns the RGBA pixels of a shape.
    fn pixels(shape: &PointerShape) -> Vec<[u8; 4]> {
        shape
            .to_rgba()
            .chunks_exact(PIXEL)
            .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
            .collect()
    }

    /// Monochrome masks make black, white, transparency, and black for
    /// inversion, outlined in white.
    #[test]
    fn converts_monochrome_pointers() {
        // A 4 × 1 shape: AND mask 0011, XOR mask 0101.
        let shape = PointerShape::new(ShapeKind::Monochrome, (4, 2), 1, vec![0x30, 0x50]).unwrap();
        assert_eq!((shape.width(), shape.height()), (4, 1));
        assert_eq!(
            pixels(&shape),
            [OPAQUE_BLACK, OPAQUE_WHITE, OPAQUE_WHITE, OPAQUE_BLACK]
        );
        let lone = PointerShape::new(ShapeKind::Monochrome, (3, 2), 1, vec![0xe0, 0x00]).unwrap();
        assert_eq!(pixels(&lone), [[0; 4]; 3]);
    }

    /// Colour pointers keep their alpha; masked colour pointers replace or
    /// invert.
    #[test]
    fn converts_colour_pointers() {
        let shape = PointerShape::new(ShapeKind::Color, (1, 1), 4, vec![1, 2, 3, 4]).unwrap();
        assert_eq!(pixels(&shape), [[3, 2, 1, 4]]);
        let masked = [[1, 2, 3, 0], [0, 0, 0, OPAQUE], [9, 9, 9, OPAQUE]].concat();
        let shape = PointerShape::new(ShapeKind::MaskedColor, (3, 1), 12, masked).unwrap();
        assert_eq!(
            pixels(&shape),
            [[3, 2, 1, OPAQUE], OPAQUE_WHITE, OPAQUE_BLACK]
        );
    }

    /// Empty and short shapes are refused.
    #[test]
    fn refuses_bad_shapes() {
        assert!(PointerShape::new(ShapeKind::Color, (2, 2), 8, vec![0; 15]).is_none());
        assert!(PointerShape::new(ShapeKind::Color, (0, 0), 0, Vec::new()).is_none());
    }

    /// Opaque black.
    const OPAQUE_BLACK: [u8; 4] = [0, 0, 0, OPAQUE];

    /// Opaque white.
    const OPAQUE_WHITE: [u8; 4] = [ALL_BITS, ALL_BITS, ALL_BITS, OPAQUE];
}
