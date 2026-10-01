// §8.1 R1-4：Capability 协商必须 fail-closed。
//
// 静态 FRAMEWORK_COMMANDS 只是壳声明全集（wire-gate 锁宏↔清单一致性），
// 不再被乐观塞进 capabilities()；真相只能来自 host_capabilities 运行期返回。
import { describe, expect, it } from 'vitest';

import { BOOTSTRAP_COMMANDS, TauriBackend } from './tauri-backend.js';
import { isAvailable } from './capabilities.js';
import { ShellClient } from './shell-client.js';

describe('TauriBackend 能力协商（fail-closed，R1-4）', () => {
  it('构造后仅 bootstrap 命令可用，其余一律 unknown = unsupported', () => {
    const backend = new TauriBackend({ commandPrefix: '' });
    expect(backend.capabilitiesNegotiated).toBe(false);
    expect([...backend.capabilities()]).toEqual([...BOOTSTRAP_COMMANDS]);

    const shell = new ShellClient({ backend });
    expect(shell.supports('host_capabilities')).toBe(true);
    expect(shell.supports('host_window_minimize')).toBe(false);
    expect(shell.supports('host_registry_install')).toBe(false);
    // 能力矩阵在协商前不得报可用（CAPABILITIES 镜像里没有 bootstrap 命令，
    // 协商前对所有镜像命令都是 false——这正是 unknown = unsupported）。
    expect(isAvailable(backend, 'host_plugin_call')).toBe(false);
  });

  it('adoptCapabilities 采纳运行期真相后才报可用', () => {
    const backend = new TauriBackend({ commandPrefix: '' });
    backend.adoptCapabilities(['host_capabilities', 'host_plugin_call', 'host_registry_install']);
    expect(backend.capabilitiesNegotiated).toBe(true);
    const shell = new ShellClient({ backend });
    expect(shell.supports('host_plugin_call')).toBe(true);
    expect(shell.supports('host_registry_install')).toBe(true);
    expect(shell.supports('host_window_minimize'), '宿主没报的命令不得凭空变有').toBe(false);
    expect(isAvailable(backend, 'host_plugin_call')).toBe(true);
  });

  it('空集合与缺 host_capabilities 的集合都拒绝采纳且不改现状', () => {
    const backend = new TauriBackend({ commandPrefix: '' });
    expect(() => backend.adoptCapabilities([])).toThrow(TypeError);
    expect(() => backend.adoptCapabilities(['host_notify'])).toThrow(/host_capabilities/);
    expect(backend.capabilitiesNegotiated).toBe(false);
    expect([...backend.capabilities()]).toEqual([...BOOTSTRAP_COMMANDS]);
  });
});
