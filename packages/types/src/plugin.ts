/**
 * tauron 插件状态与事件（设计文档 §4.3/§2.3）
 */

/**
 * 插件生命周期状态（设计文档 §4.3 的**设计模型**词表）
 *
 * 这不是宿主线名。线上状态由 Rust `tauron-host::lifecycle::State::as_str()` 单一写入
 * （TS 侧镜像 = `@tauron/host` 的 `LIFECYCLE_STATES`），两份词表只有 5 个名字重合：
 * 本表的 `ENABLING`/`DISABLING`/`ERRORED`/`UPGRADING`/`UNINSTALLING` 不在线上，
 * 线上的 `RUNNING`/`ERRORED_RETRYABLE`/`ERRORED_USER_CONFIRM`/`INSTALL_FAILED`/`UNINSTALLED`
 * 本表无法表达。逐名分歧由 `@tauron/contract-tests` 的线格式门禁钉死（轮 60），
 * 改名或收敛属破坏性变更（本包已发布 1.0.x），需单独批准。
 */
export type PluginState =
  | 'DISCOVERED'
  | 'INSTALLING'
  | 'INSTALLED'
  | 'ENABLING'
  | 'ENABLED'
  | 'DISABLING'
  | 'DISABLED'
  | 'ERRORED'
  | 'UNINSTALLING'
  | 'UPGRADING';

/**
 * 状态迁移表（表驱动）。
 *
 * 「单点收口」的说法在轮 60 被证伪并删掉：真正的收口点是 Rust
 * `tauron-host::lifecycle::TRANSITIONS`（57 条规则，表内顺序即匹配优先级，
 * 迁移由 `transition()` 校验）；本表在仓内只被 `@tauron/core` 的 `PluginRegistry`
 * 读取，而那条链路是孤儿台账在册项（无生产装配）。按本表判定为合法的迁移，
 * 宿主可能判非法——线上口径以 Rust 表为准。
 */
export const TRANSITIONS: Readonly<Record<PluginState, readonly PluginState[]>> = {
  DISCOVERED: ['INSTALLING'],
  INSTALLING: ['INSTALLED', 'ERRORED'],
  INSTALLED: ['ENABLING', 'UNINSTALLING'],
  ENABLING: ['ENABLED', 'ERRORED'],
  ENABLED: ['DISABLING', 'ERRORED', 'UPGRADING'],
  DISABLING: ['DISABLED'],
  DISABLED: ['ENABLING', 'UNINSTALLING'],
  ERRORED: ['ENABLING', 'UNINSTALLING', 'UPGRADING'],
  UNINSTALLING: [],
  UPGRADING: ['INSTALLED', 'ERRORED'],
};

/**
 * 终态集合（§4.3 设计模型口径）。
 *
 * 轮 61 把它说实：**这不是线上的终态**。`PluginState` 里根本没有 `UNINSTALLED`
 * （那是宿主线名，见轮 60 的三套词表门禁），所以本集合在类型上就表达不出
 * 「卸载完成且记录保留」这个真终态；线上还把 `ERRORED_USER_CONFIRM` 计为需用户动作后
 * 停止的态。仓内零读者（B 口径在册），别拿它当 UI 的启用/禁用依据。
 */
export const TERMINAL_STATES: ReadonlySet<PluginState> = new Set(['UNINSTALLING']);

/** 活跃态集合（可以执行调用的状态） */
export const ACTIVE_STATES: ReadonlySet<PluginState> = new Set(['ENABLED']);

/**
 * 验证状态迁移是否合法
 */
export function isValidTransition(from: PluginState, to: PluginState): boolean {
  const allowed = TRANSITIONS[from];
  return allowed ? allowed.includes(to) : false;
}

/**
 * 获取允许的目标状态
 */
export function allowedTransitions(from: PluginState): readonly PluginState[] {
  return TRANSITIONS[from] ?? [];
}

/** 插件事件（设计文档 §2.3） */
export interface PluginEvent<T = unknown> {
  /** 发出者插件 ID */
  sourcePlugin: string;
  /** 事件名（不含前缀） */
  eventName: string;
  /** 事件负载 */
  payload: T;
  /** 时间戳（ISO 8601） */
  timestamp: string;
}

/**
 * 生成事件命名空间（plugin:<id>:<event>）
 */
export function eventNamespace(pluginId: string, eventName: string): string {
  return `plugin:${pluginId}:${eventName}`;
}

/** 插件类型（4+1 形态，设计文档 §4） */
export type PluginType = 'rust' | 'js' | 'process' | 'wasm';

/** 插件形态标签（A/B/B+/C/D 类） */
export type PluginForm = 'A' | 'B' | 'B+' | 'C' | 'D';

/**
 * 判断插件类型是否支持指定形态
 */
export function pluginForm(type: PluginType): PluginForm {
  switch (type) {
    case 'rust':
      return 'A';
    case 'js':
      return 'B';
    case 'process':
      return 'C';
    case 'wasm':
      return 'D';
  }
}
