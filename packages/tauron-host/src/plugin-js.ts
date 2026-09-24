// ──────────────────────────────────────────────────────────────────────────
// JS 插件运行时配置与生命周期管理（开发计划 §4.6）。
//
// 职责：身份单元（隐藏 Webview，label `plugin-<id>`）生命周期；视图 iframe 挂载与 postMessage；contributes 注册。
//
// ⚠️ 接线状态：这是给集成方保留的**运行时参考实现**（自带单元测试）——
// 仓内当前没有生产调用点：框架层沙箱走 `@tauron/dual-world` / `@tauron/plugin-sdk`，
// 应用层插件走 `@tauron/app-plugin-sdk` 的 webview 形态。采用前请先补
// 「谁创建 PluginJsRuntime、iframe 宿主页面在哪」的装配层。
//
// 关键约束：
// - label 由宿主生成且不可被插件指定
// - 视图 iframe 固定 `sandbox="allow-scripts"`（不给 `allow-same-origin`）
// - 面板关闭即销毁视图 iframe，身份单元按架构 §4.7 空闲回收
// - 活跃身份上限 8（可配，K3 校准），超出 LRU 驱逐并保留状态快照
// ──────────────────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────────────────
// 类型
// ──────────────────────────────────────────────────────────────────────────

/** 身份单元状态。 */
export type IdentityState =
  | 'idle'      // 已创建，未挂载视图
  | 'active'    // 已挂载视图，正在运行
  | 'evicting'  // 正在驱逐（保留状态快照）
  | 'evicted';  // 已驱逐，状态快照已保存

/** 视图 iframe 配置。 */
export interface ViewConfig {
  /** iframe sandbox 属性（固定为 'allow-scripts'）。 */
  sandbox: 'allow-scripts';
  /** 是否允许 same-origin（固定为 false）。 */
  allowSameOrigin: false;
  /** 视图 URL。 */
  url: string;
  /** 视图宽度。 */
  width: number;
  /** 视图高度。 */
  height: number;
}

/** 身份单元描述。 */
export interface IdentityUnit {
  /** 插件 ID。 */
  pluginId: string;
  /** Webview label（`plugin-<id>` 格式）。 */
  label: string;
  /** 身份单元状态。 */
  state: IdentityState;
  /** 创建时间戳。 */
  createdAt: number;
  /** 最后访问时间戳。 */
  lastAccessedAt: number;
  /** 视图配置（可选）。 */
  viewConfig?: ViewConfig;
  /** 状态快照（驱逐时保留）。 */
  stateSnapshot?: unknown;
}

/** Contributes 注册项。 */
export interface ContributesEntry {
  /** 插件 ID。 */
  pluginId: string;
  /** 贡献类型（如 'view', 'command', 'setting' 等）。 */
  type: string;
  /** 贡献 ID。 */
  id: string;
  /** 贡献元数据。 */
  metadata: Record<string, unknown>;
  /** 注册时间戳。 */
  registeredAt: number;
}

/** 身份单元配置。 */
export interface PluginJsConfig {
  /** 最大活跃身份单元数（默认 8）。 */
  maxActiveIdentities: number;
  /** 身份单元空闲超时（毫秒，默认 300000 = 5 分钟）。 */
  idleTimeout: number;
  /** 是否启用状态快照（默认 true）。 */
  enableStateSnapshot: boolean;
  /** 状态快照最大数量（默认 16）。 */
  maxStateSnapshots: number;
}

/** 身份单元操作结果。 */
export type IdentityActionResult =
  | { ok: true }
  | { ok: false; code: string; message: string };

// ──────────────────────────────────────────────────────────────────────────
// PluginJsRuntime
// ──────────────────────────────────────────────────────────────────────────

/**
 * JS 插件运行时。
 *
 * 管理身份单元生命周期、视图挂载、contributes 注册。
 */
