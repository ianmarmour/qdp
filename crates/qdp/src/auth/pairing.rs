//! Pairing (Section 4.4.9): the host shows a four-digit code, the client's
//! user types it, and the endpoints run SPAKE2 with it, bound to their TLS
//! keys and to the connection, so that each learns that the other is the
//! endpoint the code was shown on or typed into.
//!
//! The client is SPAKE2's party A and the host party B. The host sends its
//! share first, since it knows the code before the client's user does; it
//! sends its confirmation only after checking the client's, so that a client
//! that guesses learns only whether its one guess was right.

use std::fmt;

use p256::Scalar;
use ring::hkdf;
use ring::rand::SecureRandom;
use thiserror::Error;

use crate::auth::spake2::{
    self, CONFIRMATION_LENGTH, Confirmations, Party, SHARE_LENGTH, Spake2, Spake2Error,
};
use crate::wire::{Reader, WireError, Writer};

/// The digits of a pairing code.
pub const CODE_DIGITS: usize = 4;

/// The number of pairing codes: 0000 to 9999.
const CODE_COUNT: u16 = 10_000;

/// The largest multiple of [`CODE_COUNT`] that a random `u16` can reach:
/// values below it map uniformly onto the codes.
const CODE_SAMPLE_LIMIT: u16 = 60_000;

/// The HKDF info from which `w` derives.
const W_INFO: &[u8] = b"QDP pairing code";

/// The bytes of HKDF output reduced to `w`: 64 bits more than the order of
/// P-256 needs, so that the reduction is uniform.
const W_LENGTH: usize = 40;

/// The bytes of the high part of the HKDF output, above 2^256.
const W_HIGH_LENGTH: usize = W_LENGTH - 32;

/// 2^256 modulo the order of P-256, big-endian.
const TWO_TO_256_MOD_ORDER: [u8; 32] = [
    0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x43, 0x19, 0x05, 0x52, 0x58, 0xe8, 0x61, 0x7b, 0x0c, 0x46, 0x35, 0x3d, 0x03, 0x9c, 0xda, 0xaf,
];

/// The most bytes of a Device Name.
pub const MAX_DEVICE_NAME: usize = 64;

/// The bytes of a key pin: a SHA-256 hash.
pub const KEY_PIN_LENGTH: usize = 32;

/// The bytes of the client's message: its share and its confirmation.
pub const CLIENT_MESSAGE_LENGTH: usize = SHARE_LENGTH + CONFIRMATION_LENGTH;

/// Why pairing failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PairingError {
    /// The exchange failed: most often, the code typed was not the one shown.
    #[error(transparent)]
    Exchange(#[from] Spake2Error),
    /// A pairing message was not of its form.
    #[error("the pairing message is malformed")]
    Malformed,
    /// No random numbers could be had.
    #[error("no random numbers are available")]
    Random,
}

/// A pairing code: four decimal digits.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PairingCode([u8; CODE_DIGITS]);

impl PairingCode {
    /// Chooses a code uniformly at random from `random`.
    pub fn generate(random: &dyn SecureRandom) -> Result<Self, PairingError> {
        loop {
            let mut bytes = [0; 2];
            random.fill(&mut bytes).map_err(|_| PairingError::Random)?;
            let value = u16::from_be_bytes(bytes);
            if value < CODE_SAMPLE_LIMIT {
                return Ok(Self::from_number(value % CODE_COUNT));
            }
        }
    }

    /// Returns the code of `number`, below 10,000.
    fn from_number(number: u16) -> Self {
        let text = format!("{number:0width$}", width = CODE_DIGITS);
        Self(
            text.as_bytes()
                .try_into()
                .expect("the code has four digits"),
        )
    }

    /// Reads a code the user typed: four digits, with any spaces between
    /// them.
    pub fn parse(text: &str) -> Option<Self> {
        let digits: Vec<u8> = text
            .bytes()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect();
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        digits.try_into().ok().map(Self)
    }

    /// Returns the code's digits.
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("a code is ASCII digits")
    }

    /// Returns SPAKE2's `w` for the code: HKDF-SHA256 of its digits, with
    /// info "QDP pairing code", 40 bytes long, reduced modulo the order of
    /// P-256.
    fn w(&self) -> Scalar {
        let mut bytes = [0; W_LENGTH];
        hkdf::Salt::new(hkdf::HKDF_SHA256, &[])
            .extract(&self.0)
            .expand(&[W_INFO], Length(W_LENGTH))
            .and_then(|okm| okm.fill(&mut bytes))
            .expect("HKDF-SHA256 expands to 40 bytes");
        let (high, low) = bytes.split_at(W_HIGH_LENGTH);
        let high = u64::from_be_bytes(high.try_into().expect("the high part is 8 bytes"));
        let low = spake2::reduce(&low.try_into().expect("the low part is 32 bytes"));
        Scalar::from(high) * spake2::reduce(&TWO_TO_256_MOD_ORDER) + low
    }
}

