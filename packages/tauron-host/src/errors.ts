// ──────────────────────────────────────────────────────────────────────────
// 结构化错误：与 `tauron-host::error::ErrorCode` 的线上协议名一一对应。
//
// ADR-04：宿主永远抛结构化错误，从不让 panic 抵达 JS 侧。
// 这里**不**定义任何 Tauri 依赖；所有错误对象都可被 JSON 序列化。
// ──────────────────────────────────────────────────────────────────────────

/** 线上错误码全集（与 Rust `ErrorCode` 枚举同序，共 20 个）。 */
export const HOST_ERROR_CODES = [
  'E_HOST_PANIC',
  'E_UNKNOWN_PLUGIN',
  'E_AUTH_DENIED',
  'E_INVALID_MANIFEST',
  'E_STATE_INVALID_TRANSITION',
  'E_CALL_NOT_FOUND',
  'E_CALL_TIMEOUT',
  'E_FORBIDDEN_PERMISSION',
  'E_ABI_MISMATCH',
  'E_PLUGIN_DISABLED',
  'E_REGISTRY_FULL',
  'E_CALL_PENDING_FULL',
  'E_SUBSCRIPTION_FULL',
  'E_PLUGIN_EXISTS',
  'E_INSTALL_FAILED',
  'E_PLUGIN_FILTERED',
  /**
   * 该插件类型没有运行期执行器（对 js/rust/wasm 插件调 `host_runtime_spawn`）。
   *
   * 诚实失败码：不静默成功、也不伪造一个 pid（P1-2 的 wasm 执行器同用）。
   */
  'E_PLUGIN_TYPE_NO_RUNTIME',
  /**
   * 运行时租约不存在或已失效（进程已回收 / 宿主已重启）。
   *
   * **不是** `E_CALL_NOT_FOUND`：这是租约语义，调用方的下一个动作不同
   * （重新 spawn，而不是放弃一次 pending 调用）。
   */
  'E_LEASE_EXPIRED',
  /**
   * 流句柄数已达上限（同一进程内并发打开的流太多）。
   *
   * **不是** `E_REGISTRY_FULL`：后者是**插件**数达上限（`max_plugins`），
   * 调用方该卸载插件；本码是**流句柄**达上限（`MAX_STREAMS`），调用方该
   * 先 `host_stream_close` 再开。
   *
   * ⚠️ 追加码必须加在数组**末尾**：wire-gate 按声明顺序与 Rust 枚举比对。
   */
  'E_STREAM_FULL',
  /**
   * 一次跨主体调用已被结算，重复回填被拒（0.4-A1）。
   *
   * 不接受覆盖：允许重复回填等于让"第一次的结果"可被第二次悄悄改写——
   * 调用方拿到哪个结果取决于时序而不是事实。宁可显式失败。
   *
   * **不是** `E_CALL_NOT_FOUND`：条目**还在**（还没被发起方取走），只是不再
   * 接受新结果；下一步动作是去 `takeCallResult` 取已结算的结果。
   */
  'E_CALL_ALREADY_SETTLED',
] as const;

export type HostErrorCode = (typeof HOST_ERROR_CODES)[number];

/**
 * 可重试错误（与 Rust `ErrorCode::retryable()` 完全一致）。
 *
 * 仅超时、宿主 panic、插件被过滤这三类值得由框架层自动重试；
 * 其余都是确定性失败，重试只会放大成本。
 */
export const RETRYABLE_HOST_ERROR_CODES: readonly HostErrorCode[] = [
  'E_CALL_TIMEOUT',
  'E_HOST_PANIC',
  'E_PLUGIN_FILTERED',
];

export function isRetryable(code: HostErrorCode | string): boolean {
  return (RETRYABLE_HOST_ERROR_CODES as readonly string[]).includes(code);
}

export function isHostErrorCode(v: string): v is HostErrorCode {
  return (HOST_ERROR_CODES as readonly string[]).includes(v);
}

/** 应用层错误码形态（`@tauron/types` 的 `PluginErrorCode`，形如 `SC-1001`）。 */
export const APP_LAYER_ERROR_CODE_PATTERN = /^SC-\d{4}$/;

