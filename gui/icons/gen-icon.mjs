// Generates icons/icon.ico (multi-size BMP entries, no image libraries).
// Draw: rounded-square blue→violet gradient with a white "M" monogram.
// Run: node gen-icon.mjs   (from gui/icons/)
import { writeFileSync } from 'node:fs';

const SIZES = [16, 32, 48, 64, 128, 256];

// Rounded-rect signed distance (in px units): negative inside.
function sdRoundRect(x, y, cx, cy, hw, hh, r) {
  const dx = Math.abs(x - cx) - (hw - r);
  const dy = Math.abs(y - cy) - (hh - r);
  const ax = Math.max(dx, 0);
  const ay = Math.max(dy, 0);
  return Math.hypot(ax, ay) + Math.min(Math.max(dx, dy), 0) - r;
}

// Distance from point to segment.
function sdSegment(px, py, ax, ay, bx, by) {
  const abx = bx - ax, aby = by - ay;
  const t = Math.max(0, Math.min(1, ((px - ax) * abx + (py - ay) * aby) / (abx * abx + aby * aby)));
  return Math.hypot(px - (ax + t * abx), py - (ay + t * aby));
}

function lerp(a, b, t) { return a + (b - a) * t; }

// Coverage (0..1) of the white "M" at normalized coords u,v in [0,1].
function mGlyph(u, v) {
  const left = u > 0.14 && u < 0.32;
  const right = u > 0.68 && u < 0.86;
  const inBox = v > 0.10 && v < 0.84 && u > 0.10 && u < 0.90;
  const bars = (left || right) && inBox;
  const d1 = sdSegment(u, v, 0.23, 0.10, 0.50, 0.46);
  const d2 = sdSegment(u, v, 0.77, 0.10, 0.50, 0.46);
  const diag = v > 0.06 && v < 0.52 && d1 < 0.085 && d2 < 0.085;
  return bars || diag ? 1 : 0;
}

function renderPixel(s, x, y) {
  // Background gradient: #3f6fe8 → #8a4ae0 along the diagonal.
  const t = (x / s + y / s) / 2;
  let r = lerp(0x3f, 0x8a, t);
  let g = lerp(0x6f, 0x4a, t);
  let b = lerp(0xe8, 0xe0, t);
  const u = x / s, v = y / s;
  const glyph = mGlyph(u, v);
  if (glyph > 0) { r = g = b = 255; }
  return [Math.round(r), Math.round(g), Math.round(b)];
}

function renderIcon(s) {
  const px = new Uint8Array(s * s * 4);
  const SS = 3; // supersample
  for (let y = 0; y < s; y++) {
    for (let x = 0; x < s; x++) {
      // Anti-aliased rounded-square coverage + glyph, supersampled.
      let bgSum = 0;
      const cols = [0, 0, 0];
      let glyphSum = 0;
      for (let sy = 0; sy < SS; sy++) {
        for (let sx = 0; sx < SS; sx++) {
          const fx = x + (sx + 0.5) / SS;
          const fy = y + (sy + 0.5) / SS;
          const d = sdRoundRect(fx, fy, s / 2, s / 2, s / 2 - 0.5, s / 2 - 0.5, s * 0.225);
          const cov = Math.max(0, Math.min(1, 0.5 - d));
          bgSum += cov;
          if (cov > 0.5) glyphSum += mGlyph(fx / s, fy / s);
        }
      }
      const bgCov = bgSum / (SS * SS);
      const glyphCov = glyphSum / (SS * SS);
      const t = (x / s + y / s) / 2;
      let r = lerp(0x3f, 0x8a, t);
      let g = lerp(0x6f, 0x4a, t);
      let b = lerp(0xe8, 0xe0, t);
      r = lerp(r, 255, glyphCov);
      g = lerp(g, 255, glyphCov);
      b = lerp(b, 255, glyphCov);
      const i = (y * s + x) * 4;
      px[i] = Math.round(b * bgCov);        // BGRA
      px[i + 1] = Math.round(g * bgCov);
      px[i + 2] = Math.round(r * bgCov);
      px[i + 3] = Math.round(255 * bgCov);
    }
  }
  return px;
}

// BMP entry: BITMAPINFOHEADER + bottom-up BGRA rows + empty AND mask.
function bmpEntry(s, px) {
  const maskRow = Math.ceil(s / 32) * 4;
  const maskSize = maskRow * s;
  const buf = Buffer.alloc(40 + s * s * 4 + maskSize);
  buf.writeUInt32LE(40, 0);
  buf.writeInt32LE(s, 4);
  buf.writeInt32LE(s * 2, 8); // height doubled: XOR + AND
  buf.writeUInt16LE(1, 12);
  buf.writeUInt16LE(32, 14);
  for (let y = 0; y < s; y++) {
    const src = y * s * 4;
    const dst = 40 + (s - 1 - y) * s * 4;
    Buffer.from(px.buffer, src, s * 4).copy(buf, dst);
  }
  return buf;
}

const images = SIZES.map((s) => ({ s, data: bmpEntry(s, renderIcon(s)) }));
const header = Buffer.alloc(6 + images.length * 16);
header.writeUInt16LE(1, 2); // type: icon
header.writeUInt16LE(images.length, 4); // entry count
let offset = header.length;
images.forEach((img, i) => {
  const e = 6 + i * 16;
  header.writeUInt8(img.s >= 256 ? 0 : img.s, e);
  header.writeUInt8(img.s >= 256 ? 0 : img.s, e + 1);
  header.writeUInt16LE(1, e + 4);  // planes
  header.writeUInt16LE(32, e + 6); // bpp
  header.writeUInt32LE(img.data.length, e + 8);
  header.writeUInt32LE(offset, e + 12);
  offset += img.data.length;
});
writeFileSync(new URL('./icon.ico', import.meta.url), Buffer.concat([header, ...images.map((i) => i.data)]));
console.log(`icon.ico written: ${SIZES.join('/')} px`);
