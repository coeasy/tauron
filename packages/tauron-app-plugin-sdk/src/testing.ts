// @tauron/app-plugin-sdk — 测试工具。

import { HostClient, MockBackend } from '@tauron/host';
import type {
  CommandHandler,
  PluginContext,
  PluginDefinition,
  PluginInstance,
} from './types.js';
import { createPlugin } from './createPlugin.js';
import { createPluginContext } from './context.js';

/** 测试用宿主上下文。 */
export interface PluginTestContext {
  /** 插件实例。 */
  plugin: PluginInstance;
  /** 插件上下文（可调用 activate/deactivate）。 */
  ctx: PluginContext;
  /** Mock 后端（检查 invoke 调用）。 */
  backend: MockBackend;
  /** 宿主客户端。 */
  host: HostClient;

  /** 激活插件。 */
  activate(): Promise<void>;
  /** 失活插件。 */
  deactivate(): Promise<void>;
}

/**
 * 创建插件测试上下文。
 *
 * 用法：
 * ```typescript
 * import { createPluginTestContext } from '@tauron/app-plugin-sdk/testing';
 *
 * describe('My Plugin', () => {
 *   const { plugin, activate, deactivate } = createPluginTestContext(def);
 *
 *   it('activates correctly', async () => {
 *     await activate();
 *     expect(plugin.isActive).toBe(true);
 *   });
 * });
 * ```
 */
export function createPluginTestContext(
  def: PluginDefinition,
  options?: {
    /** 预配置的 MockBackend 响应。 */
    backendInvocations?: Record<string, unknown>;
  },
): PluginTestContext {
  // MockBackend 的预置行为必须在构造时传入（没有运行期 mock() 方法）
  const cases = Object.entries(options?.backendInvocations ?? {}).map(([cmd, result]) => ({
    cmd,
    result,
  }));

  const backend = new MockBackend({
    capabilities: [
      'host_plugin_call',
      'host_call_end',
      'host_cancel',
      'host_lifecycle_report',
      'host_contributes_register',
      'host_events_publish',
      'host_events_subscribe',
      'host_events_unsubscribe',
      'host_events_drain',
      'host_registry_list',
    ],
    cases,
  });

  const host = new HostClient({ backend });
  const plugin = createPlugin(def);
  const ctx = createPluginContext(def.id, host);

  return {
    plugin,
    ctx,
    backend,
    host,
    activate: () => plugin.activate(ctx),
    deactivate: () => plugin.deactivate(ctx),
  };
}

/**
 * 创建模拟的 PluginContext（用于单元测试命令处理器）。
 */
export function createMockContext(pluginId = 'com.example.test'): PluginContext {
  const commands = new Map<string, CommandHandler>();

  const ctx: PluginContext = {
    pluginId,

    get host(): HostClient {
      throw new Error('MockContext 不支持 host 调用');
    },

    commands: {
      register<TArgs = unknown, TResult = unknown>(
        id: string,
        handler: CommandHandler<TArgs, TResult>,
      ): { ok: boolean; error?: string } {
        if (commands.has(id)) {
          return { ok: false, error: `命令 "${id}" 已存在` };
        }
        commands.set(id, handler as CommandHandler);
        return { ok: true };
      },
      unregister(id: string): boolean {
        return commands.delete(id);
      },
      has(id: string): boolean {
        return commands.has(id);
      },
      list(): string[] {
        return [...commands.keys()];
      },
      async execute<TArgs = unknown, TResult = unknown>(
        id: string,
        args?: TArgs,
      ): Promise<TResult> {
        const handler = commands.get(id);
        if (!handler) throw new Error(`命令 "${id}" 不存在`);
        return (await handler(args as TArgs, ctx)) as TResult;
      },
    },

    settings: {
      registerTab(): { ok: boolean } { return { ok: true }; },
      unregisterTab(): boolean { return true; },
    },

    events: {
      async publish(): Promise<void> {},
      subscribe(): () => void { return () => {}; },
    },

    log: {
      info() {},
      warn() {},
      error() {},
    },
  };

  return ctx;
}
