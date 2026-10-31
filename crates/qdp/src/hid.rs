//! HID report descriptors [HID-CLASS], as passthrough devices carry them
//! (Section 8.2.5): validating them against the limits of the HID class
//! definition, finding the usages of their top-level collections for input
//! policy, measuring their reports, and describing their input reports at
//! rest.
//!
//! The parser runs outside any kernel and accepts only descriptors whose
//! items are well formed, whose collections and pushes balance, and whose
//! sizes stay within the limits operating systems enforce.

use std::collections::BTreeMap;

use thiserror::Error;

use crate::message::{MAX_PASSTHROUGH_REPORT_LENGTH, MAX_REPORT_DESCRIPTOR_LENGTH};

/// The prefix of a long item, whose data no standard item uses.
const LONG_ITEM_PREFIX: u8 = 0xfe;

/// The deepest collections may nest.
const MAX_COLLECTION_DEPTH: usize = 32;

/// The most global states `Push` may save at once.
const MAX_PUSH_DEPTH: usize = 16;

/// The largest Report Size, in bits, as operating systems accept it.
const MAX_REPORT_SIZE: u32 = 256;

/// The largest Report Count, as operating systems accept it.
const MAX_REPORT_COUNT: u32 = 12_288;

/// The bits in a byte.
const BITS_PER_BYTE: u32 = 8;

/// The Generic Desktop usage page.
const GENERIC_DESKTOP: u16 = 0x01;

/// The Simulation Controls usage page.
const SIMULATION_CONTROLS: u16 = 0x02;

/// The Keyboard/Keypad usage page.
const KEYBOARD_PAGE: u16 = 0x07;

/// The Consumer usage page.
const CONSUMER: u16 = 0x0c;

/// The Digitizers usage page.
const DIGITIZERS: u16 = 0x0d;

/// The FIDO Alliance usage page.
const FIDO: u16 = 0xf1d0;

/// The first vendor-defined usage page.
const FIRST_VENDOR_PAGE: u16 = 0xff00;

/// The Generic Desktop axes that rest at their centre: X, Y, Z, Rx, Ry and Rz.
const CENTRED_AXES: std::ops::RangeInclusive<u16> = 0x30..=0x35;

/// The Generic Desktop Hat Switch usage.
const HAT_SWITCH: u16 = 0x39;

/// The Input, Output or Feature item flag of a constant field: padding.
const FLAG_CONSTANT: u32 = 0x01;

/// The main item flag of a variable field, one value per usage, rather than
/// an array of the usages that are on.
const FLAG_VARIABLE: u32 = 0x02;

/// The main item flag of a relative field.
const FLAG_RELATIVE: u32 = 0x04;

/// The main item flag of a field with a null state: a value outside its
/// logical range meaning no value.
const FLAG_NULL_STATE: u32 = 0x40;

/// Why a report descriptor is not accepted.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DescriptorError {
    /// The descriptor is longer than Section 8.2.1 allows.
    #[error("the report descriptor is longer than {MAX_REPORT_DESCRIPTOR_LENGTH} bytes")]
    TooLong,
    /// An item runs past the end of the descriptor.
    #[error("an item runs past the end of the report descriptor")]
    Truncated,
    /// An item has the reserved type.
    #[error("an item has the reserved type")]
    ReservedItem,
    /// An End Collection has no matching Collection, or a Collection is not
    /// ended.
    #[error("the report descriptor's collections do not balance")]
    UnbalancedCollections,
    /// Collections nest deeper than allowed.
    #[error("collections nest more than {MAX_COLLECTION_DEPTH} deep")]
    TooDeep,
    /// A Pop has no matching Push, or pushes nest deeper than allowed.
    #[error("the report descriptor's pushes and pops do not balance")]
    UnbalancedPush,
    /// A Report ID is 0, which is reserved.
    #[error("a Report ID is 0")]
    ZeroReportId,
    /// A Report Size or Report Count is beyond what operating systems accept.
    #[error("a Report Size or Report Count is too large")]
    FieldTooLarge,
    /// A field comes before the first Report ID of a descriptor that uses
    /// report IDs.
    #[error("a field comes before the first Report ID")]
    FieldWithoutReportId,
    /// An input report is longer than Section 8.2.1 allows.
    #[error("an input report is longer than {MAX_PASSTHROUGH_REPORT_LENGTH} bytes")]
    ReportTooLong,
    /// The descriptor declares no top-level collection.
    #[error("the report descriptor declares no collection")]
    NoCollection,
}

