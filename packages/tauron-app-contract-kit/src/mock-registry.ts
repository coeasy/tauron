// ──────────────────────────────────────────────────────────────────────────
// MockRegistry：插件注册表模拟（§4.28 插件测试工具）。
//
// 用途：
// - 模拟宿主注册表行为（install/uninstall/list/find）
// - 支持插件生命周期状态转换
// - 记录所有操作用于断言
//
// 与真实 Registry 的区别：
// - 无并发控制（单线程测试环境）
// - 无容量限制（测试专用）
// - 无过滤（简化测试）
// - **状态名是本 mock 的小写私有词表，不是宿主线名**：宿主状态由
//   `tauron-host::lifecycle` 单一写入并以 SCREAMING_SNAKE_CASE 上线
//   （`@tauron/host` 的 `LIFECYCLE_STATES`）。要按线上口径断言请用
//   `MockRegistry.getWireState()`（映射见 `MOCK_STATE_TO_WIRE`），
//   别把手写的小写名喂给真实宿主。
// ──────────────────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────────────────
// 类型定义
// ──────────────────────────────────────────────────────────────────────────

/**
 * MockRegistry 存储的插件 manifest。
 *
 * `@tauron/host` **不**导出 manifest 类型：宿主侧对外只暴露
 * {@link PluginDescriptor}（`host_registry_list` 返回项的运行时描述），
 * manifest 的权威定义在 `@tauron/types` 的 `PluginManifest`。
 *
 * 本 mock 只模拟注册表的生命周期行为，不校验 manifest 协议，因此在此声明它
 * 实际读取/存储的字段子集。`type` 取值与 `@tauron/types` 的 `PluginType` 对齐。
 */
export interface MockPluginManifest {
  id: string;
  name: string;
  version: string;
  description: string;
  author: string;
  homepage: string;
  type: 'rust' | 'js' | 'process' | 'wasm';
  main: string;
  permissions: string[];
  tags: string[];
}

/** 注册表操作结果。 */
export interface RegistryOpResult {
  success: boolean;
  error?: string;
}

/** 插件状态。 */
export type PluginState = 'installed' | 'enabled' | 'disabled' | 'errored' | 'uninstalled';

/**
 * 宿主状态线名中本 mock 能表达的那五个（`@tauron/host` 的 `LIFECYCLE_STATES` 子集）。
 *
 * 写成字面量而不是从 `@tauron/host` 引入类型：本包**零运行时依赖**（见 package.json），
 * 而线名的权威在 Rust `tauron-host::lifecycle`。两份词表的逐名一致性由
 * `@tauron/contract-tests` 的线格式门禁核对，不靠这里的 import。
 */
export type MockWireState =
  'INSTALLED' | 'ENABLED' | 'DISABLED' | 'ERRORED_USER_CONFIRM' | 'UNINSTALLED';

/**
 * mock 小写状态名 → 宿主状态线名（整份词表逐名映射，编译期强制全覆盖：
 * 给 {@link PluginState} 加一个名字而不在这里补行，本包直接编译失败）。
 *
 * `errored` 对应 `ERRORED_USER_CONFIRM` 而不是 `ERRORED_RETRYABLE`：mock 没有重试预算，
 * `setErrored` 当场禁用插件并要求调用方显式 `clearError`，与宿主「升级交人工」那一档同形。
 *
 * `uninstalled` 这一行**当前没有产出点**：`uninstall()` 删除条目，所以 `getState()` 之后
 * 返回 `null` 而不是 `'uninstalled'`。保留该行是为了让整份词表都有对得上的线上名字，
 * 且门禁能逐名核对；把 `'uninstalled'` 真产出来要改 `getState()` 对未知 ID 的已发布契约
 * （现在是 `null`），属破坏性改动，须单独批准。
 */
export const MOCK_STATE_TO_WIRE: Readonly<Record<PluginState, MockWireState>> = {
  installed: 'INSTALLED',
  enabled: 'ENABLED',
  disabled: 'DISABLED',
  errored: 'ERRORED_USER_CONFIRM',
  uninstalled: 'UNINSTALLED',
};

/** 插件条目。 */
export interface RegistryEntry {
  manifest: MockPluginManifest;
  state: PluginState;
  installedAt: string;
  lastUsedAt: string;
  enabled: boolean;
  /** 最近一次 {@link MockRegistry.setErrored} 记录的错误信息；`clearError` 后移除。 */
  lastError?: string;
}

/** 注册表操作记录。 */
export interface OperationRecord {
  type: 'install' | 'uninstall' | 'enable' | 'disable' | 'find' | 'list';
  pluginId: string;
  timestamp: string;
  success: boolean;
  error?: string;
}