/// Shows the code: the host displays it to its user.
impl fmt::Display for PairingCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Leaves the digits out, so that the code reaches no log by accident.
impl fmt::Debug for PairingCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairingCode(****)")
    }
}

/// A length of HKDF output, in bytes.
struct Length(usize);

impl hkdf::KeyType for Length {
    fn len(&self) -> usize {
        self.0
    }
}

/// The key pins of a pairing's endpoints, each the SHA-256 hash of a
/// certificate's SubjectPublicKeyInfo, as this endpoint saw them in the TLS
/// handshake: SPAKE2's identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairingKeys {
    /// The client's key pin: identity A.
    pub client: [u8; KEY_PIN_LENGTH],
    /// The host's key pin: identity B.
    pub host: [u8; KEY_PIN_LENGTH],
}

/// The Auth Data of an `AUTH_START` for the `PAIRING` method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingStart {
    /// The client device's name for users, which the host shows with the
    /// code.
    pub device_name: String,
}

impl PairingStart {
    /// Returns the encoded Auth Data, with the name cut to
    /// [`MAX_DEVICE_NAME`] bytes at a character boundary.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut end = self.device_name.len().min(MAX_DEVICE_NAME);
        while !self.device_name.is_char_boundary(end) {
            end -= 1;
        }
        let mut writer = Writer::new();
        writer.write_string(&self.device_name[..end]);
        writer.into_bytes()
    }

    /// Decodes Auth Data received with the `PAIRING` method.
    pub fn decode(auth_data: &[u8]) -> Result<Self, WireError> {
        let mut reader = Reader::new(auth_data);
        Ok(Self {
            device_name: reader
                .read_string_at_most("Device Name", MAX_DEVICE_NAME)?
                .to_owned(),
        })
    }
}

/// The host's side of a pairing exchange.
#[derive(Debug)]
pub struct HostPairing {
    /// The host's SPAKE2 state.
    spake2: Spake2,
    /// The connection's channel binding, SPAKE2's associated data.
    channel_binding: Vec<u8>,
}

impl HostPairing {
    /// Starts the host's side with the `code` it shows, between the
    /// endpoints of `keys`, on a connection whose channel binding is
    /// `channel_binding`.
    pub fn start(
        code: &PairingCode,
        keys: &PairingKeys,
        channel_binding: &[u8],
        random: &dyn SecureRandom,
    ) -> Result<Self, PairingError> {
        Ok(Self {
            spake2: Spake2::start(Party::B, code.w(), &keys.client, &keys.host, random)?,
            channel_binding: channel_binding.to_vec(),
        })
    }

    /// Returns the host's share, the Auth Data of its `AUTH_CONTINUE`.
    pub fn share(&self) -> &[u8; SHARE_LENGTH] {
        self.spake2.share()
    }

    /// Checks the client's message, its share and confirmation, and returns
    /// the host's confirmation, the Auth Data of a successful `AUTH_RESULT`.
    pub fn finish(&self, client_message: &[u8]) -> Result<[u8; CONFIRMATION_LENGTH], PairingError> {
        if client_message.len() != CLIENT_MESSAGE_LENGTH {
            return Err(PairingError::Malformed);
        }
        let (share, confirmation) = client_message.split_at(SHARE_LENGTH);
        let confirmations = self.spake2.finish(share, &self.channel_binding)?;
        confirmations.verify_peer(confirmation)?;
        Ok(confirmations.own())
    }
}

/// The client's side of a pairing exchange, once it has answered the host.
#[derive(Debug)]
pub struct ClientPairing {
    /// The keys with which the host's confirmation is checked.
    confirmations: Confirmations,
}

impl ClientPairing {
    /// Answers the host's share with the `code` the user typed, between the
    /// endpoints of `keys`, on a connection whose channel binding is
    /// `channel_binding`. Returns the client's side and its message, its
    /// share and confirmation, the Auth Data of its `AUTH_CONTINUE`.
    pub fn respond(
        code: &PairingCode,
        keys: &PairingKeys,
        channel_binding: &[u8],
        host_share: &[u8],
        random: &dyn SecureRandom,
    ) -> Result<(Self, Vec<u8>), PairingError> {
        let spake2 = Spake2::start(Party::A, code.w(), &keys.client, &keys.host, random)?;
        let confirmations = spake2.finish(host_share, channel_binding)?;
        let mut message = Vec::with_capacity(CLIENT_MESSAGE_LENGTH);
        message.extend_from_slice(spake2.share());
        message.extend_from_slice(&confirmations.own());
        Ok((Self { confirmations }, message))
    }