/// A usage: a page and an ID within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Usage {
    /// The usage page.
    pub page: u16,
    /// The usage ID.
    pub id: u16,
}

impl Usage {
    /// The usage that stands for a top-level collection with none, and for
    /// fields outside every collection, which no policy can tell apart: it
    /// is of the Undefined page, so its class is
    /// [`CollectionClass::Other`].
    pub const UNNAMED: Self = Self { page: 0, id: 0 };
}

/// What a top-level collection is, for a host's input policy (Section
/// 8.2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CollectionClass {
    /// A joystick, gamepad, multi-axis controller or simulation control.
    GameController,
    /// A keyboard or keypad.
    Keyboard,
    /// A mouse.
    Mouse,
    /// A pointer.
    Pointer,
    /// A digitizer: a pen, touchscreen or touchpad.
    Digitizer,
    /// System controls such as power and sleep.
    SystemControl,
    /// Consumer controls such as media keys.
    ConsumerControl,
    /// A FIDO authenticator.
    Fido,
    /// A vendor-defined collection.
    VendorDefined,
    /// Anything else.
    Other,
}

impl Usage {
    /// Returns what a top-level collection with this usage is.
    pub fn class(self) -> CollectionClass {
        match (self.page, self.id) {
            (GENERIC_DESKTOP, 0x04 | 0x05 | 0x08) | (SIMULATION_CONTROLS, _) => {
                CollectionClass::GameController
            }
            (GENERIC_DESKTOP, 0x06 | 0x07) | (KEYBOARD_PAGE, _) => CollectionClass::Keyboard,
            (GENERIC_DESKTOP, 0x02) => CollectionClass::Mouse,
            (GENERIC_DESKTOP, 0x01) => CollectionClass::Pointer,
            (GENERIC_DESKTOP, 0x80) => CollectionClass::SystemControl,
            (DIGITIZERS, _) => CollectionClass::Digitizer,
            (CONSUMER, _) => CollectionClass::ConsumerControl,
            (FIDO, _) => CollectionClass::Fido,
            (page, _) if page >= FIRST_VENDOR_PAGE => CollectionClass::VendorDefined,
            _ => CollectionClass::Other,
        }
    }
}

/// Which reports a main item's fields belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReportType {
    /// Input reports, from the device.
    Input,
    /// Output reports, to the device.
    Output,
    /// Feature reports, read and written on request.
    Feature,
}

/// The usages a field's elements take: listed, or a range.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FieldUsages {
    /// Usages listed one by one; the last repeats for further elements.
    List(Vec<Usage>),
    /// A range of usages, the first for the first element.
    Range(Usage, Usage),
}

impl FieldUsages {
    /// Returns the usage of element `index`, if the field names any.
    fn usage(&self, index: u32) -> Option<Usage> {
        match self {
            Self::List(usages) => usages
                .get(index as usize)
                .or_else(|| usages.last())
                .copied(),
            Self::Range(first, last) => {
                let id = u32::from(first.id)
                    .saturating_add(index)
                    .min(u32::from(last.id));
                Some(Usage {
                    page: first.page,
                    id: id as u16,
                })
            }
        }
    }
}

/// One Input main item: a run of equally sized elements.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Field {
    /// Where it starts in its report, in bits after the report ID byte.
    offset: u32,
    /// The bits per element.
    size: u32,
    /// The elements.
    count: u32,
    /// The main item's flags.
    flags: u32,
    /// The smallest value an element takes.
    logical_minimum: i32,
    /// The largest value an element takes.
    logical_maximum: i32,
    /// The usages of its elements.
    usages: FieldUsages,
}

impl Field {
    /// Returns the value element `index` has with its control at rest.
    fn rest_value(&self, index: u32) -> i64 {
        let (minimum, maximum) = (
            i64::from(self.logical_minimum),
            i64::from(self.logical_maximum),
        );
        let within = |value: i64| value.clamp(minimum, maximum.max(minimum));
        // Padding, relative motion and arrays of pressed usages rest at 0.
        if self.flags & FLAG_CONSTANT != 0
            || self.flags & FLAG_RELATIVE != 0
            || self.flags & FLAG_VARIABLE == 0
        {
            return 0;
        }
        let Some(usage) = self.usages.usage(index) else {
            return within(0);
        };
        match (usage.page, usage.id) {
            (GENERIC_DESKTOP, id) if CENTRED_AXES.contains(&id) => {
                if minimum < 0 {
                    within(0)
                } else {
                    (minimum + maximum + 1) / 2
                }
            }
            (GENERIC_DESKTOP, HAT_SWITCH) if self.flags & FLAG_NULL_STATE != 0 => {
                let largest = (1_i64 << self.size.min(32)) - 1;
                if maximum < largest {
                    maximum + 1
                } else {
                    minimum - 1
                }
            }
            _ => within(0),
        }
    }
}