/** 注册表快照。 */
export interface RegistrySnapshot {
  entries: Record<string, RegistryEntry>;
  operations: OperationRecord[];
  totalInstalled: number;
  totalEnabled: number;
}

// ──────────────────────────────────────────────────────────────────────────
// MockRegistry 实现
// ──────────────────────────────────────────────────────────────────────────

/**
 * 插件注册表模拟。
 *
 * 提供与真实 Registry 类似的操作接口，但使用内存存储和简化逻辑。
 */
export class MockRegistry {
  private entries: Map<string, RegistryEntry>;
  private operations: OperationRecord[];

  constructor() {
    this.entries = new Map();
    this.operations = [];
  }

  /**
   * 记录一次失败操作并返回统一结果。
   *
   * `error` 形参是必填 `string`，因此返回的对象满足
   * `exactOptionalPropertyTypes` 下 `error?: string` 的约束。
   */
  private fail(type: OperationRecord['type'], pluginId: string, error: string): RegistryOpResult {
    this.operations.push({
      type,
      pluginId,
      timestamp: new Date().toISOString(),
      success: false,
      error,
    });
    return { success: false, error };
  }

  /** 记录一次成功操作。 */
  private succeed(type: OperationRecord['type'], pluginId: string): RegistryOpResult {
    this.operations.push({
      type,
      pluginId,
      timestamp: new Date().toISOString(),
      success: true,
    });
    return { success: true };
  }

  /**
   * 安装插件。
   */
  install(manifest: MockPluginManifest): RegistryOpResult {
    if (!manifest.id) {
      return this.fail('install', manifest.id || 'unknown', '插件 ID 不能为空');
    }

    if (this.entries.has(manifest.id)) {
      return this.fail('install', manifest.id, '插件已存在');
    }

    const entry: RegistryEntry = {
      manifest,
      state: 'installed',
      installedAt: new Date().toISOString(),
      lastUsedAt: new Date().toISOString(),
      enabled: false,
    };

    this.entries.set(manifest.id, entry);

    return this.succeed('install', manifest.id);
  }

  /**
   * 卸载插件。
   */
  uninstall(pluginId: string): RegistryOpResult {
    if (!this.entries.has(pluginId)) {
      return this.fail('uninstall', pluginId, '插件不存在');
    }

    this.entries.delete(pluginId);

    return this.succeed('uninstall', pluginId);
  }

  /**
   * 启用插件。
   */
  enable(pluginId: string): RegistryOpResult {
    const entry = this.entries.get(pluginId);

    if (!entry) {
      return this.fail('enable', pluginId, '插件不存在');
    }

    entry.enabled = true;
    entry.state = 'enabled';
    entry.lastUsedAt = new Date().toISOString();

    return this.succeed('enable', pluginId);
  }

  /**
   * 禁用插件。
   */
  disable(pluginId: string): RegistryOpResult {
    const entry = this.entries.get(pluginId);

    if (!entry) {
      return this.fail('disable', pluginId, '插件不存在');
    }

    entry.enabled = false;
    entry.state = 'disabled';

    return this.succeed('disable', pluginId);
  }

  /**
   * 查找插件。
   */
  find(pluginId: string): RegistryEntry | null {
    const entry = this.entries.get(pluginId) ?? null;

    const record: OperationRecord = {
      type: 'find',
      pluginId,
      timestamp: new Date().toISOString(),
      success: entry !== null,
    };
    this.operations.push(record);

    return entry;
  }

  /**
   * 列出所有插件。
   */
  list(): RegistryEntry[] {
    const entries = Array.from(this.entries.values());

    const record: OperationRecord = {
      type: 'list',
      pluginId: '*',
      timestamp: new Date().toISOString(),
      success: true,
    };
    this.operations.push(record);

    return entries;
  }

  /**
   * 获取已启用插件列表。
   */
  listEnabled(): RegistryEntry[] {
    return Array.from(this.entries.values()).filter((e) => e.enabled);
  }

  /**
   * 设置插件错误状态。
   */
  setErrored(pluginId: string, errorMessage: string): void {
    const entry = this.entries.get(pluginId);
    if (entry) {
      entry.state = 'errored';
      entry.enabled = false;
      entry.lastError = errorMessage;
    }
  }

  /**
   * 清除插件错误状态。
   */
  clearError(pluginId: string): void {
    const entry = this.entries.get(pluginId);
    if (!entry) {
      return;
    }
    if (entry.state === 'errored') {
      entry.state = 'disabled';
    }
    // exactOptionalPropertyTypes：可选属性用 delete 移除，而非赋 undefined。
    delete entry.lastError;
  }

