//! Drivers that run the sans-I/O session state machines over a QDP connection.

use std::time::Instant;

use qdp::message::ClientHello;
use qdp::registry::ErrorCode;
use qdp::session::{ClientEvent, ClientSession, HostConfig, HostEvent, HostSession, SessionError};
use tracing::debug;

use crate::{QdpConnection, TransportError};

/// What a session state machine must provide for a driver to run it.
///
/// [`ClientSession`] and [`HostSession`] implement it, so one driver loop serves
/// both roles.
pub trait DrivenSession {
    /// The events the session reports to the application.
    type Event;

    /// Processes bytes received on the control stream.
    fn handle_control_data(&mut self, data: &[u8]) -> Result<(), SessionError>;

    /// Returns bytes waiting to be written to the control stream.
    fn poll_transmit(&mut self) -> Option<Vec<u8>>;

    /// Returns the next event for the application.
    fn poll_event(&mut self) -> Option<Self::Event>;

    /// Returns when the session's timer next expires.
    fn poll_timeout(&self) -> Option<Instant>;

    /// Processes an expired timer.
    fn handle_timeout(&mut self, now: Instant) -> Result<(), SessionError>;

    /// Returns why the session ended, once it has.
    fn close_reason(&self) -> Option<&SessionError>;
}

impl DrivenSession for HostSession {
    type Event = HostEvent;

    fn handle_control_data(&mut self, data: &[u8]) -> Result<(), SessionError> {
        HostSession::handle_control_data(self, data)
    }

    fn poll_transmit(&mut self) -> Option<Vec<u8>> {
        HostSession::poll_transmit(self)
    }

    fn poll_event(&mut self) -> Option<HostEvent> {
        HostSession::poll_event(self)
    }

    fn poll_timeout(&self) -> Option<Instant> {
        HostSession::poll_timeout(self)
    }

    fn handle_timeout(&mut self, now: Instant) -> Result<(), SessionError> {
        HostSession::handle_timeout(self, now)
    }

    fn close_reason(&self) -> Option<&SessionError> {
        HostSession::close_reason(self)
    }
}

impl DrivenSession for ClientSession {
    type Event = ClientEvent;

    fn handle_control_data(&mut self, data: &[u8]) -> Result<(), SessionError> {
        ClientSession::handle_control_data(self, data)
    }

    fn poll_transmit(&mut self) -> Option<Vec<u8>> {
        ClientSession::poll_transmit(self)
    }

    fn poll_event(&mut self) -> Option<ClientEvent> {
        ClientSession::poll_event(self)
    }

    fn poll_timeout(&self) -> Option<Instant> {
        ClientSession::poll_timeout(self)
    }

    fn handle_timeout(&mut self, now: Instant) -> Result<(), SessionError> {
        ClientSession::handle_timeout(self, now)
    }

    fn close_reason(&self) -> Option<&SessionError> {
        ClientSession::close_reason(self)
    }
}

/// Runs a session state machine over a connection: it writes what the session
/// queues to the control stream, feeds it what arrives, runs its timers, and
/// closes the connection with the session's error code when the session ends.
#[derive(Debug)]
pub struct SessionDriver<S> {
    /// The connection.
    connection: QdpConnection,
    /// The session state machine.
    session: S,
}

/// The driver of a host's session.
pub type HostDriver = SessionDriver<HostSession>;

/// The driver of a client's session.
pub type ClientDriver = SessionDriver<ClientSession>;

impl HostDriver {
    /// Starts the host's session on an accepted connection.
    pub fn host(connection: QdpConnection, config: HostConfig) -> Self {
        Self {
            connection,
            session: HostSession::new(config, Instant::now()),
        }
    }
}

impl ClientDriver {
    /// Starts the client's session on a new connection; `CLIENT_HELLO` is written
    /// by the first [`SessionDriver::flush`] or [`SessionDriver::next_event`].
    pub fn client(connection: QdpConnection, hello: ClientHello) -> Result<Self, TransportError> {
        Ok(Self {
            connection,
            session: ClientSession::new(hello)?,
        })
    }
}

impl<S: DrivenSession> SessionDriver<S> {
    /// Returns the connection.
    pub fn connection(&self) -> &QdpConnection {
        &self.connection
    }

    /// Returns the session state machine, for answering events. Call
    /// [`SessionDriver::flush`] after changing it.
    pub fn session(&mut self) -> &mut S {
        &mut self.session
    }

    /// Writes everything the session has queued to the control stream, and closes
    /// the connection if the session has ended.
    pub async fn flush(&mut self) -> Result<(), TransportError> {
        while let Some(bytes) = self.session.poll_transmit() {
            self.connection.write_control(&bytes).await?;
        }
        if let Some(reason) = self.session.close_reason() {
            let reason = reason.clone();
            debug!(code = %reason.code, reason = %reason.reason, "closing session");
            self.connection.close(reason.code, &reason.reason);
            return Err(TransportError::Session(reason));
        }
        Ok(())
    }

    /// Runs the session until it has an event for the application.
    pub async fn next_event(&mut self) -> Result<S::Event, TransportError> {
        loop {
            self.flush().await?;
            if let Some(event) = self.session.poll_event() {
                return Ok(event);
            }
            let deadline = self.session.poll_timeout();
            let sleep = async {
                match deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                read = self.connection.read_control() => {
                    match read {
                        Ok(chunk) => {
                            // An error also records the close reason, which the next
                            // flush acts on.
                            let _ = self.session.handle_control_data(&chunk);
                        }
                        Err(TransportError::ControlStreamClosed) => {
                            let error = SessionError::new(
                                ErrorCode::CLOSED_CRITICAL_STREAM,
                                "the control stream was closed",
                            );
                            self.connection.close(error.code, &error.reason);
                            return Err(TransportError::Session(error));
                        }
                        Err(error) => return Err(error),
                    }
                }
                () = sleep => {
                    let _ = self.session.handle_timeout(Instant::now());
                }
            }
        }
    }

    /// Ends the session normally with `QDP_NO_ERROR`.
    pub fn close(&self, reason: &str) {
        self.connection.close(ErrorCode::NO_ERROR, reason);
    }
}