/// The layout of one report: its length and, for input reports, its
/// fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ReportLayout {
    /// The report's length in bits, after its report ID byte.
    bits: u32,
    /// The fields of an input report.
    fields: Vec<Field>,
}

impl ReportLayout {
    /// Returns the report's length in bytes, after its report ID byte.
    fn length(&self) -> usize {
        self.bits.div_ceil(BITS_PER_BYTE) as usize
    }
}

/// The global item state, which Push and Pop save and restore.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Globals {
    /// The usage page of usages given without one.
    usage_page: u16,
    /// The smallest value of the next fields.
    logical_minimum: i32,
    /// The largest value of the next fields, read as signed.
    logical_maximum: i32,
    /// The same, read as unsigned, for fields whose minimum is not negative.
    unsigned_logical_maximum: u32,
    /// The bits per element of the next fields.
    report_size: u32,
    /// The elements of the next fields.
    report_count: u32,
    /// The report ID of the next fields, or 0 before any.
    report_id: u8,
}

/// The local item state, which every main item clears.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Locals {
    /// The usages listed.
    usages: Vec<Usage>,
    /// The first usage of a range.
    minimum: Option<Usage>,
    /// The last usage of a range.
    maximum: Option<Usage>,
}

impl Locals {
    /// Returns the usages a field takes.
    fn field_usages(&self) -> FieldUsages {
        match (self.minimum, self.maximum) {
            (Some(first), Some(last)) if self.usages.is_empty() => FieldUsages::Range(first, last),
            _ => FieldUsages::List(self.usages.clone()),
        }
    }

    /// Returns the usage a collection takes: its first.
    fn collection_usage(&self) -> Option<Usage> {
        self.usages.first().copied().or(self.minimum)
    }
}

/// Returns an item's data as an unsigned value.
fn unsigned(data: &[u8]) -> u32 {
    data.iter()
        .rev()
        .fold(0, |value, &byte| (value << BITS_PER_BYTE) | u32::from(byte))
}

/// Returns an item's data as a signed value, extending its sign.
fn signed(data: &[u8]) -> i32 {
    match data.len() {
        0 => 0,
        1 => i32::from(data[0] as i8),
        2 => i32::from(i16::from_le_bytes([data[0], data[1]])),
        _ => unsigned(data) as i32,
    }
}

/// Returns a usage given in a Usage, Usage Minimum or Usage Maximum item: a
/// four-byte item names its own page.
fn usage(data: &[u8], page: u16) -> Usage {
    let value = unsigned(data);
    if data.len() == 4 {
        Usage {
            page: (value >> 16) as u16,
            id: value as u16,
        }
    } else {
        Usage {
            page,
            id: value as u16,
        }
    }
}

/// A parsed, validated report descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportDescriptor {
    /// The usages of the top-level collections, in order.
    collections: Vec<Usage>,
    /// Whether reports start with a report ID.
    uses_report_ids: bool,
    /// The layout of each report, by type and report ID.
    reports: BTreeMap<(ReportType, u8), ReportLayout>,
}

