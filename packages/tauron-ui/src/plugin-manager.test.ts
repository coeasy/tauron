// ──────────────────────────────────────────────────────────────────────────
// PluginManagerStore 单元测试。
//
// 使用 MockBackend 模拟 IPC，不依赖 DOM。
// ──────────────────────────────────────────────────────────────────────────

import { describe, expect, it, vi, beforeEach } from 'vitest';
import { MockBackend, HostException, type MockInvokeCase } from '@tauron/host';
import {
  PluginManagerStore,
  designTokens,
  safemodeTokens,
  toCssVariables,
  toCssText,
  type AdminConfirmRequest,
  type PluginListState,
} from '../src/index.js';

// ──────────────────────────────────────────────────────────────────────────
// 测试辅助
// ──────────────────────────────────────────────────────────────────────────

function makeBackend(cases: MockInvokeCase[] = []): MockBackend {
  return new MockBackend({
    capabilities: ['host_registry_list', 'host_registry_admin'],
    cases,
  });
}

function makePlugins(count: number): unknown[] {
  return Array.from({ length: count }, (_, i) => ({
    id: `test.plugin${i}`,
    name: `Plugin ${i}`,
    version: '1.0.0',
    type: 'js',
    state: 'running',
    disabled_by_safemode: false,
    ...(i === 1 ? { icon_url: 'https://example.com/icon.png' } : {}),
    ...(i === 2 ? { description: 'A test plugin' } : {}),
  }));
}

// ──────────────────────────────────────────────────────────────────────────
// PluginManagerStore 测试
// ──────────────────────────────────────────────────────────────────────────