    /// Checks the host's confirmation, from its successful `AUTH_RESULT`:
    /// only a host that saw the code derived it.
    pub fn verify_host(&self, confirmation: &[u8]) -> Result<(), PairingError> {
        Ok(self.confirmations.verify_peer(confirmation)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ring::rand::SystemRandom;

    /// The key pins of a pairing in the tests.
    const KEYS: PairingKeys = PairingKeys {
        client: [1; KEY_PIN_LENGTH],
        host: [2; KEY_PIN_LENGTH],
    };

    /// The channel binding of a connection in the tests.
    const BINDING: [u8; 32] = [3; 32];

    /// Runs an exchange: the host with `shown` and the client with `typed`,
    /// each seeing the keys and binding given. Returns the host's result and,
    /// if it succeeded, the client's check of the host's confirmation.
    fn exchange(
        shown: &str,
        typed: &str,
        client_view: (PairingKeys, &[u8]),
    ) -> (Result<(), PairingError>, Option<Result<(), PairingError>>) {
        let random = SystemRandom::new();
        let host = HostPairing::start(
            &PairingCode::parse(shown).unwrap(),
            &KEYS,
            &BINDING,
            &random,
        )
        .unwrap();
        let (client, message) = ClientPairing::respond(
            &PairingCode::parse(typed).unwrap(),
            &client_view.0,
            client_view.1,
            host.share(),
            &random,
        )
        .unwrap();
        match host.finish(&message) {
            Ok(confirmation) => (Ok(()), Some(client.verify_host(&confirmation))),
            Err(error) => (Err(error), None),
        }
    }

    /// The same code pairs; another code, or a connection whose keys or
    /// binding differ, as through a machine in the middle, does not.
    #[test]
    fn pairs_only_with_the_shown_code() {
        assert_eq!(
            exchange("0427", "04 27", (KEYS, &BINDING)),
            (Ok(()), Some(Ok(())))
        );
        let mismatch = (Err(PairingError::Exchange(Spake2Error::Mismatch)), None);
        assert_eq!(exchange("0427", "0428", (KEYS, &BINDING)), mismatch);
        let impostor = PairingKeys {
            host: [9; KEY_PIN_LENGTH],
            ..KEYS
        };
        assert_eq!(exchange("0427", "0427", (impostor, &BINDING)), mismatch);
        assert_eq!(exchange("0427", "0427", (KEYS, &[4; 32])), mismatch);
    }

    /// A host's confirmation other than its own is refused, and a client
    /// message of the wrong length is malformed.
    #[test]
    fn rejects_forged_messages() {
        let random = SystemRandom::new();
        let code = PairingCode::parse("1234").unwrap();
        let host = HostPairing::start(&code, &KEYS, &BINDING, &random).unwrap();
        let (client, message) =
            ClientPairing::respond(&code, &KEYS, &BINDING, host.share(), &random).unwrap();
        assert!(client.verify_host(&[0; CONFIRMATION_LENGTH]).is_err());
        assert_eq!(
            host.finish(&message[..SHARE_LENGTH]),
            Err(PairingError::Malformed)
        );
    }

    /// Codes are four digits; typed ones may have spaces but nothing else.
    #[test]
    fn reads_codes() {
        let random = SystemRandom::new();
        for _ in 0..100 {
            let code = PairingCode::generate(&random).unwrap();
            assert_eq!(code.as_str().len(), CODE_DIGITS);
            assert_eq!(PairingCode::parse(code.as_str()), Some(code));
        }
        assert_eq!(PairingCode::from_number(7).as_str(), "0007");
        assert_eq!(PairingCode::parse(" 12 34 ").unwrap().as_str(), "1234");
        for typed in ["123", "12345", "12a4", "１２３４", ""] {
            assert_eq!(PairingCode::parse(typed), None, "{typed}");
        }
        assert_eq!(
            format!("{:?}", PairingCode::from_number(1234)),
            "PairingCode(****)"
        );
    }

    /// The reduction of `w` uses 2^256 modulo the group order: one more than
    /// the largest 256-bit number, reduced. Different codes give different
    /// values of `w`.
    #[test]
    fn derives_w() {
        let largest = spake2::reduce(&[0xff; 32]);
        assert_eq!(largest + Scalar::ONE, spake2::reduce(&TWO_TO_256_MOD_ORDER));
        let w = |code: u16| PairingCode::from_number(code).w();
        assert_eq!(w(1234), w(1234));
        assert_ne!(w(1234), w(1235));
    }

    /// The Device Name survives encoding, and is cut at a character
    /// boundary to its limit.
    #[test]
    fn encodes_device_names() {
        let start = PairingStart {
            device_name: "AYN Thor".to_owned(),
        };
        assert_eq!(PairingStart::decode(&start.to_bytes()).unwrap(), start);
        let long = PairingStart {
            device_name: "é".repeat(40),
        };
        let decoded = PairingStart::decode(&long.to_bytes()).unwrap();
        assert_eq!(decoded.device_name, "é".repeat(32));
    }
}
