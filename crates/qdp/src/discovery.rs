//! Local network discovery (Section 3.13): the DNS-SD service a host
//! advertises over Multicast DNS, and the TXT record that describes it.
//!
//! Nothing here is authenticated: a client uses a discovered pin only to
//! recognize a host whose key it already pinned, which the TLS handshake then
//! verifies.

/// The DNS-SD service type of QDP hosts, in the domain `local.`.
pub const SERVICE_TYPE: &str = "_qdp._udp";

/// The longest instance name, in bytes: a DNS label's.
pub const MAX_INSTANCE_NAME_LENGTH: usize = 63;

/// The instance name of a host whose name for users is empty.
pub const DEFAULT_INSTANCE_NAME: &str = "QDP host";

/// The TXT key of the ALPN tokens the host accepts.
const ALPN_KEY: &str = "alpn";

/// The TXT key of the host's key pin.
const PIN_KEY: &str = "pin";

/// The TXT key present while the host offers pairing.
const PAIR_KEY: &str = "pair";

/// The value of [`PAIR_KEY`] while the host offers pairing.
const PAIR_OFFERED: &str = "1";

/// The separator of the ALPN tokens in the `alpn` value.
const ALPN_SEPARATOR: char = ',';

/// The length of a key pin, in bytes.
const PIN_LENGTH: usize = 32;

/// Whether a host offers the `PAIRING` method (Section 4.4.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PairingOffer {
    /// New clients may pair.
    Offered,
    /// The host does not offer pairing.
    NotOffered,
}

/// What a host's TXT record says about it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceRecord {
    /// The ALPN tokens the host accepts.
    pub alpn: Vec<String>,
    /// The host's key pin: the SHA-256 hash of its certificate's
    /// SubjectPublicKeyInfo.
    pub pin: [u8; PIN_LENGTH],
    /// Whether the host offers pairing.
    pub pairing: PairingOffer,
}

impl ServiceRecord {
    /// Returns the record of a host with key pin `pin` that accepts this
    /// crate's draft.
    pub fn new(pin: [u8; PIN_LENGTH], pairing: PairingOffer) -> Self {
        Self {
            alpn: vec![String::from_utf8_lossy(crate::ALPN).into_owned()],
            pin,
            pairing,
        }
    }

    /// Returns the record's entries as keys and values.
    pub fn entries(&self) -> Vec<(&'static str, String)> {
        let mut entries = vec![
            (ALPN_KEY, self.alpn.join(&ALPN_SEPARATOR.to_string())),
            (PIN_KEY, hex(&self.pin)),
        ];
        if self.pairing == PairingOffer::Offered {
            entries.push((PAIR_KEY, PAIR_OFFERED.to_owned()));
        }
        entries
    }

    /// Returns the record's entries as the `key=value` strings a TXT record
    /// holds (RFC 6763, Section 6.3).
    pub fn strings(&self) -> Vec<String> {
        self.entries()
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect()
    }

    /// Reads a record from a TXT record's entries, keys and values, the
    /// value `None` for a key without `=`. Keys are matched without regard
    /// to case and the first of each counts (RFC 6763, Section 6.4); keys
    /// this crate does not know are ignored. Returns `None` without a valid
    /// pin.
    pub fn parse<'a>(
        entries: impl IntoIterator<Item = (&'a str, Option<&'a [u8]>)>,
    ) -> Option<Self> {
        let mut alpn = None;
        let mut pin = None;
        let mut pairing = None;
        for (key, value) in entries {
            let text = value.map(String::from_utf8_lossy);
            if key.eq_ignore_ascii_case(ALPN_KEY) && alpn.is_none() {
                alpn = Some(
                    text.as_deref()
                        .unwrap_or_default()
                        .split(ALPN_SEPARATOR)
                        .filter(|token| !token.is_empty())
                        .map(str::to_owned)
                        .collect(),
                );
            } else if key.eq_ignore_ascii_case(PIN_KEY) && pin.is_none() {
                pin = Some(text.as_deref().and_then(parse_hex));
            } else if key.eq_ignore_ascii_case(PAIR_KEY) && pairing.is_none() {
                pairing = Some(if text.as_deref() == Some(PAIR_OFFERED) {
                    PairingOffer::Offered
                } else {
                    PairingOffer::NotOffered
                });
            }
        }
        Some(Self {
            alpn: alpn.unwrap_or_default(),
            pin: pin.flatten()?,
            pairing: pairing.unwrap_or(PairingOffer::NotOffered),
        })
    }

    /// Reads a record from `key=value` strings, as [`ServiceRecord::strings`]
    /// writes them.
    pub fn parse_strings<'a>(strings: impl IntoIterator<Item = &'a [u8]>) -> Option<Self> {
        let entries: Vec<(String, Option<&[u8]>)> = strings
            .into_iter()
            .map(
                |string| match string.iter().position(|&byte| byte == b'=') {
                    Some(equals) => (
                        String::from_utf8_lossy(&string[..equals]).into_owned(),
                        Some(&string[equals + 1..]),
                    ),
                    None => (String::from_utf8_lossy(string).into_owned(), None),
                },
            )
            .collect();
        Self::parse(entries.iter().map(|(key, value)| (key.as_str(), *value)))
    }

    /// Returns whether the host accepts this crate's draft.
    pub fn accepts_this_draft(&self) -> bool {
        self.alpn
            .iter()
            .any(|token| token.as_bytes() == crate::ALPN)
    }
}