export class PluginJsRuntime {
  private _config: PluginJsConfig;
  private _identities = new Map<string, IdentityUnit>();
  private _identityOrder: string[] = []; // LRU 顺序（最近访问在后）
  private _snapshots = new Map<string, { pluginId: string; snapshot: unknown; savedAt: number }>();
  private _contributes = new Map<string, ContributesEntry>(); // key: `${pluginId}:${type}:${id}`
  private readonly _subscribers = new Set<(event: string) => void>();

  constructor(config: Partial<PluginJsConfig> = {}) {
    this._config = {
      maxActiveIdentities: config.maxActiveIdentities ?? 8,
      idleTimeout: config.idleTimeout ?? 300000,
      enableStateSnapshot: config.enableStateSnapshot ?? true,
      maxStateSnapshots: config.maxStateSnapshots ?? 16,
    };
  }

  /** 当前配置。 */
  get config(): PluginJsConfig {
    return { ...this._config };
  }

  /** 当前身份单元列表。 */
  get identities(): IdentityUnit[] {
    return Array.from(this._identities.values());
  }

  /** 当前活跃身份单元数。 */
  get activeCount(): number {
    return Array.from(this._identities.values()).filter(
      (i) => i.state === 'active' || i.state === 'idle',
    ).length;
  }

  /** 当前 contributes 列表。 */
  get contributes(): ContributesEntry[] {
    return Array.from(this._contributes.values());
  }

  /** 订阅事件。 */
  subscribe(fn: (event: string) => void): () => void {
    this._subscribers.add(fn);
    return () => {
      this._subscribers.delete(fn);
    };
  }

  /** 通知订阅者。 */
  private _notify(event: string): void {
    for (const fn of this._subscribers) {
      try {
        fn(event);
      } catch {
        // 订阅者异常不影响其他订阅者
      }
    }
  }

  // ──────────────────────────────────────────────────────────────────────
  // Actions
  // ──────────────────────────────────────────────────────────────────────

  /**
   * 创建身份单元。
   *
   * label 由宿主生成（`plugin-<id>` 格式），不可被插件指定。
   */
  createIdentity(pluginId: string): IdentityActionResult {
    // 检查 pluginId 格式
    if (!this._isValidPluginId(pluginId)) {
      return { ok: false, code: 'E_INVALID_PLUGIN_ID', message: `无效的插件 ID: ${pluginId}` };
    }

    // 检查是否已存在
    if (this._identities.has(pluginId)) {
      return { ok: false, code: 'E_ALREADY_EXISTS', message: `身份单元已存在: ${pluginId}` };
    }

    // 检查活跃身份上限
    if (this.activeCount >= this._config.maxActiveIdentities) {
      // LRU 驱逐
      const evicted = this._evictLru();
      if (!evicted) {
        return { ok: false, code: 'E_LRU_FAIL', message: '无法驱逐身份单元' };
      }
    }

    // 生成 label
    const label = this._generateLabel(pluginId);

    // 创建身份单元
    const identity: IdentityUnit = {
      pluginId,
      label,
      state: 'idle',
      createdAt: Date.now(),
      lastAccessedAt: Date.now(),
    };

    this._identities.set(pluginId, identity);
    this._identityOrder.push(pluginId);
    this._notify(`identity:created:${pluginId}`);

    return { ok: true };
  }

  /**
   * 挂载视图 iframe。
   *
   * 视图 iframe 固定 `sandbox="allow-scripts"`，不给 `allow-same-origin`。
   */
  mountView(pluginId: string, url: string, width = 800, height = 600): IdentityActionResult {
    const identity = this._identities.get(pluginId);
    if (!identity) {
      return { ok: false, code: 'E_NOT_FOUND', message: `身份单元不存在: ${pluginId}` };
    }

    if (identity.state === 'evicting' || identity.state === 'evicted') {
      return { ok: false, code: 'E_EVICTED', message: `身份单元已驱逐: ${pluginId}` };
    }

    if (identity.state === 'active') {
      return { ok: false, code: 'E_ALREADY_ACTIVE', message: `身份单元已活跃: ${pluginId}` };
    }

    // 更新最后访问时间
    identity.lastAccessedAt = Date.now();
    this._updateLruOrder(pluginId);

    // 设置视图配置
    identity.viewConfig = {
      sandbox: 'allow-scripts',
      allowSameOrigin: false,
      url,
      width,
      height,
    };

    // 更新状态
    identity.state = 'active';
    this._notify(`identity:activated:${pluginId}`);

    return { ok: true };
  }