describe('PluginManagerStore', () => {
  let store: PluginManagerStore;
  let backend: MockBackend;

  beforeEach(() => {
    backend = makeBackend();
    store = new PluginManagerStore(backend);
  });

  // ── 初始状态 ──────────────────────────────────────────────────────────

  it('初始状态为 idle', () => {
    expect(store.state).toEqual({ status: 'idle' });
  });

  it('初始选中为 null', () => {
    expect(store.selectedId).toBeNull();
  });

  it('snapshot 包含 state 和 selectedId', () => {
    const snap = store.snapshot;
    expect(snap).toEqual({
      state: { status: 'idle' },
      selectedId: null,
    });
  });

  // ── refresh ───────────────────────────────────────────────────────────

  it('refresh 成功时返回插件列表', async () => {
    const plugins = makePlugins(3);
    const b = makeBackend([
      {
        cmd: 'host_registry_list',
        result: { plugins },
      },
    ]);
    const s = new PluginManagerStore(b);
    const result = await s.refresh();
    expect(result).toHaveLength(3);
    expect(result[0]).toHaveProperty('id', 'test.plugin0');
    expect(result[0]).toHaveProperty('name', 'Plugin 0');
    expect(result[0]).toHaveProperty('pluginType', 'js');
    expect(result[0]).toHaveProperty('state', 'running');
    expect(result[0]).toHaveProperty('disabledBySafemode', false);
  });

  it('refresh 成功后状态为 success', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_list',
        result: { plugins: makePlugins(2) },
      },
    ]);
    const s = new PluginManagerStore(b);
    await s.refresh();
    expect(s.state.status).toBe('success');
    if (s.state.status === 'success') {
      expect(s.state.plugins).toHaveLength(2);
    }
  });

  it('refresh 失败时状态为 error', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_list',
        error: new HostException({
          code: 'E_HOST_PANIC',
          rawCode: 'E_HOST_PANIC',
          message: 'boom',
          retryable: false,
          retryClass: 'never',
        }),
      },
    ]);
    const s = new PluginManagerStore(b);
    const result = await s.refresh();
    expect(result).toEqual([]);
    expect(s.state.status).toBe('error');
    if (s.state.status === 'error') {
      expect(s.state.code).toBe('E_HOST_PANIC');
      expect(s.state.message).toContain('boom');
    }
  });

  it('refresh 加载过程中状态为 loading', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_list',
        result: {
          plugins: makePlugins(1),
        },
      },
    ]);
    const s = new PluginManagerStore(b);
    const promise = s.refresh();
    // 在 promise resolve 之前，状态应为 loading
    // 但 MockBackend 是同步的，所以 refresh 会立即 resolve
    // 我们用另一个方式验证：先 subscribe 再 refresh
    const states: PluginListState[] = [];
    s.subscribe((snap) => {
      states.push(snap.state);
    });
    await promise;
    expect(states).toContainEqual({ status: 'loading' });
    expect(states).toContainEqual(expect.objectContaining({ status: 'success' }));
  });

  it('refresh 后 iconUrl 正确传递', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_list',
        result: {
          plugins: [
            {
              id: 'test.icon',
              name: 'Icon Plugin',
              version: '1.0.0',
              type: 'js',
              state: 'running',
              disabled_by_safemode: false,
              icon_url: 'https://example.com/icon.png',
            },
          ],
        },
      },
    ]);
    const s = new PluginManagerStore(b);
    const result = await s.refresh();
    expect(result[0]?.iconUrl).toBe('https://example.com/icon.png');
  });

  it('refresh 后 description 正确传递', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_list',
        result: {
          plugins: [
            {
              id: 'test.desc',
              name: 'Desc Plugin',
              version: '1.0.0',
              type: 'js',
              state: 'running',
              disabled_by_safemode: false,
              description: 'A test plugin',
            },
          ],
        },
      },
    ]);
    const s = new PluginManagerStore(b);
    const result = await s.refresh();
    expect(result[0]?.description).toBe('A test plugin');
  });

  it('refresh 后无 iconUrl 时为 undefined', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_list',
        result: {
          plugins: [
            {
              id: 'test.noicon',
              name: 'No Icon',
              version: '1.0.0',
              type: 'js',
              state: 'running',
              disabled_by_safemode: false,
            },
          ],
        },
      },
    ]);
    const s = new PluginManagerStore(b);
    const result = await s.refresh();
    expect(result[0]).not.toHaveProperty('iconUrl');
  });

  // ── select ────────────────────────────────────────────────────────────

  it('select 设置选中插件', () => {
    store.select('test.plugin0');
    expect(store.selectedId).toBe('test.plugin0');
  });

  it('select null 取消选中', () => {
    store.select('test.plugin0');
    store.select(null);
    expect(store.selectedId).toBeNull();
  });

  it('select 触发订阅者通知', () => {
    const spy = vi.fn();
    store.subscribe(spy);
    store.select('test.plugin0');
    expect(spy).toHaveBeenCalledWith(expect.objectContaining({ selectedId: 'test.plugin0' }));
  });

  // ── enable/disable/uninstall/tryEnable/purge ─────────────────────────

  it('enable 成功', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_admin',
        args: { op: { op: 'enable', id: 'test.plugin0' } },
        result: {},
      },
    ]);
    const s = new PluginManagerStore(b);
    const result = await s.enable('test.plugin0');
    expect(result).toEqual({ ok: true });
  });

  it('enable 失败返回错误', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_admin',
        error: new HostException({
          code: 'E_AUTH_DENIED',
          rawCode: 'E_AUTH_DENIED',
          message: 'no perm',
          retryable: false,
          retryClass: 'never',
        }),
      },
    ]);
    const s = new PluginManagerStore(b);
    const result = await s.enable('test.plugin0');
    expect(result.ok).toBe(false);
    if (!result.ok && 'code' in result) {
      expect(result.code).toBe('E_AUTH_DENIED');
    }
  });

  it('disable 成功', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_admin',
        args: { op: { op: 'disable', id: 'test.plugin0' } },
        result: {},
      },
    ]);
    const s = new PluginManagerStore(b);
    const result = await s.disable('test.plugin0');
    expect(result).toEqual({ ok: true });
  });

  it('uninstall 走预览 → 确认 → 提交令牌（轮 43 / A83）', async () => {
    const reviewToken = {
      pluginId: 'test.plugin0',
      op: 'uninstall',
      version: '1.0.0',
      issuedAt: 1,
      expiresAt: 9999999999,
      nonce: 'n-1',
    };
    const b = makeBackend([
      {
        cmd: 'host_registry_admin',
        args: { op: { op: 'uninstall', id: 'test.plugin0', preview: true } },
        result: {
          kind: 'review',
          op: 'uninstall',
          pluginId: 'test.plugin0',
          version: '1.0.0',
          state: 'running',
          reviewToken,
        },
      },
    ]);
    const seen: AdminConfirmRequest[] = [];
    const s = new PluginManagerStore(b, (request) => {
      seen.push(request);
      return true;
    });
    const result = await s.uninstall('test.plugin0');
    expect(result).toEqual({ ok: true });
    // 第一步：预览（无副作用）。
    expect(b.invocations[0]?.args).toEqual({
      op: { op: 'uninstall', id: 'test.plugin0', preview: true },
    });
    // 确认请求携带预览事实。
    expect(seen).toEqual([
      { op: 'uninstall', pluginId: 'test.plugin0', version: '1.0.0', state: 'running' },
    ]);
    // 第二步：提交，原样带回一次性令牌。
    expect(b.invocations[1]?.args).toEqual({
      op: { op: 'uninstall', id: 'test.plugin0', reviewToken },
    });
  });

  it('uninstall 用户取消 → cancelled，无提交调用（轮 43 / A83）', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_admin',
        args: { op: { op: 'uninstall', id: 'test.plugin0', preview: true } },
        result: {
          kind: 'review',
          op: 'uninstall',
          pluginId: 'test.plugin0',
          version: '1.0.0',
          state: 'running',
          reviewToken: {
            pluginId: 'test.plugin0',
            op: 'uninstall',
            version: '1.0.0',
            issuedAt: 1,
            expiresAt: 9999999999,
            nonce: 'n-2',
          },
        },
      },
    ]);
    const s = new PluginManagerStore(b, () => false);
    const result = await s.uninstall('test.plugin0');
    expect(result).toEqual({ ok: false, cancelled: true });
    expect(b.invocations).toHaveLength(1);
    expect(b.invocations[0]?.args).toEqual({
      op: { op: 'uninstall', id: 'test.plugin0', preview: true },
    });
  });

  it('uninstall 默认确认在无 DOM 环境失败关闭（拒绝而非静默放行）', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_admin',
        args: { op: { op: 'uninstall', id: 'test.plugin0', preview: true } },
        result: {
          kind: 'review',
          op: 'uninstall',
          pluginId: 'test.plugin0',
          version: '1.0.0',
          state: 'running',
          reviewToken: {
            pluginId: 'test.plugin0',
            op: 'uninstall',
            version: '1.0.0',
            issuedAt: 1,
            expiresAt: 9999999999,
            nonce: 'n-3',
          },
        },
      },
    ]);
    // 不注入确认钩子：Node 测试环境没有 window.confirm → 必须拒绝。
    const s = new PluginManagerStore(b);
    const result = await s.uninstall('test.plugin0');
    expect(result).toEqual({ ok: false, cancelled: true });
    expect(b.invocations).toHaveLength(1);
  });

  it('uninstall 确认钩子抛错按取消处理（失败关闭）（轮 43 / A83）', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_admin',
        args: { op: { op: 'uninstall', id: 'test.plugin0', preview: true } },
        result: {
          kind: 'review',
          op: 'uninstall',
          pluginId: 'test.plugin0',
          version: '1.0.0',
          state: 'running',
          reviewToken: {
            pluginId: 'test.plugin0',
            op: 'uninstall',
            version: '1.0.0',
            issuedAt: 1,
            expiresAt: 9999999999,
            nonce: 'n-4',
          },
        },
      },
    ]);
    const s = new PluginManagerStore(b, () => {
      throw new Error('dialog broke');
    });
    const result = await s.uninstall('test.plugin0');
    expect(result).toEqual({ ok: false, cancelled: true });
    expect(b.invocations).toHaveLength(1);
  });

  it('uninstall 老宿主（预览即执行、无 review 判别）不重复提交', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_admin',
        args: { op: { op: 'uninstall', id: 'test.plugin0', preview: true } },
        result: {},
      },
    ]);
    let confirmCalls = 0;
    const s = new PluginManagerStore(b, () => {
      confirmCalls += 1;
      return true;
    });
    const result = await s.uninstall('test.plugin0');
    expect(result).toEqual({ ok: true });
    expect(b.invocations).toHaveLength(1);
    expect(confirmCalls).toBe(0);
  });

  it('tryEnable 成功', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_admin',
        args: { op: { op: 'enable', id: 'test.plugin0' } },
        result: {},
      },
    ]);
    const s = new PluginManagerStore(b);
    const result = await s.tryEnable('test.plugin0');
    expect(result).toEqual({ ok: true });
  });

  it('purge 走预览 → 确认 → 提交令牌（轮 43 / A83）', async () => {
    const reviewToken = {
      pluginId: 'test.plugin0',
      op: 'purge',
      version: '1.0.0',
      issuedAt: 1,
      expiresAt: 9999999999,
      nonce: 'n-5',
    };
    const b = makeBackend([
      {
        cmd: 'host_registry_admin',
        args: { op: { op: 'purge', id: 'test.plugin0', preview: true } },
        result: {
          kind: 'review',
          op: 'purge',
          pluginId: 'test.plugin0',
          version: '1.0.0',
          state: 'disabled',
          reviewToken,
        },
      },
    ]);
    const s = new PluginManagerStore(b, () => true);
    const result = await s.purge('test.plugin0');
    expect(result).toEqual({ ok: true });
    expect(b.invocations[0]?.args).toEqual({
      op: { op: 'purge', id: 'test.plugin0', preview: true },
    });
    expect(b.invocations[1]?.args).toEqual({
      op: { op: 'purge', id: 'test.plugin0', reviewToken },
    });
  });

  it('管理操作失败时返回结构化错误', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_admin',
        error: new Error('network error'),
      },
    ]);
    const s = new PluginManagerStore(b);
    const result = await s.disable('test.plugin0');
    expect(result.ok).toBe(false);
    if (!result.ok && 'code' in result) {
      expect(result.code).toBe('E_UNKNOWN');
      expect(result.message).toContain('network error');
    }
  });

  // ── subscribe ─────────────────────────────────────────────────────────

  it('subscribe 立即发送当前状态', () => {
    const spy = vi.fn();
    store.subscribe(spy);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy).toHaveBeenCalledWith({
      state: { status: 'idle' },
      selectedId: null,
    });
  });

  it('unsubscribe 停止通知', async () => {
    const spy = vi.fn();
    const unsub = store.subscribe(spy);
    store.select('test.plugin0');
    expect(spy).toHaveBeenCalledTimes(2); // 初始 + select
    unsub();
    store.select('test.plugin1');
    expect(spy).toHaveBeenCalledTimes(2); // 不再增加
  });

  it('多个订阅者各自独立', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_list',
        result: { plugins: makePlugins(1) },
      },
    ]);
    const s = new PluginManagerStore(b);
    const spy1 = vi.fn();
    const spy2 = vi.fn();
    s.subscribe(spy1);
    s.subscribe(spy2);
    await s.refresh();
    // 两者都应收到状态变化
    expect(spy1).toHaveBeenCalled();
    expect(spy2).toHaveBeenCalled();
  });

  it('订阅者异常不影响其他订阅者', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_list',
        result: { plugins: makePlugins(1) },
      },
    ]);
    const s = new PluginManagerStore(b);
    const spy1 = vi.fn(() => {
      throw new Error('subscriber error');
    });
    const spy2 = vi.fn();
    s.subscribe(spy1);
    s.subscribe(spy2);
    await s.refresh();
    expect(spy2).toHaveBeenCalled();
  });

  // ── disabledBySafemode 标记 ───────────────────────────────────────────

  it('disabledBySafemode 标记正确传递', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_list',
        result: {
          plugins: [
            {
              id: 'test.safemode',
              name: 'Safemode Plugin',
              version: '1.0.0',
              type: 'js',
              state: 'disabled',
              disabled_by_safemode: true,
            },
          ],
        },
      },
    ]);
    const s = new PluginManagerStore(b);
    const result = await s.refresh();
    expect(result[0]?.disabledBySafemode).toBe(true);
    expect(result[0]?.state).toBe('disabled');
  });

  it('tryEnable 操作传递 enable（D15 无 trial_enable 操作名，safemode 由宿主状态机判定）', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_admin',
        args: { op: { op: 'enable', id: 'test.safemode' } },
        result: {},
      },
    ]);
    const s = new PluginManagerStore(b);
    const result = await s.tryEnable('test.safemode');
    expect(result).toEqual({ ok: true });
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 设计令牌测试
// ──────────────────────────────────────────────────────────────────────────

