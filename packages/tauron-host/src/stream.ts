// ──────────────────────────────────────────────────────────────────────────
// 流式帧（R5 / P0-1）：与 Rust `tauron_host::stream::StreamFrame` 同构。
//
// 线形态由 wire-gate 锁死（两侧字段名/种类词表必须一致）。R5 之前「流式」只有
// 形状没有通路：`host_plugin_call` 的 `channel` 被当成不透明 JSON 收下、写完就丢，
// 于是 `kind: 'stream'` 的调用前端表现为「回调永不触发、也不报错」。
// ──────────────────────────────────────────────────────────────────────────

/** 帧种类闭集（与 Rust `StreamKind` 同构，大小写敏感）。 */
export type StreamKind = 'data' | 'end' | 'error';

/** 合法帧种类（运行时校验用；未知取值一律拒绝，不做兜底）。 */
export const STREAM_KINDS: readonly StreamKind[] = ['data', 'end', 'error'];

/** `end` / `error` 是终帧：发出后句柄失效。 */
export function isTerminalKind(kind: StreamKind): boolean {
  return kind === 'end' || kind === 'error';
}

/** 解析线值；未知取值返回 `null`（不抛，由调用方决定怎么处理）。 */
export function parseStreamKind(raw: unknown): StreamKind | null {
  return typeof raw === 'string' && (STREAM_KINDS as readonly string[]).includes(raw)
    ? (raw as StreamKind)
    : null;
}

/**
 * 一帧。
 *
 * `seq` 由**宿主**铸造（从 1 起、跨 `kind` 连续、终帧也占号）：前端自报序号就能
 * 伪造乱序/重复，接收方的去重假设随即失效。
 */
export interface StreamFrame {
  seq: number;
  kind: StreamKind;
  argsJson?: unknown;
  /**
   * 二进制载荷出口：字节到达接收方时仍是字节，不经 base64 夹带 JSON（§4.8 R6）。
   *
   * 这是 P0-1 的全部意义——在此之前二进制载荷**没有**任何线上表示。
   */
  argsRaw?: Uint8Array;
}

/** 运行时判定（宽松：只认必要字段的形状，便于跨传输边界校验）。 */
export function isStreamFrame(value: unknown): value is StreamFrame {
  if (typeof value !== 'object' || value === null) return false;
  const f = value as Record<string, unknown>;
  return (
    typeof f.seq === 'number' &&
    Number.isFinite(f.seq) &&
    parseStreamKind(f.kind) !== null &&
    (f.argsRaw === undefined || f.argsRaw instanceof Uint8Array)
  );
}
