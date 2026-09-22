# LoadLoom

**Authorized load-testing console — headless engine + native desktop shell**

[简体中文](README.md) | **English**

[![CI](https://github.com/CheerSaltman/loadloom/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/CheerSaltman/loadloom/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](rust-toolchain.toml)
[![Tauri v2](https://img.shields.io/badge/Tauri-v2-24C8DB.svg)](https://tauri.app/)

LoadLoom is split into three layers:

- **`loadloom-core`** — a pure Rust library with zero GUI dependencies. It needs no window, no browser and no event loop, so it compiles, tests and runs inside CI, containers and headless servers.
- **`src-tauri`** — the native desktop shell (Tauri v2). It only bridges IPC.
- **`src`** — a React 19 frontend responsible for rendering and passing parameters down.
- **`sidecars/loadloom-pt`** — a Go BitTorrent engine for DHT / PEX / uTP / TCP, RAM-only piece storage, and peer-health analysis.

The contract between the two sides is defined in `contract.rs`; the frontend's `src/bindings.ts` is a hand-written mirror of it, checked field by field by a test.

> ### ⚠️ Read this first: authorized targets only
>
> Use this tool **only** against systems you own or have explicit written authorization to test. Applying pressure to systems without permission may violate laws and terms of service.
>
> The app asks you to confirm authorization before traffic starts, and refuses to start if it is not confirmed (it returns a `rejected` event). Every request carries a `_ll={worker_id}-{request_id}` parameter so the target's operators can identify and trace your traffic in their access logs.

---

## Table of contents

- [Features](#features)
- [Quick start](#quick-start)
- [Tutorial: the desktop app](#tutorial-the-desktop-app)
- [Tutorial: use it as a library (headless)](#tutorial-use-it-as-a-library-headless)
- [Architecture breakdown: three layers](#architecture-breakdown-three-layers)
- [How it works: three key designs](#how-it-works-three-key-designs)
- [Building from source](#building-from-source)
- [Project structure](#project-structure)
- [More documentation](#more-documentation)

---

## Features

| Capability | Description |
| --- | --- |
| **Headless core** | `loadloom-core`'s dependency closure contains no GUI or rendering library, and it runs real traffic in a windowless environment |
| **Concurrency ramp** | 1–32 workers; dragging the slider while a run is active takes effect immediately, no restart needed |
| **LAN piece simulation** | Rotates 256 KiB–2 MiB HTTP Range requests across local/LAN peers; hostnames and public IPs are rejected |
| **Real public BitTorrent** | Accepts magnets or public `.torrent` URLs with DHT / PEX / uTP / TCP and 8–500 peer connections; payload is held in RAM only and discarded after piece verification |
| **Peer health analysis** | Reports handshakes, half-open/useful/stalled/dead peers, churn, tracker results, bad pieces, and wasted traffic; protocol keepalive is 30 seconds |
| **Global rate limit** | Token bucket; `0` means unlimited, capped at 4096 MiB/s, adjustable mid-run |
| **Pressure guard** | Watches request failure rate and sustained time-to-first-byte latency, then trips an automatic circuit breaker at the configured threshold |
| **Auto-stop** | A byte threshold (GB) and a duration threshold (minutes) apply independently; `0` disables either one |
| **Live streaming** | Metrics are pushed every 250 ms; the frontend never polls and drops out-of-order frames via a monotonic `seq` |
| **Error classification** | Aggregated by code: `TIMEOUT` / `CONNECT_FAILED` / `BODY_STREAM` / `DECODE_ERROR` / `REDIRECT_ERROR` / `REQUEST_ERROR` / `UNKNOWN` |
| **Crash forensics** | A panic hook writes both to the log file and to the in-app "Run log" page |
| **Native window** | Tauri v2 with the system WebView2; it listens on no ports and spawns no browser process |
| **Tray resident** | The close button minimizes to the tray (it does not exit); quit from the tray menu |
| **NIC link monitoring** | Link up/down, negotiated speed, rx/tx utilization, discards, errors, and the tx queue — on the same timeline as the traffic log (Windows; metrics that consumer hardware does not expose are stated as such, never fabricated) |

---

## Quick start

### Option 1: download an installer (recommended)

Grab one of the three assets from [Releases](https://github.com/CheerSaltman/loadloom/releases/latest):

| File | Who it is for |
| --- | --- |
| `LoadLoom_<version>_x64-setup.exe` | **Most users.** NSIS installer with a Start Menu entry and an uninstaller |
| `LoadLoom_<version>_x64_en-US.msi` | Enterprise deployment via Group Policy or silent install (`msiexec /i`) |
| `LoadLoom_<version>_x64-portable.exe` | Portable, no installation — just double-click |

> Requirements: Windows 10/11 x64 plus the **WebView2 Runtime** (bundled with Windows 11 and recent Windows 10 builds; if it is missing, install the [Evergreen Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) once).

### Option 2: build from source

See [Building from source](#building-from-source).

---

## Tutorial: the desktop app

### 1. Launch

Open **LoadLoom** from the Start Menu, or double-click `LoadLoom_<version>_x64-portable.exe`.
On first launch it creates its log directory: `%LOCALAPPDATA%\LoadLoom\logs\loadloom.log`.

### 2. Confirm you are authorized

The UI has an authorization checkbox:

- Not checked → pressing **Start** makes the engine return a `rejected` event and no request is ever sent.
- Checked → every request carries `_ll={worker_id}-{request_id}`, so the target's operators can recognize you in their access logs.

### 3. Fill in the target and parameters

| Parameter | How to fill it | Notes |
| --- | --- | --- |
| **Target URL** | The full address you want to test | Practise against **your own** service first |
| **Traffic profile** | HTTP download, LAN piece simulation, or real public BitTorrent | Use legitimate public content such as Ubuntu or Debian for public swarm tests |
| **Concurrency** | 1–32 | Start small. Over loopback the bottleneck is usually the machine running the test |
| **Ramp-up** | Seconds; `0` = off | Gradually raises load from one worker to the target concurrency |
| **Rate limit** | MiB/s, `0` = unlimited | Set a cap to measure behaviour under a fixed bandwidth; leave `0` to find the ceiling |
| **Failure-rate breaker** | Percent; `0` = off | Applies after at least 20 requests and stops the run when the threshold is reached |
| **Latency breaker** | Milliseconds; `0` = off | Stops after TTFB remains above the threshold for about one second; shorter spikes only warn |
| **Auto-stop · bytes** | GB, `0` = off | Independent of the duration threshold; either one triggers a stop |
| **Auto-stop · duration** | Minutes, `0` = off | Set a cap for long runs so you do not forget about them |

### 4. Start, then watch

Once you press **Start**:

- The **metrics panel** refreshes every 250 ms: current rate, in-flight requests, bytes sent, failure counts.
- The **pressure guard card** shows normal, warning, or tripped state plus the reason; warnings are logged without stopping the run.
- The **line chart** keeps the last 120 samples; the full history arrives once in the handshake frame, then only deltas are pushed.
- The **Run log** page shows engine logs and error details, mirrored to the log file on disk.

While a run is active you can change concurrency and rate limit on the fly.

### 5. Stop

- A threshold is reached → auto stop (an `autoStopped` event is pushed).
- You press **Stop** → a `stopped` event is pushed.
- You click the window's close button → it only minimizes to the tray; traffic keeps running. To really quit, use **Quit** in the tray context menu.

### 6. NIC monitoring (diagnosing "it suddenly slowed down")

The third tab is **NIC monitoring** (Windows). It shares one timeline with the traffic run and
answers a single question: is the slowdown the origin's problem, or the local link's? It samples
every 500 ms and keeps a 2-minute chart.

| What you can see | Notes |
| --- | --- |
| Link up / down | Down is logged as `NIC-010`; recovery includes the outage duration (`NIC-011`) |
| Negotiated speed | e.g. 1 Gbps dropping to 100 Mbps (`NIC-012`) — cable / port / Wi-Fi problems become obvious |
| Rx / tx utilization | Live rate ÷ negotiated speed, showing the headroom left on the link |
| Discards / errors | Recorded as *episodes* with a start and an end, plus peak value and duration (`NIC-020` ~ `NIC-023`) |
| Tx queue | `OutQLen` sustained above 16 packets is logged as backlog (`NIC-024` / `NIC-025`) — the driver cannot keep up |
| Event timeline | Every change in time order, directly comparable with the rate chart in the traffic log |

**What you cannot see** (driver-private data that consumer devices do not expose): NIC temperature,
rx/tx buffer occupancy, optical transceiver power. The page states this boundary up front — what we
cannot read is declared, not faked with a `0`.

The watched set defaults to physical NICs and can be edited by hand (virtual adapters / tunnels /
loopback each have a class tag). Monitoring is independent of the traffic run: it keeps sampling
when no run is active and after a run stops.

### 7. Real BitTorrent self-test with one Wi-Fi adapter and one router

Choose **Real public PT Swarm** and paste a legitimate public magnet or `.torrent` URL. The default is 180 peers with a 512 MiB RAM ceiling. The Go sidecar creates no download file and uploads nothing; each piece is released immediately after hash verification. The UI reports useful and wire rates, RAM peak, half-open/stalled/dead peers, and tracker health.

With only one computer, one Wi-Fi adapter, and a router, the result is necessarily the end-to-end ceiling of the Wi-Fi link, router, ISP, and public swarm. A single endpoint cannot prove that the router is never the bottleneck. Compare it with NIC monitoring: RX utilization near 100% without errors/discards points toward the wireless link; low utilization with high dead-peer, half-open, or tracker-error counts points toward the swarm, ISP, or router NAT.

### 8. Troubleshooting

| Symptom | Where to look first |
| --- | --- |
| Failures keep climbing | Error codes on the **Run log** page: `CONNECT_FAILED` is usually network/port/certificate, `TIMEOUT` usually means the target is saturated or packets are dropped |
| PT mode refuses to start | A target or peer is not a literal LAN address; hostnames are intentionally rejected to prevent DNS rebinding |
| Pressure guard stopped the run | Check the pressure card and `RUN-007` log to distinguish a failure-rate trip from sustained TTFB latency |
| Rate plateaus | Gains beyond 8 workers are typically small; first make sure the test machine or the target is not the bottleneck |
| Rate suddenly collapses / stalls | **NIC monitoring** tab: is the link down, did the negotiated speed drop (e.g. 1 Gbps → 100 Mbps), are there discard / error spikes |
| UI seems unresponsive | Check the tray icon — the window may be minimized while the process is alive |
| The app exited unexpectedly | The log file contains a panic record (guaranteed by the panic hook) |

---

## Tutorial: use it as a library (headless)

`loadloom-core` captures a runtime handle at construction time (reusing an existing one, or creating its own), so you can start traffic from any thread — including a bare `main` thread with no runtime.

The smallest runnable example:

```bash
cargo run --locked --example headless_smoke -p loadloom-core
```

```text
引擎已创建（自带执行器，不依赖调用方线程的运行时上下文）
阶段=Running 在飞=4 已传字节=0 失败=0
...
已停止，最终阶段=Idle
```

The example lives in `examples/`, compiled by `cargo test`.

Regression testing in CI, containers or headless servers:

```bash
cargo test --locked -p loadloom-core     # all core tests, no graphical environment required
```

---

## Architecture breakdown: three layers

```
┌──────────────────────────────────────────────────────────┐
│  src/            React 19 + Tailwind (render / send params)│
│    bindings.ts     mirrored IPC contract (hand-written+guard)│
│    hooks/useEngine streaming subscription, seq dedup, no poll│
└───────────────────────────┬──────────────────────────────┘
                            │  Tauri IPC (Channel / Event / Command)
┌───────────────────────────┴──────────────────────────────┐
│  src-tauri/      native shell (thin, zero business logic) │
│    9 commands, tray, window lifecycle, log persistence     │
└───────────────────────────┬──────────────────────────────┘
                            │  Rust function calls (same process)
┌───────────────────────────┴──────────────────────────────┐
│  crates/loadloom-core/   headless engine (zero GUI deps)   │
│    contract.rs   single source of truth for the contract  │
│    engine.rs     rate limiter / workers / metrics / ticks  │
└──────────────────────────────────────────────────────────┘
```

Responsibilities and dependency direction:

| Layer | Responsibility | Must not do |
| --- | --- | --- |
| `loadloom-core` | Traffic generation, rate limiting, metrics, contract definition | No windows, no IPC, no frontend types |
| `src-tauri` | IPC bridging, tray, window lifecycle, log persistence | No business rules, no data processing |
| `src` | Rendering, interaction, parameter submission | No polling, no guessing field names, no hard-coded contract |

---

## How it works: three key designs

- **Stream, do not poll**: metrics travel through `tauri::ipc::Channel<MetricsSnapshot>` every 250 ms; logs and lifecycle use events. Each frame carries a monotonic `seq`, and the frontend drops out-of-order or duplicate frames. The handshake frame carries the full 120-sample history, after which only the single `latest` point is pushed.
- **Single source of truth + mechanical guard**: `contract.rs` is the only definition. `tauri-specta` only ships `2.0.0-rc` releases, so `src/bindings.ts` is a hand-written mirror, guarded by the `contract_wire` test, which reads and parses that file and compares it field by field against the JSON serde produces.
- **Crashes must leave a trace**: the panic hook writes to the log file and to the in-app log stream. Release builds keep `panic = "unwind"` and a `line-tables-only` line table.

A contract test failure looks like this:

```text
契约漂移：EngineLimits (Rust) 与 EngineLimits (bindings.ts) 字段不一致
  仅 Rust 有: ["maxWorkers"]
  仅 TS 有  : ["maxWorkersX"]
```

---

## Building from source

**Prerequisites**: Rust (pinned by `rust-toolchain.toml`), Node.js ≥ 20, Go ≥ 1.24, and the WebView2 Runtime on Windows.

```bash
git clone https://github.com/CheerSaltman/loadloom.git
cd loadloom

npm install
npm run typecheck          # tsc --noEmit
npm run build              # emits the frontend into dist/
npm run pt:test            # tests the Go PT sidecar
npm run pt:build           # builds the RAM-only BitTorrent sidecar

cargo test --locked -p loadloom-core   # all core tests, no graphical environment needed
npm run desktop:dev        # development: hot-reloading native window
npm run desktop:build      # packaging: produces exe + NSIS + MSI
```

Build outputs:

```text
target/release/loadloom-desktop.exe                       ← portable executable
target/release/bundle/nsis/LoadLoom_<version>_x64-setup.exe   ← NSIS installer
target/release/bundle/msi/LoadLoom_<version>_x64_en-US.msi    ← MSI installer
```

Development workflow, quality gates and the release procedure live in [docs/development.md](docs/development.md) (in Chinese).

---

## Project structure

```text
loadloom/
├── crates/nicmon/                 # NIC counter collection (the only FFI isolation layer, Windows)
├── crates/loadloom-core/          # headless engine (library)
│   ├── src/
│   │   ├── lib.rs                # the only public entry point
│   │   ├── engine.rs             # the engine itself
│   │   ├── nic/                  # NIC analysis: pure verdicts + event stream
│   │   └── contract.rs           # single source of truth for the contract
│   ├── tests/                    # integration tests (outside the published artifact)
│   │   ├── contract_wire.rs      # contract guard: parses bindings.ts field by field
│   │   └── headless_e2e.rs       # headless end-to-end: real traffic against a local fake origin
│   ├── benches/throughput.rs     # benchmark entry (outside the test suite)
│   └── examples/headless_smoke.rs# runnable example (compiled by cargo test)
├── src-tauri/                    # native desktop shell
│   ├── src/{lib,main,logging}.rs
│   ├── capabilities/default.json
│   ├── icons/
│   └── tauri.conf.json
├── sidecars/loadloom-pt/         # Go: real BT swarm + verify-and-discard RAM storage + peer diagnostics
├── src/                          # React frontend
│   ├── components/LineChart.tsx  # hand-drawn Canvas chart (no third-party, no CDN)
│   ├── components/NicPanel.tsx   # NIC monitoring page (utilization bars / chart / timeline)
│   ├── hooks/useEngine.ts        # streaming subscription (seq dedup, no polling)
│   ├── hooks/useNic.ts           # NIC streaming subscription (independent of a run)
│   ├── bindings.ts               # mirrored IPC contract
│   └── App.tsx
├── docs/                         # architecture, conventions, development
├── scripts/                      # ops / acceptance scripts (not part of the build)
├── .github/                      # CI and Dependabot
└── Cargo.toml                    # workspace manifest (with inherited metadata and lints)
```

---

## More documentation

| Document | Contents |
| --- | --- |
| [docs/architecture.md](docs/architecture.md) | Architectural decisions and their trade-offs |
| [docs/development.md](docs/development.md) | Development environment, quality gates, local acceptance and the release procedure |
| [docs/conventions.md](docs/conventions.md) | Repository conventions traced back to official documentation |
| [CONTRIBUTING.md](CONTRIBUTING.md) | How to file issues and pull requests |
| [SECURITY.md](SECURITY.md) | How to report a vulnerability |
| [CHANGELOG.md](CHANGELOG.md) | Release history (Keep a Changelog format) |

> The `docs/` directory is currently written in Chinese only. Pull requests that translate it are welcome.

---

## License

[MIT](LICENSE) © 2026 CheerSaltman

The software is provided "as is", without warranty of any kind. Users are responsible for ensuring they are authorized to test their targets, and for all consequences of doing so.
