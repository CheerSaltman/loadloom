import { useMemo, useState } from "react";
import { LineChart } from "@/components/LineChart";
import { cn, formatBitrate, formatBytes, formatClock, formatRate } from "@/lib/utils";
import type { NicAdapterClass, NicAdapterDto, NicEventKind, NicLinkState } from "@/bindings";
import type { NicState } from "@/hooks/useNic";

const CLASS_LABEL: Record<NicAdapterClass, string> = {
  physical: "物理网卡",
  virtual: "虚拟网卡",
  loopback: "回环",
  tunnel: "隧道",
  other: "其它接口",
};

const LINK_LABEL: Record<NicLinkState, string> = {
  connected: "已连接",
  disconnected: "已断开",
  dormant: "休眠",
  notPresent: "设备不存在",
  unknown: "状态未知",
};

const LINK_TONE: Record<NicLinkState, string> = {
  connected: "border-good/40 bg-good/10 text-good",
  disconnected: "border-bad/40 bg-bad/10 text-bad",
  dormant: "border-warn/40 bg-warn/10 text-warn",
  notPresent: "border-line bg-surface-2 text-muted",
  unknown: "border-line bg-surface-2 text-muted",
};

const EVENT_LABEL: Record<NicEventKind, string> = {
  linkUp: "链路恢复",
  linkDown: "链路断开",
  speedChange: "速率变化",
  counterReset: "计数器重置",
  discardSpike: "丢弃",
  errorSpike: "错误",
  queueBacklog: "队列积压",
  adapterAdded: "新增网卡",
  adapterRemoved: "移除网卡",
  selection: "监测范围",
};

/** 排序权重：与后端的展示顺序一致（物理在前，回环最后）。 */
function classRank(value: NicAdapterClass): number {
  return ["physical", "other", "virtual", "tunnel", "loopback"].indexOf(value);
}

function Pill({ children, tone }: { children: React.ReactNode; tone: string }) {
  return (
    <span className={cn("inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-[11.5px] font-semibold", tone)}>
      {children}
    </span>
  );
}

/** 利用率条：百分比来自后端（实时速率 ÷ 协商速率）。 */
function UtilizationBar({ label, value, tone }: { label: string; value: number; tone: string }) {
  const clamped = Math.max(0, Math.min(100, value));
  return (
    <div className="flex items-center gap-2">
      <span className="w-8 shrink-0 text-[11px] text-muted">{label}</span>
      <div className="h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-line-soft">
        <div className={cn("h-full rounded-full transition-[width]", tone)} style={{ width: `${clamped}%` }} />
      </div>
      <span className="w-12 shrink-0 text-right text-[11px] tabular-nums text-muted">{clamped.toFixed(1)}%</span>
    </div>
  );
}

function Metric({ label, value, tone }: { label: string; value: string; tone?: string }) {
  return (
    <div>
      <div className="text-[10.5px] tracking-wider text-muted">{label}</div>
      <div className={cn("text-[13px] tabular-nums", tone ?? "text-ink")}>{value}</div>
    </div>
  );
}

