# Traffic Console · 高并发打流控制台

授权压力测试工具。**无头计算核心 + 原生桌面外壳** 的严格分层架构。

> ⚠️ 仅可用于**你拥有或已获得明确书面授权**的目标。启动时必须勾选授权确认，否则引擎直接拒绝。
> 每个请求都会带上 `_tc={worker_id}-{request_id}` 缓存穿透参数，目标方可从访问日志中识别并溯源。

---

## 一、架构：为什么这样分层

```
traffic-console/
├─ crates/traffic-core/          无头业务计算核心（零 GUI 依赖，可独立测试/嵌入）
│   ├─ src/contract.rs           ← 通信契约单一事实来源（serde + specta::Type）
│   ├─ src/engine.rs             引擎：限速器 / 指标 / worker / tick / 自动停止
│   └─ tests/
│       ├─ contract_wire.rs      契约守护：解析 bindings.ts 逐字段比对
│       └─ headless_e2e.rs       无界面端到端：本地伪 origin 跑真实打流
│
├─ src-tauri/                    原生桌面外壳（薄壳，业务为零）
│   ├─ src/lib.rs                仅 IPC 桥接：7 个 Command + 托盘 + 窗口生命周期
│   ├─ src/main.rs               入口（GUI 子系统，无控制台窗口）
│   └─ tauri.conf.json           窗口 / CSP / 打包目标
│
└─ src/                          React 19 前端
    ├─ bindings.ts               IPC 契约镜像 + 强类型封装
    ├─ hooks/useEngine.ts        推流订阅（seq 去重，零轮询）
    ├─ components/LineChart.tsx  自绘 Canvas 折线图（零第三方、零 CDN）
    └─ App.tsx                   界面
```

**关键点：`traffic-core` 不依赖任何 GUI/窗口/渲染库。** 它只依赖
`serde / serde_json / tokio / futures-util / reqwest / thiserror / specta`。
这意味着业务逻辑可以在完全无界面的环境下（CI、容器、无头服务器）编译、测试、运行。

---

## 二、前后端协同机制（零轮询）

| 数据类型 | 通道 | 频率 | 说明 |
|---|---|---|---|
| 运行指标 | `tauri::ipc::Channel<MetricsSnapshot>` | 250ms 主动推送 | 有序、低开销；**前端不做任何轮询** |
| 运行日志 | `app.emit("traffic://log")` | 事件驱动 | `LogEntry` |
| 生命周期 | `app.emit("traffic://run-event")` | 事件驱动 | `RunEvent`（started/stopped/autoStopped/rejected） |
| 参数下发 | `#[tauri::command]` | 按需 | `start_run` / `stop_run` / `set_live_config` |

两个防错设计：

1. **`seq` 单调序号** —— 每帧带自增序号，前端 `if (frame.seq < lastSeq) return` 丢弃乱序/重复帧。
2. **历史与增量分离** —— 握手帧（`get_snapshot(true)`）携带完整 120 点历史；流帧只带 `latest` 单点。
   否则每 250ms 重传 120 个点纯属浪费。

错误用**强类型外部标记枚举** `CoreError` 返回（`{ invalidInput: "..." }`），前端可精确判别分支，
不靠魔法字符串。

---

## 三、契约如何保证不漂移

原计划用 `tauri-specta` 自动生成 TypeScript，但该库目前只发布到 `2.0.0-rc.25`（非稳定版），
因此走**降级路径**：手写 `src/bindings.ts` 镜像 + 机械护栏。

护栏是真实存在的、可验证的 —— `crates/traffic-core/tests/contract_wire.rs` 会**读取并解析
`src/bindings.ts` 的源码**，提取每个 `interface` 的字段名与每个 `type` 的字面量，
再与 serde 实际产出的 JSON 逐一比对：

```bash
cargo test -p traffic-core --test contract_wire
```

任何一侧漂移都会立刻失败，并打印精确差异：

