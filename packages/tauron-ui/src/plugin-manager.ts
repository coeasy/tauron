// ──────────────────────────────────────────────────────────────────────────
// Plugin Manager 状态管理（开发计划 §4.10 `<oc-plugin-manager>`）。
//
// 职责：列出已装插件、disabled-by-safemode 逐条启用、卸载。
// 消费 `usePlugin` + `host_registry_list`。
//
// 本模块为纯状态层，不依赖 DOM 或 Lit，便于离线测试。
// ──────────────────────────────────────────────────────────────────────────

import type { Backend, HostErrorShape } from '@tauron/host';
import { normalizeError as normalizeHostError } from '@tauron/host';

// ──────────────────────────────────────────────────────────────────────────
// 类型
// ──────────────────────────────────────────────────────────────────────────

/**
 * 规范化后的宿主错误码。
 *
 * 与 `@tauron/host` 的 {@link HostErrorShape} 保持一致：已知线上错误码，
 * 或无法识别时归入 `E_UNKNOWN`。
 */
export type NormalizedErrorCode = HostErrorShape['code'];

/** 插件在列表中的展示状态。 */
export type PluginListState =
  | { status: 'idle' }
  | { status: 'loading' }
  | { status: 'error'; code: NormalizedErrorCode; message: string }
  | { status: 'success'; plugins: PluginSummary[] };

/** 插件摘要（host_registry_list 返回项的精简版）。 */
export interface PluginSummary {
  /** 插件 ID（反域名格式）。 */
  id: string;
  /** 插件名称。 */
  name: string;
  /** 当前版本。 */
  version: string;
  /** 插件类型（js/process/wasm/rust）。 */
  pluginType: string;
  /** 生命周期状态。 */
  state: string;
  /** 是否被 safemode 禁用。 */
  disabledBySafemode: boolean;
  /** 插件图标 URL（可选）。 */
  iconUrl?: string;
  /** 插件描述（可选）。 */
  description?: string;
}

/** 插件操作结果。 */
export type PluginActionResult =
  | { ok: true }
  | { ok: false; code: NormalizedErrorCode; message: string };

/** 插件管理器配置。 */
export interface PluginManagerConfig {
  /** Backend 实例（用于 IPC 调用）。 */
  backend: Backend;
}

// ──────────────────────────────────────────────────────────────────────────
// PluginManagerStore
// ──────────────────────────────────────────────────────────────────────────

/**
 * 插件管理器状态存储。
 *
 * 不依赖 Lit 或任何 UI 框架，可在 Node.js 环境中测试。
 * 订阅者通过 `subscribe` 注册回调，状态变化时自动通知。
 */
export class PluginManagerStore {
  private _state: PluginListState = { status: 'idle' };
  private _selectedId: string | null = null;
  private readonly _backend: Backend;
  private readonly _subscribers = new Set<(state: StoreSnapshot) => void>();

  constructor(backend: Backend) {
    this._backend = backend;
  }

  /** 当前状态快照。 */
  get snapshot(): StoreSnapshot {
    return {
      state: this._state,
      selectedId: this._selectedId,
    };
  }

  /** 当前列表状态。 */
  get state(): PluginListState {
    return this._state;
  }

  /** 当前选中的插件 ID。 */
  get selectedId(): string | null {
    return this._selectedId;
  }

  /** 订阅状态变化。返回取消订阅函数。 */
  subscribe(fn: (snapshot: StoreSnapshot) => void): () => void {
    this._subscribers.add(fn);
    // 立即发送当前状态（捕获异常，不影响其他订阅者）
    try {
      fn(this.snapshot);
    } catch {
      // 订阅者异常不影响其他订阅者
    }
    return () => {
      this._subscribers.delete(fn);
    };
  }

  /** 通知所有订阅者。 */
  private _notify(): void {
    const snapshot = this.snapshot;
    for (const fn of this._subscribers) {
      try {
        fn(snapshot);
      } catch {
        // 订阅者异常不影响其他订阅者
      }
    }
  }

  /** 设置列表状态。 */
  private _setState(state: PluginListState): void {
    this._state = state;
    this._notify();
  }

  /** 设置选中插件。 */
  private _setSelectedId(id: string | null): void {
    this._selectedId = id;
    this._notify();
  }

  // ──────────────────────────────────────────────────────────────────────
  // Actions
  // ──────────────────────────────────────────────────────────────────────

