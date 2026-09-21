# 变更日志

本文件格式遵循 [Keep a Changelog 1.1.0](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本 2.0.0](https://semver.org/lang/zh-CN/)。

## 未发布

- 待 `tauri-specta` 发布稳定版后，用生成的类型替换手写的 `src/bindings.ts`。
- 将 `crates/loadloom-core/benches/throughput.rs` 升级为 criterion 基准，并接入历史基线对比。

## [0.4.0] - 2026-09-22

项目更名为 LoadLoom（原 Traffic Console）。

### 变更（破坏性）

- 下载产物统一改名为 `LoadLoom_<版本>_x64-*`：
  - `LoadLoom_0.4.0_x64-setup.exe`（NSIS 安装包）
  - `LoadLoom_0.4.0_x64_en-US.msi`（MSI 安装包）
  - `LoadLoom_0.4.0_x64-portable.exe`（免安装版）
- 应用标识由 `com.agentic.trafficconsole` 改为 `com.agentic.loadloom`，安装新版不会覆盖旧版本，需先卸载旧版。
- 日志目录由 `%LOCALAPPDATA%\TrafficConsole\logs` 迁移到 `%LOCALAPPDATA%\LoadLoom\logs\loadloom.log`。
- 窗口标题与界面品牌统一为 LoadLoom。

### 新增

- `.github/workflows/release.yml`：推送 `v*` tag 时自动构建并上传 exe + NSIS + MSI。

### 修复

- README 中桌面壳 Command 数量由 7 更正为 9。

### 文档

- README 重写为面向使用者的介绍与教程；规范对照、质量门禁、发布流程移至 `docs/`。
- 新增英文版 `README.en.md`。

## [0.3.0] - 2026-09-21

架构重构：由浏览器形态转为无头核心 + 原生桌面外壳。

### 新增

- `crates/loadloom-core`：无头打流引擎，不依赖窗口、浏览器或事件循环。
- `src-tauri`：Tauri v2 桌面外壳，只做 IPC 桥接。
- 运行日志：落盘到 `%LOCALAPPDATA%\TrafficConsole\logs\loadloom.log`，并推送到界面「运行日志」页。
- panic hook：崩溃写入日志文件。
- `open_log_dir` / `get_log_path` 命令。
- 契约守护测试 `crates/loadloom-core/tests/contract_wire.rs`。

### 修复

- 点「开始」立即闪退：`start_run` 改由 `Executor` 派发，不再依赖调用方线程的 Tokio 运行时上下文。
- 不限速路径上的全局锁：`RateLimiter::acquire` 在不限速时走 `rate_bits` 原子镜像的零锁快路径。
- 运行析构 panic：自建运行时改为 `OwnedRuntime`，释放动作挪到裸线程。
- release profile：`panic = "abort"` 改为 `unwind`，保留行号表。

### 移除

- 历史遗留的 GUI 与本地服务依赖（egui / eframe / webbrowser / axum）。
- 界面上的技术标签。

[0.4.0]: https://github.com/CheerSaltman/loadloom/releases/tag/v0.4.0
[0.3.0]: https://github.com/CheerSaltman/loadloom/commit/712308638d0c14afd44958e47f735c39b8dfb812