```
契约漂移：EngineLimits (Rust) 与 EngineLimits (bindings.ts) 字段不一致
  仅 Rust 有: ["maxWorkers"]
  仅 TS 有  : ["maxWorkersX"]
```

待 `tauri-specta` 发布稳定版后，用自动生成结果覆盖 `bindings.ts` 即可，**调用侧代码零改动**。

---

## 四、构建与运行

前置：Rust 工具链、Node（本项目使用便携版 Node v22）、Windows 上需 WebView2 Runtime。

```bash
npm install

npm run typecheck       # tsc --noEmit，应该零输出
npm run build           # 产出 dist/

# 全部测试（无需图形界面）
cargo test -p traffic-core

# 开发：热重载原生窗口
npm run desktop:dev

# 打包：产出原生安装包
npm run desktop:build
```

产物：

```
target/release/traffic-console-desktop.exe                       ← 免安装可执行文件
target/release/bundle/nsis/Traffic Console_0.3.0_x64-setup.exe   ← NSIS 安装包
target/release/bundle/msi/Traffic Console_0.3.0_x64_en-US.msi    ← MSI 安装包
```

---

## 五、验收标准（硬性约束的落地情况）

| 约束 | 落地方式 | 验证手段 |
|---|---|---|
| **禁止浏览器形态** | Tauri 原生窗口（WebView2 内嵌渲染） | 启动后窗口为真实 HWND；进程**不监听任何端口**，浏览器进程数不变 |
| **彻底清除 GUI 污染** | `traffic-core` 依赖闭包内无任何 GUI 库 | 依赖守卫扫描 + `headless_e2e` 在无窗口环境跑通真实打流 |
| **契约优先 / 类型对齐** | Rust 为单一事实来源，`contract_wire` 机械守护 | 变异测试：改字段名/加字段 → 测试立即失败 |
| **长耗时异步推流** | Channel + Event 主动广播 | 前端 `useEngine` 内无 `setInterval` / 轮询 |
| **原子化增量推进** | 每步后跑 `cargo test` + `npm run typecheck` | 全绿才进入下一步 |

---

## 六、引擎行为说明

- **并发**：1–32 个 worker。运行中拖动滑块即时生效（`set_live_config`）。
- **限速**：全局限令牌桶，0 表示不限速；上限 4096 MiB/s。运行中可调整。
- **自动停止**：流量阈值（GB）与时长阈值（分钟）独立生效，0 表示该项关闭。
- **抖动**：按 RFC3550 递推公式 `jitter += (|delay - last| - jitter) / 16`。
- **错误分类**：`TIMEOUT` / `CONNECT_FAILED` / `BODY_STREAM` / `DECODE_ERROR` /
  `REDIRECT_ERROR` / `REQUEST_ERROR` / `UNKNOWN`，按码聚合计数。
- **托盘**：关闭按钮收起到托盘（非退出）；真正退出走托盘菜单「退出」。

---

## 七、历史遗留物

重构前的浏览器形态实现（`axum` HTTP + WebSocket 服务、`webbrowser::open` 自动开浏览器、
`web/index.html` Chart.js 仪表盘、`egui` 桌面版）已全部删除，备份于 `C:\Goose\代码工程\_backup_phase0`。

---

## 九、项目结构（对齐 Cargo 官方 Package Layout）

