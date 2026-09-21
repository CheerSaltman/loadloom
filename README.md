# LoadLoom

**授权压力测试控制台 —— 无头计算核心 + 原生桌面外壳**

**简体中文** | [English](README.en.md)

[![CI](https://github.com/CheerSaltman/loadloom/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/CheerSaltman/loadloom/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](rust-toolchain.toml)
[![Tauri v2](https://img.shields.io/badge/Tauri-v2-24C8DB.svg)](https://tauri.app/)

LoadLoom 分为三层：

- **`loadloom-core`** —— 纯 Rust 库，零 GUI 依赖，不依赖窗口、浏览器或事件循环，可在 CI、容器、无头服务器中编译、测试、运行。
- **`src-tauri`** —— 原生桌面外壳（Tauri v2），只做 IPC 桥接。
- **`src`** —— React 19 前端，只做展示与参数下发。

两端的通信契约定义在 `contract.rs`，前端 `src/bindings.ts` 是它的手写镜像，由测试逐字段校验。

> ### ⚠️ 使用前必读：仅限授权目标
>
> 本工具只能用于**你拥有、或已获得目标方明确书面授权**的系统。未授权对他方系统施压可能违反法律与服务条款。
>
> 程序在启动打流前会要求勾选授权确认，未勾选则引擎直接拒绝启动（返回 `rejected` 事件）。每个请求都会附带
> `_ll={worker_id}-{request_id}` 参数，方便目标方在访问日志中识别并溯源本次测试流量。

---

## 目录

- [核心特性](#核心特性)
- [快速开始](#快速开始)
- [使用教程：桌面端](#使用教程桌面端)
- [教程：把它当库用（无头模式）](#教程把它当库用无头模式)
- [结构拆解：三层架构](#结构拆解三层架构)
- [工作原理：三个关键设计](#工作原理三个关键设计)
- [从源码构建](#从源码构建)
- [项目结构](#项目结构)
- [更多文档](#更多文档)

---

## 核心特性

| 能力 | 说明 |
| --- | --- |
| **无头核心** | `loadloom-core` 依赖闭包内没有任何 GUI / 渲染库，可在无窗口环境完整跑通真实打流 |
| **并发梯度** | 1–32 个 worker，运行中拖动滑块即时生效，无需停止重来 |
| **全局限速** | 令牌桶，`0` 表示不限速，上限 4096 MiB/s，运行中可调 |
| **自动停止** | 流量阈值（GB）与时长阈值（分钟）各自独立生效，填 `0` 表示该项关闭 |
| **实时推流** | 指标每 250 ms 推送，前端零轮询；带单调 `seq` 序号去重 |
| **错误分类** | 按码聚合：`TIMEOUT` / `CONNECT_FAILED` / `BODY_STREAM` / `DECODE_ERROR` / `REDIRECT_ERROR` / `REQUEST_ERROR` / `UNKNOWN` |
| **崩溃留痕** | panic hook 同时写入日志文件与界面「运行日志」页 |
| **原生窗口** | Tauri v2 + 系统 WebView2，不监听任何端口，不拉起浏览器进程 |
| **托盘常驻** | 关闭按钮收起到托盘（非退出），真正退出走托盘菜单 |

---

## 快速开始

### 方式一：直接下载安装包（推荐）

到 [Releases](https://github.com/CheerSaltman/loadloom/releases/latest) 下载，三选一：

| 文件 | 适合谁 |
| --- | --- |
| `LoadLoom_0.4.0_x64-setup.exe` | **大多数用户**。NSIS 安装包，带开始菜单项与卸载程序 |
| `LoadLoom_0.4.0_x64_en-US.msi` | 需要走企业组策略 / 静默部署（`msiexec /i`）的场景 |
| `LoadLoom_0.4.0_x64-portable.exe` | 免安装绿色版，双击就跑 |

> 系统要求：Windows 10/11 x64，需要 **WebView2 Runtime**（Windows 11 与较新的 Windows 10 已自带；若提示缺失，装一次 [Evergreen Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) 即可）。

### 方式二：从源码构建

见 [从源码构建](#从源码构建)。

---

## 使用教程：桌面端

### 1. 启动

安装后从开始菜单打开 **LoadLoom**，或直接双击 `LoadLoom_0.4.0_x64-portable.exe`。
首次启动会创建日志目录：`%LOCALAPPDATA%\LoadLoom\logs\loadloom.log`。

### 2. 确认你已获得授权

界面上有一项授权确认勾选：

- 不勾选 → 点「开始」时引擎返回 `rejected` 事件，不会发出任何请求。
- 勾选后，发出的每个请求都会带上 `_ll={worker_id}-{request_id}`，对方运维可在访问日志里认出你。

### 3. 填写目标与参数

| 参数 | 怎么填 | 注意 |
| --- | --- | --- |
| **目标 URL** | 你要测的完整地址 | 建议先用你**自己**的服务试手 |
| **并发** | 1–32 | 从小往大加。回环地址上的瓶颈通常在测试机自身 |
| **限速** | MiB/s，`0` = 不限 | 想测"稳定带宽下的表现"就设上限；想测吞吐上限就填 `0` |
| **停止条件 · 流量** | GB，`0` = 关闭 | 与时长条件互相独立，任一满足即自动停止 |
| **停止条件 · 时长** | 分钟，`0` = 关闭 | 长跑建议设个上限，避免忘记 |

### 4. 开始，并观察

点「开始」后：

- **指标区**每 250 ms 刷新一次：实时速率、在飞请求数、已传字节、失败计数。
- **折线图**保留最近 120 个采样点，握手时一次性下发历史，之后只推增量。
- **运行日志页**展示引擎日志与错误明细，同一份内容也落盘到日志文件。

运行中可以随手改并发与限速，立刻生效。

### 5. 停止

- 到达你设的阈值 → 自动停止（会推 `autoStopped` 事件）。
- 手动点「停止」→ 推 `stopped` 事件。
- 点窗口右上角关闭 → 只是收起进托盘，打流仍在继续。要真正退出，请用托盘右键菜单里的「退出」。

### 6. 排障

| 现象 | 先看哪里 |
| --- | --- |
| 失败数一直涨 | 「运行日志」页里的错误码：`CONNECT_FAILED` 多半是网络/端口/证书，`TIMEOUT` 多半是目标扛不住或丢包 |
| 速率上不去 | 并发加到 8 以上往往收益就很小；先确认不是测试机 CPU 或目标端先到瓶颈 |
| 界面没反应 | 看托盘图标 —— 窗口可能被收起来了，进程还活着 |
| 程序异常退出 | 日志文件里有 panic 记录（panic hook 保证写盘） |

---

## 教程：把它当库用（无头模式）

`loadloom-core` 会在构造时固化运行时句柄（有则复用、无则自建），因此可以从任意线程启动打流，包括没有运行时的裸 `main` 线程。

最小可运行示例：

```bash
cargo run --locked --example headless_smoke -p loadloom-core
```

```text
引擎已创建（自带执行器，不依赖调用方线程的运行时上下文）
阶段=Running 在飞=4 已传字节=0 失败=0
...
已停止，最终阶段=Idle
```

该示例位于 `examples/`，随 `cargo test` 一并编译。

在 CI、容器或无头服务器上做回归：

```bash
cargo test --locked -p loadloom-core     # 18 个测试，无需图形环境
```

---

## 结构拆解：三层架构

```
┌──────────────────────────────────────────────────────────┐
│  src/            React 19 + Tailwind（只做展示 / 下参数）  │
│    bindings.ts     IPC 契约镜像（手写 + 机械护栏）         │
│    hooks/useEngine 推流订阅，seq 去重，零轮询               │
└───────────────────────────┬──────────────────────────────┘
                            │  Tauri IPC（Channel / Event / Command）
┌───────────────────────────┴──────────────────────────────┐
│  src-tauri/      原生外壳（薄壳，业务为零）                │
│    9 个 Command、托盘、窗口生命周期、日志落盘               │
└───────────────────────────┬──────────────────────────────┘
                            │  Rust 函数调用（同进程）
┌───────────────────────────┴──────────────────────────────┐
│  crates/loadloom-core/   无头引擎（零 GUI 依赖）            │
│    contract.rs   通信契约单一事实来源                       │
│    engine.rs     限速器 / worker / 指标 / tick / 自动停止   │
└──────────────────────────────────────────────────────────┘
```

各层职责与依赖方向：

| 层 | 职责 | 不该做的事 |
| --- | --- | --- |
| `loadloom-core` | 打流计算、限速、指标、契约定义 | 不碰窗口、不碰 IPC、不碰前端类型 |
| `src-tauri` | IPC 桥接、托盘、窗口生命周期、日志落盘 | 不写业务规则，不做数据加工 |
| `src` | 渲染、交互、参数下发 | 不轮询、不猜字段、不硬编码契约 |

---

## 工作原理：三个关键设计

- **推流，不轮询**：指标走 `tauri::ipc::Channel<MetricsSnapshot>` 每 250 ms 推送；日志与生命周期走事件。每帧带单调自增 `seq`，前端丢弃乱序/重复帧。握手帧带完整 120 点历史，之后只推 `latest` 单点。
- **契约单一事实来源 + 机械护栏**：`contract.rs` 是唯一定义处；`tauri-specta` 目前只有 `2.0.0-rc` 版本，因此手写 `src/bindings.ts` 镜像，再由 `contract_wire` 测试解析该文件源码，与 serde 产出的 JSON 逐字段比对。
- **崩溃必须留痕**：panic hook 同时写日志文件与界面日志流；release 构建保留 `panic = "unwind"` 与 `line-tables-only` 行号表。

契约测试失败时的输出：

```text
契约漂移：EngineLimits (Rust) 与 EngineLimits (bindings.ts) 字段不一致
  仅 Rust 有: ["maxWorkers"]
  仅 TS 有  : ["maxWorkersX"]
```

---

## 从源码构建

**前置**：Rust（工具链版本由 `rust-toolchain.toml` 固定）、Node.js ≥ 20、Windows 上需 WebView2 Runtime。

```bash
git clone https://github.com/CheerSaltman/loadloom.git
cd loadloom

npm install
npm run typecheck          # tsc --noEmit
npm run build              # 产出前端 dist/

cargo test --locked -p loadloom-core   # 18 个测试，无需图形环境
npm run desktop:dev        # 开发模式：热重载原生窗口
npm run desktop:build      # 打包：产出 exe + NSIS + MSI
```

打包产物：

```text
target/release/loadloom-desktop.exe                       ← 免安装可执行文件
target/release/bundle/nsis/LoadLoom_0.4.0_x64-setup.exe   ← NSIS 安装包
target/release/bundle/msi/LoadLoom_0.4.0_x64_en-US.msi    ← MSI 安装包
```

开发流程、质量门禁与发布步骤见 [docs/development.md](docs/development.md)。

---

## 项目结构

```text
loadloom/
├── crates/loadloom-core/          # 无头引擎（library）
│   ├── src/
│   │   ├── lib.rs                # 唯一对外出口
│   │   ├── engine.rs             # 打流引擎本体
│   │   └── contract.rs           # 通信契约单一事实来源
│   ├── tests/                    # 集成测试（不进发布物）
│   │   ├── contract_wire.rs      # 契约守护：解析 bindings.ts 逐字段比对
│   │   └── headless_e2e.rs       # 无界面端到端：本地伪 origin 跑真实打流
│   ├── benches/throughput.rs     # 基准入口（不进测试套件）
│   └── examples/headless_smoke.rs# 可运行示例（随 cargo test 编译）
├── src-tauri/                    # 原生桌面外壳
│   ├── src/{lib,main,logging}.rs
│   ├── capabilities/default.json
│   ├── icons/
│   └── tauri.conf.json
├── src/                          # React 前端
│   ├── components/LineChart.tsx  # 自绘 Canvas 折线图（零第三方、零 CDN）
│   ├── hooks/useEngine.ts        # 推流订阅（seq 去重，零轮询）
│   ├── bindings.ts               # IPC 契约镜像
│   └── App.tsx
├── docs/                         # 架构、约定、开发流程
├── scripts/                      # 运维 / 验收脚本（不参与构建）
├── .github/                      # CI 与 Dependabot
└── Cargo.toml                    # 工作区清单（含 lints 与元数据继承）
```

---

## 更多文档

| 文档 | 内容 |
| --- | --- |
| [docs/architecture.md](docs/architecture.md) | 架构决策与取舍理由 |
| [docs/development.md](docs/development.md) | 开发环境、质量门禁、本机验收与发布流程 |
| [docs/conventions.md](docs/conventions.md) | 仓库规范对照表 |
| [CONTRIBUTING.md](CONTRIBUTING.md) | 如何提交 issue / PR |
| [SECURITY.md](SECURITY.md) | 漏洞报告流程 |
| [CHANGELOG.md](CHANGELOG.md) | 版本变更记录（Keep a Changelog 格式） |

---

## 许可

[MIT](LICENSE) © 2026 CheerSaltman

本软件按"原样"提供，不附带任何担保。使用者须自行确保对测试目标拥有合法授权，并自行承担由此产生的一切后果。
