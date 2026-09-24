// ──────────────────────────────────────────────────────────────────────────
// Bootstrap() — 应用启动编排器（P2-1）。
//
// 职责（8 个阶段，顺序不可换）：
// 1. 初始化 Backend（Tauri IPC）
// 2. 加载配置（ConfigManager 4 层合并）
// 3. 初始化动效系统（motion.ts）
// 4. 启动 Splash 画面（SplashStore）
// 5. 注册插件（PluginRegistry，含拓扑排序 + 条件加载）
// 6. 启动 ShellController（窗口管理事件路由）
// 7. 上报本轮启动结果（§4.14 的**驱动信号**，best-effort）
// 8. 提供 teardown() 清理资源
//
// 第 7 步此前不在本清单里，但它是**恢复引擎的唯一驱动信号**：漏了这一步，
// 每次启动都会被宿主计为一次崩溃，连续两次即进入安全模式。
//
// 用法：
// ```typescript
// const app = await bootstrap({
//   backend: createTauriBackend(),
//   config: { /* TauronConfig */ },
//   plugins: [/* PluginDescriptor[] */],
// });
// // ... 应用运行 ...
// await app.teardown();
// ```
// ──────────────────────────────────────────────────────────────────────────

import type { Backend } from './backend.js';
import { ShellController } from './shell-controller.js';
import { ShellClient } from './shell-client.js';
import { PluginRegistry, ConfigManager, EventBus } from '@tauron/core';
import type { TauronConfig } from '@tauron/types';

/** 插件描述符（最小化接口） */
export interface BootstrapPluginDescriptor {
  id: string;
  /** 优先级（数字越小越先加载） */
  priority?: number;
  /** 依赖的插件 ID 列表 */
  dependsOn?: string[];
  /** 条件加载（满足条件才加载） */
  conditions?: {
    platforms?: string[];
    gate?: string;
  };
  /** 是否懒加载（首次调用时才初始化） */
  lazyLoad?: boolean;
  /** 插件入口模块 */
  entry: unknown;
  [key: string]: unknown;
}

/** Bootstrap 配置 */
export interface BootstrapOptions {
  /** 后端实例 */
  backend: Backend;
  /** 应用配置 */
  config?: Partial<TauronConfig>;
  /** 要注册的插件 */
  plugins?: BootstrapPluginDescriptor[];
  /** 是否显示 Splash 画面 */
  showSplash?: boolean;
  /** Splash 最短显示时间（ms） */
  splashMinDuration?: number;
  /** 启动超时（ms），超时后强制继续 */
  bootTimeout?: number;
}

/** Splash 存储句柄（UI 包 SplashStore 的结构子集，避免类型耦合可选 peer）。 */
export interface SplashHandle {
  markReady(): void;
  cancel(): void;
  destroy(): void;
}

/** Bootstrap 结果 */
export interface BootstrapResult {
  /** ShellController 实例 */
  shell: ShellController;
  /** ShellClient 实例 */
  client: ShellClient;
  /** 插件注册表 */
  registry: PluginRegistry;
  /** 配置管理器 */
  configManager: ConfigManager;
  /** 事件总线 */
  eventBus: EventBus;
  /** Splash 句柄（未启用 / UI 包不可用时为 null） */
  splash: SplashHandle | null;
  /** 启动耗时（ms） */
  bootTimeMs: number;
  /** 清理资源 */
  teardown: () => Promise<void>;
}

/** 启动阶段 */
export type BootPhase = 'init' | 'config' | 'motion' | 'splash' | 'plugins' | 'shell' | 'ready' | 'failed';

/**
 * Bootstrap() — 应用启动编排器。
 *
 * 按阶段初始化：
 * 1. init — 创建核心实例
 * 2. config — 加载配置
 * 3. motion — 初始化动效系统
 * 4. splash — 启动 Splash 画面
 * 5. plugins — 注册插件（含拓扑排序）
 * 6. shell — 启动 ShellController
 * 7. ready — 完成
 */
/**
 * 可选 UI 能力的最小结构面（本地定义，不 import 其类型——避免类型耦合）。
 * 字段与 `@tauron/ui-primitives` / `@tauron/ui` 的实际签名保持结构兼容。
 */
interface UiBootstrapModule {
  defineMotionTheme?: (config: Record<string, unknown>) => void;
  SplashStore?: new (config: Record<string, unknown>) => SplashHandle & { start(): void };
}

/**
 * 可选 UI 能力模块的候选清单（按优先级）。
 *
 * R3 后动画/启动画面归属零宿主依赖的 `@tauron/ui-primitives`，优先取它；装的是
 * 便利包 `@tauron/ui` 的既有应用仍可命中第二个候选（后者转出前者）。
 */
const UI_MODULE_CANDIDATES = ['@tauron/ui-primitives', '@tauron/ui'] as const;