```
traffic-console/
├── .github/
│   ├── workflows/ci.yml          # CI：fmt / clippy / test / typecheck / 桌面壳编译
│   └── dependabot.yml            # 依赖自动更新（cargo ×2 + npm + actions）
├── crates/
│   └── traffic-core/             # 无头引擎（library）
│       ├── src/
│       │   ├── lib.rs            # 唯一对外出口
│       │   ├── engine.rs         # 打流引擎本体
│       │   └── contract.rs       # 契约单一事实来源
│       ├── tests/                # 集成测试（发布物之外）
│       │   ├── contract_wire.rs
│       │   └── headless_e2e.rs
│       ├── benches/              # 基准（`cargo bench` 入口，不进测试套件）
│       │   └── throughput.rs
│       └── examples/             # 可运行示例（随 `cargo test` 一起编译，防止腐烂）
│           └── headless_smoke.rs
├── src-tauri/                    # 桌面壳（bin + lib）
│   ├── src/{lib,logging,main}.rs
│   ├── capabilities/default.json
│   ├── icons/
│   ├── build.rs
│   └── tauri.conf.json
├── src/                          # 前端（React + Tailwind）
│   ├── components/LineChart.tsx
│   ├── hooks/useEngine.ts
│   ├── lib/utils.ts
│   ├── App.tsx · bindings.ts · index.css · main.tsx
├── docs/architecture.md          # 架构决策与理由
├── scripts/                      # 运维/验收脚本（不参与构建）
│   ├── verify_launch.ps1         # 启动验收：真窗口、0 监听端口
│   ├── audit_residue.ps1         # 遗留依赖审计
│   ├── throughput_origin.js      # 外部参照 origin（排除本地 harness 干扰）
│   └── make_icon.mjs             # 图标生成
├── Cargo.toml · Cargo.lock       # 工作区清单；应用要提交锁文件
├── package.json · package-lock.json
├── index.html · vite.config.ts · tsconfig.json · tsconfig.node.json
├── README.md · CHANGELOG.md · CONTRIBUTING.md · SECURITY.md · LICENSE
└── rust-toolchain.toml · rustfmt.toml · .editorconfig · .gitattributes · .gitignore
```

## 十、规范对照（每条都能指回官方文档）

| 条目 | 依据 | 本仓库对应物 |
|---|---|---|
| 包布局 `src/tests/benches/examples` | Cargo Book · Package Layout | `crates/traffic-core/` 四分目录；基准不再混进 `tests/` |
| 清单元数据与工作区继承 | Cargo Book · Manifest / Workspaces | `[workspace.package]` + 各 crate 的 `xxx.workspace = true` |
| 统一 lint 策略 | Cargo Book · `[lints]` | `[workspace.lints.rust/clippy]` + 成员 `[lints] workspace = true` |
| 固定编译器 | rustup Book · Toolchain Overrides | `rust-toolchain.toml`（1.98.1 + rustfmt/clippy） |
| 格式化一致性 | rustfmt 配置文档 | `rustfmt.toml`，只用 stable 选项，CI 跑 `--check` |
| 缩进/换行统一 | EditorConfig 规范 | `.editorconfig` |
| 换行与二进制归属 | Git · gitattributes | `.gitattributes`（文本 LF、图标/安装包标 binary） |
| 变更记录 | Keep a Changelog 1.1.0 + SemVer 2.0.0 | `CHANGELOG.md` |
| 贡献流程 | GitHub · 社区健康文件 | `CONTRIBUTING.md` |
| 漏洞报告 | GitHub · Security policy | `SECURITY.md`（含密钥泄露处置流程） |
| 许可声明 | SPDX 标识 + Cargo `license` | `LICENSE`（MIT）+ `license.workspace = true` |
| 持续集成 | GitHub Actions 文档 | `.github/workflows/ci.yml` |
| 依赖更新 | Dependabot 配置文档 | `.github/dependabot.yml` |

**仓库地址已填实**：`Cargo.toml`、`CHANGELOG.md` 与本文件中的地址均为
`https://github.com/CheerSaltman/traffic-console`，无需再改。
`rust-version = "1.98"` 目前等于已固定的工具链版本（这是**实测**下限）；
若想放宽，请用 `cargo msrv --bisect` 测出真实 MSRV 再改，不要凭感觉写。

## 十一、本机验收命令

```bash
cargo fmt --all --check                    # 格式
cargo clippy --locked -p traffic-core --all-targets -- -D warnings
cargo test  --locked -p traffic-core       # 18 个测试，不需要图形环境
cargo run   --locked --example headless_smoke -p traffic-core
npm ci && npm run typecheck && npm run build
npm run desktop:build                      # 产出 exe + NSIS + MSI
```
