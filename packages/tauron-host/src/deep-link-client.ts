// ──────────────────────────────────────────────────────────────────────────
// DeepLinkClient — 深链接客户端（P2-14）。
//
// 职责：
// 1. 注册深链接协议（tauron://）
// 2. 监听深链接事件
// 3. 解析深链接 URL
// 4. 处理深链接（路由到插件/处理器）
//
// 用法：
// ```typescript
// const deepLink = new DeepLinkClient({ backend, protocol: 'tauron' });
// deepLink.register();
// deepLink.listen((url) => {
//   console.log('Deep link:', url);
// });
// ```
// ──────────────────────────────────────────────────────────────────────────

import type { Backend } from './backend.js';

/** 深链接事件 */
export interface DeepLinkEvent {
  /** 完整 URL */
  url: string;
  /** 协议（如 'tauron'） */
  protocol: string;
  /** 主机（插件 ID） */
  host: string;
  /** 路径 */
  path: string;
  /** 查询参数 */
  query: Record<string, string>;
  /** 哈希片段 */
  hash: string;
}

/** DeepLinkClient 配置 */
export interface DeepLinkConfig {
  /** 协议名（如 'tauron'） */
  protocol: string;
  /** 是否启用 */
  enabled: boolean;
  /** 处理器列表 */
  handlers?: Array<{
    /** 匹配路径 */
    path: string;
    /** 处理器标识 */
    handler: string;
  }>;
}

/**
 * DeepLinkClient — 深链接管理器。
 */
export class DeepLinkClient {
  private readonly _backend: Backend;
  private readonly _config: DeepLinkConfig;
  private _subscribers: Set<(event: DeepLinkEvent) => void> = new Set();
  private _unlisten: (() => void) | null = null;

  constructor(options: { backend: Backend; config: DeepLinkConfig }) {
    this._backend = options.backend;
    this._config = options.config;
  }

  /** 是否已注册 */
  get isRegistered(): boolean {
    return this._unlisten !== null;
  }

  /** 协议名 */
  get protocol(): string {
    return this._config.protocol;
  }

  /**
   * 注册深链接协议。
   */
  async register(): Promise<void> {
    if (!this._config.enabled) {
      return;
    }

    await this._backend.invoke('host_deep_link_register', {
      protocol: this._config.protocol,
    });

    // 开始监听
    this._unlisten = await this._listen();
  }

  /**
   * 取消注册。
   */
  async unregister(): Promise<void> {
    if (this._unlisten) {
      this._unlisten();
      this._unlisten = null;
    }
  }

  /**
   * 订阅深链接事件。
   */
  subscribe(fn: (event: DeepLinkEvent) => void): () => void {
    this._subscribers.add(fn);
    return () => {
      this._subscribers.delete(fn);
    };
  }

  /**
   * 监听深链接事件（内部方法）。
   */
  private async _listen(): Promise<() => void> {
    return this._backend.listen('deep-link', (payload) => {
      const event = this._parseEvent(payload as DeepLinkEvent);
      for (const fn of this._subscribers) {
        try {
          fn(event);
        } catch {
          // 忽略订阅者异常
        }
      }
    });
  }

  /**
   * 解析深链接 URL。
   */
  parseUrl(url: string): DeepLinkEvent {
    const protocol = this._config.protocol;
    const regex = new RegExp(`^${protocol}://([^/]*)([^?#]*)([^#]*)(#.*)?$`);
    const match = url.match(regex);

    if (!match) {
      return {
        url,
        protocol,
        host: '',
        path: '',
        query: {},
        hash: '',
      };
    }

    const [, host, path, queryString, hash] = match;
    const query: Record<string, string> = {};

    if (queryString) {
      const searchParams = new URLSearchParams(queryString);
      searchParams.forEach((value, key) => {
        query[key] = value;
      });
    }

    return {
      url,
      protocol,
      host: host || '',
      path: path || '',
      query,
      hash: hash || '',
    };
  }

  /**
   * 解析事件（确保字段完整）。
   */
  private _parseEvent(event: Partial<DeepLinkEvent>): DeepLinkEvent {
    return {
      url: event.url || '',
      protocol: event.protocol || this._config.protocol,
      host: event.host || '',
      path: event.path || '',
      query: event.query || {},
      hash: event.hash || '',
    };
  }

  /**
   * 匹配处理器。
   */
  matchHandler(path: string): string | null {
    if (!this._config.handlers) {
      return null;
    }

    for (const handler of this._config.handlers) {
      if (path.startsWith(handler.path)) {
        return handler.handler;
      }
    }

    return null;
  }

  /**
   * 清理资源。
   */
  async destroy(): Promise<void> {
    await this.unregister();
    this._subscribers.clear();
  }
}

/**
 * 创建 DeepLinkClient 实例。
 *
 * 便捷工厂函数。
 */
export function createDeepLinkClient(options: { backend: Backend; config: DeepLinkConfig }): DeepLinkClient {
  return new DeepLinkClient(options);
}