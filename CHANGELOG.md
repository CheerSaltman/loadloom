# 变更日志

本文件格式遵循 [Keep a Changelog 1.1.0](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本 2.0.0](https://semver.org/lang/zh-CN/)。

## 未发布

- 待 `tauri-specta` 发布稳定版后，用生成的类型替换手写的 `src/bindings.ts`。
- 将 `crates/loadloom-core/benches/throughput.rs` 升级为 criterion 基准，并接入历史基线对比。

## [0.4.3] - 2026-09-22

### 修复

- **日志轮转在 Windows 上从第二次起静默失效**：`fs::rename` 覆盖已存在的
  `loadloom.prev.log` 会失败（`ERROR_ALREADY_EXISTS`），旧实现没先删旧文件，
  于是日志一旦超过 4 MB 就不再归档、无限增长。现在先删旧代再改名，并如实上报结果。
- **轮转只在启动时判断**：旧实现仅在会话开始时看一次体积，长会话里日志同样会无限增长。
  现在按累计写入字节在**写入路径**上触发。
- **日志设施自身故障会让应用起不来**：`open_sink` 结尾的 `expect("无法创建日志文件")`
  与「永不 panic」的模块约定自相矛盾 —— 磁盘满或权限不足本该降级为「只上屏」，
  却直接让桌面壳启动失败。现在主目录不可写就退到临时目录，再不可写就只上屏。
- **界面上的日志路径可能不是真的**：日志降级到临时目录后，`get_log_path` 仍上报
  `%LOCALAPPDATA%` 下的路径。现在返回**实际生效**的文件，无法落盘时返回 `None`
  （前端应显示「未落盘」）；`open_log_dir` 打开实际生效的目录。

### 重构

- `src-tauri/src/logging.rs` 按职责重排：纯函数区（时间戳换算、行格式化）不再接触文件系统，
  `Sink` 成为唯一接触文件系统的类型，负责句柄、体积轮转与降级。
- 日志级别不再以裸字符串在模块间传递：`write` 改收 `LogLevel`，定宽标签由单测锁定，
  调用方不再需要自己拼 `"INFO "`。
- 降级路径明确为「不丢日志优先」：归档失败时继续写入原文件（宁可超限也不丢），
  句柄失效后下次写入自动重开。

### 新增

- `logging` 模块补上 7 个单元测试：UTC 时间戳与闰日换算、级别标签定宽、行格式、
  两代轮转覆盖、启动时归档超限旧文件、坏句柄降级不 panic。测试总数 26 → 33。
- CI 的 Windows `desktop` job 增加 `cargo test -p loadloom-desktop --lib`：
  日志落在桌面壳里，这条 Windows 专属缺陷不可能被跑在 Linux 上的核心任务发现。

### 变更

- `src-tauri/tauri.conf.json` 不再写死 `version`，改由 Tauri 回落到 `Cargo.toml` 的版本号：
  发布时只需改一处，安装包文件名不会与 tag 脱节。
- `release.yml` 在编译前校验 tag 与 `Cargo.toml` 版本一致，避免到上传产物阶段才报错。
- 修正 `benches/throughput.rs` 头部注释里的运行命令（`--test` 改为 `--bench`，与目录归属一致）。

## [0.4.2] - 2026-09-22

### 修复

- 补上 0.4.1 拆分后失真的模块文档：`engine.rs` 的头注释不再声称自己实现限速与指标，改为说明职责已委托给子模块；`rate.rs`、`metrics.rs` 补上模块级说明。

### 新增

- `rate.rs` 与 `metrics.rs` 补齐单元测试：速率原子镜像、桶容量随速率缩放与 128 KiB 下限、错误分布排序与 12 条截断、RFC3550 抖动递推、重置语义。测试总数 18 → 24。

## [0.4.1] - 2026-09-22

### 重构

- 拆分 `loadloom-core` 的 `engine.rs`（998 行）为三个职责单一的模块：`rate.rs`（令牌桶限速器）、`metrics.rs`（指标与抖动统计）、`engine.rs`（引擎门面）。纯搬移，运行时行为不变，18 个测试全部保持通过；但 `RateLimiter` 的可见性由 `pub` 收窄为 `pub(crate)`，`loadloom_core::engine::RateLimiter` 路径不再对外可达。
- 新增模块的对外可见性统一收敛为 `pub(crate)`，收紧 crate 公共 API 面。

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

[0.4.3]: https://github.com/CheerSaltman/loadloom/releases/tag/v0.4.3
[0.4.2]: https://github.com/CheerSaltman/loadloom/releases/tag/v0.4.2
[0.4.1]: https://github.com/CheerSaltman/loadloom/releases/tag/v0.4.1
[0.4.0]: https://github.com/CheerSaltman/loadloom/releases/tag/v0.4.0
[0.3.0]: https://github.com/CheerSaltman/loadloom/commit/712308638d0c14afd44958e47f735c39b8dfb812
