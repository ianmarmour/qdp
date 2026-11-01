//! Which reconstructed frames the encoder keeps, and in which of AV1's eight
//! reference slots, so that each frame is predicted from the one before it
//! or, to recover from a loss, from an earlier frame the receiver decoded.
//!
//! Where the driver lets frames refer to any slot, each inter frame
//! refreshes one slot in turn, so the last eight frames stay in the
//! decoder's slots. A receiver that lost a frame has decoded nothing since
//! its last decoded frame, so that frame is still in the slot it was written
//! to, and a recovery frame predicted from it decodes there. Where the
//! driver follows only the previous frame, every frame refreshes every
//! slot, and recovery needs a key frame.

use crate::KeyFrameRequest;
use crate::headers::ALL_REFERENCE_SLOTS;

/// AV1's reference slots (`NUM_REF_FRAMES`).
pub(super) const REFERENCE_SLOTS: usize = 8;

/// The reconstructed surfaces: one for each slot, and one for the frame being
/// encoded, which replaces a slot's frame only once it is encoded.
pub(super) const RECONSTRUCTED_SURFACES: usize = REFERENCE_SLOTS + 1;

/// How inter frames use the reference slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SlotUse {
    /// Every inter frame refreshes every slot and is predicted from the one
    /// before it, for drivers that keep only the previous frame.
    PreviousOnly,
    /// Each inter frame refreshes one slot in turn, so that a frame may be
    /// predicted from any of the last eight.
    Rotating,
}

/// A frame a reference slot holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SlotFrame {
    /// The frame's input index.
    pub(super) input_index: u64,
    /// The reconstructed surface holding it.
    pub(super) surface: usize,
}

/// How the next frame is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Plan {
    /// A key frame, which refreshes every slot.
    Key {
        /// The reconstructed surface it is encoded into.
        surface: usize,
    },
    /// An inter frame.
    Inter {
        /// The slot of the frame it is predicted from.
        reference_slot: u8,
        /// The slots it refreshes (`refresh_frame_flags`).
        refresh_frame_flags: u8,
        /// The reconstructed surface it is encoded into.
        surface: usize,
        /// The input index of the frame it recovers from, if it answers a
        /// loss.
        recovered_from: Option<u64>,
    },
}

impl Plan {
    /// Returns the reconstructed surface the frame is encoded into.
    pub(super) fn surface(self) -> usize {
        match self {
            Self::Key { surface } | Self::Inter { surface, .. } => surface,
        }
    }
}

/// The frames in the reference slots, as a receiver holds them after
/// decoding every frame encoded so far.
#[derive(Debug, Clone)]
pub(super) struct ReferenceSlots {
    /// How inter frames use the slots.
    slot_use: SlotUse,
    /// The frame in each slot, once a key frame filled them.
    slots: [Option<SlotFrame>; REFERENCE_SLOTS],
    /// The slot the next inter frame refreshes, when they rotate.
    next: usize,
    /// The slot holding the frame encoded last, if any.
    last: Option<usize>,
}

impl ReferenceSlots {
    /// Returns empty slots, used as `slot_use` says.
    pub(super) fn new(slot_use: SlotUse) -> Self {
        Self {
            slot_use,
            slots: [None; REFERENCE_SLOTS],
            next: 0,
            last: None,
        }
    }

    /// Returns how to encode the next frame as `request` asks: a key frame
    /// when there is no frame to predict it from.
    pub(super) fn plan(&self, request: KeyFrameRequest) -> Plan {
        let surface = self.free_surface();
        let inter = |reference_slot: usize, recovered_from: Option<u64>| Plan::Inter {
            reference_slot: reference_slot as u8,
            refresh_frame_flags: match self.slot_use {
                SlotUse::PreviousOnly => ALL_REFERENCE_SLOTS,
                SlotUse::Rotating => 1 << self.next,
            },
            surface,
            recovered_from,
        };
        let key = Plan::Key { surface };
        match request {
            KeyFrameRequest::Auto => self.last.map_or(key, |slot| inter(slot, None)),
            KeyFrameRequest::Force => key,
            KeyFrameRequest::RecoverFrom(base) => match self.slot_use {
                SlotUse::PreviousOnly => key,
                SlotUse::Rotating => self
                    .slot_holding(base)
                    .map_or(key, |slot| inter(slot, Some(base))),
            },
        }
    }

