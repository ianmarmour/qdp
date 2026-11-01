//! The gateway end to end, over QUIC on the loopback interface: users sign in
//! with a password and a TOTP code, enrolling a secret the first time, open
//! hosts by ID and by address, and their datagrams reach a host, here an
//! echo, through the tunnel.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use qdp::auth::totp::{STEP_SECONDS, TotpSecret};
use qdp::gateway::assertion::{Expected, IdentityAssertion, TrustedKey};
use qdp::gateway::{
    GATEWAY_VERSION, GatewayErrorCode, GatewayHello, GatewayMessage, MIN_TUNNEL_PAYLOAD,
    NextFactor, Open, OpenFailure, Opened,
};
use qdp::message::{AuthMethod, AuthResult, AuthStart, AuthStatus, PasswordAuthData};
use qdp::wire::ControlStreamDecoder;
use qdp_gateway::config::Config;
use qdp_gateway::directory::{Directory, DirectoryError, DirectoryUser};
use qdp_gateway::keys::GatewayKeys;
use qdp_gateway::server::Gateway;
use qdp_gateway::totp_store::TotpStore;
use qdp_quic::{HostVerification, KeyPin, TlsIdentity};
use quinn::{Connection, Endpoint, RecvStream, SendStream};
use tokio::net::UdpSocket;

/// The group whose members may reach the echo host.
const GROUP: &str = "CN=QDP Users,OU=Groups,DC=corp,DC=example,DC=com";

/// The echo host's key pin, as configured.
const HOST_PIN: &str = "59b81a9e89682a79876919776020611407a3fd03556002d7d25d0c4609584bb7";

/// A directory of users in memory.
struct Users(HashMap<&'static str, (&'static str, DirectoryUser)>);

impl Directory for Users {
    async fn authenticate(
        &self,
        user: &str,
        password: &str,
    ) -> Result<DirectoryUser, DirectoryError> {
        match self.0.get(user.to_lowercase().as_str()) {
            Some((expected, found)) if *expected == password => Ok(found.clone()),
            _ => Err(DirectoryError::Refused),
        }
    }
}

/// Returns the directory: alice and bob in the group, carol outside it.
fn users() -> Users {
    let user = |name: &str, groups: &[&str]| DirectoryUser {
        name: name.to_owned(),
        display_name: format!("{name} example"),
        groups: groups.iter().map(|group| (*group).to_owned()).collect(),
    };
    Users(HashMap::from([
        ("alice", ("wonderland", user("alice", &[GROUP]))),
        ("bob", ("builder", user("bob", &[GROUP]))),
        ("carol", ("singer", user("carol", &[]))),
    ]))
}

/// A running gateway and the echo host behind it.
struct Running {
    /// Where the gateway listens.
    address: SocketAddr,
    /// The gateway's pin.
    pin: KeyPin,
    /// The key hosts trust its assertions with.
    assertion_key: TrustedKey,
    /// The echo host's port.
    echo_port: u16,
}

/// Returns a new state directory for a test.
fn state_directory(test: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("qdp-gateway-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

/// Starts an echo host, and a gateway with its state in `directory` that
/// lists it as `echo` and routes to it by address.
async fn start(directory: &Path) -> Running {
    let echo = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let echo_port = echo.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut buffer = vec![0; 65535];
        while let Ok((length, from)) = echo.recv_from(&mut buffer).await {
            let _ = echo.send_to(&buffer[..length], from).await;
        }
    });
    let config_path = directory.join("gateway.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
        name = "Example Corp"
        state_directory = "{state}"

        [directory]
        url = "ldaps://directory.invalid"
        bind_dn = "cn=gateway"
        bind_password_file = "/dev/null"
        base_dn = "dc=corp"

        [totp]
        enrollment = "first-login"

        [assertions]
        groups = ["QDP Users"]

        [[host]]
        id = "echo"
        name = "Echo"
        address = "127.0.0.1:{echo_port}"
        pin = "{HOST_PIN}"
        groups = ["QDP Users"]

        [[route]]
        networks = ["127.0.0.0/8"]
        ports = [{echo_port}]
        groups = ["QDP Users"]
    "#,
            state = directory.display()
        ),
    )
    .unwrap();
    let config = Config::load(&config_path).unwrap();
    let keys = GatewayKeys::load(&config).unwrap();
    let pin = keys.tls_pin();
    let assertion_key = TrustedKey::new(keys.assertion_public_key());
    let totp = TotpStore::load(&config.state_directory).unwrap();
    let gateway = Arc::new(Gateway::new(config, users(), keys, totp));
    let endpoint = gateway.endpoint((Ipv4Addr::LOCALHOST, 0).into()).unwrap();
    let address = endpoint.local_addr().unwrap();
    tokio::spawn(gateway.serve(endpoint));
    Running {
        address,
        pin,
        assertion_key,
        echo_port,
    }
}