  /**
   * 刷新插件列表。
   *
   * 调用 `host_registry_list` 获取已安装插件列表。
   * 成功时返回插件摘要数组，失败时设置错误状态。
   */
  async refresh(): Promise<PluginSummary[]> {
    this._setState({ status: 'loading' });
    try {
      const result = await this._backend.invoke<RegistryListResult>('host_registry_list');
      const plugins = result.plugins.map((p) => toPluginSummary(p));
      this._setState({ status: 'success', plugins });
      return plugins;
    } catch (err) {
      const { code, message } = normalizeError(err);
      this._setState({ status: 'error', code, message });
      return [];
    }
  }

  /**
   * 选择插件。
   *
   * 选中后更新状态，但不触发 IPC 调用。
   */
  select(id: string | null): void {
    this._setSelectedId(id);
  }

  /**
   * 启用插件。
   *
   * 调用 `host_registry_admin` 执行 enable 操作。
   */
  async enable(pluginId: string): Promise<PluginActionResult> {
    return this._adminOp(pluginId, 'enable');
  }

  /**
   * 禁用插件。
   *
   * 调用 `host_registry_admin` 执行 disable 操作。
   */
  async disable(pluginId: string): Promise<PluginActionResult> {
    return this._adminOp(pluginId, 'disable');
  }

  /**
   * 卸载插件。
   *
   * 调用 `host_registry_admin` 执行 uninstall 操作。
   */
  async uninstall(pluginId: string): Promise<PluginActionResult> {
    return this._adminOp(pluginId, 'uninstall');
  }

  /**
   * 尝试启用被 safemode 禁用的插件。
   *
   * 调用 `host_registry_admin` 的 `enable` 操作：safemode 是否放行由宿主
   * 生命周期状态机判定（插件再次崩溃时宿主会重新标记 disabled-by-safemode）。
   * 线格式只有 D15 定稿的 4 个操作，不存在 `trial_enable` 操作名。
   */
  async tryEnable(pluginId: string): Promise<PluginActionResult> {
    return this._adminOp(pluginId, 'enable');
  }

  /**
   * 清除插件。
   *
   * 调用 `host_registry_admin` 执行 purge 操作（删除所有数据）。
   */
  async purge(pluginId: string): Promise<PluginActionResult> {
    return this._adminOp(pluginId, 'purge');
  }

  // ──────────────────────────────────────────────────────────────────────
  // Internal
  // ──────────────────────────────────────────────────────────────────────

  /**
   * 执行管理操作。
   *
   * 线格式（D15 / Rust `HostAdminOp`）：`{ op: { op, id } }`——操作与目标插件 id
   * 同处一个结构体参数；插件身份不由本参数决定（主窗特权命令）。
   */
  private async _adminOp(pluginId: string, op: AdminOp): Promise<PluginActionResult> {
    try {
      await this._backend.invoke('host_registry_admin', { op: { op, id: pluginId } });
      return { ok: true };
    } catch (err) {
      const { code, message } = normalizeError(err);
      return { ok: false, code, message };
    }
  }
}

// ──────────────────────────────────────────────────────────────────────────
// 辅助类型
// ──────────────────────────────────────────────────────────────────────────

/** 管理操作类型（镜像 Rust `RegistryAdminOp` 的 D15 定稿四值枚举）。 */
export type AdminOp = 'enable' | 'disable' | 'uninstall' | 'purge';

/** 状态快照（subscribe 回调的参数）。 */
export interface StoreSnapshot {
  state: PluginListState;
  selectedId: string | null;
}

/** host_registry_list 返回结果。 */
interface RegistryListResult {
  plugins: Array<{
    id: string;
    name: string;
    version: string;
    type: string;
    state: string;
    disabled_by_safemode: boolean;
    icon_url?: string;
    description?: string;
  }>;
}

// ──────────────────────────────────────────────────────────────────────────
// 辅助函数
// ──────────────────────────────────────────────────────────────────────────

/**
 * 将 RegistryListResult 中的插件项转换为 PluginSummary。
 */
function toPluginSummary(p: RegistryListResult['plugins'][number]): PluginSummary {
  return {
    id: p.id,
    name: p.name,
    version: p.version,
    pluginType: p.type,
    state: p.state,
    disabledBySafemode: p.disabled_by_safemode,
    ...(p.icon_url !== undefined ? { iconUrl: p.icon_url } : {}),
    ...(p.description !== undefined ? { description: p.description } : {}),
  };
}

/**
 * 将未知错误规范化为 { code, message }。
 *
 * 复用宿主侧的 {@link normalizeHostError}，确保 UI 与宿主对错误码的判断一致：
 * 只有已知的线上错误码会被保留，其余归为 `E_UNKNOWN`。
 */
function normalizeError(err: unknown): { code: NormalizedErrorCode; message: string } {
  const { code, message } = normalizeHostError(err);
  return { code, message };
}
