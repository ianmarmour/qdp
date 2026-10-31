//! Preparation of user names and passwords with the profiles of RFC 8265, as
//! SCRAM in QDP requires (Section 4.4.4).
//!
//! The mapping rules of both profiles are applied in full. The validation rules
//! reject the characters most likely to cause confusion (control characters,
//! and spaces in user names) rather than implementing every derived property
//! of the PRECIS framework, so some exotic code points that RFC 8264 disallows
//! are accepted. Both peers apply the same preparation, so this cannot make
//! authentication succeed with a different password.

use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

/// The first code point of the Halfwidth and Fullwidth Forms block.
const WIDTH_FORMS_START: char = '\u{ff00}';

/// The last code point of the Halfwidth and Fullwidth Forms block.
const WIDTH_FORMS_END: char = '\u{ffef}';

/// The space characters (general category Zs) other than U+0020, which the
/// OpaqueString profile maps to U+0020.
const NON_ASCII_SPACES: [char; 16] = [
    '\u{00a0}', '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}',
    '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200a}', '\u{202f}', '\u{205f}', '\u{3000}',
];

/// An error returned when a string cannot be prepared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PrecisError {
    /// The prepared string is empty.
    #[error("the string is empty")]
    Empty,
    /// The string contains a character the profile does not allow.
    #[error("the string contains a disallowed character")]
    Disallowed,
}

/// Prepares a user name with the UsernameCaseMapped profile (RFC 8265,
/// Section 3.3): width mapping, case mapping and NFC normalization.
pub fn prepare_username(username: &str) -> Result<String, PrecisError> {
    let width_mapped: String = username
        .chars()
        .flat_map(|character| {
            let decomposed: Vec<char> =
                if (WIDTH_FORMS_START..=WIDTH_FORMS_END).contains(&character) {
                    std::iter::once(character).nfkc().collect()
                } else {
                    vec![character]
                };
            decomposed
        })
        .collect();
    let prepared: String = width_mapped.to_lowercase().nfc().collect();
    if prepared.is_empty() {
        return Err(PrecisError::Empty);
    }
    if prepared
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(PrecisError::Disallowed);
    }
    Ok(prepared)
}

/// Prepares a password with the OpaqueString profile (RFC 8265, Section 4.2):
/// non-ASCII spaces become U+0020 and the result is NFC-normalized.
pub fn prepare_password(password: &str) -> Result<String, PrecisError> {
    let prepared: String = password
        .chars()
        .map(|character| {
            if NON_ASCII_SPACES.contains(&character) {
                ' '
            } else {
                character
            }
        })
        .nfc()
        .collect();
    if prepared.is_empty() {
        return Err(PrecisError::Empty);
    }
    if prepared.chars().any(char::is_control) {
        return Err(PrecisError::Disallowed);
    }
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// User names are case-folded, width-mapped and normalized.
    #[test]
    fn usernames() {
        assert_eq!(prepare_username("Ian").unwrap(), "ian");
        assert_eq!(prepare_username("ＩＡＮ").unwrap(), "ian");
        assert_eq!(prepare_username("e\u{301}").unwrap(), "\u{e9}");
        assert_eq!(prepare_username(""), Err(PrecisError::Empty));
        assert_eq!(prepare_username("a b"), Err(PrecisError::Disallowed));
        assert_eq!(prepare_username("a\u{7}"), Err(PrecisError::Disallowed));
    }

    /// Passwords keep their case, map spaces and are normalized.
    #[test]
    fn passwords() {
        assert_eq!(prepare_password("Pass word").unwrap(), "Pass word");
        assert_eq!(prepare_password("Pass\u{3000}word").unwrap(), "Pass word");
        assert_eq!(prepare_password("e\u{301}").unwrap(), "\u{e9}");
        assert_eq!(prepare_password(""), Err(PrecisError::Empty));
        assert_eq!(prepare_password("\u{0}"), Err(PrecisError::Disallowed));
    }
}
