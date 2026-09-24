// tauron 应用层贯通示例 —— 宿主主窗入口。
//
// 演示五条核心链路（全部走 tauron-adapter 的 45 条 host_* 命令面）：
// 1. iframe 沙箱插件：PluginBridge 握手（token 经 URL hash 注入）+ callPluginMethod
// 2. 窗口控制：ShellClient → Tauri 真实窗口操作
// 3. 系统能力：DialogClient（剪贴板）+ AutoUpdateClient（检查更新）
// 4. UI：@tauron/ui 的 <oc-toast> Web Component
// 5. 启动恢复（§4.14）：上报启动结果（驱动信号）+ 读回阶段决策

import { TauriBackend } from '@tauron/host/tauri';
import { ShellClient, DialogClient, AutoUpdateClient } from '@tauron/host';
import { PluginBridge, callPluginMethod } from '@tauron/plugin-sdk';
import '@tauron/ui/wc'; // 注册 <oc-toast> 等自定义元素

// ── 宿主基础 ──────────────────────────────────────────────────────────────

// root 注册（main.rs 用 tauron_generate_handler! 注册裸命令名）→ commandPrefix 传 ''。
// 若 Rust 侧改用 `.plugin(tauron_adapter::tauri::init())`（需 capability/ACL），
// 此处恢复默认前缀 'plugin:tauron|'。
const backend = new TauriBackend({ commandPrefix: '' });
const shell = new ShellClient({ backend });
const dialog = new DialogClient({ backend });
const updater = new AutoUpdateClient({
  backend,
  config: {
    endpoints: ['https://example.invalid/updates.json'],
    pubkey: '',
    checkIntervalSecs: 0, // 示例不启用定时轮询（手动点按钮检查）
    autoDownload: false,
  },
});

function el<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`missing #${id}`);
  return node as T;
}

interface ToastElement extends HTMLElement {
  push?: (item: { title: string; message: string; level: string }) => void;
}
const toast = el<ToastElement>('toast');

const log = (targetId: string, text: string): void => {
  el(targetId).textContent = text;
};
const setStatus = (text: string, ok = true): void => {
  const node = el('status');
  node.textContent = text;
  node.style.color = ok ? '#0a7d3c' : '#c62828';
};

// ── 1. iframe 沙箱插件 ───────────────────────────────────────────────────

const pluginBridge = new PluginBridge(
  // 声明给插件的权限（宿主快速失败层；权威判定在 Rust ACL）
  ['clipboard:read'],
  // 插件通过 invoke 动作请求宿主能力时的 handler
  async (method: string) => {
    if (method === 'clipboard:read') return dialog.clipboardRead();
    throw new Error(`host method not permitted in example: ${method}`);
  },
  async () => {},
);

/** 等待桥握手完成（bridge.ready：init 已回发给插件）。 */
function waitBridgeReady(bridge: PluginBridge, timeoutMs = 5000): Promise<void> {
  return new Promise((resolve, reject) => {
    const start = Date.now();
    const tick = (): void => {
      if ((bridge as unknown as { ready: boolean }).ready) {
        resolve();
      } else if (Date.now() - start > timeoutMs) {
        reject(new Error('插件握手超时（token 注入或消息路由失败）'));
      } else {
        setTimeout(tick, 50);
      }
    };
    tick();
  });
}

async function initPlugin(): Promise<void> {
  // createIframe 自动把握手 token 注入 URL hash（#tauron-token=…），
  // 插件侧 registerPlugin → createPluginContext 从 hash 读回并发送 ready。
  pluginBridge.createIframe(new URL('plugin.html', location.href).href);
  await waitBridgeReady(pluginBridge);
  el<HTMLButtonElement>('btn-plugin').disabled = false;
  setStatus('宿主 ↔ 插件握手完成，全部链路就绪');
}

el<HTMLButtonElement>('btn-plugin').addEventListener('click', () => {
  const code = el<HTMLInputElement>('plugin-input').value;
  void callPluginMethod<string>(pluginBridge, 'format', { code }).then(
    (formatted) => log('plugin-out', JSON.stringify({ input: code, formatted }, null, 2)),
    (err: Error) => log('plugin-out', `调用失败：${err.message}`),
  );
});

// ── 2. 窗口控制 ──────────────────────────────────────────────────────────

el<HTMLButtonElement>('btn-minimize').addEventListener('click', () => {
  shell.windowMinimize().catch((err: Error) => log('host-out', err.message));
});

// ── 3. 系统能力 ──────────────────────────────────────────────────────────

el<HTMLButtonElement>('btn-clipboard').addEventListener('click', () => {
  dialog
    .clipboardRead()
    .then((text) =>
      log('host-out', `剪贴板：${text === null || text === '' ? '（空）' : JSON.stringify(text)}`),
    )
    .catch((err: Error) => log('host-out', err.message));
});

el<HTMLButtonElement>('btn-update').addEventListener('click', () => {
  updater
    .checkUpdate()
    .then((info) =>
      log(
        'host-out',
        info.available
          ? `发现新版本 ${info.version ?? '?'}（当前 ${info.currentVersion}）——可 downloadUpdate()/installUpdate()`
          : `已是最新版本（${info.currentVersion}）`,
      ),
    )
    .catch((err: Error) => log('host-out', err.message));
});

// ── 4. UI 组件 ───────────────────────────────────────────────────────────

el<HTMLButtonElement>('btn-toast').addEventListener('click', () => {
  toast.push?.({ title: 'tauron', message: '来自 @tauron/ui 的通知', level: 'success' });
});

// ── 5. 启动恢复（§4.14）─────────────────────────────────────────────────

// 持久化已在 Rust 侧 `state_init()` 自动打开（app_config_dir）。崩溃检测的
// 「干净退出」判据是**本轮上报过 success**：漏报 = 每次重启被计为一次崩溃，
// 连续两次进安全模式（方向安全——一次 success 即自愈）。走 `bootstrap()` 的
// 应用由它代劳；这里演示的是裸用 ShellClient 时应用侧必须做的事。
//
// `recoverReport` 的返回即阶段决策：安全模式时 `disabledPlugins` 给出被禁用
// 名单，应用可用 `shell.recoverTrialEnable(id)` 逐个试启（试验失败 1 次即回落
// 禁用）；阶段判定已由宿主对账到 `PluginSummary.disabledBySafemode`，插件管理
// UI 无需自己读 `phase`。
void shell
  .recoverReport('success')
  .then((r) => {
    if (r.phase === 'normal') {
      log('host-out', `启动恢复：正常（持久化${r.persistence.enabled ? '已启用' : '未启用'}，加载来源 ${r.loadSource}）`);
    } else {
      const names = r.disabledPlugins.map((p) => p.pluginId).join(', ') || '（无）';
      log('host-out', `启动恢复：${r.phaseName}——被禁用插件：${names}`);
      setStatus(`启动处于${r.phaseName}，可用 recoverTrialEnable 逐个试启`, false);
    }
  })
  .catch((err: Error) => setStatus(`恢复上报失败：${err.message}`, false));

// ── 启动 ─────────────────────────────────────────────────────────────────

void initPlugin().catch((err: Error) => setStatus(`初始化失败：${err.message}`, false));
