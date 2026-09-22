# 架构说明

## 总览

LoadLoom 由两部分组成：一个无头业务核心，和一个只负责画窗口的壳。业务逻辑全部位于核心层。

```
┌──────────────────────────┐        ┌──────────────────────────────┐
│  WebView（React + TW）    │        │  loadloom-core（无头引擎）      │
│  src/                     │        │  crates/loadloom-core/        │
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

## 分层原因

早期版本把引擎和界面混在一起，引擎隐含假设「调用方线程处于 Tokio 运行时上下文中」，
而该假设只在部分调用路径上成立，于是一个纯业务问题（同步命令里调用 `tokio::spawn`）
会表现为「点开始就闪退」。分层之后：

- 引擎的约束可以用无头测试覆盖（`cargo test -p loadloom-core`，CI 中不需要图形环境）。
- 壳的失败模式局限在桥接层，业务正确性不再依赖窗口能否打开。

## 关键设计决定

### 1. 执行器在构造时固化（`Executor`）

`Engine` 构造时确定自己的运行时来源：

- 调用方线程已在运行时上下文（例如壳里用异步 command）→ 复用该句柄；
- 不在（例如同步 command、裸线程、`examples/`）→ 自建一个运行时。

此后所有后台任务都走 `self.ex.spawn(...)`，从任意线程派发都安全。

### 2. 自建运行时不由自己的 worker 析构

Tokio 的 `Runtime::drop` 需要阻塞等待线程退出；若此刻正处于运行时上下文中
（典型情形：最后一个 `Arc<Engine>` 恰好被它自己的 worker 任务释放），
Tokio 会 panic。`OwnedRuntime` 把真正的释放动作挪到一条裸线程上执行。

### 3. 不限速 = 零锁

`RateLimiter` 用一个 `AtomicU64` 保存速率的位模式作为镜像。
不限速时 `acquire()` 在第一条判断就直接返回，不加锁；只有真正限速时才进入互斥锁路径。

### 4. 推流，不轮询

| 数据 | 通道 | 用途 |
| --- | --- | --- |
| 高频指标（250 ms） | `tauri::ipc::Channel<MetricsSnapshot>` | 单向、可背压、不占事件循环 |
| 低频事件（开始/停止/拒绝） | `app.emit("loadloom://run-event")` | 广播给多个监听者 |
| 运行日志 | `app.emit("loadloom://log")` | 与落盘日志同源 |
| 网卡指标（500 ms） | `tauri::ipc::Channel<NicSnapshot>` | 独立于打流的链路监测，打流停止后仍在继续 |

前端不使用 `setInterval` 拉状态，界面上的实时数据全部来自推送。

### 5. 契约单一事实来源

`crates/loadloom-core/src/contract.rs` 是唯一权威；`src/bindings.ts` 是它的手写镜像；
`tests/contract_wire.rs` 会解析 TS 源码并与 serde 的 JSON 输出逐字段比对
（字段名、可选性、枚举字面量、`CoreError` 的判别式顺序）。

选择手写而非自动生成的原因：`tauri-specta` 目前只有非稳定的 `2.0.0-rc.25`。
待其发布稳定版后可替换，调用侧无需改动。

### 6. 崩溃必须留痕

`src-tauri/src/logging.rs` 安装 panic hook：崩溃信息既写入
`%LOCALAPPDATA%\LoadLoom\logs\loadloom.log`（4 MB 轮转），
也推进引擎日志流，界面「运行日志」页当场可见。
配套 `get_log_path` / `open_log_dir` 两个命令。
release profile 因此保留 `panic = "unwind"` 与 `debug = "line-tables-only"`。

### 7. 信任边界在核心层，不在前端

`start_run` / `set_live_config` 是 IPC Command，调用方可以是任意前端代码或调试工具，
因此前端校验只算用户体验，**不作数**。所有校验都在 `loadloom-core` 里完成：

- 非法值（`NaN`、非有限数）→ 明确拒绝，并通过日志与 `run_event` 广播原因；
- 超范围但语义合法的值 → 收敛到范围内，且如实告知被收敛过；
- 任何情况下都不 panic —— 畸形载荷最多让本次请求失败。

细节见 `docs/threat-model.md`。

### 8. worker 只认「运行代次」

一次运行里的 worker 只认同一个代次（`Control::run`）。`start()` 与 `stop()` 各自让
代次递增一次，worker 在每次循环与每片退避等待后都校验它。

这样做的原因是一个真实竞态：如果循环条件只看 `stop` 标志，那么「停止 → 立即开始」
时，一个正在退避睡眠里的旧 worker 会错过那次 stop（醒来时标志已被 `start()` 清成
false），于是作为**额外的** worker 继续打流 —— 实际并发突破用户授权值，流量也继续
打向一个已经叫停的目标。

### 9. 网卡监测：FFI 关进独立 crate，分析保持纯函数

网卡链路数据来自 Windows 的 `GetIfTable2`（`iphlpapi`），这是整个仓库里唯一需要
`unsafe` 的地方。因此把它单独拆成 `crates/nicmon`：

- `nicmon` 只做「取数 + 解析」：FFI、结构体布局、位域与错误码，对外导出与平台无关的
  `RawAdapter`。工作区级 `unsafe_code = "forbid"` 保持不动，`nicmon` 自己声明
  `deny`，并在唯一需要 `unsafe` 的模块顶部显式白名单 —— 新增 `unsafe` 无法悄悄溜进来；
- `loadloom-core::nic` 只做「分析」：差值、利用率、链路跳变、丢弃 / 错误 / 队列积压
  的起止判定，全部是不读时钟、不碰 IO 的纯函数，采集闭包由调用方注入；
- 于是 Windows 真机取数与测试里的构造序列走的是**同一条判定路径**：链路瞬断、
  计数器归零、尖峰收尾这些场景可以逐帧重放，不需要真拔网线；
- 能力边界写进契约（`CAPABILITY_NOTE`）：温度、缓冲区占用率、光功率拿不到就不提供
  字段，也不填 0。取不到的数据宁可没有，也不编一个看起来像真的。

采样走独立的 500 ms 定时任务，与打流的 250 ms 指标流互不阻塞：停止打流后链路监测
仍在继续 —— 恰恰是「停止瞬间的链路状态」这类证据最有价值。

## 目录约定

```
loadloom/
├── crates/nicmon/           # 网卡计数器采集（唯一允许 FFI 的隔离层）
├── crates/loadloom-core/    # 无头引擎（库）
│   ├── src/
│   │   └── nic/             # 网卡分析：纯函数判定与事件流
│   ├── tests/               # 集成测试（contract_wire, headless_e2e）
│   ├── benches/             # 基准（throughput）
│   └── examples/            # 可运行示例（headless_smoke）
├── src-tauri/               # 桌面壳（bin + lib）
├── src/                     # 前端（React + Tailwind）
├── scripts/                 # 运维/验收脚本
└── docs/                    # 架构与说明
```

目录遵循 Cargo Book 的 Package Layout：`src/` `tests/` `benches/` `examples/` 各司其职。
