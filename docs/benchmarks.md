# Benchmark results

One run of [the benchmarks](../bench/README.md), on the machine described
under Environment. Its README says what each column measures, and how to
measure your own.

## Low motion at 60 Hz

### lan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 32.5 (32.8) | 16.6 (16.7) | 60.0 | 0.1 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 49.4 (49.4) | 39.1 (39.2) | 60.0 | 0.0 | 0.0% | 0 | 0.3 | 1.5 | 1.3 |
| rdp | 1 | 65.9 (66.0) | 54.0 (54.1) | 60.0 | 0.1 | 0.0% | 0 | 0.5 | 1.5 | 10.5 |
| sunshine | 1 | – | 22.8 (22.8) | 60.0 | 0.0 | 0.0% | 6 | 18.9 | 1.5 | 1.7 |
| vnc | 1 | 66.0 (66.5) | 36.3 (41.9) | 29.6 | 5.6 | 50.6% | 0 | 1.0 | 4.0 | 1.7 |

### wan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 32.8 (33.0) | 16.6 (16.7) | 60.0 | 0.1 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 82.5 (82.6) | 54.8 (54.8) | 60.0 | 0.0 | 0.0% | 0 | 0.3 | 1.5 | 1.7 |
| rdp | 1 | 82.5 (82.8) | 66.5 (66.7) | 60.0 | 0.2 | 0.0% | 0 | 0.5 | 1.5 | 10.8 |
| sunshine | 1 | – | 40.2 (40.3) | 60.0 | 0.0 | 0.0% | 6 | 18.9 | 1.5 | 1.6 |
| vnc | 1 | 99.2 (100.2) | 51.2 (51.9) | 30.0 | 1.4 | 50.0% | 0 | 0.9 | 4.0 | 1.7 |

## Low motion at 120 Hz

### lan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 15.9 (16.1) | 8.3 (8.3) | 120.0 | 0.0 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 24.2 (24.4) | 19.5 (19.5) | 120.0 | 0.0 | 0.0% | 0 | 0.5 | 2.7 | 2.4 |
| sunshine | 1 | – | 17.7 (17.9) | 120.0 | 0.1 | 0.0% | 6 | 28.4 | 2.7 | 2.8 |

### wan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 15.9 (16.1) | 8.3 (8.3) | 120.0 | 0.0 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 49.4 (49.4) | 29.0 (29.1) | 119.9 | 0.4 | 0.1% | 0 | 0.5 | 2.5 | 2.3 |
| sunshine | 1 | – | 27.6 (27.6) | 120.0 | 0.0 | 0.0% | 6 | 28.4 | 3.2 | 2.7 |

## Scrolling at 60 Hz

### lan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 32.5 (32.7) | 16.4 (16.5) | 60.0 | 0.1 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 49.4 (49.5) | 38.8 (38.9) | 60.0 | 0.0 | 0.0% | 0 | 0.9 | 1.6 | 1.4 |
| rdp | 1 | 65.8 (65.9) | 53.7 (53.8) | 60.0 | 0.0 | 0.0% | 0 | 2.7 | 2.1 | 37.8 |
| sunshine | 1 | – | 22.6 (22.7) | 60.0 | 0.0 | 0.0% | 6 | 18.9 | 1.5 | 1.7 |
| vnc | 1 | 65.9 (66.5) | 37.4 (38.2) | 30.0 | 1.5 | 49.9% | 0 | 5.0 | 7.9 | 3.9 |

### wan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 32.6 (32.7) | 16.5 (16.6) | 60.0 | 0.1 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 82.5 (82.6) | 54.6 (54.6) | 60.0 | 0.0 | 0.0% | 0 | 0.9 | 1.5 | 1.7 |
| rdp | 1 | 98.9 (99.1) | 69.0 (69.1) | 60.0 | 0.0 | 0.0% | 0 | 1.9 | 2.1 | 35.3 |
| sunshine | 1 | – | 40.0 (40.2) | 60.0 | 0.0 | 0.0% | 6 | 18.9 | 1.3 | 1.7 |
| vnc | 1 | 98.9 (100.2) | 52.1 (53.3) | 30.0 | 1.4 | 49.9% | 0 | 4.7 | 7.5 | 3.7 |

## Scrolling at 120 Hz

### lan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 15.7 (15.9) | 8.0 (8.2) | 120.0 | 0.0 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 24.4 (24.4) | 19.3 (19.4) | 120.0 | 0.0 | 0.0% | 0 | 1.3 | 2.6 | 2.5 |
| sunshine | 1 | – | 14.9 (15.0) | 120.0 | 0.0 | 0.0% | 6 | 28.4 | 2.3 | 3.1 |

