//! Time-based one-time passwords for the `TOTP` method (Section 4.4.10): the
//! six-digit codes of RFC 6238 that authenticator apps such as Google
//! Authenticator, Microsoft Authenticator and Authy show, the secrets they are
//! made from, and the `otpauth://` URI that enrolls a secret in an app.

use std::fmt;

use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};

/// The number of bytes of a secret this module creates.
pub const SECRET_LENGTH: usize = 20;

/// The fewest bytes a secret may have (Section 4.4.10).
const MIN_SECRET_LENGTH: usize = 16;

/// The number of digits of a code.
pub const DIGITS: usize = 6;

/// The length of a step, in seconds.
pub const STEP_SECONDS: u64 = 30;

/// The steps either side of the current one whose codes are accepted, for
/// clock skew.
const SKEW_STEPS: u64 = 1;

/// 10 to the power of [`DIGITS`].
const CODE_MODULUS: u32 = 1_000_000;

/// The alphabet of base32 (RFC 4648, Section 6).
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// The bits a base32 character holds.
const BASE32_BITS: u32 = 5;

/// A shared TOTP secret.
#[derive(Clone, PartialEq, Eq)]
pub struct TotpSecret(Vec<u8>);

impl fmt::Debug for TotpSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The secret makes the user's codes, so it stays out of logs.
        formatter.write_str("TotpSecret(..)")
    }
}

/// Why a secret could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// The text is not base32.
    #[error("the secret is not base32")]
    NotBase32,
    /// The secret has fewer than 16 bytes.
    #[error("the secret is too short")]
    TooShort,
    /// No random bytes could be had.
    #[error("no random bytes are available")]
    NoRandomness,
}

impl TotpSecret {
    /// Returns a new random secret of [`SECRET_LENGTH`] bytes.
    pub fn generate() -> Result<Self, SecretError> {
        let mut bytes = vec![0; SECRET_LENGTH];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| SecretError::NoRandomness)?;
        Ok(Self(bytes))
    }

    /// Reads a secret in base32, as apps and `otpauth://` URIs write it: in
    /// either case, with or without padding, spaces and hyphens.
    pub fn from_base32(text: &str) -> Result<Self, SecretError> {
        let bytes = decode_base32(text).ok_or(SecretError::NotBase32)?;
        if bytes.len() < MIN_SECRET_LENGTH {
            return Err(SecretError::TooShort);
        }
        Ok(Self(bytes))
    }

    /// Returns the secret in base32, without padding, as apps take it.
    pub fn to_base32(&self) -> String {
        encode_base32(&self.0)
    }

    /// Returns the code of `step`: the HOTP value of RFC 4226 with the step as
    /// its counter.
    pub fn code(&self, step: u64) -> u32 {
        let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, &self.0);
        let tag = hmac::sign(&key, &step.to_be_bytes());
        let tag = tag.as_ref();
        // Dynamic truncation (RFC 4226, Section 5.3).
        let offset = usize::from(tag[tag.len() - 1] & 0x0f);
        let value = u32::from_be_bytes([
            tag[offset],
            tag[offset + 1],
            tag[offset + 2],
            tag[offset + 3],
        ]) & 0x7fff_ffff;
        value % CODE_MODULUS
    }

    /// Checks `code` at `now`, in seconds since the Unix epoch, against the
    /// codes of the current step and the steps either side, refusing any step
    /// at or before `last_step`, the last step accepted for the user. Returns
    /// the step accepted, which the caller keeps as the user's last step.
    pub fn verify(&self, code: &str, now: u64, last_step: Option<u64>) -> Option<u64> {
        let code = code.trim();
        if code.len() != DIGITS || !code.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let current = now / STEP_SECONDS;
        let mut accepted = None;
        // Every candidate is compared, so that the time taken does not tell
        // which matched.
        for step in current.saturating_sub(SKEW_STEPS)..=current.saturating_add(SKEW_STEPS) {
            let expected = format!("{:0width$}", self.code(step), width = DIGITS);
            let fresh = last_step.is_none_or(|last| step > last);
            if constant_time_equal(expected.as_bytes(), code.as_bytes()) && fresh {
                accepted.get_or_insert(step);
            }
        }
        accepted
    }

    /// Returns the `otpauth://` URI that enrolls the secret in an
    /// authenticator app, which shows `issuer` and `account` beside its codes.
    pub fn enrollment_uri(&self, issuer: &str, account: &str) -> String {
        format!(
            "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECONDS}",
            percent_encode(issuer),
            percent_encode(account),
            self.to_base32(),
            percent_encode(issuer),
        )
    }
}

/// Compares `a` and `b` in time that depends only on their lengths.
fn constant_time_equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .fold(0, |difference, (x, y)| difference | (x ^ y))
            == 0
}

