// tauron 沙箱插件示例 —— 运行在 iframe（sandbox="allow-scripts"）内。
//
// registerPlugin 自动完成握手：从 URL hash 读取宿主注入的
// `#tauron-token=…` 并发送 ready；宿主回 init 后即可接收
// `__invoke:<method>` 事件并回发 `__result:<callId>`。

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
