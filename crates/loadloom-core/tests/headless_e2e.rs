//! 无头端到端测试：**不打开任何窗口、不启动任何浏览器**，直接驱动业务引擎。
//!
//! 证明「剥离后的后端模块在无界面环境下可直接运行测试」这一硬性约束。

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

use loadloom_core::{CoreError, Engine, LiveConfigPatch, RunPhase, StartRunRequest};

/// 一个极简的本地 HTTP 服务：对每个连接回一个固定大小的响应体。
/// 用来在完全离线、无界面的条件下驱动真实打流链路。
async fn spawn_local_origin(body_len: usize) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地端口");
    let port = listener.local_addr().expect("读取本地地址").port();
    let body = vec![b'x'; body_len];
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let body = body.clone();
            tokio::spawn(async move {
                // 只读取请求头即可，随后立刻回包。
                let mut scratch = [0u8; 2048];
                let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut scratch).await;
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(&body).await;
                let _ = socket.flush().await;
            });
        }
    });
    port
}

fn request(url: String, authorized: bool) -> StartRunRequest {
    StartRunRequest {
        url,
        threads: 2,
        rate_mib: 0.0,
        limit_gb: 0.0,
        limit_minutes: 0.0,
        authorized,
    }
}

#[test]
fn rejects_empty_and_malformed_urls() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime.block_on(async {
        let engine = Engine::spawn();
        let empty = engine.start(request("   ".to_owned(), true));
        assert!(matches!(empty, Err(CoreError::InvalidInput(_))));

        let bad_scheme = engine.start(request("ftp://example.test/f".to_owned(), true));
        assert!(matches!(bad_scheme, Err(CoreError::InvalidInput(_))));
    });
}

#[test]
fn enforces_authorization_gate() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime.block_on(async {
        let engine = Engine::spawn();
        let denied = engine.start(request("https://example.test/f".to_owned(), false));
        assert!(matches!(denied, Err(CoreError::NotAuthorized(_))));
        assert!(!engine.is_running(), "未授权不应进入运行态");
    });
}

#[test]
fn blocks_second_start_while_running() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime.block_on(async {
        let port = spawn_local_origin(4096).await;
        let engine = Engine::spawn();
        engine
            .start(request(format!("http://127.0.0.1:{port}/blob"), true))
            .expect("首次启动应成功");

        let second = engine.start(request(format!("http://127.0.0.1:{port}/blob"), true));
        assert!(matches!(second, Err(CoreError::AlreadyRunning(_))));

        engine.stop("测试收尾");
    });
}

#[test]
fn concurrent_starts_admit_exactly_one_run() {
    let engine = Engine::spawn();
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let request = request("http://127.0.0.1:9/payload".to_owned(), true);

    let mut handles = Vec::new();
    for _ in 0..2 {
        let engine = Arc::clone(&engine);
        let barrier = Arc::clone(&barrier);
        let request = request.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            engine.start(request)
        }));
    }

    barrier.wait();
    let outcomes = handles
        .into_iter()
        .map(|handle| handle.join().expect("启动线程不应 panic"))
        .collect::<Vec<_>>();

    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Err(CoreError::AlreadyRunning(_))))
            .count(),
        1
    );
    engine.stop("测试结束");
}

#[test]
fn streams_real_traffic_without_any_ui() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime.block_on(async {
        let port = spawn_local_origin(8 * 1024).await;
        let engine: Arc<Engine> = Engine::spawn();
        let mut metrics = engine.subscribe_metrics();
        let mut logs = engine.subscribe_logs();

        engine
            .start(request(format!("http://127.0.0.1:{port}/blob"), true))
            .expect("启动成功");

        assert_eq!(engine.snapshot(false).phase, RunPhase::Running);

        // 等到累计流量出现，最多等 6 秒。
        let mut saw_bytes = false;
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if engine.snapshot(false).total_bytes > 0 {
                saw_bytes = true;
                break;
            }
        }
        assert!(saw_bytes, "无头引擎应当真实产生流量");

        // 推流而非轮询：必须能从广播通道收到指标帧与日志帧。
        let frame = tokio::time::timeout(Duration::from_secs(2), metrics.recv())
            .await
            .expect("应能在 2 秒内收到指标推流帧")
            .expect("通道未关闭");
        assert!(frame.seq > 0);
        assert_eq!(frame.phase, RunPhase::Running);

        assert!(
            tokio::time::timeout(Duration::from_secs(2), logs.recv())
                .await
                .is_ok(),
            "应能收到日志推流帧"
        );

        // 运行中动态调整：限速 + 并发
        engine.set_live(LiveConfigPatch {
            threads: Some(4),
            rate_mib: Some(1.0),
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        let live = engine.snapshot(false);
        assert_eq!(live.threads, 4);
        assert!(live.rate_limited);

        engine.stop("无头测试结束");
        let stopped = engine.snapshot(false);
        assert_eq!(stopped.phase, RunPhase::Idle);
        assert!(stopped.status.contains("无头测试结束"));
        assert_eq!(stopped.speed_bps, 0.0);

        // 握手快照携带历史曲线，且不超过环形上限。
        let handshake = engine.snapshot(true);
        assert!(handshake.history.len() <= loadloom_core::Engine::limits().history_len as usize);
    });
}