impl ReportDescriptor {
    /// Parses and validates a report descriptor.
    pub fn parse(descriptor: &[u8]) -> Result<Self, DescriptorError> {
        if descriptor.len() > MAX_REPORT_DESCRIPTOR_LENGTH {
            return Err(DescriptorError::TooLong);
        }
        let mut parsed = Self {
            collections: Vec::new(),
            uses_report_ids: false,
            reports: BTreeMap::new(),
        };
        let mut globals = Globals::default();
        let mut pushed: Vec<Globals> = Vec::new();
        let mut locals = Locals::default();
        let mut depth = 0;
        let mut rest = descriptor;
        while let Some((&prefix, after)) = rest.split_first() {
            if prefix == LONG_ITEM_PREFIX {
                let [length, _tag, ..] = after else {
                    return Err(DescriptorError::Truncated);
                };
                let length = usize::from(*length);
                rest = after.get(2 + length..).ok_or(DescriptorError::Truncated)?;
                continue;
            }
            let length = match prefix & 0x03 {
                3 => 4,
                size => usize::from(size),
            };
            let data = after.get(..length).ok_or(DescriptorError::Truncated)?;
            rest = &after[length..];
            let tag = prefix >> 4;
            match (prefix >> 2) & 0x03 {
                // Main items.
                0 => {
                    if depth == 0 && matches!(tag, 0x8 | 0x9 | 0xb) {
                        // A field outside every collection still reaches
                        // the host, so the policy must see it.
                        parsed.add_collection(Usage::UNNAMED);
                    }
                    match tag {
                        0x8 => parsed.add_field(ReportType::Input, &globals, &locals, data)?,
                        0x9 => parsed.add_field(ReportType::Output, &globals, &locals, data)?,
                        0xb => parsed.add_field(ReportType::Feature, &globals, &locals, data)?,
                        0xa => {
                            if depth == 0 {
                                parsed.add_collection(
                                    locals.collection_usage().unwrap_or(Usage::UNNAMED),
                                );
                            }
                            depth += 1;
                            if depth > MAX_COLLECTION_DEPTH {
                                return Err(DescriptorError::TooDeep);
                            }
                        }
                        0xc => {
                            depth = depth
                                .checked_sub(1)
                                .ok_or(DescriptorError::UnbalancedCollections)?;
                        }
                        _ => {}
                    }
                    locals = Locals::default();
                }
                // Global items.
                1 => match tag {
                    0x0 => globals.usage_page = unsigned(data) as u16,
                    0x1 => globals.logical_minimum = signed(data),
                    0x2 => {
                        globals.logical_maximum = signed(data);
                        globals.unsigned_logical_maximum = unsigned(data);
                    }
                    0x7 => globals.report_size = unsigned(data),
                    0x8 => {
                        let id = unsigned(data);
                        if id == 0 || id > u32::from(u8::MAX) {
                            return Err(DescriptorError::ZeroReportId);
                        }
                        globals.report_id = id as u8;
                        parsed.uses_report_ids = true;
                    }
                    0x9 => globals.report_count = unsigned(data),
                    0xa => {
                        if pushed.len() >= MAX_PUSH_DEPTH {
                            return Err(DescriptorError::UnbalancedPush);
                        }
                        pushed.push(globals);
                    }
                    0xb => globals = pushed.pop().ok_or(DescriptorError::UnbalancedPush)?,
                    _ => {}
                },
                // Local items.
                2 => match tag {
                    0x0 => locals.usages.push(usage(data, globals.usage_page)),
                    0x1 => locals.minimum = Some(usage(data, globals.usage_page)),
                    0x2 => locals.maximum = Some(usage(data, globals.usage_page)),
                    _ => {}
                },
                _ => return Err(DescriptorError::ReservedItem),
            }
        }
        if depth != 0 {
            return Err(DescriptorError::UnbalancedCollections);
        }
        if parsed.collections.is_empty() {
            return Err(DescriptorError::NoCollection);
        }
        let unnumbered = parsed.reports.keys().any(|(_, report_id)| *report_id == 0);
        if parsed.uses_report_ids && unnumbered {
            return Err(DescriptorError::FieldWithoutReportId);
        }
        let too_long = parsed.reports.iter().any(|((kind, _), layout)| {
            *kind == ReportType::Input && layout.length() > MAX_PASSTHROUGH_REPORT_LENGTH
        });
        if too_long {
            return Err(DescriptorError::ReportTooLong);
        }
        Ok(parsed)
    }

