/**
 * tauron 配置管理（设计文档 §4.6）
 *
 * 四层配置：
 * Layer 1: 全局默认（schema 默认值，编译期确定）
 * Layer 2: 用户配置（settings-store 持久化，用户可修改）
 * Layer 3: 插件覆盖（manifest.settingsSchema 中的 default 值）
 * Layer 4: 会话覆盖（内存态，退出即弃）
 *
 * 优先级：session > plugin > user > default
 */

/** 配置层标识 */
export type ConfigLayer = 'default' | 'user' | 'plugin' | 'session';

/** 配置变更回调 */
export type ConfigChangeListener = (key: string, value: unknown) => void;

/** 自有属性检查（不含原型链；防 'toString'/'constructor' 等键泄漏继承值） */
function hasOwn(obj: object, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(obj, key);
}

/** 创建无原型键值袋（防 '__proto__' 键触发原型 setter） */
function nullProtoRecord(src?: Record<string, unknown>): Record<string, unknown> {
  const target: Record<string, unknown> = Object.create(null);
  if (src) {
    for (const [k, v] of Object.entries(src)) {
      target[k] = v;
    }
  }
  return target;
}

/**
 * 配置管理器
 */
export class ConfigManager {
  private defaults: Record<string, unknown> = Object.create(null);
  private userStore: Record<string, unknown> = Object.create(null);
  private pluginOverrides: Map<string, Record<string, unknown>> = new Map();
  private sessionOverrides: Record<string, unknown> = Object.create(null);
  private listeners: Set<ConfigChangeListener> = new Set();

  /**
   * 设置默认值（Layer 1）
   */
  setDefaults(defaults: Record<string, unknown>): void {
    this.defaults = nullProtoRecord(defaults);
  }

  /**
   * 设置用户配置（Layer 2）
   */
  setUserConfig(config: Record<string, unknown>): void {
    this.userStore = nullProtoRecord(config);
  }

  /**
   * 设置插件覆盖（Layer 3）
   */
  setPluginOverride(pluginId: string, overrides: Record<string, unknown>): void {
    this.pluginOverrides.set(pluginId, nullProtoRecord(overrides));
  }

  /**
   * 移除插件覆盖
   */
  removePluginOverride(pluginId: string): void {
    this.pluginOverrides.delete(pluginId);
  }

  /**
   * 设置会话覆盖（Layer 4）
   */
  setSessionOverride(key: string, value: unknown): void {
    this.sessionOverrides[key] = value;
  }

  /**
   * 移除会话覆盖
   */
  removeSessionOverride(key: string): void {
    delete this.sessionOverrides[key];
  }

  /**
   * 获取配置值（优先级：session > plugin > user > default）
   */
  get(key: string): unknown {
    // Layer 4: session
    if (hasOwn(this.sessionOverrides, key)) {
      return this.sessionOverrides[key];
    }

    // Layer 3: plugin overrides (any plugin)
    for (const overrides of this.pluginOverrides.values()) {
      if (hasOwn(overrides, key)) {
        return overrides[key];
      }
    }

    // Layer 2: user
    if (hasOwn(this.userStore, key)) {
      return this.userStore[key];
    }

    // Layer 1: default
    return this.defaults[key];
  }

  /**
   * 设置配置值
   */
  set(key: string, value: unknown, layer: 'user' | 'session' = 'user'): void {
    if (layer === 'session') {
      this.sessionOverrides[key] = value;
    } else {
      this.userStore[key] = value;
    }

    // Notify listeners
    this.listeners.forEach((listener) => {
      try {
        listener(key, value);
      } catch (err) {
        console.error('Config change listener error:', err);
      }
    });
  }

  /**
   * 获取所有配置（合并后）
   *
   * 使用对象展开（CreateDataProperty 语义）而非 Object.assign（Set 语义）：
   * 即使某层包含名为 `__proto__` 的自有键，也不会改变结果对象的原型。
   */
  getAll(): Record<string, unknown> {
    let result: Record<string, unknown> = { ...this.defaults };

    // Apply user
    result = { ...result, ...this.userStore };

    // Apply plugin overrides
    for (const overrides of this.pluginOverrides.values()) {
      result = { ...result, ...overrides };
    }

    // Apply session
    result = { ...result, ...this.sessionOverrides };

    return result;
  }

  /**
   * 订阅配置变更
   */
  onChange(listener: ConfigChangeListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /**
   * 清空所有配置
   */
  clear(): void {
    this.defaults = Object.create(null);
    this.userStore = Object.create(null);
    this.pluginOverrides.clear();
    this.sessionOverrides = Object.create(null);
  }
}