/**
 * 动态加载可选 UI 能力模块。
 *
 * 模块名用非字面量变量：TS 不做静态解析——本包编译期**不依赖任何 UI 包**，
 * 构建拓扑保持单向（ui → host），也不会把 `lit` 与 DOM 组件拖进宿主入口。
 * 所有候选都不可用时返回 `null`，调用方静默降级。
 */
async function loadUiModule(): Promise<UiBootstrapModule | null> {
  for (const moduleName of UI_MODULE_CANDIDATES) {
    try {
      return (await import(moduleName)) as unknown as UiBootstrapModule;
    } catch {
      // 未安装该候选包：继续尝试下一个；全部失败则返回 null。
    }
  }
  return null;
}

export async function bootstrap(options: BootstrapOptions): Promise<BootstrapResult> {
  const startTime = performance.now();

  // 1. 创建核心实例
  const configManager = new ConfigManager();
  const registry = new PluginRegistry({ maxPlugins: 32 });
  const eventBus = new EventBus();

  // 2. 加载配置
  if (options.config) {
    configManager.setDefaults(normalizeConfig(options.config));
  }

  // 3. 初始化动效系统（如果配置了 motion 且 UI 包可用）
  if (options.config?.motion) {
    const ui = await loadUiModule();
    ui?.defineMotionTheme?.({
      preset: options.config.motion.preset,
      durations: options.config.motion.durations,
      easings: options.config.motion.easings,
      respectReducedMotion: options.config.motion.respectReducedMotion,
    });
  }

  // 4. 启动 Splash 画面
  let splashStore: SplashHandle | null = null;
  let splashTimer: ReturnType<typeof setTimeout> | null = null;
  if (options.showSplash && options.config?.motion?.splash?.enabled) {
    const ui = await loadUiModule();
    const SplashStore = ui?.SplashStore;
    if (SplashStore) {
      const splash = new SplashStore({
        enabled: true,
        minDuration: options.splashMinDuration ?? options.config.motion.splash.minDuration,
        title: options.config.motion.splash.title,
        ...(options.config.motion.splash.subtitle !== undefined ? { subtitle: options.config.motion.splash.subtitle } : {}),
        background: options.config.motion.splash.background,
        progress: options.config.motion.splash.progress,
        exitAnimation: options.config.motion.splash.exitAnimation,
      });
      splash.start();
      splashStore = splash;
      splashTimer = setTimeout(() => {
        splashTimer = null;
        splash.markReady();
      }, 500);
    }
  }

  // 5. 注册插件（含拓扑排序 + 条件加载）
  if (options.plugins && options.plugins.length > 0) {
    const sorted = topologicalSort(options.plugins);
    const platform = getCurrentPlatform();

    for (const plugin of sorted) {
      // 条件检查
      if (plugin.conditions) {
        if (plugin.conditions.platforms && !plugin.conditions.platforms.includes(platform)) {
          continue;
        }
      }

      // 懒加载标记
      if (plugin.lazyLoad) {
        registry.register(plugin.id, 'js', { ...plugin, lazyLoad: true });
        continue;
      }

      // 立即加载
      registry.register(plugin.id, 'js', plugin);
    }
  }

  // 6. 启动 ShellController
  const shell = new ShellController({ backend: options.backend });
  shell.start();

  const bootTimeMs = Math.round(performance.now() - startTime);

  // 7. 上报本轮启动结果（§4.14 的**驱动信号**）。
  //
  // 宿主崩溃检测的「干净退出」判据就是这一条：本轮上报过 `success`，下一次
  // 启动才不会把本轮计为一次崩溃。漏报只多计一次失败、一次成功上报即自愈
  // （方向安全），所以这里是 best-effort：后端不可用或宿主版本过旧（无此命令）
  // 时吞掉，绝不让恢复上报反过来弄垮启动。刻意不 `await`——启动耗时的度量
  // 不该包含一次恢复 RPC，而上报落在渲染前还是后对判定无影响。
  const client = new ShellClient({ backend: options.backend });
  client.recoverReport('success').catch(() => {
    // 恢复上报失败不构成启动失败：宿主侧会保守地多计一次崩溃，可自愈。
  });

  // 8. 清理函数
  const teardown = async () => {
    if (splashTimer !== null) {
      clearTimeout(splashTimer);
      splashTimer = null;
    }
    splashStore?.destroy();
    splashStore = null;
    shell.stop();
    registry.clear();
    eventBus.clear();
    configManager.clear();
  };

  return {
    shell,
    client,
    registry,
    configManager,
    eventBus,
    splash: splashStore,
    bootTimeMs,
    teardown,
  };
}

// ──────────────────────────────────────────────────────────────────────────
// 内部工具函数
// ──────────────────────────────────────────────────────────────────────────