function AdapterCard({ adapter }: { adapter: NicAdapterDto }) {
  const problems =
    adapter.rxDiscardsPerSec + adapter.txDiscardsPerSec + adapter.rxErrorsPerSec + adapter.txErrorsPerSec;
  return (
    <div className="rounded-2xl border border-line bg-surface p-4">
      <div className="flex flex-wrap items-center gap-2">
        <span className={cn("size-2.5 rounded-full", adapter.linkState === "connected" ? "bg-good" : "bg-bad")} />
        <h3 className="text-[14px] font-bold">{adapter.name}</h3>
        <Pill tone={LINK_TONE[adapter.linkState]}>{LINK_LABEL[adapter.linkState]}</Pill>
        <Pill tone="border-line bg-surface-2 text-muted">{CLASS_LABEL[adapter.class]}</Pill>
        {adapter.queueBacklog ? <Pill tone="border-warn/40 bg-warn/10 text-warn">发送队列积压</Pill> : null}
        <span className="ml-auto text-[11.5px] text-muted">{adapter.media}</span>
      </div>
      <div className="mt-1.5 break-all text-[11.5px] text-muted">
        {adapter.description || "（驱动未提供描述）"}
      </div>

      <div className="mt-3 grid grid-cols-3 gap-3">
        <Metric label="协商速率" value={formatBitrate(Math.max(adapter.transmitSpeedBps, adapter.receiveSpeedBps))} />
        <Metric label="实时接收" value={formatRate(adapter.rxBps)} />
        <Metric label="实时发送" value={formatRate(adapter.txBps)} />
      </div>

      <div className="mt-3 space-y-1.5">
        <UtilizationBar label="收" value={adapter.rxUtilization} tone="bg-accent" />
        <UtilizationBar label="发" value={adapter.txUtilization} tone="bg-indigo" />
      </div>

      <div className="mt-3 grid grid-cols-3 gap-3">
        <Metric
          label="丢弃 / 秒（收 / 发）"
          value={`${adapter.rxDiscardsPerSec.toFixed(1)} / ${adapter.txDiscardsPerSec.toFixed(1)}`}
          tone={adapter.rxDiscardsPerSec + adapter.txDiscardsPerSec > 0 ? "text-warn" : "text-ink"}
        />
        <Metric
          label="错误 / 秒（收 / 发）"
          value={`${adapter.rxErrorsPerSec.toFixed(1)} / ${adapter.txErrorsPerSec.toFixed(1)}`}
          tone={adapter.rxErrorsPerSec + adapter.txErrorsPerSec > 0 ? "text-bad" : "text-ink"}
        />
        <Metric label="发送队列" value={`${adapter.outQueueLen} 包`} tone={adapter.queueBacklog ? "text-warn" : "text-ink"} />
      </div>

      <div className="mt-3 grid grid-cols-3 gap-3 border-t border-line-soft pt-3">
        <Metric label="累计接收 / 发送" value={`${formatBytes(adapter.rxBytesTotal)} / ${formatBytes(adapter.txBytesTotal)}`} />
        <Metric
          label="累计丢弃（收 / 发）"
          value={`${adapter.rxDiscardsTotal} / ${adapter.txDiscardsTotal}`}
          tone={adapter.rxDiscardsTotal + adapter.txDiscardsTotal > 0 ? "text-warn" : "text-ink"}
        />
        <Metric
          label="累计错误（收 / 发）"
          value={`${adapter.rxErrorsTotal} / ${adapter.txErrorsTotal}`}
          tone={adapter.rxErrorsTotal + adapter.txErrorsTotal > 0 ? "text-bad" : "text-ink"}
        />
      </div>

      <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-muted">
        <span>MTU {adapter.mtu || "—"}</span>
        <span>MAC {adapter.mac || "（无）"}</span>
        <span>状态 {adapter.operStatus}</span>
        <span>{adapter.adminEnabled ? "已启用" : "已禁用"}</span>
        {problems > 0 ? <span className="text-warn">当前存在丢弃 / 错误，详见下方事件</span> : null}
      </div>
    </div>
  );
}

/**
 * 网卡监测页。
 *
 * 三条信息按重要性从上到下排：**现在怎么样**（实时读数与利用率）、**刚才发生了什么**
 * （事件时间线）、**还有哪些网卡可选**（勾选面板）。温度、缓冲区占用率这类消费级
 * 拿不到的数据不在这里留空位 —— 页面顶部直接写明它们不可获得（`snapshot.note`）。
 */