/// Encodes `bytes` in base32, without padding.
fn encode_base32(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= BASE32_BITS {
            bits -= BASE32_BITS;
            text.push(char::from(
                BASE32_ALPHABET[((buffer >> bits) & 0x1f) as usize],
            ));
        }
    }
    if bits > 0 {
        text.push(char::from(
            BASE32_ALPHABET[((buffer << (BASE32_BITS - bits)) & 0x1f) as usize],
        ));
    }
    text
}

/// Decodes base32 in either case, ignoring padding, spaces and hyphens, or
/// returns `None` if `text` holds other characters.
fn decode_base32(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(text.len() * 5 / 8);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for character in text.bytes() {
        if matches!(character, b'=' | b' ' | b'-') {
            continue;
        }
        let value = BASE32_ALPHABET
            .iter()
            .position(|&letter| letter == character.to_ascii_uppercase())?;
        buffer = (buffer << BASE32_BITS) | u32::try_from(value).ok()?;
        bits += BASE32_BITS;
        if bits >= 8 {
            bits -= 8;
            bytes.push(u8::try_from((buffer >> bits) & 0xff).ok()?);
        }
    }
    Some(bytes)
}

/// Percent-encodes `text` for a URI, leaving only unreserved characters.
fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The secret of the test vectors of RFC 4226 and RFC 6238 (SHA-1).
    fn rfc_secret() -> TotpSecret {
        TotpSecret(b"12345678901234567890".to_vec())
    }

    #[test]
    fn makes_the_hotp_values_of_rfc_4226() {
        let expected = [
            755_224, 287_082, 359_152, 969_429, 338_314, 254_676, 287_922, 162_583, 399_871,
            520_489,
        ];
        for (counter, code) in expected.into_iter().enumerate() {
            assert_eq!(rfc_secret().code(counter as u64), code, "counter {counter}");
        }
    }

    #[test]
    fn makes_the_totp_values_of_rfc_6238() {
        // The six last digits of the eight-digit SHA-1 vectors of Appendix B.
        for (time, code) in [
            (59, "287082"),
            (1_111_111_109, "081804"),
            (1_111_111_111, "050471"),
            (1_234_567_890, "005924"),
            (2_000_000_000, "279037"),
            (20_000_000_000, "353130"),
        ] {
            assert_eq!(
                rfc_secret().verify(code, time, None),
                Some(time / STEP_SECONDS)
            );
        }
    }

    #[test]
    fn accepts_skew_and_refuses_reuse() {
        let secret = rfc_secret();
        let now = 1_234_567_890;
        let step = now / STEP_SECONDS;
        let code = |step| format!("{:06}", secret.code(step));
        assert_eq!(secret.verify(&code(step - 1), now, None), Some(step - 1));
        assert_eq!(secret.verify(&code(step + 1), now, None), Some(step + 1));
        assert_eq!(secret.verify(&code(step + 2), now, None), None);
        // A code is good once, and so is any code of an earlier step.
        assert_eq!(secret.verify(&code(step), now, Some(step)), None);
        assert_eq!(secret.verify(&code(step - 1), now, Some(step)), None);
        assert_eq!(
            secret.verify(&code(step + 1), now, Some(step)),
            Some(step + 1)
        );
        assert_eq!(secret.verify(" 005924 ", now, None), Some(step));
        assert_eq!(secret.verify("5924", now, None), None);
        assert_eq!(secret.verify("00592a", now, None), None);
    }

    #[test]
    fn round_trips_base32() {
        // RFC 4648, Section 10, without padding.
        for (bytes, text) in [
            ("", ""),
            ("f", "MY"),
            ("fo", "MZXQ"),
            ("foo", "MZXW6"),
            ("foob", "MZXW6YQ"),
            ("fooba", "MZXW6YTB"),
            ("foobar", "MZXW6YTBOI"),
        ] {
            assert_eq!(encode_base32(bytes.as_bytes()), text);
            assert_eq!(decode_base32(text).unwrap(), bytes.as_bytes());
        }
        assert_eq!(decode_base32("mzxw 6ytb-oi======").unwrap(), b"foobar");
        assert!(decode_base32("MZXW1").is_none());
        let secret = TotpSecret::generate().unwrap();
        assert_eq!(
            TotpSecret::from_base32(&secret.to_base32()).unwrap(),
            secret
        );
        assert_eq!(TotpSecret::from_base32("MZXW6"), Err(SecretError::TooShort));
        assert_eq!(format!("{secret:?}"), "TotpSecret(..)");
    }

    #[test]
    fn writes_enrollment_uris() {
        let secret = TotpSecret(b"12345678901234567890".to_vec());
        assert_eq!(
            secret.enrollment_uri("Example Corp", "ian@corp.example.com"),
            "otpauth://totp/Example%20Corp:ian%40corp.example.com\
             ?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Example%20Corp\
             &algorithm=SHA1&digits=6&period=30"
        );
    }
}
