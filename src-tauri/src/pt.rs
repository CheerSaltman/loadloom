//! 真实 BitTorrent 压测 sidecar 的生命周期与 JSON 行协议桥接。
//!
//! BitTorrent/DHT/uTP 由 Go sidecar 实现；本模块只负责严格校验输入、拉起进程、
//! 把指标转换成 Rust 契约并保证应用退出时不会遗留后台进程。

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use loadloom_core::{CoreError, PressureLevel, PtPhase, PtSnapshot, PtStartRequest};
use serde::Deserialize;
use tokio::sync::broadcast;

const MAX_SOURCE_LEN: usize = 64 * 1024;

#[cfg(target_os = "windows")]
const SIDECAR_BINARY: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/binaries/loadloom-pt-x86_64-pc-windows-msvc.exe"
));

struct Inner {
    running: bool,
    snapshot: PtSnapshot,
    control: Option<ChildStdin>,
    child: Option<Child>,
}

pub struct PtManager {
    inner: Mutex<Inner>,
    frames: broadcast::Sender<PtSnapshot>,
    generation: AtomicU64,
}

impl PtManager {
    pub fn new() -> Arc<Self> {
        let (frames, _) = broadcast::channel(64);
        Arc::new(Self {
            inner: Mutex::new(Inner {
                running: false,
                snapshot: idle_snapshot(),
                control: None,
                child: None,
            }),
            frames,
            generation: AtomicU64::new(0),
        })
    }

    pub fn is_running(&self) -> bool {
        self.lock().running
    }

    pub fn snapshot(&self) -> PtSnapshot {
        self.lock().snapshot.clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PtSnapshot> {
        self.frames.subscribe()
    }

    pub fn start(self: &Arc<Self>, request: PtStartRequest) -> Result<(), CoreError> {
        validate(&request)?;
        let executable = resolve_sidecar()?;
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;

        let mut command = Command::new(&executable);
        command
            .arg("--source")
            .arg(&request.source)
            .arg("--connections")
            .arg(request.max_connections.to_string())
            .arg("--ram-mib")
            .arg(request.ram_mib.to_string())
            .arg("--duration-seconds")
            .arg(request.duration_secs.to_string())
            .arg("--max-download-gib")
            .arg(request.max_download_gib.to_string())
            .arg("--rate-mib")
            .arg(request.rate_mib.to_string())
            .arg("--stalled-seconds")
            .arg(request.stalled_peer_secs.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().map_err(|error| {
            CoreError::Internal(format!(
                "无法启动 PT sidecar {}：{error}",
                executable.display()
            ))
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CoreError::Internal("PT sidecar 未提供 stdout".to_owned()))?;
        let stderr = child.stderr.take();
        let control = child
            .stdin
            .take()
            .ok_or_else(|| CoreError::Internal("PT sidecar 未提供 stdin".to_owned()))?;

        {
            let mut inner = self.lock();
            if inner.running {
                let _ = child.kill();
                return Err(CoreError::AlreadyRunning(
                    "真实 PT 压测已经在运行".to_owned(),
                ));
            }
            inner.running = true;
            inner.control = Some(control);
            inner.child = Some(child);
            inner.snapshot = PtSnapshot {
                phase: PtPhase::Starting,
                status: "正在启动 Go PT 引擎".to_owned(),
                pressure_reason: "等待 swarm 采样".to_owned(),
                payload_persistence: "ram-verify-discard".to_owned(),
                ram_limit_bytes: u64::from(request.ram_mib) << 20,
                ..PtSnapshot::default()
            };
            self.publish(&inner.snapshot);
        }

        let manager = Arc::clone(self);
        std::thread::Builder::new()
            .name("loadloom-pt-json".to_owned())
            .spawn(move || manager.read_stdout(generation, stdout))
            .map_err(|error| CoreError::Internal(format!("无法创建 PT 指标线程：{error}")))?;
        if let Some(stderr) = stderr {
            let manager = Arc::clone(self);
            let _ = std::thread::Builder::new()
                .name("loadloom-pt-stderr".to_owned())
                .spawn(move || manager.drain_stderr(generation, stderr));
        }
        Ok(())
    }

    pub fn stop(self: &Arc<Self>) {
        let generation = self.generation.load(Ordering::Acquire);
        let mut inner = self.lock();
        if !inner.running {
            return;
        }
        if let Some(control) = inner.control.as_mut() {
            let _ = control.write_all(b"{\"type\":\"stop\"}\n");
            let _ = control.flush();
        }
        inner.snapshot.status = "正在停止 PT 引擎".to_owned();
        self.publish(&inner.snapshot);
        drop(inner);

        // 正常 sidecar 会立即响应 stdin 命令。若协议线程卡死，三秒后强制结束，
        // 防止桌面看似停止而数百条 Peer 连接仍留在后台。
        let manager = Arc::clone(self);
        let _ = std::thread::Builder::new()
            .name("loadloom-pt-stop-watchdog".to_owned())
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(3));
                if manager.generation.load(Ordering::Acquire) != generation {
                    return;
                }
                let mut inner = manager.lock();
                if inner.running {
                    if let Some(child) = inner.child.as_mut() {
                        let _ = child.kill();
                    }
                    inner.snapshot.status = "PT sidecar 未及时退出，已强制停止".to_owned();
                    manager.publish(&inner.snapshot);
                }
            });
    }

