//! Identity assertions (Section 13.7): a gateway's signed statement that a user
//! it authenticated is connecting to a host, which the client presents to the
//! host as a `TOKEN` and a host trusting the gateway's key accepts in place of
//! credentials of its own.

use std::time::Duration;

use ring::digest::{SHA256, digest};
use ring::signature::{ED25519, Ed25519KeyPair, UnparsedPublicKey};

use crate::discovery::PIN_LENGTH;
use crate::wire::{Reader, VarInt, WireError, Writer};

/// The Magic that begins every assertion, "QDGA" in ASCII.
pub const ASSERTION_MAGIC: u32 = 0x5144_4741;

/// The version of the assertion format this crate implements.
pub const ASSERTION_VERSION: VarInt = VarInt::ZERO;

/// The longest assertion, in bytes: the limit of a token (Section 4.4.3).
pub const MAX_ASSERTION_LENGTH: usize = 4096;

/// The number of bytes of a Key ID.
pub const KEY_ID_LENGTH: usize = 8;

/// The number of bytes of a Nonce.
pub const NONCE_LENGTH: usize = 16;

/// The number of bytes of an Ed25519 public key.
pub const PUBLIC_KEY_LENGTH: usize = 32;

/// The number of bytes of an Ed25519 signature.
const SIGNATURE_LENGTH: usize = 64;

/// The longest User Name, Display Name or Group, in bytes.
pub const MAX_NAME_LENGTH: usize = 255;

/// The most Groups an assertion names.
pub const MAX_GROUPS: usize = 64;

/// The clock skew a host allows when checking an assertion's times.
pub const MAX_CLOCK_SKEW: Duration = Duration::from_secs(60);

/// What the signature covers ahead of the assertion's bytes, so that no other
/// signature by the key can pass for an assertion's.
const SIGNATURE_CONTEXT: &[u8] = b"QDP gateway assertion\0";

/// Identifies a gateway's assertion key: the first 8 bytes of the SHA-256 hash
/// of its public key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyId(pub [u8; KEY_ID_LENGTH]);

impl KeyId {
    /// Returns the ID of `public_key`.
    pub fn of(public_key: &[u8; PUBLIC_KEY_LENGTH]) -> Self {
        let hash = digest(&SHA256, public_key);
        let mut id = [0; KEY_ID_LENGTH];
        id.copy_from_slice(&hash.as_ref()[..KEY_ID_LENGTH]);
        Self(id)
    }
}

/// A gateway's assertion key that a host trusts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrustedKey {
    /// The key's ID.
    id: KeyId,
    /// The Ed25519 public key.
    public_key: [u8; PUBLIC_KEY_LENGTH],
}

impl TrustedKey {
    /// Returns the trusted key `public_key`.
    pub fn new(public_key: [u8; PUBLIC_KEY_LENGTH]) -> Self {
        Self {
            id: KeyId::of(&public_key),
            public_key,
        }
    }

    /// Returns the key's ID.
    pub fn id(&self) -> KeyId {
        self.id
    }
}

/// What a gateway asserts about a user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityAssertion {
    /// The key pin of the host the assertion is for.
    pub host_pin: [u8; PIN_LENGTH],
    /// The key pin of the client certificate the client presented to the
    /// gateway, which it must present to the host too.
    pub client_pin: [u8; PIN_LENGTH],
    /// When the gateway issued it, in seconds since the Unix epoch.
    pub issued_at: u64,
    /// When it expires, in seconds since the Unix epoch.
    pub expires_at: u64,
    /// 16 random bytes, by which a host recognizes an assertion used before.
    pub nonce: [u8; NONCE_LENGTH],
    /// The user's name in the directory, at most 255 bytes.
    pub user_name: String,
    /// The user's full name, at most 255 bytes, or empty.
    pub display_name: String,
    /// The directory groups the gateway reports, at most 64 of at most 255
    /// bytes each.
    pub groups: Vec<String>,
}

/// What a host expects of an assertion presented to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expected {
    /// The host's own key pin.
    pub host_pin: [u8; PIN_LENGTH],
    /// The pin of the certificate the client presented on this connection.
    pub client_pin: [u8; PIN_LENGTH],
    /// The time now, in seconds since the Unix epoch.
    pub now: u64,
}

/// Why a host rejects an assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AssertionError {
    /// The token is not an assertion of a version this crate knows, or is
    /// malformed.
    #[error("the token is not a valid identity assertion")]
    Malformed,
    /// No trusted key has the assertion's Key ID.
    #[error("the assertion is signed with a key this host does not trust")]
    UnknownKey,
    /// The signature does not verify.
    #[error("the assertion's signature does not verify")]
    BadSignature,
    /// The assertion is for another host.
    #[error("the assertion is for another host")]
    WrongHost,
    /// The assertion names another client key.
    #[error("the assertion is for another client key")]
    WrongClient,
    /// The assertion is not valid yet, or not any more.
    #[error("the assertion is outside the times it is valid")]
    Expired,
}

