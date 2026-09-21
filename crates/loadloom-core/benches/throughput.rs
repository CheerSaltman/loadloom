//! 吞吐标定台：本机回环、**不限速**条件下的真实拉取速率与并发伸缩性。
//!
//! 默认 `#[ignore]`，需要显式运行：
//!
//! ```text
//! cargo test -p loadloom-core --test throughput -- --ignored --nocapture
//! ```
//!
//! 它会连续跑多组并发并占满 CPU 数秒，所以不进默认测试集（默认集必须又快又稳）。
//!
//! 这个文件提供的是一个**可读数的观测点**：同一台机器、同一个 origin、连着跑
//! 1/2/4/8/16/32 并发，直接看速率与伸缩比。
//!
//! ⚠️ 已知局限（实测得出，不是推测）：
//!
//! * 在本机回环上，4 → 8 并发**本来就只涨 0~15%**，1 → 32 并发总共也只有约 2.5×。
//!   瓶颈在回环链路与每请求固定开销，不在引擎。
//! * 因此**不要**用这个台子去验证「引擎并发优化有没有效」：把 `Executor` 里写死的
//!   `worker_threads(2)` 加回去，这里的数字几乎不变（32 并发 52.3 vs 54.2 MiB/s）。
//!   这条更正很重要 —— 早先版本的本文件注释断言「加回去数字会立刻塌下来」，
//!   那是错的，已被实测推翻。
//!
//! 要复现「用户端吞吐腰斩」这类问题，必须拿**真实目标**去测；回环标定台只能回答
//! 「引擎能不能跑起来、速率会不会随并发单调上升」。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use loadloom_core::{Engine, StartRunRequest};

/// 单次响应体大小。取 256 KiB：既不是小包（避免被每请求固定开销主导），
/// 也不是大包（避免请求数太少、压不出并发调度的差异）。
const BODY_LEN: usize = 256 * 1024;

/// 每组并发的采样时长（预热另计）。
const RUN_FOR: Duration = Duration::from_secs(3);

/// 预热时长：让连接池、调度器进入稳态，避免把冷启动算进速率。
const WARMUP: Duration = Duration::from_millis(600);

