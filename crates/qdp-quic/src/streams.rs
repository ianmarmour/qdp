//! Unidirectional streams (Section 3.5.2): sending a whole stream, or one that
//! stays open for data sent over time, in the background; and receiving the
//! peer's streams, each read to its end before the application sees it or
//! followed as its data arrives.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use qdp::message::StreamHeader;
use qdp::registry::ErrorCode;
use quinn::{Connection, RecvStream};
use tokio::sync::{Notify, mpsc};
use tracing::debug;

use crate::TransportError;
use crate::connection::quic_code;

/// The largest chunk read from a stream at once.
const READ_CHUNK: usize = 64 << 10;

/// The complete streams and chunks waiting for the application. When it
/// falls behind, reading further streams waits.
const RECEIVED_QUEUE: usize = 16;

/// The most cancellations kept for streams that have not arrived: the
/// oldest are forgotten first.
const MAX_CANCELLATIONS: usize = 64;

/// The streams the application no longer wants, by header, with the code
/// that aborts each, shared with the tasks that read them.
#[derive(Debug, Clone, Default)]
pub(crate) struct Cancellations(Arc<CancellationState>);

/// What [`Cancellations`] share.
#[derive(Debug, Default)]
struct CancellationState {
    /// The cancelled headers, oldest first.
    headers: Mutex<VecDeque<(StreamHeader, ErrorCode)>>,
    /// Notified when a stream is cancelled.
    changed: Notify,
}

impl Cancellations {
    /// Cancels the stream with `header`, being read or still to arrive: it
    /// is aborted with STOP_SENDING carrying `code`, and its data is not
    /// delivered.
    pub(crate) fn cancel(&self, header: StreamHeader, code: ErrorCode) {
        {
            let mut headers = self
                .0
                .headers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if headers.len() == MAX_CANCELLATIONS {
                headers.pop_front();
            }
            headers.push_back((header, code));
        }
        self.0.changed.notify_waiters();
    }

    /// Takes the cancellation of the stream with `header`, if there is one.
    fn take(&self, header: &StreamHeader) -> Option<ErrorCode> {
        let mut headers = self
            .0
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let index = headers
            .iter()
            .position(|(cancelled, _)| cancelled == header)?;
        headers.remove(index).map(|(_, code)| code)
    }
}

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
    /// Deliver the stream's data in order as it arrives, for streams that
    /// stay open as long as what they carry, such as a device's output
    /// reports.
    Follow,
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

/// Data that arrived on a followed stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamChunk {
    /// Identifies the stream among the connection's streams.
    pub stream: u64,
    /// The stream's header.
    pub header: StreamHeader,
    /// The data that arrived, after what earlier chunks carried.
    pub data: Vec<u8>,
    /// `None` while the stream continues, otherwise how it ended: `Ok` when
    /// the peer finished it.
    pub end: Option<Result<(), StreamFailure>>,
}

/// What the stream readers queue for the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Incoming {
    /// A stream read to its end.
    Complete(ReceivedStream),
    /// Data of a followed stream.
    Chunk(StreamChunk),
}

/// A unidirectional stream that stays open for data sent over time, in
/// order. Dropping it finishes the stream.
#[derive(Debug)]
pub struct OutgoingStream {
    /// The data waiting to be written, in order.
    chunks: mpsc::UnboundedSender<Vec<u8>>,
}

impl OutgoingStream {
    /// Opens a stream in the background and writes `header` to it.
    pub(crate) fn open(connection: Connection, header: Vec<u8>) -> Self {
        let (chunks, mut queued) = mpsc::unbounded_channel::<Vec<u8>>();
        tokio::spawn(async move {
            let written = async {
                let mut stream = connection.open_uni().await?;
                stream.write_all(&header).await?;
                while let Some(chunk) = queued.recv().await {
                    stream.write_all(&chunk).await?;
                }
                stream
                    .finish()
                    .map_err(|_| quinn::WriteError::ClosedStream)?;
                Ok::<(), TransportError>(())
            };
            if let Err(error) = written.await {
                debug!(%error, "stream not delivered");
            }
        });
        Self { chunks }
    }

