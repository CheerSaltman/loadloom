import { useEffect, useMemo, useState } from "react";
import { LineChart } from "@/components/LineChart";
import { useEngine } from "@/hooks/useEngine";
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

export default function App() {
  const engine = useEngine();
  const { snapshot, history, logs, connection, error } = engine;

  const [tab, setTab] = useState<"console" | "logs">("console");
  const [dark, setDark] = useState(true);
  const [scheme, setScheme] = useState("https");
  const [host, setHost] = useState("");
  const [port, setPort] = useState("443");
  const [path, setPath] = useState("/");
  const [threads, setThreads] = useState(8);
  const [rateMib, setRateMib] = useState(0);
  const [limitGbOn, setLimitGbOn] = useState(false);
  const [limitGb, setLimitGb] = useState(10);
  const [limitMinOn, setLimitMinOn] = useState(false);
  const [limitMin, setLimitMin] = useState(10);
  const [notice, setNotice] = useState<string | null>(null);

  const running = snapshot?.phase === "running";
  const limitGbValid = Number.isFinite(limitGb) && limitGb > 0;
  const limitMinValid = Number.isFinite(limitMin) && limitMin > 0;
  const url = useMemo(() => composeUrl(host, scheme, port, path), [host, scheme, port, path]);

  // 并发与限速的权威在后端：请求 64 线程会被收敛到 MAX_WORKERS。这里把快照里的
  // **实际生效值**回填给输入框，显示值不再与引擎真实状态背离。
  useEffect(() => {
    if (!snapshot) return;
    setThreads(snapshot.threads);
    setRateMib(snapshot.rateMib);
  }, [snapshot?.threads, snapshot?.rateMib]);

  const toggleTheme = () => {
    const next = !dark;
    setDark(next);
    document.documentElement.classList.toggle("light", !next);
  };

  const handleStart = () => {
    void engine.start({
      url,
      threads,
      rateMib,
      // 上限只在「已勾选 + 有限正数」时发送：0 在引擎里等于关闭自动停止。
      limitGb: limitGbOn && limitGbValid ? limitGb : 0,
      limitMinutes: limitMinOn && limitMinValid ? limitMin : 0,
      authorized: true,
    });
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
    if (running) void engine.patchLive({ threads: next, rateMib: null });
  };
  const onRate = (raw: string) => {
    const value = parseFinite(raw);
    if (value === null) return;
    setRateMib(value);
    if (running) void engine.patchLive({ threads: null, rateMib: value });
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

  const speedSeries = history.map((point) => point.speedBps);
  const latencySeries = history.map((point) => point.latencyMs);
  const jitterSeries = history.map((point) => point.jitterMs);

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
            <Pill tone="border-line bg-surface text-muted">运行时长 {formatDuration(snapshot?.elapsedSecs ?? 0)}</Pill>
          </div>
        </header>

        {tab === "console" ? (
          <div className="grid min-h-0 flex-1 grid-cols-[384px_minmax(0,1fr)] gap-4 overflow-y-auto p-5">
            {/* 参数面板 */}
            <section className="rounded-2xl border border-line bg-surface p-4">
              <h2 className="mb-3 text-[14px] font-bold">打流参数</h2>

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

              <div className="flex gap-2.5">
                <button onClick={handleStart} disabled={running}
                  className="flex-1 rounded-xl bg-gradient-to-br from-[#17924f] to-[#2fbe72] px-4 py-3 text-[14px] font-bold text-white disabled:opacity-40">
                  开始打流
                </button>
                <button onClick={() => void engine.stop("已由用户手动停止")} disabled={!running}
                  className="flex-1 rounded-xl bg-gradient-to-br from-[#a4323f] to-[#d45563] px-4 py-3 text-[14px] font-bold text-white disabled:opacity-40">
                  停止
                </button>
              </div>

              <div className={cn("mt-3 min-h-[18px] text-[12.5px]", error ? "text-bad" : notice ? "text-warn" : running ? "text-good" : "text-muted")}>
                {error ?? notice ?? snapshot?.status ?? "准备就绪 · 等待开始"}
              </div>
            </section>

            {/* 数据区 */}
            <section className="flex min-w-0 flex-col gap-4">
              <div className="grid grid-cols-[repeat(auto-fit,minmax(186px,1fr))] gap-3">
                <Card label="累计流量" value={formatBytes(snapshot?.totalBytes ?? 0)} sub={limitGbOn || limitMinOn ? "已设置自动停止" : "未设置自动停止"} tone="bg-accent" />
                <Card label="实时速率" value={formatRate(snapshot?.speedBps ?? 0)} sub={formatGbps(snapshot?.speedBps ?? 0)} tone="bg-good" />
                <Card label="首包时延 TTFB" value={snapshot?.latencyMs ? `${snapshot.latencyMs.toFixed(1)} ms` : "--"} sub="毫秒 · 最近一次" tone="bg-warn" />
                <Card label="网络抖动 Jitter" value={snapshot?.jitterMs ? `${snapshot.jitterMs.toFixed(2)} ms` : "--"} sub="RFC3550 递推" tone="bg-indigo" />
                <Card label="请求成功率" value={snapshot && snapshot.completed + snapshot.failures > 0 ? `${snapshot.successRate.toFixed(2)}%` : "--"} sub={`成功 ${snapshot?.completed ?? 0} / 失败 ${snapshot?.failures ?? 0}`} tone="bg-good" />
                <Card label="失败请求数" value={String(snapshot?.failures ?? 0)} sub={`在途连接 ${snapshot?.inFlight ?? 0}`} tone="bg-bad" />
              </div>

              <div className="rounded-2xl border border-line bg-surface p-4">
                <div className="mb-3 flex flex-wrap items-center gap-4 text-[12px] text-muted">
                  <h2 className="text-[14px] font-bold text-ink">实时速度曲线</h2>
                  <span>峰值 <b className="text-ink">{formatRate(snapshot?.peakBps ?? 0)}</b></span>
                  <span>均值 <b className="text-ink">{formatRate(snapshot?.avgBps ?? 0)}</b></span>
                  <span>限速 <b className="text-ink">{snapshot?.rateLimited ? `${snapshot.rateMib.toFixed(1)} MB/s` : "不限"}</b></span>
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
            </section>
          </div>
        ) : (
          <section className="flex min-h-0 flex-1 flex-col p-5">
            <div className="mb-3 flex items-center gap-3">
              <h2 className="text-[14px] font-bold">运行日志（Event 推流）</h2>
              <button onClick={engine.clearLogs} className="ml-auto rounded-lg border border-line px-3 py-1.5 text-[12px] text-muted hover:text-ink">
                清空
              </button>
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto rounded-2xl border border-line bg-surface p-3 font-mono text-[12px] leading-relaxed">
              {logs.length ? (
                logs.map((entry, index) => (
                  <div key={`${entry.atMs}-${index}`} className="flex gap-3">
                    <span className="shrink-0 text-muted">{formatClock(entry.atMs)}</span>
                    <span className={cn("shrink-0 uppercase", entry.level === "error" ? "text-bad" : entry.level === "warn" ? "text-warn" : "text-accent")}>
                      {entry.level}
                    </span>
                    <span className="break-all">{entry.message}</span>
                  </div>
                ))
              ) : (
                <div className="text-muted">暂无日志</div>
              )}
            </div>
          </section>
        )}
      </main>
    </div>
  );
}