/// A client's connection to the gateway.
struct Client {
    /// The endpoint, kept for as long as the connection.
    _endpoint: Endpoint,
    /// The connection.
    connection: Connection,
    /// The control stream's sending side.
    send: SendStream,
    /// The control stream's receiving side.
    receive: RecvStream,
    /// The messages split from it.
    decoder: ControlStreamDecoder,
}

impl Client {
    /// Connects to `gateway` as the client with `identity`, and says hello.
    async fn connect(gateway: &Running, identity: &TlsIdentity) -> Self {
        let config = qdp_quic::gateway_client_config(
            HostVerification::PinnedKeys(vec![gateway.pin]),
            Some(identity.clone_key()),
        )
        .unwrap();
        let mut endpoint = Endpoint::client((Ipv4Addr::LOCALHOST, 0).into()).unwrap();
        endpoint.set_default_client_config(config);
        let connection = endpoint
            .connect(gateway.address, "qdp-gateway")
            .unwrap()
            .await
            .unwrap();
        let (send, receive) = connection.open_bi().await.unwrap();
        let mut client = Self {
            _endpoint: endpoint,
            connection,
            send,
            receive,
            decoder: ControlStreamDecoder::new(),
        };
        client
            .send(GatewayMessage::Hello(GatewayHello {
                version: GATEWAY_VERSION,
                client_name: "test".to_owned(),
            }))
            .await;
        let GatewayMessage::Welcome(welcome) = client.receive().await else {
            panic!("expected GATEWAY_WELCOME");
        };
        assert_eq!(welcome.gateway_name, "Example Corp");
        assert_eq!(
            welcome.auth_methods,
            [AuthMethod::PASSWORD, AuthMethod::TOKEN]
        );
        client
    }

    /// Sends `message`.
    async fn send(&mut self, message: GatewayMessage) {
        let bytes = message.to_frame().unwrap().to_bytes().unwrap();
        self.send.write_all(&bytes).await.unwrap();
    }

    /// Returns the next message.
    async fn receive(&mut self) -> GatewayMessage {
        let mut chunk = [0; 4096];
        loop {
            if let Some(frame) = self.decoder.next_message().unwrap() {
                return GatewayMessage::decode(&frame).unwrap();
            }
            let read = self
                .receive
                .read(&mut chunk)
                .await
                .unwrap()
                .expect("the stream ended");
            self.decoder.push(&chunk[..read]);
        }
    }

    /// Starts an attempt with `method` and returns its result.
    async fn attempt(&mut self, method: AuthMethod, auth_data: Vec<u8>) -> AuthResult {
        self.send(GatewayMessage::AuthStart(AuthStart {
            auth_method: method,
            auth_data,
        }))
        .await;
        match self.receive().await {
            GatewayMessage::AuthResult(result) => result,
            other => panic!("expected AUTH_RESULT, got {other:?}"),
        }
    }

    /// Tries `user`'s `password`.
    async fn password(&mut self, user: &str, password: &str) -> AuthResult {
        let data = PasswordAuthData {
            user_name: user.to_owned(),
            password: password.to_owned(),
        };
        self.attempt(AuthMethod::PASSWORD, data.to_bytes()).await
    }

