//! The host's side of the control stream (Section 4).

use std::collections::{BTreeSet, VecDeque};
use std::time::Instant;

use crate::message::{
    AuthContinue, AuthMethod, AuthResult, AuthStatus, ClientHello, ControlMessage, DeviceId,
    DeviceKind, HostHello, MAX_AUTH_DATA_LENGTH, MessagePayload, SessionStart,
};
use crate::registry::{ErrorCode, MessageType};
use crate::session::{
    AUTHENTICATION_TIMEOUT, CLIENT_HELLO_TIMEOUT, CLIENT_READY_TIMEOUT, MAX_FAILED_ATTEMPTS,
    Negotiated, Phase, Role, SessionError, UsageError, Verdict, check,
};
use crate::wire::{ControlMessageFrame, ControlStreamDecoder};

/// The host's configuration of one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConfig {
    /// The `HOST_HELLO` to send: the host's name, the authentication methods it
    /// accepts on this connection and its capabilities.
    pub host_hello: HostHello,
    /// The Reason sent with every failed authentication attempt. It should be the
    /// same for every kind of failure (Section 4.4.1).
    pub failure_reason: String,
}

/// Something the host application must look at or answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// The client's hello arrived and the session's features are negotiated.
    ClientHello(ClientHello),
    /// The client started an authentication attempt with an offered method. The
    /// application answers with [`HostSession::respond_auth`].
    AuthStart {
        /// The method of the attempt.
        method: AuthMethod,
        /// The method-specific data of `AUTH_START`.
        auth_data: Vec<u8>,
    },
    /// The client continued the current attempt. The application answers with
    /// [`HostSession::respond_auth`].
    AuthContinue {
        /// The method-specific data of `AUTH_CONTINUE`.
        auth_data: Vec<u8>,
    },
    /// A configuration or active-phase message from the client, including
    /// `CLIENT_READY`.
    Message(ControlMessage),
}

/// The application's answer to an authentication step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthResponse {
    /// Send `AUTH_CONTINUE` with this data and wait for the client.
    Continue(Vec<u8>),
    /// The client is authenticated; send a successful `AUTH_RESULT` with this data.
    Accept(Vec<u8>),
    /// The attempt failed.
    Reject,
}

/// The state of the current authentication attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attempt {
    /// No attempt is in progress.
    Idle,
    /// The application has not yet answered the client's last message.
    AwaitingHost,
    /// The host sent `AUTH_CONTINUE` and waits for the client's.
    AwaitingClient,
}

/// Which of the messages the host must send before `SESSION_START` it has sent
/// since `CLIENT_READY` (Section 4.5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Configuration {
    /// `DISPLAY_CONFIG` was sent.
    display_config: bool,
    /// `POINTER_MODE` was sent.
    pointer_mode: bool,
    /// `CURSOR_SET` was sent.
    cursor_set: bool,
}

/// The host's control stream state machine.
#[derive(Debug)]
pub struct HostSession {
    /// The host's configuration.
    config: HostConfig,
    /// The current phase.
    phase: Phase,
    /// When the QUIC handshake completed.
    created: Instant,
    /// When authentication succeeded, while waiting for `CLIENT_READY`.
    authenticated_at: Option<Instant>,
    /// Splits the client's direction of the control stream into messages.
    decoder: ControlStreamDecoder,
    /// Encoded messages waiting to be written to the control stream.
    transmit: Vec<u8>,
    /// Events waiting for the application.
    events: VecDeque<HostEvent>,
    /// The negotiated features, once `CLIENT_HELLO` has arrived.
    negotiated: Option<Negotiated>,
    /// The current authentication attempt.
    attempt: Attempt,
    /// The number of failed attempts on this connection.
    failed_attempts: u32,
    /// Configuration messages that arrived before authentication succeeded.
    pipelined: Vec<ControlMessage>,
    /// Whether pipelined messages are being discarded until the next `AUTH_START`
    /// because an attempt failed (Section 4.4.6).
    discarding: bool,
    /// Whether `CLIENT_READY` has arrived.
    client_ready: bool,
    /// Devices attached by the client that have not been answered.
    unanswered_devices: BTreeSet<DeviceId>,
    /// Every Device ID the client has attached, which are never reused.
    attached_devices: BTreeSet<DeviceId>,
    /// Whether an `AUDIO_REQUEST` awaits its `AUDIO_CONFIG`.
    audio_config_owed: bool,
    /// The messages sent since `CLIENT_READY`.
    configuration: Configuration,
    /// Why the session ended, once it has.
    close_reason: Option<SessionError>,
}

