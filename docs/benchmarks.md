# Benchmark results

One run of [the benchmarks](../bench/README.md), on the machine described
under Environment. Its README says what each column measures, and how to
measure your own.

## Low motion at 60 Hz

### lan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 32.7 (32.9) | 16.6 (16.7) | 60.0 | 0.1 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 32.5 (32.6) | 30.7 (30.8) | 60.0 | 0.0 | 0.0% | 0 | 0.3 | 1.1 | 94.0 |
| rdp | 1 | 65.9 (66.0) | 56.1 (56.2) | 60.0 | 0.1 | 0.0% | 0 | 0.5 | 1.4 | 10.6 |
| vnc | 1 | 66.0 (66.9) | 36.2 (41.9) | 29.7 | 6.0 | 50.6% | 0 | 1.0 | 4.1 | 1.7 |

### wan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 32.6 (32.8) | 16.6 (16.7) | 60.0 | 0.1 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 66.1 (66.1) | 39.8 (40.0) | 59.8 | 1.0 | 0.3% | 0 | 0.3 | 1.9 | 85.4 |
| rdp | 1 | 99.2 (99.4) | 69.8 (69.9) | 60.0 | 0.1 | 0.0% | 0 | 0.5 | 1.3 | 10.7 |
| vnc | 1 | 99.4 (99.8) | 51.2 (51.7) | 30.0 | 1.4 | 50.1% | 0 | 0.9 | 3.7 | 2.0 |

## Low motion at 120 Hz

### lan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 15.9 (16.0) | 8.3 (8.4) | 120.0 | 0.0 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 32.7 (32.8) | 24.7 (24.7) | 120.0 | 0.0 | 0.0% | 0 | 0.5 | 1.4 | 84.9 |

### wan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 15.9 (16.2) | 8.3 (8.4) | 120.0 | 0.0 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 57.7 (57.8) | 35.2 (35.3) | 119.6 | 0.6 | 0.3% | 0 | 0.5 | 1.7 | 84.9 |

## Scrolling at 60 Hz

### lan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 32.4 (32.7) | 16.3 (16.5) | 60.0 | 0.1 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 32.5 (32.6) | 30.5 (30.6) | 60.0 | 0.0 | 0.0% | 0 | 0.9 | 1.1 | 93.9 |
| rdp | 1 | 65.9 (66.0) | 50.4 (50.5) | 60.0 | 0.1 | 0.0% | 0 | 2.6 | 2.0 | 38.7 |
| vnc | 1 | 65.9 (66.4) | 37.3 (38.1) | 30.0 | 1.5 | 49.9% | 0 | 4.8 | 7.7 | 3.9 |

### wan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 32.5 (32.7) | 16.3 (16.6) | 60.0 | 0.1 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 66.1 (66.1) | 39.7 (39.7) | 60.0 | 0.0 | 0.0% | 0 | 0.9 | 1.8 | 86.1 |
| rdp | 1 | 82.2 (82.5) | 66.0 (66.1) | 60.0 | 0.1 | 0.0% | 0 | 1.9 | 1.5 | 35.3 |
| vnc | 1 | 99.4 (99.6) | 52.2 (52.5) | 30.0 | 2.2 | 49.9% | 0 | 4.7 | 7.1 | 4.3 |

## Scrolling at 120 Hz

### lan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 15.7 (15.8) | 8.0 (8.2) | 120.0 | 0.0 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 24.2 (24.4) | 16.2 (16.3) | 120.0 | 0.0 | 0.0% | 0 | 1.3 | 1.9 | 85.7 |

### wan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 15.7 (15.8) | 8.0 (8.1) | 120.0 | 0.0 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 48.3 (50.7) | 26.8 (27.9) | 118.9 | 1.1 | 0.9% | 0 | 1.3 | 2.9 | 84.8 |

## Full motion at 60 Hz

### lan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 32.3 (32.5) | 16.3 (16.5) | 60.0 | 0.1 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 49.2 (49.2) | 32.9 (33.0) | 60.0 | 0.1 | 0.0% | 0 | 1.2 | 1.1 | 93.7 |
| rdp | 1 | 65.7 (65.8) | 56.6 (56.6) | 60.0 | 0.0 | 0.0% | 0 | 3.5 | 1.7 | 48.1 |
| vnc | 1 | 65.9 (66.6) | 41.6 (42.5) | 30.0 | 1.5 | 50.1% | 0 | 58.4 | 19.7 | 14.8 |

### wan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 32.2 (32.3) | 16.3 (16.5) | 60.0 | 0.1 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 65.7 (82.4) | 50.1 (50.2) | 60.0 | 0.0 | 0.0% | 0 | 1.2 | 0.9 | 86.4 |
| rdp | 1 | 99.0 (99.2) | 67.2 (67.3) | 60.0 | 0.2 | 0.0% | 0 | 3.5 | 1.7 | 49.1 |
| vnc | 1 | 99.3 (99.7) | 56.3 (56.7) | 30.0 | 1.4 | 50.1% | 0 | 58.9 | 18.3 | 14.2 |

## Full motion at 120 Hz

### lan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 15.6 (15.8) | 8.1 (8.2) | 120.0 | 0.0 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 24.4 (24.5) | 18.3 (18.4) | 119.6 | 0.5 | 0.3% | 0 | 1.9 | 1.9 | 86.2 |

### wan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 15.6 (15.7) | 8.1 (8.2) | 120.0 | 0.0 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 49.3 (49.7) | 27.8 (28.1) | 119.0 | 0.8 | 0.8% | 0 | 1.9 | 3.7 | 84.5 |

Latencies are medians, with the 95th percentile in parentheses. The frame time spread is the standard deviation of the time between new pictures.

## Environment

- Commit: 24bdb96f09f54879460cc6fd46368059bda86487
- Kernel: 7.2.11-200.fc44.x86_64
- CPU: AMD Ryzen 7 9800X3D 8-Core Processor
- GPU: 1002:744C
- QDP's encoder: VA-API on /dev/dri/renderD128 (Mesa Gallium driver 26.2.5-arch1.1 for AMD Radeon RX 7900 XTX (radeonsi, navi31, ACO, DRM 3.64, 7.2.11-200.fc44.x86_64))
- Screen: 1920x1080 at 60, 120 Hz, 15 seconds a run, 1 round
- Packages: mutter 51.1-1, gnome-remote-desktop 51.0-2, freerdp 2:3.32.2-1, tigervnc 1.16.2-5, mesa 1:26.2.5-1
