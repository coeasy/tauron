/**
 * tauron 运行时探测（设计文档 §9）
 *
 * isTauri() 探测：通过 __TAURI_INTERNALS__ 判断是否在 Tauri 环境中。
 * 非 Tauri 环境降级为 Web 模式，禁用部分能力。
 */

/** 检测是否在 Tauri 环境中运行 */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/** 运行时模式 */
export type RuntimeMode = 'tauri' | 'web';

/** 运行时能力集 */
export interface RuntimeCapabilities {
  store: boolean;
  http: boolean;
  tray: boolean;
  globalShortcut: boolean;
  autostart: boolean;
  updater: boolean;
  shell: boolean;
  singleInstance: boolean;
  processPlugin: boolean;
  wasmPlugin: boolean;
}

/** Tauri 运行时能力（诚实标记：仅已实现的能力为 true） */
export const TAURI_CAPABILITIES: RuntimeCapabilities = {
  store: true,
  http: true,
  tray: false,            // P0-2 诚实化：tauri-plugin-tray 未集成
  globalShortcut: false,  // P0-2 诚实化：tauri-plugin-global-shortcut 未集成
  autostart: false,       // P0-2 诚实化：tauri-plugin-autostart 未集成
  updater: false,         // P0-2 诚实化：tauri-plugin-updater 未集成
  shell: false,           // P0-2 诚实化：tauri-plugin-shell 未集成
  singleInstance: false,  // P0-2 诚实化：tauri-plugin-single-instance 未集成
  processPlugin: false,   // P0-2 诚实化：tauron-proc 是 stub 模拟
  wasmPlugin: false,      // P0-2 诚实化：tauron-wasm 是 stub 模拟
};

/** Web 运行时能力（降级） */
export const WEB_CAPABILITIES: RuntimeCapabilities = {
  store: true,
  http: true,
  tray: false,
  globalShortcut: false,
  autostart: false,
  updater: false,
  shell: false,
  singleInstance: false,
  processPlugin: false,
  wasmPlugin: false,
};

/**
 * 获取当前运行时模式
 */
export function getRuntimeMode(): RuntimeMode {
  return isTauri() ? 'tauri' : 'web';
}

/**
 * 获取当前可用的能力集
 */
export function getCapabilities(): RuntimeCapabilities {
  return isTauri() ? { ...TAURI_CAPABILITIES } : { ...WEB_CAPABILITIES };
}

/**
 * 检查指定能力是否可用
 */
export function isCapabilityAvailable(capability: keyof RuntimeCapabilities): boolean {
  const caps = getCapabilities();
  return caps[capability] ?? false;
}
