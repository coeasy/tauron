// tauron 应用插件示例（0.4-A2）—— **主推 SDK** `@tauron/app-plugin-sdk` 版。
//
// 运行位置：**插件 webview**（label `plugin-com.example.formatter`，由主窗经
// `host_window_create` 依 manifest 的 `entry.ui` 创建，页面即本目录所属的
// `plugin-window.html`）。不是 iframe——iframe 是 opaque origin，碰不到
// Tauri IPC；旧版 iframe 握手实现见 `legacy-first.ts`（deprecated）。
//
// 端到端链路（0.4-A1 调用投递，本页是**执行方**一端）：
//   主窗 `shell.callPlugin('com.example.formatter', 'format', …)`
//   → 宿主 `host_call_plugin` 受理登记（pending）
//   → `JsCallDelivery` 经事件总线把调用帧投进本插件队列
//   → 本页 SDK 内置**执行泵** drain 取回帧
//   → 按帧上 `cmd` 找到已注册命令 `format` 自动执行
//   → `host_call_result` 回填结果 → 主窗 `callTakeResult` 取走。
//
// 插件作者只需要 `createPlugin` + 声明 `commands`：注册命令即开泵，
// 结果回填全自动，不需要手写任何取件/回帧代码。

import { TauriBackend } from '@tauron/host/tauri';
import { HostClient } from '@tauron/host';
import { createPlugin, createPluginContext } from '@tauron/app-plugin-sdk';

const PLUGIN_ID = 'com.example.formatter';

const status = document.getElementById('status');
const show = (text: string): void => {
  if (status) status.textContent = text;
  console.log(`[${PLUGIN_ID}]`, text);
};

const plugin = createPlugin({
  id: PLUGIN_ID,
  name: 'Formatter 示例插件',
  version: '1.0.0',
  description: '0.4-A2 端到端证据：经主推 SDK + 调用投递跑通一次完整跨主体调用',
  commands: {
    // 帧到达时由执行泵自动调用；返回值经 host_call_result 回填给发起方，
    // handler 抛异常则按 E_CALL_EXEC_FAILED 回填（同样自动，无需手写）。
    async format(args) {
      const { code } = (args ?? {}) as { code?: string };
      // 演示逻辑：合并空白 + 去首尾（与 legacy-first.ts 一致，便于对照）
      return (code ?? '').replace(/[ \t]+/g, ' ').trim();
    },
  },
  activate(ctx) {
    show(`插件 ${ctx.pluginId} 已激活：命令 format 已注册，执行泵运行中，等待跨主体调用…`);
  },
});

void (async () => {
  try {
    // 与 main.ts 同一约定：root 注册（main.rs 用 tauron_generate_handler! 注册裸
    // 命令名）→ commandPrefix 传 ''。
    const backend = new TauriBackend({ commandPrefix: '' });
    const host = new HostClient({ backend });
    // createPluginContext 把 SDK 接到宿主 RPC 面：事件 4 原语 + 贡献注册 +
    // 调用结果回填，全部经 HostClient（self 档，身份由 webview label 解析）。
    const ctx = createPluginContext(PLUGIN_ID, host);
    await plugin.activate(ctx);
  } catch (err) {
    show(`激活失败：${err instanceof Error ? err.message : String(err)}`);
  }
})();
