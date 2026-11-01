//! SPAKE2 [RFC 9382] with the `SPAKE2-P256-SHA256-HKDF-HMAC` ciphersuite, the
//! password-authenticated key exchange that pairing runs (Section 4.4.9).
//!
//! Each party chooses a random scalar and sends its share: that scalar times
//! the generator, plus the shared secret `w` times the party's constant, M for
//! party A and N for party B. From the peer's share each derives the same
//! point K only if both used the same `w`. Both then hash the transcript of
//! the identities, shares, K and `w` into confirmation keys, and prove they
//! derived the same keys by exchanging MACs of the transcript. An attacker who
//! does not know `w` learns, from an exchange it takes part in, only whether
//! one guess at it was right.
//!
//! [RFC 9382]: https://www.rfc-editor.org/rfc/rfc9382

use p256::elliptic_curve::group::Group;
use p256::elliptic_curve::ops::Reduce;
use p256::elliptic_curve::sec1::{FromEncodedPoint, ToEncodedPoint};
use p256::elliptic_curve::{Field, PrimeField};
use p256::{AffinePoint, EncodedPoint, FieldBytes, ProjectivePoint, Scalar, U256};
use ring::rand::SecureRandom;
use ring::{digest, hkdf, hmac};
use thiserror::Error;

/// P-256's M, compressed (RFC 9382, Section 6).
const M: [u8; 33] = [
    0x02, 0x88, 0x6e, 0x2f, 0x97, 0xac, 0xe4, 0x6e, 0x55, 0xba, 0x9d, 0xd7, 0x24, 0x25, 0x79, 0xf2,
    0x99, 0x3b, 0x64, 0xe1, 0x6e, 0xf3, 0xdc, 0xab, 0x95, 0xaf, 0xd4, 0x97, 0x33, 0x3d, 0x8f, 0xa1,
    0x2f,
];

/// P-256's N, compressed (RFC 9382, Section 6).
const N: [u8; 33] = [
    0x03, 0xd8, 0xbb, 0xd6, 0xc6, 0x39, 0xc6, 0x29, 0x37, 0xb0, 0x4d, 0x99, 0x7f, 0x38, 0xc3, 0x77,
    0x07, 0x19, 0xc6, 0x29, 0xd7, 0x01, 0x4d, 0x49, 0xa2, 0x4b, 0x4f, 0x98, 0xba, 0xa1, 0x29, 0x2b,
    0x49,
];

/// The bytes of a share: an uncompressed SEC1 point.
pub const SHARE_LENGTH: usize = 65;

/// The bytes of a key confirmation: an HMAC-SHA256 output.
pub const CONFIRMATION_LENGTH: usize = 32;

/// The SEC1 tag of an uncompressed point.
const UNCOMPRESSED_TAG: u8 = 0x04;

/// The bytes of each of Ke and Ka, and of each confirmation key: half of
/// SHA-256's output.
const KEY_LENGTH: usize = 16;

/// The start of the info string from which the confirmation keys derive.
const CONFIRMATION_KEYS_INFO: &[u8] = b"ConfirmationKeys";

/// Why an exchange failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum Spake2Error {
    /// The peer's share is not an uncompressed point of P-256.
    #[error("the peer's share is not a point of P-256")]
    InvalidShare,
    /// The shared point is the point at infinity.
    #[error("the exchange produced the point at infinity")]
    Infinity,
    /// The peer's confirmation does not match: it used another secret, or
    /// another transcript.
    #[error("the peer's confirmation does not match")]
    Mismatch,
    /// No random numbers could be had.
    #[error("no random numbers are available")]
    Random,
}

/// The two parties of an exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Party {
    /// Party A, whose share uses M.
    A,
    /// Party B, whose share uses N.
    B,
}

impl Party {
    /// Returns the constant this party's share uses.
    fn constant(self) -> ProjectivePoint {
        let encoded = match self {
            Self::A => &M,
            Self::B => &N,
        };
        decode_point(encoded).expect("M and N are points of P-256")
    }

    /// Returns the other party.
    fn peer(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

/// One party's side of an exchange, once it has chosen its scalar.
pub struct Spake2 {
    /// Which party this is.
    party: Party,
    /// Its random scalar: x for A, y for B.
    scalar: Scalar,
    /// The shared secret.
    w: Scalar,
    /// Its share, encoded.
    share: [u8; SHARE_LENGTH],
    /// Party A's identity.
    id_a: Vec<u8>,
    /// Party B's identity.
    id_b: Vec<u8>,
}

/// Leaves the secrets out.
impl std::fmt::Debug for Spake2 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Spake2")
            .field("party", &self.party)
            .finish_non_exhaustive()
    }
}

