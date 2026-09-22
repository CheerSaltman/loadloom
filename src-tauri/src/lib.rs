//! Tauri v2 桌面壳：**只做 IPC 桥接，不含任何业务逻辑**。
//!
//! 职责边界：
//! * 创建原生窗口 / 托盘 / 主题跟随；
//! * 把 [`loadloom_core::Engine`] 的能力暴露为强类型 Command；
//! * 把引擎的广播通道转成 Event / Channel 推流（前端零轮询）；
//! * 把日志落盘并捕获 panic（见 [`logging`]）—— 保证任何异常都可事后分析。

mod logging;
mod pt;

use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State};

use loadloom_core::{
    CoreError, Engine, EngineLimits, LiveConfigPatch, LogEntry, MetricsSnapshot, NicAdapterDto,
    NicMonitor, NicSnapshot, PtSnapshot, PtStartRequest, StartRunRequest,
};

/// 日志流事件名。
pub const EVENT_LOG: &str = "loadloom://log";
/// 生命周期事件名。
pub const EVENT_RUN: &str = "loadloom://run-event";

/// 应用状态：仅持有一个引擎句柄。
pub struct AppState {
    pub engine: Arc<Engine>,
    /// 网卡链路监测器。与引擎并列而非嵌套：它有自己的采样周期与订阅者，
    /// 打流停不停都不影响它继续记录链路状态。
    pub nic: Arc<NicMonitor>,
    /// 真实 BitTorrent Go sidecar。与 HTTP 引擎互斥运行，共用网卡监测。
    pub pt: Arc<pt::PtManager>,
}

// ---------------------------------------------------------------------------
// Commands（请求-响应）
// ---------------------------------------------------------------------------

/// 读取引擎上限，供前端渲染范围控件。
#[tauri::command]
fn get_limits() -> EngineLimits {
    Engine::limits()
}

/// 拉取一次快照。`withHistory = true` 用于启动时补齐曲线。
#[tauri::command]
fn get_snapshot(with_history: bool, state: State<'_, AppState>) -> MetricsSnapshot {
    state.engine.snapshot(with_history)
}

/// 启动打流。失败时返回强类型 `CoreError`，前端可精确判别分支。
#[tauri::command]
fn start_run(request: StartRunRequest, state: State<'_, AppState>) -> Result<(), CoreError> {
    if state.pt.is_running() {
        return Err(CoreError::AlreadyRunning(
            "真实 PT 压测正在运行，请先停止".to_owned(),
        ));
    }
    state.engine.start(request)
}

/// 停止打流。
#[tauri::command]
fn stop_run(reason: Option<String>, state: State<'_, AppState>) {
    let reason = reason
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "已由用户手动停止".to_owned());
    state.engine.stop(&reason);
}

/// 运行中动态调整并发 / 限速。非法载荷（`NaN`、越界值）返回强类型错误 ——
/// 前端已经有统一的 `describeCoreError` 提示路径，这里不再静默丢弃。
#[tauri::command]
fn set_live_config(patch: LiveConfigPatch, state: State<'_, AppState>) -> Result<(), CoreError> {
    state.engine.set_live(patch)
}

/// 建立指标推流通道：后端主动推送，前端**不需要轮询**。
#[tauri::command]
fn subscribe_metrics(channel: Channel<MetricsSnapshot>, state: State<'_, AppState>) {
    let mut receiver = state.engine.subscribe_metrics();
    tauri::async_runtime::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(frame) => {
                    if channel.send(frame).is_err() {
                        break;
                    }
                }
                // 消费过慢导致丢帧：跳过旧帧继续，而不是中断推流。
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
}

/// 启动真实公网 BitTorrent swarm 压测。Go sidecar 只使用 RAM piece 存储。
#[tauri::command]
fn start_pt(request: PtStartRequest, state: State<'_, AppState>) -> Result<(), CoreError> {
    if state.engine.is_running() {
        return Err(CoreError::AlreadyRunning(
            "HTTP 打流正在运行，请先停止".to_owned(),
        ));
    }
    state.pt.start(request)
}

