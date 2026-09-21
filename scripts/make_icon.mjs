// Generates a 1024x1024 RGBA PNG app icon with zero third-party deps.
// Design: dark rounded square + cyan traffic waveform (matches app accent #38bdf8).
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";

const S = 1024;

/* ---------- PNG encoding ---------- */
const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const body = Buffer.concat([Buffer.from(type, "latin1"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body), 0);
  return Buffer.concat([len, body, crc]);
}

function encodePng(rgba, width, height) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // color type RGBA
  ihdr[10] = 0;
  ihdr[11] = 0;
  ihdr[12] = 0;

  const stride = width * 4;
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (stride + 1)] = 0; // filter: none
    rgba.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
  }

  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

/* ---------- math helpers ---------- */
const clamp01 = (v) => (v < 0 ? 0 : v > 1 ? 1 : v);
const lerp = (a, b, t) => a + (b - a) * t;
const mix = (a, b, t) => [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)];

// signed distance to rounded rectangle centered at (cx,cy)
function sdRoundRect(px, py, cx, cy, hw, hh, r) {
  const qx = Math.abs(px - cx) - (hw - r);
  const qy = Math.abs(py - cy) - (hh - r);
  const ax = Math.max(qx, 0);
  const ay = Math.max(qy, 0);
  return Math.sqrt(ax * ax + ay * ay) + Math.min(Math.max(qx, qy), 0) - r;
}

// signed distance from point to a segment
function sdSegment(px, py, ax, ay, bx, by) {
  const vx = bx - ax;
  const vy = by - ay;
  const wx = px - ax;
  const wy = py - ay;
  const len2 = vx * vx + vy * vy;
  const t = len2 === 0 ? 0 : clamp01((wx * vx + wy * vy) / len2);
  const cx = ax + vx * t;
  const cy = ay + vy * t;
  return Math.hypot(px - cx, py - cy);
}

/* ---------- design ---------- */
const BG_TOP = [0x0a, 0x12, 0x24];
const BG_BOTTOM = [0x10, 0x1d, 0x38];
const ACCENT = [0x38, 0xbd, 0xf8];
const ACCENT_SOFT = [0x0e, 0xa5, 0xe9];

// normalized pulse polyline (t across, v downward)
const pulse = [
  [0.0, 0.52],
  [0.2, 0.52],
  [0.3, 0.2],
  [0.4, 0.84],
  [0.5, 0.36],
  [0.6, 0.68],
  [0.72, 0.52],
  [1.0, 0.52],
];

const pad = 0.16 * S;
const pts = pulse.map(([t, v]) => [pad + t * (S - 2 * pad), pad + v * (S - 2 * pad)]);

const CORNER = 0.22 * S;
const LINE_HALF = 0.028 * S + 2.0; // stroke half width
const GLOW_HALF = 0.105 * S;

const out = Buffer.alloc(S * S * 4);

for (let y = 0; y < S; y++) {
  for (let x = 0; x < S; x++) {
    const px = x + 0.5;
    const py = y + 0.5;

    // rounded-square mask (1px AA)
    const dRect = sdRoundRect(px, py, S / 2, S / 2, S / 2, S / 2, CORNER);
    const mask = clamp01(0.5 - dRect);
    if (mask <= 0) continue; // stays transparent

    // vertical gradient background
    let col = mix(BG_TOP, BG_BOTTOM, py / S);

    // distance to the pulse polyline
    let dLine = Infinity;
    for (let i = 0; i < pts.length - 1; i++) {
      const d = sdSegment(px, py, pts[i][0], pts[i][1], pts[i + 1][0], pts[i + 1][1]);
      if (d < dLine) dLine = d;
    }

    // outer glow
    const glow = clamp01(1 - dLine / GLOW_HALF);
    col = mix(col, ACCENT_SOFT, 0.42 * glow * glow);

    // crisp stroke
    const stroke = clamp01(LINE_HALF + 0.5 - dLine);
    col = mix(col, ACCENT, stroke);

    // bright core
    const core = clamp01(0.34 * LINE_HALF + 0.5 - dLine);
    col = mix(col, [0xe0, 0xf2, 0xfe], core * 0.85);

    const o = (y * S + x) * 4;
    out[o] = Math.round(col[0]);
    out[o + 1] = Math.round(col[1]);
    out[o + 2] = Math.round(col[2]);
    out[o + 3] = Math.round(mask * 255);
  }
}

const target = process.argv[2] || "app-icon.png";
writeFileSync(target, encodePng(out, S, S));
console.log(`wrote ${target} (${S}x${S}, RGBA)`);
