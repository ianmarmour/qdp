//! Encoding primitives shared by every QDP message (Sections 1.5, 3.6 and 3.7).
//!
//! [`Reader`] and [`Writer`] read and write the field types of Section 1.5:
//! variable-length integers, fixed-width integers, strings and octet sequences.
//! [`ControlStreamDecoder`] splits the control stream into control messages and
//! [`DatagramFrame`] splits a QUIC datagram into its type and payload.

mod error;
mod framing;
mod reader;
mod varint;
mod writer;

pub use error::WireError;
pub use framing::{
    ControlMessageFrame, ControlStreamDecoder, DatagramFrame, MAX_CONTROL_MESSAGE_LENGTH,
    PRE_AUTHENTICATION_BUDGET,
};
pub use reader::Reader;
pub use varint::{SignedVarInt, VarInt, VarIntOutOfRange};
pub use writer::Writer;