#[tauri::command]
fn stop_pt(state: State<'_, AppState>) {
    state.pt.stop();
}

#[tauri::command]
fn get_pt_snapshot(state: State<'_, AppState>) -> PtSnapshot {
    state.pt.snapshot()
}

#[tauri::command]
fn subscribe_pt(channel: Channel<PtSnapshot>, state: State<'_, AppState>) {
    let mut receiver = state.pt.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(frame) => {
                    if channel.send(frame).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
}

/// 退出应用。
#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

// ---------------------------------------------------------------------------
// Commands（网卡监测）
// ---------------------------------------------------------------------------

/// 全部接口（含虚拟 / 隧道），供界面勾选监测范围。
#[tauri::command]
fn list_nic_adapters(state: State<'_, AppState>) -> Vec<NicAdapterDto> {
    state.nic.adapters()
}

/// 网卡监测快照（`withHistory = true` 时附带每块网卡的曲线）。
#[tauri::command]
fn get_nic_snapshot(with_history: bool, state: State<'_, AppState>) -> NicSnapshot {
    state.nic.snapshot(with_history)
}

/// 设置监测范围。空数组 = 默认（全部物理网卡）。
/// 非法载荷（超长标识 / 数量超限）返回强类型错误，由前端统一提示。
#[tauri::command]
fn set_nic_selection(ids: Vec<String>, state: State<'_, AppState>) -> Result<(), CoreError> {
    state.nic.select(ids)
}

/// 生成网卡报告（可直接粘贴给维护者），并留下一行 `NIC-007` 以便与日志对齐。
#[tauri::command]
fn get_nic_report(state: State<'_, AppState>) -> String {
    state.nic.report()
}

/// 建立网卡监测推流通道：后端每 500ms 主动推一帧，前端零轮询。
#[tauri::command]
fn subscribe_nic(channel: Channel<NicSnapshot>, state: State<'_, AppState>) {
    let mut receiver = state.nic.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(frame) => {
                    if channel.send(frame).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
}

/// 返回**实际生效**的日志文件路径（供界面展示，便于用户自行排查）。
///
/// 主目录不可写时日志会降级到临时目录，这里返回的就是降级后的那一个；
/// 连临时目录都写不了（`None`）时前端应显示「未落盘」，而不是一个并不存在的路径。
#[tauri::command]
fn get_log_path() -> Option<String> {
    logging::active_log_file().map(|path| path.display().to_string())
}

/// 生成诊断文本（环境头 + 落盘位置 + 崩溃报告清单 + 主日志末尾若干行）。
///
/// 前端「复制诊断信息 / 导出诊断」直接消费它：用户报障时只需要一份文本，
/// 不必在日志目录里手动挑文件、拼现场。
#[tauri::command]
fn get_diagnostics(tail_lines: Option<u32>, state: State<'_, AppState>) -> String {
    let tail = tail_lines
        .map(|value| value.clamp(50, 5_000) as usize)
        .unwrap_or(logging::DIAGNOSTICS_TAIL_LINES);
    // 网卡报告附在日志后面：吞吐异常时，「哪块网卡在什么时刻掉了多少包」与
    // 日志尾部的时间轴是同一份证据，分成两次复制迟早会错位。
    format!(
        "{}\n{}",
        logging::diagnostics_text(tail),
        state.nic.report_text()
    )
}

/// 前端未捕获异常的上报载荷。
///
/// `kind` 形如 `error` / `unhandledrejection` / `manual`；`source` 是 `文件:行:列`；
/// `stack` 是调用栈文本。全部字段按不可信输入处理（编码后再落盘）。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrontendErrorReport {
    kind: String,
    message: String,
    source: String,
    stack: String,
}

/// 记录前端（WebView）异常：写主日志（事件码 `SYS-002`）+ 生成 `crash-js-*.md`。
#[tauri::command]
fn report_frontend_error(report: FrontendErrorReport, state: State<'_, AppState>) {
    logging::report_frontend_error(
        &state.engine,
        &report.kind,
        &report.message,
        &report.source,
        &report.stack,
    );
}

/// 在资源管理器中打开**实际生效**的日志目录。
#[tauri::command]
fn open_log_dir() -> Result<String, String> {
    let dir = logging::active_log_dir();
    std::fs::create_dir_all(&dir).map_err(|error| format!("无法创建日志目录：{error}"))?;
    std::process::Command::new("explorer")
        .arg(&dir)
        .spawn()
        .map_err(|error| format!("无法打开日志目录：{error}"))?;
    Ok(dir.display().to_string())
}

// ---------------------------------------------------------------------------
// 启动
// ---------------------------------------------------------------------------

/// 创建并运行桌面应用。
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // 引擎自带执行器（见 loadloom_core::Executor），**无需**外层运行时上下文，
            // 因此在 setup 主线程里可以直接构造，不必再包一层 block_on。
            let engine = Engine::spawn();
            // 网卡监测独立启动：它记录的是链路本身的状态，与是否正在打流无关。
            // 打流前后的链路抖动同样是解释吞吐曲线的关键证据。
            let nic = NicMonitor::spawn();
            let pt = pt::PtManager::new();
            app.manage(AppState {
                engine: Arc::clone(&engine),
                nic: Arc::clone(&nic),
                pt,
            });

            // ---- 日志落盘 + panic 捕获 ----
            logging::init(app.handle().clone(), &engine);

            // ---- 日志 / 生命周期 -> 前端事件流（同时镜像到磁盘） ----
            forward_logs(app.handle().clone(), engine.subscribe_logs());
            // 网卡事件与打流日志走同一条前端事件流、同一份磁盘日志：两者必须在
            // 同一时间轴上对照，否则「吞吐掉了」与「网卡正在丢包」永远对不上号。
            forward_logs(app.handle().clone(), nic.subscribe_logs());

            let event_handle: AppHandle = app.handle().clone();
            let mut events = engine.subscribe_events();
            tauri::async_runtime::spawn(async move {
                while let Ok(event) = events.recv().await {
                    let _ = event_handle.emit(EVENT_RUN, event);
                }
            });

            // ---- 托盘 ----
            let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

            let mut tray = TrayIconBuilder::with_id("main-tray")
                .menu(&menu)
                .tooltip("LoadLoom · 打流控制台")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => reveal(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        ..
                    } = event
                    {
                        reveal(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            Ok(())
        })
        // 关闭按钮 -> 收起到托盘（桌面应用惯例），真正退出走托盘菜单。
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_limits,
            get_snapshot,
            start_run,
            stop_run,
            set_live_config,
            subscribe_metrics,
            start_pt,
            stop_pt,
            get_pt_snapshot,
            subscribe_pt,
            get_log_path,
            get_diagnostics,
            report_frontend_error,
            open_log_dir,
            list_nic_adapters,
            get_nic_snapshot,
            set_nic_selection,
            get_nic_report,
            subscribe_nic,
            quit_app
        ])
        .run(tauri::generate_context!())
        .expect("启动 LoadLoom 桌面应用失败");
}

fn reveal(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// 把后台日志流接到「落盘 + 前端事件」两条出口。
///
/// 打流引擎与网卡监测共用它：两条流各写一份转发逻辑，格式与编码迟早漂移，
/// 而漂移的那一刻，两条时间轴就对不上了 —— 这恰恰是它们存在的唯一理由。
fn forward_logs(handle: AppHandle, mut logs: tokio::sync::broadcast::Receiver<LogEntry>) {
    tauri::async_runtime::spawn(async move {
        while let Ok(entry) = logs.recv().await {
            // 事件码与来源一并落盘：磁盘日志与界面条目保持同一条记录，
            // 否则「界面看到的码」在文件里搜不到，排障时又要来回对照。
            logging::write_full(entry.level, &entry.code, &entry.source, &entry.message);
            // 上屏的那一份同样走编码：两条出口共用同一次编码，否则事件流
            // 就是绕过日志编码的后门（见 logging::encoded_entry）。
            let _ = handle.emit(EVENT_LOG, logging::encoded_entry(entry));
        }
    });
}