    /// Records that frame `input_index` was encoded as `plan` said.
    pub(super) fn encoded(&mut self, plan: Plan, input_index: u64) {
        let frame = SlotFrame {
            input_index,
            surface: plan.surface(),
        };
        let refreshed = match plan {
            Plan::Key { .. } => ALL_REFERENCE_SLOTS,
            Plan::Inter {
                refresh_frame_flags,
                ..
            } => refresh_frame_flags,
        };
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if refreshed & (1 << index) != 0 {
                *slot = Some(frame);
            }
        }
        let first = refreshed.trailing_zeros() as usize;
        self.last = Some(first);
        self.next = match plan {
            // After a key frame, which every slot holds, the rotation starts
            // again from its first slot.
            Plan::Key { .. } => 0,
            Plan::Inter { .. } => (self.next + 1) % REFERENCE_SLOTS,
        };
    }

    /// Forgets every frame, after one failed, so that the next is a key
    /// frame.
    pub(super) fn clear(&mut self) {
        *self = Self::new(self.slot_use);
    }

    /// Returns the reconstructed surface in each slot, if the slot holds a
    /// frame.
    pub(super) fn surfaces(&self) -> [Option<usize>; REFERENCE_SLOTS] {
        self.slots.map(|slot| slot.map(|frame| frame.surface))
    }

    /// Returns the slot holding frame `input_index`, if one does.
    fn slot_holding(&self, input_index: u64) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.is_some_and(|frame| frame.input_index == input_index))
    }

    /// Returns a reconstructed surface no slot holds. There is always one,
    /// since there is one surface more than there are slots.
    fn free_surface(&self) -> usize {
        (0..RECONSTRUCTED_SURFACES)
            .find(|&surface| {
                !self
                    .slots
                    .iter()
                    .any(|slot| slot.is_some_and(|frame| frame.surface == surface))
            })
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes `count` frames as `request` asks each, from `first` on,
    /// returning their plans.
    fn encode(
        slots: &mut ReferenceSlots,
        first: u64,
        count: u64,
        request: KeyFrameRequest,
    ) -> Vec<Plan> {
        (first..first + count)
            .map(|index| {
                let plan = slots.plan(request);
                slots.encoded(plan, index);
                plan
            })
            .collect()
    }

    /// The first frame is a key frame, and each later one is predicted from
    /// the one before, refreshing one slot in turn, into a surface no slot
    /// holds.
    #[test]
    fn rotates_through_the_slots() {
        let mut slots = ReferenceSlots::new(SlotUse::Rotating);
        let plans = encode(&mut slots, 0, 10, KeyFrameRequest::Auto);
        assert!(matches!(plans[0], Plan::Key { .. }));
        let mut previous_slot = 0;
        for (index, plan) in plans.iter().enumerate().skip(1) {
            let Plan::Inter {
                reference_slot,
                refresh_frame_flags,
                surface,
                recovered_from,
            } = *plan
            else {
                panic!("frame {index} is not an inter frame");
            };
            assert_eq!(reference_slot, previous_slot);
            assert_eq!(refresh_frame_flags, 1 << ((index - 1) % REFERENCE_SLOTS));
            assert_eq!(recovered_from, None);
            assert_ne!(surface, plans[index - 1].surface());
            previous_slot = refresh_frame_flags.trailing_zeros() as u8;
        }
        // The slots hold the last eight frames, each in its own surface.
        let mut surfaces = slots.surfaces().map(Option::unwrap).to_vec();
        surfaces.sort_unstable();
        surfaces.dedup();
        assert_eq!(surfaces.len(), REFERENCE_SLOTS);
    }

    /// A recovery frame is predicted from the slot its base was written to,
    /// while the base is among the last eight frames, and is a key frame
    /// once it is not.
    #[test]
    fn recovers_from_recent_frames() {
        let mut slots = ReferenceSlots::new(SlotUse::Rotating);
        encode(&mut slots, 0, 6, KeyFrameRequest::Auto);
        // Frame 3, the third inter frame, refreshed slot 2.
        let plan = slots.plan(KeyFrameRequest::RecoverFrom(3));
        assert!(matches!(
            plan,
            Plan::Inter {
                reference_slot: 2,
                recovered_from: Some(3),
                ..
            }
        ));
        slots.encoded(plan, 6);
        // The key frame is still in the slots the inter frames have not
        // reached.
        assert!(matches!(
            slots.plan(KeyFrameRequest::RecoverFrom(0)),
            Plan::Inter {
                recovered_from: Some(0),
                ..
            }
        ));
        encode(&mut slots, 7, 8, KeyFrameRequest::Auto);
        assert!(matches!(
            slots.plan(KeyFrameRequest::RecoverFrom(3)),
            Plan::Key { .. }
        ));
        assert!(matches!(
            slots.plan(KeyFrameRequest::Force),
            Plan::Key { .. }
        ));
    }

    /// Drivers that follow only the previous frame refresh every slot and
    /// recover with key frames; a failed frame forgets every slot.
    #[test]
    fn follows_only_the_previous_frame() {
        let mut slots = ReferenceSlots::new(SlotUse::PreviousOnly);
        let plans = encode(&mut slots, 0, 4, KeyFrameRequest::Auto);
        for plan in &plans[1..] {
            assert!(matches!(
                plan,
                Plan::Inter {
                    reference_slot: 0,
                    refresh_frame_flags: ALL_REFERENCE_SLOTS,
                    ..
                }
            ));
        }
        assert_ne!(plans[2].surface(), plans[3].surface());
        assert!(matches!(
            slots.plan(KeyFrameRequest::RecoverFrom(2)),
            Plan::Key { .. }
        ));
        slots.clear();
        assert!(matches!(
            slots.plan(KeyFrameRequest::Auto),
            Plan::Key { .. }
        ));
    }
}