/// 在**独立线程的独立运行时**上起一个高并发本地 origin。
///
/// 与引擎的运行时分开：否则两者抢同一组 worker 线程，测出来的数字既不是
/// 引擎的吞吐，也不是 origin 的吞吐。
fn spawn_fast_origin(body_len: usize) -> (u16, Arc<tokio::runtime::Runtime>) {
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("throughput-origin")
            .enable_all()
            .build()
            .expect("origin 运行时创建失败"),
    );

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定本地端口");
    let port = listener.local_addr().expect("读取本地地址").port();
    listener.set_nonblocking(true).expect("切换非阻塞");

    // 探针：origin 侧到底收了多少连接、回了多少、有多少卡在读请求上。
    let accepted = Arc::new(AtomicU64::new(0));
    let served = Arc::new(AtomicU64::new(0));
    let stalled = Arc::new(AtomicU64::new(0));
    {
        let (accepted, served, stalled) = (
            Arc::clone(&accepted),
            Arc::clone(&served),
            Arc::clone(&stalled),
        );
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(1000));
            eprintln!(
                "[origin] 已接受 {} · 已回包 {} · 读请求未收齐 {}",
                accepted.load(Ordering::Relaxed),
                served.load(Ordering::Relaxed),
                stalled.load(Ordering::Relaxed)
            );
        });
    }

    let accepted_task = Arc::clone(&accepted);
    let served_task = Arc::clone(&served);
    let stalled_task = Arc::clone(&stalled);

    runtime.spawn(async move {
        let listener = TcpListener::from_std(listener).expect("接管监听套接字");
        let body = vec![b'x'; body_len];
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let body = body.clone();
            let header = header.clone();
            let accepted = Arc::clone(&accepted_task);
            let served = Arc::clone(&served_task);
            let stalled = Arc::clone(&stalled_task);
            accepted.fetch_add(1, Ordering::Relaxed);
            tokio::spawn(async move {
                // **长连接**：同一连接上反复「读请求 → 回包」。
                //
                // 早期版本每条请求都用一条新连接（响应里带 Connection: close），在
                // Windows 回环上会踩到「关闭时缓冲区仍有未读数据 → RST」的坑，表现
                // 为引擎侧大量 DECODE_ERROR（响应体解码不完整）与低到离谱的速率。
                // 真实 HTTP 服务都是长连接，这里照做，顺带把这类噪声从测量里剔除。
                let mut scratch = [0u8; 2048];
                let mut seen = Vec::new();
                loop {
                    seen.clear();
                    let mut got_full_request = false;
                    while seen.len() < 8192 {
                        match socket.read(&mut scratch).await {
                            Ok(0) => break,
                            Ok(n) => {
                                seen.extend_from_slice(&scratch[..n]);
                                if seen.windows(4).any(|w| w == b"\r\n\r\n") {
                                    got_full_request = true;
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    if !got_full_request {
                        stalled.fetch_add(1, Ordering::Relaxed);
                        break;
                    }
                    if socket.write_all(header.as_bytes()).await.is_err() {
                        break;
                    }
                    if socket.write_all(&body).await.is_err() {
                        break;
                    }
                    if socket.flush().await.is_err() {
                        break;
                    }
                    served.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });

    (port, runtime)
}

/// 在给定的并发下测一段时间的稳态速率，返回 MiB/s。
fn measure(port: u16, threads: u32) -> f64 {
    // 从**裸线程**（无 Tokio 运行时）构造引擎：这正是 Tauri 同步 command
    // 所处的环境，也是 `Executor` 自建运行时的那条路径。
    let engine = Engine::spawn();
    engine
        .start(StartRunRequest {
            url: format!("http://127.0.0.1:{port}/blob"),
            threads,
            rate_mib: 0.0,
            limit_gb: 0.0,
            limit_minutes: 0.0,
            authorized: true,
        })
        .expect("启动打流");

    std::thread::sleep(WARMUP);

    let before = engine.snapshot(false).total_bytes;
    let started = Instant::now();
    std::thread::sleep(RUN_FOR);
    let elapsed = started.elapsed().as_secs_f64();
    let snapshot = engine.snapshot(false);
    let bytes = snapshot.total_bytes - before;

    engine.stop("吞吐标定结束");
    let _ = &engine;
    drop(engine);

    let mib_per_sec = bytes as f64 / elapsed / 1_048_576.0;
    println!(
        "并发 {threads:>2} → {mib_per_sec:8.1} MiB/s   \
         （{} 字节 / {elapsed:.2} s，完成 {} 次，失败 {} 次，在飞 {}）",
        bytes, snapshot.completed, snapshot.failures, snapshot.in_flight
    );
    if !snapshot.errors.is_empty() {
        let top: Vec<String> = snapshot
            .errors
            .iter()
            .map(|e| format!("{}×{}", e.code, e.count))
            .collect();
        println!("          错误明细：{}", top.join("  "));
    }
    mib_per_sec
}

/// 允许用**外部参照 origin**跑标定：设置 `TC_ORIGIN_PORT` 即可。
///
/// 用途：把「origin 不够快」这个变量从测量里彻底剔除。内置 origin 是手写的极简实现，
/// 一旦数字难看，第一嫌疑永远是它而不是引擎；换一个成熟的 HTTP 服务（如 node http）
/// 就能一刀切开「是谁的问题」。
fn reference_origin_port() -> Option<u16> {
    std::env::var("TC_ORIGIN_PORT").ok()?.trim().parse().ok()
}

#[test]
#[ignore = "吞吐标定：需 --ignored 显式运行，会占满 CPU 数秒"]
fn throughput_scales_with_threads() {
    let (port, origin, origin_label) = match reference_origin_port() {
        Some(port) => (port, None, format!("外部参照 origin（端口 {port}）")),
        None => {
            let (port, runtime) = spawn_fast_origin(BODY_LEN);
            (port, Some(runtime), "内置 origin".to_owned())
        }
    };
    println!(
        "{origin_label} · 响应体 {} KiB · 每组采样 {:.1}s · 不限速\n",
        BODY_LEN / 1024,
        RUN_FOR.as_secs_f64()
    );

    let mut results = Vec::new();
    for threads in [1_u32, 2, 4, 8, 16, 32] {
        results.push((threads, measure(port, threads)));
    }

    println!("\n--- 伸缩性小结 ---");
    let single = results.first().map(|(_, v)| *v).unwrap_or(0.0);
    for (threads, mib_per_sec) in &results {
        let speedup = if single > 0.0 {
            mib_per_sec / single
        } else {
            0.0
        };
        println!("{threads:>2} 线程：{mib_per_sec:8.1} MiB/s（相对 1 线程 {speedup:5.2}×）");
    }

    drop(origin);
}
