# 变更日志

## 0.4.6

### 安全修复

- **停止之后在途请求仍在跑（中危）**：`stop()` 只拦得住「下一次请求」，已经发出去的那一次最长会拖到连接（10s）/ 读取（30s）超时 —— 用户按下停止后流量仍在继续。现在请求整体与停止信号赛跑，最迟 25ms 内放弃在途请求与响应体。
- **日志编码只覆盖了控制字符（低危）**：`sanitize` 只查 `char::is_control()`（Cc），漏掉 U+2028 / U+2029（Zl / Zp，多数渲染器视为换行）与 Cf（含双向覆写 U+202E，可颠倒整行的显示顺序）；同时 webview 的 `loadloom://log` 事件流直接发原始文本，是绕过编码的后门。现在两类字符一并编码，且落盘与上屏共用同一份编码（`encoding::encoded_entry`）。

### 发布链路

- **发布 job 不再同时持有写权限与构建脚本（中危）**：`release.yml` 拆成 `build`（`contents: read`，跑 npm ci / cargo build）与 `publish`（`contents: write`，不检出仓库，只下载 artifact 后上传），产物经 artifact 传递；所有 action 改为按 commit SHA 固定，不再使用可移动的 tag / 分支。

### 其它

- `start()` 的「已收敛」告警移到「已在运行」判定之后：一次注定被拒绝的启动不再在日志里留下噪音，掩盖真正生效的那次收敛。
- `set_live_config` 收敛限速值时同样明确告知（原先静默生效 —— 限速是用户对第三方的承诺，不该悄悄改）。

### 可观测性（日志与排障）

「无法定位、无法复现」本身就是一类缺陷。本版把日志从自由文本升级为结构化记录：

- **稳定事件码**：`SYS / CFG / RUN / NET` 四域、`域-三位序号`、永不复用 —— 用户报出一个码（如 `NET-002`）即可定位到唯一一条代码分支；
- **来源位置**：每条日志自带 `文件:行`（引擎用 `#[track_caller]` 自动取调用点，不手写常量）；
- **生效参数快照与停止汇总**：开始打流时记录并发 / 限速 / 上限 / HTTP 客户端状态；停止时记录时长、累计流量、成功失败计数与错误分布 —— 事后复现一次运行所需的第一手材料；
- **失败节流**：同一代次同一错误码只完整记录前 3 次，之后按 10 的幂补记，消息带 `run=` 与 `worker#`；一个不回包的源站不再在几秒内刷爆 4 MiB 主日志；
- **崩溃报告**：Rust panic 与前端未捕获异常各自生成 `crash-*.md`（环境头 + 完整回溯 + 最近日志），回溯同时逐行写入主日志；
- **前端日志页**：级别筛选、关键词搜索、自动滚动、复制诊断信息、导出诊断文件、打开日志目录，并显示事件码与来源位置；
- **环境头与构建号**：日志首行记录版本、构建哈希、平台与 PID。版本号不变的热更新下，构建号是区分两次发布的唯一标识。

### 网卡链路监测（新功能）

打流吞吐突然掉下去时，用户手里往往只有一句「忽然就慢了」。要区分源站限速、链路抖动、
本机网卡丢包，需要一条与吞吐同源、同一时钟的网卡侧记录。本版新增「网卡监测」页（Windows）：

- **能拿到什么**：链路通断时间线与中断时长、协商速率及其变化、收发速率与**利用率**、
  收发丢弃 / 错误（记录尖峰的开始、峰值、持续时长）、**发送队列长度**（`OutQLen`）；
- **拿不到什么**：网卡温度、收发缓冲区占用率、光模块功率 —— 这些属于驱动私有数据，
  消费级设备读不到。界面与报告里如实写明边界，**不留一个看起来像数据的 0**；
