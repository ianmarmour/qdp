//! Unidirectional streams (Section 3.5.2): sending a whole stream in the
//! background, and receiving the peer's streams, each read to its end in the
//! background before the application sees it.

use std::sync::Arc;

use qdp::message::StreamHeader;
use qdp::registry::ErrorCode;
use quinn::{Connection, RecvStream};
use tokio::sync::mpsc;
use tracing::debug;

use crate::TransportError;
use crate::connection::quic_code;

/// The largest chunk read from a stream at once.
const READ_CHUNK: usize = 64 << 10;

/// The complete streams waiting for the application. When it falls behind,
/// reading further streams waits.
const RECEIVED_QUEUE: usize = 16;

/// How much of a stream's data to read, and the code that aborts a stream
/// with more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamLimit {
    /// The most bytes accepted after the stream's header.
    pub max_bytes: usize,
    /// The STOP_SENDING code sent if the stream carries more.
    pub overflow: ErrorCode,
}

/// What to do with a stream the peer opened, by its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamPolicy {
    /// Read the stream to its end, within the limit.
    Read(StreamLimit),
    /// Abort reading with STOP_SENDING carrying this code, as for unknown
    /// stream types.
    Refuse(ErrorCode),
    /// Close the connection with this code: the peer must not open the
    /// stream, as for known streams in the wrong direction.
    Forbid(ErrorCode),
}

/// Why a stream's data could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFailure {
    /// The stream carried more than its limit, and reading was aborted with
    /// the limit's code.
    TooLong,
    /// The peer reset the stream with this code.
    Reset(u64),
    /// The connection ended before the stream did.
    ConnectionLost,
}

/// A stream the peer opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedStream {
    /// The stream's header.
    pub header: StreamHeader,
    /// The data after the header, up to the end of the stream.
    pub data: Result<Vec<u8>, StreamFailure>,
}

/// Sends `data` on a new unidirectional stream and finishes it.
pub(crate) async fn write_stream(
    connection: &Connection,
    data: &[u8],
) -> Result<(), TransportError> {
    let mut stream = connection.open_uni().await?;
    stream.write_all(data).await?;
    stream
        .finish()
        .map_err(|_| quinn::WriteError::ClosedStream)?;
    Ok(())
}

/// Accepts the peer's streams until the connection ends, reading each in a
/// task of its own and queueing the complete ones on `received`.
async fn accept_streams(
    connection: Connection,
    policy: Arc<dyn Fn(&StreamHeader) -> StreamPolicy + Send + Sync>,
    received: mpsc::Sender<ReceivedStream>,
) {
    while let Ok(stream) = connection.accept_uni().await {
        let connection = connection.clone();
        let policy = Arc::clone(&policy);
        let received = received.clone();
        tokio::spawn(async move {
            if let Some(stream) = read_stream(&connection, stream, &*policy).await {
                let _ = received.send(stream).await;
            }
        });
    }
}

/// Reads one stream: its header, then its data as `policy` directs. Returns
/// `None` for streams that end before their header does, which are ignored,
/// and for streams that are refused or forbidden.
async fn read_stream(
    connection: &Connection,
    mut stream: RecvStream,
    policy: &(dyn Fn(&StreamHeader) -> StreamPolicy + Send + Sync),
) -> Option<ReceivedStream> {
    let mut buffer = Vec::new();
    let (header, header_length) = loop {
        if let Some(decoded) = StreamHeader::decode(&buffer) {
            break decoded;
        }
        match stream.read_chunk(READ_CHUNK, true).await {
            Ok(Some(chunk)) => buffer.extend_from_slice(&chunk.bytes),
            Ok(None) | Err(_) => return None,
        }
    };
    let limit = match policy(&header) {
        StreamPolicy::Read(limit) => limit,
        StreamPolicy::Refuse(code) => {
            debug!(?header, "stream refused");
            let _ = stream.stop(quic_code(code));
            return None;
        }
        StreamPolicy::Forbid(code) => {
            connection.close(quic_code(code), b"a stream was opened that is not allowed");
            return None;
        }
    };
    buffer.drain(..header_length);
    let data = loop {
        if buffer.len() > limit.max_bytes {
            let _ = stream.stop(quic_code(limit.overflow));
            break Err(StreamFailure::TooLong);
        }
        match stream.read_chunk(READ_CHUNK, true).await {
            Ok(Some(chunk)) => buffer.extend_from_slice(&chunk.bytes),
            Ok(None) => break Ok(buffer),
            Err(quinn::ReadError::Reset(code)) => {
                break Err(StreamFailure::Reset(code.into_inner()));
            }
            Err(_) => break Err(StreamFailure::ConnectionLost),
        }
    };
    Some(ReceivedStream { header, data })
}

/// Returns the queue that receives the peer's complete streams, and starts
/// accepting them with `policy`.
pub(crate) fn receive_streams(
    connection: Connection,
    policy: Arc<dyn Fn(&StreamHeader) -> StreamPolicy + Send + Sync>,
) -> mpsc::Receiver<ReceivedStream> {
    let (sender, receiver) = mpsc::channel(RECEIVED_QUEUE);
    tokio::spawn(accept_streams(connection, policy, sender));
    receiver
}