impl HostSession {
    /// Creates the session for a connection whose QUIC handshake completed at
    /// `now`.
    pub fn new(config: HostConfig, now: Instant) -> Self {
        Self {
            config,
            phase: Phase::Hello,
            created: now,
            authenticated_at: None,
            decoder: ControlStreamDecoder::with_pre_authentication_budget(),
            transmit: Vec::new(),
            events: VecDeque::new(),
            negotiated: None,
            attempt: Attempt::Idle,
            failed_attempts: 0,
            pipelined: Vec::new(),
            discarding: false,
            client_ready: false,
            unanswered_devices: BTreeSet::new(),
            attached_devices: BTreeSet::new(),
            audio_config_owed: false,
            configuration: Configuration::default(),
            close_reason: None,
        }
    }

    /// Returns the current phase.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Returns the negotiated features, once `CLIENT_HELLO` has arrived.
    pub fn negotiated(&self) -> Option<&Negotiated> {
        self.negotiated.as_ref()
    }

    /// Returns why the session ended, once it has. The transport closes the
    /// connection with this code after writing any pending data.
    pub fn close_reason(&self) -> Option<&SessionError> {
        self.close_reason.as_ref()
    }

    /// Returns whether datagrams from the client are processed; the host drops
    /// those that arrive before it has sent `SESSION_START` (Section 3.7.3).
    pub fn accepts_datagrams(&self) -> bool {
        self.phase == Phase::Active
    }

    /// Returns the bytes waiting to be written to the control stream, if any.
    pub fn poll_transmit(&mut self) -> Option<Vec<u8>> {
        (!self.transmit.is_empty()).then(|| std::mem::take(&mut self.transmit))
    }

    /// Returns the next event for the application, if any.
    pub fn poll_event(&mut self) -> Option<HostEvent> {
        self.events.pop_front()
    }

    /// Returns when [`HostSession::handle_timeout`] must next be called, if a
    /// handshake timer is running.
    pub fn poll_timeout(&self) -> Option<Instant> {
        match self.phase {
            Phase::Hello => Some(self.created + CLIENT_HELLO_TIMEOUT),
            Phase::Authenticating => Some(self.created + AUTHENTICATION_TIMEOUT),
            Phase::Configuring if !self.client_ready => self
                .authenticated_at
                .map(|authenticated_at| authenticated_at + CLIENT_READY_TIMEOUT),
            _ => None,
        }
    }

    /// Closes the session with `QDP_HANDSHAKE_TIMEOUT` if a handshake timer has
    /// expired at `now`.
    pub fn handle_timeout(&mut self, now: Instant) -> Result<(), SessionError> {
        match self.poll_timeout() {
            Some(deadline) if now >= deadline => Err(self.close(SessionError::new(
                ErrorCode::HANDSHAKE_TIMEOUT,
                "the session did not become active in time",
            ))),
            _ => Ok(()),
        }
    }

    /// Ends the session, for example because the host is shutting down.
    pub fn close(&mut self, error: SessionError) -> SessionError {
        self.phase = Phase::Closed;
        self.close_reason.get_or_insert(error).clone()
    }

