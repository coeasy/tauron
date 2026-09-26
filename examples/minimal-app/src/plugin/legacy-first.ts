// ⚠️ deprecated（0.4-A2）：本文件是 **legacy iframe 插件**的旧实现，仅作对照保留。
//
// 运行位置：`sandbox="allow-scripts"` 的 iframe（opaque origin，碰不到 Tauri
// IPC），经 `@tauron/plugin-sdk` 的 postMessage 握手协议与宿主通信。
//
// 主推路径已切换到 `@tauron/app-plugin-sdk`（见 `first.ts`）：插件运行在
// `plugin-<id>` 标签的 webview 里，经事件总线 + 调用投递（0.4-A1）与宿主
// 及其他插件通信，无需宿主侧逐个写桥接 handler。
//
// 新插件请勿参考本文件；宿主侧的 `PluginBridge` / `callPluginMethod` 仍可用于
// 纯 iframe 嵌入场景（那是另一条产品线，不属于插件系统调用链）。

import { registerPlugin } from '@tauron/plugin-sdk';

registerPlugin({
  name: 'com.example.formatter',
  version: '1.0.0',
  methods: {
    async format(opts) {
      const { code } = opts.args as { code: string };
      // 演示逻辑：合并空白 + 去首尾
      return code.replace(/[ \t]+/g, ' ').trim();
    },
  },
  onEnable(ctx) {
    console.log('[formatter] 插件已握手并启用，权限：', ctx.permissions);
  },
});
