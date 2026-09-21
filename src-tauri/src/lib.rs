//! Tauri v2 桌面壳：**只做 IPC 桥接，不含任何业务逻辑**。
//!
//! 职责边界：
//! * 创建原生窗口 / 托盘 / 主题跟随；
//! * 把 [`traffic_core::Engine`] 的能力暴露为强类型 Command；
//! * 把引擎的广播通道转成 Event / Channel 推流（前端零轮询）；
//! * 把日志落盘并捕获 panic（见 [`logging`]）—— 保证任何异常都可事后分析。

mod logging;

use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State};

use traffic_core::{
    CoreError, Engine, EngineLimits, LiveConfigPatch, MetricsSnapshot, StartRunRequest,
};

/// 日志流事件名。
pub const EVENT_LOG: &str = "traffic://log";
/// 生命周期事件名。
pub const EVENT_RUN: &str = "traffic://run-event";

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

/// 运行中动态调整并发 / 限速。
#[tauri::command]
fn set_live_config(patch: LiveConfigPatch, state: State<'_, AppState>) {
    state.engine.set_live(patch);
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

/// 返回日志文件路径（供界面展示，便于用户自行排查）。
#[tauri::command]
fn get_log_path() -> String {
    logging::log_file().display().to_string()
}

/// 在资源管理器中打开日志目录。
#[tauri::command]
fn open_log_dir() -> Result<String, String> {
    let dir = logging::log_dir();
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
            // 引擎自带执行器（见 traffic_core::Executor），**无需**外层运行时上下文，
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
                    logging::write(logging::level_label(entry.level), &entry.message);
                    let _ = log_handle.emit(EVENT_LOG, entry);
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
                .tooltip("Traffic Console · 打流控制台")
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
            open_log_dir,
            quit_app
        ])
        .run(tauri::generate_context!())
        .expect("启动 Traffic Console 桌面应用失败");
}

fn reveal(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