### wan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 15.8 (16.0) | 8.1 (8.2) | 120.0 | 0.1 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 49.4 (50.5) | 28.8 (29.9) | 119.2 | 0.9 | 0.7% | 0 | 1.3 | 2.6 | 2.7 |
| sunshine | 1 | – | 27.4 (27.5) | 120.0 | 0.0 | 0.0% | 6 | 28.4 | 2.7 | 2.9 |

## Full motion at 60 Hz

### lan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 32.3 (32.5) | 16.4 (16.5) | 60.0 | 0.0 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 49.2 (49.4) | 37.8 (37.9) | 60.0 | 0.0 | 0.0% | 0 | 1.2 | 1.5 | 1.3 |
| rdp | 1 | 65.5 (65.7) | 49.2 (49.4) | 60.0 | 0.1 | 0.0% | 0 | 3.5 | 1.6 | 40.9 |
| sunshine | 1 | – | 23.3 (23.4) | 60.0 | 0.0 | 0.0% | 6 | 19.2 | 1.6 | 1.7 |
| vnc | 1 | 65.2 (66.6) | 40.3 (41.5) | 30.0 | 1.5 | 49.9% | 0 | 59.1 | 17.2 | 12.3 |

### wan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 32.4 (32.5) | 16.4 (16.5) | 60.0 | 0.0 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 82.7 (82.8) | 53.6 (53.7) | 60.0 | 0.0 | 0.0% | 0 | 1.2 | 1.7 | 1.8 |
| rdp | 1 | 82.2 (82.3) | 60.8 (60.9) | 60.0 | 0.1 | 0.0% | 0 | 3.5 | 1.9 | 41.7 |
| sunshine | 1 | – | 46.6 (46.7) | 60.0 | 0.0 | 0.0% | 6 | 19.2 | 1.6 | 1.6 |
| vnc | 1 | 99.0 (100.3) | 55.1 (56.5) | 30.0 | 1.4 | 49.9% | 0 | 59.1 | 16.9 | 12.0 |

## Full motion at 120 Hz

### lan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 15.6 (15.8) | 8.1 (8.2) | 120.0 | 0.0 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 16.0 (16.1) | 12.9 (13.0) | 120.0 | 0.0 | 0.0% | 0 | 1.9 | 3.3 | 2.5 |
| sunshine | 1 | – | 19.1 (19.1) | 120.0 | 0.0 | 0.0% | 6 | 28.4 | 3.1 | 3.1 |

### wan

| Protocol | Runs | Click to photon (ms) | Display latency (ms) | Frame rate (fps) | Frame time spread (ms) | Skipped frames | Missed clicks | Bandwidth (Mbit/s) | server CPU (%) | viewer CPU (%) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| local | 1 | 15.6 (15.7) | 8.1 (8.2) | 120.0 | 0.0 | 0.0% | 0 | 0.0 | – | – |
| qdp | 1 | 49.4 (49.8) | 30.2 (30.8) | 119.5 | 0.6 | 0.4% | 0 | 1.9 | 2.7 | 3.0 |
| sunshine | 1 | – | 29.4 (29.5) | 120.0 | 0.0 | 0.0% | 6 | 28.4 | 3.0 | 2.9 |

Latencies are medians, with the 95th percentile in parentheses. The frame time spread is the standard deviation of the time between new pictures.

## Environment

- Commit: 324021965025b90300153ae86cb043f5d067294d
- Kernel: 7.2.11-200.fc44.x86_64
- CPU: AMD Ryzen 7 9800X3D 8-Core Processor
- GPU: 1002:744C
- QDP's encoder: VA-API on /dev/dri/renderD128 (Mesa Gallium driver 26.2.5-arch1.1 for AMD Radeon RX 7900 XTX (radeonsi, navi31, ACO, DRM 3.64, 7.2.11-200.fc44.x86_64))
- QDP's decoder: VA-API on /dev/dri/renderD128 (Mesa Gallium driver 26.2.5-arch1.1 for AMD Radeon RX 7900 XTX (radeonsi, navi31, ACO, DRM 3.64, 7.2.11-200.fc44.x86_64))
- Screen: 1920x1080 at 60, 120 Hz, 15 seconds a run, 1 round
- Packages: mutter 51.1-1, gnome-remote-desktop 51.0-2, freerdp 2:3.32.2-1, tigervnc 1.16.2-5, sunshine 2026.1103.235308-1, moonlight-qt 6.2.0-1, mesa 1:26.2.5-1
