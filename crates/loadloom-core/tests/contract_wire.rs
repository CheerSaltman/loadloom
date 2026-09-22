//! 契约守护测试 —— 把「Rust 线格式」与「前端 `src/bindings.ts`」机械对齐。
//!
//! 与只断言 Rust 自身期望值的普通单测不同，本测试会**真的去读前端契约文件**并
//! 解析其中的 `interface` 字段名与 `type` 字面量，再与 serde 实际产出的 JSON 键名
//! 逐一比对。因此只要两侧有一方漂移（改字段名、加字段、改枚举拼写、把可选字段
//! 变成非可选），本测试立刻失败 —— 无需依赖 `tauri-specta` 也能获得等价的护栏。
//!
//! 运行：`cargo test -p loadloom-core --test contract_wire`

use std::collections::BTreeSet;
use std::path::PathBuf;

use loadloom_core::{
    CoreError, EngineLimits, ErrorCount, HistoryPoint, LiveConfigPatch, LogEntry, LogLevel,
    MetricsSnapshot, NicAdapterClass, NicAdapterDto, NicAdapterHistory, NicEvent, NicEventKind,
    NicLinkState, NicSeriesPoint, NicSnapshot, RunEvent, RunEventKind, RunPhase, StartRunRequest,
};
use serde::Serialize;

// ---------------------------------------------------------------------------
// bindings.ts 解析器
// ---------------------------------------------------------------------------

fn bindings_source() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("src")
        .join("bindings.ts");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("无法读取前端契约文件 {}: {error}", path.display()))
}

/// 提取 `export interface NAME { ... }` 中的字段名集合。
fn ts_interface_fields(name: &str) -> BTreeSet<String> {
    let source = bindings_source();
    let header = format!("export interface {name} ");
    let start = source
        .find(&header)
        .unwrap_or_else(|| panic!("bindings.ts 中找不到 interface {name}"));
    let body_start = source[start..]
        .find('{')
        .map(|offset| start + offset + 1)
        .expect("interface 缺少左花括号");
    let body_end = source[body_start..]
        .find("\n}")
        .map(|offset| body_start + offset)
        .expect("interface 缺少右花括号");

    source[body_start..body_end]
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty()
                || line.starts_with("//")
                || line.starts_with('*')
                || line.starts_with("/*")
            {
                return None;
            }
            let (key, _type) = line.split_once(':')?;
            let key = key.trim();
            if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return None;
            }
            Some(key.to_owned())
        })
        .collect()
}