impl Spake2 {
    /// Starts `party`'s side of an exchange with secret `w`, between parties
    /// identified as `id_a` and `id_b`, choosing its scalar from `random`.
    pub fn start(
        party: Party,
        w: Scalar,
        id_a: &[u8],
        id_b: &[u8],
        random: &dyn SecureRandom,
    ) -> Result<Self, Spake2Error> {
        Ok(Self::with_scalar(
            party,
            w,
            random_scalar(random)?,
            id_a,
            id_b,
        ))
    }

    /// Starts `party`'s side with the given scalar.
    fn with_scalar(party: Party, w: Scalar, scalar: Scalar, id_a: &[u8], id_b: &[u8]) -> Self {
        let share = ProjectivePoint::GENERATOR * scalar + party.constant() * w;
        Self {
            party,
            scalar,
            w,
            share: encode_point(&share),
            id_a: id_a.to_vec(),
            id_b: id_b.to_vec(),
        }
    }

    /// Returns this party's share, to send to the peer.
    pub fn share(&self) -> &[u8; SHARE_LENGTH] {
        &self.share
    }

    /// Finishes the exchange with the peer's share, deriving the
    /// confirmation keys with associated data `aad`.
    pub fn finish(&self, peer_share: &[u8], aad: &[u8]) -> Result<Confirmations, Spake2Error> {
        let peer = decode_share(peer_share)?;
        let k = (peer - self.party.peer().constant() * self.w) * self.scalar;
        if bool::from(k.is_identity()) {
            return Err(Spake2Error::Infinity);
        }
        let (share_a, share_b) = match self.party {
            Party::A => (&self.share[..], peer_share),
            Party::B => (peer_share, &self.share[..]),
        };
        let transcript = transcript(&[
            &self.id_a,
            &self.id_b,
            share_a,
            share_b,
            &encode_point(&k),
            &self.w.to_repr(),
        ]);
        let hash = digest::digest(&digest::SHA256, &transcript);
        let (shared_secret, ka) = hash.as_ref().split_at(KEY_LENGTH);
        let info = [CONFIRMATION_KEYS_INFO, aad];
        let mut keys = [0; 2 * KEY_LENGTH];
        hkdf::Salt::new(hkdf::HKDF_SHA256, &[])
            .extract(ka)
            .expand(&info, Length(keys.len()))
            .and_then(|okm| okm.fill(&mut keys))
            .expect("HKDF-SHA256 expands to 32 bytes");
        let (key_a, key_b) = keys.split_at(KEY_LENGTH);
        let (own, peer) = match self.party {
            Party::A => (key_a, key_b),
            Party::B => (key_b, key_a),
        };
        Ok(Confirmations {
            transcript,
            own_key: own.try_into().expect("a confirmation key is 16 bytes"),
            peer_key: peer.try_into().expect("a confirmation key is 16 bytes"),
            shared_secret: shared_secret
                .try_into()
                .expect("the shared secret is 16 bytes"),
        })
    }
}

/// The keys of a finished exchange, with which each party proves to the other
/// that it derived them.
pub struct Confirmations {
    /// The transcript, which the confirmations authenticate.
    transcript: Vec<u8>,
    /// This party's confirmation key: KcA for A, KcB for B.
    own_key: [u8; KEY_LENGTH],
    /// The peer's confirmation key.
    peer_key: [u8; KEY_LENGTH],
    /// The shared secret Ke, which pairing does not use: TLS protects the
    /// connection.
    #[cfg_attr(not(test), allow(dead_code))]
    shared_secret: [u8; KEY_LENGTH],
}

/// Leaves the keys out.
impl std::fmt::Debug for Confirmations {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Confirmations")
            .finish_non_exhaustive()
    }
}

impl Confirmations {
    /// Returns this party's confirmation, to send to the peer.
    pub fn own(&self) -> [u8; CONFIRMATION_LENGTH] {
        let tag = hmac::sign(
            &hmac::Key::new(hmac::HMAC_SHA256, &self.own_key),
            &self.transcript,
        );
        tag.as_ref()
            .try_into()
            .expect("an HMAC-SHA256 tag is 32 bytes")
    }

    /// Checks the peer's confirmation, in constant time.
    pub fn verify_peer(&self, confirmation: &[u8]) -> Result<(), Spake2Error> {
        hmac::verify(
            &hmac::Key::new(hmac::HMAC_SHA256, &self.peer_key),
            &self.transcript,
            confirmation,
        )
        .map_err(|_| Spake2Error::Mismatch)
    }
}

