import { useCallback, useEffect, useRef, useState } from "react";
import {
  commands,
  describeCoreError,
  events,
  type EngineLimits,
  type FrontendErrorReport,
  type HistoryPoint,
  type LiveConfigPatch,
  type LogEntry,
  type MetricsSnapshot,
  type RunEvent,
  type StartRunRequest,
} from "@/bindings";

export type ConnectionState = "connecting" | "live" | "down";

/**
 * 前端日志缓冲区上限。
 *
 * 无上限的数组在一场长跑里会被网络失败日志灌满（32 个 worker × 每秒多条），
 * 最终把渲染拖垮 —— 排障工具不能变成新的故障源。超出后丢弃最旧的条目：
 * 完整历史始终在磁盘日志里（`%LOCALAPPDATA%\LoadLoom\logs`）。
 */
const LOG_BUFFER_LIMIT = 2000;

export interface EngineState {
  limits: EngineLimits | null;
  snapshot: MetricsSnapshot | null;
  history: HistoryPoint[];
  logs: LogEntry[];
  /** 实际生效的日志文件路径（用于界面展示「日志在哪」）。 */
  logPath: string | null;
  lastEvent: RunEvent | null;
  connection: ConnectionState;
  error: string | null;
  start: (request: StartRunRequest) => Promise<void>;
  stop: (reason?: string) => Promise<void>;
  patchLive: (patch: LiveConfigPatch) => Promise<void>;
  clearLogs: () => void;
  /** 上报前端异常（写盘 + 崩溃报告），失败静默 —— 报错通道自身不该再弹错。 */
  reportError: (report: FrontendErrorReport) => void;
  quit: () => Promise<void>;
}

/**
 * 引擎状态订阅 Hook。
 *
 * - 指标：通过 `Channel` 由后端**主动推送**（后端每 250ms 一帧），前端零轮询；
 * - 日志 / 生命周期：通过 `Event` 推送；
 * - 只有在建连时调用一次 `get_snapshot(true)` 补齐历史曲线，之后全部消费推送帧。
 */
export function useEngine(): EngineState {
  const [limits, setLimits] = useState<EngineLimits | null>(null);
  const [snapshot, setSnapshot] = useState<MetricsSnapshot | null>(null);
  const [history, setHistory] = useState<HistoryPoint[]>([]);
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [logPath, setLogPath] = useState<string | null>(null);
  const [lastEvent, setLastEvent] = useState<RunEvent | null>(null);
  const [connection, setConnection] = useState<ConnectionState>("connecting");
  const [error, setError] = useState<string | null>(null);
  const lastSeq = useRef(-1);

  const appendHistory = useCallback((points: HistoryPoint[], cap: number) => {
    setHistory((previous) => {
      const merged = [...previous, ...points];
      return merged.length > cap ? merged.slice(merged.length - cap) : merged;
    });
  }, []);

  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];

    // 日志路径单独取：即使这一项失败，也不该把整条推送链判断为断开。
    void commands
      .getLogPath()
      .then((path) => {
        if (!disposed) setLogPath(path);
      })
      .catch(() => {
        if (!disposed) setLogPath(null);
      });

    void (async () => {
      try {
        const [engineLimits, initial] = await Promise.all([
          commands.getLimits(),
          commands.getSnapshot(true),
        ]);
        if (disposed) return;
        setLimits(engineLimits);
        setSnapshot(initial);
        lastSeq.current = initial.seq;
        setHistory(initial.history.slice(-engineLimits.historyLen));

        // 指标推流（Channel，非轮询）
        await commands.subscribeMetrics((frame) => {
          // 丢弃乱序/重复帧，保证 UI 单调前进。
          if (frame.seq <= lastSeq.current) return;
          lastSeq.current = frame.seq;
          setSnapshot(frame);
          if (frame.latest) appendHistory([frame.latest], engineLimits.historyLen);
        });
        setConnection("live");

        unlisteners.push(
          await events.onLog((entry) => {
            setLogs((previous) => {
              const next = [...previous, entry];
              return next.length > LOG_BUFFER_LIMIT
                ? next.slice(next.length - LOG_BUFFER_LIMIT)
                : next;
            });
          }),
        );
        unlisteners.push(await events.onRunEvent((event) => setLastEvent(event)));
      } catch (cause) {
        if (!disposed) {
          setConnection("down");
          setError(describeCoreError(cause));
        }
      }
    })();

    return () => {
      disposed = true;
      for (const unlisten of unlisteners) unlisten();
    };
  }, [appendHistory]);

  const start = useCallback(async (request: StartRunRequest) => {
    setError(null);
    try {
      await commands.startRun(request);
    } catch (cause) {
      setError(describeCoreError(cause));
    }
  }, []);

  const stop = useCallback(async (reason?: string) => {
    setError(null);
    try {
      await commands.stopRun(reason);
    } catch (cause) {
      setError(describeCoreError(cause));
    }
  }, []);

  const patchLive = useCallback(async (patch: LiveConfigPatch) => {
    try {
      await commands.setLiveConfig(patch);
    } catch (cause) {
      setError(describeCoreError(cause));
    }
  }, []);

  const clearLogs = useCallback(() => setLogs([]), []);
  const reportError = useCallback((report: FrontendErrorReport) => {
    // 静默失败：异常上报是排障的最后一环，它自己再抛错只会制造新的异常风暴。
    void commands.reportFrontendError(report).catch(() => undefined);
  }, []);
  const quit = useCallback(() => commands.quitApp(), []);

  return {
    limits,
    snapshot,
    history,
    logs,
    logPath,
    lastEvent,
    connection,
    error,
    start,
    stop,
    patchLive,
    clearLogs,
    reportError,
    quit,
  };
}