/// 提取 `export type NAME = "a" | "b";` 中的字符串字面量集合。
fn ts_string_union(name: &str) -> BTreeSet<String> {
    let source = bindings_source();
    let header = format!("export type {name} =");
    let start = source
        .find(&header)
        .unwrap_or_else(|| panic!("bindings.ts 中找不到 type {name}"));
    let body_start = start + header.len();
    let body_end = source[body_start..]
        .find(';')
        .map(|offset| body_start + offset)
        .expect("type 别名缺少分号");

    source[body_start..body_end]
        .split('|')
        .map(|piece| piece.trim())
        .filter_map(|piece| {
            let piece = piece.strip_prefix('"')?.strip_suffix('"')?;
            Some(piece.to_owned())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Rust 线格式提取器
// ---------------------------------------------------------------------------

/// 序列化后取 JSON 对象的顶层键名集合。
fn wire_keys<T: Serialize>(value: &T) -> BTreeSet<String> {
    match serde_json::to_value(value).expect("序列化失败") {
        serde_json::Value::Object(map) => map.into_iter().map(|(key, _)| key).collect(),
        other => panic!("期望 JSON 对象，实际得到 {other}"),
    }
}

/// 序列化后取字符串字面量（用于枚举）。
fn wire_literal<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .expect("序列化失败")
        .as_str()
        .expect("期望 JSON 字符串")
        .to_owned()
}

/// 全字段归零的快照，供「可选字段线格式」测试做基线（`MetricsSnapshot` 自身未派生 `Default`）。
fn empty_snapshot() -> MetricsSnapshot {
    MetricsSnapshot {
        seq: 0,
        phase: RunPhase::Idle,
        url: String::new(),
        threads: 0,
        rate_mib: 0.0,
        rate_limited: false,
        elapsed_secs: 0.0,
        total_bytes: 0,
        speed_bps: 0.0,
        peak_bps: 0.0,
        avg_bps: 0.0,
        latency_ms: 0.0,
        jitter_ms: 0.0,
        completed: 0,
        failures: 0,
        success_rate: 0.0,
        in_flight: 0,
        errors: Vec::new(),
        last_error: String::new(),
        status: String::new(),
        history: Vec::new(),
        latest: None,
    }
}

fn assert_aligned<T: Serialize>(rust_name: &str, ts_name: &str, sample: &T) {
    let rust = wire_keys(sample);
    let ts = ts_interface_fields(ts_name);
    assert_eq!(
        rust,
        ts,
        "\n契约漂移：{rust_name} (Rust) 与 {ts_name} (bindings.ts) 字段不一致\n  仅 Rust 有: {:?}\n  仅 TS 有  : {:?}\n",
        rust.difference(&ts).collect::<Vec<_>>(),
        ts.difference(&rust).collect::<Vec<_>>(),
    );
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[test]
fn struct_wire_fields_match_typescript() {
    assert_aligned(
        "EngineLimits",
        "EngineLimits",
        &EngineLimits {
            max_workers: 32,
            max_rate_mib: 4096.0,
            history_len: 120,
            tick_ms: 250,
        },
    );

    assert_aligned(
        "StartRunRequest",
        "StartRunRequest",
        &StartRunRequest {
            url: "https://example.com".into(),
            threads: 4,
            rate_mib: 0.0,
            limit_gb: 1.0,
            limit_minutes: 0.0,
            authorized: true,
        },
    );

    // 两个字段都不为 None，否则 serde 默认会省略 -> 键名比对失去意义。
    assert_aligned(
        "LiveConfigPatch",
        "LiveConfigPatch",
        &LiveConfigPatch {
            threads: Some(8),
            rate_mib: Some(100.0),
        },
    );

    assert_aligned(
        "HistoryPoint",
        "HistoryPoint",
        &HistoryPoint {
            speed_bps: 1.0,
            latency_ms: 2.0,
            jitter_ms: 3.0,
        },
    );

    assert_aligned(
        "ErrorCount",
        "ErrorCount",
        &ErrorCount {
            code: "TIMEOUT".into(),
            count: 7,
        },
    );

    assert_aligned(
        "LogEntry",
        "LogEntry",
        &LogEntry {
            level: LogLevel::Info,
            code: "RUN-005".into(),
            source: "crates/loadloom-core/src/engine.rs:1".into(),
            message: "hello".into(),
            at_ms: 42,
        },
    );

    assert_aligned(
        "RunEvent",
        "RunEvent",
        &RunEvent {
            kind: RunEventKind::Started,
            code: "RUN-001".into(),
            source: "crates/loadloom-core/src/engine.rs:1".into(),
            message: "go".into(),
            at_ms: 42,
        },
    );
}

#[test]
fn metrics_snapshot_frame_matches_typescript() {
    assert_aligned(
        "MetricsSnapshot",
        "MetricsSnapshot",
        &MetricsSnapshot {
            seq: 1,
            phase: RunPhase::Running,
            url: "https://example.com".into(),
            threads: 4,
            rate_mib: 0.0,
            rate_limited: false,
            elapsed_secs: 1.5,
            total_bytes: 1024,
            speed_bps: 2048.0,
            peak_bps: 4096.0,
            avg_bps: 1024.0,
            latency_ms: 12.0,
            jitter_ms: 1.0,
            completed: 3,
            failures: 0,
            success_rate: 100.0,
            in_flight: 1,
            errors: vec![ErrorCount {
                code: "TIMEOUT".into(),
                count: 1,
            }],
            last_error: String::new(),
            status: "运行中".into(),
            history: vec![HistoryPoint {
                speed_bps: 1.0,
                latency_ms: 2.0,
                jitter_ms: 3.0,
            }],
            // 必须是 Some —— 否则 serde 会省略键，`latest: HistoryPoint | null` 就失去校验。
            latest: Some(HistoryPoint {
                speed_bps: 1.0,
                latency_ms: 2.0,
                jitter_ms: 3.0,
            }),
        },
    );
}

#[test]
fn option_fields_serialize_as_explicit_null_not_omitted() {
    // 前端 TS 里 `latest: HistoryPoint | null` 要求键**始终存在**且可为 null；
    // 若 Rust 侧 someday 加上 skip_serializing_if，键会消失 -> 这里拦住。
    let snapshot = MetricsSnapshot {
        latest: None,
        ..empty_snapshot()
    };
    let value = serde_json::to_value(&snapshot).unwrap();
    assert!(
        value.get("latest").is_some(),
        "latest 在 None 时必须序列化为显式 null，而不是被省略"
    );
    assert!(value["latest"].is_null());

    let patch = LiveConfigPatch {
        threads: None,
        rate_mib: None,
    };
    let value = serde_json::to_value(&patch).unwrap();
    assert_eq!(
        value,
        serde_json::json!({ "threads": null, "rateMib": null }),
        "LiveConfigPatch 的空值必须显式成对出现"
    );
}

#[test]
fn enum_literals_match_typescript_unions() {
    for (rust, literal) in [(RunPhase::Idle, "idle"), (RunPhase::Running, "running")] {
        assert_eq!(wire_literal(&rust), literal);
    }
    assert_eq!(
        ts_string_union("RunPhase"),
        BTreeSet::from(["idle".to_owned(), "running".to_owned()])
    );
    assert_eq!(
        ts_string_union("RunPhase"),
        [RunPhase::Idle, RunPhase::Running]
            .iter()
            .map(wire_literal)
            .collect::<BTreeSet<_>>()
    );

    for (rust, literal) in [
        (LogLevel::Info, "info"),
        (LogLevel::Warn, "warn"),
        (LogLevel::Error, "error"),
    ] {
        assert_eq!(wire_literal(&rust), literal);
    }
    assert_eq!(
        ts_string_union("LogLevel"),
        [LogLevel::Info, LogLevel::Warn, LogLevel::Error]
            .iter()
            .map(wire_literal)
            .collect::<BTreeSet<_>>()
    );

    for (rust, literal) in [
        (RunEventKind::Started, "started"),
        (RunEventKind::Stopped, "stopped"),
        (RunEventKind::AutoStopped, "autoStopped"),
        (RunEventKind::Rejected, "rejected"),
    ] {
        assert_eq!(wire_literal(&rust), literal);
    }
    assert_eq!(
        ts_string_union("RunEventKind"),
        [
            RunEventKind::Started,
            RunEventKind::Stopped,
            RunEventKind::AutoStopped,
            RunEventKind::Rejected,
        ]
        .iter()
        .map(wire_literal)
        .collect::<BTreeSet<_>>()
    );
}

#[test]
fn core_error_is_externally_tagged_matching_typescript() {
    // TS: type CoreError = { invalidInput: string } | { notAuthorized: string } | ...
    let cases: [(CoreError, &str); 4] = [
        (CoreError::InvalidInput("bad url".into()), "invalidInput"),
        (CoreError::NotAuthorized("nope".into()), "notAuthorized"),
        (CoreError::AlreadyRunning("busy".into()), "alreadyRunning"),
        (CoreError::Internal("boom".into()), "internal"),
    ];

    // bindings.ts 中 describeCoreError 的判别键顺序也必须一致。
    let ts_discriminants = {
        let source = bindings_source();
        let start = source
            .find("for (const key of [")
            .expect("找不到判别键列表");
        let end = source[start..].find(']').map(|o| start + o).unwrap();
        source[start..end]
            .split('"')
            .filter(|piece| !piece.contains('[') && !piece.contains("for"))
            .map(|piece| piece.trim().trim_end_matches(',').to_owned())
            .filter(|piece| !piece.is_empty())
            .collect::<Vec<_>>()
    };

    for (index, (error, tag)) in cases.iter().enumerate() {
        let value = serde_json::to_value(error).unwrap();
        let map = value.as_object().expect("CoreError 应为单键对象");
        assert_eq!(map.len(), 1, "CoreError 变体必须只有一个键");
        assert!(map.contains_key(*tag), "缺少判别键 {tag}，实际 {map:?}");
        assert_eq!(
            ts_discriminants.get(index).map(String::as_str),
            Some(*tag),
            "describeCoreError 的判别键顺序与 Rust 变体顺序不一致"
        );
    }
}

#[test]
fn start_run_request_defaults_are_backend_owned() {
    // 前端只传 url / threads / authorized 时，其余字段必须由 serde 默认值兜底。
    let request: StartRunRequest = serde_json::from_value(serde_json::json!({
        "url": "https://example.com",
        "threads": 8,
        "rateMib": 0.0,
        "limitGb": 0.0,
        "limitMinutes": 0.0,
        "authorized": true
    }))
    .expect("前端形状的载荷必须能反序列化");

    assert_eq!(request.threads, 8);
    assert!(request.authorized);
    assert_eq!(request.rate_mib, 0.0);

    // 最小载荷：只给 url + authorized。
    let minimal: StartRunRequest = serde_json::from_value(serde_json::json!({
        "url": "https://example.com",
        "authorized": true
    }))
    .expect("最小载荷必须能反序列化");
    assert_eq!(minimal.threads, 0);
    assert_eq!(minimal.limit_gb, 0.0);
    assert_eq!(minimal.limit_minutes, 0.0);
}

#[test]
fn every_contract_type_is_covered_by_a_bindings_interface() {
    // 防止「Rust 新增了 DTO 但忘了在 bindings.ts 里镜像」。
    let source = bindings_source();
    for name in [
        "EngineLimits",
        "StartRunRequest",
        "LiveConfigPatch",
        "HistoryPoint",
        "ErrorCount",
        "MetricsSnapshot",
        "LogEntry",
        "RunEvent",
        "NicSeriesPoint",
        "NicAdapterHistory",
        "NicAdapterDto",
        "NicEvent",
        "NicSnapshot",
    ] {
        assert!(
            source.contains(&format!("export interface {name}")),
            "bindings.ts 缺少 export interface {name}"
        );
    }
    for name in [
        "RunPhase",
        "LogLevel",
        "RunEventKind",
        "NicAdapterClass",
        "NicLinkState",
        "NicEventKind",
    ] {
        assert!(
            source.contains(&format!("export type {name} =")),
            "bindings.ts 缺少 export type {name}"
        );
    }
    assert!(source.contains("export type CoreError ="));
}

// ---------------------------------------------------------------------------
// 网卡监测
// ---------------------------------------------------------------------------

/// 一块「什么都在跑」的网卡：所有可选/实时字段都非零，否则键名比对会失去意义。
fn sample_adapter() -> NicAdapterDto {
    NicAdapterDto {
        id: "luid-0000000000000001".into(),
        name: "以太网".into(),
        description: "Intel(R) Ethernet Controller I225-V".into(),
        class: NicAdapterClass::Physical,
        media: "以太网".into(),
        mac: "AA-BB-CC-DD-EE-FF".into(),
        mtu: 1500,
        link_state: NicLinkState::Connected,
        oper_status: "已启用".into(),
        admin_enabled: true,
        transmit_speed_bps: 1_000_000_000.0,
        receive_speed_bps: 1_000_000_000.0,
        monitored: true,
        selected: true,
        rx_bps: 1_048_576.0,
        tx_bps: 2_097_152.0,
        rx_utilization: 0.8,
        tx_utilization: 1.6,
        out_queue_len: 0,
        queue_backlog: false,
        rx_discards_per_sec: 0.0,
        tx_discards_per_sec: 0.0,
        rx_errors_per_sec: 0.0,
        tx_errors_per_sec: 0.0,
        rx_bytes_total: 4_000_000_000,
        tx_bytes_total: 1_000_000_000,
        rx_discards_total: 3,
        tx_discards_total: 1,
        rx_errors_total: 0,
        tx_errors_total: 2,
        rx_unknown_protos_total: 7,
    }
}

#[test]
fn nic_wire_frames_match_typescript() {
    assert_aligned(
        "NicSeriesPoint",
        "NicSeriesPoint",
        &NicSeriesPoint {
            rx_bps: 1.0,
            tx_bps: 2.0,
            rx_utilization: 3.0,
            tx_utilization: 4.0,
        },
    );

    assert_aligned(
        "NicAdapterHistory",
        "NicAdapterHistory",
        &NicAdapterHistory {
            id: "luid-0000000000000001".into(),
            points: vec![NicSeriesPoint {
                rx_bps: 1.0,
                tx_bps: 2.0,
                rx_utilization: 3.0,
                tx_utilization: 4.0,
            }],
        },
    );

    assert_aligned("NicAdapterDto", "NicAdapterDto", &sample_adapter());

    assert_aligned(
        "NicEvent",
        "NicEvent",
        &NicEvent {
            at_ms: 1_500,
            adapter_id: "luid-0000000000000001".into(),
            adapter: "以太网".into(),
            kind: NicEventKind::LinkDown,
            code: "NIC-010".into(),
            resolved: false,
            message: "网卡「以太网」链路断开".into(),
        },
    );

    assert_aligned(
        "NicSnapshot",
        "NicSnapshot",
        &NicSnapshot {
            seq: 1,
            supported: true,
            note: "可获得：……".into(),
            sampling: true,
            sample_ms: 500,
            selection: vec!["luid-0000000000000001".into()],
            adapters: vec![sample_adapter()],
            events: vec![NicEvent {
                at_ms: 1_500,
                adapter_id: "luid-0000000000000001".into(),
                adapter: "以太网".into(),
                kind: NicEventKind::LinkUp,
                code: "NIC-011".into(),
                resolved: false,
                message: "网卡「以太网」链路恢复".into(),
            }],
            last_error: String::new(),
            history: vec![NicAdapterHistory {
                id: "luid-0000000000000001".into(),
                points: Vec::new(),
            }],
        },
    );
}

#[test]
fn nic_enums_match_typescript() {
    for (rust, literal) in [
        (NicAdapterClass::Physical, "physical"),
        (NicAdapterClass::Virtual, "virtual"),
        (NicAdapterClass::Loopback, "loopback"),
        (NicAdapterClass::Tunnel, "tunnel"),
        (NicAdapterClass::Other, "other"),
    ] {
        assert_eq!(wire_literal(&rust), literal);
    }
    assert_eq!(
        ts_string_union("NicAdapterClass"),
        [
            NicAdapterClass::Physical,
            NicAdapterClass::Virtual,
            NicAdapterClass::Loopback,
            NicAdapterClass::Tunnel,
            NicAdapterClass::Other,
        ]
        .iter()
        .map(wire_literal)
        .collect::<BTreeSet<_>>()
    );

    for (rust, literal) in [
        (NicLinkState::Connected, "connected"),
        (NicLinkState::Disconnected, "disconnected"),
        (NicLinkState::Dormant, "dormant"),
        (NicLinkState::NotPresent, "notPresent"),
        (NicLinkState::Unknown, "unknown"),
    ] {
        assert_eq!(wire_literal(&rust), literal);
    }
    assert_eq!(
        ts_string_union("NicLinkState"),
        [
            NicLinkState::Connected,
            NicLinkState::Disconnected,
            NicLinkState::Dormant,
            NicLinkState::NotPresent,
            NicLinkState::Unknown,
        ]
        .iter()
        .map(wire_literal)
        .collect::<BTreeSet<_>>()
    );

    for (rust, literal) in [
        (NicEventKind::LinkUp, "linkUp"),
        (NicEventKind::LinkDown, "linkDown"),
        (NicEventKind::SpeedChange, "speedChange"),
        (NicEventKind::CounterReset, "counterReset"),
        (NicEventKind::DiscardSpike, "discardSpike"),
        (NicEventKind::ErrorSpike, "errorSpike"),
        (NicEventKind::QueueBacklog, "queueBacklog"),
        (NicEventKind::AdapterAdded, "adapterAdded"),
        (NicEventKind::AdapterRemoved, "adapterRemoved"),
        (NicEventKind::Selection, "selection"),
    ] {
        assert_eq!(wire_literal(&rust), literal);
    }
    assert_eq!(
        ts_string_union("NicEventKind"),
        [
            NicEventKind::LinkUp,
            NicEventKind::LinkDown,
            NicEventKind::SpeedChange,
            NicEventKind::CounterReset,
            NicEventKind::DiscardSpike,
            NicEventKind::ErrorSpike,
            NicEventKind::QueueBacklog,
            NicEventKind::AdapterAdded,
            NicEventKind::AdapterRemoved,
            NicEventKind::Selection,
        ]
        .iter()
        .map(wire_literal)
        .collect::<BTreeSet<_>>()
    );
}
