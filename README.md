# Traffic Console

**授权压力测试控制台 —— 无头计算核心 + 原生桌面外壳**

[![CI](https://github.com/CheerSaltman/traffic-console/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/CheerSaltman/traffic-console/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](rust-toolchain.toml)
[![Tauri v2](https://img.shields.io/badge/Tauri-v2-24C8DB.svg)](https://tauri.app/)

Traffic Console 把"打流计算"和"界面"彻底拆开：

- **`traffic-core`** —— 纯 Rust 库，**零 GUI 依赖**。不依赖窗口、浏览器、事件循环，可以在 CI、容器、无头服务器里编译、测试、运行。
- **`src-tauri`** —— 原生桌面外壳（Tauri v2），只做 IPC 桥接，业务逻辑为零。
- **`src`** —— React 19 前端，只做展示与参数下发。

两者之间的通信契约是**单一事实来源 + 机械护栏**：任何一侧漂移，测试立刻失败。

> ### ⚠️ 使用前必读：仅限授权目标
>
> 本工具只能用于**你拥有、或已获得目标方明确书面授权**的系统。未授权对他方系统施压可能违反法律与服务条款。
>
> 程序在启动打流前会要求勾选授权确认，**未勾选则引擎直接拒绝启动**（返回 `rejected` 事件）。此外，每个请求都会附带
> `_tc={worker_id}-{request_id}` 参数，方便目标方在访问日志中识别并溯源本次测试流量。

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
| **无头核心** | `traffic-core` 依赖闭包内没有任何 GUI / 渲染库，可在无窗口环境完整跑通真实打流 |
| **并发梯度** | 1–32 个 worker，**运行中拖动滑块即时生效**，无需停止重来 |
| **全局限速** | 令牌桶，`0` 表示不限速，上限 4096 MiB/s，运行中可调 |
| **自动停止** | 流量阈值（GB）与时长阈值（分钟）各自独立生效，填 `0` 表示该项关闭 |
| **实时推流** | 指标每 250 ms 主动推送，**前端零轮询**；带单调 `seq` 序号去重 |
| **错误分类** | 按码聚合：`TIMEOUT` / `CONNECT_FAILED` / `BODY_STREAM` / `DECODE_ERROR` / `REDIRECT_ERROR` / `REQUEST_ERROR` / `UNKNOWN` |
| **崩溃留痕** | panic hook 同时写入日志文件与界面「运行日志」页，崩溃不再无声消失 |
| **原生窗口** | Tauri v2 + 系统 WebView2，**不监听任何端口**，不拉起浏览器进程 |
| **托盘常驻** | 关闭按钮收起到托盘（非退出），真正退出走托盘菜单 |

---

## 快速开始

### 方式一：直接下载安装包（推荐）

到 [Releases](https://github.com/CheerSaltman/traffic-console/releases/latest) 下载，三选一：

| 文件 | 适合谁 |
| --- | --- |
| `Traffic Console_0.3.0_x64-setup.exe` | **大多数用户**。NSIS 安装包，带开始菜单项与卸载程序 |
| `Traffic Console_0.3.0_x64_en-US.msi` | 需要走企业组策略 / 静默部署（`msiexec /i`）的场景 |
| `traffic-console-desktop.exe` | 免安装绿色版，双击就跑 |

> 系统要求：Windows 10/11 x64，需要 **WebView2 Runtime**（Windows 11 与较新的 Windows 10 已自带；若提示缺失，装一次 [Evergreen Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) 即可）。

### 方式二：从源码构建

见 [从源码构建](#从源码构建)。

---

## 使用教程：桌面端

### 1. 启动

安装后从开始菜单打开 **Traffic Console**，或直接双击 `traffic-console-desktop.exe`。
首次启动会创建日志目录：`%LOCALAPPDATA%\TrafficConsole\logs\traffic-console.log`。

### 2. 确认你已获得授权

界面上有一项**授权确认勾选**。这一项不是装饰：

- 不勾选 → 点「开始」时引擎返回 `rejected` 事件，不会发出任何请求。
- 勾选后，发出的每个请求都会带上 `_tc={worker_id}-{request_id}`，对方运维可在访问日志里认出你。

### 3. 填写目标与参数

| 参数 | 怎么填 | 注意 |
| --- | --- | --- |
| **目标 URL** | 你要测的完整地址 | 建议先用你**自己**的服务试手 |
| **并发** | 1–32 | 从小往大加。回环地址上的瓶颈通常在测试机自身 |
| **限速** | MiB/s，`0` = 不限 | 想测"稳定带宽下的表现"就设上限；想测吞吐上限就填 `0` |
| **停止条件 · 流量** | GB，`0` = 关闭 | 与时长条件**互相独立**，任一满足即自动停止 |
| **停止条件 · 时长** | 分钟，`0` = 关闭 | 长跑建议设个上限，避免忘记 |

### 4. 开始，并观察

点「开始」后：

- **指标区**每 250 ms 刷新一次：实时速率、在飞请求数、已传字节、失败计数。
- **折线图**保留最近 120 个采样点，握手时一次性下发历史，之后只推增量。
- **运行日志页**展示引擎日志与错误明细，同一份内容也落盘到日志文件。

运行中你可以**随手改并发与限速**，立刻生效，不需要停止再启动。

### 5. 停止

- 到达你设的阈值 → 自动停止（会推 `autoStopped` 事件）。
- 手动点「停止」→ 推 `stopped` 事件。
- 点窗口右上角关闭 → **只是收起进托盘**，打流仍在继续。要真正退出，请用托盘右键菜单里的「退出」。

### 6. 排障

| 现象 | 先看哪里 |
| --- | --- |
| 失败数一直涨 | 「运行日志」页里的错误码：`CONNECT_FAILED` 多半是网络/端口/证书，`TIMEOUT` 多半是目标扛不住或丢包 |
| 速率上不去 | 并发加到 8 以上往往收益就很小了；先确认不是测试机 CPU 或目标端先到瓶颈 |
| 界面没反应 | 看托盘图标 —— 窗口可能被收起来了，进程还活着 |
| 程序异常退出 | 日志文件里一定有 panic 记录（panic hook 保证写盘） |

---

## 教程：把它当库用（无头模式）

`traffic-core` 不假设调用方线程处于 Tokio 运行时上下文中 —— 它会在构造时固化运行时句柄（有则复用、无则自建），所以你可以**从任意线程**启动打流，包括没有运行时的裸 `main` 线程。

最小可运行示例就在仓库里：

```bash
cargo run --locked --example headless_smoke -p traffic-core
```

```text
引擎已创建（自带执行器，不依赖调用方线程的运行时上下文）
阶段=Running 在飞=4 已传字节=0 失败=0
...
已停止，最终阶段=Idle
```

这个示例属于 `examples/`，会随 `cargo test` 一起被编译，因此**不会腐烂**。

在 CI、容器或无头服务器上做回归也很直接：

```bash
cargo test --locked -p traffic-core     # 18 个测试，全程不需要图形环境
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
│    7 个 Command、托盘、窗口生命周期、日志落盘               │
└───────────────────────────┬──────────────────────────────┘
                            │  Rust 函数调用（同进程）
┌───────────────────────────┴──────────────────────────────┐
│  crates/traffic-core/   无头引擎（零 GUI 依赖）            │
│    contract.rs   通信契约单一事实来源                       │
│    engine.rs     限速器 / worker / 指标 / tick / 自动停止   │
└──────────────────────────────────────────────────────────┘
```

**为什么要这么切？**

1. **可测试性**：GUI 一旦混进业务层，测试就得跑起窗口和事件循环。现在核心逻辑在 CI 里
   是 `cargo test`，秒级、无界面、可并行。
2. **可替换性**：外壳与前端都是可替换的。想换成 CLI、Web 服务或个人机上的守护进程，
   只需重写外壳 —— `traffic-core` 一行不动。
3. **约束可验证**：「禁止浏览器形态」「不许有 GUI 污染」这类要求如果不能被自动检查，
   就会在几次迭代后悄悄失效。分层之后，这两条都能用测试和依赖扫描守住。

各层职责与依赖方向：

| 层 | 职责 | 不该做的事 |
| --- | --- | --- |
| `traffic-core` | 打流计算、限速、指标、契约定义 | 不碰窗口、不碰 IPC、不碰前端类型 |
| `src-tauri` | IPC 桥接、托盘、窗口生命周期、日志落盘 | 不写业务规则，不做数据加工 |
| `src` | 渲染、交互、参数下发 | 不轮询、不猜字段、不硬编码契约 |

---

## 工作原理：三个关键设计

<table>
<tr><th>设计</th><th>做法</th></tr>
<tr>
<td><b>推流，不轮询</b></td>
<td>指标走 <code>tauri::ipc::Channel&lt;MetricsSnapshot&gt;</code> 每 250 ms 主动推送；日志与生命周期走事件。
每帧带单调自增 <code>seq</code>，前端丢弃乱序/重复帧。握手帧带完整 120 点历史，之后只推 <code>latest</code> 单点 ——
否则每 250 ms 重传 120 个点纯属浪费。</td>
</tr>
<tr>
<td><b>契约单一事实来源 + 机械护栏</b></td>
<td>Rust 侧的 <code>contract.rs</code> 是唯一定义处。原计划用 <code>tauri-specta</code> 自动生成 TypeScript，
但它目前只有 <code>2.0.0-rc</code> 版本，因此走降级路径：手写 <code>src/bindings.ts</code> 镜像，
再用 <code>contract_wire</code> 测试<b>读取并解析 bindings.ts 源码</b>，与 serde 实际产出的 JSON 逐字段比对。
改错字段名会立刻看到人话报错：</td>
</tr>
<tr>
<td><b>崩溃必须留痕</b></td>
<td>panic hook 同时写日志文件与界面日志流；release 构建刻意保留 <code>panic = "unwind"</code>
（而非 <code>abort</code>），并保留 <code>line-tables-only</code> 行号表，让一次后台任务的崩溃既能被捕获、也能定位到行，
而不是整个进程无声消失。</td>
</tr>
</table>

契约测试失败时的输出长这样：

```text
契约漂移：EngineLimits (Rust) 与 EngineLimits (bindings.ts) 字段不一致
  仅 Rust 有: ["maxWorkers"]
  仅 TS 有  : ["maxWorkersX"]
```

---

## 从源码构建

**前置**：Rust（工具链版本由 `rust-toolchain.toml` 固定）、Node.js ≥ 20、Windows 上需 WebView2 Runtime。

```bash
git clone https://github.com/CheerSaltman/traffic-console.git
cd traffic-console

npm install
npm run typecheck          # tsc --noEmit，应该零输出
npm run build              # 产出前端 dist/

cargo test --locked -p traffic-core   # 18 个测试，无需图形环境
npm run desktop:dev        # 开发模式：热重载原生窗口
npm run desktop:build      # 打包：产出 exe + NSIS + MSI
```

打包产物：

```text
target/release/traffic-console-desktop.exe                       ← 免安装可执行文件
target/release/bundle/nsis/Traffic Console_0.3.0_x64-setup.exe   ← NSIS 安装包
target/release/bundle/msi/Traffic Console_0.3.0_x64_en-US.msi    ← MSI 安装包
```

开发流程、质量门禁与发布步骤见 [docs/development.md](docs/development.md)。

---

## 项目结构

```text
traffic-console/
├── crates/traffic-core/          # 无头引擎（library）
│   ├── src/
│   │   ├── lib.rs                # 唯一对外出口
│   │   ├── engine.rs             # 打流引擎本体
│   │   └── contract.rs           # 通信契约单一事实来源
│   ├── tests/                    # 集成测试（不进发布物）
│   │   ├── contract_wire.rs      # 契约守护：解析 bindings.ts 逐字段比对
│   │   └── headless_e2e.rs       # 无界面端到端：本地伪 origin 跑真实打流
│   ├── benches/throughput.rs     # 基准入口（不进测试套件）
│   └── examples/headless_smoke.rs# 可运行示例（随 cargo test 编译，防止腐烂）
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
| [docs/architecture.md](docs/architecture.md) | 架构决策与取舍理由（含六个关键设计决定的来龙去脉） |
| [docs/development.md](docs/development.md) | 开发环境、质量门禁、本机验收与发布流程 |
| [docs/conventions.md](docs/conventions.md) | 仓库规范对照表（每条都能指回官方文档） |
| [CONTRIBUTING.md](CONTRIBUTING.md) | 如何提交 issue / PR |
| [SECURITY.md](SECURITY.md) | 漏洞报告与密钥泄露处置流程 |
| [CHANGELOG.md](CHANGELOG.md) | 版本变更记录（Keep a Changelog 格式） |

---

## 许可

[MIT](LICENSE) © 2026 CheerSaltman

本软件按"原样"提供，不附带任何担保。使用者须自行确保对测试目标拥有合法授权，并自行承担由此产生的一切后果。
