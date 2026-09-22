import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

/** shadcn 约定的类名合并工具。 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}

const KIB = 1024;
const MIB = 1024 * 1024;
const GIB = 1024 * 1024 * 1024;
const TIB = 1024 * 1024 * 1024 * 1024;

export function formatBytes(bytes: number): string {
  if (bytes >= TIB) return `${(bytes / TIB).toFixed(3)} TB`;
  if (bytes >= GIB) return `${(bytes / GIB).toFixed(2)} GB`;
  if (bytes >= MIB) return `${(bytes / MIB).toFixed(2)} MB`;
  if (bytes >= KIB) return `${(bytes / KIB).toFixed(1)} KB`;
  return `${Math.round(bytes)} B`;
}

export function formatRate(bps: number): string {
  return `${(bps / MIB).toFixed(2)} MB/s`;
}

export function formatGbps(bps: number): string {
  return `${((bps * 8) / 1e9).toFixed(3)} Gbps`;
}

/**
 * 链路速率（bit/s）的人类读法。
 *
 * 与 `formatGbps` 分开：解析协商速率时「1.00 Gbps / 100 Mbps」比
 * 「1.000 Gbps / 0.100 Gbps」更贴近用户在系统里看到的样子（Windows 也这么显示）。
 */
export function formatBitrate(bps: number): string {
  if (!Number.isFinite(bps) || bps <= 0) return "未知";
  if (bps >= 1e9) return `${(bps / 1e9).toFixed(2)} Gbps`;
  if (bps >= 1e6) return `${(bps / 1e6).toFixed(0)} Mbps`;
  if (bps >= 1e3) return `${(bps / 1e3).toFixed(0)} Kbps`;
  return `${Math.round(bps)} bit/s`;
}

/**
 * 复制文本到剪贴板。
 *
 * 优先用 Clipboard API，失败后退回 `execCommand`：Tauri 的 WebView2 在部分
 * 权限 / 焦点状态下会拒绝前者，而「复制诊断信息」恰恰是故障时最需要能用的按钮，
 * 不能因为剪贴板策略变化就静默失效（两个都失败时返回 false，由调用方如实提示）。
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // 继续走兜底路径。
  }
  try {
    const area = document.createElement("textarea");
    area.value = text;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(area);
    return ok;
  } catch {
    return false;
  }
}

export function formatDuration(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  const hh = String(Math.floor(total / 3600)).padStart(2, "0");
  const mm = String(Math.floor((total % 3600) / 60)).padStart(2, "0");
  const ss = String(total % 60).padStart(2, "0");
  return `${hh}:${mm}:${ss}`;
}

export function formatClock(atMs: number): string {
  const total = Math.floor(atMs / 1000);
  const mm = String(Math.floor((total % 3600) / 60)).padStart(2, "0");
  const ss = String(total % 60).padStart(2, "0");
  const cs = String(Math.floor((atMs % 1000) / 100));
  return `${mm}:${ss}.${cs}`;
}

/** 组合 URL：允许用户直接粘贴完整 URL，也支持 scheme/host/port/path 分栏填写。 */
export function composeUrl(
  raw: string,
  scheme: string,
  port: string,
  path: string,
): string {
  const host = raw.trim();
  if (!host) return "";
  if (host.includes("://")) return host;
  const normalizedPath = (() => {
    const trimmed = path.trim() || "/";
    return trimmed.startsWith("/") ? trimmed : `/${trimmed}`;
  })();
  const portPart = port.trim() ? `:${port.trim()}` : "";
  return `${scheme}://${host}${portPart}${normalizedPath}`;
}
