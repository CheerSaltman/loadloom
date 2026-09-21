//! 最小可运行示例：在**没有 Tokio 运行时、没有窗口、没有事件循环**的普通线程上
//! 启动一次打流。
//!
//! 这个示例存在的意义是把 `traffic-core` 的核心约束钉死成可执行的文档：
//! 引擎不假设调用方线程处于任何运行时上下文中 —— 而这个隐含假设曾经导致
//! 生产事故（点「开始」整个进程闪退）。任何破坏该约束的改动都会让这个示例编译
//! 或运行失败。
//!
//! 运行：
//! ```text
//! cargo run --example headless_smoke -p traffic-core
//! ```
//!
//! 目标地址指向本机回环上一个**关闭**的端口：示例因此不需要外网、不触碰任何
//! 第三方服务，同时也顺带演示了「目标不可达时引擎只记错误、不会崩」。
//! 换成真实目标时，请确认你有该目标的**明确授权**。

use std::time::Duration;

use traffic_core::{Engine, StartRunRequest};

fn main() {
    // 前置断言：本示例刻意跑在一个裸线程上（main 线程没有 Tokio 运行时）。
    // 如果这里失败，说明有人给 main 套了 #[tokio::main] 之类的运行时，
    // 那就不再是在验证「无运行时上下文也能启动」这件事了。
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "本示例必须运行在没有 Tokio 运行时的线程上，否则它证明不了任何东西"
    );

    let engine = Engine::spawn();
    println!("引擎已创建（自带执行器，不依赖调用方线程的运行时上下文）");

    engine
        .start(StartRunRequest {
            // 回环 + 关闭端口：安全、离线、可重复。
            url: "http://127.0.0.1:9/payload".to_owned(),
            threads: 4,
            rate_mib: 0.0, // 0 = 不限速
            limit_gb: 0.0, // 0 = 不按流量停止
            limit_minutes: 0.0,
            authorized: true,
        })
        .expect("启动打流不应失败（URL 合法且已声明授权）");

    // 轮询等待，而不是写死 sleep：机器快慢不影响结论。
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
        let snap = engine.snapshot(false);
        println!(
            "阶段={:?} 在飞={} 已传字节={} 失败={}",
            snap.phase,
            snap.in_flight,
            snap.total_bytes,
            snap.errors.iter().map(|e| e.count).sum::<u64>()
        );
        if snap.total_bytes > 0 {
            break;
        }
    }

    engine.stop("示例结束");
    println!("已停止，最终阶段={:?}", engine.snapshot(false).phase);
}