    /// Opens `target`, returning what the gateway answered.
    async fn open(&mut self, target: &str) -> GatewayMessage {
        self.send(GatewayMessage::Open(Open {
            target: target.to_owned(),
        }))
        .await;
        self.receive().await
    }

    /// Checks that a datagram goes to the host and back, at the size every
    /// tunnel must carry (Section 13.2).
    async fn echoes(&self) {
        assert!(self.connection.max_datagram_size().unwrap() >= MIN_TUNNEL_PAYLOAD);
        let payload = Bytes::from(vec![0x5a; MIN_TUNNEL_PAYLOAD]);
        self.connection.send_datagram(payload.clone()).unwrap();
        let echoed = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.connection.read_datagram(),
        )
        .await
        .expect("no echo")
        .unwrap();
        assert_eq!(echoed, payload);
    }
}

/// Returns the time now, in seconds since the Unix epoch.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Returns the code of `secret` now.
fn code(secret: &TotpSecret) -> Vec<u8> {
    format!("{:06}", secret.code(now() / STEP_SECONDS)).into_bytes()
}

/// Signs `user` in with `password` on `client`, enrolling the secret the
/// gateway offers, and returns the secret and the session token.
async fn sign_in_enrolling(
    client: &mut Client,
    user: &str,
    password: &str,
) -> (TotpSecret, Vec<u8>) {
    let result = client.password(user, password).await;
    assert_eq!(result.status, AuthStatus::NextFactor);
    let next = NextFactor::from_bytes(&result.auth_data).unwrap();
    assert_eq!(next.auth_methods, [AuthMethod::TOTP]);
    let uri = next
        .enrollment
        .expect("an enrollment for a user without a secret");
    // The directory's name for the user, whatever case they typed.
    let name = user.to_lowercase();
    assert!(uri.starts_with(&format!("otpauth://totp/Example%20Corp:{name}?secret=")));
    let secret = uri
        .split(['?', '&'])
        .find_map(|field| field.strip_prefix("secret="))
        .map(|secret| TotpSecret::from_base32(secret).unwrap())
        .unwrap();
    let result = client.attempt(AuthMethod::TOTP, code(&secret)).await;
    assert_eq!(result.status, AuthStatus::Success);
    let GatewayMessage::SessionToken(token) = client.receive().await else {
        panic!("expected SESSION_TOKEN");
    };
    assert_eq!(token.lifetime.as_secs(), 12 * 3600);
    (secret, token.token)
}

