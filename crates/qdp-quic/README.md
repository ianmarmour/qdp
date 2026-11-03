# qdp-quic

QUIC transport for the Quick Desktop Protocol (QDP), built on
[quinn](https://github.com/quinn-rs/quinn) and
[rustls](https://github.com/rustls/rustls). It connects the sans-I/O protocol
logic of [`qdp`](../qdp/README.md) to the network, as Section 3 of the
[specification](../../docs/spec/03-transport.md) describes.

## What's inside

- `HostEndpoint` and `ClientEndpoint` establish connections with TLS 1.3,
  ALPN `qdp-draft-00` and QUIC datagrams, with no session resumption and no
  0-RTT, and open the control stream.
- `HostVerification` chooses how a client trusts the host: a pinned key,
  checked by `PinnedKeyVerifier`, or public key infrastructure.
- `TlsIdentity` holds a host's certificate and key, and can generate a
  self-signed one whose `KeyPin` clients pin; a client presents one to hosts
  that authorize it by its key.
- `SessionDriver` runs a client or host session state machine over a
  `QdpConnection`, and delivers the peer's unidirectional streams as
  `ReceivedStream`s.
- `probe` checks that a QDP host answers at an address without connecting to
  it, with a QUIC version negotiation, as the apps do to show whether paired
  computers are online.

## Where it fits

[`qdp-runtime`](../qdp-runtime/README.md) builds hosts and clients on it, and
[`qdp-gateway`](../qdp-gateway/README.md) tunnels sessions with it. The tests
in `tests/loopback.rs` connect a host and a client on one machine.

## Documentation

```bash
cargo doc -p qdp-quic --open
```

## License

Apache-2.0, like the rest of [QDP](../../README.md).
