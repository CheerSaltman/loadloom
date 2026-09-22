import { useEffect, useMemo, useRef, useState } from "react";
import { LineChart } from "@/components/LineChart";
import { NicPanel } from "@/components/NicPanel";
import { useNic } from "@/hooks/useNic";
import { useEngine } from "@/hooks/useEngine";
import { usePt } from "@/hooks/usePt";
import { commands, type LogLevel } from "@/bindings";
import {
  cn,
  composeUrl,
  formatBytes,
  formatClock,
  formatDuration,
  formatGbps,
  formatRate,
} from "@/lib/utils";

const PRESETS: Record<string, { scheme: string; host: string; port: string; path: string }> = {
  cf100: { scheme: "https", host: "speed.cloudflare.com", port: "443", path: "/__down?bytes=104857600" },
  cf10: { scheme: "https", host: "speed.cloudflare.com", port: "443", path: "/__down?bytes=10485760" },
};

function Card({ label, value, sub, tone }: { label: string; value: string; sub?: string; tone: string }) {
  return (
    <div className="relative overflow-hidden rounded-xl border border-line bg-surface px-4 py-3">
      <span className={cn("absolute inset-x-0 top-0 h-0.5 opacity-60", tone)} />
      <div className="text-[12px] tracking-wide text-muted">{label}</div>
      <div className="mt-2 text-[25px] font-bold tabular-nums">{value}</div>
      {sub ? <div className="mt-1 text-[12px] tabular-nums text-muted">{sub}</div> : null}
    </div>
  );
}

function Pill({ children, tone }: { children: React.ReactNode; tone: string }) {
  return (
    <span className={cn("inline-flex items-center gap-2 rounded-full border px-3 py-1.5 text-[12.5px] font-semibold", tone)}>
      {children}
    </span>
  );
}

/** 日志位置只显示 `文件名:行`：完整路径会把正文挤到屏幕外。 */
function shortSource(source: string): string {
  if (!source) return "-";
  return source.split(/[\\/]/).pop() ?? source;
}

/**
 * 前端异常上报的护栏：同一异常只报一次，整场上限 50 条。
 *
 * 没有护栏时，一个在渲染期持续抛出的异常会形成「异常 → 上报 → 写日志 → 推送日志
 * → 重新渲染 → 再抛」的自激循环，把排障工具本身变成故障放大器。后端还有一层
 * 里程碑节流，两层各自独立生效。
 */
const reportedErrors = new Set<string>();
let reportedErrorCount = 0;
const REPORT_LIMIT = 50;

function shouldReportError(key: string): boolean {
  if (reportedErrors.has(key) || reportedErrorCount >= REPORT_LIMIT) return false;
  reportedErrors.add(key);
  reportedErrorCount += 1;
  return true;
}