export function NicPanel({ nic }: { nic: NicState }) {
  const [showAll, setShowAll] = useState(false);
  const [hint, setHint] = useState<string | null>(null);
  const snapshot = nic.snapshot;
  const monitored = snapshot?.adapters ?? [];

  const chart = useMemo(() => {
    const first = monitored[0];
    if (!first) return [];
    const points = nic.series[first.id] ?? [];
    return [
      { label: "接收", color: "var(--accent)", values: points.map((point) => point.rxBps) },
      { label: "发送", color: "var(--indigo)", values: points.map((point) => point.txBps) },
    ];
  }, [monitored, nic.series]);

  const list = useMemo(() => {
    const all = [...nic.adapters].sort(
      (left, right) => classRank(left.class) - classRank(right.class) || left.name.localeCompare(right.name),
    );
    return showAll ? all : all.filter((adapter) => adapter.class === "physical");
  }, [nic.adapters, showAll]);

  const copyReport = async () => {
    const ok = await nic.copyReport();
    setHint(ok ? "网卡报告已复制到剪贴板" : "复制失败：可改用「复制诊断信息」，或在日志页打开日志目录");
  };

  return (
    <div className="grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_384px] gap-4 overflow-y-auto p-5">
      <section className="space-y-4">
        <div className="flex flex-wrap items-center gap-2">
          <h2 className="text-[15px] font-bold">网卡链路监测</h2>
          <Pill tone={nic.connected ? "border-good/40 bg-good/10 text-good" : "border-bad/40 bg-bad/10 text-bad"}>
            {nic.connected ? `采样中 · ${snapshot?.sampleMs ?? 500} ms` : "未连接"}
          </Pill>
          {snapshot ? (
            <Pill tone={snapshot.supported ? "border-line bg-surface-2 text-muted" : "border-warn/40 bg-warn/10 text-warn"}>
              {snapshot.supported ? `监测 ${monitored.length} 块网卡` : "当前平台不支持"}
            </Pill>
          ) : null}
          <div className="ml-auto flex flex-wrap items-center gap-2">
            <button
              onClick={() => void copyReport()}
              className="rounded-lg border border-line px-3 py-1.5 text-[12px] text-muted hover:text-ink"
            >
              复制网卡报告
            </button>
            <button
              onClick={() => void nic.refreshAdapters()}
              className="rounded-lg border border-line px-3 py-1.5 text-[12px] text-muted hover:text-ink"
            >
              刷新接口列表
            </button>
            <button
              onClick={() => void nic.resetSelection()}
              className="rounded-lg border border-line px-3 py-1.5 text-[12px] text-muted hover:text-ink"
            >
              恢复默认范围
            </button>
          </div>
        </div>

        <div className="rounded-xl border border-line-soft bg-surface-2 px-3 py-2 text-[11.5px] leading-relaxed text-muted">
          {snapshot?.note ?? "正在读取网卡能力说明…"}
        </div>
        {hint ? <div className="text-[12px] text-accent">{hint}</div> : null}
        {nic.error ? <div className="text-[12px] text-bad">{nic.error}</div> : null}
        {snapshot?.lastError ? (
          <div className="text-[12px] text-warn">最近一次采样失败：{snapshot.lastError}</div>
        ) : null}

        {monitored.length ? (
          monitored.map((adapter) => <AdapterCard key={adapter.id} adapter={adapter} />)
        ) : (
          <div className="rounded-2xl border border-line bg-surface p-6 text-[13px] text-muted">
            没有正在监测的网卡。请在下方勾选至少一块（默认监测全部物理网卡）。
          </div>
        )}

        <div className="rounded-2xl border border-line bg-surface p-4">
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <h3 className="text-[13.5px] font-bold">监测范围</h3>
            <span className="text-[11.5px] text-muted">
              勾选结果会记住；未勾选任何网卡时按默认范围（全部物理网卡）监测
            </span>
            <label className="ml-auto flex items-center gap-1.5 text-[12px] text-muted">
              <input type="checkbox" checked={showAll} onChange={(event) => setShowAll(event.target.checked)} />
              显示虚拟 / 隧道网卡
            </label>
          </div>
          <div className="grid grid-cols-2 gap-x-4 gap-y-1.5">
            {list.map((adapter) => (
              <label key={adapter.id} className="flex items-start gap-2 text-[12px]">
                <input
                  type="checkbox"
                  className="mt-0.5"
                  checked={adapter.monitored}
                  onChange={() => void nic.toggleAdapter(adapter.id)}
                />
                <span className="min-w-0">
                  <span className="font-semibold">{adapter.name}</span>
                  <span className="text-muted"> · {CLASS_LABEL[adapter.class]}</span>
                  <span className="block truncate text-[11px] text-muted" title={adapter.description}>
                    {adapter.description || "（驱动未提供描述）"}
                  </span>
                </span>
                <span className="ml-auto shrink-0 text-[11px] text-muted">
                  {adapter.linkState === "connected" ? formatBitrate(Math.max(adapter.transmitSpeedBps, adapter.receiveSpeedBps)) : LINK_LABEL[adapter.linkState]}
                </span>
              </label>
            ))}
            {list.length === 0 ? <div className="text-[12px] text-muted">未发现任何接口</div> : null}
          </div>
        </div>
      </section>

      <section className="space-y-4">
        <div className="rounded-2xl border border-line bg-surface p-4">
          <div className="mb-3 flex items-center gap-3 text-[12px] text-muted">
            <h3 className="text-[13.5px] font-bold text-ink">收发速率（2 分钟）</h3>
            <span className="text-accent">■ 接收</span>
            <span className="text-indigo">■ 发送</span>
          </div>
          {monitored.length ? (
            <>
              <LineChart series={chart} height={180} />
              <div className="mt-2 text-[11.5px] text-muted">曲线对应：{monitored[0]?.name}</div>
            </>
          ) : (
            <div className="text-[12px] text-muted">暂无可绘制的数据</div>
          )}
        </div>

        <div className="rounded-2xl border border-line bg-surface p-4">
          <h3 className="mb-3 text-[13.5px] font-bold">事件时间线</h3>
          <div className="max-h-[420px] space-y-2 overflow-y-auto">
            {nic.events.length ? (
              nic.events.map((event, index) => (
                <div key={`${event.atMs}-${index}`} className="flex gap-2 text-[12px]">
                  <span className="shrink-0 tabular-nums text-muted">{formatClock(event.atMs)}</span>
                  <span className="shrink-0 font-mono text-indigo">{event.code}</span>
                  <span className="min-w-0 break-all">
                    <span
                      className={cn(
                        "mr-1 font-semibold",
                        event.resolved ? "text-good" : event.kind === "linkDown" || event.kind === "speedChange" ? "text-warn" : "text-ink",
                      )}
                    >
                      {EVENT_LABEL[event.kind]}
                    </span>
                    {event.message}
                  </span>
                </div>
              ))
            ) : (
              <div className="text-[12px] text-muted">暂无事件。链路通断、协商速率变化、丢弃 / 错误 / 队列积压都会记录在这里。</div>
            )}
          </div>
        </div>
      </section>
    </div>
  );
}
