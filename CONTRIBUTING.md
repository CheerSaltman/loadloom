# 贡献指南

## 环境

- Rust：由 `rust-toolchain.toml` 固定，装了 rustup 就自动生效。
- Node：见 `package.json` 的 `engines` 约定；本项目在 Node 22 上验证通过。

## 本地构建与验证

```bash
# 1. 无头核心（不需要任何图形环境，CI 只跑这一层）
cargo test --locked -p loadloom-core

# 2. 前端类型与打包
npm ci
npm run typecheck
npm run build

# 3. 原生桌面应用（Windows 上产出 exe + NSIS + MSI）
npm run desktop:build

# 4. 可选：吞吐标定（会占满 CPU 数秒，默认被 #[ignore] 跳过）
cargo bench --locked -p loadloom-core -- --ignored --nocapture
```

提交前请确保 `cargo fmt --all` 与 `cargo clippy --all-targets` 无新增问题。

## 三条架构约束

改动前请先读 `docs/architecture.md`。

1. **`loadloom-core` 中不得出现任何 GUI / 窗口 / 浏览器依赖。**
   它是无头业务核心，需能在 CI 和无桌面环境的机器上运行。
   新增依赖前请确认不会引入 `windows` / `webview` / `eframe` 等。
2. **长耗时的数据流一律用异步推送，禁止轮询。**
   高频指标走 `tauri::ipc::Channel`，低频事件走 `app.emit`；
   前端不使用 `setInterval` 拉状态。
3. **契约改动必须三处同步**：`crates/loadloom-core/src/contract.rs`（单一事实来源）、
   `src/bindings.ts`（TS 镜像）、以及 `tests/contract_wire.rs`（机械护栏）。
   护栏会解析 TS 源码逐字段比对，只改一边会让 CI 失败。

## 测试要求

- 修 bug 时请附一个能复现该 bug 的回归测试；该测试应在还原修复后失败。
- 涉及时序的测试请轮询等待（带明确的截止时间），不要写死 `sleep(固定毫秒)`，
  否则会在快机器上假通过、慢机器上假失败。

## 提交信息

采用 [Conventional Commits](https://www.conventionalcommits.org/zh-hans/)：

```
<类型>(<范围>): <简短描述>

类型：feat | fix | perf | refactor | docs | test | build | chore | ci
范围：core | desktop | ui | contract | deps ...
```

示例：`fix(core): 同步 command 无运行时时不再 panic`

## 分支与 PR

- 分支名：`feat/xxx`、`fix/xxx`、`chore/xxx`。
- PR 请说明：改了什么、为什么、怎么验证的（贴命令与输出）。
- 涉及行为的改动请同步更新 `CHANGELOG.md` 的「未发布」段。
