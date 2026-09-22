//! 桌面壳的日志落盘与崩溃转储。
//!
//! **为什么需要它**：这次「一点开始就闪退」的事故暴露了一个致命盲区 —— Tauri 的
//! release 构建一旦 panic，默认不给用户**任何**可见线索：窗口直接消失，没有控制台，
//! 没有文件，用户只能看到「闪退」两个字。本模块保证任何异常都留下可分析的痕迹。
//!
//! 两条出口，缺一不可：
//! 1. **落盘** —— 引擎日志与崩溃报告写入 `%LOCALAPPDATA%\LoadLoom\logs\`，
//!    即使进程被强杀，事后依然可以回溯；
//! 2. **上屏** —— panic 信息同时推进引擎的日志流，前端「运行日志」页立刻可见，
//!    用户不必先去翻文件才知道发生了什么。
//!
//! ## 内部分工
//!
//! * 纯函数区（时间戳换算、行格式化）不碰文件系统，因此可以无副作用地单测；
//! * `Sink` 是唯一接触文件系统的类型，负责句柄、体积轮转与降级。
//!
//! ## 两条硬性约束
//!
//! * **永不 panic**：日志设施自身失败（磁盘满、权限不足、互斥锁中毒）绝不能反过来
//!   拖垮应用 —— 那正是它最该派上用场的时刻。落盘失败就降级为「只上屏」，不中断启动。
//! * **不撒谎**：界面拿到的日志路径必须是**真正在写**的那一个；退回临时目录时也要如实反映。
//! * **不让内容越权**：日志正文可能来自远端（错误文本、响应片段），其中的换行与
//!   控制字符会被编码（见 `sanitize`），否则攻击者可以伪造出额外的日志行
//!   （CWE-117 日志注入），让「运行日志」页与磁盘日志都出现由他编排的记录。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use loadloom_core::{Engine, LogLevel};

/// 单文件上限，超过则轮转为 `loadloom.prev.log`。
const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;
/// 主日志文件名。
const LOG_FILE_NAME: &str = "loadloom.log";
/// 轮转后保留的上一代文件名（只留一代，避免日志把磁盘吃满）。
const PREV_LOG_FILE_NAME: &str = "loadloom.prev.log";

/// 落盘句柄。外层 `None` 表示「还没初始化」，内层 `None` 表示「日志无法落盘，只上屏」——
/// 后者是允许的降级状态，不是错误。
static SINK: OnceLock<Option<Mutex<Sink>>> = OnceLock::new();

// ---------------------------------------------------------------------------
// 路径
// ---------------------------------------------------------------------------

/// 日志目录：`%LOCALAPPDATA%\LoadLoom\logs`。
pub(crate) fn log_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("LoadLoom")
        .join("logs")
}

/// 当前**真正在写**的日志文件。
///
/// 正常情况落在 [`log_dir`]；主目录不可写而退回临时目录时，返回的是临时目录里那一个。
/// 落盘完全不可用时返回 `None` —— 界面据此显示「未落盘」，而不是展示一个并不存在的路径。
pub(crate) fn active_log_file() -> Option<PathBuf> {
    sink().map(|sink| sink.path.clone())
}

/// 当前**真正在写**的日志目录，供「打开日志目录」使用（降级时也指向正确的位置）。
pub(crate) fn active_log_dir() -> PathBuf {
    active_log_file()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(log_dir)
}

/// 借用全局 sink。
///
/// 互斥锁中毒（上一次写日志时 panic）视同可用并取出内部值：因为一次 panic 就永久
/// 丢掉日志，恰恰和「崩溃必须留痕」的目标相反。
fn sink() -> Option<MutexGuard<'static, Sink>> {
    let slot = SINK.get()?;
    let sink = slot.as_ref()?;
    Some(sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
}

// ---------------------------------------------------------------------------
// 纯函数区：时间戳与行格式（不碰文件系统，可单测）
// ---------------------------------------------------------------------------