/** 是否应用层错误码（`SC-####`）。两套词表**并存**，本函数只做形态识别。 */
export function isAppLayerErrorCode(v: string): boolean {
  return APP_LAYER_ERROR_CODE_PATTERN.test(v);
}

/**
 * 是否是「像错误码」的串（宿主 `E_*` 或应用层 `SC-####`）。
 *
 * 存在的理由：边界归一若只认自己那套词表，另一套的码会被**整条丢弃**，
 * 跨层回溯就失去了唯一线索。识别形态 ≠ 接受语义：非宿主码仍是
 * `E_UNKNOWN`（判定用），只是原始码被保留（诊断用）。
 */
export function isCodeLike(v: string): boolean {
  return v.startsWith('E_') || isAppLayerErrorCode(v);
}

/**
 * 尝试把字符串解析为宿主错误对象（`to_tauri_err` 的 JSON 序列化形态）。
 *
 * Tauri 的 `InvokeError` 底层是 `serde_json::Value`，结构化错误可能以**对象**
 * 抵达，也可能以 **JSON 字符串**抵达（取决于运行时把拒绝值包成什么）。
 * 统一在这里解析，使两条路径都还原出完整的 `{ code, message, retryable }`。
 * 解析失败或形状不符时返回 `null`，调用方回退到正则抽取。
 */
function tryParseHostError(value: string | null | undefined): HostErrorShape | null {
  if (!value) return null;
  const t = value.trim();
  if (!t.startsWith('{')) return null;
  try {
    const rec = JSON.parse(t) as Record<string, unknown>;
    // R2-c：接受**任何**码形态（宿主 `E_*` 或应用层 `SC-####`）——只认 `E_`
    // 会把跨层的应用层码整条丢掉，那正是「边界隐式」的典型症状。
    if (typeof rec?.code === 'string' && isCodeLike(rec.code)) {
      const rawCode = rec.code;
      const code = isHostErrorCode(rawCode) ? rawCode : 'E_UNKNOWN';
      return {
        code,
        rawCode,
        message: typeof rec.message === 'string' ? rec.message : t,
        retryable: typeof rec.retryable === 'boolean' ? rec.retryable : isRetryable(code),
      };
    }
  } catch {
    // 不是 JSON → 交给正则兜底
  }
  return null;
}

/**
 * 从错误消息里抽取线上错误码（正则兜底）。
 * 用于宿主尚未结构化、或 Tauri 把错误压成纯字符串的场景。
 *
 * 返回原始码串：未识别的码也要保留（见 {@link HostErrorShape.rawCode}）。
 * R2-c：同时认应用层形态 `SC-####`——跨层错误常常只有消息里带码。
 */
function extractCode(msg: string): {
  code: HostErrorCode | 'E_UNKNOWN';
  rawCode: string | null;
} {
  const raw = msg.match(/\b(E_[A-Z0-9_]+|SC-\d{4})\b/)?.[1] ?? null;
  return { code: raw && isHostErrorCode(raw) ? raw : 'E_UNKNOWN', rawCode: raw };
}

/** 规范化后的宿主错误（跨 IPC 的最小契约）。 */
export interface HostErrorShape {
  /**
   * 线上错误码，见 {@link HOST_ERROR_CODES}；不在表内时为 `E_UNKNOWN`。
   *
   * 分支判断只用本字段（窄类型）；想知道宿主机到底发了什么看
   * {@link HostErrorShape.rawCode}。
   */
  code: HostErrorCode | 'E_UNKNOWN';
  /**
   * 宿主实际发来的原始码串：未识别的码原样保留，消息里根本没有码时为 `null`。
   *
   * 版本偏斜（第三方客户端内嵌旧 TS + 新宿主）下新码会被收窄成 `E_UNKNOWN`；
   * 丢掉原串会让日志与遥测彻底失去定位依据（`E_UNKNOWN` 只表示"本端不认识"）。
   */
  rawCode: string | null;
  /** 面向开发者/日志的可读描述；**不得**作为 UI 文案或分支判断依据。 */
  message: string;
  /** 宿主是否建议重试（= {@link RETRYABLE_HOST_ERROR_CODES}）。 */
  retryable: boolean;
}

