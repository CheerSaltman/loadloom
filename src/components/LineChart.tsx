import { useEffect, useRef } from "react";

export interface Series {
  label: string;
  color: string;
  values: number[];
}

interface LineChartProps {
  series: Series[];
  height?: number;
}

/**
 * 轻量 Canvas 折线图：**零第三方依赖、零 CDN**（桌面应用必须离线可用）。
 */
export function LineChart({ series, height = 200 }: LineChartProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const context = canvas.getContext("2d");
    if (!context) return;

    const ratio = window.devicePixelRatio || 1;
    const width = canvas.clientWidth || 640;
    const cssHeight = canvas.clientHeight || height;
    if (canvas.width !== Math.floor(width * ratio) || canvas.height !== Math.floor(cssHeight * ratio)) {
      canvas.width = Math.floor(width * ratio);
      canvas.height = Math.floor(cssHeight * ratio);
    }
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, width, cssHeight);

    const styles = getComputedStyle(document.documentElement);
    const gridColor = styles.getPropertyValue("--line-soft").trim() || "#172238";
    const mutedColor = styles.getPropertyValue("--muted").trim() || "#8ea1c2";

    context.strokeStyle = gridColor;
    context.lineWidth = 1;
    for (let line = 1; line <= 4; line += 1) {
      const y = cssHeight - (cssHeight * line) / 5;
      context.beginPath();
      context.moveTo(0, y);
      context.lineTo(width, y);
      context.stroke();
    }

    let max = 1;
    for (const item of series) {
      for (const value of item.values) {
        if (Number.isFinite(value) && value > max) max = value;
      }
    }
    max *= 1.15;

    context.font = "11px system-ui, sans-serif";
    context.fillStyle = mutedColor;
    context.fillText(formatAxis(max), 6, 12);

    const step = width / Math.max(1, series[0]?.values.length ? series[0].values.length - 1 : 1);
    for (const item of series) {
      const points = item.values;
      if (points.length < 2) continue;
      context.beginPath();
      points.forEach((value, index) => {
        const x = index * step;
        const y = cssHeight - (Math.max(0, value) / max) * (cssHeight - 10) - 4;
        if (index === 0) context.moveTo(x, y);
        else context.lineTo(x, y);
      });
      context.strokeStyle = item.color;
      context.lineWidth = 2;
      context.stroke();
      context.lineTo((points.length - 1) * step, cssHeight);
      context.lineTo(0, cssHeight);
      context.closePath();
      context.globalAlpha = 0.16;
      context.fillStyle = item.color;
      context.fill();
      context.globalAlpha = 1;
    }
  }, [series, height]);

  return <canvas ref={canvasRef} style={{ width: "100%", height }} />;
}

function formatAxis(value: number): string {
  if (value >= 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)} MB/s`;
  if (value >= 1024) return `${(value / 1024).toFixed(1)} KB/s`;
  return `${value.toFixed(1)}`;
}
