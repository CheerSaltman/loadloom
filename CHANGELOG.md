# 变更日志

本文件格式遵循 [Keep a Changelog 1.1.0](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本 2.0.0](https://semver.org/lang/zh-CN/)。

## 计划中

（尚未发布的改动会先记在这里，发版时移到对应版本号下。）

- tauri-specta 发布稳定版后，用自动生成的类型替换手写的 `src/bindings.ts`
  （调用侧零改动，见 `docs/architecture.md`）。
- 把 `crates/loadloom-core/benches/throughput.rs` 升级为 criterion 基准，
  并接入历史基线对比。

## [0.4.0] - 2026-09-22

品牌更名版：项目由 **Traffic Console** 正式更名为 **LoadLoom**（`loom` 取「织机」之意——
把多个 worker 织成一股流量）。

### 变更（破坏性）

- **下载产物全部改名**，Release 资产统一为 `LoadLoom_<版本>_x64-*`：
  - `LoadLoom_0.4.0_x64-setup.exe` —— NSIS 安装包，推荐大多数用户使用
  - `LoadLoom_0.4.0_x64_en-US.msi` —— 静默部署（`msiexec /i`）
  - `LoadLoom_0.4.0_x64-portable.exe` —— 免安装绿色版
- **需要重新安装**：应用标识由 `com.agentic.trafficconsole` 改为 `com.agentic.loadloom`，
  安装包的升级标识随之改变，旧版本不会被自动覆盖或卸载——请先卸载旧版再装新版。
- **日志目录迁移**：`%LOCALAPPDATA%\TrafficConsole\logs` → `%LOCALAPPDATA%\LoadLoom\logs\loadloom.log`
  （旧目录不再写入，可自行删除）。
- 窗口标题与界面品牌统一为「LoadLoom · 高并发打流控制台」。

### 新增

- `.github/workflows/release.yml`：推送 `v*` tag 时自动运行质量门禁、构建 exe + NSIS + MSI
  并上传到同名 Release（只补产物，不覆盖 Release 说明）。

### 变更

- README 中桌面壳 Command 数量由 7 更正为 9，与实际 `#[tauri::command]` 数量对齐。

### 文档

- README 重写为面向使用者的「介绍 + 教程 + 结构拆解」；原先堆在 README 里的本机性内容归位：
  规范对照表移到 `docs/conventions.md`，质量门禁、本机验收、发布流程与构建注意事项移到
  `docs/development.md`。
- 新增英文版 `README.en.md`，与中文版结构一一对应、顶部互相链接；两份需同步维护。
- 修正 `docs/conventions.md` 中的工具链记录（`1.98.1` → `stable`）。

### 开发者注意（内部标识符迁移）

- crate `traffic-core` → `loadloom-core`；桌面壳包 `traffic-console-desktop` → `loadloom-desktop`
- 请求溯源参数 `_tc=` → `_ll=`；User-Agent → `loadloom/0.4 (+authorized-load-test)`
- IPC 事件 `traffic://log` / `traffic://run-event` → `loadloom://log` / `loadloom://run-event`
- npm 包名 `traffic-console` → `loadloom`；CSS 类前缀 `tc-` → `ll-`
- 本地构建产物路径不变（仍是 `target/release/loadloom-desktop.exe`），仅上传到 Release 时改名

## [0.3.0] - 2026-09-21

这一版是架构重构版：从「浏览器形态」整体转为**无头业务核心 + 原生桌面窗口**。

### 新增

- `crates/loadloom-core`：无头打流引擎，零 GUI 依赖，可在没有窗口、没有浏览器、
  没有事件循环的环境下运行与测试。
- `src-tauri`：Tauri v2 原生桌面壳，只做 IPC 桥接，不含业务逻辑。
- 运行日志：落盘到 `%LOCALAPPDATA%\LoadLoom\logs\loadloom.log`
  （4 MB 自动轮转），并同时推送到界面「运行日志」页。
- panic hook：捕获崩溃写入日志文件并推进日志流，崩溃不再是「无声闪退」。
- `open_log_dir` / `get_log_path` 两个命令，便于用户自行取证。
- 契约守护测试 `crates/loadloom-core/tests/contract_wire.rs`：解析
  `src/bindings.ts` 源码，与 Rust 侧 serde 输出逐字段比对。

### 修复

- **点「开始」立即闪退**（致命）：`start_run` 是同步 command，Tauri 在事件循环
  主线程执行它，该线程没有 Tokio 运行时上下文，`tokio::spawn` panic；
  叠加 `panic = "abort"` 导致整个进程当场消失。
  修法：引入 `Executor`，在构造时固化运行时（有则复用句柄、无则自建），
  此后从任意线程派发后台任务都安全。
- **不限速路径上的全局锁**：`RateLimiter::acquire` 对每个数据块都要抢一次互斥锁，
  即使不限速也如此；现用 `rate_bits` 原子镜像做零锁快路径。
- **运行析构 panic**：自建 Tokio 运行时被它自己的 worker 线程 drop 时 panic
  （`Cannot drop a runtime in a context where blocking is not allowed`）；
  改为 `OwnedRuntime` 包装，把释放动作挪到裸线程。
- 修正 release profile：`panic = "abort"` → `unwind`，`strip` → `debug = "line-tables-only"`，
  让单个后台任务 panic 不再拖垮 UI，且崩溃回溯能定位到代码行。

### 移除

- 历史遗留的 GUI 与本地服务依赖（egui / eframe / webbrowser / axum）：
  仓库不再包含任何界面渲染框架或内置 HTTP 服务代码。
- 界面上的技术标签（引擎 / 壳 / 推流说明）：不把实现细节暴露给使用者。

[0.4.0]: https://github.com/CheerSaltman/loadloom/releases/tag/v0.4.0
[0.3.0]: https://github.com/CheerSaltman/loadloom/commit/712308638d0c14afd44958e47f735c39b8dfb812
