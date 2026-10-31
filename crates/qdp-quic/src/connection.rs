//! An established QDP connection: the QUIC connection and its control stream.

use std::time::Duration;

use bytes::Bytes;
use qdp::auth::scram::{CHANNEL_BINDING_LENGTH, ChannelBinding, EXPORTER_LABEL};
use qdp::registry::ErrorCode;
use quinn::{Connection, RecvStream, SendStream};

use tracing::debug;

use crate::TransportError;
use crate::config::DATAGRAM_SEND_BUFFER;
use crate::streams::write_stream;

/// The largest chunk read from the control stream at once.
const CONTROL_READ_CHUNK: usize = 64 << 10;

/// The QUIC application error code sent if a QDP code cannot be represented,
/// which cannot happen because both are variable-length integers.
const FALLBACK_ERROR_CODE: quinn::VarInt = quinn::VarInt::from_u32(0x01);

/// What a connection's path has seen so far, from the QUIC stack.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PathStats {
    /// The smoothed round-trip time.
    pub rtt: Duration,
    /// The congestion window: how many bytes may be in flight.
    pub congestion_window: u64,
    /// How many times the congestion controller has shrunk the window.
    pub congestion_events: u64,
    /// The packets declared lost.
    pub lost_packets: u64,
    /// The bytes sent in UDP datagrams.
    pub sent_bytes: u64,
}

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

    /// Returns the statistics of the connection's path.
    pub fn path_stats(&self) -> PathStats {
        let stats = self.connection.stats();
        PathStats {
            rtt: stats.path.rtt,
            congestion_window: stats.path.cwnd,
            congestion_events: stats.path.congestion_events,
            lost_packets: stats.path.lost_packets,
            sent_bytes: stats.udp_tx.bytes,
        }
    }

    /// Returns the bytes of datagrams waiting to be sent because the
    /// congestion controller is holding them back. Every queued byte adds
    /// latency, so senders should skip frames rather than let this grow.
    pub fn queued_datagram_bytes(&self) -> usize {
        DATAGRAM_SEND_BUFFER.saturating_sub(self.connection.datagram_send_buffer_space())
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

    /// Sends `data` on a new unidirectional stream, which it finishes, in the
    /// background (Section 3.5.2). The stream starts with its header, which
    /// is part of `data`. A stream the peer aborts is dropped.
    pub fn send_stream(&self, data: Vec<u8>) {
        let connection = self.connection.clone();
        tokio::spawn(async move {
            if let Err(error) = write_stream(&connection, &data).await {
                debug!(%error, "stream not delivered");
            }
        });
    }

    /// Writes bytes to the control stream.
    pub(crate) async fn write_control(&mut self, data: &[u8]) -> Result<(), TransportError> {
        Ok(self.control_send.write_all(data).await?)
    }

    /// Reads the next chunk of the peer's control stream. A finished control
    /// stream is an error (Section 3.5.1). Cancelling the read loses no data.
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
        self.connection.close(quic_code(code), reason.as_bytes());
    }
}

/// Returns the QUIC application error code carrying a QDP error code.
pub(crate) fn quic_code(code: ErrorCode) -> quinn::VarInt {
    quinn::VarInt::from_u64(code.0.into_inner()).unwrap_or(FALLBACK_ERROR_CODE)
}