/** 拓扑排序（Kahn 算法） */
function topologicalSort<T extends BootstrapPluginDescriptor>(plugins: T[]): T[] {
  const map = new Map<string, T>();
  const inDegree = new Map<string, number>();
  const dependents = new Map<string, string[]>(); // pluginId → [dependsOn...]

  for (const p of plugins) {
    map.set(p.id, p);
    inDegree.set(p.id, 0);
  }

  for (const p of plugins) {
    for (const dep of p.dependsOn ?? []) {
      if (map.has(dep)) {
        inDegree.set(p.id, (inDegree.get(p.id) ?? 0) + 1);
        if (!dependents.has(dep)) dependents.set(dep, []);
        dependents.get(dep)!.push(p.id);
      }
    }
  }

  // 按优先级排序同入度的节点
  const queue: string[] = [];
  for (const [id, deg] of inDegree) {
    if (deg === 0) queue.push(id);
  }
  queue.sort((a, b) => (map.get(a)?.priority ?? 999) - (map.get(b)?.priority ?? 999));

  const result: T[] = [];
  while (queue.length > 0) {
    const id = queue.shift()!;
    result.push(map.get(id)!);

    for (const dep of dependents.get(id) ?? []) {
      const newDeg = (inDegree.get(dep) ?? 1) - 1;
      inDegree.set(dep, newDeg);
      if (newDeg === 0) {
        queue.push(dep);
        queue.sort((a, b) => (map.get(a)?.priority ?? 999) - (map.get(b)?.priority ?? 999));
      }
    }
  }

  // 如果有环，剩余节点按原顺序追加
  const placed = new Set(result.map((p) => p.id));
  for (const p of plugins) {
    if (!placed.has(p.id)) result.push(p);
  }

  return result;
}

/** 获取当前平台 */
function getCurrentPlatform(): string {
  if (typeof navigator !== 'undefined') {
    const ua = navigator.userAgent.toLowerCase();
    if (ua.includes('windows')) return 'windows';
    if (ua.includes('mac') || ua.includes('darwin')) return 'macos';
    if (ua.includes('linux')) return 'linux';
  }
  return 'unknown';
}

/** 规范化配置（补全默认值） */
function normalizeConfig(config: Partial<TauronConfig>): Record<string, unknown> {
  const defaults: Record<string, unknown> = {
    capabilities: ['base'],
    plugins: { local: './plugins', registry: '', autoUpdate: false },
    shell: { type: 'local', distDir: './dist' },
    updater: { endpoints: [], pubkey: '' },
    brands: {},
    i18n: { defaultLocale: 'en', supported: ['en'] },
    motion: {
      preset: 'standard',
      durations: { fast: 150, normal: 300, slow: 500 },
      easings: {
        standard: { name: 'standard', cubicBezier: [0.4, 0.0, 0.2, 1.0], value: 'cubic-bezier(0.4, 0.0, 0.2, 1.0)' },
        emphasized: { name: 'emphasized', cubicBezier: [0.2, 0.0, 0.0, 1.0], value: 'cubic-bezier(0.2, 0.0, 0.0, 1.0)' },
        decelerated: { name: 'decelerated', cubicBezier: [0.0, 0.0, 0.2, 1.0], value: 'cubic-bezier(0.0, 0.0, 0.2, 1.0)' },
        accelerated: { name: 'accelerated', cubicBezier: [0.4, 0.0, 1.0, 1.0], value: 'cubic-bezier(0.4, 0.0, 1.0, 1.0)' },
      },
      splash: { enabled: false, minDuration: 1500, title: 'Tauron', background: '#fff', progress: 'bar', exitAnimation: 'fade' },
      exit: { animation: 'fade', duration: 300, savingPrompt: 'Saving...', prompt: 'Confirm exit?' },
      transitions: { toast: true, dialog: true, commandPalette: true, pluginList: true, themeSwitch: true },
      respectReducedMotion: true,
    },
  };

  // 合并用户配置
  return deepMerge(defaults, config);
}

/** 深合并两个对象（跳过危险键，防原型污染） */
const UNSAFE_MERGE_KEYS = new Set(['__proto__', 'constructor', 'prototype']);

function deepMerge(base: Record<string, unknown>, override: Record<string, unknown>): Record<string, unknown> {
  const result = { ...base };
  for (const [key, val] of Object.entries(override)) {
    if (UNSAFE_MERGE_KEYS.has(key)) continue;
    if (val && typeof val === 'object' && !Array.isArray(val)) {
      const baseVal = result[key];
      if (baseVal && typeof baseVal === 'object' && !Array.isArray(baseVal)) {
        result[key] = deepMerge(baseVal as Record<string, unknown>, val as Record<string, unknown>);
      } else {
        result[key] = val;
      }
    } else {
      result[key] = val;
    }
  }
  return result;
}