import { describe, expect, it } from 'vitest';

import { MockBackend } from './backend.js';
import { CAPABILITIES, capabilityMatrix, capabilityOf, isAvailable } from './capabilities.js';

describe('CAPABILITIES（计划 §2.1 命令面镜像）', () => {
  it('共 32 条：20 条插件命令 + 12 条主窗特权命令', () => {
    expect(CAPABILITIES).toHaveLength(32);
  });

  it('插件命令 20 条，其中 scoped-read 恰好 2 条（host_registry_list / host_contributes_list）', () => {
    const plugin = CAPABILITIES.filter((c) => c.consumer === 'plugin');
    expect(plugin).toHaveLength(20);
    expect(plugin.filter((c) => c.tier === 'scoped-read')).toEqual([
      expect.objectContaining({ command: 'host_registry_list' }),
      expect.objectContaining({ command: 'host_contributes_list' }),
    ]);
    // 审计补登记：插件侧流式 open/write/grant/close + 事件取件泵都必须有档位，
    // 否则 HostClient 能调用但能力白名单/授权表不承认，形成半通链路。
    // `host_capabilities` 同理——能力协商的入口必须能在矩阵里为自己的成功作证。
    expect(plugin.map((c) => c.command)).toEqual(
      expect.arrayContaining([
        'host_capabilities',
        'host_events_drain',
        'host_stream_open',
        'host_stream_write',
        'host_stream_grant',
        'host_stream_close',
      ]),
    );
  });

  it('privileged 命令集合 = 注册表管理 + P0-2 进程运行时 + 更新通道，消费方均为主窗', () => {
    const priv = CAPABILITIES.filter((c) => c.tier === 'privileged');
    // P0-2/M8 起 privileged 有管理、运行时和资源诊断命令；轮 40 起含更新通道
    // （market download/install 装配腿注入后为真）。集合仍是**闭集**，
    // 任何新增特权命令都必须显式改这里（增量可见）。
    expect(priv.map((c) => c.command).sort()).toEqual([
      'host_events_approvals',
      'host_events_approve',
      'host_events_revoke',
      'host_market_download',
      'host_market_install',
      'host_production_doctor',
      'host_registry_admin',
      'host_registry_install',
      'host_registry_install_preview',
      'host_resource_stats',
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

  it('capabilityMatrix 覆盖全部 32 条命令（主窗主体：全量可见）', () => {
    const backend = new MockBackend({
      capabilities: CAPABILITIES.map((c) => c.command),
    });
    const matrix = capabilityMatrix(backend);
    expect(Object.keys(matrix)).toHaveLength(32);
    expect(Object.values(matrix).every(Boolean)).toBe(true);
  });

  it('插件主体：宿主全量注册（32 条）也只看到 20 条插件命令', () => {
    // 非循环夹具：宿主 `host_capabilities` 返回的是**构建级**命令集（不带调用方
    // 参数），插件 webview 拿到的原始注册集就是全量 32 条——可见性必须由被测代码
    // 按主体过滤；测试自己先按 consumer 预筛会变成同义反复（过滤是测试做的）。
    const backend = new MockBackend({
      capabilities: CAPABILITIES.map((c) => c.command),
      pluginId: 'com.example.x',
    });
    const matrix = capabilityMatrix(backend);
    expect(isAvailable(backend, 'host_plugin_call')).toBe(true);
    expect(isAvailable(backend, 'host_registry_admin')).toBe(false);
    // 进程执行原语：插件侧必须不可见（可见 = 任何插件都能起别人的 sidecar）。
    expect(matrix['host_runtime_spawn']).toBe(false);
    expect(matrix['host_runtime_health']).toBe(false);
    expect(matrix['host_registry_install_preview']).toBe(false);
    expect(matrix['host_production_doctor']).toBe(false);
    expect(Object.values(matrix).filter(Boolean)).toHaveLength(20);
  });

  it('畸形主体（invalid label）不按主窗放行：特权命令仍不可见', () => {
    const backend = new MockBackend({
      capabilities: CAPABILITIES.map((c) => c.command),
      principal: { kind: 'invalid', label: 'plugin-' },
    });
    expect(isAvailable(backend, 'host_registry_admin')).toBe(false);
    expect(isAvailable(backend, 'host_plugin_call')).toBe(true);
  });

  it('capabilityOf 查无则 undefined', () => {
    expect(capabilityOf('host_plugin_call')?.tier).toBe('self');
    expect(capabilityOf('nope')).toBeUndefined();
  });
});
