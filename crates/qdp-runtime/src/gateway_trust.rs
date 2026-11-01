//! A host's trust of gateways (Section 13.7): the identity assertions a
//! trusted gateway signs, presented as tokens, admit the users it
//! authenticated, without credentials of the host's own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use qdp::gateway::assertion::{
    Expected, IdentityAssertion, NONCE_LENGTH, PUBLIC_KEY_LENGTH, TrustedKey,
};
use qdp_quic::KeyPin;
use tracing::info;

/// The nonces of accepted assertions, with when each assertion expires, in
/// seconds since the Unix epoch. Clones share them.
type Nonces = Arc<Mutex<HashMap<[u8; NONCE_LENGTH], u64>>>;

/// The gateways a host trusts, and which of their users it admits.
#[derive(Debug, Clone)]
pub struct GatewayTrust {
    /// The gateways' assertion keys.
    keys: Vec<TrustedKey>,
    /// The host's own key pin, which assertions must name.
    host_pin: KeyPin,
    /// The groups a user must be in, one of them, if any are given.
    groups: Vec<String>,
    /// The nonces of the assertions accepted, so that none is accepted twice.
    nonces: Nonces,
}

/// A user a gateway vouched for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayUser {
    /// The user's name in the gateway's directory.
    pub name: String,
    /// Their full name, or empty.
    pub display_name: String,
    /// The groups the gateway reports them in.
    pub groups: Vec<String>,
}

impl GatewayTrust {
    /// Returns a trust of no gateway yet, for the host whose key pin is
    /// `host_pin`.
    pub fn new(host_pin: KeyPin) -> Self {
        Self {
            keys: Vec::new(),
            host_pin,
            groups: Vec::new(),
            nonces: Nonces::default(),
        }
    }

    /// Trusts the gateway whose assertion key is `public_key`, as
    /// `qdp-gateway keys` prints it.
    pub fn add_key(&mut self, public_key: [u8; PUBLIC_KEY_LENGTH]) {
        self.keys.push(TrustedKey::new(public_key));
    }

    /// Admits only users the gateway reports in `group`, or in another group
    /// required this way.
    pub fn require_group(&mut self, group: String) {
        self.groups.push(group);
    }

    /// Returns whether any gateway is trusted.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Returns the user `token` vouches for, if it is an assertion a trusted
    /// gateway signed for this host and the client that presented the
    /// certificate with `client_key`, valid at `now`, not used before, and
    /// naming a required group.
    pub fn admit(&self, token: &[u8], client_key: Option<KeyPin>, now: u64) -> Option<GatewayUser> {
        if !IdentityAssertion::is_assertion(token) {
            return None;
        }
        // An assertion names the client's key, so it needs one.
        let client_key = client_key?;
        let expected = Expected {
            host_pin: self.host_pin.0,
            client_pin: client_key.0,
            now,
        };
        let assertion = match IdentityAssertion::verify(token, &self.keys, expected) {
            Ok(assertion) => assertion,
            Err(error) => {
                info!(%error, "refused an identity assertion");
                return None;
            }
        };
        let in_group = self.groups.is_empty()
            || assertion.groups.iter().any(|group| {
                self.groups
                    .iter()
                    .any(|required| required.eq_ignore_ascii_case(group))
            });
        if !in_group {
            info!(
                user = assertion.user_name,
                "refused a gateway's user in no required group"
            );
            return None;
        }
        let mut nonces = self.nonces.lock().unwrap_or_else(PoisonError::into_inner);
        nonces.retain(|_, expires| *expires >= now);
        if nonces
            .insert(assertion.nonce, assertion.expires_at)
            .is_some()
        {
            info!(
                user = assertion.user_name,
                "refused an identity assertion used before"
            );
            return None;
        }
        info!(
            user = assertion.user_name,
            groups = ?assertion.groups,
            "admitted a user a gateway vouched for"
        );
        Some(GatewayUser {
            name: assertion.user_name,
            display_name: assertion.display_name,
            groups: assertion.groups,
        })
    }
}

#[cfg(test)]
mod tests {
    use qdp::discovery::PIN_LENGTH;
    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    use super::*;

    /// The time the tests run at.
    const NOW: u64 = 1_793_000_000;

    /// Returns a gateway's assertion key.
    fn gateway() -> Ed25519KeyPair {
        let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Ed25519KeyPair::from_pkcs8(document.as_ref()).unwrap()
    }

    /// Returns an assertion of `user` in `groups` for host `0xaa…` and client
    /// `0xcc…`, with `nonce`.
    fn assertion(key: &Ed25519KeyPair, groups: &[&str], nonce: u8) -> Vec<u8> {
        IdentityAssertion {
            host_pin: [0xaa; PIN_LENGTH],
            client_pin: [0xcc; PIN_LENGTH],
            issued_at: NOW,
            expires_at: NOW + 120,
            nonce: [nonce; NONCE_LENGTH],
            user_name: "alice".to_owned(),
            display_name: "Alice Liddell".to_owned(),
            groups: groups.iter().map(|group| (*group).to_owned()).collect(),
        }
        .sign(key)
    }

    /// Returns the trust of `key` by host `0xaa…`.
    fn trust(key: &Ed25519KeyPair) -> GatewayTrust {
        let mut trust = GatewayTrust::new(KeyPin([0xaa; PIN_LENGTH]));
        trust.add_key(key.public_key().as_ref().try_into().unwrap());
        trust
    }

    #[test]
    fn admits_a_trusted_gateways_user_once() {
        let key = gateway();
        let trust = trust(&key);
        let client = Some(KeyPin([0xcc; PIN_LENGTH]));
        let token = assertion(&key, &["QDP Users"], 1);
        let user = trust.admit(&token, client, NOW).unwrap();
        assert_eq!(user.name, "alice");
        assert_eq!(user.groups, ["QDP Users"]);
        // Clones share the nonces they accepted.
        assert_eq!(trust.clone().admit(&token, client, NOW), None);
        // Another client's key, no key, another gateway or a plain token
        // admit no one.
        let other = assertion(&key, &[], 2);
        assert_eq!(
            trust.admit(&other, Some(KeyPin([0xdd; PIN_LENGTH])), NOW),
            None
        );
        assert_eq!(trust.admit(&other, None, NOW), None);
        assert_eq!(
            trust.admit(&assertion(&gateway(), &[], 3), client, NOW),
            None
        );
        assert_eq!(trust.admit(b"my-secret", client, NOW), None);
        assert!(trust.admit(&other, client, NOW).is_some());
    }

    #[test]
    fn requires_a_group_when_told_to() {
        let key = gateway();
        let mut trust = trust(&key);
        trust.require_group("qdp users".to_owned());
        let client = Some(KeyPin([0xcc; PIN_LENGTH]));
        assert_eq!(
            trust.admit(&assertion(&key, &["Sales"], 1), client, NOW),
            None
        );
        assert!(
            trust
                .admit(&assertion(&key, &["QDP Users"], 2), client, NOW)
                .is_some()
        );
    }
}
