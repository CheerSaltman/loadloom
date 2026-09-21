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