/// A length of HKDF output, in bytes.
struct Length(usize);

impl hkdf::KeyType for Length {
    fn len(&self) -> usize {
        self.0
    }
}

/// Returns the transcript of `fields`, each preceded by its length as an
/// eight-byte little-endian number (RFC 9382, Section 3.3).
fn transcript(fields: &[&[u8]]) -> Vec<u8> {
    let mut transcript = Vec::new();
    for field in fields {
        transcript.extend_from_slice(&(field.len() as u64).to_le_bytes());
        transcript.extend_from_slice(field);
    }
    transcript
}

/// Returns a scalar chosen uniformly from 1 to p - 1, by rejection sampling.
fn random_scalar(random: &dyn SecureRandom) -> Result<Scalar, Spake2Error> {
    loop {
        let mut bytes = FieldBytes::default();
        random.fill(&mut bytes).map_err(|_| Spake2Error::Random)?;
        let scalar: Option<Scalar> = Scalar::from_repr(bytes).into();
        if let Some(scalar) = scalar.filter(|scalar| !bool::from(scalar.is_zero())) {
            return Ok(scalar);
        }
    }
}

/// Returns a scalar from a big-endian number of up to 32 bytes, reduced
/// modulo the order of P-256.
pub(crate) fn reduce(bytes: &[u8; 32]) -> Scalar {
    <Scalar as Reduce<U256>>::reduce_bytes(&FieldBytes::from(*bytes))
}

/// Decodes a share: an uncompressed point of P-256.
fn decode_share(bytes: &[u8]) -> Result<ProjectivePoint, Spake2Error> {
    if bytes.len() != SHARE_LENGTH || bytes[0] != UNCOMPRESSED_TAG {
        return Err(Spake2Error::InvalidShare);
    }
    decode_point(bytes).ok_or(Spake2Error::InvalidShare)
}

/// Decodes a SEC1 point of P-256, rejecting the point at infinity.
fn decode_point(bytes: &[u8]) -> Option<ProjectivePoint> {
    let encoded = EncodedPoint::from_bytes(bytes).ok()?;
    let point: Option<AffinePoint> = AffinePoint::from_encoded_point(&encoded).into();
    point
        .map(ProjectivePoint::from)
        .filter(|point| !bool::from(point.is_identity()))
}

