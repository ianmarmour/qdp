//! Clipboard sharing (Section 10): offers when the local clipboard changes,
//! requests for the peer's offers, and the data that answers them.
//!
//! The endpoint shares plain text, the format every endpoint supports. The
//! operating system clipboards this runs on cannot all hold a promise of data
//! to come, so text the peer offers is requested as soon as it is offered and
//! placed on the local clipboard when it arrives; the requester's Max Size
//! bounds it.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use qdp::message::{
    Capability, ClipboardCapability, ClipboardOffer, ClipboardReject, ClipboardRequest,
    ClipboardRequestId, ControlMessage, Directions, OfferId, PLAIN_TEXT_FORMAT, RejectReason,
};
use qdp::registry::ErrorCode;
use qdp::session::{ClipboardLimit, Negotiated};
use qdp::wire::VarInt;
use qdp_quic::{StreamFailure, StreamLimit, StreamPolicy};
use tracing::{debug, warn};

/// How often the clipboard is checked for changes.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// How long a request waits for its answer (Section 10.3).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// The most requests served for one offer, so that a peer cannot make the
/// endpoint read its clipboard without end (Section 10.3).
const MAX_SERVES_PER_OFFER: u32 = 16;

/// The name of the clipboard thread, as debuggers and profilers show it.
const THREAD_NAME: &str = "qdp-clipboard";

/// The largest clipboard data the endpoint accepts, in bytes.
const MAX_SIZE: u32 = 16 << 20;

/// Which way an endpoint shares its clipboard, as the user or policy allows
/// (Section 4.6.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClipboardDirections {
    /// The endpoint offers its clipboard and accepts the peer's.
    Both,
    /// The endpoint only offers its clipboard.
    SendOnly,
    /// The endpoint only accepts the peer's clipboard.
    ReceiveOnly,
}

impl ClipboardDirections {
    /// Returns the directions as the capability's mask.
    fn mask(self) -> Directions {
        match self {
            Self::Both => Directions(Directions::SEND.0 | Directions::RECEIVE.0),
            Self::SendOnly => Directions::SEND,
            Self::ReceiveOnly => Directions::RECEIVE,
        }
    }
}

/// A clipboard to share in a session, and which way.
pub struct SharedClipboard {
    /// The clipboard.
    pub clipboard: Box<dyn Clipboard>,
    /// Which way it is shared.
    pub directions: ClipboardDirections,
}

impl std::fmt::Debug for SharedClipboard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SharedClipboard")
            .field("clipboard", &self.clipboard.description())
            .field("directions", &self.directions)
            .finish()
    }
}

/// Returns the clipboard capability of an endpoint sharing its clipboard
/// `directions` (Section 4.6.6).
pub(crate) fn capability(directions: ClipboardDirections) -> Capability {
    Capability::Clipboard(ClipboardCapability {
        directions: directions.mask(),
        max_size: VarInt::from_u32(MAX_SIZE),
    })
}

/// Returns how to read a `CLIPBOARD_DATA` stream: within the endpoint's Max
/// Size when the peer may offer, and refused otherwise (Section 10.4).
pub(crate) fn data_stream_policy(receive_limit: Option<usize>) -> StreamPolicy {
    match receive_limit {
        Some(max_bytes) => StreamPolicy::Read(StreamLimit {
            max_bytes,
            overflow: ErrorCode::RESOURCE_LIMIT,
        }),
        None => StreamPolicy::Refuse(ErrorCode::REQUEST_CANCELLED),
    }
}

/// The local system clipboard, as far as sessions share it: plain text.
pub trait Clipboard: Send {
    /// Returns the clipboard's text, or `None` if it holds no text, or
    /// content its platform marks as concealed or transient, such as a
    /// password copied from a password manager (Section 10.2).
    fn text(&mut self) -> Result<Option<String>, String>;

    /// Replaces the clipboard's contents with `text`.
    fn set_text(&mut self, text: &str) -> Result<(), String>;

    /// Returns which clipboard this is, for logs.
    fn description(&self) -> &str;
}

/// Which end of the session the endpoint is, which decides who offers first
/// and whose offer wins when two cross (Section 10.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    /// The host.
    Host,
    /// The client.
    Client,
}

