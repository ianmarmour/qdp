//! The client's side of the control stream (Section 4).

use std::collections::VecDeque;
use std::time::Instant;

use crate::message::{
    AuthContinue, AuthMethod, AuthResult, AuthStart, AuthStatus, ClientHello, ClientReady,
    ControlMessage, DeviceKind, HostHello, MAX_AUTH_DATA_LENGTH, MessagePayload,
};
use crate::registry::{ErrorCode, MessageType};
use crate::session::{
    Negotiated, Phase, Role, SESSION_START_TIMEOUT, SessionError, UsageError, Verdict, check,
};
use crate::wire::{ControlMessageFrame, ControlStreamDecoder};

/// Something the client application must look at or answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    /// The host's hello arrived and the session's features are negotiated. The
    /// application starts authentication with [`ClientSession::start_auth`] unless
    /// it already pipelined an attempt.
    HostHello(HostHello),
    /// The host continued the current attempt. The application answers with
    /// [`ClientSession::continue_auth`].
    AuthContinue {
        /// The method-specific data of `AUTH_CONTINUE`.
        auth_data: Vec<u8>,
    },
    /// The host ended the current attempt. On success the session is in the
    /// `CONFIGURING` phase; on failure the application may start another attempt
    /// and must send its configuration again after a later success.
    AuthResult(AuthResult),
    /// A configuration or active-phase message from the host.
    Message(ControlMessage),
    /// `SESSION_START` arrived and the session is active.
    SessionStarted,
}

/// The client's control stream state machine.
#[derive(Debug)]
pub struct ClientSession {
    /// The hello the client sent.
    client_hello: ClientHello,
    /// The current phase.
    phase: Phase,
    /// Splits the host's direction of the control stream into messages.
    decoder: ControlStreamDecoder,
    /// Encoded messages waiting to be written to the control stream.
    transmit: Vec<u8>,
    /// Events waiting for the application.
    events: VecDeque<ClientEvent>,
    /// The host's hello, once it has arrived.
    host_hello: Option<HostHello>,
    /// The negotiated features, once `HOST_HELLO` has arrived.
    negotiated: Option<Negotiated>,
    /// Whether an authentication attempt awaits the host's answer.
    attempt_in_progress: bool,
    /// When `CLIENT_READY` was sent, in the current configuration.
    ready_sent_at: Option<Instant>,
    /// Why the session ended, once it has.
    close_reason: Option<SessionError>,
}

impl ClientSession {
    /// Creates the session after the QUIC handshake and queues `CLIENT_HELLO`, the
    /// first message on the control stream.
    pub fn new(client_hello: ClientHello) -> Result<Self, UsageError> {
        let mut session = Self {
            client_hello,
            phase: Phase::Hello,
            decoder: ControlStreamDecoder::new(),
            transmit: Vec::new(),
            events: VecDeque::new(),
            host_hello: None,
            negotiated: None,
            attempt_in_progress: false,
            ready_sent_at: None,
            close_reason: None,
        };
        let hello = session.client_hello.to_frame();
        session.queue(&hello)?;
        Ok(session)
    }

    /// Returns the current phase.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Returns the host's hello, once it has arrived.
    pub fn host_hello(&self) -> Option<&HostHello> {
        self.host_hello.as_ref()
    }

    /// Returns the negotiated features, once `HOST_HELLO` has arrived.
    pub fn negotiated(&self) -> Option<&Negotiated> {
        self.negotiated.as_ref()
    }

    /// Returns why the session ended, once it has.
    pub fn close_reason(&self) -> Option<&SessionError> {
        self.close_reason.as_ref()
    }

    /// Returns whether the client may send datagrams and open unidirectional
    /// streams, which it may only after `SESSION_START` (Section 4.5).
    pub fn can_send_media(&self) -> bool {
        self.phase == Phase::Active
    }

    /// Returns whether host datagrams and streams are accepted. Before
    /// `SESSION_START` the client buffers them; before authentication succeeds it
    /// discards them (Section 3.7.3).
    pub fn accepts_media(&self) -> bool {
        matches!(self.phase, Phase::Configuring | Phase::Active)
    }

    /// Returns the bytes waiting to be written to the control stream, if any.
    pub fn poll_transmit(&mut self) -> Option<Vec<u8>> {
        (!self.transmit.is_empty()).then(|| std::mem::take(&mut self.transmit))
    }

