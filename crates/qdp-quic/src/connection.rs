//! An established QDP connection: the QUIC connection and its control stream.

use std::time::Duration;

use bytes::Bytes;
use qdp::auth::scram::{CHANNEL_BINDING_LENGTH, ChannelBinding, EXPORTER_LABEL};
use qdp::registry::ErrorCode;
use quinn::{Connection, RecvStream, SendStream};

use crate::TransportError;

/// The largest chunk read from the control stream at once.
const CONTROL_READ_CHUNK: usize = 64 << 10;

/// The QUIC application error code sent if a QDP code cannot be represented,
/// which cannot happen because both are variable-length integers.
const FALLBACK_ERROR_CODE: quinn::VarInt = quinn::VarInt::from_u32(0x01);

/// A QUIC connection carrying a QDP session, with its control stream.
#[derive(Debug)]
pub struct QdpConnection {
    /// The QUIC connection.
    connection: Connection,
    /// The local endpoint's direction of the control stream.
    control_send: SendStream,
    /// The peer's direction of the control stream.
    control_recv: RecvStream,
}

impl QdpConnection {
    /// Wraps an established connection and its control stream, closing the
    /// connection with `QDP_TRANSPORT_REQUIREMENTS` if the peer does not support
    /// QUIC datagrams (Section 3.3).
    pub(crate) fn new(
        connection: Connection,
        control_send: SendStream,
        control_recv: RecvStream,
    ) -> Result<Self, TransportError> {
        let connection = Self {
            connection,
            control_send,
            control_recv,
        };
        if connection.connection.max_datagram_size().is_none() {
            connection.close(
                ErrorCode::TRANSPORT_REQUIREMENTS,
                "QUIC datagrams are required",
            );
            return Err(TransportError::DatagramsUnsupported);
        }
        Ok(connection)
    }

    /// Returns the underlying QUIC connection, which can be cloned to send and
    /// receive datagrams and unidirectional streams from other tasks.
    pub fn quic(&self) -> &Connection {
        &self.connection
    }

    /// Returns the connection's smoothed RTT (Section 3.11).
    pub fn smoothed_rtt(&self) -> Duration {
        self.connection.rtt()
    }

    /// Returns the largest datagram that can be sent now, which can change when
    /// the path changes (Section 3.7.2).
    pub fn max_datagram_size(&self) -> usize {
        self.connection.max_datagram_size().unwrap_or_default()
    }

    /// Returns the `tls-exporter` channel binding of the connection, for
    /// SCRAM-SHA-256-PLUS (Section 4.4.4).
    pub fn channel_binding(&self) -> Result<ChannelBinding, TransportError> {
        let mut output = [0; CHANNEL_BINDING_LENGTH];
        self.connection
            .export_keying_material(&mut output, EXPORTER_LABEL, &[])
            .map_err(|_| TransportError::Config("the TLS exporter is unavailable"))?;
        Ok(ChannelBinding(output))
    }

    /// Sends a datagram, which must fit [`QdpConnection::max_datagram_size`].
    pub fn send_datagram(&self, datagram: Vec<u8>) -> Result<(), TransportError> {
        Ok(self.connection.send_datagram(Bytes::from(datagram))?)
    }

    /// Writes bytes to the control stream.
    pub(crate) async fn write_control(&mut self, data: &[u8]) -> Result<(), TransportError> {
        Ok(self.control_send.write_all(data).await?)
    }

    /// Reads the next chunk of the peer's control stream. A finished control
    /// stream is an error (Section 3.5.1).
    pub(crate) async fn read_control(&mut self) -> Result<Bytes, TransportError> {
        match self
            .control_recv
            .read_chunk(CONTROL_READ_CHUNK, true)
            .await?
        {
            Some(chunk) => Ok(chunk.bytes),
            None => Err(TransportError::ControlStreamClosed),
        }
    }

    /// Closes the connection with a QDP error code (Section 3.10).
    pub fn close(&self, code: ErrorCode, reason: &str) {
        let code = quinn::VarInt::from_u64(code.0.into_inner()).unwrap_or(FALLBACK_ERROR_CODE);
        self.connection.close(code, reason.as_bytes());
    }
}
