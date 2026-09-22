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
- **`sidecars/loadloom-pt`** —— Go BitTorrent 引擎，负责 DHT / PEX / uTP / TCP、RAM-only piece 存储和 Peer 健康分析。

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
| **局域网分片仿真** | 对本机或局域网多个 HTTP Peer 轮转发起 256 KiB–2 MiB Range 分片请求；拒绝域名和公网 IP |
| **真实公网 PT** | 接受 magnet 或公开 `.torrent` URL，启用 DHT / PEX / uTP / TCP 和 8–500 Peer 连接；payload 只进 RAM，piece 校验后立即丢弃 |
| **Peer 健康分析** | 统计握手、半开连接、有效/卡死/死 Peer、连接淘汰、Tracker 成败、坏块和浪费流量；30 秒协议 keepalive |
| **全局限速** | 令牌桶，`0` 表示不限速，上限 4096 MiB/s，运行中可调 |
| **压力保护** | 监测请求失败率与持续首包时延，达到阈值自动熔断；界面显示正常 / 预警 / 已熔断及原因 |
| **自动停止** | 流量阈值（GB）与时长阈值（分钟）各自独立生效，填 `0` 表示该项关闭 |
| **实时推流** | 指标每 250 ms 推送，前端零轮询；带单调 `seq` 序号去重 |
| **错误分类** | 按码聚合：`TIMEOUT` / `CONNECT_FAILED` / `BODY_STREAM` / `DECODE_ERROR` / `REDIRECT_ERROR` / `REQUEST_ERROR` / `UNKNOWN` |
| **崩溃留痕** | panic hook 同时写入日志文件与界面「运行日志」页 |
| **原生窗口** | Tauri v2 + 系统 WebView2，不监听任何端口，不拉起浏览器进程 |
| **托盘常驻** | 关闭按钮收起到托盘（非退出），真正退出走托盘菜单 |
| **网卡链路监测** | 链路通断 / 协商速率 / 收发利用率 / 丢弃 / 错误 / 发送队列，与打流日志同一条时间轴（Windows；拿不到的指标如实说明，不编造） |

---

## 快速开始

### 方式一：直接下载安装包（推荐）

