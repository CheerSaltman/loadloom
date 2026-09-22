import { useCallback, useEffect, useRef, useState } from "react";
import {
  commands,
  describeCoreError,
  events,
  type EngineLimits,
  type HistoryPoint,
  type LiveConfigPatch,
  type LogEntry,
  type MetricsSnapshot,
  type RunEvent,
  type StartRunRequest,
} from "@/bindings";

export type ConnectionState = "connecting" | "live" | "down";

export interface EngineState {
  limits: EngineLimits | null;
  snapshot: MetricsSnapshot | null;
  history: HistoryPoint[];
  logs: LogEntry[];
  lastEvent: RunEvent | null;
  connection: ConnectionState;
  error: string | null;
  start: (request: StartRunRequest) => Promise<void>;
  stop: (reason?: string) => Promise<void>;
  patchLive: (patch: LiveConfigPatch) => Promise<void>;
  clearLogs: () => void;
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
            setLogs((previous) => [...previous, entry]);
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
  const quit = useCallback(() => commands.quitApp(), []);

  return {
    limits,
    snapshot,
    history,
    logs,
    lastEvent,
    connection,
    error,
    start,
    stop,
    patchLive,
    clearLogs,
    quit,
  };
}
