// @tauron/app-plugin-sdk — createPlugin 工厂函数。

import type { PluginContext, PluginDefinition, PluginInstance } from './types.js';

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

      // 回滚账本：`activate` 中途抛错时必须把自己做过的事收干净。
      //
      // 为什么不能只靠 `deactivate` 兜底：`isActive` 只在 `activate` **跑完**才置
      // true，而 `deactivate` 开头就是 `if (!isActive) return`——激活失败后再调
      // deactivate 是空操作。于是注册过的命令会留下（跨主体调用仍会被受理并执行，
      // 尽管插件自己认为没激活），取件泵也不会停（`ctx.commands.register` 会
      // `startPump()`，而泵是 `setTimeout` 自续的，会一直空转 IPC）。
      const registeredCommands: string[] = [];
      const registeredTabs: string[] = [];

      try {
        // 1. 注册声明式命令
        if (def.commands) {
          for (const [id, handler] of Object.entries(def.commands)) {
            if (ctx.commands.register(id, handler).ok) registeredCommands.push(id);
          }
        }

        // 2. 注册声明式设置 Tab
        if (def.settings) {
          for (const tab of def.settings) {
            ctx.settings.registerTab(tab);
            registeredTabs.push(tab.id);
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
          // 3b. 注册完立刻**对账**（0.4-W3）：把「声明了却没注册」这类落差变成可
          //     检出的告警，而不是等到用户点了入口发现没反应。best-effort——对账
          //     失败不阻断激活（它本身就是诊断），但必须留痕。
          try {
            await ctx.host.contributesReconcile();
          } catch (err) {
            ctx.log.warn('contributes 对账发现分叉（声明与注册不一致，不影响激活）', err);
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
      } catch (err) {
        // 回滚：撤销注册（`commands.unregister` 会连带 `stopPumpIfIdle()`）、
        // 释放已建立的订阅。**原样抛出**——激活失败必须让调用方知道，
        // 不能因为「回滚成功」就假装激活成功。
        for (const id of registeredCommands) ctx.commands.unregister(id);
        for (const id of registeredTabs) ctx.settings.unregisterTab(id);
        for (const unsubscribe of eventUnsubscribes.splice(0)) unsubscribe();
        throw err;
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