  /**
   * 卸载视图 iframe。
   *
   * 面板关闭即销毁视图 iframe。
   */
  unmountView(pluginId: string): IdentityActionResult {
    const identity = this._identities.get(pluginId);
    if (!identity) {
      return { ok: false, code: 'E_NOT_FOUND', message: `身份单元不存在: ${pluginId}` };
    }

    if (identity.state !== 'active') {
      return { ok: false, code: 'E_NOT_ACTIVE', message: `身份单元未活跃: ${pluginId}` };
    }

    // 更新最后访问时间
    identity.lastAccessedAt = Date.now();
    this._updateLruOrder(pluginId);

    // 清除视图配置（exactOptionalPropertyTypes 下必须用 delete 而非赋 undefined）
    delete identity.viewConfig;

    // 更新状态
    identity.state = 'idle';
    this._notify(`identity:deactivated:${pluginId}`);

    return { ok: true };
  }

  /**
   * 销毁身份单元。
   */
  destroyIdentity(pluginId: string): IdentityActionResult {
    const identity = this._identities.get(pluginId);
    if (!identity) {
      return { ok: false, code: 'E_NOT_FOUND', message: `身份单元不存在: ${pluginId}` };
    }

    // 如果正在活跃，先卸载视图
    if (identity.state === 'active') {
      delete identity.viewConfig;
    }

    // 保存状态快照
    if (this._config.enableStateSnapshot && identity.stateSnapshot !== undefined) {
      this._saveSnapshot(pluginId, identity.stateSnapshot);
    }

    // 从 LRU 顺序中移除
    const index = this._identityOrder.indexOf(pluginId);
    if (index !== -1) {
      this._identityOrder.splice(index, 1);
    }

    // 删除身份单元
    this._identities.delete(pluginId);

    // 清理该插件的 contributes
    this._cleanContributes(pluginId);

    this._notify(`identity:destroyed:${pluginId}`);

    return { ok: true };
  }

  /**
   * 驱逐 LRU 身份单元。
   */
  private _evictLru(): boolean {
    // 找到最久未访问的活跃身份单元（LRU 顺序中最早的）
    for (const pluginId of this._identityOrder) {
      const identity = this._identities.get(pluginId);
      if (identity && (identity.state === 'active' || identity.state === 'idle')) {
        return this._evictIdentity(pluginId);
      }
    }
    return false;
  }

  /**
   * 驱逐指定身份单元。
   */
  private _evictIdentity(pluginId: string): boolean {
    const identity = this._identities.get(pluginId);
    if (!identity) {
      return false;
    }

    identity.state = 'evicting';
    this._notify(`identity:evicting:${pluginId}`);

    // 保存状态快照
    if (this._config.enableStateSnapshot && identity.viewConfig !== undefined) {
      const snapshot = {
        viewConfig: identity.viewConfig,
        savedAt: Date.now(),
      };
      this._saveSnapshot(pluginId, snapshot);
    }

    // 从 LRU 顺序中移除
    const index = this._identityOrder.indexOf(pluginId);
    if (index !== -1) {
      this._identityOrder.splice(index, 1);
    }

    // 删除身份单元
    this._identities.delete(pluginId);

    identity.state = 'evicted';
    this._notify(`identity:evicted:${pluginId}`);

    return true;
  }

