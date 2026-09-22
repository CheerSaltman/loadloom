import { useCallback, useEffect, useRef, useState } from "react";
import {
  commands,
  describeCoreError,
  type NicAdapterDto,
  type NicEvent,
  type NicSeriesPoint,
  type NicSnapshot,
} from "@/bindings";
import { copyText } from "@/lib/utils";

/** 勾选结果的持久化键：重启后仍监测同一批网卡。 */
const SELECTION_KEY = "loadloom.nic.selection";
/** 每块网卡的曲线长度上限（与后端 NIC_HISTORY_LEN 对齐：500ms × 240 ≈ 2 分钟）。 */
const SERIES_LIMIT = 240;
/** 接口清单刷新周期：拔插网卡才会变，低频足够，避免无谓 IPC。 */
const ADAPTER_REFRESH_MS = 10_000;

function storedSelection(): string[] {
  try {
    const raw = window.localStorage.getItem(SELECTION_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed)
      ? parsed.filter((item): item is string => typeof item === "string")
      : [];
  } catch {
    return [];
  }
}

/** 握手帧里的历史 -> 图表用的序列。 */
function seedSeries(snapshot: NicSnapshot): Record<string, NicSeriesPoint[]> {
  const seeded: Record<string, NicSeriesPoint[]> = {};
  for (const history of snapshot.history) {
    seeded[history.id] = history.points.slice(-SERIES_LIMIT);
  }
  return seeded;
}

/** 推流帧 -> 在已有序列后追加一个点。 */
function appendFrame(
  previous: Record<string, NicSeriesPoint[]>,
  frame: NicSnapshot,
): Record<string, NicSeriesPoint[]> {
  const next = { ...previous };
  for (const adapter of frame.adapters) {
    const point: NicSeriesPoint = {
      rxBps: adapter.rxBps,
      txBps: adapter.txBps,
      rxUtilization: adapter.rxUtilization,
      txUtilization: adapter.txUtilization,
    };
    const series = [...(next[adapter.id] ?? []), point];
    next[adapter.id] = series.length > SERIES_LIMIT ? series.slice(series.length - SERIES_LIMIT) : series;
  }
  return next;
}

export interface NicState {
  /** 最近一帧快照（含被监测网卡、事件、平台说明）。 */
  snapshot: NicSnapshot | null;
  /** 完整接口清单（含虚拟 / 隧道），供勾选面板使用。 */
  adapters: NicAdapterDto[];
  /** 每块被监测网卡的实时曲线。 */
  series: Record<string, NicSeriesPoint[]>;
  events: NicEvent[];
  error: string | null;
  connected: boolean;
  /** 设置监测范围（空数组 = 默认：全部物理网卡）。 */
  setSelection: (ids: string[]) => Promise<void>;
  /** 勾选 / 取消勾选单块网卡（自动处理「默认范围」与「显式范围」的差异）。 */
  toggleAdapter: (id: string) => Promise<void>;
  /** 回到默认范围。 */
  resetSelection: () => Promise<void>;
  refreshAdapters: () => Promise<void>;
  /** 生成并复制网卡报告；返回是否复制成功。 */
  copyReport: () => Promise<boolean>;
}

/**
 * 网卡链路监测订阅 Hook。
 *
 * 与 `useEngine` 同一套路：握手帧补齐历史，之后全部消费后端推流（500ms 一帧），
 * 前端零轮询。勾选结果存 localStorage —— 后端不碰用户的偏好设置，只负责执行。
 */
export function useNic(): NicState {
  const [snapshot, setSnapshot] = useState<NicSnapshot | null>(null);
  const [adapters, setAdapters] = useState<NicAdapterDto[]>([]);
  const [series, setSeries] = useState<Record<string, NicSeriesPoint[]>>({});
  const [events, setEvents] = useState<NicEvent[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [connected, setConnected] = useState(false);
  const lastSeq = useRef(-1);

  const refreshAdapters = useCallback(async () => {
    try {
      setAdapters(await commands.listNicAdapters());
    } catch (cause) {
      setError(describeCoreError(cause));
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;

    void (async () => {
      try {
        const saved = storedSelection();
        if (saved.length > 0) {
          // 恢复上次的勾选；已被拔出的网卡由后端如实记录，不阻断启动。
          await commands.setNicSelection(saved).catch(() => undefined);
        }
        const initial = await commands.getNicSnapshot(true);
        if (disposed) return;
        setSnapshot(initial);
        lastSeq.current = initial.seq;
        setSeries(seedSeries(initial));
        setEvents(initial.events);

        await commands.subscribeNic((frame) => {
          // 丢弃乱序/重复帧，保证曲线单调前进。
          if (frame.seq <= lastSeq.current) return;
          lastSeq.current = frame.seq;
          setSnapshot(frame);
          setEvents(frame.events);
          setSeries((previous) => appendFrame(previous, frame));
        });
        setConnected(true);
        await refreshAdapters();
        timer = window.setInterval(() => void refreshAdapters(), ADAPTER_REFRESH_MS);
      } catch (cause) {
        if (disposed) return;
        const message = describeCoreError(cause);
        setError(message);
        // 静默降级是这个项目最反对的失效方式：网卡链路初始化失败时，除了界面上
        // 那一行红字，磁盘日志里也必须留下一条 `SYS-002` —— 用户报「网卡页没数据」
        // 时，维护者要能在日志里直接看到原因，而不是只能猜。
        void commands
          .reportFrontendError({
            kind: "nic-init",
            message,
            source: "useNic",
            stack: "",
          })
          .catch(() => undefined);
      }
    })();

    return () => {
      disposed = true;
      if (timer !== undefined) window.clearInterval(timer);
    };
  }, [refreshAdapters]);

  const setSelection = useCallback(
    async (ids: string[]) => {
      setError(null);
      // 先落本地偏好：即使后端拒绝（例如数量超限），下次启动仍会尝试同一份勾选，
      // 而不是悄悄退回默认范围。
      window.localStorage.setItem(SELECTION_KEY, JSON.stringify(ids));
      try {
        await commands.setNicSelection(ids);
        await refreshAdapters();
      } catch (cause) {
        setError(describeCoreError(cause));
      }
    },
    [refreshAdapters],
  );

  const toggleAdapter = useCallback(
    async (id: string) => {
      const explicit = snapshot?.selection ?? [];
      const monitored = (snapshot?.adapters ?? []).map((item) => item.id);
      const next =
        explicit.length === 0
          ? monitored.includes(id)
            ? monitored.filter((item) => item !== id)
            : [...monitored, id]
          : explicit.includes(id)
            ? explicit.filter((item) => item !== id)
            : [...explicit, id];
      await setSelection(next);
    },
    [setSelection, snapshot],
  );

  const resetSelection = useCallback(() => setSelection([]), [setSelection]);

  const copyReport = useCallback(async () => {
    try {
      const text = await commands.getNicReport();
      return await copyText(text);
    } catch (cause) {
      setError(describeCoreError(cause));
      return false;
    }
  }, []);

  return {
    snapshot,
    adapters,
    series,
    events,
    error,
    connected,
    setSelection,
    toggleAdapter,
    resetSelection,
    refreshAdapters,
    copyReport,
  };
}