export default function App() {
  const engine = useEngine();
  const pt = usePt();
  // 网卡监测与打流各自独立推流：曲线与事件按 500ms 一帧进入同一个界面，
  // 但互不阻塞 —— 打流停止后链路监测仍在继续。
  const nic = useNic();
  const { snapshot, history, logs, connection, error, lastEvent, logPath } = engine;

  const [tab, setTab] = useState<"console" | "logs" | "nic">("console");
  const [dark, setDark] = useState(true);
  const [scheme, setScheme] = useState("https");
  const [host, setHost] = useState("");
  const [port, setPort] = useState("443");
  const [path, setPath] = useState("/");
  const [threads, setThreads] = useState(8);
  const [rateMib, setRateMib] = useState(0);
  const [rampUpSecs, setRampUpSecs] = useState(0);
  const [profile, setProfile] = useState<"httpDownload" | "localPt" | "torrentSwarm">("httpDownload");
  const [peerUrls, setPeerUrls] = useState("");
  const [torrentSource, setTorrentSource] = useState("");
  const [ptConnections, setPtConnections] = useState(180);
  const [ptRamMib, setPtRamMib] = useState(512);
  const [ptDurationSecs, setPtDurationSecs] = useState(600);
  const [ptMaxDownloadGib, setPtMaxDownloadGib] = useState(0);
  const [ptStalledSecs, setPtStalledSecs] = useState(15);
  const [failureStopPercent, setFailureStopPercent] = useState(20);
  const [latencyStopMs, setLatencyStopMs] = useState(3000);
  const [limitGbOn, setLimitGbOn] = useState(false);
  const [limitGb, setLimitGb] = useState(10);
  const [limitMinOn, setLimitMinOn] = useState(false);
  const [limitMin, setLimitMin] = useState(10);
  const [notice, setNotice] = useState<string | null>(null);
  const [levelFilter, setLevelFilter] = useState<LogLevel | "all">("all");
  const [logQuery, setLogQuery] = useState("");
  const [autoScroll, setAutoScroll] = useState(true);
  const logPaneRef = useRef<HTMLDivElement | null>(null);

  const ptRunning = pt.snapshot?.phase === "starting" || pt.snapshot?.phase === "metadata" || pt.snapshot?.phase === "downloading";
  const httpRunning = snapshot?.phase === "running";
  const running = httpRunning || ptRunning;
  const torrentMode = profile === "torrentSwarm";
  const limitGbValid = Number.isFinite(limitGb) && limitGb > 0;
  const limitMinValid = Number.isFinite(limitMin) && limitMin > 0;
  const url = useMemo(() => composeUrl(host, scheme, port, path), [host, scheme, port, path]);
  const ptBottleneckHint = useMemo(() => {
    const adapter = [...(nic.snapshot?.adapters ?? [])]
      .filter((item) => item.monitored && item.class === "physical")
      .sort((left, right) => right.rxBps - left.rxBps)[0];
    if (!adapter) return "没有可用的物理网卡采样，暂时无法定位瓶颈。";
    if (adapter.rxErrorsPerSec > 0 || adapter.rxDiscardsPerSec > 0) {
      return `${adapter.name} 正在出现接收错误或丢弃，优先排查 Wi-Fi 信号、驱动和信道拥塞。`;
    }
    if (adapter.rxUtilization >= 85) {
      return `${adapter.name} RX 利用率 ${adapter.rxUtilization.toFixed(1)}%，无线链路已接近协商速率上限。`;
    }
    if ((pt.snapshot?.deadPeerPercent ?? 0) >= 60 || (pt.snapshot?.stalledPeers ?? 0) * 2 >= (pt.snapshot?.activePeers ?? 1)) {
      return `网卡 RX 利用率仅 ${adapter.rxUtilization.toFixed(1)}%，同时 Peer 死链/卡死比例偏高，公网 swarm 更可能先成为瓶颈。`;
    }
    if ((pt.snapshot?.halfOpenPeers ?? 0) >= 20 && (pt.snapshot?.activePeers ?? 0) < 8) {
      return `存在 ${pt.snapshot?.halfOpenPeers ?? 0} 个半开连接但已建立连接较少，路由器 NAT、运营商或公网握手路径可能受限。`;
    }
    return `${adapter.name} RX 利用率 ${adapter.rxUtilization.toFixed(1)}%；当前证据不足以把瓶颈唯一归因于网卡或路由器。`;
  }, [nic.snapshot, pt.snapshot]);

  // 并发与限速的权威在后端：请求 64 线程会被收敛到 MAX_WORKERS。这里把快照里的
  // **实际生效值**回填给输入框，显示值不再与引擎真实状态背离。
  useEffect(() => {
    if (!snapshot) return;
    setThreads(snapshot.threads);
    setRateMib(snapshot.rateMib);
  }, [snapshot?.threads, snapshot?.rateMib]);

  // 未捕获异常必须留痕：release 构建的 WebView 没有可见控制台，不主动上报就等于
  // 「用户看到界面卡住 / 白屏，而日志里什么都没有」—— 正是最难定位的那类故障。
  useEffect(() => {
    const submit = (kind: string, message: string, source: string, stack: string) => {
      if (!shouldReportError(`${kind}|${message}|${source}`)) return;
      engine.reportError({ kind, message, source, stack });
    };
    const onError = (event: ErrorEvent) => {
      submit(
        "error",
        event.message || "未知脚本错误",
        `${event.filename || "webview"}:${event.lineno || 0}:${event.colno || 0}`,
        event.error instanceof Error ? (event.error.stack ?? "") : "",
      );
    };
    const onRejection = (event: PromiseRejectionEvent) => {
      const reason: unknown = event.reason;
      const message = (() => {
        if (reason instanceof Error) return reason.message;
        if (typeof reason === "string") return reason;
        try {
          return JSON.stringify(reason) ?? String(reason);
        } catch {
          return String(reason);
        }
      })();
      submit(
        "unhandledrejection",
        message || "未知 Promise 拒绝",
        "webview:promise",
        reason instanceof Error ? (reason.stack ?? "") : "",
      );
    };
    window.addEventListener("error", onError);
    window.addEventListener("unhandledrejection", onRejection);
    return () => {
      window.removeEventListener("error", onError);
      window.removeEventListener("unhandledrejection", onRejection);
    };
  }, [engine.reportError]);

  const filteredLogs = useMemo(() => {
    const needle = logQuery.trim().toLowerCase();
    return logs.filter((entry) => {
      if (levelFilter !== "all" && entry.level !== levelFilter) return false;
      if (!needle) return true;
      return `${entry.code} ${entry.source} ${entry.message}`.toLowerCase().includes(needle);
    });
  }, [logs, levelFilter, logQuery]);

  // 自动滚动：默认跟随最新日志，用户手动回看历史时可以关掉。
  useEffect(() => {
    if (!autoScroll || tab !== "logs") return;
    const pane = logPaneRef.current;
    if (pane) pane.scrollTop = pane.scrollHeight;
  }, [filteredLogs, autoScroll, tab]);

  const notify = (message: string) => {
    setNotice(message);
    window.setTimeout(() => {
      setNotice((current) => (current === message ? null : current));
    }, 5000);
  };

  // 诊断文本 = 界面内存里的会话摘要 + 磁盘日志尾部（含环境头与崩溃报告清单）。
  // 两者拼在一起，用户只要复制一次，维护者就能拿到「当时界面看到什么 + 落盘记录」。
  const buildDiagnostics = async (): Promise<string> => {
    const disk = await commands.getDiagnostics(500).catch(() => "（无法读取磁盘日志）");
    const summary = [
      "LoadLoom 会话摘要（界面内存快照）",
      `推送链路：${connection}`,
      `运行阶段：${snapshot?.phase ?? "未知"}`,
      `目标地址：${snapshot?.url || "（无）"}`,
      `并发 / 限速：${snapshot?.threads ?? 0} · ${snapshot?.rateMib ?? 0} MiB/s`,
      `累计流量 / 失败请求：${snapshot?.totalBytes ?? 0} B · ${snapshot?.failures ?? 0}`,
      `最近事件：${lastEvent ? `${lastEvent.code} ${lastEvent.message}` : "（无）"}`,
      `日志文件：${logPath ?? "（未落盘）"}`,
    ].join("\n");
    return `${summary}\n\n${disk}`;
  };

  const copyText = async (text: string): Promise<boolean> => {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      // 剪贴板 API 在某些环境不可用：退回旧式复制，失败才如实告知。
      try {
        const area = document.createElement("textarea");
        area.value = text;
        area.style.position = "fixed";
        area.style.opacity = "0";
        document.body.appendChild(area);
        area.select();
        const ok = document.execCommand("copy");
        area.remove();
        return ok;
      } catch {
        return false;
      }
    }
  };

  const copyDiagnostics = async () => {
    const text = await buildDiagnostics();
    notify((await copyText(text)) ? "诊断信息已复制到剪贴板" : "复制失败：请改用「导出诊断文件」");
  };

  const exportDiagnostics = async () => {
    const text = await buildDiagnostics();
    const picker = (
      window as unknown as {
        showSaveFilePicker?: (options: unknown) => Promise<{
          createWritable: () => Promise<{
            write: (data: string) => Promise<void>;
            close: () => Promise<void>;
          }>;
        }>;
      }
    ).showSaveFilePicker;
    if (!picker) {
      notify(
        (await copyText(text))
          ? "当前环境不支持直接保存，诊断信息已复制到剪贴板"
          : "导出失败：当前环境不支持保存文件",
      );
      return;
    }
    try {
      const stamp = new Date().toISOString().replace(/[:.]/g, "-");
      const handle = await picker({
        suggestedName: `loadloom-diagnostics-${stamp}.txt`,
        types: [{ description: "诊断文本", accept: { "text/plain": [".txt"] } }],
      });
      const writable = await handle.createWritable();
      await writable.write(text);
      await writable.close();
      notify("诊断文件已保存");
    } catch {
      notify("未保存（已取消或当前环境不支持）");
    }
  };

  const openLogDir = async () => {
    try {
      await commands.openLogDir();
    } catch (cause) {
      notify(typeof cause === "string" ? cause : "无法打开日志目录");
    }
  };

  const toggleTheme = () => {
    const next = !dark;
    setDark(next);
    document.documentElement.classList.toggle("light", !next);
  };

  const handleStart = () => {
    if (torrentMode) {
      void pt.start({
        source: torrentSource.trim(),
        maxConnections: ptConnections,
        ramMib: ptRamMib,
        durationSecs: ptDurationSecs,
        maxDownloadGib: ptMaxDownloadGib,
        rateMib,
        stalledPeerSecs: ptStalledSecs,
        authorized: true,
      });
      return;
    }
    void engine.start({
      url,
      threads,
      rateMib,
      rampUpSecs,
      profile: profile === "localPt" ? "localPt" : "httpDownload",
      peerUrls: peerUrls.split(/\r?\n/).map((value) => value.trim()).filter(Boolean),
      failureStopPercent,
      latencyStopMs,
      // 上限只在「已勾选 + 有限正数」时发送：0 在引擎里等于关闭自动停止。
      limitGb: limitGbOn && limitGbValid ? limitGb : 0,
      limitMinutes: limitMinOn && limitMinValid ? limitMin : 0,
      authorized: true,
    });
  };

  const handleStop = () => {
    if (ptRunning) {
      void pt.stop();
    } else {
      void engine.stop("已由用户手动停止");
    }
  };

  // 只接受有限数：`Number("") || 0` 会把「清空输入框」折算成 0（静默关掉
  // 限速/上限），`1e999` 会变成 Infinity，JSON 序列化成 null 后只换来一句
  // 英文参数错误。非有限值一律不送 IPC。
  const parseFinite = (raw: string): number | null => {
    const value = Number(raw);
    return Number.isFinite(value) ? value : null;
  };

  const onThreads = (raw: string) => {
    const value = parseFinite(raw);
    if (value === null) return;
    // 后端字段是 u32：小数会被 serde 当成参数错误拒掉。这里只做整数化，
    // 范围收敛仍归后端（1..=MAX_WORKERS），显示值随后由快照回填。
    const next = Math.trunc(value);
    setThreads(next);
    if (httpRunning) void engine.patchLive({ threads: next, rateMib: null });
  };
  const onRate = (raw: string) => {
    const value = parseFinite(raw);
    if (value === null) return;
    setRateMib(value);
    if (httpRunning) void engine.patchLive({ threads: null, rateMib: value });
  };

  const onRampUp = (raw: string) => {
    const value = parseFinite(raw);
    if (value === null) return;
    setRampUpSecs(Math.max(0, value));
  };

  const onFailureStopPercent = (raw: string) => {
    const value = parseFinite(raw);
    if (value === null) return;
    setFailureStopPercent(Math.max(0, Math.min(100, value)));
  };

  const onLatencyStopMs = (raw: string) => {
    const value = parseFinite(raw);
    if (value === null) return;
    setLatencyStopMs(Math.max(0, value));
  };

  // 上限为 0 / 非有限值 = 引擎侧「没有上限」。绝不能让它带着勾选状态静默送出，
  // 否则卡片仍写着「已设置自动停止」，而守卫根本没生效。
  const rejectLimit = (label: string, setEnabled: (enabled: boolean) => void) => {
    setEnabled(false);
    setNotice(`${label}未生效：请输入大于 0 的数值`);
  };

  const onLimitGb = (raw: string) => {
    const value = parseFinite(raw);
    if (value === null) {
      rejectLimit("流量上限", setLimitGbOn);
      return;
    }
    // 0 / 负数照原样保留在输入框里，让用户看到自己输入的值，同时取消勾选。
    setLimitGb(value);
    if (value <= 0) {
      rejectLimit("流量上限", setLimitGbOn);
      return;
    }
    setNotice(null);
  };
  const onLimitMin = (raw: string) => {
    const value = parseFinite(raw);
    if (value === null) {
      rejectLimit("时长上限", setLimitMinOn);
      return;
    }
    setLimitMin(value);
    if (value <= 0) {
      rejectLimit("时长上限", setLimitMinOn);
      return;
    }
    setNotice(null);
  };
  const onToggleLimitGb = (checked: boolean) => {
    if (checked && !limitGbValid) {
      rejectLimit("流量上限", setLimitGbOn);
      return;
    }
    setNotice(null);
    setLimitGbOn(checked);
  };
  const onToggleLimitMin = (checked: boolean) => {
    if (checked && !limitMinValid) {
      rejectLimit("时长上限", setLimitMinOn);
      return;
    }
    setNotice(null);
    setLimitMinOn(checked);
  };

  const activeHistory = torrentMode ? pt.history : history;
  const speedSeries = activeHistory.map((point) => point.speedBps);
  const latencySeries = activeHistory.map((point) => point.latencyMs);
  const jitterSeries = activeHistory.map((point) => point.jitterMs);

  return (
    <div className="flex h-screen w-screen">
      {/* ---------------- 侧边栏 ---------------- */}
      <aside className="flex w-[236px] shrink-0 flex-col border-r border-line bg-surface-2 px-4 py-5">
        <div className="flex items-center gap-3">
          <span className={cn("size-3 rounded-full bg-accent shadow-[0_0_14px_var(--accent)]", running && "ll-live-dot")} />
          <div>
            <div className="text-[15px] font-bold leading-tight">LoadLoom</div>
            <div className="text-[10.5px] tracking-[1.6px] text-muted">打流控制台</div>
          </div>
        </div>

        <nav className="mt-7 flex flex-col gap-1">
          {([
            ["console", "实时监控"],
            ["nic", "网卡监测"],
            ["logs", `运行日志${logs.length ? ` (${logs.length})` : ""}`],
          ] as const).map(([key, label]) => (
            <button
              key={key}
              onClick={() => setTab(key)}
              className={cn(
                "rounded-lg px-3 py-2 text-left text-[13px] font-semibold transition",
                tab === key ? "bg-accent/15 text-accent" : "text-muted hover:bg-line-soft hover:text-ink",
              )}
            >
              {label}
            </button>
          ))}
        </nav>

        <div className="mt-auto space-y-2">
          <button onClick={toggleTheme} className="w-full rounded-lg border border-line px-3 py-2 text-[12.5px] text-muted hover:text-ink">
            {dark ? "切换到浅色模式" : "切换到深色模式"}
          </button>
          <button onClick={() => void engine.quit()} className="w-full rounded-lg border border-line px-3 py-2 text-[12.5px] text-muted hover:text-bad">
            退出程序
          </button>
        </div>
      </aside>

      {/* ---------------- 主区域 ---------------- */}
      <main className="flex min-w-0 flex-1 flex-col overflow-hidden">
        <header className="flex flex-wrap items-center gap-3 border-b border-line px-5 py-3.5">
          <h1 className="text-[17px] font-bold">高并发打流控制台</h1>
          <div className="ml-auto flex flex-wrap items-center gap-2">
            <Pill tone={connection === "live" ? "border-good/40 bg-good/10 text-good" : "border-bad/40 bg-bad/10 text-bad"}>
              <i className={cn("size-2 rounded-full bg-current", connection === "live" && "ll-live-dot")} />
              {connection === "live" ? "推送链路已连接" : connection === "connecting" ? "连接中…" : "链路断开"}
            </Pill>
            <Pill tone={running ? "border-good/40 bg-good/10 text-good" : "border-line bg-surface text-muted"}>
              {running ? "打流中" : "待机"}
            </Pill>
            <Pill tone="border-line bg-surface text-muted">运行时长 {formatDuration(torrentMode ? (pt.snapshot?.elapsedSecs ?? 0) : (snapshot?.elapsedSecs ?? 0))}</Pill>
          </div>
        </header>

        {tab === "console" ? (
          <div className="grid min-h-0 flex-1 grid-cols-[384px_minmax(0,1fr)] gap-4 overflow-y-auto p-5">
            {/* 参数面板 */}
            <section className="rounded-2xl border border-line bg-surface p-4">
              <h2 className="mb-3 text-[14px] font-bold">打流参数</h2>

              <label className="mb-1 block text-[12px] text-muted">流量模型</label>
              <select value={profile} onChange={(e) => setProfile(e.target.value as "httpDownload" | "localPt" | "torrentSwarm")}
                className="mb-3 w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-[13px]">
                <option value="httpDownload">HTTP 持续下载</option>
                <option value="localPt">局域网 HTTP 分片仿真</option>
                <option value="torrentSwarm">真实公网 PT Swarm（RAM 丢弃）</option>
              </select>

              {torrentMode ? (
                <>
                  <label className="mb-1 block text-[12px] text-muted">Magnet 或公开 .torrent URL</label>
                  <textarea value={torrentSource} onChange={(e) => setTorrentSource(e.target.value)} rows={5}
                    placeholder="magnet:?xt=urn:btih:…（建议使用 Ubuntu / Debian 等合法公开发行版）"
                    className="mb-2 w-full resize-y rounded-lg border border-line bg-surface-2 px-3 py-2 font-mono text-[11px] outline-none focus:border-accent" />
                  <div className="mb-3 grid grid-cols-2 gap-2">
                    <label className="text-[12px] text-muted">最大 Peer 连接
                      <input type="number" min={8} max={500} step={1} value={ptConnections}
                        onChange={(e) => setPtConnections(Math.trunc(Number(e.target.value)))}
                        className="mt-1 w-full rounded-lg border border-line bg-surface-2 px-2 py-2 text-[13px]" />
                    </label>
                    <label className="text-[12px] text-muted">RAM 上限（MiB）
                      <input type="number" min={64} max={4096} step={64} value={ptRamMib}
                        onChange={(e) => setPtRamMib(Math.trunc(Number(e.target.value)))}
                        className="mt-1 w-full rounded-lg border border-line bg-surface-2 px-2 py-2 text-[13px]" />
                    </label>
                    <label className="text-[12px] text-muted">运行时长（秒）
                      <input type="number" min={0} max={86400} step={60} value={ptDurationSecs}
                        onChange={(e) => setPtDurationSecs(Math.trunc(Number(e.target.value)))}
                        className="mt-1 w-full rounded-lg border border-line bg-surface-2 px-2 py-2 text-[13px]" />
                    </label>
                    <label className="text-[12px] text-muted">下载上限（GiB，0=整包）
                      <input type="number" min={0} max={1024} step={1} value={ptMaxDownloadGib}
                        onChange={(e) => setPtMaxDownloadGib(Number(e.target.value))}
                        className="mt-1 w-full rounded-lg border border-line bg-surface-2 px-2 py-2 text-[13px]" />
                    </label>
                    <label className="text-[12px] text-muted">死链窗口（秒）
                      <input type="number" min={5} max={120} step={5} value={ptStalledSecs}
                        onChange={(e) => setPtStalledSecs(Math.trunc(Number(e.target.value)))}
                        className="mt-1 w-full rounded-lg border border-line bg-surface-2 px-2 py-2 text-[13px]" />
                    </label>
                    <label className="text-[12px] text-muted">限速（MiB/s，0=不限）
                      <input type="number" min={0} max={4096} step={1} value={rateMib}
                        onChange={(e) => onRate(e.target.value)}
                        className="mt-1 w-full rounded-lg border border-line bg-surface-2 px-2 py-2 text-[13px]" />
                    </label>
                  </div>
                  <p className="mb-3 rounded-lg border border-accent/25 bg-accent/5 px-3 py-2 text-[11px] leading-relaxed text-muted">
                    DHT、PEX、uTP 与 TCP 全部启用；不上传、不创建下载文件。每个 piece 只在 RAM 中保留到哈希校验，随后立即释放。结果是公网端到端上限，需结合网卡页判断瓶颈。
                  </p>
                </>
              ) : (
                <>
              <label className="mb-1 block text-[12px] text-muted">目标地址 / 端口</label>
              <div className="mb-3 flex gap-2">
                <select value={scheme} onChange={(e) => { setScheme(e.target.value); setPort(e.target.value === "https" ? "443" : "80"); }}
                  className="w-[92px] rounded-lg border border-line bg-surface-2 px-2 py-2 text-[13px]">
                  <option value="https">https</option>
                  <option value="http">http</option>
                </select>
                <input value={host} onChange={(e) => setHost(e.target.value)} placeholder="example.com"
                  className="min-w-0 flex-1 rounded-lg border border-line bg-surface-2 px-3 py-2 text-[13px] outline-none focus:border-accent" />
                <input value={port} onChange={(e) => setPort(e.target.value)} placeholder="端口"
                  className="w-[76px] rounded-lg border border-line bg-surface-2 px-2 py-2 text-[13px] outline-none focus:border-accent" />
              </div>

              {profile === "localPt" ? (
                <>
                  <label className="mb-1 block text-[12px] text-muted">额外 Peer URL（每行一个）</label>
                  <textarea value={peerUrls} onChange={(e) => setPeerUrls(e.target.value)} rows={3}
                    placeholder={"http://192.168.1.20:8080/file.bin\nhttp://192.168.1.21:8080/file.bin"}
                    className="mb-1 w-full resize-y rounded-lg border border-line bg-surface-2 px-3 py-2 text-[12px] outline-none focus:border-accent" />
                  <p className="mb-3 text-[11px] text-muted">仅接受 localhost、回环或私有 IP；请求会轮转 Peer 并模拟 256 KiB–2 MiB 分片。</p>
                </>
              ) : null}

              <label className="mb-1 block text-[12px] text-muted">请求路径</label>
              <div className="mb-3 flex gap-2">
                <input value={path} onChange={(e) => setPath(e.target.value)} placeholder="/file.bin"
                  className="min-w-0 flex-1 rounded-lg border border-line bg-surface-2 px-3 py-2 text-[13px] outline-none focus:border-accent" />
                <select
                  className="w-[132px] rounded-lg border border-line bg-surface-2 px-2 py-2 text-[13px]"
                  value=""
                  onChange={(e) => {
                    const preset = PRESETS[e.target.value];
                    if (!preset) return;
                    setScheme(preset.scheme); setHost(preset.host); setPort(preset.port); setPath(preset.path);
                  }}
                >
                  <option value="">预设…</option>
                  <option value="cf100">Cloudflare 100MB</option>
                  <option value="cf10">Cloudflare 10MB</option>
                </select>
              </div>

              <label className="mb-1 block text-[12px] text-muted">并发线程（运行中可调）</label>
              <input type="number" step={1} value={threads} onChange={(e) => onThreads(e.target.value)}
                className="mb-3 w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-[13px] outline-none focus:border-accent" />

              <label className="mb-1 block text-[12px] text-muted">限速带宽（MB/s，0 = 不限速）</label>
              <input type="number" step={0.5} value={rateMib} onChange={(e) => onRate(e.target.value)}
                className="mb-3 w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-[13px] outline-none focus:border-accent" />

              <label className="mb-1 block text-[12px] text-muted">渐进升压（秒，0 = 立即达到并发）</label>
              <input type="number" min={0} step={1} value={rampUpSecs} onChange={(e) => onRampUp(e.target.value)}
                className="mb-3 w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-[13px] outline-none focus:border-accent" />

              <div className="mb-3 grid grid-cols-2 gap-2">
                <label className="text-[12px] text-muted">
                  失败率熔断（%）
                  <input type="number" min={0} max={100} step={1} value={failureStopPercent} onChange={(e) => onFailureStopPercent(e.target.value)}
                    className="mt-1 w-full rounded-lg border border-line bg-surface-2 px-2 py-2 text-[13px] outline-none focus:border-accent" />
                </label>
                <label className="text-[12px] text-muted">
                  时延熔断（ms）
                  <input type="number" min={0} step={100} value={latencyStopMs} onChange={(e) => onLatencyStopMs(e.target.value)}
                    className="mt-1 w-full rounded-lg border border-line bg-surface-2 px-2 py-2 text-[13px] outline-none focus:border-accent" />
                </label>
              </div>

              <div className="mb-3 space-y-2 text-[12.5px]">
                <label className="flex items-center gap-2">
                  <input type="checkbox" checked={limitGbOn} onChange={(e) => onToggleLimitGb(e.target.checked)} className="accent-[var(--accent)]" />
                  累计流量达到
                  <input type="number" step={1} value={limitGb} onChange={(e) => onLimitGb(e.target.value)}
                    className="w-20 rounded-lg border border-line bg-surface-2 px-2 py-1" /> GB
                </label>
                <label className="flex items-center gap-2">
                  <input type="checkbox" checked={limitMinOn} onChange={(e) => onToggleLimitMin(e.target.checked)} className="accent-[var(--accent)]" />
                  运行时长达到
                  <input type="number" step={1} value={limitMin} onChange={(e) => onLimitMin(e.target.value)}
                    className="w-20 rounded-lg border border-line bg-surface-2 px-2 py-1" /> 分钟
                </label>
              </div>
                </>
              )}

              <div className="flex gap-2.5">
                <button onClick={handleStart} disabled={running}
                  className="flex-1 rounded-xl bg-gradient-to-br from-[#17924f] to-[#2fbe72] px-4 py-3 text-[14px] font-bold text-white disabled:opacity-40">
                  开始打流
                </button>
                <button onClick={handleStop} disabled={!running}
                  className="flex-1 rounded-xl bg-gradient-to-br from-[#a4323f] to-[#d45563] px-4 py-3 text-[14px] font-bold text-white disabled:opacity-40">
                  停止
                </button>
              </div>

              <div className={cn("mt-3 min-h-[18px] text-[12.5px]", (error || pt.error) ? "text-bad" : notice ? "text-warn" : running ? "text-good" : "text-muted")}>
                {pt.error ?? error ?? notice ?? (torrentMode ? pt.snapshot?.status : snapshot?.status) ?? "准备就绪 · 等待开始"}
              </div>
            </section>

            {/* 数据区 */}
            <section className="flex min-w-0 flex-col gap-4">
              <div className="grid grid-cols-[repeat(auto-fit,minmax(186px,1fr))] gap-3">
                {torrentMode ? (
                  <>
                    <Card label="有效 PT 流量" value={formatBytes(pt.snapshot?.totalBytes ?? 0)} sub={`线速字节 ${formatBytes(pt.snapshot?.wireBytes ?? 0)}`} tone="bg-accent" />
                    <Card label="实时速率" value={formatRate(pt.snapshot?.speedBps ?? 0)} sub={formatGbps(pt.snapshot?.speedBps ?? 0)} tone="bg-good" />
                    <Card label="Peer 连接" value={String(pt.snapshot?.activePeers ?? 0)} sub={`有效 ${pt.snapshot?.usefulPeers ?? 0} · 半开 ${pt.snapshot?.halfOpenPeers ?? 0}`} tone="bg-indigo" />
                    <Card label="死链分析" value={`${(pt.snapshot?.deadPeerPercent ?? 0).toFixed(1)}%`} sub={`死链 ${pt.snapshot?.deadPeers ?? 0} · 卡死 ${pt.snapshot?.stalledPeers ?? 0}`} tone="bg-bad" />
                    <Card label="RAM Piece 缓冲" value={formatBytes(pt.snapshot?.ramUsedBytes ?? 0)} sub={`峰值 ${formatBytes(pt.snapshot?.ramPeakBytes ?? 0)} / 上限 ${formatBytes(pt.snapshot?.ramLimitBytes ?? 0)}`} tone="bg-warn" />
                    <Card label="Piece 校验" value={String(pt.snapshot?.goodPieces ?? 0)} sub={`失败 ${pt.snapshot?.badPieces ?? 0} · 进度 ${(pt.snapshot?.progressPercent ?? 0).toFixed(1)}%`} tone="bg-good" />
                    <Card label="压力分析" value={pt.snapshot?.pressureLevel === "critical" ? "严重" : pt.snapshot?.pressureLevel === "warning" ? "预警" : "正常"}
                      sub={pt.snapshot?.pressureReason ?? "等待采样"} tone={pt.snapshot?.pressureLevel === "critical" ? "bg-bad" : pt.snapshot?.pressureLevel === "warning" ? "bg-warn" : "bg-good"} />
                  </>
                ) : (
                  <>
                <Card label="累计流量" value={formatBytes(snapshot?.totalBytes ?? 0)} sub={limitGbOn || limitMinOn ? "已设置自动停止" : "未设置自动停止"} tone="bg-accent" />
                <Card label="实时速率" value={formatRate(snapshot?.speedBps ?? 0)} sub={formatGbps(snapshot?.speedBps ?? 0)} tone="bg-good" />
                <Card label="首包时延 TTFB" value={snapshot?.latencyMs ? `${snapshot.latencyMs.toFixed(1)} ms` : "--"} sub="毫秒 · 最近一次" tone="bg-warn" />
                <Card label="网络抖动 Jitter" value={snapshot?.jitterMs ? `${snapshot.jitterMs.toFixed(2)} ms` : "--"} sub="RFC3550 递推" tone="bg-indigo" />
                <Card label="请求成功率" value={snapshot && snapshot.completed + snapshot.failures > 0 ? `${snapshot.successRate.toFixed(2)}%` : "--"} sub={`成功 ${snapshot?.completed ?? 0} / 失败 ${snapshot?.failures ?? 0}`} tone="bg-good" />
                <Card label="失败请求数" value={String(snapshot?.failures ?? 0)} sub={`在途连接 ${snapshot?.inFlight ?? 0}`} tone="bg-bad" />
                <Card label="压力保护" value={snapshot?.pressureLevel === "critical" ? "已熔断" : snapshot?.pressureLevel === "warning" ? "预警" : "正常"}
                  sub={snapshot?.pressureReason ?? "等待采样"} tone={snapshot?.pressureLevel === "critical" ? "bg-bad" : snapshot?.pressureLevel === "warning" ? "bg-warn" : "bg-good"} />
                  </>
                )}
              </div>

              {torrentMode ? (
                <>
                <div className="rounded-2xl border border-line bg-surface p-4">
                  <div className="mb-3 flex flex-wrap items-center gap-4 text-[12px] text-muted">
                    <h2 className="text-[14px] font-bold text-ink">实时 PT 速度曲线</h2>
                    <span>峰值 <b className="text-ink">{formatRate(Math.max(0, ...speedSeries))}</b></span>
                    <span>均值 <b className="text-ink">{formatRate(pt.snapshot?.averageBps ?? 0)}</b></span>
                    <span>限速 <b className="text-ink">{rateMib > 0 ? `${rateMib.toFixed(1)} MiB/s` : "不限"}</b></span>
                  </div>
                  <LineChart series={[{ label: "速率", color: "var(--accent)", values: speedSeries }]} height={190} />
                </div>
                <div className="grid grid-cols-2 gap-4">
                  <div className="rounded-2xl border border-line bg-surface p-4 text-[12.5px]">
                    <h2 className="mb-3 text-[14px] font-bold">Peer / Tracker 诊断</h2>
                    <div className="grid grid-cols-2 gap-y-2 text-muted">
                      <span>握手成功</span><b className="text-right text-ink">{pt.snapshot?.peerHandshakes ?? 0}</b>
                      <span>连接关闭</span><b className="text-right text-ink">{pt.snapshot?.closedPeers ?? 0}</b>
                      <span>Tracker 成功 / 错误</span><b className="text-right text-ink">{pt.snapshot?.trackerSuccesses ?? 0} / {pt.snapshot?.trackerErrors ?? 0}</b>
                      <span>无效/浪费流量</span><b className="text-right text-ink">{formatBytes(pt.snapshot?.wastedBytes ?? 0)}</b>
                      <span>存储错误</span><b className="text-right text-ink">{pt.snapshot?.storageErrors ?? 0}</b>
                    </div>
                  </div>
                  <div className="rounded-2xl border border-line bg-surface p-4 text-[12.5px]">
                    <h2 className="mb-3 text-[14px] font-bold">瓶颈判读</h2>
                    <p className="leading-relaxed text-ink">{ptBottleneckHint}</p>
                    <p className="mt-2 leading-relaxed text-muted">公网测试始终是无线网卡、路由器、宽带和 swarm 的共同上限；单端无法绝对排除路由器。</p>
                    {pt.snapshot?.lastError ? <p className="mt-2 break-all text-bad">{pt.snapshot.lastError}</p> : null}
                  </div>
                </div>
                </>
              ) : (
                <>
              <div className="rounded-2xl border border-line bg-surface p-4">
                <div className="mb-3 flex flex-wrap items-center gap-4 text-[12px] text-muted">
                  <h2 className="text-[14px] font-bold text-ink">实时速度曲线</h2>
                  <span>峰值 <b className="text-ink">{formatRate(torrentMode ? Math.max(0, ...speedSeries) : (snapshot?.peakBps ?? 0))}</b></span>
                  <span>均值 <b className="text-ink">{formatRate(torrentMode ? (pt.snapshot?.averageBps ?? 0) : (snapshot?.avgBps ?? 0))}</b></span>
                  <span>限速 <b className="text-ink">{torrentMode ? (rateMib > 0 ? `${rateMib.toFixed(1)} MiB/s` : "不限") : snapshot?.rateLimited ? `${snapshot.rateMib.toFixed(1)} MB/s` : "不限"}</b></span>
                </div>
                <LineChart series={[{ label: "速率", color: "var(--accent)", values: speedSeries }]} height={190} />
              </div>

              <div className="rounded-2xl border border-line bg-surface p-4">
                <div className="mb-3 flex items-center gap-4 text-[12px] text-muted">
                  <h2 className="text-[14px] font-bold text-ink">延迟 / 抖动波形</h2>
                  <span className="text-warn">■ 首包时延</span>
                  <span className="text-indigo">■ 抖动</span>
                </div>
                <LineChart
                  series={[
                    { label: "时延", color: "var(--warn)", values: latencySeries },
                    { label: "抖动", color: "var(--indigo)", values: jitterSeries },
                  ]}
                  height={150}
                />
              </div>

              <div className="rounded-2xl border border-line bg-surface p-4">
                <h2 className="mb-3 text-[14px] font-bold">错误分布</h2>
                <table className="w-full text-[12.5px]">
                  <thead>
                    <tr className="text-[11.5px] uppercase tracking-wider text-muted">
                      <th className="text-left font-semibold">错误码</th>
                      <th className="text-right font-semibold">次数</th>
                    </tr>
                  </thead>
                  <tbody>
                    {snapshot?.errors.length ? (
                      snapshot.errors.map((item) => (
                        <tr key={item.code} className="border-t border-line-soft">
                          <td className="py-2 font-mono text-bad">{item.code}</td>
                          <td className="py-2 text-right tabular-nums">{item.count}</td>
                        </tr>
                      ))
                    ) : (
                      <tr><td colSpan={2} className="py-3 text-center text-muted">暂无错误记录</td></tr>
                    )}
                  </tbody>
                </table>
                {snapshot?.lastError ? <div className="mt-2 break-all text-[12px] text-bad">{snapshot.lastError}</div> : null}
              </div>
                </>
              )}
            </section>
          </div>
        ) : tab === "logs" ? (
          <section className="flex min-h-0 flex-1 flex-col p-5">
            <div className="mb-3 flex flex-wrap items-center gap-2">
              <h2 className="text-[14px] font-bold">运行日志（Event 推流）</h2>
              <div className="flex items-center gap-0.5 rounded-lg border border-line p-0.5">
                {([
                  ["all", "全部"],
                  ["info", "信息"],
                  ["warn", "警告"],
                  ["error", "错误"],
                ] as const).map(([key, label]) => (
                  <button
                    key={key}
                    onClick={() => setLevelFilter(key)}
                    className={cn(
                      "rounded-md px-2.5 py-1 text-[12px] font-semibold transition",
                      levelFilter === key ? "bg-accent/15 text-accent" : "text-muted hover:text-ink",
                    )}
                  >
                    {label}
                  </button>
                ))}
              </div>
              <input
                value={logQuery}
                onChange={(event) => setLogQuery(event.target.value)}
                placeholder="搜索事件码 / 位置 / 关键词"
                className="w-[220px] rounded-lg border border-line bg-surface-2 px-3 py-1.5 text-[12px] outline-none focus:border-accent"
              />
              <label className="flex items-center gap-1.5 text-[12px] text-muted">
                <input
                  type="checkbox"
                  checked={autoScroll}
                  onChange={(event) => setAutoScroll(event.target.checked)}
                />
                自动滚动
              </label>
              <div className="ml-auto flex flex-wrap items-center gap-2">
                <button onClick={() => void copyDiagnostics()} className="rounded-lg border border-line px-3 py-1.5 text-[12px] text-muted hover:text-ink">
                  复制诊断信息
                </button>
                <button onClick={() => void exportDiagnostics()} className="rounded-lg border border-line px-3 py-1.5 text-[12px] text-muted hover:text-ink">
                  导出诊断文件
                </button>
                <button onClick={() => void openLogDir()} className="rounded-lg border border-line px-3 py-1.5 text-[12px] text-muted hover:text-ink">
                  打开日志目录
                </button>
                <button onClick={engine.clearLogs} className="rounded-lg border border-line px-3 py-1.5 text-[12px] text-muted hover:text-ink">
                  清空
                </button>
              </div>
            </div>
            <div className="mb-2 break-all text-[11.5px] text-muted">
              {logPath ? `日志文件：${logPath}` : "日志未落盘（仅本页可见）"}
              {` · 显示 ${filteredLogs.length} / ${logs.length} 条`}
            </div>
            <div
              ref={logPaneRef}
              className="min-h-0 flex-1 overflow-y-auto rounded-2xl border border-line bg-surface p-3 font-mono text-[12px] leading-relaxed"
            >
              {filteredLogs.length ? (
                filteredLogs.map((entry, index) => (
                  <div key={`${entry.atMs}-${index}`} className="flex gap-3">
                    <span className="shrink-0 text-muted">{formatClock(entry.atMs)}</span>
                    <span className={cn("shrink-0 uppercase", entry.level === "error" ? "text-bad" : entry.level === "warn" ? "text-warn" : "text-accent")}>
                      {entry.level}
                    </span>
                    <span className="shrink-0 text-indigo">{entry.code || "-"}</span>
                    <span className="shrink-0 text-muted/70" title={entry.source}>
                      {shortSource(entry.source)}
                    </span>
                    <span className="break-all">{entry.message}</span>
                  </div>
                ))
              ) : (
                <div className="text-muted">{logs.length ? "没有匹配的日志" : "暂无日志"}</div>
              )}
            </div>
          </section>
        ) : (
          <NicPanel nic={nic} />
        )}
      </main>
    </div>
  );
}
