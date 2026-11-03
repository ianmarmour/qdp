# qdp

The Quick Desktop Protocol (QDP): a low-latency protocol for remote desktop
and game streaming over QUIC. This crate implements the protocol itself, with
no I/O: state machines that take and return messages, which the other crates
connect to the network, the codecs and the operating system.

It follows the [specification](../../docs/spec/README.md) chapter by
chapter; each module names the sections it implements.

## Modules

| Module | What it holds | Spec |
|---|---|---|
| `registry` | The code points of every type and identifier space | 12 |
| `wire` | Variable-length integers, strings and octet sequences, and the framing of control messages and datagrams | 1.5, 3.6, 3.7 |
| `message` | The typed control messages and datagrams | 4 to 10 |
| `session` | The client's and the host's control-stream state machines | 4 |
| `auth` | SCRAM-SHA-256-PLUS, SPAKE2 pairing and TOTP | 4.4 |
| `discovery` | The DNS-SD service hosts advertise on a local network | 3.13 |
| `video` | Fragmentation, reassembly, retransmission, loss recovery and composition | 6 |
| `hid` | Parsing and validating the report descriptors of passed-through devices | 8.2.5 |
| `input` | Delivering input reports: retention and repeats on the client, ordering and acknowledgement on the host | 8 |
| `time` | The session clock, and estimating the peer's clock offset | 5.1, 5.2 |
| `gateway` | The messages and identity assertions of gateways | 13 |

## Where it fits

Every other QDP crate builds on this one: [`qdp-quic`](../qdp-quic/README.md)
runs its sessions over QUIC, and [`qdp-runtime`](../qdp-runtime/README.md)
builds hosts and clients from them. The
[architecture overview](../../docs/architecture.md) shows the rest.

The protocol is a draft: implementations of it identify themselves with the
ALPN token `qdp-draft-00`, and anything may change before version 1.

## Documentation

```bash
cargo doc -p qdp --open
```

The tests in `tests/` run sessions, lossy video and input end to end, without
a network.

## License

Apache-2.0, like the rest of [QDP](../../README.md).