  /**
   * 获取插件最近一次记录的错误信息。
   */
  getLastError(pluginId: string): string | null {
    return this.entries.get(pluginId)?.lastError ?? null;
  }

  /**
   * 获取注册表快照。
   */
  snapshot(): RegistrySnapshot {
    return {
      entries: Object.fromEntries(this.entries),
      operations: [...this.operations],
      totalInstalled: this.entries.size,
      totalEnabled: Array.from(this.entries.values()).filter((e) => e.enabled).length,
    };
  }

  /**
   * 获取操作记录。
   */
  getOperations(): OperationRecord[] {
    return [...this.operations];
  }

  /**
   * 获取操作记录（按类型过滤）。
   */
  getOperationsByType(type: OperationRecord['type']): OperationRecord[] {
    return this.operations.filter((op) => op.type === type);
  }

  /**
   * 获取成功操作数量。
   */
  getSuccessCount(): number {
    return this.operations.filter((op) => op.success).length;
  }

  /**
   * 获取失败操作数量。
   */
  getFailureCount(): number {
    return this.operations.filter((op) => !op.success).length;
  }

  /**
   * 清空注册表。
   */
  clear(): void {
    this.entries.clear();
    this.operations = [];
  }

  /**
   * 获取插件数量。
   */
  size(): number {
    return this.entries.size;
  }

  /**
   * 检查插件是否存在。
   */
  has(pluginId: string): boolean {
    return this.entries.has(pluginId);
  }

  /**
   * 获取插件状态。
   */
  getState(pluginId: string): PluginState | null {
    const entry = this.entries.get(pluginId);
    return entry ? entry.state : null;
  }

  /**
   * 以**宿主线名**获取插件状态（`@tauron/host` 的 `LIFECYCLE_STATES` 子集）。
   *
   * `getState()` 返回的是本 mock 的小写私有词表；真实宿主的状态由
   * `tauron-host::lifecycle` 单一写入并以 SCREAMING_SNAKE_CASE 上线。被测量代码若按
   * 线名分支（例如把 `host_registry_list` 的 `state` 透传进 UI），用本方法在 mock 上
   * 走同一条分支，不要手写字符串——手写的小写名喂给真实宿主必然反序列化失败。
   *
   * 未安装（含已被 `uninstall()` 移除）的插件返回 `null`。
   */
  getWireState(pluginId: string): MockWireState | null {
    const state = this.getState(pluginId);
    return state === null ? null : MOCK_STATE_TO_WIRE[state];
  }

  /**
   * 检查插件是否已启用。
   */
  isEnabled(pluginId: string): boolean {
    const entry = this.entries.get(pluginId);
    return entry?.enabled ?? false;
  }
}

// ──────────────────────────────────────────────────────────────────────────
// 工厂函数
// ──────────────────────────────────────────────────────────────────────────

/**
 * 创建 MockRegistry 实例。
 */
export function createMockRegistry(): MockRegistry {
  return new MockRegistry();
}

/**
 * 创建预填充的 MockRegistry（包含示例插件）。
 */
export function createPopulatedRegistry(): MockRegistry {
  const registry = new MockRegistry();

  const plugin1: MockPluginManifest = {
    id: 'test.plugin.a',
    name: 'Plugin A',
    version: '1.0.0',
    description: 'Test Plugin A',
    author: 'Test Author',
    homepage: 'https://example.com',
    type: 'js',
    main: 'dist/index.js',
    permissions: ['storage.read', 'storage.write'],
    tags: ['test', 'example'],
  };

  const plugin2: MockPluginManifest = {
    id: 'test.plugin.b',
    name: 'Plugin B',
    version: '2.0.0',
    description: 'Test Plugin B',
    author: 'Test Author',
    homepage: 'https://example.com',
    type: 'wasm',
    main: 'dist/index.wasm',
    permissions: ['network.request'],
    tags: ['test', 'wasm'],
  };

  const plugin3: MockPluginManifest = {
    id: 'test.plugin.c',
    name: 'Plugin C',
    version: '3.0.0',
    description: 'Test Plugin C',
    author: 'Test Author',
    homepage: 'https://example.com',
    type: 'process',
    main: 'dist/sidecar',
    permissions: ['filesystem.read'],
    tags: ['test', 'process'],
  };

  registry.install(plugin1);
  registry.enable(plugin1.id);

  registry.install(plugin2);
  registry.install(plugin3);

  return registry;
}
