//! Errors raised while decoding QDP data.

use thiserror::Error;

use crate::registry::ErrorCode;

/// An error found while decoding data received from the peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum WireError {
    /// A structure ended before all of its fields were read (Section 1.6, rule 5).
    #[error("unexpected end of data")]
    UnexpectedEnd,

    /// A String field held bytes that are not valid UTF-8 (Section 1.5.4).
    #[error("string is not valid UTF-8")]
    InvalidUtf8,

    /// A control message was longer than the receiver accepts (Sections 3.6.1 and
    /// 11.7).
    #[error("control message of {length} bytes exceeds the limit of {limit} bytes")]
    MessageTooLarge {
        /// The Message Length the peer sent.
        length: u64,
        /// The largest Message Length the receiver accepted at that point.
        limit: u64,
    },

    /// A field held more bytes or entries than the specification allows.
    #[error("{field} exceeds its limit of {limit}")]
    LimitExceeded {
        /// The name of the field, as the specification writes it.
        field: &'static str,
        /// The largest length or count the field may have.
        limit: u64,
    },

    /// A field held a value that the specification does not allow.
    #[error("{field} holds a value that is not allowed")]
    InvalidValue {
        /// The name of the field, as the specification writes it.
        field: &'static str,
    },
}

impl WireError {
    /// Returns the error code with which the receiver closes the connection.
    pub const fn error_code(self) -> ErrorCode {
        match self {
            Self::UnexpectedEnd
            | Self::InvalidUtf8
            | Self::LimitExceeded { .. }
            | Self::InvalidValue { .. } => ErrorCode::PROTOCOL_VIOLATION,
            Self::MessageTooLarge { .. } => ErrorCode::MESSAGE_TOO_LARGE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each error closes the connection with the code the specification requires.
    #[test]
    fn error_codes() {
        assert_eq!(
            WireError::UnexpectedEnd.error_code(),
            ErrorCode::PROTOCOL_VIOLATION
        );
        assert_eq!(
            WireError::InvalidUtf8.error_code(),
            ErrorCode::PROTOCOL_VIOLATION
        );
        let too_large = WireError::MessageTooLarge {
            length: 2,
            limit: 1,
        };
        assert_eq!(too_large.error_code(), ErrorCode::MESSAGE_TOO_LARGE);
        let limit = WireError::LimitExceeded {
            field: "Client Name",
            limit: 255,
        };
        assert_eq!(limit.error_code(), ErrorCode::PROTOCOL_VIOLATION);
        let invalid = WireError::InvalidValue { field: "Status" };
        assert_eq!(invalid.error_code(), ErrorCode::PROTOCOL_VIOLATION);
    }
}