describe('designTokens', () => {
  it('包含所有必需的颜色令牌', () => {
    expect(designTokens.colorPrimary).toBeDefined();
    expect(designTokens.colorDanger).toBeDefined();
    expect(designTokens.colorSuccess).toBeDefined();
    expect(designTokens.colorWarning).toBeDefined();
    expect(designTokens.colorText).toBeDefined();
    expect(designTokens.colorBackground).toBeDefined();
  });

  it('包含所有间距令牌', () => {
    expect(designTokens.spacingXs).toBeDefined();
    expect(designTokens.spacingSm).toBeDefined();
    expect(designTokens.spacingMd).toBeDefined();
    expect(designTokens.spacingLg).toBeDefined();
    expect(designTokens.spacingXl).toBeDefined();
  });

  it('包含所有字体令牌', () => {
    expect(designTokens.fontSizeXs).toBeDefined();
    expect(designTokens.fontSizeSm).toBeDefined();
    expect(designTokens.fontSizeMd).toBeDefined();
    expect(designTokens.fontSizeLg).toBeDefined();
    expect(designTokens.fontSizeXl).toBeDefined();
  });

  it('包含所有圆角令牌', () => {
    expect(designTokens.borderRadiusSm).toBeDefined();
    expect(designTokens.borderRadiusMd).toBeDefined();
    expect(designTokens.borderRadiusLg).toBeDefined();
  });

  it('包含阴影令牌', () => {
    expect(designTokens.boxShadowSm).toBeDefined();
    expect(designTokens.boxShadowMd).toBeDefined();
    expect(designTokens.boxShadowLg).toBeDefined();
  });

  it('包含焦点令牌', () => {
    expect(designTokens.focusRingWidth).toBeDefined();
    expect(designTokens.focusRingOffset).toBeDefined();
    expect(designTokens.focusRingColor).toBeDefined();
  });

  it('包含安全模式令牌', () => {
    expect(designTokens.colorSafemode).toBeDefined();
    expect(designTokens.safemodeBannerHeight).toBeDefined();
  });

  it('toCssVariables 生成正确的 CSS 变量名', () => {
    const vars = toCssVariables();
    expect(vars).toHaveProperty('--oc-color-primary');
    expect(vars).toHaveProperty('--oc-spacing-md');
    expect(vars).toHaveProperty('--oc-font-size-lg');
    expect(vars).toHaveProperty('--oc-border-radius-sm');
  });

  it('toCssVariables 所有键都是 kebab-case', () => {
    const vars = toCssVariables();
    for (const key of Object.keys(vars)) {
      expect(key).toMatch(/^--oc-[a-z0-9-]+$/);
    }
  });

  it('toCssText 生成有效的 CSS', () => {
    const css = toCssText();
    expect(css).toContain(':root {');
    expect(css).toContain('--oc-color-primary:');
    expect(css).toContain('--oc-spacing-md:');
    expect(css).toContain('}');
  });

  it('toCssText 所有行以分号结尾', () => {
    const css = toCssText();
    const lines = css
      .split('\n')
      .filter((l) => l.trim() && !l.startsWith(':root') && l.trim() !== '}');
    for (const line of lines) {
      if (line.trim()) {
        expect(line.trimEnd()).toMatch(/;$/);
      }
    }
  });
});

