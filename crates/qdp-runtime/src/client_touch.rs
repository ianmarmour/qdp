//! The client's touchscreen: the fingers touching the window, as
//! `TOUCHSCREEN` reports listing every contact (Section 8.4.4).

use qdp::input::ReportKind;
use qdp::message::{MAX_CONTACTS, TouchContact, TouchContactFlags, TouchscreenReport};

use crate::host_input::TouchPhase;

/// The fingers touching, each with the contact the host knows it by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Touchscreen {
    /// The fingers, by the platform's identifier, in the order they touched.
    fingers: Vec<(u64, TouchContact)>,
}

impl Touchscreen {
    /// Applies one finger's change at a desktop position in 1/256 pixel,
    /// returning the report listing the contacts and its kind, or `None` if
    /// nothing the host sees changed.
    pub(crate) fn touch(
        &mut self,
        finger: u64,
        phase: TouchPhase,
        position: (i32, i32),
        pressure: u16,
    ) -> Option<(TouchscreenReport, ReportKind)> {
        let index = self.fingers.iter().position(|(id, _)| *id == finger);
        match (phase, index) {
            (TouchPhase::Down, None) => {
                if self.fingers.len() >= MAX_CONTACTS {
                    return None;
                }
                let contact = TouchContact {
                    id: self.free_contact_id(),
                    flags: TouchContactFlags::default(),
                    x: position.0,
                    y: position.1,
                    pressure,
                };
                self.fingers.push((finger, contact));
                Some((self.report(), ReportKind::Transition))
            }
            (TouchPhase::Down | TouchPhase::Move, Some(index)) => {
                let contact = &mut self.fingers[index].1;
                let moved = TouchContact {
                    x: position.0,
                    y: position.1,
                    pressure,
                    ..*contact
                };
                if moved == *contact {
                    return None;
                }
                *contact = moved;
                Some((self.report(), ReportKind::Motion))
            }
            (TouchPhase::Up, Some(index)) => {
                self.fingers.remove(index);
                Some((self.report(), ReportKind::Transition))
            }
            (TouchPhase::Cancel, Some(index)) => {
                // A cancelled contact appears once more, flagged, then not at
                // all.
                self.fingers[index].1.flags = TouchContactFlags::CANCELLED;
                let report = self.report();
                self.fingers.remove(index);
                Some((report, ReportKind::Transition))
            }
            (TouchPhase::Move | TouchPhase::Up | TouchPhase::Cancel, None) => None,
        }
    }

    /// Cancels every contact, as when the window loses focus, returning the
    /// report that ends them, or `None` if nothing touches.
    pub(crate) fn cancel_all(&mut self) -> Option<TouchscreenReport> {
        if self.fingers.is_empty() {
            return None;
        }
        for (_, contact) in &mut self.fingers {
            contact.flags = TouchContactFlags::CANCELLED;
        }
        let report = self.report();
        self.fingers.clear();
        Some(report)
    }

    /// Returns the report listing every contact.
    fn report(&self) -> TouchscreenReport {
        TouchscreenReport {
            contacts: self.fingers.iter().map(|(_, contact)| *contact).collect(),
        }
    }

    /// Returns the lowest Contact ID no finger uses.
    fn free_contact_id(&self) -> u8 {
        (0..=u8::MAX)
            .find(|id| self.fingers.iter().all(|(_, contact)| contact.id != *id))
            .unwrap_or(u8::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns the Contact IDs and flags a report lists.
    fn contacts(report: &TouchscreenReport) -> Vec<(u8, TouchContactFlags)> {
        report
            .contacts
            .iter()
            .map(|contact| (contact.id, contact.flags))
            .collect()
    }

    /// Fingers get the lowest free Contact IDs, move without changing the
    /// list, and leave it when they lift.
    #[test]
    fn tracks_fingers() {
        let mut screen = Touchscreen::default();
        let (report, kind) = screen.touch(70, TouchPhase::Down, (10, 20), 0).unwrap();
        assert_eq!(kind, ReportKind::Transition);
        assert_eq!(contacts(&report), [(0, TouchContactFlags::default())]);
        screen.touch(71, TouchPhase::Down, (30, 40), 0).unwrap();
        assert_eq!(screen.touch(70, TouchPhase::Move, (10, 20), 0), None);
        let (report, kind) = screen.touch(70, TouchPhase::Move, (11, 20), 0).unwrap();
        assert_eq!(kind, ReportKind::Motion);
        assert_eq!((report.contacts[0].x, report.contacts[1].id), (11, 1));
        let (report, _) = screen.touch(70, TouchPhase::Up, (11, 20), 0).unwrap();
        assert_eq!(contacts(&report), [(1, TouchContactFlags::default())]);
        let (report, _) = screen.touch(72, TouchPhase::Down, (0, 0), 0).unwrap();
        assert_eq!(report.contacts[1].id, 0);
        assert_eq!(screen.touch(99, TouchPhase::Move, (0, 0), 0), None);
    }

    /// A cancelled contact is listed once with its flag, then dropped, and
    /// losing focus cancels every contact.
    #[test]
    fn cancels_contacts() {
        let mut screen = Touchscreen::default();
        screen.touch(1, TouchPhase::Down, (0, 0), 0).unwrap();
        screen.touch(2, TouchPhase::Down, (5, 5), 0).unwrap();
        let (report, _) = screen.touch(1, TouchPhase::Cancel, (0, 0), 0).unwrap();
        assert_eq!(
            contacts(&report),
            [
                (0, TouchContactFlags::CANCELLED),
                (1, TouchContactFlags::default())
            ]
        );
        let report = screen.cancel_all().unwrap();
        assert_eq!(contacts(&report), [(1, TouchContactFlags::CANCELLED)]);
        assert_eq!(screen.cancel_all(), None);
    }

    /// No more than 16 fingers are tracked.
    #[test]
    fn limits_contacts() {
        let mut screen = Touchscreen::default();
        for finger in 0..20 {
            screen.touch(finger, TouchPhase::Down, (0, 0), 0);
        }
        assert_eq!(screen.report().contacts.len(), MAX_CONTACTS);
    }
}