  /**
   * 保存状态快照。
   */
  private _saveSnapshot(pluginId: string, snapshot: unknown): void {
    // 检查快照数量上限
    if (this._snapshots.size >= this._config.maxStateSnapshots) {
      // 删除最旧的快照
      const oldest = Array.from(this._snapshots.entries())
        .sort((a, b) => a[1].savedAt - b[1].savedAt)
        .pop();
      if (oldest) {
        this._snapshots.delete(oldest[0]);
      }
    }

    this._snapshots.set(pluginId, {
      pluginId,
      snapshot,
      savedAt: Date.now(),
    });
  }

  /**
   * 更新 LRU 顺序。
   */
  private _updateLruOrder(pluginId: string): void {
    const index = this._identityOrder.indexOf(pluginId);
    if (index !== -1) {
      this._identityOrder.splice(index, 1);
    }
    this._identityOrder.push(pluginId);
  }

  // ──────────────────────────────────────────────────────────────────────
  // Contributes 注册
  // ──────────────────────────────────────────────────────────────────────

  /**
   * 注册 contributes。
   */
  registerContributes(
    pluginId: string,
    type: string,
    id: string,
    metadata: Record<string, unknown>,
  ): IdentityActionResult {
    const identity = this._identities.get(pluginId);
    if (!identity) {
      return { ok: false, code: 'E_NOT_FOUND', message: `身份单元不存在: ${pluginId}` };
    }

    if (identity.state === 'evicting' || identity.state === 'evicted') {
      return { ok: false, code: 'E_EVICTED', message: `身份单元已驱逐: ${pluginId}` };
    }

    const key = `${pluginId}:${type}:${id}`;

    // 检查是否已存在
    if (this._contributes.has(key)) {
      return { ok: false, code: 'E_ALREADY_EXISTS', message: `Contributes 已存在: ${key}` };
    }

    const entry: ContributesEntry = {
      pluginId,
      type,
      id,
      metadata,
      registeredAt: Date.now(),
    };

    this._contributes.set(key, entry);
    this._notify(`contributes:registered:${key}`);

    return { ok: true };
  }

  /**
   * 注销 contributes。
   */
  unregisterContributes(pluginId: string, type: string, id: string): IdentityActionResult {
    const key = `${pluginId}:${type}:${id}`;

    if (!this._contributes.has(key)) {
      return { ok: false, code: 'E_NOT_FOUND', message: `Contributes 不存在: ${key}` };
    }

    this._contributes.delete(key);
    this._notify(`contributes:unregistered:${key}`);

    return { ok: true };
  }

  /**
   * 清理插件的所有 contributes。
   */
  private _cleanContributes(pluginId: string): void {
    const toRemove: string[] = [];
    for (const key of this._contributes.keys()) {
      if (key.startsWith(`${pluginId}:`)) {
        toRemove.push(key);
      }
    }
    for (const key of toRemove) {
      this._contributes.delete(key);
      this._notify(`contributes:unregistered:${key}`);
    }
  }

  // ──────────────────────────────────────────────────────────────────────
  // 内部工具
  // ──────────────────────────────────────────────────────────────────────

  /**
   * 验证插件 ID 格式。
   *
   * 插件 ID 应为 reverse-domain 格式（如 `test.nonexistent`）。
   */
  private _isValidPluginId(pluginId: string): boolean {
    return /^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$/.test(pluginId);
  }

  /**
   * 生成 Webview label。
   *
   * label 由宿主生成（`plugin-<id>` 格式）。
   */
  private _generateLabel(pluginId: string): string {
    return `plugin-${pluginId}`;
  }

  /**
   * 获取状态快照。
   */
  getStateSnapshot(pluginId: string): { snapshot: unknown; savedAt: number } | undefined {
    const entry = this._snapshots.get(pluginId);
    return entry ? { snapshot: entry.snapshot, savedAt: entry.savedAt } : undefined;
  }

  /**
   * 清除状态快照。
   */
  clearStateSnapshot(pluginId: string): void {
    this._snapshots.delete(pluginId);
  }
}