    /// Processes bytes received on the client's direction of the control stream.
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
            if self.phase == Phase::Closed {
                return Ok(());
            }
        }
    }

    /// Answers the client's last authentication message.
    pub fn respond_auth(&mut self, response: AuthResponse, now: Instant) -> Result<(), UsageError> {
        if self.phase != Phase::Authenticating || self.attempt != Attempt::AwaitingHost {
            return Err(UsageError::InvalidState(
                "no authentication step awaits an answer",
            ));
        }
        match response {
            AuthResponse::Continue(auth_data) => {
                check_auth_data(&auth_data)?;
                self.queue(&AuthContinue { auth_data }.to_frame())?;
                self.attempt = Attempt::AwaitingClient;
            }
            AuthResponse::Accept(auth_data) => {
                check_auth_data(&auth_data)?;
                self.queue(
                    &AuthResult {
                        status: AuthStatus::Success,
                        auth_data,
                        reason: String::new(),
                    }
                    .to_frame(),
                )?;
                self.phase = Phase::Configuring;
                self.attempt = Attempt::Idle;
                self.authenticated_at = Some(now);
                self.decoder.clear_budget();
                for message in std::mem::take(&mut self.pipelined) {
                    if let Err(error) = self.handle_message(message) {
                        self.close(error);
                        break;
                    }
                }
            }
            AuthResponse::Reject => self.fail_attempt(),
        }
        Ok(())
    }

    /// Sends a configuration or active-phase message to the client.
    ///
    /// Hello, authentication and `SESSION_START` messages are sent by the session
    /// itself; `SESSION_START` through [`HostSession::start`].
    pub fn send(&mut self, message: &ControlMessage) -> Result<(), UsageError> {
        let message_type = message.message_type();
        let owned_by_session = matches!(
            message_type,
            MessageType::HOST_HELLO
                | MessageType::AUTH_CONTINUE
                | MessageType::AUTH_RESULT
                | MessageType::SESSION_START
        );
        if owned_by_session
            || check(Role::Host, self.phase, self.client_ready, message_type) != Verdict::Allowed
        {
            return Err(UsageError::NotAllowed {
                message_type,
                phase: self.phase,
            });
        }
        if let ControlMessage::DeviceStatus(status) = message {
            if !self.unanswered_devices.contains(&status.device_id) {
                return Err(UsageError::InvalidState(
                    "DEVICE_STATUS must answer an unanswered DEVICE_ATTACH",
                ));
            }
        }

        self.queue(&message.to_frame())?;
        match message {
            ControlMessage::DeviceStatus(status) => {
                self.unanswered_devices.remove(&status.device_id);
            }
            ControlMessage::DisplayConfig(_) => self.configuration.display_config = true,
            ControlMessage::AudioConfig(_) => self.audio_config_owed = false,
            ControlMessage::PointerMode(_) => self.configuration.pointer_mode = true,
            ControlMessage::CursorSet(_) => self.configuration.cursor_set = true,
            _ => {}
        }
        Ok(())
    }

    /// Sends `SESSION_START` and enters the `ACTIVE` phase, once every message
    /// that must precede it has been sent (Section 4.5).
    pub fn start(&mut self) -> Result<(), UsageError> {
        if self.phase != Phase::Configuring || !self.client_ready {
            return Err(UsageError::InvalidState(
                "SESSION_START requires CLIENT_READY in the configuring phase",
            ));
        }
        if !self.unanswered_devices.is_empty() {
            return Err(UsageError::InvalidState(
                "every attached device must be answered first",
            ));
        }
        let Configuration {
            display_config,
            pointer_mode,
            cursor_set,
        } = self.configuration;
        if !(display_config && pointer_mode && cursor_set) || self.audio_config_owed {
            return Err(UsageError::InvalidState(
                "DISPLAY_CONFIG, AUDIO_CONFIG, POINTER_MODE and CURSOR_SET must be sent first",
            ));
        }
        self.queue(&SessionStart.to_frame())?;
        self.phase = Phase::Active;
        Ok(())
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
            (Phase::Hello, ControlMessage::ClientHello(hello)) => self.handle_client_hello(hello),
            (Phase::Hello, _) => Err(SessionError::protocol_violation(
                "the first message must be CLIENT_HELLO",
            )),
            (_, ControlMessage::ClientHello(_)) => Err(SessionError::protocol_violation(
                "CLIENT_HELLO sent more than once",
            )),
            (_, message) => self.handle_message(message),
        }
    }

    /// Answers `CLIENT_HELLO` with `HOST_HELLO` and negotiates the session.
    fn handle_client_hello(&mut self, hello: ClientHello) -> Result<(), SessionError> {
        let host_hello = self.config.host_hello.clone();
        self.queue(&host_hello.to_frame())
            .map_err(|_| SessionError::new(ErrorCode::INTERNAL_ERROR, "HOST_HELLO is too large"))?;
        self.phase = Phase::Authenticating;
        // The host closes the connection after sending HOST_HELLO if negotiation
        // fails (Section 4.3.2).
        self.negotiated = Some(Negotiated::new(&hello, &host_hello)?);
        self.events.push_back(HostEvent::ClientHello(hello));
        Ok(())
    }

    /// Processes a message after the hello, according to the table of Section 4.2.
    fn handle_message(&mut self, message: ControlMessage) -> Result<(), SessionError> {
        match check(
            Role::Client,
            self.phase,
            self.client_ready,
            message.message_type(),
        ) {
            Verdict::Violation => Err(SessionError::protocol_violation(format!(
                "{} is not allowed in the {:?} phase",
                message.message_type(),
                self.phase
            ))),
            Verdict::Pipelined => self.pipeline(message),
            Verdict::Allowed => self.dispatch(message),
        }
    }

    /// Holds a configuration message that arrived before authentication succeeded
    /// (Section 4.4.6).
    fn pipeline(&mut self, message: ControlMessage) -> Result<(), SessionError> {
        if self.discarding {
            return Ok(());
        }
        if let ControlMessage::DeviceAttach(attach) = &message {
            if attach.kind == DeviceKind::PASSTHROUGH {
                return Err(SessionError::protocol_violation(
                    "passthrough devices may not be pipelined",
                ));
            }
        }
        self.pipelined.push(message);
        Ok(())
    }

    /// Acts on a message that is allowed in the current phase.
    fn dispatch(&mut self, message: ControlMessage) -> Result<(), SessionError> {
        match message {
            ControlMessage::AuthStart(start) => {
                if self.attempt != Attempt::Idle {
                    return Err(SessionError::protocol_violation(
                        "AUTH_START during an authentication attempt",
                    ));
                }
                self.discarding = false;
                if self
                    .config
                    .host_hello
                    .auth_methods
                    .contains(&start.auth_method)
                {
                    self.attempt = Attempt::AwaitingHost;
                    self.events.push_back(HostEvent::AuthStart {
                        method: start.auth_method,
                        auth_data: start.auth_data,
                    });
                } else {
                    // A method the host did not offer is a failed attempt
                    // (Section 4.4.1).
                    self.fail_attempt();
                }
                return Ok(());
            }
            ControlMessage::AuthContinue(next) => {
                if self.attempt != Attempt::AwaitingClient {
                    return Err(SessionError::protocol_violation(
                        "AUTH_CONTINUE outside an exchange that uses it",
                    ));
                }
                self.attempt = Attempt::AwaitingHost;
                self.events.push_back(HostEvent::AuthContinue {
                    auth_data: next.auth_data,
                });
                return Ok(());
            }
            ControlMessage::ClientReady(_) => {
                if self.client_ready {
                    return Err(SessionError::protocol_violation(
                        "CLIENT_READY sent more than once",
                    ));
                }
                self.client_ready = true;
            }
            ControlMessage::DeviceAttach(ref attach) => {
                if !self.attached_devices.insert(attach.device_id) {
                    return Err(SessionError::protocol_violation("Device ID reused"));
                }
                self.unanswered_devices.insert(attach.device_id);
            }
            ControlMessage::AudioRequest(_) => self.audio_config_owed = true,
            ControlMessage::Unknown(_) => return Ok(()),
            _ => {}
        }
        self.events.push_back(HostEvent::Message(message));
        Ok(())
    }

    /// Ends the current attempt as failed, closing the connection after the last
    /// allowed failure (Section 4.4.1).
    fn fail_attempt(&mut self) {
        self.failed_attempts += 1;
        self.attempt = Attempt::Idle;
        self.pipelined.clear();
        self.discarding = true;
        let is_final = self.failed_attempts >= MAX_FAILED_ATTEMPTS;
        let result = AuthResult {
            status: if is_final {
                AuthStatus::FailedFinal
            } else {
                AuthStatus::FailedRetry
            },
            auth_data: Vec::new(),
            reason: self.config.failure_reason.clone(),
        };
        if self.queue(&result.to_frame()).is_err() || is_final {
            self.close(SessionError::new(
                ErrorCode::AUTH_FAILED,
                "too many failed authentication attempts",
            ));
        }
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
