//! Composition of the motion and refinement layers of one monitor (Section 6.9).

use crate::message::{BlockGrid, BlockSet, FrameNumber};

/// Which layer a composition block is shown from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockSource {
    /// The latest decoded motion-layer frame.
    Motion,
    /// The refined image.
    Refined,
}

/// The per-block bookkeeping that decides which layer each block is shown from.
///
/// The caller keeps the pixels: the latest motion frame and the refined image, into
/// which it copies each covered block of a decoded refinement frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composition {
    /// For each block, the most recent decoded motion frame that changed it.
    changed: Vec<FrameNumber>,
    /// For each block, the Base Frame Number of the most recent refinement frame
    /// that covered it.
    refined: Vec<Option<FrameNumber>>,
}

impl Composition {
    /// Creates the composition state of a monitor divided into `grid`, with no
    /// block refined.
    pub fn new(grid: BlockGrid) -> Self {
        Self {
            changed: vec![FrameNumber::NONE; grid.len()],
            refined: vec![None; grid.len()],
        }
    }

    /// Records a decoded motion frame and its changed blocks. When `resets` is true
    /// (Frame Number 1, or a key frame that ends a loss), every block is marked
    /// changed whatever the map says.
    pub fn on_motion_frame(&mut self, frame_number: FrameNumber, changed: &BlockSet, resets: bool) {
        for (block, last_change) in self.changed.iter_mut().enumerate() {
            if resets || changed.contains(block) {
                *last_change = frame_number;
            }
        }
    }

    /// Records a decoded refinement frame. The caller copies the pixels of each
    /// block in `covered` into the refined image; pixels of other blocks must not be
    /// used.
    pub fn on_refinement_frame(&mut self, base_frame_number: FrameNumber, covered: &BlockSet) {
        for block in covered.iter() {
            if let Some(refined) = self.refined.get_mut(block) {
                *refined = Some(base_frame_number);
            }
        }
    }

    /// Returns which layer block `block` is shown from.
    pub fn source(&self, block: usize) -> BlockSource {
        match (self.refined.get(block), self.changed.get(block)) {
            (Some(Some(refined)), Some(changed)) if refined >= changed => BlockSource::Refined,
            _ => BlockSource::Motion,
        }
    }

    /// Forgets every refinement, as when a monitor's streams are replaced.
    pub fn reset(&mut self) {
        self.refined.fill(None);
        self.changed.fill(FrameNumber::NONE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::VarInt;

    /// Returns a frame number.
    fn frame(number: u32) -> FrameNumber {
        FrameNumber(VarInt::from_u32(number))
    }

    /// Returns a set of `len` blocks containing `blocks`.
    fn set(len: usize, blocks: &[usize]) -> BlockSet {
        let mut set = BlockSet::empty(len);
        for &block in blocks {
            set.insert(block);
        }
        set
    }

    /// A refinement shows until a later motion frame changes the block.
    #[test]
    fn refinement_until_change() {
        let grid = BlockGrid::for_monitor(128, 64);
        let mut composition = Composition::new(grid);
        composition.on_motion_frame(frame(1), &BlockSet::empty(2), true);
        assert_eq!(composition.source(0), BlockSource::Motion);

        composition.on_refinement_frame(frame(1), &set(2, &[0, 1]));
        assert_eq!(composition.source(0), BlockSource::Refined);
        assert_eq!(composition.source(1), BlockSource::Refined);

        composition.on_motion_frame(frame(2), &set(2, &[1]), false);
        assert_eq!(composition.source(0), BlockSource::Refined);
        assert_eq!(composition.source(1), BlockSource::Motion);

        // A refinement of an older source does not override a newer change.
        composition.on_refinement_frame(frame(1), &set(2, &[1]));
        assert_eq!(composition.source(1), BlockSource::Motion);
        composition.on_refinement_frame(frame(2), &set(2, &[1]));
        assert_eq!(composition.source(1), BlockSource::Refined);
    }

    /// A resetting motion frame and a stream replacement drop refinements.
    #[test]
    fn resets() {
        let grid = BlockGrid::for_monitor(64, 64);
        let mut composition = Composition::new(grid);
        composition.on_motion_frame(frame(1), &BlockSet::empty(1), true);
        composition.on_refinement_frame(frame(1), &set(1, &[0]));
        composition.on_motion_frame(frame(5), &BlockSet::empty(1), true);
        assert_eq!(composition.source(0), BlockSource::Motion);

        composition.on_refinement_frame(frame(5), &set(1, &[0]));
        composition.reset();
        assert_eq!(composition.source(0), BlockSource::Motion);
    }
}
