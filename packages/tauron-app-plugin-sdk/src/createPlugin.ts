// @tauron/app-plugin-sdk — createPlugin 工厂函数。

import type { PluginContext, PluginDefinition, PluginInstance } from './types.js';

/**
 * 宿主设置变更镜像 topic（§33 R2-4 / W6）。
 *
 * 宿主在设置**落盘提交之后**把 `{ key, value, source, revision }` 镜像到这个
 * topic（`event` 通道：可丢，慢消费者丢最旧）。它是宿主所有的私有 topic，
 * 默认无人可见——插件要收到帧，必须由主窗批准：
 * `host_events_approve(pluginId, HOST_SETTINGS_CHANGED_TOPIC)`。
 *
 * 与 Rust 侧 `HOST_SETTINGS_CHANGED_TOPIC` 的取值一致性由
 * `@tauron/contract-tests` 的 wire-gate 把守（改名即失败）。
 */
export const HOST_SETTINGS_CHANGED_TOPIC = 'host:settings:changed';

/**
 * 判定某条设置变更是否属于本插件的命名空间。
 *
 * 宿主侧的读写边界（`host_settings_get/set`）用的是**显式比较**而非前缀包含，
 * 这里保持一致：`plugin:p` 的插件不应收到 `plugin:p.a` 的变更（`plugin:p` 是
 * `plugin:p.a` 的前缀）。整 topic 获批后仍按此过滤，是「跨插件观察被拒」
 * 这条门禁在 SDK 侧的落点。
 */
function isOwnSettingsKey(pluginId: string, key: string): boolean {
  const namespace = `plugin:${pluginId}`;
  return key === namespace || key.startsWith(`${namespace}.`);
}

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

        // 5b. `onSettingsChanged` 接消息面（R2-4/W6）：以前这个钩子只在类型里
        //     声明、没有任何投递路径——插件作者写了它永远不会被调用。
        //     现在它走宿主的设置提交镜像 topic。宿主侧的 Store 观察队列
        //     （`SettingsStore::watch`，Rust 嵌入方扩展点）与这条镜像挂在同一个
        //     提交口上，两份投递各拿各的队列，不会互相饿死也不会分叉。
        //     未获主窗批准时订阅静默降级（context.ts 既有语义），钩子不触发；
        //     获批后仍只投递本插件命名空间的键（见 isOwnSettingsKey）。
        if (def.onSettingsChanged) {
          eventUnsubscribes.push(
            ctx.events.subscribe(HOST_SETTINGS_CHANGED_TOPIC, (payload) => {
              const frame = payload as { key?: unknown; value?: unknown } | null;
              if (typeof frame?.key !== 'string' || !isOwnSettingsKey(def.id, frame.key)) return;
              const hook = def.onSettingsChanged;
              if (!hook) return;
              // 钩子可能是 async：同步异常由 dispatchEvent 吞掉，rejected
              // promise 得在这里收口，否则一次设置变更就能打死事件泵。
              void Promise.resolve(hook({ [frame.key]: frame.value }, ctx)).catch((err) =>
                ctx.log.warn(`onSettingsChanged（${frame.key}）抛出异常（已吞掉）`, err),
              );
            }),
          );
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