/**
 * 把任意未知错误规范化成宿主错误形状。
 *
 * Tauri 的 `invoke` 在跨 IPC 反序列化失败、命令未注册等情况下会抛出
 * 非结构化值（字符串、Error、`{ __TAURI_INTERNALS__ }` 包裹对象）。
 * 这里统一收口，调用方只需要处理 {@link HostErrorShape}。
 */
export function normalizeError(err: unknown): HostErrorShape {
  if (err && typeof err === 'object') {
    const rec = err as Record<string, unknown>;

    // 1) 标准宿主错误：Rust 侧 `#[serde]` 序列化出来的 `{ code, message, retryable }`。
    //    R2-c：也接住应用层码（`SC-####`）——它不属于宿主词表，但**必须留下原始码**，
    //    否则跨层翻译时无法回溯是哪套词表的哪一条。
    if (typeof rec.code === 'string' && isCodeLike(rec.code)) {
      const rawCode = rec.code;
      const code = isHostErrorCode(rawCode) ? rawCode : 'E_UNKNOWN';
      const message = typeof rec.message === 'string' ? rec.message : String(err);
      const retryable = typeof rec.retryable === 'boolean' ? rec.retryable : isRetryable(code);
      return { code, rawCode, message, retryable };
    }

    // 2) Tauri/IPC 层错误：内层消息可能是 `to_tauri_err` 的 JSON 字符串，也可能是纯文本。
    const msg = typeof rec.message === 'string' ? rec.message : String(err);
    const parsed = tryParseHostError(msg);
    if (parsed) return parsed;
    const { code, rawCode } = extractCode(msg);
    return { code, rawCode, message: msg, retryable: isRetryable(code) };
  }

  if (err instanceof Error) {
    const parsed = tryParseHostError(err.message);
    if (parsed) return parsed;
    const { code, rawCode } = extractCode(err.message);
    return { code, rawCode, message: err.message, retryable: isRetryable(code) };
  }

  const msg = typeof err === 'string' ? err : String(err);
  return (
    tryParseHostError(msg) ?? {
      code: 'E_UNKNOWN',
      rawCode: extractCode(msg).rawCode,
      message: msg,
      retryable: false,
    }
  );
}

/** 携带类型信息的宿主错误，用于 `throw`。 */
export class HostException extends Error {
  readonly code: HostErrorCode | 'E_UNKNOWN';
  /** 宿主实际发来的原始码串（见 {@link HostErrorShape.rawCode}）。 */
  readonly rawCode: string | null;
  readonly retryable: boolean;

  constructor(shape: HostErrorShape) {
    super(shape.message);
    this.name = 'HostException';
    this.code = shape.code;
    this.rawCode = shape.rawCode;
    this.retryable = shape.retryable;
  }

  /** 序列化成可跨 IPC 传输的形状。 */
  toShape(): HostErrorShape {
    return {
      code: this.code,
      rawCode: this.rawCode,
      message: this.message,
      retryable: this.retryable,
    };
  }
}

// ──────────────────────────────────────────────────────────────────────────
// R2-c：错误词表边界显式化（B3）
//
// 仓库里有**两套**错误词表，它们各自服务不同的层，**不合并**：
// - 宿主词表 `HOST_ERROR_CODES`（`E_*`，Rust `ErrorCode`）：宿主命令的失败。
// - 应用层词表 `PluginErrorCode`（`SC-####`，见 `@tauron/types`）：插件实现的失败。
//
// 问题不是"有两套"，而是"穿越边界时哪套胜出"从未定义：错误从插件
// webview 穿到宿主、或从宿主穿回插件时，可能被**静默换码**，调用方按
// `code` 分支却拿到另一套词表的值。这里把边界变成**显式枚举 + 显式翻译**：
// 穿边界必留痕（`translated` / `foreignVocabulary`），原始码永不丢弃。
// ──────────────────────────────────────────────────────────────────────────