describe('safemodeTokens', () => {
  it('包含安全模式相关的所有令牌', () => {
    expect(safemodeTokens.bannerBg).toBeDefined();
    expect(safemodeTokens.bannerText).toBeDefined();
    expect(safemodeTokens.badgeBg).toBeDefined();
    expect(safemodeTokens.badgeText).toBeDefined();
    expect(safemodeTokens.badgeBorder).toBeDefined();
    expect(safemodeTokens.actionBtnBg).toBeDefined();
    expect(safemodeTokens.actionBtnText).toBeDefined();
  });

  it('banner 使用 safemode 颜色', () => {
    expect(safemodeTokens.bannerBg).toBe(designTokens.colorSafemode);
    expect(safemodeTokens.actionBtnText).toBe(designTokens.colorSafemode);
  });

  it('banner 文字为白色', () => {
    expect(safemodeTokens.bannerText).toBe('#ffffff');
  });

  it('badge 文字为白色', () => {
    expect(safemodeTokens.badgeText).toBe('#ffffff');
  });

  it('actionBtn 背景为白色', () => {
    expect(safemodeTokens.actionBtnBg).toBe('#ffffff');
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 端到端集成测试
// ──────────────────────────────────────────────────────────────────────────

describe('PluginManagerStore 集成场景', () => {
  it('完整流程：refresh → select → enable → refresh', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_list',
        result: { plugins: makePlugins(2) },
      },
      {
        cmd: 'host_registry_admin',
        args: { op: { op: 'enable', id: 'test.plugin0' } },
        result: {},
      },
      {
        cmd: 'host_registry_list',
        result: { plugins: makePlugins(2) },
      },
    ]);
    const s = new PluginManagerStore(b);

    // 1. 刷新列表
    const plugins = await s.refresh();
    expect(plugins).toHaveLength(2);

    // 2. 选中插件
    s.select('test.plugin0');
    expect(s.selectedId).toBe('test.plugin0');

    // 3. 启用插件
    const result = await s.enable('test.plugin0');
    expect(result).toEqual({ ok: true });

    // 4. 再次刷新
    const refreshed = await s.refresh();
    expect(refreshed).toHaveLength(2);
  });

  it('完整流程：safemode 场景', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_list',
        result: {
          plugins: [
            {
              id: 'test.safemode',
              name: 'Safemode Plugin',
              version: '1.0.0',
              type: 'js',
              state: 'disabled',
              disabled_by_safemode: true,
            },
            {
              id: 'test.normal',
              name: 'Normal Plugin',
              version: '1.0.0',
              type: 'js',
              state: 'running',
              disabled_by_safemode: false,
            },
          ],
        },
      },
      {
        cmd: 'host_registry_admin',
        args: { op: { op: 'enable', id: 'test.safemode' } },
        result: {},
      },
    ]);
    const s = new PluginManagerStore(b);

    // 1. 刷新列表
    const plugins = await s.refresh();
    expect(plugins).toHaveLength(2);

    // 2. 找到 safemode 禁用的插件
    const safemodePlugin = plugins.find((p) => p.disabledBySafemode);
    expect(safemodePlugin).toBeDefined();
    expect(safemodePlugin?.state).toBe('disabled');

    // 3. 尝试启用
    const result = await s.tryEnable('test.safemode');
    expect(result).toEqual({ ok: true });
  });

  it('多个订阅者在完整流程中保持一致', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_list',
        result: { plugins: makePlugins(1) },
      },
    ]);
    const s = new PluginManagerStore(b);
    const snapshots1: unknown[] = [];
    const snapshots2: unknown[] = [];

    s.subscribe((snap) => snapshots1.push(snap));
    s.subscribe((snap) => snapshots2.push(snap));

    await s.refresh();
    s.select('test.plugin0');

    expect(snapshots1.length).toBe(snapshots2.length);
    expect(snapshots1).toEqual(snapshots2);
  });

  it('错误处理：refresh 失败后仍可操作', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_list',
        error: new Error('network error'),
      },
    ]);
    const s = new PluginManagerStore(b);

    // 1. 刷新失败
    const result = await s.refresh();
    expect(result).toEqual([]);
    expect(s.state.status).toBe('error');

    // 2. 选中仍可工作
    s.select('test.plugin0');
    expect(s.selectedId).toBe('test.plugin0');
  });

  it('状态转换序列正确', async () => {
    const b = makeBackend([
      {
        cmd: 'host_registry_list',
        result: { plugins: makePlugins(1) },
      },
    ]);
    const s = new PluginManagerStore(b);
    const statuses: string[] = [];

    s.subscribe((snap) => {
      statuses.push(snap.state.status);
    });

    await s.refresh();

    // 应包含：idle → loading → success
    expect(statuses[0]).toBe('idle'); // 初始订阅
    expect(statuses).toContain('loading');
    expect(statuses).toContain('success');
    expect(statuses[statuses.length - 1]).toBe('success');
  });
});