    fn read_stdout(self: &Arc<Self>, generation: u64, stdout: impl std::io::Read) {
        let mut terminal_event = false;
        for line in BufReader::new(stdout).lines() {
            if self.generation.load(Ordering::Acquire) != generation {
                break;
            }
            match line {
                Ok(line) if !line.trim().is_empty() => {
                    terminal_event |= self.handle_line(&line);
                }
                Ok(_) => {}
                Err(error) => {
                    self.fail(generation, format!("读取 PT 指标失败：{error}"));
                    break;
                }
            }
        }

        let exit = {
            let mut inner = self.lock();
            inner.control = None;
            inner.child.take().and_then(|mut child| child.wait().ok())
        };
        if self.generation.load(Ordering::Acquire) != generation {
            return;
        }
        let mut inner = self.lock();
        inner.running = false;
        if !terminal_event && inner.snapshot.phase != PtPhase::Failed {
            inner.snapshot.phase = PtPhase::Failed;
            inner.snapshot.status = "PT sidecar 意外退出".to_owned();
            inner.snapshot.last_error = exit
                .map(|status| format!("sidecar exit={status}"))
                .unwrap_or_else(|| "无法读取 sidecar 退出状态".to_owned());
        }
        self.publish(&inner.snapshot);
    }

    fn handle_line(&self, line: &str) -> bool {
        let envelope: Envelope = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                let mut inner = self.lock();
                inner.snapshot.last_error = format!("PT sidecar JSON 无效：{error}");
                self.publish(&inner.snapshot);
                return false;
            }
        };
        match envelope.kind.as_str() {
            "ready" => {
                let mut inner = self.lock();
                inner.snapshot.phase = PtPhase::Metadata;
                inner.snapshot.status =
                    "正在通过 Tracker / DHT / PEX 发现 Peer 与元数据".to_owned();
                self.publish(&inner.snapshot);
                false
            }
            "log" => {
                if let Ok(event) = serde_json::from_str::<SidecarLog>(line) {
                    let mut inner = self.lock();
                    inner.snapshot.status = event.message.clone();
                    if event.level == "error" {
                        inner.snapshot.last_error = format!("{} {}", event.code, event.message);
                    }
                    self.publish(&inner.snapshot);
                }
                false
            }
            "fatal" => {
                let event = serde_json::from_str::<SidecarLog>(line).ok();
                let message = event
                    .map(|value| format!("{} {}", value.code, value.message))
                    .unwrap_or_else(|| "PT sidecar 报告致命错误".to_owned());
                let mut inner = self.lock();
                inner.snapshot.phase = PtPhase::Failed;
                inner.snapshot.status = "PT 压测失败".to_owned();
                inner.snapshot.last_error = message;
                self.publish(&inner.snapshot);
                true
            }
            "metrics" => {
                if let Ok(event) = serde_json::from_str::<SidecarMetrics>(line) {
                    let mut inner = self.lock();
                    inner.snapshot = event.into_snapshot();
                    self.publish(&inner.snapshot);
                }
                false
            }
            "completed" | "stopped" => {
                let event = serde_json::from_str::<TerminalEvent>(line).ok();
                let mut inner = self.lock();
                inner.snapshot.phase = if envelope.kind == "completed" {
                    PtPhase::Completed
                } else {
                    PtPhase::Idle
                };
                inner.snapshot.status = event
                    .map(|value| value.reason)
                    .unwrap_or_else(|| "PT 压测已结束".to_owned());
                self.publish(&inner.snapshot);
                true
            }
            _ => false,
        }
    }

    fn drain_stderr(&self, generation: u64, stderr: impl std::io::Read) {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if self.generation.load(Ordering::Acquire) != generation {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let mut inner = self.lock();
            inner.snapshot.last_error = trimmed.chars().take(500).collect();
            self.publish(&inner.snapshot);
        }
    }

    fn fail(&self, generation: u64, message: String) {
        if self.generation.load(Ordering::Acquire) != generation {
            return;
        }
        let mut inner = self.lock();
        inner.snapshot.phase = PtPhase::Failed;
        inner.snapshot.status = "PT 指标通道失败".to_owned();
        inner.snapshot.last_error = message;
        self.publish(&inner.snapshot);
    }

    fn publish(&self, snapshot: &PtSnapshot) {
        let _ = self.frames.send(snapshot.clone());
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Drop for PtManager {
    fn drop(&mut self) {
        let inner = self
            .inner
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(mut child) = inner.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn idle_snapshot() -> PtSnapshot {
    PtSnapshot {
        phase: PtPhase::Idle,
        status: "真实 PT 引擎待机".to_owned(),
        pressure_reason: "等待开始".to_owned(),
        payload_persistence: "ram-verify-discard".to_owned(),
        ..PtSnapshot::default()
    }
}

fn validate(request: &PtStartRequest) -> Result<(), CoreError> {
    if !request.authorized {
        return Err(CoreError::NotAuthorized(
            "请确认仅使用合法公开或已获授权的 torrent".to_owned(),
        ));
    }
    let source = request.source.trim();
    if source.len() > MAX_SOURCE_LEN
        || !(source.starts_with("magnet:?")
            || source.starts_with("https://")
            || source.starts_with("http://"))
    {
        return Err(CoreError::InvalidInput(
            "PT 来源必须是 magnet URI 或 HTTP(S) .torrent URL".to_owned(),
        ));
    }
    if !(8..=500).contains(&request.max_connections) {
        return Err(CoreError::InvalidInput(
            "PT 最大连接数必须在 8..=500".to_owned(),
        ));
    }
    if !(64..=4096).contains(&request.ram_mib) {
        return Err(CoreError::InvalidInput(
            "PT RAM 上限必须在 64..=4096 MiB".to_owned(),
        ));
    }
    if request.duration_secs > 86_400 || !(5..=120).contains(&request.stalled_peer_secs) {
        return Err(CoreError::InvalidInput(
            "PT 时长或死链判定窗口超出允许范围".to_owned(),
        ));
    }
    for (value, max, label) in [
        (request.max_download_gib, 1024.0, "下载上限"),
        (request.rate_mib, 4096.0, "限速"),
    ] {
        if !value.is_finite() || value < 0.0 || value > max {
            return Err(CoreError::InvalidInput(format!(
                "PT {label}必须是 0..={max} 的有限数字"
            )));
        }
    }
    Ok(())
}

fn resolve_sidecar() -> Result<PathBuf, CoreError> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("LOADLOOM_PT_SIDECAR") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("loadloom-pt.exe"));
            candidates.push(parent.join("loadloom-pt-x86_64-pc-windows-msvc.exe"));
        }
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    candidates.push(
        manifest
            .join("binaries")
            .join("loadloom-pt-x86_64-pc-windows-msvc.exe"),
    );
    candidates.push(
        manifest
            .join("..")
            .join("sidecars")
            .join("loadloom-pt")
            .join("bin")
            .join("loadloom-pt.exe"),
    );
    if let Some(path) = candidates.into_iter().find(|path| path.is_file()) {
        return Ok(path);
    }

    #[cfg(target_os = "windows")]
    {
        extract_embedded_sidecar()
    }

    #[cfg(not(target_os = "windows"))]
    Err(CoreError::Internal(
        "找不到 loadloom-pt sidecar；请先运行 npm run pt:build".to_owned(),
    ))
}

