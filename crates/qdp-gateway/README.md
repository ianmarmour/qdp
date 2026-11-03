# qdp-gateway

The QDP gateway, which gives people in an organization access to the
computers inside its network.
[Gateway Setup](../../docs/gateway.md) shows how to deploy it.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together. It's internal to QDP, with no stable API.

API documentation: `cargo doc -p qdp-gateway --open`

## Usage

```text
Usage:
  qdp-gateway serve [--config FILE]
                          Serve users: sign them in with the directory and
                          TOTP, and tunnel their sessions to the hosts they
                          may reach
  qdp-gateway service install [--config FILE]
                          Run the gateway as a service from boot: a systemd
                          unit as the user qdp-gateway on Linux, a Windows
                          service as LocalService on Windows. Without a
                          configuration, writes an example to edit first
  qdp-gateway service uninstall
                          Stop and remove the service; its configuration and
                          keys stay
  qdp-gateway service status
                          Show whether the service runs
  qdp-gateway keys [--config FILE]
                          Print the pin clients pin the gateway by, and the
                          assertion key hosts trust it by
  qdp-gateway check [--config FILE]
                          Check the configuration, and that the gateway's own
                          directory account can bind
  qdp-gateway login USER [TARGET] [--config FILE]
                          Check USER's password, from QDP_PASSWORD, with the
                          directory, and show who they are, which hosts they
                          may reach and the groups assertions report; with
                          TARGET, a host's ID or its name or address inside
                          the network, whether they may open it
  qdp-gateway totp enroll USER [--config FILE]
                          Make USER a new TOTP secret, replacing any they had,
                          and print the otpauth:// URI that adds it to an
                          authenticator app, such as Google Authenticator,
                          Microsoft Authenticator or Authy (qrencode -t
                          ansiutf8 URI shows it as a QR code)
  qdp-gateway totp remove USER [--config FILE]
                          Forget USER's TOTP secret, so that they enroll again
  qdp-gateway totp list [--config FILE]
                          List the users with a TOTP secret
  qdp-gateway help        Show this help

The configuration is read from /etc/qdp-gateway/gateway.toml on Linux and
C:\ProgramData\qdp-gateway\gateway.toml on Windows unless --config names
another file.
```

## Dependencies

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| C compiler | Any | All | Building bundled C code |

## Build

On Linux or Windows, set up your system as
[Building](../../docs/building.md) describes, then:

```bash
cargo build --release -p qdp-gateway
```

The gateway is built as `target/release/qdp-gateway`. Its packages build from
there: see [Packaging](../../packaging/README.md).

## Test

```bash
cargo test -p qdp-gateway
```

## License

`qdp-gateway` is licensed under the [Apache License 2.0](../../LICENSE).