/// 日志级别 -> 定宽 5 字符标签，保证 `[INFO ]` 与 `[ERROR]` 落在同一列。
fn level_label(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Info => "INFO ",
        LogLevel::Warn => "WARN ",
        LogLevel::Error => "ERROR",
    }
}

/// 组装一整行（含结尾换行）。落盘与上屏共用同一份文本，避免两处格式各自漂移。
fn format_line(at: SystemTime, level: LogLevel, message: &str) -> String {
    format!("{} [{}] {}\n", timestamp(at), level_label(level), message)
}

/// 单条日志正文的字符上限。超出部分截断 —— 消息可能来自远端响应或错误文本，
/// 不该由它决定一行日志能有多大。
const MAX_MESSAGE_CHARS: usize = 2_000;

/// 把控制字符编码成可见转义，并截断超长消息。
///
/// 这是**输出编码**：日志正文来自进程外部（远端错误文本、服务器响应片段），
/// 直接落盘等于把「能否伪造日志行」的决定权交给对端 —— 一个带 `\n` 的错误信息
/// 就能在 `loadloom.log` 与界面「运行日志」里插入一行看似正常的记录
/// （日志注入，CWE-117）。这里把换行 / 回车 / 制表符换成 `\n` `\r` `\t` 字面量，
/// 其余控制字符换成 `\u{XXXX}`，保证**一条消息永远只占一行**。
fn sanitize(message: &str) -> String {
    let mut out = String::with_capacity(message.len().min(MAX_MESSAGE_CHARS));
    for character in message.chars().take(MAX_MESSAGE_CHARS) {
        match character {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ if character.is_control() => {
                out.push_str(&format!("\\u{{{:04X}}}", character as u32));
            }
            _ => out.push(character),
        }
    }
    if message.chars().count() > MAX_MESSAGE_CHARS {
        out.push('…');
    }
    out
}

/// `YYYY-MM-DD HH:MM:SS.mmmZ`（UTC）。
///
/// 不引入 `chrono` / `time`：桌面壳只需要一个时间戳，不值得为此扩大依赖树。
/// 代价是显示 UTC 而非本地时间 —— 日志里已显式标注 `Z`，排序与排查不受影响。
fn timestamp(at: SystemTime) -> String {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);

    let secs = since_epoch.as_secs() as i64;
    let millis = since_epoch.subsec_millis();
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

// ---------------------------------------------------------------------------
// Sink：唯一接触文件系统的部分
// ---------------------------------------------------------------------------

/// 落盘句柄 + 体积轮转状态。
struct Sink {
    /// `None` = 当前不可写（磁盘满、权限变化）；下一次写入会尝试重开自愈。
    file: Option<File>,
    /// 实际生效的文件路径（可能是临时目录里的降级路径）。
    path: PathBuf,
    /// 本会话累计写入字节数，用于触发轮转。
    written: u64,
    /// 轮转阈值。做成字段而不是直接读常量，单测才能用极小阈值把轮转逼出来。
    limit: u64,
}

impl Sink {
    /// 按生产阈值在 `dir` 下打开落盘句柄。
    fn open(dir: &Path) -> std::io::Result<Self> {
        Self::open_with_limit(dir, MAX_LOG_BYTES)
    }

    /// 在 `dir` 下打开落盘句柄；若已有日志超过 `limit`，先归档再开新文件。
    fn open_with_limit(dir: &Path, limit: u64) -> std::io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(LOG_FILE_NAME);

        // 上一会话残留的日志已经超限时先归档，避免这次接着往一个超限文件里追加。
        if file_len(&path) > limit {
            rotate_files(&path);
        }

