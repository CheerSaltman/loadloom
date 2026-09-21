/**
 * ⚠️ 契约镜像文件 —— 与 `crates/traffic-core/src/contract.rs` **逐字段对齐**。
 *
 * 单一事实来源是 Rust 端；本文件由契约测试 `cargo test -p traffic-core --test contract_wire`
 * 守护（该测试逐字段断言 JSON 线格式，字段一旦漂移即失败）。
 * 若后续引入 `tauri-specta`，仅需用自动生成结果覆盖本文件，调用侧代码零改动。
 */

// ------------------------------ DTO ------------------------------

/** Command `get_limits` 返回。 */
export interface EngineLimits {
  maxWorkers: number;
  maxRateMib: number;
  historyLen: number;
  tickMs: number;
}

/** Command `start_run` 入参。 */
export interface StartRunRequest {
  url: string;
  threads: number;
  rateMib: number;
  limitGb: number;
  limitMinutes: number;
  authorized: boolean;
}

/** Command `set_live_config` 入参（null 表示保持现状）。 */
export interface LiveConfigPatch {
  threads: number | null;
  rateMib: number | null;
}

/** 采样点。 */
export interface HistoryPoint {
  speedBps: number;
  latencyMs: number;
  jitterMs: number;
}

/** 错误码聚合项。 */
export interface ErrorCount {
  code: string;
  count: number;
}

export type RunPhase = "idle" | "running";

/** Command `get_snapshot` 返回 / Event Channel 帧。 */
export interface MetricsSnapshot {
  seq: number;
  phase: RunPhase;
  url: string;
  threads: number;
  rateMib: number;
  rateLimited: boolean;
  elapsedSecs: number;
  totalBytes: number;
  speedBps: number;
  peakBps: number;
  avgBps: number;
  latencyMs: number;
  jitterMs: number;
  completed: number;
  failures: number;
  successRate: number;
  inFlight: number;
  errors: ErrorCount[];
  lastError: string;
  status: string;
  history: HistoryPoint[];
  latest: HistoryPoint | null;
}

export type LogLevel = "info" | "warn" | "error";

/** Event `traffic://log` 载荷。 */
export interface LogEntry {
  level: LogLevel;
  message: string;
  atMs: number;
}

export type RunEventKind = "started" | "stopped" | "autoStopped" | "rejected";

/** Event `traffic://run-event` 载荷。 */
export interface RunEvent {
  kind: RunEventKind;
  message: string;
  atMs: number;
}

/** 强类型错误（Rust `CoreError` 的外部标记枚举线格式）。 */
export type CoreError =
  | { invalidInput: string }
  | { notAuthorized: string }
  | { alreadyRunning: string }
  | { internal: string };

/** 把 `CoreError` 还原为人类可读文案 + 判别标签。 */
export function describeCoreError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object") {
    const record = error as Record<string, string>;
    for (const key of ["invalidInput", "notAuthorized", "alreadyRunning", "internal"]) {
      if (typeof record[key] === "string") return record[key];
    }
  }
  return "发生未知错误";
}

// ------------------------------ Commands ------------------------------

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export const EVENT_LOG = "traffic://log";
export const EVENT_RUN = "traffic://run-event";

/** 请求-响应型 IPC。全部具备完整类型推断。 */
export const commands = {
  getLimits: () => invoke<EngineLimits>("get_limits"),
  getSnapshot: (withHistory: boolean) =>
    invoke<MetricsSnapshot>("get_snapshot", { withHistory }),
  startRun: (request: StartRunRequest) => invoke<void>("start_run", { request }),
  stopRun: (reason?: string) => invoke<void>("stop_run", { reason: reason ?? null }),
  setLiveConfig: (patch: LiveConfigPatch) =>
    invoke<void>("set_live_config", { patch }),
  quitApp: () => invoke<void>("quit_app"),

  /**
   * 指标推流订阅：后端每 TICK 主动推送，前端**不做任何轮询**。
   * 返回的 Promise 在通道建立（或失败）后 resolve。
   */
  subscribeMetrics: (onFrame: (frame: MetricsSnapshot) => void) => {
    const channel = new Channel<MetricsSnapshot>();
    channel.onmessage = onFrame;
    return invoke<void>("subscribe_metrics", { channel });
  },
};

/** 事件流订阅。 */
export const events = {
  onLog: (handler: (entry: LogEntry) => void): Promise<UnlistenFn> =>
    listen<LogEntry>(EVENT_LOG, (event) => handler(event.payload)),
  onRunEvent: (handler: (event: RunEvent) => void): Promise<UnlistenFn> =>
    listen<RunEvent>(EVENT_RUN, (event) => handler(event.payload)),
};

/** 事件名常量（供前端做类型安全的字符串引用）。 */
export const IPC = {
  events: { log: EVENT_LOG, runEvent: EVENT_RUN },
} as const;
