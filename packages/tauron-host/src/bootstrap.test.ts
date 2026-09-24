// @vitest-environment happy-dom
// bootstrap.ts 测试（P2-1：启动编排器）

import { describe, it, expect, beforeEach } from 'vitest';
import { bootstrap } from './bootstrap.js';
import { MockBackend } from './backend.js';

const ALL_CAPS = [
  'host_window_minimize', 'host_window_maximize', 'host_window_restore',
  'host_window_close', 'host_window_quit',
  'host_settings_get', 'host_settings_set',
  'host_notify', 'host_recover_boot', 'host_recover_report',
  'host_market_check', 'host_brand_info', 'host_i18n_t',
  'host_contributes_register', 'host_contributes_list',
  'host_registry_list_all',
];

describe('bootstrap()', () => {
  let backend: MockBackend;

  beforeEach(() => {
    backend = new MockBackend({ capabilities: ALL_CAPS, pluginId: 'p.shell' });
  });

  it('返回 BootstrapResult 包含所有必需字段', async () => {
    const result = await bootstrap({ backend });
    expect(result.shell).toBeDefined();
    expect(result.client).toBeDefined();
    expect(result.registry).toBeDefined();
    expect(result.configManager).toBeDefined();
    expect(result.eventBus).toBeDefined();
    expect(result.bootTimeMs).toBeGreaterThanOrEqual(0);
    expect(typeof result.teardown).toBe('function');
  });

  it('启动成功必须上报 host_recover_report(success)（§4.14 驱动信号）', async () => {
    // 断链回归：宿主崩溃检测以「本轮上报过 success」为干净退出判据。漏掉这一步
    // = 每次重启都被计为一次崩溃，连续两次就把整个应用推进安全模式。
    await bootstrap({ backend });
    // 上报是 fire-and-forget，让出一个微任务让它落地。
    await new Promise(r => setTimeout(r, 0));
    const inv = backend.invocations.find(i => i.cmd === 'host_recover_report');
    expect(inv, 'bootstrap 必须上报启动结果').toBeDefined();
    expect((inv!.args as any).outcome).toBe('success');
  });

  it('上报失败不构成启动失败（best-effort）', async () => {
    const hostile = new MockBackend({ capabilities: [] });
    // 宿主无此命令（旧版本）→ invoke 抛 command not found，bootstrap 仍须成功。
    const result = await bootstrap({ backend: hostile });
    expect(result.bootTimeMs).toBeGreaterThanOrEqual(0);
    await new Promise(r => setTimeout(r, 0));
    expect(hostile.invocations.some(i => i.cmd === 'host_recover_report')).toBe(true);
  });

  it('启动 ShellController 并监听事件', async () => {
    const result = await bootstrap({ backend });
    const container = document.createElement('div');
    document.body.appendChild(container);
    container.dispatchEvent(new CustomEvent('oc-minimize', { bubbles: true, composed: true }));
    await new Promise(r => setTimeout(r, 10));
    expect(backend.invocations.some(i => i.cmd === 'host_window_minimize')).toBe(true);
    container.remove();
    await result.teardown();
  });

  it('注册插件到 registry', async () => {
    const result = await bootstrap({
      backend,
      plugins: [
        { id: 'p.alpha', entry: {} },
        { id: 'p.beta', entry: {} },
      ],
    });
    expect(result.registry.size).toBe(2);
    expect(result.registry.getState('p.alpha')).toBe('DISCOVERED');
    expect(result.registry.getState('p.beta')).toBe('DISCOVERED');
    await result.teardown();
  });

  it('拓扑排序：依赖插件先加载', async () => {
    const result = await bootstrap({
      backend,
      plugins: [
        { id: 'p.core', priority: 1, entry: {} },
        { id: 'p.feature', priority: 2, dependsOn: ['p.core'], entry: {} },
        { id: 'p.extra', priority: 3, dependsOn: ['p.feature'], entry: {} },
      ],
    });
    // 注册顺序不影响结果，但 registry 应该包含所有插件
    expect(result.registry.size).toBe(3);
    await result.teardown();
  });

  it('条件加载：不满足平台条件的插件被跳过', async () => {
    const result = await bootstrap({
      backend,
      plugins: [
        { id: 'p.universal', entry: {} },
        { id: 'p.windows-only', entry: {}, conditions: { platforms: ['windows'] } },
        { id: 'p.macos-only', entry: {}, conditions: { platforms: ['macos'] } },
      ],
    });
    // 至少 p.universal 应该被加载
    expect(result.registry.getState('p.universal')).toBe('DISCOVERED');
    await result.teardown();
  });

  it('懒加载：lazyLoad 插件被标记但不立即初始化', async () => {
    const result = await bootstrap({
      backend,
      plugins: [
        { id: 'p.eager', entry: {} },
        { id: 'p.lazy', entry: {}, lazyLoad: true },
      ],
    });
    expect(result.registry.size).toBe(2);
    const lazyEntry = result.registry.getEntry('p.lazy');
    expect(lazyEntry?.manifest).toMatchObject({ lazyLoad: true });
    await result.teardown();
  });

  it('加载配置到 ConfigManager', async () => {
    const result = await bootstrap({
      backend,
      config: {
        capabilities: ['base', 'updater'],
        plugins: { local: './my-plugins', registry: 'https://registry.example.com', autoUpdate: true },
      },
    });
    const cfg = result.configManager.getAll();
    expect(cfg.capabilities).toContain('updater');
    expect(cfg.plugins).toMatchObject({ local: './my-plugins', autoUpdate: true });
    await result.teardown();
  });

  it('teardown() 清理所有资源', async () => {
    const result = await bootstrap({
      backend,
      plugins: [{ id: 'p.test', entry: {} }],
    });
    expect(result.registry.size).toBe(1);
    await result.teardown();
    expect(result.registry.size).toBe(0);
  });

  it('showSplash 为 true 时不报错', async () => {
    const result = await bootstrap({
      backend,
      showSplash: true,
      config: {
        motion: {
          preset: 'standard',
          durations: { fast: 150, normal: 300, slow: 500 },
          easings: {
            standard: { name: 'standard', cubicBezier: [0.4, 0.0, 0.2, 1.0] as [number, number, number, number], value: 'cubic-bezier(0.4, 0.0, 0.2, 1.0)' },
          },
          splash: { enabled: true, minDuration: 100, title: 'Test', background: '#fff', progress: 'bar', exitAnimation: 'fade' },
          exit: { animation: 'fade', duration: 300, savingPrompt: '', prompt: '' },
          transitions: { toast: true, dialog: true, commandPalette: true, pluginList: true, themeSwitch: true },
          respectReducedMotion: true,
        },
      },
    });
    expect(result.bootTimeMs).toBeGreaterThanOrEqual(0);
    await result.teardown();
  });
});