        let file = open_append(&path)?;
        let written = file_len(&path);
        Ok(Self {
            file: Some(file),
            path,
            written,
            limit,
        })
    }

    /// 写一行；必要时先轮转。任何一步失败都不 panic。
    fn write_line(&mut self, line: &str) {
        let length = line.len() as u64;
        if self.written + length > self.limit {
            self.rotate();
        }
        // 上一轮重开失败（例如写满的磁盘刚被腾空）后自愈：能开就继续写。
        if self.file.is_none() {
            self.file = open_append(&self.path).ok();
        }
        let Some(file) = self.file.as_mut() else {
            return;
        };
        if file.write_all(line.as_bytes()).is_err() || file.flush().is_err() {
            // 句柄坏了就丢掉，下次写入重开；日志不能因为一个坏句柄永久断流。
            self.file = None;
            return;
        }
        self.written += length;
    }

    /// 把当前文件归档为 `.prev`，再重开一个空的主文件。
    fn rotate(&mut self) {
        // 先松手：Windows 上被打开的文件不允许改名。
        self.file = None;
        self.written = 0;
        if !rotate_files(&self.path) {
            // 归档失败（例如 `.prev` 正被别的进程占用）时不丢日志：退回同一个文件继续追加。
            // 此时文件会超过上限，但「日志超限」远好于「日志丢失」。
            self.written = file_len(&self.path);
            return;
        }
        self.file = open_append(&self.path).ok();
    }
}