/// Encodes a point as an uncompressed SEC1 point.
fn encode_point(point: &ProjectivePoint) -> [u8; SHARE_LENGTH] {
    point
        .to_affine()
        .to_encoded_point(false)
        .as_bytes()
        .try_into()
        .expect("an uncompressed P-256 point is 65 bytes")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes hexadecimal digits.
    fn hex(text: &str) -> Vec<u8> {
        let digits: Vec<u8> = text
            .bytes()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect();
        digits
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    /// Returns the scalar of 32 hexadecimal bytes.
    fn scalar(text: &str) -> Scalar {
        reduce(&hex(text).try_into().unwrap())
    }

    /// One of RFC 9382's test vectors (Appendix B).
    struct Vector {
        id_a: &'static str,
        id_b: &'static str,
        w: &'static str,
        x: &'static str,
        y: &'static str,
        pa: &'static str,
        pb: &'static str,
        ke: &'static str,
        conf_a: &'static str,
        conf_b: &'static str,
    }

    /// RFC 9382's first two test vectors: with both identities, and with
    /// A's empty.
    const VECTORS: [Vector; 2] = [
        Vector {
            id_a: "server",
            id_b: "client",
            w: "2ee57912099d31560b3a44b1184b9b4866e904c49d12ac5042c97dca461b1a5f",
            x: "43dd0fd7215bdcb482879fca3220c6a968e66d70b1356cac18bb26c84a78d729",
            y: "dcb60106f276b02606d8ef0a328c02e4b629f84f89786af5befb0bc75b6e66be",
            pa: "04a56fa807caaa53a4d28dbb9853b9815c61a411118a6fe516a8798434751470
                 f9010153ac33d0d5f2047ffdb1a3e42c9b4e6be662766e1eeb4116988ede5f912c",
            pb: "0406557e482bd03097ad0cbaa5df82115460d951e3451962f1eaf4367a420676
                 d09857ccbc522686c83d1852abfa8ed6e4a1155cf8f1543ceca528afb591a1e0b7",
            ke: "0e0672dc86f8e45565d338b0540abe69",
            conf_a: "58ad4aa88e0b60d5061eb6b5dd93e80d9c4f00d127c65b3b35b1b5281fee38f0",
            conf_b: "d3e2e547f1ae04f2dbdbf0fc4b79f8ecff2dff314b5d32fe9fcef2fb26dc459b",
        },
        Vector {
            id_a: "",
            id_b: "client",
            w: "0548d8729f730589e579b0475a582c1608138ddf7054b73b5381c7e883e2efae",
            x: "403abbe3b1b4b9ba17e3032849759d723939a27a27b9d921c500edde18ed654b",
            y: "903023b6598908936ea7c929bd761af6039577a9c3f9581064187c3049d87065",
            pa: "04a897b769e681c62ac1c2357319a3d363f610839c4477720d24cbe32f5fd8
                 5f44fb92ba966578c1b712be6962498834078262caa5b441ecfa9d4a9485720e918a",
            pb: "04e0f816fd1c35e22065d5556215c097e799390d16661c386e0ecc84593974
                 a61b881a8c82327687d0501862970c64565560cb5671f696048050ca66ca5f8cc7fc",
            ke: "642f05c473c2cd79909f9a841e2f30a7",
            conf_a: "47d29e6666af1b7dd450d571233085d7a9866e4d49d2645e2df975489521232b",
            conf_b: "3313c5cefc361d27fb16847a91c2a73b766ffa90a4839122a9b70a2f6bd1d6df",
        },
    ];

    /// Both parties reproduce RFC 9382's shares, shared secret and
    /// confirmations, and accept each other's.
    #[test]
    fn matches_the_rfc_test_vectors() {
        for vector in &VECTORS {
            let w = scalar(vector.w);
            let (id_a, id_b) = (vector.id_a.as_bytes(), vector.id_b.as_bytes());
            let a = Spake2::with_scalar(Party::A, w, scalar(vector.x), id_a, id_b);
            let b = Spake2::with_scalar(Party::B, w, scalar(vector.y), id_a, id_b);
            assert_eq!(a.share().to_vec(), hex(vector.pa), "{}", vector.id_a);
            assert_eq!(b.share().to_vec(), hex(vector.pb), "{}", vector.id_a);
            let at_a = a.finish(b.share(), &[]).unwrap();
            let at_b = b.finish(a.share(), &[]).unwrap();
            assert_eq!(at_a.shared_secret.to_vec(), hex(vector.ke));
            assert_eq!(at_b.shared_secret.to_vec(), hex(vector.ke));
            assert_eq!(at_a.own().to_vec(), hex(vector.conf_a));
            assert_eq!(at_b.own().to_vec(), hex(vector.conf_b));
            assert_eq!(at_a.verify_peer(&at_b.own()), Ok(()));
            assert_eq!(at_b.verify_peer(&at_a.own()), Ok(()));
        }
    }

    /// Parties with different secrets, or different associated data, or
    /// different identities, reject each other's confirmations.
    #[test]
    fn rejects_mismatches() {
        let random = ring::rand::SystemRandom::new();
        let w = scalar(VECTORS[0].w);
        let start = |party, w, id_b: &[u8]| Spake2::start(party, w, b"a", id_b, &random).unwrap();
        let a = start(Party::A, w, b"b");
        for (b, aad) in [
            (start(Party::B, scalar(VECTORS[1].w), b"b"), &b""[..]),
            (start(Party::B, w, b"b"), &b"other connection"[..]),
            (start(Party::B, w, b"impostor"), &b""[..]),
        ] {
            let at_a = a.finish(b.share(), b"").unwrap();
            let at_b = b.finish(a.share(), aad).unwrap();
            assert_eq!(at_a.verify_peer(&at_b.own()), Err(Spake2Error::Mismatch));
            assert_eq!(at_b.verify_peer(&at_a.own()), Err(Spake2Error::Mismatch));
        }
        // The same secret, data and identities agree.
        let b = start(Party::B, w, b"b");
        let at_a = a.finish(b.share(), b"").unwrap();
        assert_eq!(
            b.finish(a.share(), b"").unwrap().verify_peer(&at_a.own()),
            Ok(())
        );
    }

    /// Shares that are not uncompressed points of P-256 are refused.
    #[test]
    fn rejects_invalid_shares() {
        let random = ring::rand::SystemRandom::new();
        let a = Spake2::start(Party::A, scalar(VECTORS[0].w), b"", b"", &random).unwrap();
        let mut off_curve = hex(VECTORS[0].pb);
        off_curve[64] ^= 1;
        let compressed = &M[..];
        for share in [&off_curve[..], compressed, &[0x04; 65][..], &[][..]] {
            assert_eq!(a.finish(share, b"").unwrap_err(), Spake2Error::InvalidShare);
        }
    }
}
