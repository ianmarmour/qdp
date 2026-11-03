# qdp-bench

The `qdp-bench` command, which measures how quickly a remote desktop protocol
shows a host's screen and answers a client's input, between two headless
GNOME sessions on one machine. [Benchmarks](../../bench/README.md) runs it for
QDP, RDP and VNC in a container, which is the way to use it.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together.

## Usage

```text
qdp-bench: measure remote desktop latency between headless GNOME sessions

Usage:
  qdp-bench probe
  qdp-bench measure --protocol NAME --profile NAME --out FILE [options]
  qdp-bench report DIRECTORY
  qdp-bench help

probe shows the probe full screen, for the host's session.

measure watches the client's session, where a viewer shows the probe full
screen, and writes what it measured to FILE:
  --protocol NAME      The protocol the viewer streams with
  --profile NAME       The network profile the stream runs over
  --round N            The round of the benchmark this run is (default 1)
  --seconds N          How long to measure (default 30)
  --process ROLE=PID   Measure the CPU time of process PID as ROLE
                       (repeatable)
  --interface NAME     The network interface whose traffic to count
                       (default lo)
  --out FILE           Where to write the results

report summarises every run's results in DIRECTORY as Markdown.
```

## Dependencies

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| C compiler | Any | Linux | Building bundled C code |
| NASM | 2.15+ | x86-64 | Building the codecs |
| PipeWire | 0.3+ | Linux | Reading the client's screen |
| libclang | 9+ | Linux | Building screen capture |
| Mutter | 46+ | Linux | The sessions it measures between |

## Build

On Linux, set up your system as [Building](../../docs/building.md#linux)
describes, then:

```bash
cargo build --release -p qdp-bench
```

The command is built as `target/release/qdp-bench`.

## Test

```bash
cargo test -p qdp-bench
```

## License

`qdp-bench` is licensed under the [Apache License 2.0](../../LICENSE).