impl From<WireError> for AssertionError {
    fn from(_: WireError) -> Self {
        Self::Malformed
    }
}

impl IdentityAssertion {
    /// Returns the assertion, signed with the gateway's assertion key `key`, as
    /// the token the client presents to the host.
    pub fn sign(&self, key: &Ed25519KeyPair) -> Vec<u8> {
        let mut public_key = [0; PUBLIC_KEY_LENGTH];
        public_key.copy_from_slice(ring::signature::KeyPair::public_key(key).as_ref());
        let mut writer = Writer::new();
        writer.write_bytes(SIGNATURE_CONTEXT);
        writer.write_u32(ASSERTION_MAGIC);
        writer.write_varint(ASSERTION_VERSION);
        writer.write_bytes(&KeyId::of(&public_key).0);
        writer.write_bytes(&self.host_pin);
        writer.write_bytes(&self.client_pin);
        writer.write_varint(VarInt::new(self.issued_at).unwrap_or(VarInt::MAX));
        writer.write_varint(VarInt::new(self.expires_at).unwrap_or(VarInt::MAX));
        writer.write_bytes(&self.nonce);
        writer.write_string_at_most(&self.user_name, MAX_NAME_LENGTH);
        writer.write_string_at_most(&self.display_name, MAX_NAME_LENGTH);
        let groups = &self.groups[..self.groups.len().min(MAX_GROUPS)];
        writer.write_count(groups.len());
        for group in groups {
            writer.write_string_at_most(group, MAX_NAME_LENGTH);
        }
        let mut token = writer.into_bytes();
        let signature = key.sign(&token);
        token.extend_from_slice(signature.as_ref());
        token.drain(..SIGNATURE_CONTEXT.len());
        token
    }

    /// Returns whether `token` begins as an assertion does, so that a host can
    /// tell assertions from its other tokens.
    pub fn is_assertion(token: &[u8]) -> bool {
        token.starts_with(&ASSERTION_MAGIC.to_be_bytes())
    }

    /// Verifies `token` as Section 13.7 requires of a host trusting `keys`:
    /// its signature, that it is for this host and this client's key, and its
    /// times. Returns the assertion; the caller checks its nonce has not been
    /// used, and decides whether to admit the user.
    pub fn verify(
        token: &[u8],
        keys: &[TrustedKey],
        expected: Expected,
    ) -> Result<Self, AssertionError> {
        if token.len() > MAX_ASSERTION_LENGTH || token.len() < SIGNATURE_LENGTH {
            return Err(AssertionError::Malformed);
        }
        let (signed, signature) = token.split_at(token.len() - SIGNATURE_LENGTH);
        let mut reader = Reader::new(signed);
        if reader.read_u32()? != ASSERTION_MAGIC || reader.read_varint()? != ASSERTION_VERSION {
            return Err(AssertionError::Malformed);
        }
        let key_id = KeyId(fixed(&mut reader)?);
        let key = keys
            .iter()
            .find(|key| key.id == key_id)
            .ok_or(AssertionError::UnknownKey)?;
        let mut message = Vec::with_capacity(SIGNATURE_CONTEXT.len() + signed.len());
        message.extend_from_slice(SIGNATURE_CONTEXT);
        message.extend_from_slice(signed);
        UnparsedPublicKey::new(&ED25519, key.public_key)
            .verify(&message, signature)
            .map_err(|_| AssertionError::BadSignature)?;
        let assertion = Self {
            host_pin: fixed(&mut reader)?,
            client_pin: fixed(&mut reader)?,
            issued_at: reader.read_varint()?.into_inner(),
            expires_at: reader.read_varint()?.into_inner(),
            nonce: fixed(&mut reader)?,
            user_name: reader
                .read_string_at_most("User Name", MAX_NAME_LENGTH)?
                .to_owned(),
            display_name: reader
                .read_string_at_most("Display Name", MAX_NAME_LENGTH)?
                .to_owned(),
            groups: {
                let count = reader.read_count("Group Count", MAX_GROUPS as u64)?;
                (0..count)
                    .map(|_| {
                        Ok(reader
                            .read_string_at_most("Group", MAX_NAME_LENGTH)?
                            .to_owned())
                    })
                    .collect::<Result<_, WireError>>()?
            },
        };
        if assertion.host_pin != expected.host_pin {
            return Err(AssertionError::WrongHost);
        }
        if assertion.client_pin != expected.client_pin {
            return Err(AssertionError::WrongClient);
        }
        let skew = MAX_CLOCK_SKEW.as_secs();
        if expected.now.saturating_add(skew) < assertion.issued_at
            || expected.now > assertion.expires_at.saturating_add(skew)
        {
            return Err(AssertionError::Expired);
        }
        Ok(assertion)
    }
}

