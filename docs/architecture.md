# 架构说明

## 一句话

**一个无头业务核心，加一个只负责画窗口的壳。** 业务逻辑一行都不许待在壳里。

```
┌──────────────────────────┐        ┌──────────────────────────────┐
│  WebView（React + TW）    │        │  traffic-core（无头引擎）      │
│  src/                     │        │  crates/traffic-core/        │
│   · 只做展示与交互          │        │   · 打流、限速、计量、日志      │
│   · 不轮询、不算业务        │        │   · 零 GUI 依赖，可独立测试     │
└───────────▲──────────────┘        └──────────────▲───────────────┘
            │  IPC（命令 + Channel + 事件）           │
            └──────────────┬───────────────────────┘
                           │
              ┌────────────┴─────────────┐
              │  src-tauri（壳）          │
              │  仅做桥接，不含业务        │
              └──────────────────────────┘
```

## 为什么这么切

事故驱动。早期版本把引擎和界面揉在一起，结果一个纯业务问题（同步命令里调
`tokio::spawn`）演变成了「点开始就闪退」——因为引擎隐含假设「调用方线程一定
处于 Tokio 运行时上下文中」，而这条假设只在某些调用路径上成立。

切分之后：

- 引擎的每一条约束都能用**无头测试**钉住（`cargo test -p traffic-core`，
  CI 里不需要任何图形环境）。
- 壳的失败模式被压缩到「桥接层」这一小块，业务正确性不再依赖窗口能不能开。

## 关键设计决定

### 1. 执行器在构造时固化（`Executor`）

`Engine` 构造时确定自己的运行时来源：

- 调用方线程**已在**运行时上下文（例如壳里用异步 command）→ 复用该句柄；
- **不在**（例如同步 command、裸线程、`examples/`）→ 自建一个运行时。

此后所有后台任务都走 `self.ex.spawn(...)`，从任意线程派发都安全。
这条正是闪退事故的正面修复 —— 引擎不再对调用方做任何隐含假设。

### 2. 自建运行时不许被自己的 worker 析构

Tokio 的 `Runtime::drop` 需要阻塞等待线程退出；若此刻正处于运行时上下文中
（典型情形：最后一个 `Arc<Engine>` 恰好被它自己的 worker 任务释放），
Tokio 会 panic。`OwnedRuntime` 把真正的释放动作挪到一条裸线程上执行。

### 3. 不限速 = 零锁

`RateLimiter` 用一个 `AtomicU64` 保存速率的位模式作为镜像。
不限速（常态）时 `acquire()` 在第一条判断就直接返回，不加锁、不等待、不唤醒；
只有真正限速时才进入互斥锁路径。

### 4. 推流，不轮询

| 数据 | 通道 | 理由 |
| --- | --- | --- |
| 高频指标（250 ms） | `tauri::ipc::Channel<MetricsSnapshot>` | 单向、可背压、不占事件循环 |
| 低频事件（开始/停止/拒绝） | `app.emit("traffic://run-event")` | 需要广播给多个监听者 |
| 运行日志 | `app.emit("traffic://log")` | 与落盘日志同源，界面可即时取证 |

前端不得用 `setInterval` 拉状态。用户可见的「实时」全部来自推送。

### 5. 契约单一事实来源

`crates/traffic-core/src/contract.rs` 是唯一权威；`src/bindings.ts` 是它的手写镜像；
`tests/contract_wire.rs` 会解析 TS 源码并与 serde 的 JSON 输出逐字段比对
（字段名、可选性、枚举字面量、`CoreError` 的判别式顺序）。

为什么手写而不是自动生成：`tauri-specta` 目前只有非稳定的 `2.0.0-rc.25`。
引入 RC 版本会让「契约」这件事本身变成不稳定项，所以走手写 + 机械护栏。
待其发布稳定版后可无痛替换，调用侧零改动。

### 6. 崩溃必须留痕

`src-tauri/src/logging.rs` 安装 panic hook：崩溃信息既写入
`%LOCALAPPDATA%\TrafficConsole\logs\traffic-console.log`（4 MB 轮转），
也推进引擎日志流，界面「运行日志」页当场可见。
配套 `get_log_path` / `open_log_dir` 两个命令，用户可自行取证。
release profile 因此保留 `panic = "unwind"` 与 `debug = "line-tables-only"`。

## 目录约定

```
traffic-console/
├── crates/traffic-core/     # 无头引擎（库）
│   ├── src/
│   ├── tests/               # 集成测试（contract_wire, headless_e2e）
│   ├── benches/             # 基准（throughput）
│   └── examples/            # 可运行示例（headless_smoke）
├── src-tauri/               # 桌面壳（bin + lib）
├── src/                     # 前端（React + Tailwind）
├── scripts/                 # 运维/验收脚本
└── docs/                    # 架构与说明
```

遵循 Cargo Book 的 Package Layout：`src/` `tests/` `benches/` `examples/`
各司其职 —— 基准放 `tests/` 会被误当成集成测试跑，示例放文档里则永远不会被编译。