#[tokio::test]
async fn signs_in_enrolls_and_tunnels_with_an_assertion() {
    let directory = state_directory("tunnel");
    let gateway = start(&directory).await;
    let identity = TlsIdentity::generate_self_signed(vec!["qdp-client".to_owned()]).unwrap();
    let mut client = Client::connect(&gateway, &identity).await;
    let (secret, _) = sign_in_enrolling(&mut client, "Alice", "wonderland").await;
    client.send(GatewayMessage::HostsRequest).await;
    let GatewayMessage::Hosts(hosts) = client.receive().await else {
        panic!("expected HOSTS");
    };
    assert_eq!(hosts.len(), 1);
    assert_eq!(
        (hosts[0].host_id.as_str(), hosts[0].name.as_str()),
        ("echo", "Echo")
    );
    let GatewayMessage::Opened(Opened {
        host_pin,
        server_name,
        assertion,
    }) = client.open("echo").await
    else {
        panic!("expected OPENED");
    };
    let host_pin = host_pin.expect("the listed host's pin");
    assert_eq!(KeyPin(host_pin).to_hex(), HOST_PIN);
    assert_eq!(server_name, "qdp-host");
    let verified = IdentityAssertion::verify(
        &assertion,
        &[gateway.assertion_key],
        Expected {
            host_pin,
            client_pin: identity.key_pin().unwrap().0,
            now: now(),
        },
    )
    .unwrap();
    assert_eq!(verified.user_name, "alice");
    assert_eq!(verified.groups, ["QDP Users"]);
    client.echoes().await;

    // The secret was kept, and the code just used is not accepted again.
    let mut again = Client::connect(&gateway, &identity).await;
    let result = again.password("alice", "wonderland").await;
    assert_eq!(result.status, AuthStatus::NextFactor);
    assert_eq!(
        NextFactor::from_bytes(&result.auth_data)
            .unwrap()
            .enrollment,
        None
    );
    let result = again.attempt(AuthMethod::TOTP, code(&secret)).await;
    assert_eq!(result.status, AuthStatus::FailedRetry);
    assert_eq!(result.reason, "Sign-in failed");
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn reconnects_with_its_token_and_opens_hosts_by_address() {
    let directory = state_directory("token");
    let gateway = start(&directory).await;
    let identity = TlsIdentity::generate_self_signed(vec!["qdp-client".to_owned()]).unwrap();
    let mut first = Client::connect(&gateway, &identity).await;
    let (_, token) = sign_in_enrolling(&mut first, "bob", "builder").await;

    let mut client = Client::connect(&gateway, &identity).await;
    let result = client.attempt(AuthMethod::TOKEN, token.clone()).await;
    assert_eq!(result.status, AuthStatus::Success);
    assert!(matches!(
        client.receive().await,
        GatewayMessage::SessionToken(_)
    ));
    let target = format!("127.0.0.1:{}", gateway.echo_port);
    let GatewayMessage::Opened(opened) = client.open(&target).await else {
        panic!("expected OPENED");
    };
    // The gateway does not know a routed host's key, so it vouches for no one.
    assert_eq!(opened.host_pin, None);
    assert!(opened.assertion.is_empty());
    client.echoes().await;

    // A token is good only with the key it was issued to.
    let stranger = TlsIdentity::generate_self_signed(vec!["qdp-client".to_owned()]).unwrap();
    let mut thief = Client::connect(&gateway, &stranger).await;
    assert_eq!(
        thief.attempt(AuthMethod::TOKEN, token).await.status,
        AuthStatus::FailedRetry
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn refuses_wrong_passwords_and_hosts_the_policy_does_not_allow() {
    let directory = state_directory("refusals");
    let gateway = start(&directory).await;
    let identity = TlsIdentity::generate_self_signed(vec!["qdp-client".to_owned()]).unwrap();

    let mut client = Client::connect(&gateway, &identity).await;
    for status in [
        AuthStatus::FailedRetry,
        AuthStatus::FailedRetry,
        AuthStatus::FailedFinal,
    ] {
        let result = client.password("alice", "looking-glass").await;
        assert_eq!(result.status, status);
        assert_eq!(result.reason, "Sign-in failed");
    }
    let closed = client.connection.closed().await;
    let quinn::ConnectionError::ApplicationClosed(close) = closed else {
        panic!("expected the gateway to close, got {closed:?}");
    };
    assert_eq!(
        close.error_code.into_inner(),
        GatewayErrorCode::AUTH_FAILED.0.into_inner()
    );

    // An unknown user fails the same way.
    let mut client = Client::connect(&gateway, &identity).await;
    assert_eq!(
        client.password("mallory", "x").await.status,
        AuthStatus::FailedRetry
    );
    // TOTP before a password is not offered.
    assert_eq!(
        client
            .attempt(AuthMethod::TOTP, b"123456".to_vec())
            .await
            .status,
        AuthStatus::FailedRetry
    );

    // Carol may sign in, but reaches nothing.
    let mut client = Client::connect(&gateway, &identity).await;
    sign_in_enrolling(&mut client, "carol", "singer").await;
    for target in [
        "echo".to_owned(),
        format!("127.0.0.1:{}", gateway.echo_port),
        "10.9.9.9".to_owned(),
    ] {
        let GatewayMessage::OpenFailed(failed) = client.open(&target).await else {
            panic!("expected OPEN_FAILED for {target}");
        };
        assert_eq!(failed.code, OpenFailure::UNKNOWN_HOST);
    }
    let _ = std::fs::remove_dir_all(directory);
}
