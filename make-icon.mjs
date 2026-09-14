// 生成 1024x1024 应用图标 app-icon.png（纯 Node，无外部依赖）
// 主题：深色底 + 渐变闪电圆，代表「AI 测速」
import { writeFileSync } from "node:fs";
import { deflateSync } from "node:zlib";

const S = 1024;

// RGBA 帧缓冲
const px = Buffer.alloc(S * S * 4);

function setPx(x, y, r, g, b, a = 255) {
  if (x < 0 || y < 0 || x >= S || y >= S) return;
  const i = (y * S + x) * 4;
  // alpha 混合到深色底之上一次即可（此图无重叠半透明绘制顺序问题）
  const af = a / 255;
  px[i] = Math.round(px[i] * (1 - af) + r * af);
  px[i + 1] = Math.round(px[i + 1] * (1 - af) + g * af);
  px[i + 2] = Math.round(px[i + 2] * (1 - af) + b * af);
  px[i + 3] = Math.min(255, Math.round(px[i + 3] + a));
}

// 圆角方形 SDF
const R = 235; // 圆角半径
function insideSquircle(x, y, half = 512, radius = R) {
  const qx = Math.abs(x - 512) - (half - radius);
  const qy = Math.abs(y - 512) - (half - radius);
  const d = Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - radius;
  return d;
}

// 背景：深到略亮的斜向渐变
for (let y = 0; y < S; y++) {
  for (let x = 0; x < S; x++) {
    const d = insideSquircle(x, y);
    if (d > 0.5) continue; // 圆角外保持透明
    const t = (x + y) / (2 * S);
    const edge = d > -0.5 ? (0.5 + d) * 255 : 255; // 简单抗锯齿
    setPx(x, y, Math.round(13 + t * 14), Math.round(16 + t * 16), Math.round(26 + t * 22), Math.round(255 * Math.min(1, edge / 255 + (d > -0.5 ? 0 : 1))));
  }
}

// 闪电形状（多边形），用 even-odd 射线法判断
const bolt = [
  [585, 165], [365, 560], [505, 560], [420, 860], [690, 430], [540, 430], [680, 165],
];
function inPoly(x, y) {
  let c = false;
  for (let i = 0, j = bolt.length - 1; i < bolt.length; j = i++) {
    const [xi, yi] = bolt[i], [xj, yj] = bolt[j];
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) c = !c;
  }
  return c;
}

// 渐变：#5eead4 -> #818cf8（左上到右下）
function grad(t) {
  return [
    Math.round(0x5e + (0x81 - 0x5e) * t),
    Math.round(0xea + (0x8c - 0xea) * t),
    Math.round(0xd4 + (0xf8 - 0xd4) * t),
  ];
}

for (let y = 165; y <= 860; y++) {
  for (let x = 365; x <= 690; x++) {
    if (!inPoly(x, y)) continue;
    const t = (x - 365 + (y - 165)) / (325 + 695);
    const [r, g, b] = grad(t);
    setPx(x, y, r, g, b);
  }
}

// 光晕：闪电周边像素提亮（简易 bloom）
const glow = Buffer.from(px);
for (let y = 2; y < S - 2; y++) {
  for (let x = 2; x < S - 2; x++) {
    const i = (y * S + x) * 4;
    if (glow[i + 3] > 0 && px[i + 3] > 0) {
      for (const [dx, dy] of [[3, 0], [-3, 0], [0, 3], [0, -3]]) {
        const j = ((y + dy) * S + (x + dx)) * 4;
        if (px[j + 3] === 0) {
          setPx(x + dx, y + dy, glow[i], glow[i + 1], glow[i + 2], 60);
        }
      }
    }
  }
}

// ---------- 编码为 PNG ----------
function crc32(buf) {
  let c, table = crc32.t || (crc32.t = (() => {
    const t = [];
    for (let n = 0; n < 256; n++) {
      c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      t[n] = c >>> 0;
    }
    return t;
  })());
  c = 0xffffffff;
  for (const b of buf) c = table[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type), data]);
  const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

const raw = Buffer.alloc(S * (1 + S * 4));
for (let y = 0; y < S; y++) {
  raw[y * (1 + S * 4)] = 0; // filter none
  px.copy(raw, y * (1 + S * 4) + 1, y * S * 4, (y + 1) * S * 4);
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0); ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8; ihdr[9] = 6; // 8bit RGBA

const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

writeFileSync("app-icon.png", png);
console.log("app-icon.png:", png.length, "bytes");
