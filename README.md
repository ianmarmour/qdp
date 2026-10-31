# qdp
Quick Desktop Protocol (QDP): a fast and modern alternative to RDP and VNC

The protocol is specified in [`doc/spec`](doc/spec), with the reasoning behind
its design in [`doc/design-rationale.md`](doc/design-rationale.md).

## Trying it

Building needs a Rust toolchain and a C compiler, which the codecs use for
their hand-written assembly. On x86-64 it also needs NASM:

```bash
sudo dnf install nasm
```

On Debian and Ubuntu that is `sudo apt install nasm`, on Arch
`sudo pacman -S nasm`, and on macOS with Homebrew `brew install nasm` (only
needed on Intel Macs). Apple Silicon Macs need only the Xcode command line
tools.

Build the `qdp` command, which is both the host and the client:

```bash
cargo build --release
```

On the host, print the key pin that clients use to recognize it, then serve.
The host creates its key in `./qdp-identity` the first time:

```bash
./target/release/qdp pin
```

```bash
./target/release/qdp serve --token my-secret
```

On the client, connect with the host's address or name and its pin. The host
listens on UDP port 4433 unless given `--listen`; add `:PORT` to the address
for another port:

```bash
./target/release/qdp connect 192.0.2.7 --pin PIN --token my-secret --output session.y4m
```

To use a password instead of a token, pass `--user NAME` on both sides with the
password in the `QDP_PASSWORD` environment variable; it is checked with
SCRAM-SHA-256-PLUS, so it never crosses the network.

For now the host streams a test pattern and the client writes the decoded
video to a YUV4MPEG2 file, which ffmpeg and most players open. Screen capture,
input, a client window and hardware video codecs come next. Run `qdp help` for
every option, and set `QDP_LOG=debug` for more detail.

## Crates

| Crate | Contents |
|---|---|
| `qdp` | The protocol: messages, session state machines, video and input logic, with no I/O |
| `qdp-quic` | QUIC transport with quinn and rustls |
| `qdp-codec` | Software AV1 encoding with rav1e and decoding with rav1d |
| `qdp-runtime` | Host and client media pipelines |
| `qdp-cli` | The `qdp` command |