    /// Returns the next event for the application, if any.
    pub fn poll_event(&mut self) -> Option<ClientEvent> {
        self.events.pop_front()
    }

    /// Returns when [`ClientSession::handle_timeout`] must next be called, if the
    /// `SESSION_START` timer is running.
    pub fn poll_timeout(&self) -> Option<Instant> {
        match self.phase {
            Phase::Configuring => self
                .ready_sent_at
                .map(|sent_at| sent_at + SESSION_START_TIMEOUT),
            _ => None,
        }
    }

    /// Closes the session with `QDP_HANDSHAKE_TIMEOUT` if `SESSION_START` has not
    /// arrived in time.
    pub fn handle_timeout(&mut self, now: Instant) -> Result<(), SessionError> {
        match self.poll_timeout() {
            Some(deadline) if now >= deadline => Err(self.close(SessionError::new(
                ErrorCode::HANDSHAKE_TIMEOUT,
                "SESSION_START did not arrive in time",
            ))),
            _ => Ok(()),
        }
    }

    /// Ends the session, for example when the user disconnects.
    pub fn close(&mut self, error: SessionError) -> SessionError {
        self.phase = Phase::Closed;
        self.close_reason.get_or_insert(error).clone()
    }

    /// Starts an authentication attempt.
    ///
    /// This may be called before `HOST_HELLO` arrives, to pipeline a method the
    /// host is known to accept (Section 4.4.6).
    pub fn start_auth(&mut self, method: AuthMethod, auth_data: Vec<u8>) -> Result<(), UsageError> {
        if !matches!(self.phase, Phase::Hello | Phase::Authenticating) || self.attempt_in_progress {
            return Err(UsageError::InvalidState(
                "an attempt can start only while authenticating and idle",
            ));
        }
        if let Some(host_hello) = &self.host_hello {
            if !host_hello.auth_methods.contains(&method) {
                return Err(UsageError::InvalidState(
                    "the host did not offer this method",
                ));
            }
        }
        check_auth_data(&auth_data)?;
        self.queue(
            &AuthStart {
                auth_method: method,
                auth_data,
            }
            .to_frame(),
        )?;
        self.attempt_in_progress = true;
        Ok(())
    }

    /// Answers the host's `AUTH_CONTINUE`.
    pub fn continue_auth(&mut self, auth_data: Vec<u8>) -> Result<(), UsageError> {
        if !self.attempt_in_progress {
            return Err(UsageError::InvalidState("no attempt is in progress"));
        }
        check_auth_data(&auth_data)?;
        self.queue(&AuthContinue { auth_data }.to_frame())
    }

    /// Sends a configuration or active-phase message to the host.
    ///
    /// Configuration messages may be pipelined while an attempt is in progress
    /// (Section 4.4.6), except passthrough device attachments. Hello and
    /// authentication messages are sent through the session's own methods, and
    /// `CLIENT_READY` through [`ClientSession::ready`].
    pub fn send(&mut self, message: &ControlMessage) -> Result<(), UsageError> {
        let message_type = message.message_type();
        let not_allowed = UsageError::NotAllowed {
            message_type,
            phase: self.phase,
        };
        let owned_by_session = matches!(
            message_type,
            MessageType::CLIENT_HELLO
                | MessageType::AUTH_START
                | MessageType::AUTH_CONTINUE
                | MessageType::CLIENT_READY
        );
        if owned_by_session {
            return Err(not_allowed);
        }
        match self.verdict(message_type) {
            Verdict::Allowed => {}
            Verdict::Pipelined => {
                if let ControlMessage::DeviceAttach(attach) = message {
                    if attach.kind == DeviceKind::PASSTHROUGH {
                        return Err(UsageError::InvalidState(
                            "passthrough devices may not be pipelined",
                        ));
                    }
                }
            }
            Verdict::Violation => return Err(not_allowed),
        }
        self.queue(&message.to_frame())
    }

    /// Sends `CLIENT_READY`, once the configuration messages have been sent.
    pub fn ready(&mut self, now: Instant) -> Result<(), UsageError> {
        if self.ready_sent_at.is_some()
            || self.verdict(MessageType::CLIENT_READY) == Verdict::Violation
        {
            return Err(UsageError::NotAllowed {
                message_type: MessageType::CLIENT_READY,
                phase: self.phase,
            });
        }
        self.queue(&ClientReady.to_frame())?;
        self.ready_sent_at = Some(now);
        Ok(())
    }