/// 文件字节数；不存在或不可读都按 0 处理（日志路径上的错误不值得中断流程）。
fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn open_append(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// 把 `loadloom.log` 归档为 `loadloom.prev.log`，只保留一代。
///
/// 必须先删掉上一代 `.prev`：Windows 的 `fs::rename` 在目标已存在时会直接失败
/// （`ERROR_ALREADY_EXISTS`），于是从第二次轮转起就静默失效、日志无限增长。
/// 返回是否归档成功；失败时调用方继续往原文件追加，绝不丢日志。
fn rotate_files(path: &Path) -> bool {
    let previous = path.with_file_name(PREV_LOG_FILE_NAME);
    let _ = fs::remove_file(&previous);
    fs::rename(path, &previous).is_ok()
}

// ---------------------------------------------------------------------------
// 对外入口
// ---------------------------------------------------------------------------

/// 打开落盘句柄：主目录优先，失败退回系统临时目录，再失败就只上屏。
///
/// 刻意不用 `expect`：因为「写不出日志」而让应用启动失败，是本末倒置 ——
/// 日志设施存在的意义就是在故障时还能说话。
fn open_sink() -> Option<Sink> {
    let primary = log_dir();
    if let Ok(sink) = Sink::open(&primary) {
        return Some(sink);
    }

    match Sink::open(&std::env::temp_dir()) {
        Ok(sink) => {
            note(&format!(
                "无法写入 {}，日志改写到 {}",
                primary.display(),
                sink.path.display()
            ));
            Some(sink)
        }
        Err(error) => {
            note(&format!("日志无法落盘（{error}），本次会话只上屏"));
            None
        }
    }
}

/// 把降级原因写到 stderr。
///
/// release 的 GUI 进程没有控制台，`eprintln!` 无处可去，因此只在 debug 构建里输出；
/// 用 `cfg!` 而非 `#[cfg]`，让这段代码在两种 profile 下都参与编译检查。
fn note(message: &str) {
    if cfg!(debug_assertions) {
        eprintln!("[logging] {message}");
    }
}

/// 写一行日志（落盘；debug 构建下同时上屏）。
///
/// 永不 panic：日志设施自身失败（磁盘满、权限不足、互斥锁中毒）绝不能
/// 反过来拖垮应用 —— 那正是它在故障时最该派上用场的时刻。
pub(crate) fn write(level: LogLevel, message: &str) {
    // 唯一写文件 / 上屏的出口，因此编码也放在这里：任何调用方都不可能绕过
    // （包括 panic hook 里那段可能夹杂远端文本的消息）。
    let line = format_line(SystemTime::now(), level, &sanitize(message));

    if let Some(mut sink) = sink() {
        sink.write_line(&line);
    }

    #[cfg(debug_assertions)]
    eprint!("{line}");
}

/// 初始化落盘句柄并安装 panic hook。
pub(crate) fn init(app: AppHandle, engine: &Arc<Engine>) {
    let _ = SINK.set(open_sink().map(Mutex::new));

    write(LogLevel::Info, "================ 会话开始 ================");
    let location = active_log_file()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "（未落盘）".to_owned());
    write(
        LogLevel::Info,
        &format!(
            "日志文件 {location} · 进程 PID {} · 时间戳为 UTC",
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

        write(
            LogLevel::Error,
            &format!("PANIC [{thread}] {message} @ {location}"),
        );
        write(
            LogLevel::Error,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64, millis: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs) + Duration::from_millis(millis)
    }

    /// 独立临时目录：测试之间不共享状态，也不碰真实的 `%LOCALAPPDATA%`。
    fn temp_dir(tag: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("loadloom-logging-{tag}-{unique}"));
        fs::create_dir_all(&dir).expect("创建测试目录");
        dir
    }

    #[test]
    fn timestamp_reports_utc_with_milliseconds() {
        assert_eq!(timestamp(at(0, 0)), "1970-01-01 00:00:00.000Z");
        assert_eq!(timestamp(at(0, 123)), "1970-01-01 00:00:00.123Z");
        assert_eq!(timestamp(at(86_399, 999)), "1970-01-01 23:59:59.999Z");
        assert_eq!(timestamp(at(1_000_000_000, 0)), "2001-09-09 01:46:40.000Z");
        // 闰日：2 月 29 日必须存在，且不能溢到 3 月 1 日。
        assert_eq!(timestamp(at(1_709_164_800, 0)), "2024-02-29 00:00:00.000Z");
        // 世纪闰年规则里「能被 400 整除才是闰年」的另一侧。
        assert_eq!(timestamp(at(951_868_800, 0)), "2000-03-01 00:00:00.000Z");
    }

    #[test]
    fn control_characters_cannot_forge_extra_log_lines() {
        // 回归点（CWE-117）：远端错误文本里带换行时，必须被编码成字面量，
        // 否则攻击者可以在日志里插入一行伪装成其它级别的记录。
        assert_eq!(
            sanitize("请求失败：boom\n2026-01-01 00:00:00.000Z [ERROR] 伪造的崩溃"),
            "请求失败：boom\\n2026-01-01 00:00:00.000Z [ERROR] 伪造的崩溃"
        );
        assert_eq!(sanitize("a\r\nb"), "a\\r\\nb");
        assert_eq!(sanitize("tab\there"), "tab\\there");
        assert_eq!(sanitize("bell\u{7}"), "bell\\u{0007}");
        // 正常文本原样通过，中日文与 emoji 不受影响。
        assert_eq!(sanitize("正常 one two 🚀"), "正常 one two 🚀");
    }

    #[test]
    fn an_overlong_message_is_truncated_to_one_bounded_line() {
        let huge = "x".repeat(MAX_MESSAGE_CHARS * 3);
        let sanitized = sanitize(&huge);
        assert_eq!(sanitized.chars().count(), MAX_MESSAGE_CHARS + 1);
        assert!(sanitized.ends_with('…'));
        assert!(!sanitized.contains('\n'));

        // 每条日志行（含时间戳与级别）仍然只占一行。
        let line = format_line(at(0, 0), LogLevel::Error, &sanitize("boom\nboom"));
        assert_eq!(line.matches('\n').count(), 1);
        assert!(line.ends_with("boom\\nboom\n"));
    }

    #[test]
    fn level_labels_share_one_width_so_columns_align() {
        for level in [LogLevel::Info, LogLevel::Warn, LogLevel::Error] {
            assert_eq!(
                level_label(level).len(),
                "ERROR".len(),
                "{level:?} 的标签宽度与其它级别不一致，日志列会对不齐"
            );
        }
    }

    #[test]
    fn formatted_line_layout_is_stable() {
        assert_eq!(
            format_line(at(0, 0), LogLevel::Error, "boom"),
            "1970-01-01 00:00:00.000Z [ERROR] boom\n"
        );
        assert_eq!(
            format_line(at(0, 0), LogLevel::Info, "ok"),
            "1970-01-01 00:00:00.000Z [INFO ] ok\n"
        );
    }

    #[test]
    fn rotation_overwrites_the_previous_generation() {
        let dir = temp_dir("rotate");
        let path = dir.join(LOG_FILE_NAME);
        let previous = dir.join(PREV_LOG_FILE_NAME);

        fs::write(&path, b"first").expect("写入第一代");
        assert!(rotate_files(&path), "第一次归档应当成功");
        assert!(!path.exists(), "归档后主文件应当已被挪走");
        assert_eq!(fs::read_to_string(&previous).unwrap(), "first");

        // 关键回归点：Windows 上 `fs::rename` 遇到已存在的目标会失败
        // （ERROR_ALREADY_EXISTS）。旧实现没先删 `.prev`，于是从第二次轮转起
        // 就静默失效 —— 断言覆盖（而不是保留）上一代，才能钉住这个行为。
        fs::write(&path, b"second").expect("写入第二代");
        assert!(rotate_files(&path), "第二次归档必须同样成功");
        assert_eq!(fs::read_to_string(&previous).unwrap(), "second");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_oversized_log_is_archived_when_opening() {
        let dir = temp_dir("stale");
        let path = dir.join(LOG_FILE_NAME);
        fs::write(&path, vec![b'x'; 64]).expect("写入上一会话的日志");

        let sink = Sink::open_with_limit(&dir, 32).expect("打开 sink");

        assert_eq!(sink.written, 0, "超限的旧日志应先归档，新文件从 0 开始计");
        assert_eq!(sink.path, path);
        assert_eq!(file_len(&dir.join(PREV_LOG_FILE_NAME)), 64);
        assert_eq!(file_len(&path), 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_write_path_rotates_once_the_limit_is_crossed() {
        let dir = temp_dir("sink");
        // 每行 8 字节、阈值 32 字节：用小阈值把轮转逼出来，单测不必真写 4 MiB。
        let mut sink = Sink::open_with_limit(&dir, 32).expect("打开 sink");

        for index in 0..10_u32 {
            sink.write_line(&format!("{index:07}\n"));
        }

        assert_eq!(
            fs::read_to_string(&sink.path).expect("读主日志"),
            "0000008\n0000009\n",
            "主文件应只保留最后一次轮转之后写入的行"
        );
        assert_eq!(
            fs::read_to_string(dir.join(PREV_LOG_FILE_NAME)).expect("读上一代日志"),
            "0000004\n0000005\n0000006\n0000007\n",
            "第二轮轮转必须覆盖上一代；保留第一代说明归档在第二次失败了"
        );
        assert!(file_len(&sink.path) <= 32, "主文件不应超过阈值");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_sink_degrades_instead_of_panicking() {
        let dir = temp_dir("broken");
        let mut sink = Sink::open_with_limit(&dir, MAX_LOG_BYTES).expect("打开 sink");

        // 手工把句柄指向一个父目录不存在的路径：模拟运行中磁盘/权限失效。
        sink.file = None;
        sink.path = dir.join("missing-subdir").join(LOG_FILE_NAME);

        sink.write_line("still alive\n"); // 不得 panic
        assert_eq!(sink.written, 0);

        // 路径恢复可写之后应当自愈，而不是永久静默丢弃日志。
        sink.path = dir.join(LOG_FILE_NAME);
        sink.write_line("recovered\n");
        assert_eq!(
            fs::read_to_string(&sink.path).expect("读主日志"),
            "recovered\n"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
