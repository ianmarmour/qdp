# Benchmarks

Measures how quickly QDP, RDP and VNC show a host's screen and answer a
client's input. Everything runs in one container on one Linux machine, with
every package pinned, so that results compare across machines and over time.

## Run

```bash
bench/run
```

It needs Podman or Docker, and uses the machine's GPU when it has one. The
first run builds the container and QDP, which takes a while; later runs reuse
both.

The report is written to `bench/results/TIME/report.md`, beside each run's
measurements and logs.

## Options

| Variable | Default | What it sets |
|---|---|---|
| `QDP_BENCH_PROTOCOLS` | `local qdp rdp vnc` | The protocols to measure |
| `QDP_BENCH_PROFILES` | `lan wan` | The network profiles to measure them over |
| `QDP_BENCH_ROUNDS` | `3` | How many times each is measured |
| `QDP_BENCH_SECONDS` | `30` | How long each measurement lasts |
| `QDP_BENCH_SIZE` | `1920x1080` | The host's and the client's screen size |
| `QDP_BENCH_REFRESH` | `60 120` | The screens' refresh rates, in hertz |

For example, `QDP_BENCH_ROUNDS=5 bench/run`.

## Protocols

| Protocol | Server | Viewer |
|---|---|---|
| `local` | None | None |
| `qdp` | `qdp serve` | `qdp connect`, full screen |
| `rdp` | GNOME Remote Desktop | FreeRDP, full screen |
| `vnc` | GNOME Remote Desktop | TigerVNC, in a window |

`local` is the baseline: the probe on the client's own screen, with no
protocol in between.

## Network profiles

| Profile | Round trip | Packet loss |
|---|---|---|
| `lan` | The machine's own | None |
| `wan` | 30 ms more | None |
| `lossy` | 30 ms more | 1% each way |

Profiles other than `lan` need the kernel's `sch_netem` module:
`sudo modprobe sch_netem`.

## What it measures

The host and the client are headless GNOME sessions, measured at each refresh
rate in turn. On the host's screen, a probe shows in every frame the time it
drew it, and lights a marker while a button is held. On the client's, each
protocol's viewer shows the host's screen, and the measurer finds the probe
there, reads it and clicks on it.
Both read the same clock.

| Column | What it is |
|---|---|
| Click to photon | From a click on the client to its effect showing on the client's screen |
| Display latency | From the host's screen changing to the change showing on the client's |
| Frame rate | New pictures a second on the client's screen, of one each refresh |
| Frame time spread | How unevenly the pictures arrive: the standard deviation of the time between them |
| Skipped frames | The client screen's refreshes that brought no new picture |
| Missed clicks | Clicks whose effect never showed |
| Bandwidth | The traffic between the host and the client |
| CPU | The host's server's and the client's viewer's, in percent of one core |