    /// Adds a main item's fields to their report.
    fn add_field(
        &mut self,
        kind: ReportType,
        globals: &Globals,
        locals: &Locals,
        data: &[u8],
    ) -> Result<(), DescriptorError> {
        if globals.report_size > MAX_REPORT_SIZE || globals.report_count > MAX_REPORT_COUNT {
            return Err(DescriptorError::FieldTooLarge);
        }
        let layout = self.reports.entry((kind, globals.report_id)).or_default();
        let offset = layout.bits;
        layout.bits = layout
            .bits
            .checked_add(globals.report_size * globals.report_count)
            .ok_or(DescriptorError::ReportTooLong)?;
        if kind == ReportType::Input {
            // A non-negative minimum means an unsigned field, whose maximum
            // some devices encode in too few bytes to read as signed.
            let logical_maximum = if globals.logical_minimum >= 0 && globals.logical_maximum < 0 {
                i32::try_from(globals.unsigned_logical_maximum).unwrap_or(i32::MAX)
            } else {
                globals.logical_maximum
            };
            layout.fields.push(Field {
                offset,
                size: globals.report_size,
                count: globals.report_count,
                flags: unsigned(data),
                logical_minimum: globals.logical_minimum,
                logical_maximum,
                usages: locals.field_usages(),
            });
        }
        Ok(())
    }

    /// Returns the usages of the top-level collections, in order, with
    /// [`Usage::UNNAMED`] standing for collections without one and for
    /// fields outside every collection.
    pub fn collections(&self) -> &[Usage] {
        &self.collections
    }

    /// Notes a top-level collection, once for unnamed ones.
    fn add_collection(&mut self, usage: Usage) {
        if usage != Usage::UNNAMED || !self.collections.contains(&usage) {
            self.collections.push(usage);
        }
    }

    /// Returns whether the device's reports start with a report ID.
    pub fn uses_report_ids(&self) -> bool {
        self.uses_report_ids
    }

    /// Returns the length of a report in bytes, after its report ID byte, if
    /// the descriptor declares it.
    pub fn report_length(&self, kind: ReportType, report_id: u8) -> Option<usize> {
        self.reports
            .get(&(kind, report_id))
            .map(ReportLayout::length)
    }

    /// Returns one input report per input report ID with every control at
    /// rest, each starting with its report ID byte, which is 0 when the
    /// descriptor uses no report IDs (Section 8.2.1). Buttons, padding and
    /// relative motion rest at 0, axes that centre at their centre, and hat
    /// switches with a null state at their null value.
    pub fn rest_reports(&self) -> Vec<Vec<u8>> {
        self.reports
            .iter()
            .filter(|((kind, _), _)| *kind == ReportType::Input)
            .map(|((_, report_id), layout)| {
                let mut report = vec![0; 1 + layout.length()];
                report[0] = *report_id;
                for field in &layout.fields {
                    for index in 0..field.count {
                        let offset = field.offset + index * field.size;
                        write_bits(
                            &mut report[1..],
                            offset,
                            field.size,
                            field.rest_value(index),
                        );
                    }
                }
                report
            })
            .collect()
    }
}

