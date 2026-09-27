// ──────────────────────────────────────────────────────────────────────────
// @tauron/app-cli — 占位图标生成（一键脚手架的「最后一公里」）。
//
// 为什么需要它：`tauri-build` 在 **Windows 上生成资源文件时会读
// `src-tauri/icons/icon.ico`**，缺它连 `cargo check` 都跑不过（不只是 `tauri build`）。
// 也就是说，一个不产图标的脚手架产出的是「装完不能编译」的工程——这跟「一键」矛盾。
//
// 这里生成的是**占位图**（纯代码构造的 PNG / ICO / ICNS，不是品牌资产），并在调用方
// 如实提示「请替换」。选择「代码构造二进制」而不是「内联 base64 大常量」的理由：
// base64 常量在源码里不可读、不可审、体积固定且无法换尺寸。
//
// 编码实现的诚实边界：
// - PNG 用 zlib 的 **stored（不压缩）** 块 + 正确的 CRC32 / Adler32——合法可解码，
//   只是体积大（占位图无所谓）。不引入任何第三方依赖（本包是零依赖 CLI）。
// - ICO 用 32bpp BGRA 的 BMP 载体 + 全零 AND 掩码（全不透明）。
// - ICNS 用 `ic07`（128×128 PNG 载荷）容器。
// ──────────────────────────────────────────────────────────────────────────

/** 占位图的背景色（深板岩）与前景色（tauron 蓝）。 */
const BACKGROUND: readonly [number, number, number] = [0x1f, 0x29, 0x37];
const FOREGROUND: readonly [number, number, number] = [0x3b, 0x82, 0xf6];