    /// Queues `data` to be written after everything sent before, returning
    /// `false` if the stream has failed.
    pub fn send(&self, data: Vec<u8>) -> bool {
        self.chunks.send(data).is_ok()
    }
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
/// task of its own and queueing what they carry on `received`.
async fn accept_streams(
    connection: Connection,
    policy: Arc<dyn Fn(&StreamHeader) -> StreamPolicy + Send + Sync>,
    cancellations: Cancellations,
    received: mpsc::Sender<Incoming>,
) {
    while let Ok(stream) = connection.accept_uni().await {
        let connection = connection.clone();
        let policy = Arc::clone(&policy);
        let cancellations = cancellations.clone();
        let received = received.clone();
        tokio::spawn(async move {
            read_stream(&connection, stream, &*policy, &cancellations, &received).await;
        });
    }
}

/// Reads one stream: its header, then its data as `policy` directs, queued
/// on `received`. Streams that end before their header does are ignored, as
/// are streams that are refused, forbidden or among `cancellations`.
async fn read_stream(
    connection: &Connection,
    mut stream: RecvStream,
    policy: &(dyn Fn(&StreamHeader) -> StreamPolicy + Send + Sync),
    cancellations: &Cancellations,
    received: &mpsc::Sender<Incoming>,
) {
    let mut buffer = Vec::new();
    let (header, header_length) = loop {
        if let Some(decoded) = StreamHeader::decode(&buffer) {
            break decoded;
        }
        match stream.read_chunk(READ_CHUNK, true).await {
            Ok(Some(chunk)) => buffer.extend_from_slice(&chunk.bytes),
            Ok(None) | Err(_) => return,
        }
    };
    let limit = match policy(&header) {
        StreamPolicy::Read(limit) => limit,
        StreamPolicy::Follow => {
            buffer.drain(..header_length);
            follow_stream(stream, header, buffer, received).await;
            return;
        }
        StreamPolicy::Refuse(code) => {
            debug!(?header, "stream refused");
            let _ = stream.stop(quic_code(code));
            return;
        }
        StreamPolicy::Forbid(code) => {
            connection.close(quic_code(code), b"a stream was opened that is not allowed");
            return;
        }
    };
    buffer.drain(..header_length);
    let data = loop {
        if buffer.len() > limit.max_bytes {
            let _ = stream.stop(quic_code(limit.overflow));
            break Err(StreamFailure::TooLong);
        }
        // Listening before looking means a cancellation in between is not
        // missed.
        let cancelled = cancellations.0.changed.notified();
        tokio::pin!(cancelled);
        cancelled.as_mut().enable();
        if let Some(code) = cancellations.take(&header) {
            debug!(?header, "stream cancelled");
            let _ = stream.stop(quic_code(code));
            return;
        }
        tokio::select! {
            chunk = stream.read_chunk(READ_CHUNK, true) => match chunk {
                Ok(Some(chunk)) => buffer.extend_from_slice(&chunk.bytes),
                Ok(None) => break Ok(buffer),
                Err(quinn::ReadError::Reset(code)) => {
                    break Err(StreamFailure::Reset(code.into_inner()));
                }
                Err(_) => break Err(StreamFailure::ConnectionLost),
            },
            () = &mut cancelled => {}
        }
    };
    let _ = received
        .send(Incoming::Complete(ReceivedStream { header, data }))
        .await;
}

/// Queues a followed stream's data on `received` as it arrives, starting
/// with `first`, the data read with its header, then its end.
async fn follow_stream(
    mut stream: RecvStream,
    header: StreamHeader,
    first: Vec<u8>,
    received: &mpsc::Sender<Incoming>,
) {
    let id = u64::from(stream.id());
    let chunk = |data: Vec<u8>, end: Option<Result<(), StreamFailure>>| {
        Incoming::Chunk(StreamChunk {
            stream: id,
            header,
            data,
            end,
        })
    };
    if !first.is_empty() && received.send(chunk(first, None)).await.is_err() {
        return;
    }
    let end = loop {
        match stream.read_chunk(READ_CHUNK, true).await {
            Ok(Some(data)) => {
                if received
                    .send(chunk(data.bytes.to_vec(), None))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Ok(None) => break Ok(()),
            Err(quinn::ReadError::Reset(code)) => {
                break Err(StreamFailure::Reset(code.into_inner()));
            }
            Err(_) => break Err(StreamFailure::ConnectionLost),
        }
    };
    let _ = received.send(chunk(Vec::new(), Some(end))).await;
}

/// Returns the queue that receives what the peer's streams carry, and starts
/// accepting them with `policy`.
pub(crate) fn receive_streams(
    connection: Connection,
    policy: Arc<dyn Fn(&StreamHeader) -> StreamPolicy + Send + Sync>,
    cancellations: Cancellations,
) -> mpsc::Receiver<Incoming> {
    let (sender, receiver) = mpsc::channel(RECEIVED_QUEUE);
    tokio::spawn(accept_streams(connection, policy, cancellations, sender));
    receiver
}
