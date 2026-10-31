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
for another port. The stream opens in a window, and closing the window
disconnects:

```bash
./target/release/qdp connect 192.0.2.7 --pin PIN --token my-secret
```

With `--headless` the client opens no window and only logs statistics, and
`--output session.y4m` also writes the decoded video to a YUV4MPEG2 file, which
ffmpeg and most players open. To build without window support, for a machine
that only serves, use `cargo build --release --no-default-features`.

To use a password instead of a token, pass `--user NAME` on both sides with the
password in the `QDP_PASSWORD` environment variable; it is checked with
SCRAM-SHA-256-PLUS, so it never crosses the network.

On Linux the host encodes on the GPU through VA-API when the GPU can encode
AV1 (AMD RDNA 3 or newer, Intel Arc or newer), and otherwise falls back to
software encoding. It needs the GPU's VA-API driver, which desktop
distributions install with Mesa (`mesa-va-drivers`) or Intel's media driver
(`intel-media-driver`); `vainfo` lists `VAProfileAV1Profile0` with
`VAEntrypointEncSlice` when hardware encoding is available. The host logs which
encoder it uses, and `--encoder software` or `--encoder hardware` forces one.

Clients decode on the GPU where they can: on Macs with hardware AV1 decoding
(Apple M3 and newer) through VideoToolbox, falling back to software decoding
elsewhere or if the hardware cannot handle a stream. `--decoder software` or
`--decoder hardware` forces one.

For now the host streams a test pattern. Screen capture and input come next. Run `qdp help` for every option, and set `QDP_LOG=debug`
for more detail.

## Crates

| Crate | Contents |
|---|---|
| `qdp` | The protocol: messages, session state machines, video and input logic, with no I/O |
| `qdp-quic` | QUIC transport with quinn and rustls |
| `qdp-codec` | Software AV1 encoding with rav1e and decoding with rav1d |
| `qdp-runtime` | Host and client media pipelines |
| `qdp-cli` | The `qdp` command |
