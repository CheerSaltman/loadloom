//! 桌面壳的日志落盘与崩溃转储。
//!
//! **为什么需要它**：这次「一点开始就闪退」的事故暴露了一个致命盲区 —— Tauri 的
//! release 构建一旦 panic，默认不给用户**任何**可见线索：窗口直接消失，没有控制台，
//! 没有文件，用户只能看到「闪退」两个字。本模块保证任何异常都留下可分析的痕迹。
//!
//! 两条出口，缺一不可：
//! 1. **落盘** —— 引擎日志与崩溃报告写入 `%LOCALAPPDATA%\TrafficConsole\logs\`，
//!    即使进程被强杀，事后依然可以回溯；
//! 2. **上屏** —— panic 信息同时推进引擎的日志流，前端「运行日志」页立刻可见，
//!    用户不必先去翻文件才知道发生了什么。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use tauri::AppHandle;

use traffic_core::{Engine, LogLevel};

/// 单文件上限，超过则轮转为 `traffic-console.prev.log`。
const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;

static SINK: OnceLock<Mutex<File>> = OnceLock::new();

/// 日志目录：`%LOCALAPPDATA%\TrafficConsole\logs`。
pub(crate) fn log_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("TrafficConsole")
        .join("logs")
}

/// 主日志文件路径。
pub(crate) fn log_file() -> PathBuf {
    log_dir().join("traffic-console.log")
}

fn open_sink() -> File {
    let dir = log_dir();
    let _ = fs::create_dir_all(&dir);
    let path = log_file();

    // 简单轮转，避免日志无限增长。
    if fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0) > MAX_LOG_BYTES {
        let _ = fs::rename(&path, dir.join("traffic-console.prev.log"));
    }

    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .or_else(|_| File::create(std::env::temp_dir().join("traffic-console.log")))
        .expect("无法创建日志文件")
}

/// 日志级别 -> 定宽标签，保证 `[ERROR]` 与 `[INFO ]` 对齐。
pub(crate) fn level_label(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Info => "INFO ",
        LogLevel::Warn => "WARN ",
        LogLevel::Error => "ERROR",
    }
}

/// 写一行日志。
///
/// **刻意永不 panic**：日志设施自身失败（磁盘满、权限不足、互斥锁中毒）绝不能
/// 反过来拖垮应用 —— 那正是它在故障时最该派上用场的时刻。
pub(crate) fn write(level: &str, message: &str) {
    let line = format!("{} [{}] {}\n", timestamp(), level, message);

    if let Some(sink) = SINK.get() {
        if let Ok(mut file) = sink.lock() {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }

    #[cfg(debug_assertions)]
    eprint!("{line}");
}

/// 初始化落盘句柄并安装 panic hook。
pub(crate) fn init(app: AppHandle, engine: &Arc<Engine>) {
    let _ = SINK.set(Mutex::new(open_sink()));
    write("INFO ", "================ 会话开始 ================");
    write(
        "INFO ",
        &format!(
            "日志文件 {} · 进程 PID {} · 时间戳为 UTC",
            log_file().display(),
            std::process::id()
        ),
    );
    install_panic_hook(app, Arc::clone(engine));
}

/// 安装 panic hook：先落盘 + 上屏，再交回默认 hook（保留 stderr 输出）。
fn install_panic_hook(app: AppHandle, engine: Arc<Engine>) {
    let default_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "未知 panic 载荷".to_owned());

        let location = info
            .location()
            .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
            .unwrap_or_else(|| "未知位置".to_owned());

        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_owned();

        write("ERROR", &format!("PANIC [{thread}] {message} @ {location}"));
        write(
            "ERROR",
            &format!("回溯：{}", std::backtrace::Backtrace::force_capture()),
        );

        // 上屏：走引擎既有的日志流，前端「运行日志」页立刻可见。
        engine.log_external(
            LogLevel::Error,
            format!("程序内部错误：{message}（{location}）"),
        );

        let _ = &app;
        default_hook(info);
    }));
}

/// `YYYY-MM-DD HH:MM:SS.mmmZ`（UTC）。
///
/// 刻意不引入 `chrono` / `time`：桌面壳只差一个时间戳，不值得为它扩大依赖树。
/// 代价是显示 UTC 而非本地时间 —— 日志里已显式标注 `Z`，排序与排查不受影响。
fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();

    let secs = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let days = secs.div_euclid(86_400);
    let time_of_day = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);

    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{millis:03}Z",
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60
    )
}

/// Howard Hinnant 的 `civil_from_days`：把「1970-01-01 起的天数」换算成公历年月日。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;

    (if month <= 2 { year + 1 } else { year }, month, day)
}
