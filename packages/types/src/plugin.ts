/**
 * tauron 插件状态与事件（设计文档 §4.3/§2.3）
 */

/** 插件生命周期状态 */
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

/** 状态迁移表（DSH 模式：表驱动 + 单点收口） */
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

/** 终态集合 */
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