#[cfg(target_os = "windows")]
fn extract_embedded_sidecar() -> Result<PathBuf, CoreError> {
    let directory = std::env::temp_dir().join("LoadLoom").join("sidecars");
    std::fs::create_dir_all(&directory)
        .map_err(|error| CoreError::Internal(format!("无法创建 PT sidecar 临时目录: {error}")))?;

    let target = directory.join(format!("loadloom-pt-{}.exe", env!("CARGO_PKG_VERSION")));
    if std::fs::read(&target).is_ok_and(|existing| existing == SIDECAR_BINARY) {
        return Ok(target);
    }

    let staging = directory.join(format!(
        "loadloom-pt-{}-{}.tmp",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    ));
    std::fs::write(&staging, SIDECAR_BINARY)
        .map_err(|error| CoreError::Internal(format!("无法释放 PT sidecar: {error}")))?;

    if target.exists() {
        std::fs::remove_file(&target).map_err(|error| {
            CoreError::Internal(format!("无法更新 PT sidecar 临时文件: {error}"))
        })?;
    }
    std::fs::rename(&staging, &target)
        .map_err(|error| CoreError::Internal(format!("无法启用 PT sidecar: {error}")))?;
    Ok(target)
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SidecarLog {
    level: String,
    code: String,
    message: String,
}

#[derive(Deserialize)]
struct TerminalEvent {
    reason: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SidecarMetrics {
    seq: u64,
    name: String,
    info_hash: String,
    elapsed_secs: f64,
    progress_percent: f64,
    total_bytes: u64,
    wire_bytes: u64,
    verified_bytes: u64,
    wasted_bytes: u64,
    speed_bps: f64,
    average_bps: f64,
    active_peers: u32,
    pending_peers: u32,
    half_open_peers: u32,
    connected_seeders: u32,
    useful_peers: u32,
    stalled_peers: u32,
    peer_handshakes: u64,
    closed_peers: u64,
    dead_peers: u64,
    dead_peer_percent: f64,
    tracker_errors: u64,
    tracker_successes: u64,
    good_pieces: u64,
    bad_pieces: u64,
    ram_used_bytes: u64,
    ram_peak_bytes: u64,
    ram_limit_bytes: u64,
    storage_errors: u64,
    pressure_level: String,
    pressure_reason: String,
    payload_persistence: String,
}

impl SidecarMetrics {
    fn into_snapshot(self) -> PtSnapshot {
        let pressure_level = match self.pressure_level.as_str() {
            "warning" => PressureLevel::Warning,
            "critical" => PressureLevel::Critical,
            _ => PressureLevel::Normal,
        };
        PtSnapshot {
            seq: self.seq,
            phase: PtPhase::Downloading,
            name: self.name,
            info_hash: self.info_hash,
            elapsed_secs: self.elapsed_secs,
            progress_percent: self.progress_percent,
            total_bytes: self.total_bytes,
            wire_bytes: self.wire_bytes,
            verified_bytes: self.verified_bytes,
            wasted_bytes: self.wasted_bytes,
            speed_bps: self.speed_bps,
            average_bps: self.average_bps,
            active_peers: self.active_peers,
            pending_peers: self.pending_peers,
            half_open_peers: self.half_open_peers,
            connected_seeders: self.connected_seeders,
            useful_peers: self.useful_peers,
            stalled_peers: self.stalled_peers,
            peer_handshakes: self.peer_handshakes,
            closed_peers: self.closed_peers,
            dead_peers: self.dead_peers,
            dead_peer_percent: self.dead_peer_percent,
            tracker_errors: self.tracker_errors,
            tracker_successes: self.tracker_successes,
            good_pieces: self.good_pieces,
            bad_pieces: self.bad_pieces,
            ram_used_bytes: self.ram_used_bytes,
            ram_peak_bytes: self.ram_peak_bytes,
            ram_limit_bytes: self.ram_limit_bytes,
            storage_errors: self.storage_errors,
            pressure_level,
            pressure_reason: self.pressure_reason,
            status: "真实 PT swarm 下载中 · piece 校验后立即丢弃".to_owned(),
            last_error: String::new(),
            payload_persistence: self.payload_persistence,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_validation_enforces_safe_bounds() {
        let valid = PtStartRequest {
            source: "magnet:?xt=urn:btih:0123456789012345678901234567890123456789".to_owned(),
            max_connections: 180,
            ram_mib: 512,
            duration_secs: 600,
            max_download_gib: 10.0,
            rate_mib: 0.0,
            stalled_peer_secs: 15,
            authorized: true,
        };
        assert!(validate(&valid).is_ok());
        assert!(validate(&PtStartRequest {
            max_connections: 501,
            ..valid.clone()
        })
        .is_err());
        assert!(validate(&PtStartRequest {
            rate_mib: f64::NAN,
            ..valid.clone()
        })
        .is_err());
        assert!(validate(&PtStartRequest {
            authorized: false,
            ..valid
        })
        .is_err());
    }

    #[test]
    fn sidecar_metrics_map_to_the_shared_contract() {
        let line = r#"{"seq":7,"name":"Ubuntu","infoHash":"abc","elapsedSecs":1.5,"progressPercent":2.0,"totalBytes":100,"wireBytes":120,"verifiedBytes":80,"wastedBytes":20,"speedBps":50.0,"averageBps":40.0,"activePeers":5,"pendingPeers":7,"halfOpenPeers":2,"connectedSeeders":3,"usefulPeers":4,"stalledPeers":1,"peerHandshakes":9,"closedPeers":4,"deadPeers":2,"deadPeerPercent":50.0,"trackerErrors":1,"trackerSuccesses":2,"goodPieces":3,"badPieces":1,"ramUsedBytes":10,"ramPeakBytes":20,"ramLimitBytes":100,"storageErrors":0,"pressureLevel":"warning","pressureReason":"test","payloadPersistence":"ram-verify-discard"}"#;
        let event: SidecarMetrics = serde_json::from_str(line).unwrap();
        let snapshot = event.into_snapshot();
        assert_eq!(snapshot.seq, 7);
        assert_eq!(snapshot.phase, PtPhase::Downloading);
        assert_eq!(snapshot.pressure_level, PressureLevel::Warning);
        assert_eq!(snapshot.dead_peers, 2);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn embedded_sidecar_is_released_byte_for_byte() {
        let path = extract_embedded_sidecar().expect("embedded sidecar should be extractable");
        assert_eq!(std::fs::read(path).unwrap(), SIDECAR_BINARY);
    }
}
