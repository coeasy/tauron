// @tauron/app-plugin-sdk — createPlugin 工厂函数。

import type { HostClient } from '@tauron/host';
import type {
  CommandHandler,
  PluginContext,
  PluginDefinition,
  PluginInstance,
} from './types.js';

/**
 * 创建插件实例。
 *
 * 用法：
 * ```typescript
 * const plugin = createPlugin({
 *   id: 'com.example.myplugin',
 *   name: 'My Plugin',
 *   version: '1.0.0',
 *   async activate(ctx) {
 *     ctx.commands.register('hello', (args) => ({ msg: 'hi' }));
 *   },
 * });
 * export default plugin;
 * ```
 */
export function createPlugin(def: PluginDefinition): PluginInstance {
  /** 已建立的事件订阅退订函数（deactivate 时统一释放）。 */
  const eventUnsubscribes: (() => void)[] = [];

  let isActive = false;

  const instance: PluginInstance = {
    id: def.id,
    name: def.name,
    version: def.version,
    get isActive() {
      return isActive;
    },

    async activate(ctx: PluginContext) {
      if (isActive) return;

      // 1. 注册声明式命令
      if (def.commands) {
        for (const [id, handler] of Object.entries(def.commands)) {
          ctx.commands.register(id, handler);
        }
      }

      // 2. 注册声明式设置 Tab
      if (def.settings) {
        for (const tab of def.settings) {
          ctx.settings.registerTab(tab);
        }
      }

      // 3. 注册声明式贡献到宿主（self 档：署名由宿主从 webview label 解析，
      //    线形不带 pluginId）。best-effort：重复 id 等宿主错误只告警不阻断
      //    ——贡献是声明式附加物，不该让激活失败；卸载时由宿主按插件 id
      //    整体回收（`clear_plugin`）。
      if (def.contributes) {
        const entries: Array<{ kind: string; id: string; label: string }> = [
          ...(def.contributes.commands ?? []).map((c) => ({
            kind: 'command',
            id: c.id,
            label: c.title,
          })),
          // menus 没有展示名字段：label 用命令引用，UI 侧解析命令标题展示。
          ...(def.contributes.menus ?? []).map((m) => ({
            kind: 'menu',
            id: m.id,
            label: m.command,
          })),
          ...(def.contributes.panels ?? []).map((p) => ({
            kind: 'panel',
            id: p.id,
            label: p.title,
          })),
          ...(def.contributes.settingsTabs ?? []).map((t) => ({
            kind: 'settings',
            id: t.id,
            label: t.title,
          })),
          // 快捷键没有天然 id：用命令 id（同命令多绑定按 best-effort 告警）。
          ...(def.contributes.shortcuts ?? []).map((s) => ({
            kind: 'shortcut',
            id: s.command,
            label: s.accelerator,
          })),
        ];
        for (const entry of entries) {
          try {
            await ctx.host.contributesRegister(entry);
          } catch (err) {
            ctx.log.warn('contributes 注册失败（不影响激活）', entry, err);
          }
        }
      }

      // 4. 调用生命周期钩子
      if (def.activate) {
        await def.activate(ctx);
      }

      // 5. 注册事件订阅（投递到 def.onEvent）
      if (def.events?.subscribe) {
        for (const topic of def.events.subscribe) {
          eventUnsubscribes.push(
            ctx.events.subscribe(topic, (payload) => {
              def.onEvent?.(topic, payload, ctx);
            }),
          );
        }
      }

      isActive = true;
    },

    async deactivate(ctx: PluginContext) {
      if (!isActive) return;

      // 1. 调用失活钩子
      if (def.deactivate) {
        await def.deactivate(ctx);
      }

      // 2. 注销命令
      if (def.commands) {
        for (const id of Object.keys(def.commands)) {
          ctx.commands.unregister(id);
        }
      }

      // 3. 释放事件订阅（否则宿主侧订阅会悬挂）
      for (const unsubscribe of eventUnsubscribes.splice(0)) {
        unsubscribe();
      }

      isActive = false;
    },

    async dispose(ctx: PluginContext) {
      if (isActive) {
        await instance.deactivate(ctx);
      }

      // 调用清理钩子
      if (def.dispose) {
        await def.dispose(ctx);
      }
    },
  };

  return instance;
}