到 [Releases](https://github.com/CheerSaltman/loadloom/releases/latest) 下载，三选一：

| 文件 | 适合谁 |
| --- | --- |
| `LoadLoom_<版本>_x64-setup.exe` | **大多数用户**。NSIS 安装包，带开始菜单项与卸载程序 |
| `LoadLoom_<版本>_x64_en-US.msi` | 需要走企业组策略 / 静默部署（`msiexec /i`）的场景 |
| `LoadLoom_<版本>_x64-portable.exe` | 免安装绿色版，双击就跑 |

> 系统要求：Windows 10/11 x64，需要 **WebView2 Runtime**（Windows 11 与较新的 Windows 10 已自带；若提示缺失，装一次 [Evergreen Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) 即可）。

### 方式二：从源码构建

见 [从源码构建](#从源码构建)。

---

## 使用教程：桌面端

### 1. 启动

安装后从开始菜单打开 **LoadLoom**，或直接双击 `LoadLoom_<版本>_x64-portable.exe`。
首次启动会创建日志目录：`%LOCALAPPDATA%\LoadLoom\logs\loadloom.log`。

### 2. 确认你已获得授权

界面上有一项授权确认勾选：

- 不勾选 → 点「开始」时引擎返回 `rejected` 事件，不会发出任何请求。
- 勾选后，发出的每个请求都会带上 `_ll={worker_id}-{request_id}`，对方运维可在访问日志里认出你。

### 3. 填写目标与参数

| 参数 | 怎么填 | 注意 |
| --- | --- | --- |
| **目标 URL** | 你要测的完整地址 | 建议先用你**自己**的服务试手 |
| **流量模型** | HTTP 下载、局域网分片仿真或真实公网 PT | 公网 PT 建议只使用 Ubuntu / Debian 等合法公开发行版 |
| **并发** | 1–32 | 从小往大加。回环地址上的瓶颈通常在测试机自身 |
| **渐进升压** | 秒，`0` = 关闭 | 从 1 worker 逐步升到目标并发，减少瞬时冲击 |
| **限速** | MiB/s，`0` = 不限 | 想测"稳定带宽下的表现"就设上限；想测吞吐上限就填 `0` |
| **失败率熔断** | 百分比，`0` = 关闭 | 至少采集 20 次请求后生效；达到阈值自动停止 |
| **时延熔断** | 毫秒，`0` = 关闭 | 首包时延连续超标约 1 秒后自动停止，短暂尖峰只预警 |
| **停止条件 · 流量** | GB，`0` = 关闭 | 与时长条件互相独立，任一满足即自动停止 |
| **停止条件 · 时长** | 分钟，`0` = 关闭 | 长跑建议设个上限，避免忘记 |

### 4. 开始，并观察

点「开始」后：

- **指标区**每 250 ms 刷新一次：实时速率、在飞请求数、已传字节、失败计数。
- **压力保护卡片**显示当前等级和判定原因；预警只记录日志，达到熔断条件才自动停止。
- **折线图**保留最近 120 个采样点，握手时一次性下发历史，之后只推增量。
- **运行日志页**展示引擎日志与错误明细，同一份内容也落盘到日志文件。

运行中可以随手改并发与限速，立刻生效。

### 5. 停止

- 到达你设的阈值 → 自动停止（会推 `autoStopped` 事件）。
- 手动点「停止」→ 推 `stopped` 事件。
- 点窗口右上角关闭 → 只是收起进托盘，打流仍在继续。要真正退出，请用托盘右键菜单里的「退出」。

### 6. 网卡监测（定位「忽然就慢」）

第三个标签页是**网卡监测**（Windows）。它与打流共用同一条时间轴，用来回答一个问题：
慢下来是源站的问题，还是本机链路的问题。每 500 ms 采一帧，保留 2 分钟曲线。

| 能看到的 | 说明 |
| --- | --- |
| 链路通断 | 断开记 `NIC-010`，恢复时附中断时长（`NIC-011`） |
| 协商速率 | 例如 1 Gbps 掉到 100 Mbps（`NIC-012`）—— 网线 / 端口 / 无线的锅一眼可见 |
| 收发利用率 | 实时速率 ÷ 协商速率，看链路还剩多少余量 |
| 丢弃 / 错误 | 按「一次事件」记录起止，结束时报峰值与持续时长（`NIC-020` ~ `NIC-023`） |
| 发送队列 | `OutQLen` 持续大于 16 包即记积压（`NIC-024` / `NIC-025`）：驱动来不及发 |
| 事件时间线 | 所有变化按时间排列，可直接对照打流日志里的速率曲线 |

**看不到的**（驱动私有数据，消费级设备读不到）：网卡温度、收发缓冲区占用率、光模块功率。
页面顶部会如实写出这条边界 —— 拿不到就写拿不到，不用 0 冒充数据。

监测范围默认勾选物理网卡，可手动增删（虚拟网卡 / 隧道 / 回环各有分类标签）。
监测独立于打流：不打流时它也在跑，停止打流后仍然继续。

### 7. 单机无线网卡 + 路由器的真实 PT 自测

选择「真实公网 PT Swarm」，粘贴合法公开资源的 magnet 或 `.torrent` URL。默认最多 180 个 Peer、512 MiB RAM；Go sidecar 不创建下载文件，不上传，piece 通过哈希校验后立即释放内存。界面同时显示有效速率、线速字节、RAM 峰值、Peer 半开/卡死/死链比例和 Tracker 状态。

只有一台电脑、一张无线网卡和一个路由器时，结果必然是**无线网卡 + 路由器 + 宽带 + 公网 swarm 的端到端上限**，无法从单端证明路由器绝不是瓶颈。应结合「网卡监测」判断：RX 利用率接近 100% 且无错误/丢弃，才说明无线链路较可能到顶；利用率低而死链率、半开连接或 Tracker 错误高，则更可能是 swarm、宽带或路由器 NAT 限制。

### 8. 排障

| 现象 | 先看哪里 |
| --- | --- |
| 失败数一直涨 | 「运行日志」页里的错误码：`CONNECT_FAILED` 多半是网络/端口/证书，`TIMEOUT` 多半是目标扛不住或丢包 |
| PT 模式拒绝启动 | 目标或额外 Peer 不是字面量局域网地址；为防 DNS 重绑定，此模式不接受域名 |
| 压力保护自动停止 | 看压力卡片与 `RUN-007` 日志：区分失败率熔断和持续首包时延熔断 |
| 速率上不去 | 并发加到 8 以上往往收益就很小；先确认不是测试机 CPU 或目标端先到瓶颈 |
| 速率突然掉底 / 断流 | 「网卡监测」页：链路是否断开、协商速率是否掉档（如 1 Gbps → 100 Mbps）、有无丢弃 / 错误尖峰 |
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
cargo test --locked -p loadloom-core     # 无头核心全部测试，无需图形环境
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

**前置**：Rust（工具链版本由 `rust-toolchain.toml` 固定）、Node.js ≥ 20、Go ≥ 1.24，以及 Windows WebView2 Runtime。

```bash
git clone https://github.com/CheerSaltman/loadloom.git
cd loadloom

npm install
npm run typecheck          # tsc --noEmit
npm run build              # 产出前端 dist/
npm run pt:test            # Go PT sidecar 测试
npm run pt:build           # 构建 RAM-only BT sidecar

cargo test --locked -p loadloom-core   # 无头核心全部测试，无需图形环境
npm run desktop:dev        # 开发模式：热重载原生窗口
npm run desktop:build      # 打包：产出 exe + NSIS + MSI
```

打包产物：

```text
target/release/loadloom-desktop.exe                       ← 免安装可执行文件
target/release/bundle/nsis/LoadLoom_<版本>_x64-setup.exe   ← NSIS 安装包
target/release/bundle/msi/LoadLoom_<版本>_x64_en-US.msi    ← MSI 安装包
```

开发流程、质量门禁与发布步骤见 [docs/development.md](docs/development.md)。

---

## 项目结构

```text
loadloom/
├── crates/nicmon/                 # 网卡计数器采集（唯一 FFI 隔离层，Windows）
├── crates/loadloom-core/          # 无头引擎（library）
│   ├── src/
│   │   ├── lib.rs                # 唯一对外出口
│   │   ├── engine.rs             # 打流引擎本体
│   │   ├── nic/                  # 网卡分析：纯函数判定 + 事件流
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
├── sidecars/loadloom-pt/         # Go：真实 BT swarm + RAM 校验后丢弃 + Peer 诊断
├── src/                          # React 前端
│   ├── components/LineChart.tsx  # 自绘 Canvas 折线图（零第三方、零 CDN）
│   ├── components/NicPanel.tsx   # 网卡监测页（利用率条 / 曲线 / 事件时间线）
│   ├── hooks/useEngine.ts        # 推流订阅（seq 去重，零轮询）
│   ├── hooks/useNic.ts           # 网卡推流订阅（与打流互不阻塞）
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
