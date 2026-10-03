// ──────────────────────────────────────────────────────────────────────────
// 跨语言契约门禁：断言 core 与 Rust 侧 `tauron-host` 保持同构。
//
// 这些检查读真实源码，因此任何一侧改了协议而忘了同步另一侧都会在这里失败。
// ──────────────────────────────────────────────────────────────────────────
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { basename, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { HOST_ERROR_CODES, HOST_RETRY_CLASS, RETRYABLE_HOST_ERROR_CODES } from './errors.js';
import { CAPABILITIES } from './capabilities.js';
import { CHANNEL_KINDS, MAX_QUEUE, OVERFLOW_STREAK_LIMIT } from './channels.js';
import { GRANT_SET_SCHEMA_VERSION, IDENTITY_LABEL_PREFIX, RISKS } from './grants.js';

import { describe, expect, it } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
// packages/tauron-host/src → packages/tauron-host → packages → workspace root
const workspaceRoot = resolve(here, '..', '..', '..');
const read = (rel: string): string => readFileSync(resolve(workspaceRoot, rel), 'utf8');

function walk(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const full = resolve(dir, name);
    if (statSync(full).isDirectory()) out.push(...walk(full));
    else if (name.endsWith('.ts')) out.push(full);
  }
  return out;
}

const SRC_FILES = walk(resolve(here));

/** 匹配真正的 import 语句（静态/动态），不匹配注释与文档字符串。 */
const TAIL_IMPORT = /from\s+['"]@tauri-apps\/api|import\s*\(\s*['"]@tauri-apps\/api/;

describe('门禁 §8-1：唯一 @tauri-apps/api 引用点', () => {
  it('整个 src/ 只有 tauri-backend.ts 导入 @tauri-apps/api', () => {
    const offenders = SRC_FILES.filter((f) => TAIL_IMPORT.test(readFileSync(f, 'utf8'))).map((f) =>
      basename(f),
    );
    expect(offenders).toEqual(['tauri-backend.ts']);
  });

  it('MockBackend 不依赖任何真实 IPC', () => {
    expect(read('packages/tauron-host/src/backend.ts')).not.toMatch(TAIL_IMPORT);
  });
});

describe('门禁：错误码线与 Rust ErrorCode 完全一致', () => {
  const src = read('crates/tauron-host/src/error.rs');
  const block = src.slice(
    src.indexOf('pub enum ErrorCode'),
    src.indexOf('\n}', src.indexOf('pub enum ErrorCode')) + 1,
  );
  const rustCodes = [...block.matchAll(/^\s*(E_[A-Z_]+)\s*,\s*$/gm)].map((m) => m[1]!);

  it('枚举变体数量一致', () => {
    expect(rustCodes.length).toBe(HOST_ERROR_CODES.length);
  });

  it('逐个线名一致（按名集合，不按声明顺序）', () => {
    // 线上承载的是**码名**（`ErrorCode` 的 serde 表示就是变体名字符串），声明顺序
    // 不进入任何一端的分支。按序比对会把「在中间插入一个码」判成协议变更，让人
    // 只能把新码堆到末尾——那是把测试的实现细节当成协议约束（V4 A69 取消的正是这个耦合）。
    // 集合相等仍然钉死「一个都不多、一个都不少」：换名/漏名/多名都会红。
    expect(new Set(HOST_ERROR_CODES).size, 'TS 码表有重复项').toBe(HOST_ERROR_CODES.length);
    expect(new Set(rustCodes).size, 'Rust 码表有重复项').toBe(rustCodes.length);
    const missing = rustCodes.filter((c) => !HOST_ERROR_CODES.includes(c as never));
    const extra = [...HOST_ERROR_CODES].filter((c) => !rustCodes.includes(c as string));
    expect(missing, 'TS 侧缺码').toEqual([]);
    expect(extra, 'TS 侧多码').toEqual([]);
  });

  it('V4 RetryClass 与 Rust retry_class() 一致，自动重试集合默认为空', () => {
    const start = src.indexOf('pub const fn retry_class');
    const end = src.indexOf('/// Legacy wire compatibility', start);
    const retryBlock = src.slice(start, end);
    expect(start, 'Rust retry_class() missing').toBeGreaterThan(-1);

    const explicit = new Map<string, string>();
    for (const m of retryBlock.matchAll(/Self::(E_[A-Z_]+)\s*=>\s*RetryClass::(\w+)/g)) {
      explicit.set(m[1]!, m[2]!.replace(/([a-z0-9])([A-Z])/g, '$1-$2').toLowerCase());
    }
    for (const code of rustCodes) {
      expect(HOST_RETRY_CLASS[code as keyof typeof HOST_RETRY_CLASS], code).toBe(
        explicit.get(code) ?? 'never',
      );
    }
    expect(RETRYABLE_HOST_ERROR_CODES).toEqual([]);
  });
});

describe('门禁：命令面与 Rust 授权表完全一致', () => {
  const src = read('crates/tauron-host/src/authz.rs');
  const beforeTests = src.slice(0, src.indexOf('mod tests'));
  const adapter = read('crates/tauron-adapter/src/lib.rs');
  const optionalAuth = adapter.slice(
    adapter.indexOf('pub const PLUGIN_INSTALL_AUTH'),
    adapter.indexOf('/// 命令状态'),
  );
  const optionalCommands = [...optionalAuth.matchAll(/command:\s*"(host_[^"]+)"/g)].map(
    (m) => m[1]!,
  );
  const rustAllCommands = [
    ...new Set(
      [...beforeTests.matchAll(/command:\s*"(host_[^"]+)"/g)]
        .map((m) => m[1]!)
        .concat(optionalCommands),
    ),
  ];

  it('命令数量一致', () => {
    expect(rustAllCommands.length).toBe(CAPABILITIES.length);
  });

  it('命令名逐个一致（含主窗特权命令）', () => {
    expect([...CAPABILITIES.map((c) => c.command)].sort()).toEqual([...rustAllCommands].sort());
  });

  it('禁止的 v1 命令两侧都不存在', () => {
    for (const banned of ['host_grant_request', 'host_call_begin']) {
      expect(CAPABILITIES.map((c) => c.command)).not.toContain(banned);
      expect(beforeTests).not.toContain(`command: "${banned}"`);
    }
  });
});