/// What the clipboard logic asks the endpoint to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClipboardOutput {
    /// Send a control message.
    Message(ControlMessage),
    /// Answer a request with data on a `CLIPBOARD_DATA` stream.
    Data {
        /// The request answered.
        request_id: ClipboardRequestId,
        /// The data.
        data: Vec<u8>,
    },
    /// Put the peer's text on the local clipboard.
    Publish(String),
}

/// Returns whether a format names UTF-8 plain text, compared as Section 10.1
/// asks: case-insensitively and ignoring whitespace.
fn is_plain_text(format: &str) -> bool {
    let canonical: String = format
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    canonical == PLAIN_TEXT_FORMAT
}

/// Returns text with LF line endings, as it is sent (Section 10.4).
fn to_wire_text(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// Returns received text with the platform's line endings.
fn to_local_text(text: String) -> String {
    if cfg!(windows) {
        text.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        text
    }
}

/// Returns a counter as an identifier value, saturating.
fn id_value(value: u64) -> VarInt {
    VarInt::new(value).unwrap_or(VarInt::MAX)
}

/// The clipboard sharing state of one session.
#[derive(Debug)]
pub(crate) struct ClipboardSync {
    /// Which end of the session this is.
    side: Side,
    /// The peer's Max Size, when the endpoint may offer its clipboard.
    send_limit: Option<usize>,
    /// The endpoint's Max Size, when the peer may offer its clipboard.
    receive_limit: Option<usize>,
    /// The local clipboard's text, once read.
    local: Option<Option<String>>,
    /// The text last placed on the local clipboard from the peer, whose
    /// appearance there is not offered back (Section 10.2).
    published: Option<String>,
    /// Whether the session is active.
    started: bool,
    /// Whether offers may be sent: for the host, once the client's initial
    /// offer arrived.
    offers_open: bool,
    /// The Offer ID of the endpoint's latest offer.
    last_offer_id: u64,
    /// The text the latest offer provides.
    offered: Option<String>,
    /// The requests served for the latest offer.
    served: u32,
    /// The highest Offer ID received from the peer.
    highest_peer_offer: OfferId,
    /// The Request ID of the endpoint's latest request.
    last_request_id: u64,
    /// The requests waiting for an answer, and when they were sent.
    requests: HashMap<ClipboardRequestId, Instant>,
}

impl ClipboardSync {
    /// Returns the clipboard state of a session with the negotiated limits.
    pub(crate) fn new(
        side: Side,
        send_limit: ClipboardLimit,
        receive_limit: ClipboardLimit,
    ) -> Self {
        let limit = |limit: ClipboardLimit| {
            limit.map(|size| usize::try_from(size.into_inner()).unwrap_or(usize::MAX))
        };
        Self {
            side,
            send_limit: limit(send_limit),
            receive_limit: limit(receive_limit),
            local: None,
            published: None,
            started: false,
            offers_open: false,
            last_offer_id: 0,
            offered: None,
            served: 0,
            highest_peer_offer: OfferId::NONE,
            last_request_id: 0,
            requests: HashMap::new(),
        }
    }

    /// Returns whether the session shares the clipboard in either direction.
    pub(crate) fn is_shared(&self) -> bool {
        self.send_limit.is_some() || self.receive_limit.is_some()
    }

    /// Returns the largest clipboard data the endpoint accepts, when the
    /// peer may offer its clipboard.
    pub(crate) fn receive_limit(&self) -> Option<usize> {
        self.receive_limit
    }

    /// Starts sharing once the session is active. The client offers first;
    /// the host waits for the client's initial offer, unless the client does
    /// not offer its clipboard.
    pub(crate) fn start(&mut self) -> Vec<ClipboardOutput> {
        self.started = true;
        self.offers_open = match self.side {
            Side::Client => true,
            Side::Host => self.receive_limit.is_none(),
        };
        self.offer_if_open()
    }

    /// Notes the local clipboard's text, as read now, and offers it if it
    /// changed.
    pub(crate) fn local_changed(&mut self, text: Option<String>) -> Vec<ClipboardOutput> {
        let first = self.local.is_none();
        let echo = text.is_some() && text == self.published;
        let changed = self.local.as_ref() != Some(&text);
        self.local = Some(text);
        if echo || !(changed || first) {
            return Vec::new();
        }
        self.offer_if_open()
    }

    /// Offers the local clipboard, if the endpoint may offer now.
    fn offer_if_open(&mut self) -> Vec<ClipboardOutput> {
        let Some(text) = self.local.clone() else {
            return Vec::new();
        };
        if !(self.started && self.offers_open) || self.send_limit.is_none() {
            return Vec::new();
        }
        self.last_offer_id += 1;
        self.served = 0;
        let formats = if text.is_some() {
            vec![PLAIN_TEXT_FORMAT.to_owned()]
        } else {
            Vec::new()
        };
        self.offered = text;
        vec![ClipboardOutput::Message(ControlMessage::ClipboardOffer(
            ClipboardOffer {
                offer_id: OfferId(id_value(self.last_offer_id)),
                peer_offer_id: self.highest_peer_offer,
                formats,
            },
        ))]
    }

    /// Acts on the peer's offer: requests its text unless the offer crossed
    /// one of the endpoint's and lost.
    pub(crate) fn handle_offer(
        &mut self,
        offer: &ClipboardOffer,
        now: Instant,
    ) -> Vec<ClipboardOutput> {
        if self.receive_limit.is_none() {
            debug!("clipboard offer ignored: the peer may not offer");
            return Vec::new();
        }
        self.highest_peer_offer = self.highest_peer_offer.max(offer.offer_id);
        let crossed = offer.peer_offer_id.0.into_inner() < self.last_offer_id;
        if crossed && self.side == Side::Client {
            debug!("the host's clipboard offer crossed the client's, which wins");
            return Vec::new();
        }
        if !offer.formats.is_empty() {
            // The local clipboard is about to hold the peer's content, so the
            // endpoint's own offer can no longer be served.
            self.offered = None;
        }
        let mut outputs = Vec::new();
        if self.side == Side::Host && !self.offers_open {
            // The client's initial offer: the host offers its own clipboard
            // only if the client's was empty, so that the client's wins.
            self.offers_open = true;
            if offer.formats.is_empty() {
                outputs.extend(self.offer_if_open());
            }
        }
        let Some(format) = offer.formats.iter().find(|format| is_plain_text(format)) else {
            return outputs;
        };
        self.last_request_id += 1;
        let request_id = ClipboardRequestId(id_value(self.last_request_id));
        self.requests.insert(request_id, now);
        outputs.push(ClipboardOutput::Message(ControlMessage::ClipboardRequest(
            ClipboardRequest {
                request_id,
                offer_id: offer.offer_id,
                format: format.clone(),
            },
        )));
        outputs
    }

    /// Answers the peer's request with the offered text, or a rejection.
    pub(crate) fn handle_request(&mut self, request: &ClipboardRequest) -> ClipboardOutput {
        let reject = |reason| {
            ClipboardOutput::Message(ControlMessage::ClipboardReject(ClipboardReject {
                request_id: request.request_id,
                reason,
            }))
        };
        let Some(limit) = self.send_limit else {
            return reject(RejectReason::Denied);
        };
        if request.offer_id.0.into_inner() != self.last_offer_id {
            return reject(RejectReason::StaleOffer);
        }
        let Some(text) = self
            .offered
            .as_deref()
            .filter(|_| is_plain_text(&request.format))
        else {
            return reject(RejectReason::Unavailable);
        };
        if self.served >= MAX_SERVES_PER_OFFER {
            return reject(RejectReason::Denied);
        }
        let data = to_wire_text(text).into_bytes();
        if data.len() > limit {
            return reject(RejectReason::TooLarge);
        }
        self.served += 1;
        ClipboardOutput::Data {
            request_id: request.request_id,
            data,
        }
    }

    /// Forgets a request the peer refused.
    pub(crate) fn handle_reject(&mut self, reject: &ClipboardReject) {
        if self.requests.remove(&reject.request_id).is_some() {
            debug!(reason = ?reject.reason, "clipboard request refused");
        }
    }

    /// Acts on data answering one of the endpoint's requests: valid text
    /// goes on the local clipboard.
    pub(crate) fn handle_data(
        &mut self,
        request_id: ClipboardRequestId,
        data: Result<Vec<u8>, StreamFailure>,
    ) -> Vec<ClipboardOutput> {
        if self.requests.remove(&request_id).is_none() {
            debug!("clipboard data for no waiting request ignored");
            return Vec::new();
        }
        let text = match data.map(String::from_utf8) {
            Ok(Ok(text)) => to_local_text(text),
            Ok(Err(_)) => {
                warn!("clipboard text that is not UTF-8 ignored");
                return Vec::new();
            }
            Err(failure) => {
                debug!(?failure, "clipboard data not received");
                return Vec::new();
            }
        };
        self.published = Some(text.clone());
        self.local = Some(Some(text.clone()));
        vec![ClipboardOutput::Publish(text)]
    }

    /// Returns when the oldest waiting request expires.
    pub(crate) fn poll_timeout(&self) -> Option<Instant> {
        self.requests
            .values()
            .min()
            .map(|sent| *sent + REQUEST_TIMEOUT)
    }

    /// Gives up on requests that waited too long.
    pub(crate) fn handle_timeout(&mut self, now: Instant) {
        self.requests
            .retain(|_, sent| now.saturating_duration_since(*sent) < REQUEST_TIMEOUT);
    }
}

/// A thread that owns the system clipboard: it reports the clipboard's text
/// when it changes and writes text to it, so that slow clipboard owners never
/// hold up the session.
pub(crate) struct ClipboardWorker {
    /// Texts to write; dropping it stops the thread.
    writes: Option<Sender<String>>,
    /// The clipboard's text each time it changed.
    changes: Receiver<Option<String>>,
    /// The thread, joined when this is dropped.
    handle: Option<JoinHandle<()>>,
}

impl ClipboardWorker {
    /// Starts the thread, which reads the clipboard at once and then every
    /// [`POLL_INTERVAL`].
    pub(crate) fn spawn(mut clipboard: Box<dyn Clipboard>) -> Result<Self, String> {
        let (write_sender, writes) = mpsc::channel::<String>();
        let (change_sender, changes) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .spawn(move || {
                let mut last: Option<Option<String>> = None;
                loop {
                    let current = match clipboard.text() {
                        Ok(text) => text,
                        Err(error) => {
                            debug!(error, "the clipboard cannot be read");
                            None
                        }
                    };
                    if last.as_ref() != Some(&current) {
                        last = Some(current.clone());
                        if change_sender.send(current).is_err() {
                            return;
                        }
                    }
                    match writes.recv_timeout(POLL_INTERVAL) {
                        Ok(text) => match clipboard.set_text(&text) {
                            Ok(()) => last = Some(Some(text)),
                            Err(error) => warn!(error, "the clipboard cannot be written"),
                        },
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            writes: Some(write_sender),
            changes,
            handle: Some(handle),
        })
    }

    /// Writes text to the clipboard.
    pub(crate) fn set_text(&self, text: String) {
        if let Some(writes) = &self.writes {
            let _ = writes.send(text);
        }
    }

    /// Returns the clipboard's text after its latest change, if it changed
    /// since the last call.
    pub(crate) fn latest_change(&self) -> Option<Option<String>> {
        self.changes.try_iter().last()
    }

    /// Returns when to check for changes next.
    pub(crate) fn next_check(now: Instant) -> Instant {
        now + POLL_INTERVAL
    }
}

/// A session's clipboard sharing and the system clipboard it shares.
pub(crate) struct ClipboardLink {
    /// The sharing state.
    sync: ClipboardSync,
    /// The system clipboard's thread.
    worker: ClipboardWorker,
    /// When to check the system clipboard for changes next.
    next_check: Instant,
}

impl ClipboardLink {
    /// Returns the link of a session that negotiated clipboard sharing in
    /// either direction, or `None`.
    pub(crate) fn new(
        side: Side,
        negotiated: &Negotiated,
        worker: ClipboardWorker,
    ) -> Option<Self> {
        let (send, receive) = match side {
            Side::Host => (negotiated.clipboard_to_client, negotiated.clipboard_to_host),
            Side::Client => (negotiated.clipboard_to_host, negotiated.clipboard_to_client),
        };
        let sync = ClipboardSync::new(side, send, receive);
        sync.is_shared().then(|| Self {
            sync,
            worker,
            next_check: Instant::now(),
        })
    }

    /// Returns the largest clipboard data the endpoint accepts, when the
    /// peer may offer its clipboard.
    pub(crate) fn receive_limit(&self) -> Option<usize> {
        self.sync.receive_limit()
    }

    /// Starts sharing once the session is active.
    pub(crate) fn start(&mut self) -> Vec<ClipboardOutput> {
        let mut outputs = self.service(Instant::now());
        outputs.extend(self.sync.start());
        outputs
    }

    /// Returns when the link next needs [`ClipboardLink::service`].
    pub(crate) fn poll_timeout(&self) -> Instant {
        self.sync
            .poll_timeout()
            .map_or(self.next_check, |expiry| expiry.min(self.next_check))
    }

    /// Takes changes of the system clipboard and expires requests.
    pub(crate) fn service(&mut self, now: Instant) -> Vec<ClipboardOutput> {
        self.sync.handle_timeout(now);
        if now < self.next_check {
            return Vec::new();
        }
        self.next_check = ClipboardWorker::next_check(now);
        match self.worker.latest_change() {
            Some(text) => self.sync.local_changed(text),
            None => Vec::new(),
        }
    }

    /// Acts on a clipboard message from the peer; other messages are
    /// ignored.
    pub(crate) fn handle_message(
        &mut self,
        message: &ControlMessage,
        now: Instant,
    ) -> Vec<ClipboardOutput> {
        match message {
            ControlMessage::ClipboardOffer(offer) => self.sync.handle_offer(offer, now),
            ControlMessage::ClipboardRequest(request) => vec![self.sync.handle_request(request)],
            ControlMessage::ClipboardReject(reject) => {
                self.sync.handle_reject(reject);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Acts on a `CLIPBOARD_DATA` stream from the peer.
    pub(crate) fn handle_data(
        &mut self,
        request_id: ClipboardRequestId,
        data: Result<Vec<u8>, StreamFailure>,
    ) -> Vec<ClipboardOutput> {
        self.sync.handle_data(request_id, data)
    }

    /// Puts the peer's text on the system clipboard.
    pub(crate) fn publish(&self, text: String) {
        self.worker.set_text(text);
    }
}

impl Drop for ClipboardWorker {
    fn drop(&mut self) {
        // Closing the write channel ends the thread's loop.
        self.writes = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The negotiated limit of a direction that is shared.
    const LIMIT: ClipboardLimit = Some(VarInt::from_u32(1 << 20));

    /// Returns the offer among outputs.
    fn offer(outputs: &[ClipboardOutput]) -> ClipboardOffer {
        outputs
            .iter()
            .find_map(|output| match output {
                ClipboardOutput::Message(ControlMessage::ClipboardOffer(offer)) => {
                    Some(offer.clone())
                }
                _ => None,
            })
            .expect("an offer")
    }

    /// Returns the request among outputs.
    fn request(outputs: &[ClipboardOutput]) -> ClipboardRequest {
        outputs
            .iter()
            .find_map(|output| match output {
                ClipboardOutput::Message(ControlMessage::ClipboardRequest(request)) => {
                    Some(request.clone())
                }
                _ => None,
            })
            .expect("a request")
    }

    /// Text copied on the client reaches the host: offer, request, data.
    #[test]
    fn shares_text() {
        let now = Instant::now();
        let mut client = ClipboardSync::new(Side::Client, LIMIT, LIMIT);
        let mut host = ClipboardSync::new(Side::Host, LIMIT, LIMIT);
        assert!(host.start().is_empty(), "the host waits for the client");
        assert!(host.local_changed(Some("host text".to_owned())).is_empty());
        assert!(client.local_changed(Some("a\r\nb".to_owned())).is_empty());
        let initial = offer(&client.start());
        assert_eq!(initial.formats, [PLAIN_TEXT_FORMAT]);

        // The client's offer has text, so the host does not offer its own.
        let outputs = host.handle_offer(&initial, now);
        assert_eq!(outputs.len(), 1);
        let asked = request(&outputs);
        let ClipboardOutput::Data { request_id, data } = client.handle_request(&asked) else {
            panic!("data answers the request");
        };
        assert_eq!(data, b"a\nb", "line endings are LF on the wire");
        let published = host.handle_data(request_id, Ok(data));
        assert_eq!(
            published,
            [ClipboardOutput::Publish(to_local_text("a\nb".to_owned()))]
        );

        // Seeing its own publication on the clipboard, the host offers nothing.
        assert!(
            host.local_changed(Some(to_local_text("a\nb".to_owned())))
                .is_empty()
        );
        // A new copy on the host is offered, naming the client's offer.
        let next = offer(&host.local_changed(Some("new".to_owned())));
        assert_eq!(next.peer_offer_id, initial.offer_id);
    }

    /// The host offers first when the client's initial offer is empty, and
    /// when the client does not offer at all.
    #[test]
    fn host_offers_after_an_empty_client_offer() {
        let now = Instant::now();
        let mut client = ClipboardSync::new(Side::Client, LIMIT, LIMIT);
        let mut host = ClipboardSync::new(Side::Host, LIMIT, LIMIT);
        host.local_changed(Some("host".to_owned()));
        host.start();
        client.local_changed(None);
        let empty = offer(&client.start());
        assert!(empty.formats.is_empty());
        let outputs = host.handle_offer(&empty, now);
        assert_eq!(offer(&outputs).formats, [PLAIN_TEXT_FORMAT]);

        let mut receive_only = ClipboardSync::new(Side::Host, LIMIT, None);
        receive_only.local_changed(Some("host".to_owned()));
        assert_eq!(receive_only.start().len(), 1);
    }

    /// When offers cross, the client's wins on both sides.
    #[test]
    fn the_client_wins_crossed_offers() {
        let now = Instant::now();
        let mut client = ClipboardSync::new(Side::Client, LIMIT, LIMIT);
        let mut host = ClipboardSync::new(Side::Host, LIMIT, LIMIT);
        client.local_changed(None);
        host.local_changed(None);
        host.start();
        let initial = offer(&client.start());
        host.handle_offer(&initial, now);
        let from_host = offer(&host.local_changed(Some("host".to_owned())));
        let from_client = offer(&client.local_changed(Some("client".to_owned())));
        assert!(
            client.handle_offer(&from_host, now).is_empty(),
            "the client ignores the host's"
        );
        let outputs = host.handle_offer(&from_client, now);
        assert_eq!(request(&outputs).offer_id, from_client.offer_id);
    }

    /// Requests are refused for replaced offers, missing formats, sizes over
    /// the limit, and directions that are not shared.
    #[test]
    fn rejects_bad_requests() {
        let mut host = ClipboardSync::new(Side::Host, Some(VarInt::from_u32(4)), None);
        host.local_changed(Some("12345".to_owned()));
        let offer = offer(&host.start());
        let ask = |offer_id: OfferId, format: &str| ClipboardRequest {
            request_id: ClipboardRequestId(VarInt::from_u32(1)),
            offer_id,
            format: format.to_owned(),
        };
        let reason = |output: ClipboardOutput| match output {
            ClipboardOutput::Message(ControlMessage::ClipboardReject(reject)) => reject.reason,
            other => panic!("a rejection: {other:?}"),
        };
        assert_eq!(
            reason(host.handle_request(&ask(OfferId::NONE, PLAIN_TEXT_FORMAT))),
            RejectReason::StaleOffer
        );
        assert_eq!(
            reason(host.handle_request(&ask(offer.offer_id, "image/png"))),
            RejectReason::Unavailable
        );
        assert_eq!(
            reason(host.handle_request(&ask(offer.offer_id, "Text/Plain; Charset=UTF-8"))),
            RejectReason::TooLarge
        );
        let mut closed = ClipboardSync::new(Side::Host, None, LIMIT);
        assert_eq!(
            reason(closed.handle_request(&ask(offer.offer_id, PLAIN_TEXT_FORMAT))),
            RejectReason::Denied
        );
    }

    /// Unanswered requests expire, and late or invalid data is ignored.
    #[test]
    fn expires_requests() {
        let now = Instant::now();
        let mut client = ClipboardSync::new(Side::Client, LIMIT, LIMIT);
        client.local_changed(None);
        client.start();
        let offer = ClipboardOffer {
            offer_id: OfferId(VarInt::from_u32(1)),
            peer_offer_id: OfferId(VarInt::from_u32(1)),
            formats: vec![PLAIN_TEXT_FORMAT.to_owned()],
        };
        let asked = request(&client.handle_offer(&offer, now));
        assert_eq!(client.poll_timeout(), Some(now + REQUEST_TIMEOUT));
        client.handle_timeout(now + REQUEST_TIMEOUT);
        assert_eq!(client.poll_timeout(), None);
        assert!(
            client
                .handle_data(asked.request_id, Ok(b"late".to_vec()))
                .is_empty()
        );

        let asked = request(&client.handle_offer(&offer, now));
        assert!(
            client
                .handle_data(asked.request_id, Ok(vec![0xff]))
                .is_empty()
        );
    }

    /// Format names match ignoring case and whitespace.
    #[test]
    fn matches_format_names() {
        assert!(is_plain_text("text/plain;charset=utf-8"));
        assert!(is_plain_text(" Text/Plain ; charset = UTF-8 "));
        assert!(!is_plain_text("text/plain"));
        assert!(!is_plain_text("text/html"));
    }
}
