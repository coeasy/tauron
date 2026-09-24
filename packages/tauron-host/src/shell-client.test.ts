// shell-client.ts 测试（P1 补齐：孤儿命令接通）

import { describe, it, expect, beforeEach } from 'vitest';
import { ShellClient } from './shell-client.js';
import type { RecoveryOutcome } from './shell-client.js';
import { MockBackend } from './backend.js';

const ALL_SHELL_CAPS = [
  'host_window_minimize',
  'host_window_maximize',
  'host_window_restore',
  'host_window_close',
  'host_window_quit',
  'host_window_relaunch',
  'host_window_create',
  'host_settings_get',
  'host_settings_set',
  'host_settings_adopt_legacy',
  'host_settings_migrate',
  'host_notify',
  'host_notifications_list',
  'host_notifications_read',
  'host_recover_boot',
  'host_recover_report',
  'host_recover_trial_enable',
  'host_market_check',
  'host_market_download',
  'host_market_install',
  'host_brand_info',
  'host_i18n_t',
  'host_i18n_t_params',
  'host_i18n_set_locale',
  'host_i18n_load',
  'host_i18n_stats',
  'host_i18n_cleanup_plugin',
  'host_contributes_register',
  'host_contributes_list',
  'host_registry_list_all',
  // P0-2：进程插件运行时（同属主窗特权档）
  'host_runtime_spawn',
  'host_runtime_health',
];

