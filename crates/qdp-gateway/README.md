# qdp-gateway

> **Internal crate.** It exists for QDP's own programs, has no stable API,
> and isn't published to crates.io.

The QDP gateway, which lets users outside an organization's network reach the
QDP hosts inside it. It checks each user with the organization's directory,
LDAP or Active Directory, then with a TOTP code from their authenticator app,
and tunnels their QDP sessions, end to end, to the hosts its policy lets them
reach.

**To deploy it, see [the gateway guide](../../docs/gateway.md)**: installing,
configuration and commands. Section 13 of the
[specification](../../docs/spec/13-gateway.md) defines the protocol.

It stands apart from QDP's apps and hosts: it never reads a session, and
needs only [`qdp`](../qdp/README.md)'s gateway messages and
[`qdp-quic`](../qdp-quic/README.md).

## Code

| File | What it does |
|---|---|
| `src/main.rs` | The `qdp-gateway` command |
| `src/config.rs` | The configuration file |
| `src/server.rs` | Signing users in and tunnelling their sessions |
| `src/directory.rs` | LDAP and Active Directory |
| `src/totp_store.rs`, `src/enrollments.rs` | TOTP secrets and enrolling them |
| `src/policy.rs` | Which users may reach which hosts |
| `src/tokens.rs` | Session tokens |
| `src/keys.rs` | The gateway's keys and identity assertions |
| `src/limiter.rs` | Limits on failed sign-ins |
| `src/service` | Running as a systemd unit or a Windows service |

`tests/gateway.rs` runs the gateway end to end on the loopback interface:
users sign in with a password and a TOTP code, enrolling a secret the first
time, open hosts by ID and by address, and reach them through the tunnel.

## Building

```bash
cargo build --release -p qdp-gateway
```

The packages build from it: see [`packaging`](../../packaging/README.md).

## License

Apache-2.0, like the rest of [QDP](../../README.md).