// ---------------------------------------------------------------------------
// 闪退回归测试
// ---------------------------------------------------------------------------

/// 极简**阻塞式** HTTP origin（不依赖任何异步运行时）。
///
/// 上面的 `spawn_local_origin` 需要 Tokio 运行时，而下面的回归测试恰恰必须在
/// *没有* 运行时的线程上运行，因此这里用 `std::net` 另起一个专用线程。
fn spawn_blocking_origin(body_len: usize) -> u16 {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定本地端口");
    let port = listener.local_addr().expect("读取本地地址").port();

    std::thread::spawn(move || {
        let body = vec![b'x'; body_len];
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {body_len}\r\nConnection: close\r\n\r\n"
        );
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };

            // 先读掉请求，再回包 —— 这一步不能省。
            //
            // Windows 上 closesocket 时，若接收缓冲区里还残留**未读数据**，内核会直接
            // 发 RST 而不是 FIN。对端看到的是「连接被重置」，且已经收到的字节会被丢弃。
            // 此前这个 origin 从不读请求，于是**每一个**连接都被 RST：worker 全部失败、
            // 退避 400ms 再重试，`total_bytes` 长时间为 0。这是测试脚手架自身的缺陷，
            // 与引擎无关（上面那个异步 origin 读了请求，所以它一直是绿的）。
            let mut scratch = [0u8; 2048];
            let mut seen = Vec::new();
            while seen.len() < 8192 {
                match stream.read(&mut scratch) {
                    Ok(0) => break,
                    Ok(n) => {
                        seen.extend_from_slice(&scratch[..n]);
                        if seen.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }

            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
    });

    port
}

/// **闪退回归测试**：在完全没有 Tokio 运行时上下文的裸线程上启动打流。
///
/// 事故复盘：Tauri v2 的**同步** command 运行在事件循环主线程上，而该线程没有
/// Tokio 运行时。旧实现的 `Engine::start` 内部直接调用 `tokio::spawn`，于是点击
/// 「开始」时 panic：
///
/// ```text
/// there is no reactor running, must be called from the context of a Tokio 1.x runtime
/// ```
///
/// 叠加 release profile 里的 `panic = "abort"`，整个桌面进程当场 abort —— 用户看到
/// 的就是「一点开始就闪退」，且不留任何线索。本测试把那个运行环境原样复现出来。
///
/// 关键点：本测试是**同步的**（`#[test]` 而非 `#[tokio::test]`），并在一开始就断言
/// 当前线程确实没有运行时 —— 否则这个测试就失去意义了。
#[test]
fn starts_from_a_thread_with_no_tokio_runtime() {
    let port = spawn_blocking_origin(64 * 1024);

    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "前置条件失败：本测试必须运行在没有 Tokio 运行时的线程上"
    );

    // 构造 + 启动都必须在无运行时环境下成功 —— 这正是原来的崩溃点。
    let t_spawn = Instant::now();
    let engine = Engine::spawn();
    println!(
        "Engine::spawn() 耗时 {:.1} ms",
        t_spawn.elapsed().as_secs_f64() * 1000.0
    );

    let t_start = Instant::now();
    engine
        .start(StartRunRequest {
            url: format!("http://127.0.0.1:{port}/payload"),
            threads: 4,
            rate_mib: 0.0,
            limit_gb: 0.0,
            limit_minutes: 0.0,
            authorized: true,
        })
        .expect("无运行时线程上启动打流不应失败");
    println!(
        "start() 耗时 {:.1} ms",
        t_start.elapsed().as_secs_f64() * 1000.0
    );

    // 真实跑一会儿，确认后台任务确实在跑、数据确实在流动。
    //
    // 这里用「轮询 + 截止时间」而**不是**固定 `sleep`：本测试跑在完全没有
    // 运行时的裸线程上，引擎要先自建 Tokio 运行时并真正拉起多线程调度，冷启动
    // 开销受机器负载影响很大。写死 600ms 等于把测试绑死在某一台机器的启动速度
    // 上，属于假失败。截止时间给足 10 秒，但没有数据照样判失败 —— 判据一点没放松。
    assert_eq!(engine.snapshot(false).phase, RunPhase::Running);

    let deadline = Instant::now() + Duration::from_secs(10);
    let first_byte_started = Instant::now();
    let mut snapshot = engine.snapshot(false);
    while snapshot.total_bytes == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
        snapshot = engine.snapshot(false);
    }
    println!(
        "首字节耗时 {:.1} ms（本机 localhost，不限速）",
        first_byte_started.elapsed().as_secs_f64() * 1000.0
    );

    assert!(
        snapshot.total_bytes > 0,
        "后台 worker 应已在无运行时上下文的线程上跑起来并拉到真实数据；\
         等待 10 秒仍为 0 字节（phase={:?}、in_flight={}、failures={}、最近错误={:?}）",
        snapshot.phase,
        snapshot.in_flight,
        snapshot.failures,
        snapshot
            .errors
            .last()
            .map(|e| format!("{} ×{}", e.code, e.count)),
    );

    engine.stop("闪退回归测试结束");
    assert_eq!(engine.snapshot(false).phase, RunPhase::Idle);
}