// ── 校验和 ──

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) {
      c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(bytes: Uint8Array): number {
  let c = 0xffffffff;
  for (const byte of bytes) {
    c = CRC_TABLE[(c ^ byte) & 0xff]! ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}

function adler32(bytes: Uint8Array): number {
  let a = 1;
  let b = 0;
  for (const byte of bytes) {
    a = (a + byte) % 65521;
    b = (b + a) % 65521;
  }
  return ((b << 16) | a) >>> 0;
}

// ── 字节辅助 ──

function concat(chunks: readonly Uint8Array[]): Uint8Array {
  const total = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
}

function u32be(value: number): Uint8Array {
  return new Uint8Array([
    (value >>> 24) & 0xff,
    (value >>> 16) & 0xff,
    (value >>> 8) & 0xff,
    value & 0xff,
  ]);
}

function u32le(value: number): Uint8Array {
  return new Uint8Array([
    value & 0xff,
    (value >>> 8) & 0xff,
    (value >>> 16) & 0xff,
    (value >>> 24) & 0xff,
  ]);
}

// ── zlib（stored 块） ──

const MAX_STORED_BLOCK = 0xffff;

function zlibStored(raw: Uint8Array): Uint8Array {
  // zlib 头：CMF=0x78（deflate/32K 窗口）、FLG=0x01（无预设字典，校验通过）
  const chunks: Uint8Array[] = [new Uint8Array([0x78, 0x01])];
  if (raw.length === 0) {
    chunks.push(new Uint8Array([0x01, 0x00, 0x00, 0xff, 0xff]));
  }
  for (let offset = 0; offset < raw.length; offset += MAX_STORED_BLOCK) {
    const length = Math.min(MAX_STORED_BLOCK, raw.length - offset);
    const isLast = offset + length === raw.length ? 1 : 0;
    const header = new Uint8Array(5);
    header[0] = isLast;
    header[1] = length & 0xff;
    header[2] = (length >>> 8) & 0xff;
    header[3] = ~length & 0xff;
    header[4] = (~length >>> 8) & 0xff;
    chunks.push(header, raw.subarray(offset, offset + length));
  }
  chunks.push(u32be(adler32(raw)));
  return concat(chunks);
}

// ── PNG ──

function pngChunk(type: string, data: Uint8Array): Uint8Array {
  const typeBytes = new TextEncoder().encode(type);
  return concat([u32be(data.length), typeBytes, data, u32be(crc32(concat([typeBytes, data])))]);
}

/**
 * 逐像素画出占位图的配色：外圈背景色，内部 60% 区域前景色，边界留 1px 过渡。
 *
 * 纯函数、无随机——同一尺寸每次产出**逐字节相同**的图（可提交进版本库、可断言）。
 */
function pixelAt(x: number, y: number, size: number): readonly [number, number, number] {
  const margin = Math.max(1, Math.round(size * 0.2));
  const inside = x >= margin && x < size - margin && y >= margin && y < size - margin;
  return inside ? FOREGROUND : BACKGROUND;
}

/** 生成一张 `size × size` 的 RGBA PNG（8 位色深、无透明、不压缩）。 */
export function placeholderPng(size: number): Uint8Array {
  const raw = new Uint8Array((size * 4 + 1) * size);
  let offset = 0;
  for (let y = 0; y < size; y++) {
    raw[offset++] = 0; // 扫描线过滤器：None
    for (let x = 0; x < size; x++) {
      const [r, g, b] = pixelAt(x, y, size);
      raw[offset++] = r;
      raw[offset++] = g;
      raw[offset++] = b;
      raw[offset++] = 0xff;
    }
  }
  const ihdr = concat([
    u32be(size),
    u32be(size),
    new Uint8Array([8, 6, 0, 0, 0]), // 8 位 / 颜色类型 6（RGBA）/ 无压缩 / 无过滤 / 非隔行
  ]);
  return concat([
    new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk('IHDR', ihdr),
    pngChunk('IDAT', zlibStored(raw)),
    pngChunk('IEND', new Uint8Array(0)),
  ]);
}

// ── ICO ──

/** 生成一张 `size × size` 的 32bpp BMP（ICO 的载荷，行序自下而上）。 */
function icoBitmap(size: number): Uint8Array {
  const header = concat([
    u32le(40), // BITMAPINFOHEADER 大小
    u32le(size),
    u32le(size * 2), // 高度含 AND 掩码，故为 2 倍
    new Uint8Array([1, 0]), // planes = 1
    new Uint8Array([32, 0]), // bitCount = 32
    u32le(0), // 无压缩
    u32le(0), // sizeImage（BI_RGB 可为 0）
    u32le(0),
    u32le(0),
    u32le(0),
    u32le(0),
  ]);
  const xor = new Uint8Array(size * size * 4);
  let offset = 0;
  for (let y = size - 1; y >= 0; y--) {
    for (let x = 0; x < size; x++) {
      const [r, g, b] = pixelAt(x, y, size);
      xor[offset++] = b;
      xor[offset++] = g;
      xor[offset++] = r;
      xor[offset++] = 0xff;
    }
  }
  // AND 掩码：每行按 32 位对齐；全 0 表示「按 XOR 的 alpha 决定不透明」
  const maskRowBytes = Math.ceil(size / 32) * 4;
  const mask = new Uint8Array(maskRowBytes * size);
  return concat([header, xor, mask]);
}

/** 生成单帧 `size × size` 的 ICO。 */
export function placeholderIco(size = 32): Uint8Array {
  const bitmap = icoBitmap(size);
  const dir = concat([
    new Uint8Array([0, 0]), // reserved
    new Uint8Array([1, 0]), // type = 1（图标）
    new Uint8Array([1, 0]), // 帧数
  ]);
  const entry = concat([
    new Uint8Array([size >= 256 ? 0 : size, size >= 256 ? 0 : size, 0, 0]),
    new Uint8Array([1, 0, 32, 0]),
    u32le(bitmap.length),
    u32le(dir.length + 16),
  ]);
  return concat([dir, entry, bitmap]);
}

// ── ICNS ──

/** 生成只含 `ic07`（128×128 PNG）一帧的 ICNS。 */
export function placeholderIcns(size = 128): Uint8Array {
  const payload = placeholderPng(size);
  const body = concat([new TextEncoder().encode('ic07'), u32be(payload.length), payload]);
  const total = 8 + body.length;
  return concat([new TextEncoder().encode('icns'), u32be(total), body]);
}

// ── 对外 ──

/**
 * 生成 `src-tauri/icons/` 下的整套占位图标。
 *
 * 覆盖三个打包目标各自的最小要求：Windows 要 `icon.ico`、macOS 要 `icon.icns`、
 * Linux 要 `32x32.png` / `128x128.png`（`tauri-bundler` 的默认图标名）。
 */
export function placeholderIconFiles(): Map<string, Uint8Array> {
  return new Map<string, Uint8Array>([
    ['32x32.png', placeholderPng(32)],
    ['128x128.png', placeholderPng(128)],
    ['128x128@2x.png', placeholderPng(256)],
    ['icon.png', placeholderPng(512)],
    ['icon.ico', placeholderIco(32)],
    ['icon.icns', placeholderIcns(128)],
  ]);
}