/** 错误穿越的边界点。 */
export type HostBoundary =
  /** 插件 webview → 宿主（invoke 拒绝）：**强制**归一为宿主词表的 {@link HostException}。 */
  | 'plugin-webview→host'
  /** 宿主 → 插件 webview：宿主码原样保留，不做二次翻译。 */
  | 'host→plugin-webview'
  /**
   * 插件内部：错误**不**越界，应用层词表（`SC-####`）必须原样保留。
   *
   * 这条存在的意义：插件作者按 `SC-*` 分支是合法的，边界翻译不得把它
   * 偷偷换成 `E_UNKNOWN` 之外的任何宿主码。
   */
  | 'plugin-internal';

/**
 * **已声明但尚未接线**的边界点（诚实清单，由门禁锁定）。
 *
 * `HostBoundary` 是"错误可能穿越的边界"全集；只有生产代码里真的调用过
 * {@link translate_at_boundary} 的边界才算接线。这两个值目前**没有生产调用点**，
 * 原因不是忘了，而是**依赖方向**：
 *
 * - `host→plugin-webview`：消费方是插件侧 SDK（把宿主拒绝/终帧翻成异常给插件作者），
 *   而插件侧 SDK 不应依赖 `@tauron/host`（那会把整个宿主客户端打进插件包）。
 * - `plugin-internal`：同上，且它的正确用法是取
 *   {@link BoundaryTranslation.rawCode} 而**不**采用宿主词表——插件内部的错误必须
 *   留在 `SC-####` 词表里。已知的**已定位缺口**（尚未修，见
 *   `packages/tauron-plugin-sdk/src/bridge.ts` 的 `invokeHandler` 失败分支）：
 *   插件 handler 抛出的 `SC-####` 会被硬写成通用 `SC-9001`，码被丢掉。
 *
 * 门禁要求：每个 `HostBoundary` 值必须"有生产使用点"或"在此清单里"——两处都没有
 * 才会红。接线之后要把值从这里删掉（清单变小是有意义的信号）。
 */
export const UNWIRED_BOUNDARIES: readonly HostBoundary[] = [
  'host→plugin-webview',
  'plugin-internal',
];

/** 边界翻译结果：**翻译事件本身**是可观察的，而不是悄悄发生。 */
export interface BoundaryTranslation {
  /** 归一后的异常（边界之后，上层只需处理这一种）。 */
  error: HostException;
  /** 在哪个边界发生的翻译。 */
  boundary: HostBoundary;
  /**
   * 码是否被边界**改写**过。
   *
   * `true` = 原始码不在宿主词表内（被收窄成 `E_UNKNOWN`，原始码见
   * {@link BoundaryTranslation.rawCode}）。日志/遥测据此区分「宿主真的返回了
   * E_UNKNOWN」与「本端不认识那个码」——两者混在一个 `code` 里会让定位失真。
   */
  translated: boolean;
  /** 原始码是否属于**另一套**词表（应用层 `SC-####`）。 */
  foreignVocabulary: boolean;
  /** 边界前的原始码（可能是宿主码、应用层码或未知串；消息里没有码则 `null`）。 */
  rawCode: string | null;
}

/**
 * 在边界处翻译错误（R2-c）。
 *
 * 不变量：
 * - 返回值**永远**是宿主词表的 {@link HostException}：边界之后没有"可能是
 *   Error、可能是字符串、可能是 `{message}`"的第四种形态。
 * - `rawCode` 永远保留原始码，`translated` 标记它是否被改写——**不丢信息**。
 * - 应用层码（`SC-####`）在 `plugin-internal` 边界**原样保留**（只标记
 *   `foreignVocabulary`），不并入宿主词表。
 */
export function translate_at_boundary(err: unknown, boundary: HostBoundary): BoundaryTranslation {
  const shape = normalizeError(err);
  const foreignVocabulary = shape.rawCode !== null && isAppLayerErrorCode(shape.rawCode);
  // `plugin-internal` 的原始码就是"当前层的码"，不构成"被改写"。
  const translated =
    boundary !== 'plugin-internal' && shape.rawCode !== null && !isHostErrorCode(shape.rawCode);
  return {
    error: new HostException(shape),
    boundary,
    translated,
    foreignVocabulary,
    rawCode: shape.rawCode,
  };
}