- **怎么采**：读 Windows 公开的 `GetIfTable2` 计数器，500 ms 采样、每块网卡保留 2 分钟
  滚动历史；速率由相邻帧差值换算，并能识别「计数器被清零」（重插 / 驱动重载）后归零重算，
  而不是画出一段负速率；
- **事件码**：新增 `NIC` 域（`NIC-001` … `NIC-025`）：链路断 / 通、协商速率变化、
  丢弃 / 错误 / 队列积压的开始与结束各自带码，可与打流日志放到同一条时间轴上对照；
- **订阅可靠性**：监测启动早于页面打开，因此「首帧发现清单」这类建立期日志先进入待发队列、
  订阅建立后补发 —— 否则它们会因为无人订阅而永久丢失；
- **平台降级**：非 Windows 明确上报 `NIC-003`（不支持），不返回空列表假装没有网卡；
- **实现边界**：FFI 隔离在独立 crate `crates/nicmon`（仓库内唯一允许 `unsafe` 的地方，
  自身声明 `unsafe_code = "deny"`）；判定逻辑是纯函数，测试可逐帧重放。

> 本次以「不改变版本号」的方式替换了 0.4.6 的发布产物（热更新）：源码与 Release 中的可执行文件均为最新提交，版本号仍是 0.4.6；日志环境头里的构建号可用来确认手里是哪一次构建。