describe('门禁：通道语义与 Rust eventbus 一致', () => {
  const src = read('crates/tauron-host/src/eventbus.rs');
  const enumBlock = src.slice(
    src.indexOf('pub enum ChannelKind'),
    src.indexOf('\n}', src.indexOf('pub enum ChannelKind')) + 1,
  );
  const rustKinds = [...enumBlock.matchAll(/^\s*(Event|Request|State)\b/gm)].map((m) => m[1]!);
  // Rust 侧标了 `#[serde(rename_all = "kebab-case")]`。
  const rustWired = rustKinds.map((k) => k.charAt(0).toLowerCase() + k.slice(1));

  it('通道种类与线名一致', () => {
    expect([...CHANNEL_KINDS]).toEqual(rustWired);
  });

  it('队列上限一致', () => {
    const n = Number(src.match(/pub const MAX_QUEUE:\s*usize\s*=\s*(\d+)/)?.[1]);
    expect(MAX_QUEUE).toBe(n);
  });

  it('熔断触发阈值一致', () => {
    const n = Number(src.match(/pub const OVERFLOW_STREAK_LIMIT:\s*usize\s*=\s*(\d+)/)?.[1]);
    expect(OVERFLOW_STREAK_LIMIT).toBe(n);
  });
});

describe('门禁：授予集契约与 Rust tauron-acl 一致', () => {
  const schema = read('crates/tauron-acl/src/grant.rs');
  const manifest = read('crates/tauron-host/src/manifest.rs');
  const authz = read('crates/tauron-host/src/authz.rs');

  it('授予集 schema 版本一致', () => {
    const n = Number(schema.match(/pub const GRANT_SET_SCHEMA_VERSION:\s*u32\s*=\s*(\d+)/)?.[1]);
    expect(GRANT_SET_SCHEMA_VERSION).toBe(n);
  });

  it('风险档位线名一致', () => {
    const enumBlock = manifest.slice(
      manifest.indexOf('pub enum Risk'),
      manifest.indexOf('\n}', manifest.indexOf('pub enum Risk')) + 1,
    );
    const rust = [...enumBlock.matchAll(/^\s*(Low|Elevated|High)\b/gm)].map((m) => m[1]!);
    // Rust 侧标了 `#[serde(rename_all = "lowercase")]`。
    expect([...RISKS]).toEqual(rust.map((r) => r.toLowerCase()));
  });

  it('插件身份 label 前缀一致', () => {
    const rust = authz.match(/pub const IDENTITY_LABEL_PREFIX:\s*&str\s*=\s*"([^"]+)"/)?.[1];
    expect(IDENTITY_LABEL_PREFIX).toBe(rust);
    // R4：`tauri-backend.ts` 另有一份拷贝（`pluginIdFromLabel` / `principalFromLabel`
    // 用它解析主体）。三处前缀必须一致，否则身份解析会在边界上静默错位。
    const backendPrefix = read('packages/tauron-host/src/tauri-backend.ts').match(
      /export const IDENTITY_LABEL_PREFIX = '([^']+)'/,
    )?.[1];
    expect(backendPrefix, 'tauri-backend.ts 未声明 IDENTITY_LABEL_PREFIX').toBeDefined();
    expect(backendPrefix).toBe(rust);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 第四轮新增门禁（逻辑完整性审查 D24–D28 / D13–D16 / D1）
// ──────────────────────────────────────────────────────────────────────────

describe('门禁 §8-16：失效路径终止性（D24–D28）', () => {
  const lifecycle = read('crates/tauron-host/src/lifecycle.rs');
  const recovery = read('crates/tauron-recovery/src/lib.rs');

  it('状态机含熔断阈值常量（MAX_RETRY）', () => {
    const match = lifecycle.match(/pub const MAX_RETRY:\s*usize\s*=\s*(\d+)/);
    expect(match).not.toBeNull();
    expect(Number(match![1])).toBeGreaterThan(0);
  });

  it('恢复模块含启动失败计数阈值（SAFEMODE_THRESHOLD）', () => {
    const match = recovery.match(/const SAFEMODE_THRESHOLD:\s*u32\s*=\s*(\d+)/);
    expect(match).not.toBeNull();
    expect(Number(match![1])).toBeGreaterThanOrEqual(2);
  });

  it('恢复模块含修复模式终态（D25：堵住永久 boot loop）', () => {
    expect(recovery).toContain('Repairmode');
    expect(recovery).toContain('Safemode');
  });

  it('状态机 ERRORED 含 retryable 与 user-confirm 两档（D24）', () => {
    expect(lifecycle).toContain('retryable');
    expect(lifecycle).toContain('user-confirm');
  });

  it('状态机含 INSTALL_FAILED 准终态（D3）', () => {
    expect(lifecycle).toContain('INSTALL_FAILED');
  });
});

describe('门禁 §8-17：命令面消费方登记（D13–D16）', () => {
  const authz = read('crates/tauron-host/src/authz.rs');
  const tauriBackend = read('packages/tauron-host/src/tauri-backend.ts');

  /**
   * 解析 `authz.rs` 的 `CommandAuth { … }` 条目（容忍 rustfmt 折行）。
   *
   * 旧写法是 `/command:\s*"([^"]+)"[\s\S]*?tier:\s*"self"/`——**永远匹配不到**：
   * `tier` 是 `AuthTier::Self_` 而不是字符串 `"self"`。于是集合恒为空、循环空转，
   * 这条门禁长期是**假绿**（本仓反复出现的「解析到 0 条却不失败」）。
   */
  function commandAuths(): Array<{ command: string; tier: string; consumer: string }> {
    const out: Array<{ command: string; tier: string; consumer: string }> = [];
    for (const m of authz.matchAll(/CommandAuth\s*\{([\s\S]*?)\n\s*\},/g)) {
      const body = m[1]!;
      const command = /command:\s*"([^"]+)"/.exec(body)?.[1];
      const tier = /tier:\s*AuthTier::(\w+)/.exec(body)?.[1];
      const consumer = /consumer:\s*"([^"]*)"/.exec(body)?.[1];
      if (command !== undefined && tier !== undefined && consumer !== undefined) {
        out.push({ command, tier, consumer });
      }
    }
    return out;
  }

  it('authz 命令登记解析非空，且三档齐全（防解析失配的假绿）', () => {
    const all = commandAuths();
    expect(all.length, 'authz.rs 的 CommandAuth 解析到 0 条（门禁定位失败）').toBeGreaterThan(20);
    for (const tier of ['Self_', 'ScopedRead', 'Privileged']) {
      expect(
        all.filter((c) => c.tier === tier).length,
        `authz 里 ${tier} 档解析到 0 条（分档断言会空转）`,
      ).toBeGreaterThan(0);
    }
  });

  it('每条登记命令都写明了消费方（consumer 非空）', () => {
    const empty = commandAuths().filter((c) => c.consumer.trim().length === 0);
    expect(
      empty.map((c) => c.command),
      '以下命令未登记消费方（孤儿命令面）',
    ).toEqual([]);
  });

  it('所有 self 档命令在 tauri-backend 中有 invoke 入口', () => {
    const selfCommands = commandAuths().filter((c) => c.tier === 'Self_');
    expect(selfCommands.length, 'authz 里 self 档命令解析到 0 条').toBeGreaterThan(0);
    for (const { command } of selfCommands) {
      expect(tauriBackend, `self 档命令 ${command} 在 tauri-backend.ts 无入口`).toContain(command);
    }
  });

  it('self 档的 contributes 注册入口在插件侧客户端（主窗调不了 self 档）', () => {
    // 断链回归：方法曾挂在主窗 ShellClient 上（self 档命令挂错客户端 =
    // 要么永不成功、要么绕过 label 绑定）。它必须在 HostClient 上。
    const host = read('packages/tauron-host/src/host.ts');
    expect(host).toMatch(/async contributesRegister\(/);
    const shell = read('packages/tauron-host/src/shell-client.ts');
    expect(shell).not.toMatch(/async contributesRegister\(/);
  });

  it('所有 privileged 档命令都能在 TS 侧被调到（tauri-backend + 客户端方法）', () => {
    // 旧断言是**同义反复**：从 authz 里取出命令名，再断言 authz 里含这个命令名——
    // 恒真，等于没检查。现在断言它们真的存在 TS 调用点（否则就是「有登记、无入口」）。
    const priv = commandAuths().filter((c) => c.tier === 'Privileged');
    expect(priv.length, 'authz 里 privileged 档命令解析到 0 条').toBeGreaterThan(0);
    const clients =
      read('packages/tauron-host/src/shell-client.ts') + read('packages/tauron-host/src/host.ts');
    for (const { command } of priv) {
      expect(tauriBackend, `privileged 命令 ${command} 在 tauri-backend.ts 无入口`).toContain(
        command,
      );
      expect(clients, `privileged 命令 ${command} 在任何主窗客户端都没有调用点`).toContain(command);
    }
  });

  it('禁止授予清单非空（D3/ADR-17）', () => {
    const match = authz.match(/pub const FORBIDDEN_PLAIN[^;]*;/);
    expect(match).not.toBeNull();
    expect(match![0].length).toBeGreaterThan(40);
  });
});