/// Returns the instance name for a host named `name` for users: the name, cut
/// to [`MAX_INSTANCE_NAME_LENGTH`] bytes at a character's end, or
/// [`DEFAULT_INSTANCE_NAME`] if it is empty.
pub fn instance_name(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return DEFAULT_INSTANCE_NAME.to_owned();
    }
    let mut end = name.len().min(MAX_INSTANCE_NAME_LENGTH);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_owned()
}

/// Returns `bytes` as lower-case hexadecimal digits.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Reads a key pin from hexadecimal digits of either case.
fn parse_hex(text: &str) -> Option<[u8; PIN_LENGTH]> {
    if text.len() != PIN_LENGTH * 2 || !text.is_ascii() {
        return None;
    }
    let mut pin = [0; PIN_LENGTH];
    for (byte, pair) in pin.iter_mut().zip(text.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(pin)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record is written as the specification's keys and read back from
    /// them.
    #[test]
    fn writes_and_reads_records() {
        let record = ServiceRecord::new([0xab; PIN_LENGTH], PairingOffer::Offered);
        let strings = record.strings();
        assert_eq!(
            strings,
            [
                "alpn=qdp-draft-00".to_owned(),
                format!("pin={}", "ab".repeat(PIN_LENGTH)),
                "pair=1".to_owned(),
            ]
        );
        let parsed = ServiceRecord::parse_strings(strings.iter().map(|string| string.as_bytes()));
        assert_eq!(parsed.as_ref(), Some(&record));
        assert!(record.accepts_this_draft());

        let closed = ServiceRecord::new([0x01; PIN_LENGTH], PairingOffer::NotOffered);
        assert!(
            !closed
                .strings()
                .iter()
                .any(|string| string.starts_with("pair"))
        );
    }

    /// Reading ignores unknown keys and case, keeps the first of each key,
    /// and needs a valid pin.
    #[test]
    fn reads_records_leniently() {
        let pin = "AB".repeat(PIN_LENGTH);
        let other = "cd".repeat(PIN_LENGTH);
        let entries = [
            ("Pin", Some(pin.as_bytes())),
            ("pin", Some(other.as_bytes())),
            ("color", Some(b"blue".as_slice())),
            ("pair", None),
        ];
        let record = ServiceRecord::parse(entries).unwrap();
        assert_eq!(record.pin, [0xab; PIN_LENGTH]);
        assert_eq!(record.pairing, PairingOffer::NotOffered);
        assert_eq!(record.alpn, Vec::<String>::new());
        assert!(!record.accepts_this_draft());

        assert_eq!(
            ServiceRecord::parse([("pin", Some(b"abcd".as_slice()))]),
            None
        );
        assert_eq!(
            ServiceRecord::parse([("alpn", Some(b"qdp-draft-00".as_slice()))]),
            None
        );
    }

    /// Instance names are cut to a DNS label at a character's end, and an
    /// empty name gets the default.
    #[test]
    fn names_instances() {
        assert_eq!(instance_name("  studio  "), "studio");
        assert_eq!(instance_name(""), DEFAULT_INSTANCE_NAME);
        let long = "é".repeat(40);
        let name = instance_name(&long);
        assert_eq!(name.len(), 62);
        assert!(long.starts_with(&name));
    }
}
