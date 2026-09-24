import { describe, expect, it } from 'vitest';

import { MockBackend } from './backend.js';
import {
  CAPABILITIES,
  capabilityMatrix,
  capabilityOf,
  isAvailable,
} from './capabilities.js';

describe('CAPABILITIES（计划 §2.1 命令面镜像）', () => {
  it('共 16 条：13 条插件命令 + 3 条主窗特权命令', () => {
    expect(CAPABILITIES).toHaveLength(16);
  });

  it('插件命令 13 条，其中 scoped-read 恰好 1 条（host_registry_list）', () => {
    const plugin = CAPABILITIES.filter((c) => c.consumer === 'plugin');
    expect(plugin).toHaveLength(13);
    expect(plugin.filter((c) => c.tier === 'scoped-read')).toEqual([
      expect.objectContaining({ command: 'host_registry_list' }),
    ]);
    // 审计补登记：插件侧的流式三连 + 事件取件泵此前**没有任何档位**，
    // 而 `HostClient` 已在调用它们（脚手架的能力白名单也因此拒绝这 4 个名字）。
    expect(plugin.map((c) => c.command)).toEqual(
      expect.arrayContaining([
        'host_events_drain',
        'host_stream_open',
        'host_stream_write',
        'host_stream_close',
      ]),
    );
  });

  it('privileged 命令集合 = 注册表管理 + P0-2 进程运行时，消费方均为主窗', () => {
    const priv = CAPABILITIES.filter((c) => c.tier === 'privileged');
    // P0-2 起 privileged 不再是「唯一一条」：`host_runtime_spawn` 能按入参
    // pluginId 启动可执行文件，与注册表管理同级；集合本身仍是**闭集**，
    // 任何新增特权命令都必须显式改这里（增量可见）。
    expect(priv.map((c) => c.command).sort()).toEqual([
      'host_registry_admin',
      'host_runtime_health',
      'host_runtime_spawn',
    ]);
    expect(
      priv.every((c) => c.consumer === 'main-window'),
      '特权命令的消费方必须是主窗（下放给 plugin 即越权）',
    ).toBe(true);
  });

  it('D16：host_grant_request 已从 v1 删除', () => {
    expect(CAPABILITIES.map((c) => c.command)).not.toContain('host_grant_request');
  });

  it('D2：host_call_begin 已被 host_plugin_call 取代', () => {
    expect(CAPABILITIES.map((c) => c.command)).not.toContain('host_call_begin');
    expect(CAPABILITIES.map((c) => c.command)).toContain('host_plugin_call');
  });

  it('每条命令都有 tier / consumer / description（审批 UI 文案来源，§4.5）', () => {
    for (const c of CAPABILITIES) {
      expect(['self', 'scoped-read', 'privileged']).toContain(c.tier);
      expect(['plugin', 'main-window']).toContain(c.consumer);
      expect(c.description.length).toBeGreaterThan(5);
    }
  });

  it('无重复命令名', () => {
    const names = CAPABILITIES.map((c) => c.command);
    expect(new Set(names).size).toBe(names.length);
  });
});

describe('isAvailable / capabilityMatrix', () => {
  it('宿主未注册该命令 → false', () => {
    const backend = new MockBackend({ capabilities: ['host_plugin_call'] });
    expect(isAvailable(backend, 'host_plugin_call')).toBe(true);
    expect(isAvailable(backend, 'host_registry_admin')).toBe(false);
  });

  it('未知命令名一律 false（不因为恰好可用而放行）', () => {
    const backend = new MockBackend({ capabilities: ['totally-unknown'] });
    expect(isAvailable(backend, 'totally-unknown')).toBe(false);
  });

  it('capabilityMatrix 覆盖全部 16 条命令', () => {
    const backend = new MockBackend({
      capabilities: CAPABILITIES.map((c) => c.command),
    });
    const matrix = capabilityMatrix(backend);
    expect(Object.keys(matrix)).toHaveLength(16);
    expect(Object.values(matrix).every(Boolean)).toBe(true);
  });

  it('插件 webview 视图看不到主窗特权命令（含 P0-2 进程运行时）', () => {
    // 模拟：插件 webview 只注册了 13 条插件命令。
    const pluginCaps = CAPABILITIES.filter((c) => c.consumer === 'plugin').map((c) => c.command);
    const backend = new MockBackend({ capabilities: pluginCaps, pluginId: 'com.example.x' });
    const matrix = capabilityMatrix(backend);
    expect(matrix['host_registry_admin']).toBe(false);
    // 进程执行原语：插件侧必须不可见（可见 = 任何插件都能起别人的 sidecar）。
    expect(matrix['host_runtime_spawn']).toBe(false);
    expect(matrix['host_runtime_health']).toBe(false);
    expect(Object.values(matrix).filter(Boolean)).toHaveLength(13);
  });

  it('capabilityOf 查无则 undefined', () => {
    expect(capabilityOf('host_plugin_call')?.tier).toBe('self');
    expect(capabilityOf('nope')).toBeUndefined();
  });
});