describe('门禁 §8-18：事件发布唯一入口（D1/ADR-19）', () => {
  const events = read('packages/tauron-host/src/events.ts');
  const host = read('packages/tauron-host/src/host.ts');
  const authz = read('crates/tauron-host/src/authz.rs');
  const capabilities = read('packages/tauron-host/src/capabilities.ts');

  it('events.ts 声明 host_events_publish 为唯一发布入口', () => {
    expect(events).toContain('host_events_publish');
    expect(events).toContain('唯一通道');
  });

  it('host.ts 的事件发布方法调用 host_events_publish', () => {
    expect(host).toContain('host_events_publish');
  });

  it('authz.rs 中 host_events_publish 为 self 档', () => {
    const block = authz.slice(
      authz.indexOf('host_events_publish'),
      authz.indexOf('host_events_publish') + 200,
    );
    expect(block).toContain('Self_');
  });

  it('capabilities.ts 中 host_events_publish 为 self 档', () => {
    expect(capabilities).toContain('host_events_publish');
    const block = capabilities.slice(
      capabilities.indexOf('host_events_publish'),
      capabilities.indexOf('host_events_publish') + 200,
    );
    expect(block).toContain('self');
  });
});

describe('门禁 §8-19：配置化插件选择加载（PluginFilter）', () => {
  const registry = read('crates/tauron-host/src/registry.rs');
  const error = read('crates/tauron-host/src/error.rs');
  const lib = read('crates/tauron-host/src/lib.rs');

  it('PluginFilter 结构体存在且含 allow/deny/types/platforms/include_builtins 五字段', () => {
    expect(registry).toContain('pub struct PluginFilter');
    expect(registry).toContain('pub allow: Vec<String>');
    expect(registry).toContain('pub deny: Vec<String>');
    expect(registry).toContain('pub types: Vec<String>');
    expect(registry).toContain('pub platforms: Vec<String>');
    expect(registry).toContain('pub include_builtins: bool');
  });

  it('PluginFilter 实现了 matches() 方法（安装期过滤）', () => {
    expect(registry).toContain('pub fn matches(&self, manifest: &PluginManifest)');
  });

  it('RegistryConfig 含 plugin_filter 可选字段', () => {
    expect(registry).toContain('pub plugin_filter: Option<PluginFilter>');
  });

  it('install() 方法在容量检查之前执行过滤器检查', () => {
    // 过滤器检查必须在 entries 读取之前
    const installIdx = registry.indexOf('pub fn install(');
    const filterCheckIdx = registry.indexOf('if let Some(ref filter)');
    const entriesIdx = registry.indexOf('self.entries.read()', installIdx);
    expect(filterCheckIdx).toBeGreaterThan(installIdx);
    expect(entriesIdx).toBeGreaterThan(filterCheckIdx);
  });

  it('E_PLUGIN_FILTERED 错误码存在且属于 manual retry', () => {
    expect(error).toContain('E_PLUGIN_FILTERED');
    const retryClassBlock = error.slice(
      error.indexOf('pub const fn retry_class'),
      error.indexOf('/// Legacy wire compatibility'),
    );
    expect(retryClassBlock).toContain('Self::E_PLUGIN_FILTERED => RetryClass::Manual');
  });

  it('PluginFilter 从 lib.rs 导出', () => {
    expect(lib).toContain('PluginFilter');
  });

  it('default_config() 含 plugin_filter: None', () => {
    expect(registry).toContain('plugin_filter: None');
  });
});

