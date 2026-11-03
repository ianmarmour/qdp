//! Checking that a QUIC server answers at an address without connecting to
//! it, the way QUIC itself provides (RFC 9000, Section 6): a datagram of a
//! version no server supports draws a Version Negotiation packet from any
//! QUIC server listening there, before any connection or handshake exists,
//! so the server neither keeps state for it nor authenticates anyone. The
//! apps check the hosts they paired with this way where the local network
//! cannot be searched, as on a cellular network.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, ErrorKind};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

/// A version reserved for exercising version negotiation, which no server
/// supports (RFC 9000, Section 15).
const PROBE_VERSION: u32 = 0x1a2a_3a4a;

/// The size of the probe: servers answer only datagrams at least as large
/// as a client's first (RFC 9000, Section 14.1).
const PROBE_SIZE: usize = 1200;

/// The length of the connection IDs the probe chooses, in bytes.
const ID_LENGTH: usize = 8;

/// The first byte of the probe: a long header, its fixed bit set.
const LONG_HEADER: u8 = 0xC0;

/// The bit of a packet's first byte set for a long header.
const HEADER_FORM: u8 = 0x80;

/// The version a Version Negotiation packet carries.
const NEGOTIATION_VERSION: [u8; 4] = [0; 4];

/// How many times the probe is sent within its time, in case one is lost.
const ATTEMPTS: u32 = 2;

/// The largest reply read.
const MAX_REPLY: usize = 1500;

/// Returns whether a QUIC server answers at `address` within `timeout`. A
/// host that refuses the datagram, or does not answer, does not; errors are
/// the local socket's.
pub fn probe(address: SocketAddr, timeout: Duration) -> io::Result<bool> {
    let local: SocketAddr = if address.is_ipv4() {
        (Ipv4Addr::UNSPECIFIED, 0).into()
    } else {
        (Ipv6Addr::UNSPECIFIED, 0).into()
    };
    let socket = UdpSocket::bind(local)?;
    // Only the server's replies are read, and a refusal is reported.
    socket.connect(address)?;
    let source_id = random_id();
    let datagram = probe_datagram(&random_id(), &source_id);
    let deadline = Instant::now() + timeout;
    let mut reply = [0u8; MAX_REPLY];
    for attempt in 0..ATTEMPTS {
        match socket.send(&datagram) {
            Ok(_) => {}
            Err(error) if refused(&error) => return Ok(false),
            Err(error) => return Err(error),
        }
        // Each attempt waits its share of the time left.
        let left = deadline.saturating_duration_since(Instant::now());
        let until = Instant::now() + left / (ATTEMPTS - attempt);
        loop {
            let wait = until.saturating_duration_since(Instant::now());
            if wait.is_zero() {
                break;
            }
            socket.set_read_timeout(Some(wait))?;
            match socket.recv(&mut reply) {
                Ok(length) if is_version_negotiation(&reply[..length], &source_id) => {
                    return Ok(true);
                }
                Ok(_) => {}
                Err(error) if timed_out(&error) => break,
                Err(error) if refused(&error) => return Ok(false),
                Err(error) => return Err(error),
            }
        }
    }
    Ok(false)
}

/// Returns the probe: a long-header packet of [`PROBE_VERSION`] from
/// `source` to `destination`, padded to [`PROBE_SIZE`].
fn probe_datagram(destination: &[u8; ID_LENGTH], source: &[u8; ID_LENGTH]) -> Vec<u8> {
    let mut datagram = Vec::with_capacity(PROBE_SIZE);
    datagram.push(LONG_HEADER);
    datagram.extend_from_slice(&PROBE_VERSION.to_be_bytes());
    datagram.push(ID_LENGTH as u8);
    datagram.extend_from_slice(destination);
    datagram.push(ID_LENGTH as u8);
    datagram.extend_from_slice(source);
    datagram.resize(PROBE_SIZE, 0);
    datagram
}

/// Returns whether `packet` is a Version Negotiation packet answering the
/// probe from `source_id`: a long header of version 0 whose destination
/// connection ID is the probe's source (RFC 9000, Section 17.2.1).
fn is_version_negotiation(packet: &[u8], source_id: &[u8; ID_LENGTH]) -> bool {
    let Some((&first, rest)) = packet.split_first() else {
        return false;
    };
    if first & HEADER_FORM == 0 || rest.len() < NEGOTIATION_VERSION.len() {
        return false;
    }
    let (version, rest) = rest.split_at(NEGOTIATION_VERSION.len());
    if version != NEGOTIATION_VERSION {
        return false;
    }
    let Some((&length, rest)) = rest.split_first() else {
        return false;
    };
    rest.get(..usize::from(length)) == Some(&source_id[..])
}

/// Returns a connection ID no other probe shares.
fn random_id() -> [u8; ID_LENGTH] {
    // Each `RandomState` is keyed afresh, so its hash of nothing differs.
    RandomState::new().build_hasher().finish().to_be_bytes()
}

/// Returns whether `error` is a read that waited its time out.
fn timed_out(error: &io::Error) -> bool {
    matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

/// Returns whether `error` says nothing listens at the address, as the
/// host's ICMP reply to a closed port reports on a connected socket.
fn refused(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::ConnectionRefused | ErrorKind::ConnectionReset
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe is a long header of the reserved version, with the
    /// connection IDs it was given, padded to a client's first datagram.
    #[test]
    fn writes_a_long_header_of_an_unsupported_version() {
        let datagram = probe_datagram(&[1; ID_LENGTH], &[2; ID_LENGTH]);
        assert_eq!(datagram.len(), PROBE_SIZE);
        assert_eq!(datagram[0], LONG_HEADER);
        assert_eq!(&datagram[1..5], &PROBE_VERSION.to_be_bytes());
        assert_eq!(datagram[5] as usize, ID_LENGTH);
        assert_eq!(&datagram[6..14], &[1; ID_LENGTH]);
        assert_eq!(datagram[14] as usize, ID_LENGTH);
        assert_eq!(&datagram[15..23], &[2; ID_LENGTH]);
    }

    /// Only a Version Negotiation packet sent back to the probe's source
    /// answers it.
    #[test]
    fn recognizes_the_answer_to_its_probe() {
        let source = [7; ID_LENGTH];
        let mut answer = vec![0x80 | 0x2a, 0, 0, 0, 0, ID_LENGTH as u8];
        answer.extend_from_slice(&source);
        answer.push(ID_LENGTH as u8);
        answer.extend_from_slice(&[9; ID_LENGTH]);
        answer.extend_from_slice(&1u32.to_be_bytes());
        assert!(is_version_negotiation(&answer, &source));
        assert!(!is_version_negotiation(&answer, &[8; ID_LENGTH]));
        let mut short_header = answer.clone();
        short_header[0] = 0x40;
        assert!(!is_version_negotiation(&short_header, &source));
        let mut other_version = answer.clone();
        other_version[4] = 1;
        assert!(!is_version_negotiation(&other_version, &source));
        assert!(!is_version_negotiation(&answer[..3], &source));
    }

    /// A port where nothing answers is not reachable.
    #[test]
    fn silent_port_is_unreachable() {
        let silent = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = silent.local_addr().unwrap();
        assert!(!probe(address, Duration::from_millis(200)).unwrap());
    }

    /// Connection IDs differ from probe to probe.
    #[test]
    fn chooses_new_ids() {
        assert_ne!(random_id(), random_id());
    }
}