/// Reads a fixed-length field of `N` bytes.
fn fixed<const N: usize>(reader: &mut Reader<'_>) -> Result<[u8; N], WireError> {
    reader
        .read_bytes(N)?
        .try_into()
        .map_err(|_| WireError::UnexpectedEnd)
}

#[cfg(test)]
mod tests {
    use ring::rand::SystemRandom;
    use ring::signature::KeyPair;

    use super::*;

    /// The time the tests issue assertions at.
    const NOW: u64 = 1_793_000_000;

    /// Returns a new assertion key, and the host's trust of it.
    fn key() -> (Ed25519KeyPair, TrustedKey) {
        let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(document.as_ref()).unwrap();
        let trusted = TrustedKey::new(pair.public_key().as_ref().try_into().unwrap());
        (pair, trusted)
    }

    /// Returns an assertion for host `0xaa…` and client `0xcc…`.
    fn assertion() -> IdentityAssertion {
        IdentityAssertion {
            host_pin: [0xaa; PIN_LENGTH],
            client_pin: [0xcc; PIN_LENGTH],
            issued_at: NOW,
            expires_at: NOW + 120,
            nonce: [7; NONCE_LENGTH],
            user_name: "ian@corp.example.com".to_owned(),
            display_name: "Ian Armour".to_owned(),
            groups: vec!["QDP Users".to_owned(), "Engineering".to_owned()],
        }
    }

    /// What the host of [`assertion`] expects at `now`.
    fn expected(now: u64) -> Expected {
        Expected {
            host_pin: [0xaa; PIN_LENGTH],
            client_pin: [0xcc; PIN_LENGTH],
            now,
        }
    }

    #[test]
    fn verifies_what_it_signs() {
        let (pair, trusted) = key();
        let token = assertion().sign(&pair);
        assert!(IdentityAssertion::is_assertion(&token));
        assert_eq!(&token[..4], b"QDGA");
        assert_eq!(
            IdentityAssertion::verify(&token, &[trusted], expected(NOW + 30)),
            Ok(assertion())
        );
    }

    #[test]
    fn rejects_other_keys_and_changes() {
        let (pair, trusted) = key();
        let (_, stranger) = key();
        let token = assertion().sign(&pair);
        assert_eq!(
            IdentityAssertion::verify(&token, &[stranger], expected(NOW)),
            Err(AssertionError::UnknownKey)
        );
        // A host trusting several keys picks the one the assertion names.
        assert!(IdentityAssertion::verify(&token, &[stranger, trusted], expected(NOW)).is_ok());
        let mut changed = token.clone();
        let user = changed.len() - SIGNATURE_LENGTH - 20;
        changed[user] ^= 1;
        assert_eq!(
            IdentityAssertion::verify(&changed, &[trusted], expected(NOW)),
            Err(AssertionError::BadSignature)
        );
        assert_eq!(
            IdentityAssertion::verify(b"my-secret", &[trusted], expected(NOW)),
            Err(AssertionError::Malformed)
        );
    }

    #[test]
    fn binds_the_host_and_the_client() {
        let (pair, trusted) = key();
        let token = assertion().sign(&pair);
        let mut elsewhere = expected(NOW);
        elsewhere.host_pin = [0xbb; PIN_LENGTH];
        assert_eq!(
            IdentityAssertion::verify(&token, &[trusted], elsewhere),
            Err(AssertionError::WrongHost)
        );
        let mut thief = expected(NOW);
        thief.client_pin = [0xdd; PIN_LENGTH];
        assert_eq!(
            IdentityAssertion::verify(&token, &[trusted], thief),
            Err(AssertionError::WrongClient)
        );
    }

    #[test]
    fn checks_times_with_skew() {
        let (pair, trusted) = key();
        let token = assertion().sign(&pair);
        let skew = MAX_CLOCK_SKEW.as_secs();
        for (now, valid) in [
            (NOW - skew, true),
            (NOW - skew - 1, false),
            (NOW + 120 + skew, true),
            (NOW + 120 + skew + 1, false),
        ] {
            assert_eq!(
                IdentityAssertion::verify(&token, &[trusted], expected(now)).is_ok(),
                valid,
                "{now}"
            );
        }
    }
}
