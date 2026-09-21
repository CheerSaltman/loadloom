# LoadLoom

**Authorized load-testing console — headless engine + native desktop shell**

[简体中文](README.md) | **English**

[![CI](https://github.com/CheerSaltman/loadloom/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/CheerSaltman/loadloom/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](rust-toolchain.toml)
[![Tauri v2](https://img.shields.io/badge/Tauri-v2-24C8DB.svg)](https://tauri.app/)

LoadLoom cleanly separates the generation of traffic from the user interface:

- **`loadloom-core`** — a pure Rust library with **zero GUI dependencies**. It needs no window, no browser and no event loop, so it compiles, tests and runs inside CI, containers and headless servers.
- **`src-tauri`** — the native desktop shell (Tauri v2). It only bridges IPC; business logic lives elsewhere.
- **`src`** — a React 19 frontend responsible for rendering and passing parameters down.

The contract between them has a **single source of truth plus a mechanical guard**: if either side drifts, tests fail immediately.

> ### ⚠️ Read this first: authorized targets only
>
> Use this tool **only** against systems you own or have explicit written authorization to test. Applying pressure to systems without permission may violate laws and terms of service.
>
> The app asks you to confirm authorization before traffic starts, and **refuses to start if it is not confirmed** (it returns a `rejected` event). Every request additionally carries a `_ll={worker_id}-{request_id}` parameter so the target's operators can identify and trace your traffic in their access logs.

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
| **Concurrency ramp** | 1–32 workers; **dragging the slider while a run is active takes effect immediately**, no restart needed |
| **Global rate limit** | Token bucket; `0` means unlimited, capped at 4096 MiB/s, adjustable mid-run |
| **Auto-stop** | A byte threshold (GB) and a duration threshold (minutes) apply independently; `0` disables either one |
| **Live streaming** | Metrics are pushed every 250 ms; **the frontend never polls** and drops out-of-order frames via a monotonic `seq` |
| **Error classification** | Aggregated by code: `TIMEOUT` / `CONNECT_FAILED` / `BODY_STREAM` / `DECODE_ERROR` / `REDIRECT_ERROR` / `REQUEST_ERROR` / `UNKNOWN` |
| **Crash forensics** | A panic hook writes both to the log file and to the in-app "Run log" page — crashes no longer vanish silently |
| **Native window** | Tauri v2 with the system WebView2; **it listens on no ports** and spawns no browser process |
| **Tray resident** | The close button minimizes to the tray (it does not exit); quit from the tray menu |

---

## Quick start

### Option 1: download an installer (recommended)

Grab one of the three assets from [Releases](https://github.com/CheerSaltman/loadloom/releases/latest):

| File | Who it is for |
| --- | --- |
| `LoadLoom_0.4.0_x64-setup.exe` | **Most users.** NSIS installer with a Start Menu entry and an uninstaller |
| `LoadLoom_0.4.0_x64_en-US.msi` | Enterprise deployment via Group Policy or silent install (`msiexec /i`) |
| `LoadLoom_0.4.0_x64-portable.exe` | Portable, no installation — just double-click |

> Requirements: Windows 10/11 x64 plus the **WebView2 Runtime** (bundled with Windows 11 and recent Windows 10 builds; if it is missing, install the [Evergreen Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) once).

### Option 2: build from source

See [Building from source](#building-from-source).

---

## Tutorial: the desktop app

### 1. Launch

Open **LoadLoom** from the Start Menu, or double-click `LoadLoom_0.4.0_x64-portable.exe`.
On first launch it creates its log directory: `%LOCALAPPDATA%\LoadLoom\logs\loadloom.log`.

### 2. Confirm you are authorized

The UI has an **authorization checkbox**. It is not decorative:

- Not checked → pressing **Start** makes the engine return a `rejected` event and no request is ever sent.
- Checked → every request carries `_ll={worker_id}-{request_id}`, so the target's operators can recognize you in their access logs.

### 3. Fill in the target and parameters

| Parameter | How to fill it | Notes |
| --- | --- | --- |
| **Target URL** | The full address you want to test | Practise against **your own** service first |
| **Concurrency** | 1–32 | Start small. Over loopback the bottleneck is usually the machine running the test |
| **Rate limit** | MiB/s, `0` = unlimited | Set a cap to measure behaviour under a fixed bandwidth; leave `0` to find the ceiling |
| **Auto-stop · bytes** | GB, `0` = off | Independent of the duration threshold; either one triggers a stop |
| **Auto-stop · duration** | Minutes, `0` = off | Set a cap for long runs so you do not forget about them |

### 4. Start, then watch

Once you press **Start**:

- The **metrics panel** refreshes every 250 ms: current rate, in-flight requests, bytes sent, failure counts.
- The **line chart** keeps the last 120 samples; the full history arrives once in the handshake frame, then only deltas are pushed.
- The **Run log** page shows engine logs and error details, mirrored to the log file on disk.

While a run is active you can **change concurrency and rate limit on the fly** — no stop and restart.

### 5. Stop

- A threshold is reached → auto stop (a `autoStopped` event is pushed).
- You press **Stop** → a `stopped` event is pushed.
- You click the window's close button → it **only minimizes to the tray**; traffic keeps running. To really quit, use **Quit** in the tray context menu.

### 6. Troubleshooting

| Symptom | Where to look first |
| --- | --- |
| Failures keep climbing | Error codes on the **Run log** page: `CONNECT_FAILED` is usually network/port/certificate, `TIMEOUT` usually means the target is saturated or packets are dropped |
| Rate plateaus | Gains beyond 8 workers are typically small; first make sure the test machine or the target is not the bottleneck |
| UI seems unresponsive | Check the tray icon — the window may be minimized while the process is alive |
| The app exited unexpectedly | The log file will contain a panic record (guaranteed by the panic hook) |

---

## Tutorial: use it as a library (headless)

`loadloom-core` does not assume the calling thread has a Tokio runtime context. It captures a runtime handle at construction time (reusing an existing one, or creating its own), so you can **start traffic from any thread** — including a bare `main` thread with no runtime.

The smallest runnable example ships with the repo:

```bash
cargo run --locked --example headless_smoke -p loadloom-core
```

```text
引擎已创建（自带执行器，不依赖调用方线程的运行时上下文）
阶段=Running 在飞=4 已传字节=0 失败=0
...
已停止，最终阶段=Idle
```

That example lives in `examples/`, which means `cargo test` compiles it every time, so it **cannot rot**.

Regression testing in CI, containers or headless servers is just as direct:

```bash
cargo test --locked -p loadloom-core     # 18 tests, no graphical environment required
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

**Why cut it this way?**

1. **Testability.** Once a GUI leaks into the business layer, testing requires a window and an event loop. Today the core logic is `cargo test` in CI: 18 tests, seconds, no display, parallelizable. `headless_e2e.rs` can even run real traffic on a **bare thread with no Tokio runtime** — and that is exactly the regression test for the original crash bug.

2. **Replaceability.** The shell and the frontend are disposable. Want a CLI, a web service, or a daemon on a server instead? Rewrite the shell — `loadloom-core` does not change by a single line.

3. **Verifiable constraints.** Requirements like "no browser form factor" and "no GUI pollution" silently decay after a few iterations if they rely on human memory. Once layered, they become **assertions a machine can check**: the process listens on no ports (`scripts/verify_launch.ps1`), and the core dependency closure contains no GUI library (`scripts/audit_residue.ps1`).

**How does the seam between layers stay intact?** This is the most distinctive part of the project. `contract.rs` is the **only** place the contract is defined, and the frontend's `bindings.ts` is a hand-written mirror of it (because `tauri-specta` has not shipped a stable release). To stop the two from drifting, `contract_wire.rs` **parses `bindings.ts` as source text**, extracts every interface's field names, and compares them field by field against the JSON serde actually produces. Get one field name wrong and the test tells you exactly which field differs.

Responsibilities and dependency direction:

| Layer | Responsibility | Must not do |
| --- | --- | --- |
| `loadloom-core` | Traffic generation, rate limiting, metrics, contract definition | No windows, no IPC, no frontend types |
| `src-tauri` | IPC bridging, tray, window lifecycle, log persistence | No business rules, no data processing |
| `src` | Rendering, interaction, parameter submission | No polling, no guessing field names, no hard-coded contract |

---

## How it works: three key designs

<table>
<tr><th>Design</th><th>Approach</th></tr>
<tr>
<td><b>Stream, do not poll</b></td>
<td>Metrics travel through <code>tauri::ipc::Channel&lt;MetricsSnapshot&gt;</code> every 250 ms; logs and lifecycle use events.
Each frame carries a monotonic <code>seq</code>, and the frontend drops out-of-order or duplicate frames. The handshake frame carries the full
120-sample history, after which only the single <code>latest</code> point is pushed — re-sending 120 points every 250 ms would be pure waste.</td>
</tr>
<tr>
<td><b>Single source of truth + mechanical guard</b></td>
<td><code>contract.rs</code> on the Rust side is the only definition. The original plan was to generate TypeScript with
<code>tauri-specta</code>, but it only ships <code>2.0.0-rc</code> releases, so we took the fallback path: a hand-written mirror in
<code>src/bindings.ts</code>, guarded by the <code>contract_wire</code> test which <b>reads and parses bindings.ts</b> and compares it
field by field against the JSON serde really produces. Rename a field incorrectly and you get a plain-language failure.</td>
</tr>
<tr>
<td><b>Crashes must leave a trace</b></td>
<td>The panic hook writes to the log file and to the in-app log stream. Release builds deliberately keep <code>panic = "unwind"</code>
(rather than <code>abort</code>) and keep a <code>line-tables-only</code> line table, so a background-task panic is both captured and
attributable to a source line instead of taking the whole process down without a word.</td>
</tr>
</table>

A contract test failure looks like this:

```text
契约漂移：EngineLimits (Rust) 与 EngineLimits (bindings.ts) 字段不一致
  仅 Rust 有: ["maxWorkers"]
  仅 TS 有  : ["maxWorkersX"]
```

---

## Building from source

**Prerequisites**: Rust (the toolchain version is pinned by `rust-toolchain.toml`), Node.js ≥ 20, and the WebView2 Runtime on Windows.

```bash
git clone https://github.com/CheerSaltman/loadloom.git
cd loadloom

npm install
npm run typecheck          # tsc --noEmit, should print nothing
npm run build              # emits the frontend into dist/

cargo test --locked -p loadloom-core   # 18 tests, no graphical environment needed
npm run desktop:dev        # development: hot-reloading native window
npm run desktop:build      # packaging: produces exe + NSIS + MSI
```

Build outputs:

```text
target/release/loadloom-desktop.exe                       ← portable executable
target/release/bundle/nsis/LoadLoom_0.4.0_x64-setup.exe   ← NSIS installer
target/release/bundle/msi/LoadLoom_0.4.0_x64_en-US.msi    ← MSI installer
```

Development workflow, quality gates and the release procedure live in [docs/development.md](docs/development.md) (in Chinese).

---

## Project structure

```text
loadloom/
├── crates/loadloom-core/          # headless engine (library)
│   ├── src/
│   │   ├── lib.rs                # the only public entry point
│   │   ├── engine.rs             # the engine itself
│   │   └── contract.rs           # single source of truth for the contract
│   ├── tests/                    # integration tests (outside the published artifact)
│   │   ├── contract_wire.rs      # contract guard: parses bindings.ts field by field
│   │   └── headless_e2e.rs       # headless end-to-end: real traffic against a local fake origin
│   ├── benches/throughput.rs     # benchmark entry (outside the test suite)
│   └── examples/headless_smoke.rs# runnable example (compiled by cargo test, cannot rot)
├── src-tauri/                    # native desktop shell
│   ├── src/{lib,main,logging}.rs
│   ├── capabilities/default.json
│   ├── icons/
│   └── tauri.conf.json
├── src/                          # React frontend
│   ├── components/LineChart.tsx  # hand-drawn Canvas chart (no third-party, no CDN)
│   ├── hooks/useEngine.ts        # streaming subscription (seq dedup, no polling)
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
| [docs/architecture.md](docs/architecture.md) | Architectural decisions and their trade-offs (the story behind six key decisions) |
| [docs/development.md](docs/development.md) | Development environment, quality gates, local acceptance and the release procedure |
| [docs/conventions.md](docs/conventions.md) | Repository conventions, each traced back to official documentation |
| [CONTRIBUTING.md](CONTRIBUTING.md) | How to file issues and pull requests |
| [SECURITY.md](SECURITY.md) | Vulnerability reporting and secret-leak handling |
| [CHANGELOG.md](CHANGELOG.md) | Release history (Keep a Changelog format) |

> The `docs/` directory is currently written in Chinese only. Pull requests that translate it are welcome.

---

## License

[MIT](LICENSE) © 2026 CheerSaltman

The software is provided "as is", without warranty of any kind. Users are responsible for ensuring they are authorized to test their targets, and for all consequences of doing so.