本文件格式遵循 [Keep a Changelog 1.1.0](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本 2.0.0](https://semver.org/lang/zh-CN/)。

## 未发布

- 待 `tauri-specta` 发布稳定版后，用生成的类型替换手写的 `src/bindings.ts`。
- 将 `crates/loadloom-core/benches/throughput.rs` 升级为 criterion 基准，并接入历史基线对比。

## [0.4.5] - 2026-09-22

本版以**攻击者视角**重审了引擎的信任边界。前提很具体：`start_run` / `set_live_config`
是 IPC Command，载荷可以来自任意前端代码或调试工具，因此里面每个数字都按
**不可信输入**处理 —— 非法值明确拒绝，超范围值收敛后如实告知，绝不静默改写。

### 安全修复

- **畸形限速值让 worker 静默消失（高危）**：`rateMib: 1e-300` 是合法 JSON，`clamp`
  之后仍是极小正值，令牌桶算出 `deficit / rate ≈ 6e298` 秒并交给
  `Duration::from_secs_f64` —— 该函数在超出表示范围时直接 **panic**，worker 任务
  被静默终止（release 为 `panic = "unwind"`，进程不崩，只是少了一条打流通道）。
  现在等待时长统一由 `wait_from_seconds` 构造：非有限 / 非正数归零，超过 5 秒封顶，
  且令牌债务下探不超过一个桶容量。
- **「停止 → 立即开始」会留下未授权的多余 worker（高危）**：worker 的循环条件只看
  `stop` 标志，而 `start()` 会把它清回 false —— 一个正在退避睡眠里的旧 worker 就此
  「复活」，继续按上一轮的地址打流：实际并发超过用户授权值，流量也持续打向一个
  已被叫停的目标。现在引入**运行代次**（`Control::run`），`start()` / `stop()`
  各递增一次，旧代次的 worker 在下一次判定时立即退出。
- **自动停止守卫可被静默关闭（中危）**：`limit_gb` 大到 `limit_gb * GIB` 溢出成
  `inf` 时，`total_bytes as f64 >= inf` 永远为假 —— 用户以为设了安全上限，实际是
  无上限流量。阈值现在在信任边界收敛到可达范围（1 PiB / 约 1 年），且被修改时
  必定记一条 Warn 日志。
- **`NaN` 会把限速与自动停止静默关掉（中危）**：`f64::clamp` 对 `NaN` 原样返回，
  而 `NaN > 0.0` 为假，令牌桶于是把「限速」当成「不限速」；`NaN.max(0.0)` 返回 0，
  在契约里正表示「关闭自动停止」。现在非有限值一律拒绝（`InvalidInput`）并广播原因。
- **日志注入（CWE-117，中危）**：日志正文可能来自远端（`format!("请求失败：{error}")`），
  其中的换行足以伪造出额外的日志行。现在所有正文在落盘 / 上屏的唯一边界上做输出
  编码（换行、回车、制表符及控制字符转义），并截断到 2000 字符。
- **HTTP 客户端构建失败会 fail-open（中危）**：`build_client` 结尾的
  `Err(_) => reqwest::Client::new()` 会丢掉全部连接与读超时（任何一个不回包的源站
  都能把 worker 永久挂死），而 `Client::new()` 自身在构建失败时还会 panic。现在退到
  「只保留超时约束」的最小配置，并在第一次 `start()` 时把降级原因播报到日志流；连它也构造不出来时 `start()` 明确拒绝。

- **亚正规限速值让限速静默失效（低危）**：`rateMib` 小到 `1e-320` 时等待时长溢出为 `+inf`，被误当作「无需等待」，限速形同虚设。现在 `+inf` 封顶 5 秒，只有 `NaN` / 非正数归零。
- **无界的目标地址被反复广播（中危）**：`url` 原无长度上限，却进入每一帧快照（4 Hz）并随日志广播；现在在信任边界截断到 2048 字节。

### 修复

- 失败退避由固定 400ms 改为**指数增长 + 按 worker 抖动**（400ms → 3.2s 封顶），
  避免 32 个 worker 踩同一节拍重试形成惊群；退避切成 100ms 分片，`stop()` 不必等满
  整个退避才生效。
- 在途计数改为饱和加减：旧代次请求的迟到收尾不再可能把 `in_flight` 减成 `u32::MAX`。
- `User-Agent` 的版本号取自包元数据（旧值恒为 `0.4`）。
- `cache_busted_url` 兼容以 `?` / `&` 结尾的地址，不再产生 `?&_ll=` 这类空参数。

### 重构

- `rate.rs`：`Bucket::reserve` 成为纯计算，等待时长的构造与封顶收敛到唯一的
  `wait_from_seconds`；`set_rate` 对任意 `f64` 都是全函数。
- `engine.rs`：新增信任边界校验层（`sanitize_rate_mib` / `sanitize_limit` /
  `Engine::checked`）；`set_live` 返回 `Result` 而非静默忽略非法载荷；HTTP 客户端
  改为 `Option<Client>`，不可用时不静默裸奔。
- `metrics.rs`：在途计数收敛为 `request_started` / `request_finished` 两个饱和操作。

### 新增

- 新增 `docs/threat-model.md`：资产、信任边界、攻击者可控输入与已接受的风险。
- 新增 18 个测试（核心 26 → 42，桌面壳 7 → 9）：畸形速率不 panic 的回归、令牌债务
  封顶、运行代次（含「已停止的一代不得继续产生流量」的端到端连接计数断言）、
  自动停止上限可达性、非有限值拒绝、饱和在途计数、日志控制字符转义与截断。
- CI 显式声明 `permissions: contents: read`（最小权限），与 `release.yml` 的
  `contents: write` 形成对照。

### 变更

- `LiveConfigPatch` 的线格式不变；`set_live_config` 的失败路径改为返回强类型
  `CoreError`，前端既有的 `describeCoreError` 提示路径可直接复用。

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

[0.4.5]: https://github.com/CheerSaltman/loadloom/releases/tag/v0.4.5
[0.4.3]: https://github.com/CheerSaltman/loadloom/releases/tag/v0.4.3
[0.4.2]: https://github.com/CheerSaltman/loadloom/releases/tag/v0.4.2
[0.4.1]: https://github.com/CheerSaltman/loadloom/releases/tag/v0.4.1
[0.4.0]: https://github.com/CheerSaltman/loadloom/releases/tag/v0.4.0
[0.3.0]: https://github.com/CheerSaltman/loadloom/commit/712308638d0c14afd44958e47f735c39b8dfb812