/// Writes the low `size` bits of `value` into `report` from bit `offset`,
/// least significant first, as HID packs fields.
fn write_bits(report: &mut [u8], offset: u32, size: u32, value: i64) {
    for bit in 0..size.min(64) {
        if (value >> bit) & 1 == 0 {
            continue;
        }
        let position = (offset + bit) as usize;
        if let Some(byte) = report.get_mut(position / BITS_PER_BYTE as usize) {
            *byte |= 1 << (position % BITS_PER_BYTE as usize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gamepad with 16 buttons, two 8-bit axes and a hat switch with a null
    /// state in report 1, and a vendor-defined output report 2.
    const GAMEPAD: &[u8] = &[
        0x05, 0x01, // Usage Page (Generic Desktop)
        0x09, 0x05, // Usage (Game Pad)
        0xa1, 0x01, // Collection (Application)
        0x85, 0x01, //   Report ID (1)
        0x05, 0x09, //   Usage Page (Button)
        0x19, 0x01, //   Usage Minimum (1)
        0x29, 0x10, //   Usage Maximum (16)
        0x15, 0x00, //   Logical Minimum (0)
        0x25, 0x01, //   Logical Maximum (1)
        0x75, 0x01, //   Report Size (1)
        0x95, 0x10, //   Report Count (16)
        0x81, 0x02, //   Input (Data, Variable, Absolute)
        0x05, 0x01, //   Usage Page (Generic Desktop)
        0x09, 0x30, //   Usage (X)
        0x09, 0x31, //   Usage (Y)
        0x15, 0x00, //   Logical Minimum (0)
        0x26, 0xff, 0x00, // Logical Maximum (255)
        0x75, 0x08, //   Report Size (8)
        0x95, 0x02, //   Report Count (2)
        0x81, 0x02, //   Input (Data, Variable, Absolute)
        0x09, 0x39, //   Usage (Hat Switch)
        0x15, 0x00, //   Logical Minimum (0)
        0x25, 0x07, //   Logical Maximum (7)
        0x75, 0x04, //   Report Size (4)
        0x95, 0x01, //   Report Count (1)
        0x81, 0x42, //   Input (Data, Variable, Absolute, Null State)
        0x81, 0x03, //   Input (Constant): 4 bits of padding
        0x85, 0x02, //   Report ID (2)
        0x06, 0x00, 0xff, // Usage Page (Vendor 0xFF00)
        0x09, 0x01, //   Usage (1)
        0x26, 0xff, 0x00, // Logical Maximum (255)
        0x75, 0x08, //   Report Size (8)
        0x95, 0x04, //   Report Count (4)
        0x91, 0x02, //   Output (Data, Variable, Absolute)
        0xc0, // End Collection
    ];

    /// The boot keyboard of the HID class definition, Appendix B.1.
    const KEYBOARD: &[u8] = &[
        0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0x00, 0x25,
        0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x95, 0x05,
        0x75, 0x01, 0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x91, 0x02, 0x95, 0x01, 0x75, 0x03, 0x91,
        0x01, 0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0x65, 0x05, 0x07, 0x19, 0x00, 0x29, 0x65,
        0x81, 0x00, 0xc0,
    ];

    /// A gamepad's collection, report lengths and rest report are found:
    /// buttons released, axes centred, the hat at its null value.
    #[test]
    fn parses_a_gamepad() {
        let descriptor = ReportDescriptor::parse(GAMEPAD).unwrap();
        assert_eq!(
            descriptor.collections(),
            [Usage {
                page: GENERIC_DESKTOP,
                id: 0x05
            }]
        );
        assert_eq!(
            descriptor.collections()[0].class(),
            CollectionClass::GameController
        );
        assert!(descriptor.uses_report_ids());
        assert_eq!(descriptor.report_length(ReportType::Input, 1), Some(5));
        assert_eq!(descriptor.report_length(ReportType::Output, 2), Some(4));
        assert_eq!(descriptor.report_length(ReportType::Input, 2), None);
        assert_eq!(
            descriptor.rest_reports(),
            [vec![0x01, 0x00, 0x00, 0x80, 0x80, 0x08]]
        );
    }

    /// The boot keyboard is a keyboard, without report IDs, resting at all
    /// zeros.
    #[test]
    fn parses_a_keyboard() {
        let descriptor = ReportDescriptor::parse(KEYBOARD).unwrap();
        assert_eq!(
            descriptor.collections()[0].class(),
            CollectionClass::Keyboard
        );
        assert!(!descriptor.uses_report_ids());
        assert_eq!(descriptor.report_length(ReportType::Input, 0), Some(8));
        assert_eq!(descriptor.report_length(ReportType::Output, 0), Some(1));
        assert_eq!(descriptor.rest_reports(), [vec![0; 9]]);
    }

    /// A top-level collection without a usage, and fields outside every
    /// collection, are seen as unnamed collections, which are of no class
    /// a policy accepts.
    #[test]
    fn sees_unnamed_collections() {
        // A gamepad collection, then an unnamed one with a keyboard field.
        let unnamed = [
            GAMEPAD,
            &[
                0x05, 0x07, 0xa1, 0x01, 0x85, 0x03, 0x75, 0x08, 0x95, 0x01, 0x81, 0x00, 0xc0,
            ],
        ]
        .concat();
        let descriptor = ReportDescriptor::parse(&unnamed).unwrap();
        assert_eq!(descriptor.collections().last(), Some(&Usage::UNNAMED));
        assert_eq!(Usage::UNNAMED.class(), CollectionClass::Other);
        // A keyboard field outside every collection.
        let outside = [
            &[0x85, 0x03, 0x05, 0x07, 0x75, 0x08, 0x95, 0x01, 0x81, 0x00][..],
            GAMEPAD,
        ]
        .concat();
        let descriptor = ReportDescriptor::parse(&outside).unwrap();
        assert_eq!(descriptor.collections()[0], Usage::UNNAMED);
    }

    /// Collections are classed as the input policy needs.
    #[test]
    fn classes_collections() {
        let class = |page, id| Usage { page, id }.class();
        assert_eq!(
            class(GENERIC_DESKTOP, 0x04),
            CollectionClass::GameController
        );
        assert_eq!(
            class(SIMULATION_CONTROLS, 0x01),
            CollectionClass::GameController
        );
        assert_eq!(class(GENERIC_DESKTOP, 0x02), CollectionClass::Mouse);
        assert_eq!(class(GENERIC_DESKTOP, 0x80), CollectionClass::SystemControl);
        assert_eq!(class(DIGITIZERS, 0x02), CollectionClass::Digitizer);
        assert_eq!(class(CONSUMER, 0x01), CollectionClass::ConsumerControl);
        assert_eq!(class(FIDO, 0x01), CollectionClass::Fido);
        assert_eq!(class(0xff42, 0x01), CollectionClass::VendorDefined);
        assert_eq!(class(0x08, 0x01), CollectionClass::Other);
    }

    /// Malformed descriptors are refused.
    #[test]
    fn refuses_malformed_descriptors() {
        let parse = |bytes: &[u8]| ReportDescriptor::parse(bytes).unwrap_err();
        assert_eq!(
            parse(&GAMEPAD[..GAMEPAD.len() - 1]),
            DescriptorError::UnbalancedCollections
        );
        assert_eq!(parse(&[0x05]), DescriptorError::Truncated);
        assert_eq!(parse(&[0xc0]), DescriptorError::UnbalancedCollections);
        assert_eq!(parse(&[0xb4]), DescriptorError::UnbalancedPush);
        assert_eq!(parse(&[0x0c]), DescriptorError::ReservedItem);
        assert_eq!(
            parse(&[0x09, 0x05, 0xa1, 0x01, 0x85, 0x00, 0xc0]),
            DescriptorError::ZeroReportId
        );
        assert_eq!(parse(&[0x09, 0x05]), DescriptorError::NoCollection);
        assert_eq!(
            parse(&vec![0; MAX_REPORT_DESCRIPTOR_LENGTH + 1]),
            DescriptorError::TooLong
        );
        // Report Count (1025), Report Size (8), Input: 1025 bytes.
        assert_eq!(
            parse(&[
                0x09, 0x05, 0xa1, 0x01, 0x96, 0x01, 0x04, 0x75, 0x08, 0x81, 0x02, 0xc0
            ]),
            DescriptorError::ReportTooLong
        );
        // A field before the first Report ID.
        assert_eq!(
            parse(&[
                0x09, 0x05, 0xa1, 0x01, 0x75, 0x08, 0x95, 0x01, 0x81, 0x02, 0x85, 0x01, 0x81, 0x02,
                0xc0
            ]),
            DescriptorError::FieldWithoutReportId
        );
    }

    /// A maximum encoded too short for its unsigned field is read as
    /// unsigned, and a signed axis rests at 0.
    #[test]
    fn reads_logical_ranges() {
        let descriptor = ReportDescriptor::parse(&[
            0x05, 0x01, 0x09, 0x04, 0xa1, 0x01, // Joystick
            0x09, 0x30, 0x15, 0x00, 0x25, 0xff, // X, 0 to 255 encoded as -1
            0x75, 0x08, 0x95, 0x01, 0x81, 0x02, //
            0x09, 0x31, 0x15, 0x81, 0x25, 0x7f, // Y, -127 to 127
            0x81, 0x02, 0xc0,
        ])
        .unwrap();
        assert_eq!(descriptor.rest_reports(), [vec![0x00, 0x80, 0x00]]);
    }

    /// Arbitrary bytes, and corruptions of a real descriptor, are refused or
    /// parsed without panicking, and every rest report has its declared
    /// length.
    #[test]
    fn survives_arbitrary_bytes() {
        let mut state: u64 = 0x5eed;
        let mut next = || {
            // A linear congruential generator: deterministic, so failures
            // reproduce.
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as u8
        };
        for round in 0..5000 {
            let bytes: Vec<u8> = if round % 2 == 0 {
                let length = usize::from(next()) % 96;
                (0..length).map(|_| next()).collect()
            } else {
                let mut corrupted = GAMEPAD.to_vec();
                let position = usize::from(next()) % corrupted.len();
                corrupted[position] = next();
                corrupted
            };
            if let Ok(descriptor) = ReportDescriptor::parse(&bytes) {
                for report in descriptor.rest_reports() {
                    let length = descriptor.report_length(ReportType::Input, report[0]);
                    assert_eq!(Some(report.len() - 1), length);
                }
            }
        }
    }
}