describe('门禁 §8-20：客户端配置聚合（ClientConfig）', () => {
  const config = read('crates/tauron-host/src/config.rs');
  const lib = read('crates/tauron-host/src/lib.rs');
  const tsConfig = read('packages/tauron-host/src/client-config.ts');

  it('ClientConfig 结构体存在且含 registry/log_level/auto_update 等字段', () => {
    expect(config).toContain('pub struct ClientConfig');
    expect(config).toContain('pub registry: Option<RegistryConfigOverride>');
    expect(config).toContain('pub log_level: Option<String>');
    expect(config).toContain('pub auto_update: Option<bool>');
  });

  it('ClientConfig 实现了 from_json/from_file/to_json/validate', () => {
    expect(config).toContain('pub fn from_json');
    expect(config).toContain('pub fn from_file');
    expect(config).toContain('pub fn to_json');
    expect(config).toContain('pub fn validate');
  });

  it('ClientConfig 实现了 registry_config() 合并缺省值', () => {
    expect(config).toContain('pub fn registry_config');
  });

  it('ClientConfig 从 lib.rs 导出', () => {
    expect(lib).toContain('ClientConfig');
    expect(lib).toContain('RegistryConfigOverride');
  });

  it('TS 侧 ClientConfig 类型与 Rust 镜像（字段一致）', () => {
    expect(tsConfig).toContain('export interface ClientConfig');
    expect(tsConfig).toContain('registry?: RegistryConfigOverride');
    expect(tsConfig).toContain('log_level?:');
    expect(tsConfig).toContain('auto_update?: boolean');
    expect(tsConfig).toContain('plugin_filter?: PluginFilter');
  });

  it('TS 侧 validateClientConfig 纯函数校验', () => {
    expect(tsConfig).toContain('export function validateClientConfig');
  });

  it('TS 侧提供三个预置配置模板', () => {
    expect(tsConfig).toContain('export function fullLoadConfig');
    expect(tsConfig).toContain('export function minimalConfig');
    expect(tsConfig).toContain('export function templateConfig');
  });
});