describe('ShellClient', () => {
  let backend: MockBackend;
  let client: ShellClient;

  beforeEach(() => {
    backend = new MockBackend({
      capabilities: ALL_SHELL_CAPS,
      pluginId: 'p.shell',
      // R8 的两条新命令 + 商城的诚实线字段：mock 按**宿主真实形状**应答，
      // 于是测试验证的是"我们如实转达了宿主的降级/模拟信息"，而不是自造成功。
      cases: [
        {
          cmd: 'host_window_relaunch',
          result: {
            reconcile: { scanned: 0, entered: 0, exited: 0, ignored: 0 },
            relaunchRequested: false,
            reason: '宿主没有重启原语（非 Tauri 宿主）',
          },
        },
        {
          cmd: 'host_window_create',
          result: {
            label: 'plugin-p.shell',
            pluginId: 'p.shell',
            created: false,
            reason: '宿主没有建窗原语（非 Tauri 宿主）',
          },
        },
        {
          cmd: 'host_market_check',
          result: {
            available: false,
            simulated: true,
            version: null,
            reason: '未接入更新源',
          },
        },
        {
          cmd: 'host_market_download',
          result: { ok: true, simulated: true, version: '2.0.0', reason: '未接入更新源' },
        },
        {
          cmd: 'host_market_install',
          result: { ok: true, simulated: true, version: null, reason: '未接入更新源' },
        },
      ],
    });
    client = new ShellClient({ backend });
  });

  // ── 窗口管理 ──

  describe('window', () => {
    it('windowMinimize 调用 host_window_minimize', async () => {
      await client.windowMinimize();
      expect(backend.invocations.some(i => i.cmd === 'host_window_minimize')).toBe(true);
    });

    it('windowMaximize 调用 host_window_maximize', async () => {
      await client.windowMaximize();
      expect(backend.invocations.some(i => i.cmd === 'host_window_maximize')).toBe(true);
    });

    it('windowRestore 调用 host_window_restore', async () => {
      await client.windowRestore();
      expect(backend.invocations.some(i => i.cmd === 'host_window_restore')).toBe(true);
    });

    it('windowClose 调用 host_window_close', async () => {
      await client.windowClose();
      expect(backend.invocations.some(i => i.cmd === 'host_window_close')).toBe(true);
    });

    it('windowQuit 调用 host_window_quit', async () => {
      await client.windowQuit();
      expect(backend.invocations.some(i => i.cmd === 'host_window_quit')).toBe(true);
    });

    it('windowRelaunch 带回对账结果，且降级路径如实上报（不假装已重启）', async () => {
      // 顺序不变量：宿主先对账、后重启。返回值必须能把对账结果带出来，
      // 否则"顺序对了"这件事在运行时不可观测。
      const outcome = await client.windowRelaunch();
      expect(backend.invocations.some(i => i.cmd === 'host_window_relaunch')).toBe(true);
      expect(outcome.reconcile).toEqual({ scanned: 0, entered: 0, exited: 0, ignored: 0 });
      // 非 Tauri/mock 后端没有重启原语 → 如实 false + reason，测试不得断言 true。
      expect(outcome.relaunchRequested).toBe(false);
      expect(outcome.reason).toBeTruthy();
    });

    it('windowCreate 传 pluginId 与可选尺寸，降级时不谎报 created', async () => {
      const outcome = await client.windowCreate('p.shell', { title: '面板', width: 800 });
      const inv = backend.invocations.find(i => i.cmd === 'host_window_create');
      expect(inv).toBeDefined();
      expect((inv!.args as any).pluginId).toBe('p.shell');
      expect((inv!.args as any).title).toBe('面板');
      expect((inv!.args as any).width).toBe(800);
      // 未传的字段不得出现在线形里（缺省由宿主决定，不传 undefined 更干净）。
      expect('height' in (inv!.args as Record<string, unknown>)).toBe(false);
      expect(outcome.label).toBe('plugin-p.shell');
      expect(outcome.created).toBe(false);
      expect(outcome.reason).toBeTruthy();
    });
  });

  // ── 设置 ──

  describe('settings', () => {
    it('settingsGet 调用 host_settings_get', async () => {
      await client.settingsGet('plugin:p.theme');
      const inv = backend.invocations.find(i => i.cmd === 'host_settings_get');
      expect(inv).toBeDefined();
      expect((inv!.args as any).key).toBe('plugin:p.theme');
    });

    it('settingsSet 调用 host_settings_set', async () => {
      await client.settingsSet('plugin:p.theme', 'dark');
      const inv = backend.invocations.find(i => i.cmd === 'host_settings_set');
      expect(inv).toBeDefined();
      expect((inv!.args as any).key).toBe('plugin:p.theme');
      expect((inv!.args as any).value).toBe('dark');
    });

    it('settingsAdoptLegacy / settingsMigrate 有线上路径（R7：迁移能力曾被实现但不可达）', async () => {
      // 断链回归：`host_settings_adopt_legacy` / `host_settings_migrate` 曾是纯 Rust
      // 函数、没注册成命令，于是 v1→v2 迁移只有单元测试能碰到。这条测试钉住
      // 「客户端真的能调它们」——两侧任一改名即红。
      await client.settingsAdoptLegacy({ 'plugin:p.theme': 'dark' });
      const adopt = backend.invocations.find(i => i.cmd === 'host_settings_adopt_legacy');
      expect(adopt).toBeDefined();
      expect((adopt!.args as any).doc).toEqual({ 'plugin:p.theme': 'dark' });

      const steps = await client.settingsMigrate();
      const migrate = backend.invocations.find(i => i.cmd === 'host_settings_migrate');
      expect(migrate).toBeDefined();
      expect(migrate!.args).toBeUndefined();
      // 返回迁移步数（0 = 已是v2，幂等）。
      expect(typeof steps === 'number' || steps === undefined).toBe(true);
    });
  });

  // ── 通知 ──

  describe('notify', () => {
    it('notify 调用 host_notify', async () => {
      await client.notify('p.audio', 'Title', 'Body');
      const inv = backend.invocations.find(i => i.cmd === 'host_notify');
      expect(inv).toBeDefined();
      const args = inv!.args as any;
      expect(args.pluginId).toBe('p.audio');
      expect(args.title).toBe('Title');
      expect(args.body).toBe('Body');
    });

    it('notificationsList / notificationsRead 是 host_notify 的配对读取端', async () => {
      // 断链回归：通知存储此前只写不读——通知中心无处取数。
      await client.notificationsList(10);
      let inv = backend.invocations.find(i => i.cmd === 'host_notifications_list');
      expect((inv!.args as any).limit).toBe(10);

      await client.notificationsRead('n1');
      inv = backend.invocations.find(i => i.cmd === 'host_notifications_read');
      expect((inv!.args as any).id).toBe('n1');

      // id 缺省 = 全部已读：线形不带 id 键。
      await client.notificationsRead();
      const last = backend.invocations.filter(i => i.cmd === 'host_notifications_read').pop();
      expect(last!.args).toBeUndefined();
    });
  });

  // ── 市场 ──

  describe('market', () => {
    it('marketCheck 调用 host_market_check，并把 simulated 如实带出来', async () => {
      const result = await client.marketCheck();
      expect(backend.invocations.some(i => i.cmd === 'host_market_check')).toBe(true);
      // R8 之前 `simulated` 只写在宿主注释里，前端拿不到 → 无法区分模拟与真实。
      expect(result.simulated).toBe(true);
      expect(result.available).toBe(false);
      expect(result.reason).toBeTruthy();
    });

    it('marketDownload / marketInstall 是 AutoUpdateClient 之外的第二条更新路径', async () => {
      // 断链回归：更新对话框「开始更新」此前零监听——ShellController 接线
      // 后经由这两个方法走真实宿主命令。
      const downloaded = await client.marketDownload('2.0.0');
      let inv = backend.invocations.find(i => i.cmd === 'host_market_download');
      expect((inv!.args as any).version).toBe('2.0.0');
      // 旧签名是 `Promise<void>`，把宿主的 `simulated`/`reason` 丢了 —— 现在必须带回来。
      expect(downloaded.simulated).toBe(true);
      expect(downloaded.version).toBe('2.0.0');

      const installed = await client.marketInstall();
      inv = backend.invocations.find(i => i.cmd === 'host_market_install');
      expect(inv!.args).toBeUndefined();
      expect(installed.simulated).toBe(true);
      expect(installed.reason).toBeTruthy();
    });
  });

  // ── 恢复 ──

  describe('recoverBoot', () => {
    it('调用 host_recover_boot', async () => {
      await client.recoverBoot();
      expect(backend.invocations.some(i => i.cmd === 'host_recover_boot')).toBe(true);
    });
  });

  describe('recoverReport', () => {
    it('success 上报不带 pluginId', async () => {
      await client.recoverReport('success');
      const inv = backend.invocations.find(i => i.cmd === 'host_recover_report');
      expect(inv).toBeDefined();
      expect((inv!.args as any).outcome).toBe('success');
      expect((inv!.args as any).pluginId).toBeUndefined();
    });

    it('failure 上报可带 pluginId（试验失败路径）', async () => {
      await client.recoverReport('failure', 'p.audio');
      const inv = backend.invocations.find(i => i.cmd === 'host_recover_report');
      expect((inv!.args as any).outcome).toBe('failure');
      expect((inv!.args as any).pluginId).toBe('p.audio');
    });

    it('outcome 是闭集：表外值在类型层不可表达', () => {
      // 类型门禁的运行时镜像：词表只有两个值。
      const allowed: RecoveryOutcome[] = ['success', 'failure'];
      expect(allowed).toHaveLength(2);
    });
  });

  describe('recoverTrialEnable', () => {
    it('调用 host_recover_trial_enable 并带 pluginId', async () => {
      await client.recoverTrialEnable('p.audio');
      const inv = backend.invocations.find(i => i.cmd === 'host_recover_trial_enable');
      expect(inv).toBeDefined();
      expect((inv!.args as any).pluginId).toBe('p.audio');
    });
  });

  // ── 市场 ──

  describe('marketCheck', () => {
    it('调用 host_market_check', async () => {
      await client.marketCheck();
      expect(backend.invocations.some(i => i.cmd === 'host_market_check')).toBe(true);
    });
  });

  // ── 品牌 ──

  describe('brandInfo', () => {
    it('调用 host_brand_info', async () => {
      await client.brandInfo();
      expect(backend.invocations.some(i => i.cmd === 'host_brand_info')).toBe(true);
    });
  });

  // ── i18n ──

  describe('i18nT', () => {
    it('调用 host_i18n_t', async () => {
      await client.i18nT('greeting');
      const inv = backend.invocations.find(i => i.cmd === 'host_i18n_t');
      expect(inv).toBeDefined();
      expect((inv!.args as any).key).toBe('greeting');
    });
  });

  describe('i18nTParams', () => {
    it('调用 host_i18n_t_params 并传 key + params', async () => {
      await client.i18nTParams('welcome', { name: 'Ada' });
      const inv = backend.invocations.find(i => i.cmd === 'host_i18n_t_params');
      expect(inv).toBeDefined();
      expect((inv!.args as any).key).toBe('welcome');
      expect((inv!.args as any).params).toEqual({ name: 'Ada' });
    });
  });

  describe('i18nSetLocale', () => {
    it('调用 host_i18n_set_locale 并传 locale', async () => {
      await client.i18nSetLocale('zh-CN');
      const inv = backend.invocations.find(i => i.cmd === 'host_i18n_set_locale');
      expect(inv).toBeDefined();
      expect((inv!.args as any).locale).toBe('zh-CN');
    });
  });

  describe('i18nLoad', () => {
    it('应用级装载：不带 pluginId', async () => {
      await client.i18nLoad('zh-CN', { 'oc.title': '标题' });
      const inv = backend.invocations.find(i => i.cmd === 'host_i18n_load');
      const args = inv!.args as any;
      expect(args.locale).toBe('zh-CN');
      expect(args.entries).toEqual({ 'oc.title': '标题' });
      expect(args.pluginId).toBeUndefined();
    });

    it('插件级装载：带 pluginId（宿主自动加命名空间前缀）', async () => {
      await client.i18nLoad('zh-CN', { title: '标题' }, 'p.audio');
      const inv = backend.invocations.find(i => i.cmd === 'host_i18n_load');
      const args = inv!.args as any;
      expect(args.pluginId).toBe('p.audio');
      expect(args.entries).toEqual({ title: '标题' });
    });
  });

  describe('i18nStats', () => {
    it('调用 host_i18n_stats 且不带参数', async () => {
      await client.i18nStats();
      const inv = backend.invocations.find(i => i.cmd === 'host_i18n_stats');
      expect(inv).toBeDefined();
      expect(inv!.args).toBeUndefined();
    });
  });

  describe('i18nCleanupPlugin', () => {
    it('调用 host_i18n_cleanup_plugin 并带 pluginId', async () => {
      await client.i18nCleanupPlugin('p.audio');
      const inv = backend.invocations.find(i => i.cmd === 'host_i18n_cleanup_plugin');
      expect(inv).toBeDefined();
      expect((inv!.args as any).pluginId).toBe('p.audio');
    });
  });

  // ── 贡献 ──
  // 注册入口已移至插件侧 HostClient（self 档绑 webview label），
  // 主窗客户端只保留读取。

  describe('contributes', () => {
    it('contributesList 调用 host_contributes_list', async () => {
      await client.contributesList();
      expect(backend.invocations.some(i => i.cmd === 'host_contributes_list')).toBe(true);
    });

    it('contributesList 带 kind', async () => {
      await client.contributesList('command');
      const inv = backend.invocations.find(i => i.cmd === 'host_contributes_list');
      expect((inv!.args as any).kind).toBe('command');
    });
  });

  // ── 注册表 ──

  describe('registryListAll', () => {
    it('调用 host_registry_list_all', async () => {
      await client.registryListAll();
      expect(backend.invocations.some(i => i.cmd === 'host_registry_list_all')).toBe(true);
    });
  });

  // ── 进程插件运行时（P0-2）──

  describe('runtimeSpawn / runtimeHealth', () => {
    const profile = {
      args: ['--port', '0'],
      env: { RUST_LOG: 'info' },
      signature: { algorithm: 'ed25519', signature: 'sig', signerId: 'acme' },
      binaryHash: 'sha256:abc',
      abi: { rustVersion: '1.83', interfaceHash: 'iface' },
    };

    it('spawn 传顶层 pluginId + profile（线形与 Rust 两个顶层参数对齐）', async () => {
      await client.runtimeSpawn('com.example.sidecar', profile);
      const inv = backend.invocations.find(i => i.cmd === 'host_runtime_spawn');
      // 顶层两个键——若包成 `req`，Tauri 会因缺 `plugin_id` 反序列化失败。
      expect(Object.keys(inv!.args as object).sort()).toEqual(['pluginId', 'profile']);
      expect((inv!.args as any).pluginId).toBe('com.example.sidecar');
      expect((inv!.args as any).profile.abi).toEqual({
        rustVersion: '1.83',
        interfaceHash: 'iface',
      });
      // 不得偷偷补 generatedAt：校验时刻由宿主时钟决定。
      expect(JSON.stringify(inv!.args)).not.toContain('generatedAt');
    });

    it('health 只传 lease（不传 pluginId——租约才是句柄）', async () => {
      await client.runtimeHealth('lease-1');
      const inv = backend.invocations.find(i => i.cmd === 'host_runtime_health');
      expect(inv!.args).toEqual({ lease: 'lease-1' });
    });

    it('租约失效错误按租约语义穿越（E_LEASE_EXPIRED，不是 E_CALL_NOT_FOUND）', async () => {
      const failing = new MockBackend({
        capabilities: ['host_runtime_health'],
        cases: [
          {
            cmd: 'host_runtime_health',
            args: { lease: 'nope' },
            error: { code: 'E_LEASE_EXPIRED', message: 'gone', retryable: false },
          },
        ],
      });
      const c = new ShellClient({ backend: failing });
      await expect(c.runtimeHealth('nope')).rejects.toMatchObject({
        code: 'E_LEASE_EXPIRED',
        retryable: false,
      });
    });
  });
});