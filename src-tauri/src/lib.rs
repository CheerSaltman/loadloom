//! Tauri v2 桌面壳：**只做 IPC 桥接，不含任何业务逻辑**。
//!
//! 职责边界：
//! * 创建原生窗口 / 托盘 / 主题跟随；
//! * 把 [`loadloom_core::Engine`] 的能力暴露为强类型 Command；
//! * 把引擎的广播通道转成 Event / Channel 推流（前端零轮询）；
//! * 把日志落盘并捕获 panic（见 [`logging`]）—— 保证任何异常都可事后分析。

mod logging;

use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State};

use loadloom_core::{
    CoreError, Engine, EngineLimits, LiveConfigPatch, MetricsSnapshot, StartRunRequest,
};

/// 日志流事件名。
pub const EVENT_LOG: &str = "loadloom://log";
/// 生命周期事件名。
pub const EVENT_RUN: &str = "loadloom://run-event";

/// 应用状态：仅持有一个引擎句柄。
pub struct AppState {
    pub engine: Arc<Engine>,
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

/// 退出应用。
#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
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
fn get_diagnostics(tail_lines: Option<u32>) -> String {
    let tail = tail_lines
        .map(|value| value.clamp(50, 5_000) as usize)
        .unwrap_or(logging::DIAGNOSTICS_TAIL_LINES);
    logging::diagnostics_text(tail)
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
            app.manage(AppState {
                engine: Arc::clone(&engine),
            });

            // ---- 日志落盘 + panic 捕获 ----
            logging::init(app.handle().clone(), &engine);

            // ---- 日志 / 生命周期 -> 前端事件流（同时镜像到磁盘） ----
            let log_handle: AppHandle = app.handle().clone();
            let mut logs = engine.subscribe_logs();
            tauri::async_runtime::spawn(async move {
                while let Ok(entry) = logs.recv().await {
                    // 事件码与来源一并落盘：磁盘日志与界面条目保持同一条记录，
                    // 否则「界面看到的码」在文件里搜不到，排障时又要来回对照。
                    logging::write_full(entry.level, &entry.code, &entry.source, &entry.message);
                    // 上屏的那一份同样走编码：两条出口共用同一次编码，否则事件流
                    // 就是绕过日志编码的后门（见 logging::encoded_entry）。
                    let _ = log_handle.emit(EVENT_LOG, logging::encoded_entry(entry));
                }
            });

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
            get_log_path,
            get_diagnostics,
            report_frontend_error,
            open_log_dir,
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