    /// Processes bytes received on the host's direction of the control stream.
    pub fn handle_control_data(&mut self, data: &[u8]) -> Result<(), SessionError> {
        if self.phase == Phase::Closed {
            return Ok(());
        }
        self.decoder.push(data);
        loop {
            let frame = match self.decoder.next_message() {
                Ok(Some(frame)) => frame,
                Ok(None) => return Ok(()),
                Err(error) => return Err(self.close(error.into())),
            };
            if let Err(error) = self.handle_frame(&frame) {
                return Err(self.close(error));
            }
        }
    }

    /// Decides whether the client may send `message_type` now, treating the hello
    /// phase with a pipelined attempt like the authenticating phase.
    fn verdict(&self, message_type: MessageType) -> Verdict {
        let verdict = match self.phase {
            Phase::Hello => check(Role::Client, Phase::Authenticating, false, message_type),
            phase => check(
                Role::Client,
                phase,
                self.ready_sent_at.is_some(),
                message_type,
            ),
        };
        if verdict == Verdict::Pipelined && !self.attempt_in_progress {
            Verdict::Violation
        } else {
            verdict
        }
    }

    /// Appends an encoded message to the control stream.
    fn queue(&mut self, frame: &ControlMessageFrame) -> Result<(), UsageError> {
        let bytes = frame.to_bytes().map_err(|_| UsageError::TooLarge)?;
        self.transmit.extend_from_slice(&bytes);
        Ok(())
    }

    /// Processes one received control message frame.
    fn handle_frame(&mut self, frame: &ControlMessageFrame) -> Result<(), SessionError> {
        let message = ControlMessage::decode(frame)?;
        match (self.phase, message) {
            (Phase::Closed, _) => Ok(()),
            (Phase::Hello, ControlMessage::HostHello(hello)) => {
                self.negotiated = Some(Negotiated::new(&self.client_hello, &hello)?);
                self.host_hello = Some(hello.clone());
                self.phase = Phase::Authenticating;
                self.events.push_back(ClientEvent::HostHello(hello));
                Ok(())
            }
            (Phase::Hello, _) => Err(SessionError::protocol_violation(
                "the first message must be HOST_HELLO",
            )),
            (_, ControlMessage::HostHello(_)) => Err(SessionError::protocol_violation(
                "HOST_HELLO sent more than once",
            )),
            (_, message) => self.handle_message(message),
        }
    }

    /// Processes a message after the hello, according to the table of Section 4.2.
    fn handle_message(&mut self, message: ControlMessage) -> Result<(), SessionError> {
        let client_ready = self.ready_sent_at.is_some();
        if check(Role::Host, self.phase, client_ready, message.message_type()) != Verdict::Allowed {
            return Err(SessionError::protocol_violation(format!(
                "{} is not allowed in the {:?} phase",
                message.message_type(),
                self.phase
            )));
        }
        match message {
            ControlMessage::AuthContinue(next) => {
                if !self.attempt_in_progress {
                    return Err(SessionError::protocol_violation(
                        "AUTH_CONTINUE outside an attempt",
                    ));
                }
                self.events.push_back(ClientEvent::AuthContinue {
                    auth_data: next.auth_data,
                });
            }
            ControlMessage::AuthResult(result) => {
                if !self.attempt_in_progress {
                    return Err(SessionError::protocol_violation(
                        "AUTH_RESULT outside an attempt",
                    ));
                }
                self.attempt_in_progress = false;
                if result.status == AuthStatus::Success {
                    self.phase = Phase::Configuring;
                } else {
                    // The host discards pipelined configuration after a failed
                    // attempt, so it must be sent again (Section 4.4.6).
                    self.ready_sent_at = None;
                }
                self.events.push_back(ClientEvent::AuthResult(result));
            }
            ControlMessage::SessionStart(_) => {
                self.phase = Phase::Active;
                self.events.push_back(ClientEvent::SessionStarted);
            }
            ControlMessage::Unknown(_) => {}
            message => self.events.push_back(ClientEvent::Message(message)),
        }
        Ok(())
    }
}

/// Checks that Auth Data fits the limit of Section 4.4.1.
fn check_auth_data(auth_data: &[u8]) -> Result<(), UsageError> {
    if auth_data.len() > MAX_AUTH_DATA_LENGTH {
        Err(UsageError::TooLarge)
    } else {
        Ok(())
    }
}
