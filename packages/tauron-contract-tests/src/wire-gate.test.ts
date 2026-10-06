/**
 * 跨语言线上协议门禁（TS ↔ Rust，设计文档 §2.1/§2.3/§3.2）
 *
 * 这里读取**真实 Rust 源码**并断言与 TS 常量同构，因此任何一侧改了协议
 * 而忘了同步另一侧都会在 CI 失败。
 *
 * 覆盖的断链风险：
 * 1. 命令名漂移 —— TS `TAURON_COMMANDS` vs Rust `#[tauri::command]` 函数名
 * 2. 字段命名漂移 —— Rust 必须是 camelCase（否则前端读到 `undefined`）
 * 3. 错误码漂移 —— TS `PluginErrorCode` vs Rust `PluginErrorCode`
 * 4. 命令核心入口存在性 —— `HostState::handle_*` 必须齐全
 */

import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { TAURON_COMMANDS } from '@tauron/core';
import {
  CAPABILITIES,
  HOST_ERROR_CODES,
  HOST_RETRY_CLASS,
  LIFECYCLE_EVENTS,
  LIFECYCLE_STATES,
  PLUGIN_REPORTABLE_EVENTS,
  RETRYABLE_HOST_ERROR_CODES,
} from '@tauron/host';
import { PluginErrorCode, RETRYABLE_ERROR_CODES, TRANSITIONS } from '@tauron/types';

const here = dirname(fileURLToPath(import.meta.url));
// packages/tauron-contract-tests/src → packages/tauron-contract-tests → packages → root
const workspaceRoot = resolve(here, '..', '..', '..');
const read = (rel: string): string => readFileSync(resolve(workspaceRoot, rel), 'utf8');

/**
 * `@tauron/host` 的 `src/` 文件名清单（含测试）。
 *
 * 用于「整包不得再 import X」这类**全目录**断言：只查几个已知文件会让
 * 新增文件绕过门禁。解析到 0 条即失败（防正则/路径失配的假绿）。
 */
function hostSrcFiles(): string[] {
  const dir = join(workspaceRoot, 'packages', 'tauron-host', 'src');
  const files = readdirSync(dir, { withFileTypes: true })
    .filter((e) => e.isFile() && e.name.endsWith('.ts'))
    .map((e) => e.name);
  expect(files.length, '@tauron/host/src 解析到 0 个 .ts 文件（门禁定位失败）').toBeGreaterThan(0);
  return files;
}

const SHELL = 'crates/tauron-shell/src';

describe('门禁：命令名 TS ↔ Rust 一致', () => {
  const commands = read(`${SHELL}/commands.rs`);

  it('三条命令都在 Rust 侧被 #[tauri::command] 导出', () => {
    const declared = [...commands.matchAll(/#\[tauri::command\]\s*pub fn (\w+)/g)].map(
      (m) => m[1]!,
    );
    expect(declared.sort()).toEqual(
      [TAURON_COMMANDS.invoke, TAURON_COMMANDS.cancel, TAURON_COMMANDS.emit].sort(),
    );
  });

  it('handler 宏注册的命令与 TAURON_COMMANDS 一致', () => {
    const macro = commands.slice(commands.indexOf('macro_rules! tauron_generate_handler'));
    const registered = [...macro.matchAll(/commands::(\w+)/g)].map((m) => m[1]!);
    expect(registered.sort()).toEqual(
      [TAURON_COMMANDS.invoke, TAURON_COMMANDS.cancel, TAURON_COMMANDS.emit].sort(),
    );
  });

  it('框架层插件标识为 tauron-shell（与应用层 tauron 区分，可叠加注册）', () => {
    // 两层插件同时注册时名字必须互不相同（同名的插件注册会被 Tauri 拒绝）。
    // 应用层标识 "tauron" 的门禁在下方 adapter describe 中。
    expect(commands).toMatch(/Builder(?:::<\w+>)?::new\("tauron-shell"\)/);
  });
});

describe('门禁：命令核心入口存在', () => {
  const dispatch = read(`${SHELL}/dispatch.rs`);

  it('HostState 暴露 invoke/cancel/emit 三个处理入口', () => {
    for (const method of ['handle_invoke', 'handle_cancel', 'handle_emit']) {
      expect(dispatch).toContain(`pub fn ${method}`);
    }
  });

  it('分发器 trait 与注册表状态机被接入主链路', () => {
    expect(dispatch).toContain('pub trait PluginDispatcher');
    expect(dispatch).toContain('PluginState::Enabled');
    expect(dispatch).toContain('PluginErrorCode::PluginNotFound');
  });

  it('事件 topic 前缀与 TS eventNamespace 一致', () => {
    expect(dispatch).toContain('EVENT_TOPIC_PREFIX: &str = "plugin:"');
    // Rust dispatch 以冒号切分 `<id>:<event>`；TS eventNamespace 必须产出同形
    expect(dispatch).toMatch(/split_once\('\:'\)|splitn\(2, ':'\)/);
    const types = read('packages/types/src/plugin.ts');
    expect(types).toContain('`plugin:${pluginId}:${eventName}`');
  });
});

describe('门禁：Rust 序列化必须为 camelCase', () => {
  interface TypeSpec {
    file: string;
    type: string;
  }

  const specs: TypeSpec[] = [
    { file: 'envelope.rs', type: 'PluginInvokeRequest' },
    { file: 'envelope.rs', type: 'PluginInvokeResponse' },
    { file: 'envelope.rs', type: 'PluginErrorBody' },
    { file: 'envelope.rs', type: 'ProgressEvent' },
    { file: 'envelope.rs', type: 'PluginCancelRequest' },
    { file: 'eventbus.rs', type: 'Event' },
    { file: 'acl.rs', type: 'PluginPermissionGrant' },
  ];

  for (const { file, type } of specs) {
    it(`${type} 声明了 rename_all = "camelCase"`, () => {
      const src = read(`${SHELL}/${file}`);
      // 取类型声明前的属性块
      const idx = src.search(new RegExp(`pub (struct|enum) ${type}\\b`));
      expect(idx, `${type} not found in ${file}`).toBeGreaterThan(-1);

      const attrs = src.slice(Math.max(0, idx - 400), idx);
      const lastDerive = attrs.lastIndexOf('derive');
      const attrBlock = attrs.slice(lastDerive);
      expect(attrBlock).toContain('rename_all = "camelCase"');
    });
  }
});

describe('门禁：进度通道（前端 Channel ↔ Rust 投递）', () => {
  const commands = read(`${SHELL}/commands.rs`);
  const envelope = read(`${SHELL}/envelope.rs`);
  const typesEnvelope = read('packages/types/src/envelope.ts');
  const backend = read('packages/tauron-core/src/tauri-backend.ts');
  const hostSrc = read('packages/tauron-host/src/host.ts');
  const adapter = read('crates/tauron-adapter/src/tauri.rs');

  /** TS interface 的字段名（顶层，忽略可选标记）。 */
  const tsFields = (src: string, name: string): string[] => {
    const m = src.match(new RegExp(`export interface ${name} \\{([\\s\\S]*?)\\n\\}`));
    if (!m) throw new Error(`${name} not found`);
    return [...m[1]!.matchAll(/^\s{2}(\w+)\??:/gm)].map((x) => x[1]!).sort();
  };

  /** Rust struct 的字段名（snake_case → camelCase）。 */
  const rustFields = (src: string, name: string): string[] => {
    const m = src.match(new RegExp(`pub struct ${name} \\{([\\s\\S]*?)\\n\\}`));
    if (!m) throw new Error(`${name} not found`);
    return [...m[1]!.matchAll(/^\s+pub (\w+):/gm)]
      .map((x) => x[1]!.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase()))
      .sort();
  };

  it('ProgressEvent 字段 TS ↔ Rust 逐字段一致', () => {
    // 既有门禁只校验了 rename_all，字段集漂移（少一个/多一个）看不见。
    expect(rustFields(envelope, 'ProgressEvent')).toEqual(tsFields(typesEnvelope, 'ProgressEvent'));
    expect(rustFields(envelope, 'ProgressEvent').length).toBeGreaterThan(0);
  });

  it('plugin_invoke 真的声明并向进度通道投递（不是收下就丢）', () => {
    const sig = commands.match(/pub fn plugin_invoke[\s\S]*?-> PluginInvokeResponse/)?.[0] ?? '';
    expect(sig, 'plugin_invoke 未找到').not.toBe('');
    // 必须可选：无进度调用不得被迫传通道（Option<Channel<T>> 无法编译，
    // 故用 JavaScriptChannelId + 注入 webview 转换）。
    expect(sig).toContain('Option<tauri::ipc::JavaScriptChannelId>');
    expect(sig).toContain('webview: tauri::Webview<R>');
    // 必须真的转换并投递帧——否则前端 onProgress 永不触发（死接线）。
    expect(commands).toContain('channel_on::<R, ProgressEvent>');
    expect(commands).toMatch(/ch\.send\(ProgressEvent/);
  });

  it('前端在 onProgress 时把通道作为顶层参数传出', () => {
    expect(backend).toContain('args.channel = channel');
    // 与 Rust 形参名一致（Tauri 按 key 匹配）。
    expect(commands).toContain('channel: Option<tauri::ipc::JavaScriptChannelId>');
  });

  it('pluginCall 的 kind 词表 TS ↔ Rust 校验字面量一致', () => {
    const iface = hostSrc.match(/export interface PluginCallRequest \{([\s\S]*?)\n\}/)?.[1] ?? '';
    const tsKinds = [...(iface.match(/kind: ([^;]+);/)?.[1] ?? '').matchAll(/'(\w+)'/g)].map(
      (m) => m[1]!,
    );
    expect(tsKinds.length, 'PluginCallRequest.kind 未找到').toBeGreaterThan(0);

    // Rust 侧校验分支里的字面量集合必须与 TS 联合完全一致。
    const guard = adapter.match(/pub fn wire_plugin_call[\s\S]*?\n\}/)?.[0] ?? '';
    const rustKinds = [...guard.matchAll(/kind != "(\w+)"/g)].map((m) => m[1]!);
    expect(rustKinds.sort()).toEqual([...tsKinds].sort());
  });
});

describe('门禁：错误码 TS ↔ Rust 一致', () => {
  const errorSrc = read(`${SHELL}/error.rs`);

  /** Rust `Display` impl 中的 variant → 线名映射。 */
  function variantToCode(): Map<string, string> {
    const start = errorSrc.indexOf('impl std::fmt::Display');
    const block = errorSrc.slice(start, errorSrc.indexOf('\n}', start));
    return new Map(
      [...block.matchAll(/Self::(\w+)\s*=>\s*"(SC-\d{4})"/g)].map((m) => [m[1]!, m[2]!]),
    );
  }

  it('14 个错误码线名与 TS 枚举逐项一致', () => {
    const tsCodes = (Object.values(PluginErrorCode) as string[]).sort();
    expect(tsCodes).toHaveLength(14);

    const rustCodes = [...variantToCode().values()].sort();
    expect(rustCodes).toEqual(tsCodes);
  });

  it('可重试集合一致（Rust variant → 线名 → TS 集合）', () => {
    const open = errorSrc.indexOf('matches!(', errorSrc.indexOf('pub fn retryable'));
    expect(open, 'retryable() matches! not found').toBeGreaterThan(-1);
    const block = errorSrc.slice(open, errorSrc.indexOf(')', open) + 1);
    const variants = [...block.matchAll(/Self::(\w+)/g)].map((m) => m[1]!);

    const lookup = variantToCode();
    const rustRetryable = variants.map((v) => {
      const code = lookup.get(v);
      expect(code, `variant ${v} has no wire code`).toBeDefined();
      return code!;
    });

    expect(rustRetryable.sort()).toEqual([...RETRYABLE_ERROR_CODES].sort());
    expect(rustRetryable).toHaveLength(3);
  });
});

describe('门禁：应用层宿主错误码 TS ↔ Rust 一致', () => {
  // 与上面框架层门禁**不同**：这里读 tauron-host 的 `E_*` 码表，
  // 不是 tauron-shell 的 `SC-####` 码表。两层码表各自独立，都必须锁。
  const hostErrorSrc = read('crates/tauron-host/src/error.rs');

  /** Rust `Display` impl 中的 variant → 线名映射。 */
  function variantToCode(): Map<string, string> {
    const start = hostErrorSrc.indexOf('impl fmt::Display for ErrorCode');
    expect(start, 'ErrorCode 的 Display impl 必须存在').toBeGreaterThan(-1);
    const block = hostErrorSrc.slice(start, hostErrorSrc.indexOf('\n}', start));
    // 兼容两种写法：`=> "SC-0001"` 与 `=> write!(f, "E_HOST_PANIC")`。
    return new Map(
      [...block.matchAll(/Self::(E_\w+)\s*=>[^}]*?"([^"]+)"/g)].map((m) => [m[1]!, m[2]!]),
    );
  }

  it('线名集合与 canonical registry 一致；source declaration order 不属于协议', () => {
    const rustCodes = [...variantToCode().values()];
    const registry = JSON.parse(read('contracts/error/error-codes.json')) as {
      codes: Array<{ code: string; retryClass: string }>;
    };
    const canonicalCodes = registry.codes.map((entry) => entry.code);
    expect(new Set(rustCodes).size).toBe(rustCodes.length);
    expect(new Set(HOST_ERROR_CODES).size).toBe(HOST_ERROR_CODES.length);
    expect([...rustCodes].sort()).toEqual([...canonicalCodes].sort());
    expect([...HOST_ERROR_CODES].sort()).toEqual([...canonicalCodes].sort());
    expect(HOST_ERROR_CODES.length).toBeGreaterThanOrEqual(18);
  });

  it('线名必须等于枚举变体名（E_* 大写蛇形，禁止 camelCase 漂移）', () => {
    const variants = [...variantToCode()];
    expect(variants.length).toBeGreaterThan(0);
    for (const [variant, code] of variants) {
      expect(code, `${variant} 的线名必须等于变体名`).toBe(variant);
    }
  });

  it('V4 retryClass 一致，panic 不得自动重试', () => {
    const lookup = variantToCode();
    const retryBlock =
      /pub const fn retry_class\(self\)[\s\S]*?\n    \}/.exec(hostErrorSrc)?.[0] ?? '';
    expect(retryBlock, 'retry_class() must exist').not.toBe('');
    const rustClass = new Map<string, string>();
    for (const m of retryBlock.matchAll(/Self::(E_\w+)\s*=>\s*RetryClass::(\w+)/g)) {
      const code = lookup.get(m[1]!);
      expect(code, `variant ${m[1]} has no wire code`).toBeDefined();
      const cls = m[2]!.replace(/([a-z0-9])([A-Z])/g, '$1-$2').toLowerCase();
      rustClass.set(code!, cls);
    }
    const registry = JSON.parse(read('contracts/error/error-codes.json')) as {
      codes: Array<{ code: keyof typeof HOST_RETRY_CLASS; retryClass: string }>;
    };
    // Unlisted Rust variants use the explicit Never fallback; canonical registry is the
    // language-neutral public source and every binding must agree with it.
    for (const entry of registry.codes) {
      const expected = rustClass.get(entry.code) ?? 'never';
      expect(HOST_RETRY_CLASS[entry.code], entry.code).toBe(expected);
      expect(entry.retryClass, entry.code).toBe(expected);
    }
    expect(HOST_RETRY_CLASS.E_HOST_PANIC).toBe('never');
    expect(HOST_RETRY_CLASS.E_CALL_TIMEOUT).toBe('manual');
    expect(HOST_RETRY_CLASS.E_LEASE_EXPIRED).toBe('after-reconnect');
    expect(RETRYABLE_HOST_ERROR_CODES).toEqual([]);
  });

  it('HostError 必须以结构化 JSON 穿越 IPC（禁止 {:?} 文本转储）', () => {
    // 派生 Serialize：前端 normalizeError 分支 1 依赖 { code, message, retryable, retryClass }。
    const derive = hostErrorSrc.match(/#\[derive\(([^)]*)\)\][\s\S]{0,160}?#\[error/)?.[1] ?? '';
    expect(derive, 'HostError 必须 derive(Serialize)').toContain('Serialize');
    expect(hostErrorSrc).toMatch(/pub struct HostError[\s\S]*?pub retry_class: RetryClass/);
    expect(hostErrorSrc).toMatch(/serde\(rename_all = "camelCase"\)/);
    // 文本转储会丢掉 message 字段，前端只能靠正则从转储里反抠错误码。
    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    expect(tauri).not.toContain('format!("{:?}", e)');
  });
});

describe('门禁：sidecar ABI 契约 TS ↔ Rust 同值', () => {
  // `host_runtime_spawn` 会比对宿主 ABI 契约与调用方声明的 `profile.abi`，不符即
  // `E_ABI_MISMATCH`。前端必须能拿到**同一份**契约值，否则每次 spawn 都被拒——这正是
  // 「文档承诺了码、实现却不产生」那类断链的镜像：常量漂移会让码变得不可达。
  const procSrc = read('crates/tauron-proc/src/lib.rs');
  const tsSrc = read('packages/tauron-host/src/shell-client.ts');

  const rustConst = (name: string): string => {
    const m = procSrc.match(new RegExp(`pub const ${name}: &str = "([^"]+)"`));
    expect(m, `Rust 常量 ${name} 必须存在`).not.toBeNull();
    return m![1]!;
  };

  it('rust_version 维度同值', () => {
    expect(tsSrc).toContain(`rustVersion: '${rustConst('SIDECAR_ABI_RUST_VERSION')}'`);
  });

  it('interface_hash 维度同值', () => {
    expect(tsSrc).toContain(`interfaceHash: '${rustConst('SIDECAR_ABI_INTERFACE_HASH')}'`);
  });

  it('契约指纹由两个常量构造（不散落硬编码）', () => {
    expect(procSrc).toContain('pub fn current_abi_contract()');
    expect(procSrc).toMatch(
      /AbiFingerprint::now\(SIDECAR_ABI_RUST_VERSION, SIDECAR_ABI_INTERFACE_HASH\)/,
    );
  });

  it('spawn 前真的调用了 validate_abi（否则 E_ABI_MISMATCH 永不产生 = 孤儿码）', () => {
    const adapterSrc = read('crates/tauron-adapter/src/lib.rs');
    expect(adapterSrc).toMatch(
      /validate_abi\(&current_abi_contract\(\), &cfg\.abi\)\.map_err\(proc_error_to_host\)\?;/,
    );
    // 映射必须独立成码，不得并进 E_INSTALL_FAILED（否则前端分不出"版本不兼容"）。
    expect(adapterSrc).toMatch(/ProcError::AbiMismatch \{ \.\. \} => ErrorCode::E_ABI_MISMATCH/);
  });

  it('E_ABI_MISMATCH 在 TS 码表里（前端能识别）', () => {
    expect([...HOST_ERROR_CODES]).toContain('E_ABI_MISMATCH');
  });
});

describe('门禁：tsconfig 中 Rust crate 路径真实存在', () => {
  it('tauron-host 门禁引用的 crate 目录存在', () => {
    // 这些路径被 packages/tauron-host/src/gates.test.ts 读取
    for (const rel of [
      'crates/tauron-host/src/error.rs',
      'crates/tauron-host/src/authz.rs',
      'crates/tauron-host/src/eventbus.rs',
      'crates/tauron-acl/src/grant.rs',
    ]) {
      expect(() => read(rel), `${rel} missing`).not.toThrow();
    }
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 应用层命令族（host_*）：TS HostClient 调用点 ↔ Rust 注册表
// ──────────────────────────────────────────────────────────────────────────

/** TS `HostClient` 实际调用的 host_* 命令（host.ts 中的字符串字面量）。 */
function tsHostCommands(): string[] {
  const src = read('packages/tauron-host/src/host.ts');
  return [...src.matchAll(/'host_\w+'/g)].map((m) => m[0]!.replaceAll("'", ''));
}

/**
 * Rust `tauri.rs` 中**全量** handler 宏注册的 host_* 命令。
 *
 * R1a 之后 `tauron_generate_handler!` 只是 `tauron_plugin_handler![]` 的别名，
 * 命令清单移到了 `tauron_plugin_handler!` 里——这里解析全量集合（底座 + 插件域），
 * 因此所有「TS 客户端调用的命令必须在 Rust 注册表里」的门禁仍按全量口径判定。
 */
function rustHostCommands(): string[] {
  const src = read('crates/tauron-adapter/src/tauri.rs');
  const macroStart = src.indexOf('macro_rules! tauron_plugin_handler');
  expect(macroStart, 'tauron_plugin_handler! macro not found').toBeGreaterThan(-1);
  const macroEnd = src.indexOf('}', src.indexOf(']', macroStart));
  const macro = src.slice(macroStart, macroEnd);
  return [...macro.matchAll(/tauri::(host_\w+)/g)].map((m) => m[1]!);
}

/**
 * 解析 `tauri.rs` 的**两组编译期可选命令集合**（R1a）。
 *
 * R1b 起这两组集合与命令的 `State` bound 必须一一对应：底座集合里的命令绑
 * `SubstrateState`，差集（插件域）的命令绑 `PluginRuntimeState`。
 */
function rustHandlerFamilies(): { substrate: string[]; full: string[] } {
  const src = read('crates/tauron-adapter/src/tauri.rs');
  const commandsOf = (macroName: string): string[] => {
    const start = src.indexOf(`macro_rules! ${macroName}`);
    expect(start, `未找到宏 ${macroName}`).toBeGreaterThan(-1);
    const end = src.indexOf('\n}\n', start);
    const body = src.slice(start, end > start ? end : undefined);
    return [...body.matchAll(/\$crate::tauri::(host_[a-z0-9_]+),/g)].map((m) => m[1]!);
  };
  return {
    substrate: commandsOf('tauron_substrate_handler'),
    full: commandsOf('tauron_plugin_handler'),
  };
}

/** TS `ShellClient` 实际调用的 host_* 命令（shell-client.ts 中的字符串字面量）。 */
function tsShellCommands(): string[] {
  const src = read('packages/tauron-host/src/shell-client.ts');
  return [...src.matchAll(/'host_\w+'/g)].map((m) => m[0]!.replaceAll("'", ''));
}

describe('门禁：应用层 host_* 命令族 TS ↔ Rust 一致', () => {
  it('应用层插件标识为 tauron（前端 TauriBackend 默认前缀据此路由）', () => {
    const src = read('crates/tauron-adapter/src/tauri.rs');
    // 允许泛型形式 `Builder::<R>::new("tauron")`；与框架层 tauron-shell 不同名。
    expect(src).toMatch(/Builder(?:::<\w+>)?::new\("tauron"\)/);
  });

  it('HostClient 调用的每个命令都在 Rust 注册表中', () => {
    const rust = new Set(rustHostCommands());
    const ts = tsHostCommands();
    expect(rust.size, 'Rust 注册宏必须非空').toBeGreaterThanOrEqual(36);
    expect(ts.length, 'HostClient must reference host_* commands').toBeGreaterThan(0);

    const missing = [...new Set(ts)].filter((c) => !rust.has(c));
    expect(missing, `TS 调用了 Rust 未注册的命令: ${missing.join(', ')}`).toEqual([]);
  });

  it('ShellClient 调用的每个命令都在 Rust 注册表中', () => {
    // 断链回归：host.ts（HostClient）有门禁而 shell-client.ts 没有——恢复/i18n
    // 的真实接通全走 ShellClient，一旦命令名拼写错或 Rust 侧漏注册，只会在
    // 运行时 command not found。这里把第二个调用面也钉住。
    const rust = new Set(rustHostCommands());
    const ts = tsShellCommands();
    expect(ts.length, 'ShellClient must reference host_* commands').toBeGreaterThanOrEqual(10);

    const missing = [...new Set(ts)].filter((c) => !rust.has(c));
    expect(missing, `ShellClient 调用了 Rust 未注册的命令: ${missing.join(', ')}`).toEqual([]);
  });

  it('ShellClient 的每个方法都有 Rust 命令（防孤儿方法：写了包装却没接线）', () => {
    // 死方法检测：`async xxx(...): Promise<T> { return this.call<T>('host_yyy'`
    // 的方法名与命令名必须一一配对——方法存在而命令缺失（或反之）都是断链。
    const src = read('packages/tauron-host/src/shell-client.ts');
    const methods = [
      ...src.matchAll(
        /async (\w+)\([^)]*\): Promise<[^>]+> \{\s*return this\.call<[^>]*>\('host_\w+'/g,
      ),
    ].map((m) => m[1]!);
    // 正则失配保护：ShellClient 至少有这些真实命令包装方法。
    expect(methods.length, '未解析出 ShellClient 命令方法（正则失配）').toBeGreaterThanOrEqual(10);

    const calls = tsShellCommands();
    // 每个命令字面量都应被某个方法用到（孤儿 = 字面量出现但无方法持有，或反之）。
    for (const cmd of [
      'host_recover_report',
      'host_recover_trial_enable',
      'host_i18n_set_locale',
      'host_i18n_load',
      'host_i18n_stats',
      'host_i18n_cleanup_plugin',
    ]) {
      expect(calls, `ShellClient 缺少 ${cmd} 的包装方法`).toContain(cmd);
    }
  });

  it('Rust 注册的每个命令都有 TS 调用点（防孤儿命令：注册了却没人调）', () => {
    // 反向门禁。此前只有「TS 调了 Rust 没注册」这一个方向被钉住，于是
    // `host_stream_write` 可以长期注册着、在 TS SDK 里**没有任何入口**——
    // `pluginCall` 的错误文案甚至拿它当"已接线"的替代路径来推荐。
    //
    // 口径：把 Rust 注册表当作全集，要求每条命令至少在一个**真实调用面**上出现。
    // 刻意排除三类文件，否则门禁会自我满足：
    //   - `*.test.ts`（测试里出现不算接通）；
    //   - `capabilities.ts`（它按定义列举**全部**命令，是清单不是调用点）；
    //   - `tauri-backend.ts`（路由表，同样列举全部命令）。
    const rust = [...new Set(rustHostCommands())];
    expect(rust.length, 'Rust 注册命令数异常').toBeGreaterThanOrEqual(36);

    // 相对**仓库根**遍历（`read()` 也是相对根解析的）。
    const files: string[] = [];
    const walk = (rel: string): void => {
      for (const entry of readdirSync(resolve(workspaceRoot, rel), { withFileTypes: true })) {
        const p = `${rel}/${entry.name}`;
        if (entry.isDirectory()) {
          if (entry.name === 'node_modules' || entry.name === 'dist') continue;
          walk(p);
        } else if (
          entry.name.endsWith('.ts') &&
          !entry.name.endsWith('.test.ts') &&
          entry.name !== 'capabilities.ts' &&
          entry.name !== 'tauri-backend.ts'
        ) {
          files.push(p);
        }
      }
    };
    walk('packages');
    expect(files.length, '未扫到任何 TS 源文件（路径失配）').toBeGreaterThan(50);

    // **必须剥注释**再匹配：否则一句「TODO: 接 host_xxx」的注释就能让一条
    // 从没被调用过的命令通过门禁（假绿）。`stripComments` 保留字符串字面量，
    // 因此 `invoke('host_xxx')` 这类真实调用点仍然算数。
    const blob = files.map((f) => stripComments(read(f))).join('\n');
    const orphaned = rust.filter((cmd) => !blob.includes(`'${cmd}'`));
    expect(
      orphaned,
      `Rust 注册但 TS 无任何调用点的命令（孤儿命令）: ${orphaned.join(', ')}`,
    ).toEqual([]);
  });

  it('订阅链路四件套齐全（subscribe / unsubscribe / drain / publish）', () => {
    // 断链回归：drain 是订阅的取件步骤，缺失则队列只进不出。
    const rust = new Set(rustHostCommands());
    for (const cmd of [
      'host_events_publish',
      'host_events_subscribe',
      'host_events_unsubscribe',
      'host_events_drain',
    ]) {
      expect(rust.has(cmd), `Rust 缺少 ${cmd}`).toBe(true);
    }

    const ts = new Set(tsHostCommands());
    for (const cmd of ['host_events_subscribe', 'host_events_unsubscribe', 'host_events_drain']) {
      expect(ts.has(cmd), `HostClient 缺少 ${cmd}`).toBe(true);
    }
  });

  it('Rust drain 通道名与 TS eventsDrain 参数一致（kebab-case）', () => {
    const hostRs = read('crates/tauron-host/src/eventbus.rs');
    // ChannelKind::parse 必须接受 TS 侧发送的三种通道名
    for (const kind of ['event', 'request', 'state']) {
      expect(hostRs).toContain(`"${kind}" =>`);
    }
    const hostTs = read('packages/tauron-host/src/host.ts');
    expect(hostTs).toContain("'event' | 'request' | 'state'");
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 门禁：应用层 host_* **参数形状** TS ↔ Rust 一致
//
// 只比对命令名的门禁发现不了"名对得上、形状对不上"这类断链：TS 发
// `{ req: {…} }` 而 Rust 期待 `cmd`/`args` 时，命令名两侧都在、运行时却必然以
// missing required key 失败（曾经真实发生过：7 条命令全断）。
// 本门禁解析两侧源码，逐调用点校验：
//   ① TS 的每个顶层键都是 Rust 声明的线参数（camelCase）；
//   ② Rust 的每个必填参数都被该调用点提供；
//   ③ 结构体参数（req/evt/op/sub…）的嵌套键都存在于对应 Rust 结构体。
// ──────────────────────────────────────────────────────────────────────────

const snakeToCamel = (s: string): string =>
  s.replace(/_([a-z])/g, (_all, c: string) => c.toUpperCase());

/** 取 `src[start]` 处 `{`/`[` 开头的平衡片段（跳过字符串字面量与转义）。 */
function balanced(src: string, start: number): string {
  const open = src[start]!;
  const close = open === '{' ? '}' : ']';
  let depth = 0;
  let quote: string | null = null;
  for (let i = start; i < src.length; i += 1) {
    const ch = src[i]!;
    if (quote !== null) {
      if (ch === '\\') i += 1;
      else if (ch === quote) quote = null;
      continue;
    }
    if (ch === "'" || ch === '"' || ch === '`') {
      quote = ch;
      continue;
    }
    if (ch === open) depth += 1;
    else if (ch === close) {
      depth -= 1;
      if (depth === 0) return src.slice(start, i + 1);
    }
  }
  return src.slice(start);
}

/** 取对象字面量的顶层键（含简写属性；展开项跳过）。 */
function objectKeys(obj: string): string[] {
  const inner = obj.slice(1, -1);
  const keys: string[] = [];
  let depth = 0;
  let quote: string | null = null;
  let token = '';
  const flush = (): void => {
    const t = token.trim();
    token = '';
    const m = /^(?:\.\.\.)?([A-Za-z_$][\w$]*)/.exec(t);
    if (m) keys.push(m[1]!);
  };
  for (let i = 0; i < inner.length; i += 1) {
    const ch = inner[i]!;
    if (quote !== null) {
      if (ch === '\\') i += 1;
      else if (ch === quote) quote = null;
      token += ch;
      continue;
    }
    if (ch === "'" || ch === '"' || ch === '`') {
      quote = ch;
      token += ch;
      continue;
    }
    if (ch === '{' || ch === '[' || ch === '(') depth += 1;
    else if (ch === '}' || ch === ']' || ch === ')') depth -= 1;
    if (depth === 0 && ch === ',') {
      flush();
      continue;
    }
    token += ch;
  }
  flush();
  return keys;
}

/** 取对象字面量某顶层键的嵌套键：`{…}` 直接取，`[{…}]` 取首个元素。 */
function nestedKeys(obj: string, key: string): Set<string> {
  const inner = obj.slice(1, -1);
  const re = new RegExp(`(?:^|,)\\s*(?:\\.\\.\\.)?${key}\\s*:\\s*`);
  const m = re.exec(inner);
  if (!m) return new Set();
  const valueStart = m.index + m[0].length;
  let p = valueStart;
  while (p < inner.length && /\s/.test(inner[p]!)) p += 1;
  if (inner[p] === '{') return new Set(objectKeys(balanced(inner, p)));
  if (inner[p] === '[') {
    const arr = balanced(inner, p);
    const open = arr.indexOf('{');
    if (open >= 0) return new Set(objectKeys(balanced(arr, open)));
  }
  return new Set();
}

/** 剥离注释（保留字符串字面量，尊重转义），避免文档注释里的命令名被当成调用点。 */
function stripComments(src: string): string {
  let out = '';
  let quote: string | null = null;
  for (let i = 0; i < src.length; i += 1) {
    const ch = src[i]!;
    const next = src[i + 1];
    if (quote !== null) {
      out += ch;
      if (ch === '\\') {
        out += next ?? '';
        i += 1;
        continue;
      }
      if (ch === quote) quote = null;
      continue;
    }
    if (ch === "'" || ch === '"' || ch === '`') {
      quote = ch;
      out += ch;
      continue;
    }
    if (ch === '/' && next === '/') {
      while (i < src.length && src[i] !== '\n') i += 1;
      out += '\n';
      continue;
    }
    if (ch === '/' && next === '*') {
      i += 2;
      while (i < src.length && !(src[i] === '*' && src[i + 1] === '/')) i += 1;
      i += 1;
      out += ' ';
      continue;
    }
    out += ch;
  }
  return out;
}

interface TsCallSite {
  file: string;
  cmd: string;
  /** `null` = 无参调用（`invoke('host_x')`）。 */
  keys: Set<string> | null;
  nested: Map<string, Set<string>>;
}

/** 收集所有 TS 调用点（`invoke('host_x', {…})` / `call('host_x', {…})`）。 */
function tsHostCallSites(): TsCallSite[] {
  const sites: TsCallSite[] = [];
  const skip = /(^|[\\/])(node_modules|dist|__tests__)([\\/]|$)|\.test\.tsx?$/;
  const walk = (dir: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full = join(dir, entry.name);
      if (skip.test(full)) continue;
      if (entry.isDirectory()) {
        walk(full);
        continue;
      }
      if (!entry.name.endsWith('.ts') && !entry.name.endsWith('.tsx')) continue;

      const src = stripComments(readFileSync(full, 'utf8'));
      const rel = full.slice(workspaceRoot.length + 1).replaceAll('\\', '/');
      // 只认 `invoke('host_x'…)` / `call('host_x'…)`：能力表、mock、注释里的
      // 字符串字面量都不是调用点。
      const re = /\b(?:invoke|call)(?:<[^<>]*>)?\(\s*'(host_\w+)'/g;
      for (const m of src.matchAll(re)) {
        const cmd = m[1]!;
        const after = src.slice(m.index + m[0].length);
        const withArgs = /^\s*,\s*\{/.exec(after);
        if (withArgs) {
          const objStart = m.index + m[0].length + withArgs[0].length - 1;
          const obj = balanced(src, objStart);
          const keys = new Set(objectKeys(obj));
          const nested = new Map<string, Set<string>>();
          for (const key of keys) {
            const nk = nestedKeys(obj, key);
            if (nk.size > 0) nested.set(key, nk);
          }
          sites.push({ file: rel, cmd, keys, nested });
        } else if (/^\s*\)/.test(after)) {
          sites.push({ file: rel, cmd, keys: null, nested: new Map() });
        }
      }
    }
  };
  for (const pkg of readdirSync(join(workspaceRoot, 'packages'), { withFileTypes: true })) {
    const sourceDir = join(workspaceRoot, 'packages', pkg.name, 'src');
    if (pkg.isDirectory() && existsSync(sourceDir)) walk(sourceDir);
  }
  return sites;
}

interface RustParam {
  key: string;
  type: string;
  required: boolean;
}

/** 解析 `tauri.rs` 每个 `#[tauri::command]` 的线参数（注入参数除外）。 */
function rustCommandParams(): Map<string, RustParam[]> {
  // 去掉行注释：参数表里可能夹带说明注释，注释文本不是类型
  const src = read('crates/tauron-adapter/src/tauri.rs').replace(/\/\/[^\n]*/g, '');
  const out = new Map<string, RustParam[]>();
  const re = /#\[tauri::command\]\s*pub fn (host_\w+)\(([\s\S]*?)\)\s*->/g;
  for (const m of src.matchAll(re)) {
    const cmd = m[1]!;
    const params: RustParam[] = [];
    let depth = 0;
    let token = '';
    const push = (): void => {
      // 参数上的属性（`#[allow(unused_variables)]`）不是类型的一部分
      const t = token
        .trim()
        .replace(/#\[[^\]]*\]/g, '')
        .trim();
      token = '';
      if (!t) return;
      const idx = t.indexOf(':');
      if (idx < 0) return;
      const name = t.slice(0, idx).trim();
      const type = t.slice(idx + 1).trim();
      // 注入参数（Tauri DI）不是线参数：State<'_, _> 生命周期标注、
      // 运行时句柄（CallerSource/WebviewWindow/AppHandle/Window/Webview，可能带 tauri:: 路径）。
      const injected =
        type.includes("'") ||
        /\b(CallerSource|TauriCallerSource|WebviewWindow|Webview|AppHandle|Window|State)\b/.test(
          type,
        );
      if (injected) return;
      params.push({
        key: snakeToCamel(name),
        type,
        required: !type.startsWith('Option<'),
      });
    };
    for (const ch of m[2]!) {
      if (ch === '<') depth += 1;
      if (ch === '>') depth -= 1;
      if (ch === ',' && depth === 0) {
        push();
        continue;
      }
      token += ch;
    }
    push();
    out.set(cmd, params);
  }
  return out;
}

/** 解析 `tauri.rs` 中的应用层线格式结构体：结构体名 → camelCase 字段集。 */
function rustWireStructs(): Map<string, Set<string>> {
  const src = read('crates/tauron-adapter/src/tauri.rs');
  const out = new Map<string, Set<string>>();
  const re = /pub struct (Host\w+) \{([\s\S]*?)\n\}/g;
  for (const m of src.matchAll(re)) {
    const fields = new Set<string>();
    for (const fm of m[2]!.matchAll(/pub (\w+)\s*:/g)) fields.add(snakeToCamel(fm[1]!));
    out.set(m[1]!, fields);
  }
  return out;
}

/** 取参数类型里引用的应用层结构体名（`T` / `Option<T>` / `Vec<T>`）。 */
function structOf(type: string): string | undefined {
  const inner = type.replace(/^Option<(.+)>$/, '$1').replace(/^Vec<(.+)>$/, '$1');
  return /^Host\w+$/.test(inner) ? inner : undefined;
}

describe('门禁：应用层 host_* 参数形状 TS ↔ Rust 一致', () => {
  it('两侧解析出足量形状（防正则静默失配）', () => {
    expect(tsHostCallSites().length, 'TS 调用点数量').toBeGreaterThanOrEqual(10);
    expect(rustCommandParams().size, 'Rust 命令数量').toBeGreaterThanOrEqual(30);
    expect(rustWireStructs().size, '线格式结构体数量').toBeGreaterThanOrEqual(6);
  });

  it('每个 TS 调用点提供的顶层键都是 Rust 声明的线参数', () => {
    const rust = rustCommandParams();
    const problems: string[] = [];
    for (const site of tsHostCallSites()) {
      const params = rust.get(site.cmd);
      if (!params) {
        problems.push(`${site.file}: ${site.cmd} 未在 Rust 侧导出`);
        continue;
      }
      const known = new Set(params.map((p) => p.key));
      for (const key of site.keys ?? []) {
        if (!known.has(key)) {
          problems.push(
            `${site.file}: ${site.cmd} 的 \`${key}\` 不是 Rust 参数（合法：${[...known].join('/')}）`,
          );
        }
      }
    }
    expect(problems, problems.join('\n')).toEqual([]);
  });

  it('Rust 的每个必填参数都被对应调用点提供', () => {
    const rust = rustCommandParams();
    const problems: string[] = [];
    for (const site of tsHostCallSites()) {
      const params = rust.get(site.cmd);
      if (!params) continue;
      const provided = site.keys ?? new Set<string>();
      for (const p of params.filter((x) => x.required)) {
        if (!provided.has(p.key)) {
          problems.push(`${site.file}: ${site.cmd} 缺少必填参数 \`${p.key}\``);
        }
      }
    }
    expect(problems, problems.join('\n')).toEqual([]);
  });

  it('结构体参数的嵌套键都存在于对应 Rust 结构体', () => {
    const rust = rustCommandParams();
    const structs = rustWireStructs();
    const problems: string[] = [];
    for (const site of tsHostCallSites()) {
      const params = rust.get(site.cmd);
      if (!params) continue;
      for (const [key, keys] of site.nested) {
        const param = params.find((p) => p.key === key);
        if (!param) continue;
        const struct = structOf(param.type);
        if (!struct) continue;
        const fields = structs.get(struct);
        if (!fields) {
          problems.push(`${site.file}: ${site.cmd} 的 ${struct} 结构体未找到`);
          continue;
        }
        for (const k of keys) {
          if (!fields.has(k)) {
            problems.push(
              `${site.file}: ${site.cmd}.${key}.${k} 不在 ${struct}（合法：${[...fields].join('/')}）`,
            );
          }
        }
      }
    }
    expect(problems, problems.join('\n')).toEqual([]);
  });

  it('生命周期上报必须是事件（不是状态）——状态由宿主单一写入', () => {
    const sites = tsHostCallSites().filter((s) => s.cmd === 'host_lifecycle_report');
    expect(sites.length, 'HostClient 必须调用 host_lifecycle_report').toBeGreaterThan(0);
    for (const site of sites) {
      expect([...(site.keys ?? [])], `${site.file} 的线参数`).toEqual(['evt']);
      expect([...(site.nested.get('evt') ?? [])], `${site.file} 的 evt 字段`).toContain('event');
    }
    // Rust 侧结构体字段必须是 event（不是 state）
    expect([...(rustWireStructs().get('HostLifecycleEvt') ?? [])]).toContain('event');
  });

  it('多选择器订阅必须有分组退订（不留悬挂订阅）', () => {
    const src = read('crates/tauron-adapter/src/tauri.rs');
    expect(src).toContain('subscription_groups');
    expect(src).toContain('wire_events_unsubscribe');
    expect([...tsHostCallSites()].some((s) => s.cmd === 'host_events_subscribe')).toBe(true);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 门禁：生命周期状态机镜像（TS 词表 ↔ Rust 枚举）
//
// `LIFECYCLE_STATES` / `LIFECYCLE_EVENTS` 必须与 Rust `lifecycle::State` /
// `lifecycle::Event` 的变体逐名一致（SCREAMING_SNAKE_CASE 线名）。
// 曾经的断链：TS 把状态名（RUNNING）当事件名上报，宿主反序列化必失败。
// ──────────────────────────────────────────────────────────────────────────

const SCREAMING = (s: string): string => s.replaceAll(/([a-z0-9])([A-Z])/g, '$1_$2').toUpperCase();

/** 解析 Rust 枚举的全部变体名（按声明顺序）。 */
function rustEnumVariants(path: string, enumName: string): string[] {
  const src = read(path);
  const m = new RegExp(`pub enum ${enumName} \\{([\\s\\S]*?)\\n\\}`).exec(src);
  expect(m, `Rust enum ${enumName} not found in ${path}`).not.toBeNull();
  return [...m![1]!.matchAll(/^\s{4}([A-Z]\w+),?\s*(?:\/\/.*)?$/gm)].map((v) => v[1]!);
}

describe('门禁：生命周期状态机镜像 TS ↔ Rust', () => {
  const LIFECYCLE_RS = 'crates/tauron-host/src/lifecycle.rs';

  it('状态词表与 Rust State 逐名一致（顺序相同）', () => {
    const rust = rustEnumVariants(LIFECYCLE_RS, 'State').map(SCREAMING);
    const ts: readonly string[] = LIFECYCLE_STATES;
    expect(rust.length, 'Rust State 变体数').toBeGreaterThanOrEqual(10);
    expect(ts, `状态线名不一致\nRust: ${rust.join(',')}\nTS:   ${ts.join(',')}`).toEqual(rust);
  });

  it('事件词表与 Rust Event 逐名一致（顺序相同）', () => {
    const rust = rustEnumVariants(LIFECYCLE_RS, 'Event').map(SCREAMING);
    const ts: readonly string[] = LIFECYCLE_EVENTS;
    expect(rust.length, 'Rust Event 变体数').toBeGreaterThanOrEqual(17);
    expect(ts, `事件线名不一致\nRust: ${rust.join(',')}\nTS:   ${ts.join(',')}`).toEqual(rust);
  });

  it('TS 侧不再把状态名当事件上报（断链回归）', () => {
    const hostTs = read('packages/tauron-host/src/host.ts');
    // 事件类型必须是**收窄后的** `PluginReportableEvent`，不是完整枚举：
    // `host_lifecycle_report` 是 self 档，越权事件 Rust 会以 E_AUTH_DENIED 硬拒。
    expect(hostTs).toMatch(/lifecycleReport\(evt: \{ event: PluginReportableEvent/);
    expect(hostTs).not.toMatch(/lifecycleReport\(evt: \{ event: LifecycleEvent/);
    expect(hostTs).not.toMatch(/lifecycleReport\(evt: \{ state:/);
  });

  // ── self 档事件白名单（P0：插件不得自报越权事件）──────────────────
  // `host_lifecycle_report` 由插件自己调用。若白名单缺失或过宽，插件就能
  // 自报 ENABLE（跳过用户放行）/ SAFEMODE_EXIT（自己解禁）/ UNINSTALL
  // （进终态，槽位再也收不回）。Rust `Event::PLUGIN_REPORTABLE` 是单一真相源。

  /** 解析 Rust `Event::PLUGIN_REPORTABLE` 数组的事件名（容忍 rustfmt 折行）。 */
  const rustPluginReportable = (): string[] => {
    const src = read(LIFECYCLE_RS);
    const m = /pub const PLUGIN_REPORTABLE:\s*\[Event;\s*\d+\]\s*=\s*\[([\s\S]*?)\];/.exec(src);
    expect(m, 'Rust 未定义 Event::PLUGIN_REPORTABLE').not.toBeNull();
    const names = [...m![1]!.matchAll(/Event::(\w+)/g)].map((v) => SCREAMING(v[1]!));
    // 解析到 0 条 = 正则失配的**假绿**，必须硬失败。
    expect(names.length, 'PLUGIN_REPORTABLE 解析到 0 条（门禁定位失败）').toBeGreaterThan(0);
    return names;
  };

  it('插件可自报事件白名单 TS ↔ Rust 逐名一致', () => {
    const rust = rustPluginReportable();
    expect([...PLUGIN_REPORTABLE_EVENTS].sort()).toEqual([...rust].sort());
  });

  it('白名单是完整事件枚举的严格子集，且排除全部越权事件', () => {
    const rust = rustPluginReportable();
    // 严格子集：等于全量就等于没设闸。
    expect(rust.length, '白名单不得等于完整事件枚举').toBeLessThan(LIFECYCLE_EVENTS.length);
    for (const e of rust) {
      expect(LIFECYCLE_EVENTS as readonly string[]).toContain(e);
    }

    // 越权事件逐一钉死（含「Disabled→Enabled 出边」的全部三条：ENABLE /
    // TRIAL_ENABLE / SAFEMODE_EXIT——它们一旦可自报，插件就能自行解禁）。
    const forbidden = [
      'ENABLE',
      'DISABLE',
      'TRIAL_ENABLE',
      'SAFEMODE_ENTER',
      'SAFEMODE_EXIT',
      'INSTALL_START',
      'INSTALL_OK',
      'INSTALL_FAIL',
      'UNINSTALL',
      'PURGE',
    ];
    const leaked = forbidden.filter((e) => rust.includes(e));
    expect(leaked, `以下越权事件混进了插件可自报白名单：${leaked.join(', ')}`).toEqual([]);

    // 交叉校验：TRANSITIONS 里 `Disabled → Enabled` 的出边事件必须全部不可自报。
    const reenable = new Set(
      [
        ...transitions().matchAll(
          /rule!\(\s*State::Disabled,\s*Event::(\w+),\s*(?:Guard::\w+,\s*)?State::Enabled/g,
        ),
      ].map((m) => SCREAMING(m[1]!)),
    );
    expect(reenable.size, '未解析到 Disabled→Enabled 出边（门禁定位失败）').toBeGreaterThan(0);
    const selfUnlock = [...reenable].filter((e) => rust.includes(e));
    expect(
      selfUnlock,
      `插件可自行解禁（Disabled→Enabled 出边事件在白名单里）：${selfUnlock.join(', ')}`,
    ).toEqual([]);
  });

  it('白名单强制点必须落在 self 档入口（registry.lifecycle_report）', () => {
    const reg = read('crates/tauron-host/src/registry.rs');
    expect(reg, 'lifecycle_report 未调用 plugin_reportable() 做闸').toMatch(
      /fn lifecycle_report\([\s\S]{0,1200}?plugin_reportable\(\)/,
    );
    // 宿主内部路径 `report_event` 不得被这道闸拦住（否则恢复引擎/管理面会被自锁）。
    const reportEvent = /pub fn report_event\([\s\S]*?\n    \}/.exec(reg);
    expect(reportEvent, '未找到 report_event').not.toBeNull();
    expect(reportEvent![0]).not.toMatch(/plugin_reportable/);
  });

  // ── 转移表健全性 / 活性（词表一致 ≠ 表可用）────────────────────
  // 词表镜像只保证「名字对得上」；下面三条保证**表本身**可用：
  // 没有永远非法的事件、没有不可达状态、没有非终态死路。

  /** `TRANSITIONS` 表源码文本。 */
  const transitions = (): string => {
    const m = /pub static TRANSITIONS[\s\S]*?\n\];/.exec(read(LIFECYCLE_RS));
    expect(m, 'TRANSITIONS 未找到').not.toBeNull();
    return m![0];
  };

  it('每个事件在 TRANSITIONS 里都有规则（无「上报必非法」的事件）', () => {
    const table = transitions();
    const covered = new Set(
      [...table.matchAll(/rule!\(\s*State::\w+,\s*Event::(\w+)/g)].map((m) => m[1]!),
    );

    const missing = rustEnumVariants(LIFECYCLE_RS, 'Event').filter((e) => !covered.has(e));
    expect(
      missing,
      `以下事件在 TRANSITIONS 中没有任何规则，上报必被判非法：${missing.join(', ')}`,
    ).toEqual([]);

    // TS 词表同样必须全部覆盖（防未来新增事件只改词表）。
    const coveredScreaming = new Set([...covered].map(SCREAMING));
    const tsMissing = LIFECYCLE_EVENTS.filter((e) => !coveredScreaming.has(e));
    expect(tsMissing, `TS 词表中无规则的事件：${tsMissing.join(', ')}`).toEqual([]);
  });

  it('除初始态 Discovered 外所有状态可达（无不可达状态）', () => {
    const table = transitions();
    // 守卫是可选的第 3 个位置参数，必须允许（否则带守卫的规则目标会漏掉）。
    const tos = new Set(
      [
        ...table.matchAll(/rule!\(\s*State::\w+,\s*Event::\w+,\s*(?:Guard::\w+,\s*)?State::(\w+)/g),
      ].map((m) => m[1]!),
    );

    const unreachable = rustEnumVariants(LIFECYCLE_RS, 'State').filter(
      (s) => !tos.has(s) && s !== 'Discovered',
    );
    expect(unreachable, `状态无任何规则转入（不可达）：${unreachable.join(', ')}`).toEqual([]);
  });

  it('唯一无出边的状态是终态 Uninstalled（其余状态不得卡死）', () => {
    const table = transitions();
    const froms = new Set([...table.matchAll(/rule!\(\s*State::(\w+),/g)].map((m) => m[1]!));

    const deadEnds = rustEnumVariants(LIFECYCLE_RS, 'State').filter((s) => !froms.has(s));
    expect(
      deadEnds,
      `非终态死路（进入即永久卡死）：${deadEnds.filter((s) => s !== 'Uninstalled').join(', ')}`,
    ).toEqual(['Uninstalled']);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 门禁：能力表（命令 → 授权档位）TS ↔ Rust 同构
//
// TS `capabilities.ts` 自称"与 Rust authz::COMMANDS / ADMIN_COMMANDS 同构"，
// 这里兑现这句话：逐命令比对档位，漂移即失败（审批 UI 文案与真实档位脱节
// 属于安全面回归）。
// ──────────────────────────────────────────────────────────────────────────

/** Rust `AuthTier` 变体 → 线名（authz.rs 标注 kebab-case）。 */
const TIER_WIRE: Record<string, string> = {
  Self_: 'self',
  ScopedRead: 'scoped-read',
  Privileged: 'privileged',
};

/** 解析 authz.rs 命令表的 (command, tier)。 */
function rustAuthTable(): Map<string, string> {
  const core = read('crates/tauron-host/src/authz.rs');
  const adapter = read('crates/tauron-adapter/src/lib.rs');
  const optional = adapter.slice(
    adapter.indexOf('pub const PLUGIN_INSTALL_AUTH'),
    adapter.indexOf('/// 命令状态'),
  );
  const normalizedOptional = optional.replace(/tauron_host::authz::AuthTier::/g, 'AuthTier::');
  const src = `${core}\n${normalizedOptional}`.replace(/\/\/[^\n]*/g, '');
  const out = new Map<string, string>();
  for (const m of src.matchAll(/command:\s*"(host_\w+)",\s*tier:\s*AuthTier::(\w+)/g)) {
    const tier = TIER_WIRE[m[2]!];
    expect(tier, `未知 AuthTier 变体 ${m[2]}`).toBeDefined();
    out.set(m[1]!, tier!);
  }
  return out;
}

describe('门禁：能力表（命令 → 档位）TS ↔ Rust 同构', () => {
  it('命令集合一致（插件面 20 条 + 主窗特权命令）', () => {
    const rust = rustAuthTable();
    const ts = new Map(CAPABILITIES.map((c) => [c.command, c.tier]));
    // 不写死总数（会随命令面增长而漂移）：只钉住两表**逐条相等**与结构比例。
    expect(ts.size, 'TS CAPABILITIES 条目数').toBe(rust.size);
    expect([...ts.keys()].sort(), '命令集合').toEqual([...rust.keys()].sort());
    const pluginFace = CAPABILITIES.filter((c) => c.tier !== 'privileged');
    // 20 = 13（0.4-A1 之前）+ 跨主体调用 3 条 + 0.4 审计补登记 host_contributes_list
    // + 0.4-W3 扩展点对账 host_contributes_reconcile + V4 A79 host_stream_grant
    // + 轮 7 补登记的能力协商入口 host_capabilities。
    expect(pluginFace.length, '插件面（self + scoped-read）命令数').toBe(20);
    expect(
      CAPABILITIES.filter((c) => c.consumer === 'plugin')
        .map((c) => c.command)
        .sort(),
    ).toEqual(pluginFace.map((c) => c.command).sort());
    // 审计补登记（轮 11）：这 4 条此前**没有任何档位**，但 `HostClient` 已在调用。
    for (const cmd of [
      'host_events_drain',
      'host_stream_open',
      'host_stream_write',
      'host_stream_close',
    ]) {
      expect(ts.get(cmd), `${cmd} 必须有档位`).toBe('self');
      expect(rust.get(cmd), `Rust 侧 ${cmd} 档位`).toBe('self');
    }
  });

  it('进程执行原语必须是主窗特权（P0-2 越权面）', () => {
    // `host_runtime_spawn` 能按入参 plugin_id 启动可执行文件：若为 self/scoped 档，
    // 任何插件 webview 都能起别人的 sidecar（越权执行），故档位是安全属性而非文案。
    const rust = rustAuthTable();
    for (const cmd of ['host_runtime_spawn', 'host_runtime_health']) {
      expect(rust.get(cmd), `${cmd} 未登记档位`).toBe('privileged');
      expect(
        CAPABILITIES.find((c) => c.command === cmd)?.tier,
        `${cmd} 的 TS 档位必须同为 privileged`,
      ).toBe('privileged');
    }
    // 特权档命令**不得**出现在插件可见能力里。（轮 40：`host_market_download` /
    // `host_market_install` 从模拟桩升为装配腿真路径后进特权面——供应链操作没有
    // "某个插件"能成为主体，判定与审计同源，见 `cmd_market_download_as`。）
    expect(
      CAPABILITIES.filter((c) => c.tier === 'privileged')
        .map((c) => c.command)
        .sort(),
    ).toEqual([
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
  });

  it('每条命令的授权档位一致', () => {
    const rust = rustAuthTable();
    const problems: string[] = [];
    for (const c of CAPABILITIES) {
      const rt = rust.get(c.command);
      if (rt !== c.tier) {
        problems.push(`${c.command}: TS=${c.tier} Rust=${rt ?? '缺失'}`);
      }
    }
    expect(problems, problems.join('\n')).toEqual([]);
  });

  it('Rust AuthTier 序列化为 kebab-case 线名', () => {
    const src = read('crates/tauron-host/src/authz.rs');
    const m = /pub enum AuthTier \{/.exec(src);
    expect(m).not.toBeNull();
    const derive = src.slice(Math.max(0, m!.index - 200), m!.index);
    expect(derive).toContain('rename_all = "kebab-case"');
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 门禁：**返回值**形状 TS ↔ Rust 一致（Rust → 前端消费方向）
//
// 入参形状断链修完后的镜像风险：Rust 返回结构与 TS 消费字段不符（字段缺失、
// snake_case 上线、桩返回 Null 让前端对 null 取属性崩溃）。
// ──────────────────────────────────────────────────────────────────────────

/** 解析某 Rust 结构体：字段名集合 + 是否标注 rename_all = "camelCase"。 */
function rustStruct(rel: string, structName: string): { fields: Set<string>; camelCase: boolean } {
  const src = read(rel).replace(/\/\/[^\n]*/g, '');
  const m = new RegExp(`pub struct ${structName} \\{([\\s\\S]*?)\\n\\}`).exec(src);
  expect(m, `struct ${structName} not found in ${rel}`).not.toBeNull();
  const before = src.slice(Math.max(0, m!.index - 220), m!.index);
  const fields = new Set<string>();
  for (const fm of m![1]!.matchAll(/pub (\w+)\s*:/g)) fields.add(fm[1]!);
  return { fields, camelCase: before.includes('rename_all = "camelCase"') };
}

describe('门禁：返回值形状 TS ↔ Rust 一致', () => {
  it('PluginSummary 必须是 camelCase 且带 disabledBySafemode（oc-plugin-manager 消费）', () => {
    const s = rustStruct('crates/tauron-host/src/registry.rs', 'PluginSummary');
    expect(s.camelCase, 'PluginSummary 必须 rename_all = "camelCase"').toBe(true);
    expect(s.fields.has('disabled_by_safemode'), '缺少 safemode 角标字段').toBe(true);
    expect(s.fields.has('plugin_type'), '缺少 plugin_type').toBe(true);
    const ts = read('packages/tauron-host/src/events.ts');
    expect(ts).toContain('disabledBySafemode');
    expect(ts).toMatch(/pluginType\?: string/);
  });

  it('事件总线帧 Frame ↔ TS EventFrame 逐字段一致', () => {
    const frame = rustStruct('crates/tauron-host/src/eventbus.rs', 'Frame');
    expect(frame.camelCase, 'Frame 必须 rename_all = "camelCase"').toBe(true);
    expect([...frame.fields].sort()).toEqual([
      'causation_id',
      'event_hop',
      'event_id',
      'max_causation_depth',
      'payload',
      'receiver',
      'sender',
      'seq',
      'state_revision',
      'topic',
    ]);
    const ts = read('packages/tauron-host/src/events.ts');
    const m = /export interface EventFrame \{([\s\S]*?)\n\}/.exec(ts);
    expect(m, 'TS EventFrame 必须存在').not.toBeNull();
    const tsFields = [...m![1]!.matchAll(/^\s{2}(\w+)[?]*:/gm)].map((x) => x[1]!);
    expect(tsFields.sort()).toEqual([
      'causationId',
      'eventHop',
      'eventId',
      'maxCausationDepth',
      'payload',
      'receiver',
      'sender',
      'seq',
      'stateRevision',
      'topic',
    ]);
    // drain 的返回类型必须是 EventFrame（不是流式 CallFrame）
    const host = read('packages/tauron-host/src/host.ts');
    expect(host).toMatch(/eventsDrain\([^)]*\): Promise<EventFrame\[\]>/);
  });

  it('auto-update 桩必须返回可消费形状（不得返回 Null 让前端崩溃）', () => {
    const lib = read('crates/tauron-adapter/src/lib.rs');
    expect(lib).toContain('"available": false');
    const ts = read('packages/tauron-host/src/auto-update-client.ts');
    expect(ts).toMatch(/result\?\.simulated/);
    expect(ts).toMatch(/info\?\.available/);
    // 品牌 provider 缺失必须返回显式 Unsupported，而非空对象假装已连接。
    // R9 起 `cmd_brand_info` 接 `tauron-brand` 真实现：返回 `ProviderResult<BrandInfo>`
    // ——未配置来源时走 `ProviderResult::Unsupported`，配置了则跑品牌配置校验。
    const brandFn = /pub fn cmd_brand_info\([\s\S]*?\n\}/.exec(lib)?.[0] ?? '';
    expect(brandFn, 'cmd_brand_info 缺失').not.toBe('');
    expect(brandFn, '品牌 provider 缺失必须显式 Unsupported').toContain(
      'ProviderResult::Unsupported',
    );
    expect(brandFn, '品牌真实现必须返回 ProviderResult<BrandInfo>').toContain(
      'HostResult<ProviderResult<BrandInfo>>',
    );
  });

  it('provider 缺失时的底座桩必须用类型化结果诚实披露', () => {
    const lib = read('crates/tauron-adapter/src/lib.rs');
    const body = /pub struct UnsupportedBody \{([\s\S]*?)\n\}/.exec(lib)?.[1] ?? '';
    expect(body).toMatch(/supported:\s*bool/);
    expect(body).toMatch(/reason:\s*String/);
    expect(body).toMatch(/fallback:\s*Option<String>/);
    for (const cmd of ['open', 'save', 'message', 'confirm']) {
      const impl = new RegExp(`pub fn cmd_dialog_${cmd}\\([\\s\\S]*?\\n\\}`).exec(lib)?.[0] ?? '';
      expect(impl, `cmd_dialog_${cmd} 缺失`).not.toBe('');
      expect(impl, `cmd_dialog_${cmd} 未判定 provider 能力`).toContain('native_supported()');
      expect(impl, `cmd_dialog_${cmd} 未返回 Unsupported`).toContain('ProviderResult::Unsupported');
    }
    expect(lib).toMatch(/pub fn cmd_clipboard_write[\s\S]*?HostResult<UnsupportedBody>/);
    expect(lib).toMatch(/pub fn cmd_clipboard_read[\s\S]*?HostResult<DegradedValue<String>>/);
    expect(lib).toMatch(/pub fn cmd_deep_link_register[\s\S]*?HostResult<ProviderResult<\(\)>>/);
    expect(lib).toMatch(/pub fn cmd_brand_info[\s\S]*?HostResult<UnsupportedBody>/);
    expect(lib).toMatch(/pub struct MarketCheckResult[\s\S]*?pub simulated: bool/);
    expect(lib).toMatch(/pub struct MarketUpdateResult[\s\S]*?pub simulated: bool/);
    const dialogTs = read('packages/tauron-host/src/dialog-client.ts');
    expect(dialogTs).toContain('clipboardReadDetailed');
    expect(dialogTs).toContain('isUnsupportedBody');
    expect(read('packages/tauron-host/src/deep-link-client.ts')).toMatch(
      /Promise<ProviderResult<void>/,
    );
  });

  it('ContributeEntry 必须 camelCase（pluginId 双向：register 反序列化 + list 序列化）', () => {
    const s = rustStruct('crates/tauron-adapter/src/lib.rs', 'ContributeEntry');
    expect(s.camelCase, 'ContributeEntry 必须 rename_all = "camelCase"').toBe(true);
    expect(s.fields.has('plugin_id')).toBe(true);
    const ts = read('packages/tauron-host/src/shell-client.ts');
    expect(ts).toMatch(/interface ContributeEntry \{[\s\S]*?pluginId: string/);
  });

  it('recover_boot 返回键必须是 camelCase（TS RecoveryBootResult 消费）', () => {
    const lib = read('crates/tauron-adapter/src/lib.rs');
    expect(lib).toContain('"phaseName"');
    expect(lib).toContain('"consecutiveFailures"');
    expect(lib).toContain('"safemodeFailures"');
    // 真实持久化接通后新增的四个字段：断链 = 前端读到 undefined。
    expect(lib).toContain('"disabledPlugins"');
    expect(lib).toContain('"requiredPlugins"');
    expect(lib).toContain('"loadSource"');
    expect(lib).toContain('"bootInFlight"');
    expect(lib).toContain('"lastError"');
    // report/trial 的扩展键。
    expect(lib).toContain('"engineAction"');
    expect(lib).toContain('"suspectedPlugin"');
    expect(lib).toContain('"phaseReconcile"');

    const ts = read('packages/tauron-host/src/shell-client.ts');
    expect(ts).toMatch(/phaseName: string/);
    expect(ts).toMatch(/consecutiveFailures: number/);
    expect(ts).toMatch(/health: HealthReport/);
    expect(ts).toMatch(/liveness: 'alive' \| 'dead' \| 'unknown'/);
    expect(ts).toMatch(/readiness: 'ready' \| 'not-ready'/);
    expect(ts).toMatch(/degradation: 'full' \| 'degraded'/);
    expect(ts).toMatch(/disabledPlugins: DisabledPlugin\[\]/);
    expect(ts).toMatch(/requiredPlugins: string\[\]/);
    expect(ts).toMatch(/loadSource: LoadSource/);
    expect(ts).toMatch(/bootInFlight: boolean/);
    expect(ts).toMatch(/lastError: string \| null/);
    expect(ts).toMatch(/suspectedPlugin: string \| null/);
    expect(ts).toMatch(/phaseReconcile: PhaseReconcile/);
  });

  it('i18n 返回键必须是 camelCase（TS I18nState 消费）', () => {
    const lib = read('crates/tauron-adapter/src/lib.rs');
    expect(lib).toContain('"fallbackChain"');
    expect(lib).toContain('"registeredLocales"');
    expect(lib).toContain('"bundleKeys"');
    expect(lib).toContain('"missingTotal"');
    expect(lib).toContain('"loadedKeys"');
    expect(lib).toContain('"newKeys"');

    const ts = read('packages/tauron-host/src/shell-client.ts');
    expect(ts).toMatch(/fallbackChain: string\[\]/);
    expect(ts).toMatch(/registeredLocales: string\[\]/);
    expect(ts).toMatch(/bundleKeys: Record<string, number>/);
    expect(ts).toMatch(/missingTotal: number/);
    expect(ts).toMatch(/loadedKeys: number/);
    expect(ts).toMatch(/newKeys: number/);
  });

  it('i18n_t 全缺失时返回 key 本身而非空串（两侧文档一致，防前端按空串分支）', () => {
    const lib = read('crates/tauron-adapter/src/lib.rs');
    expect(lib).toMatch(/全部缺失时返回 key 本身/);
    const ts = read('packages/tauron-host/src/shell-client.ts');
    expect(ts).toMatch(/全部落空时返回 key 本身/);
    // 旧的「恒返回空串」桩警告必须已经移除，否则文档说谎。
    expect(ts).not.toMatch(/恒返回空串/);
  });

  it('recover 阶段对账必须在宿主侧发生（引擎只判、状态机执行）', () => {
    // 对账缺失 = 引擎进入 safemode 而注册表的 disabledBySafemode 不变，
    // <oc-plugin-manager> 角标会静默失真。
    const lib = read('crates/tauron-adapter/src/lib.rs');
    expect(lib).toMatch(/fn reconcile_recovery_phase/);
    // report 与 trial_enable 两条写路径都必须调用对账。
    expect(lib).toMatch(/fn cmd_recover_report[\s\S]{0,4000}reconcile_recovery_phase\(state\)/);
    expect(lib).toMatch(
      /fn cmd_recover_trial_enable[\s\S]{0,9000}reconcile_recovery_phase\(state\)/,
    );
    // trial_enable 走 D28 `TrialEnable`（清标记 + 记独立试验预算 +
    // trialFromSafemode 置位，供插件自报错误时按 D28 回落）。改回
    // SafemodeExit = 注册表不记预算、D28 回落不可达——必须同时让本门变红。
    expect(lib).toMatch(/fn cmd_recover_trial_enable[\s\S]{0,9000}Event::TrialEnable/);
    expect(lib).not.toMatch(/fn cmd_recover_trial_enable[\s\S]{0,9000}Event::SafemodeExit/);
    // 试启结果必须回传前置对账（诊断 + TS RecoveryTrialResult 契约字段）。
    expect(lib).toMatch(/fn cmd_recover_trial_enable[\s\S]{0,10000}result\["phaseReconcile"\]/);
  });
});

describe('门禁：出站事件（plugin_emit → 前端 listen）', () => {
  /** TS interface 的字段名（顶层，忽略可选标记；容忍 `Interface<T = X>` 泛型参数）。 */
  const tsFields = (src: string, name: string): string[] => {
    const m = src.match(
      new RegExp(`export interface ${name}(?:<[^>]*>)?\\s*\\{([\\s\\S]*?)\\n\\}`),
    );
    if (!m) throw new Error(`${name} not found`);
    return [...m[1]!.matchAll(/^\s{2}(\w+)\??:/gm)].map((x) => x[1]!).sort();
  };

  it('Rust Event 必须是 camelCase 且字段与 TS PluginEvent 逐项一致', () => {
    const rust = rustStruct('crates/tauron-shell/src/eventbus.rs', 'Event');
    expect(
      rust.camelCase,
      'Event 必须 rename_all = "camelCase"，否则前端 event.sourcePlugin 读到 undefined',
    ).toBe(true);

    const camel = [...rust.fields]
      .map((f) => f.replace(/_([a-z])/g, (_m, c: string) => c.toUpperCase()))
      .sort();
    expect(camel).toEqual(tsFields(read('packages/types/src/plugin.ts'), 'PluginEvent'));
  });

  it('出站投递必须用原始 topic 串（否则前端 listen("plugin:<id>:<name>") 永不命中）', () => {
    // 内部总线与出站投递是两条独立通路：topic 必须原样传给 Tauri 事件系统。
    const dispatch = read('crates/tauron-shell/src/dispatch.rs');
    expect(dispatch).toMatch(/self\.sink\.read\(\)\.deliver\(topic, &event\)/);

    const shellCmd = read(`${SHELL}/commands.rs`);
    expect(shellCmd, 'sink 必须真的调用 Tauri 的 app.emit').toContain('app.emit(topic, event)');
  });
});

describe('门禁：应用层命令注册完整性（未注册 = 前端 command not found）', () => {
  it('每个 #[tauri::command] 都出现在 generate_handler! 列表里，且无多余项', () => {
    // 必须**先剥注释**：把某个注册项注释掉要能被判为「未注册」——
    // 否则正则会把 // 里的文本当成已注册，门禁假绿（本门禁首版就是这个问题）。
    const adapter = read('crates/tauron-adapter/src/tauri.rs')
      .replace(/\/\*[\s\S]*?\*\//g, '')
      .replace(/\/\/[^\n]*/g, '');
    const cmds = [
      ...adapter.matchAll(/#\[tauri::command\]\s*pub(?:\(crate\))?\s*(?:async\s+)?fn\s+(\w+)/g),
    ].map((m) => m[1]!);
    // 正则失配保护：命令数为 0 时下面的比对会"假绿"。
    expect(cmds.length, '未解析出命令函数（正则失配）').toBeGreaterThan(20);

    // R1a 之后有**两组**编译期可选集合（底座-only / 全量）：注册项必须对所有列表
    // 取并集。只看第一个列表会误判（本门禁首版即因只切到 substrate 集合而失败）。
    const lists = [...adapter.matchAll(/tauri::generate_handler!\[([\s\S]*?)\]/g)].map(
      (m) => m[1]!,
    );
    expect(lists.length, '未解析出 generate_handler! 列表（正则失配）').toBeGreaterThanOrEqual(2);
    const registered = new Set([
      ...lists.flatMap((l) => [...l.matchAll(/tauri::(\w+)/g)].map((m) => m[1]!)),
      ...[...adapter.matchAll(/\$crate::tauri::(host_registry_install(?:_preview)?),/g)].map(
        (m) => m[1]!,
      ),
    ]);

    const missing = cmds.filter((c) => !registered.has(c));
    expect(missing, `以下命令未注册，前端调用会 command not found：${missing.join(', ')}`).toEqual(
      [],
    );
    expect([...registered].filter((r) => !cmds.includes(r))).toEqual([]);
    expect(registered.size).toBe(cmds.length);
  });

  it('host_capabilities 命令集与底座 / 插件 handler 宏严格同源', () => {
    const rust = read('crates/tauron-adapter/src/lib.rs');
    const parse = (name: string): string[] => {
      // A8-1：不能按字面串 `indexOf('... = &[')` 匹配——rustfmt 会把 `= &[` 折成
      // `=\n    &[`，字面串匹配随即失配（曾让 `PLUGIN_INSTALL_COMMANDS` 解析成 -1，
      // 使本门禁在默认构建下假红）。解析类辅助一律容忍换行，并断言结果非空：
      // 解析到 0 条必须失败，否则正则失配会表现为"空数组 = 一致"的假绿。
      const decl = new RegExp(`pub const ${name}: &\\[&str\\]\\s*=\\s*&\\s*\\[([\\s\\S]*?)\\];`);
      const matched = decl.exec(rust);
      expect(matched, `missing ${name}（正则失配）`).not.toBeNull();
      const entries = [...matched![1]!.matchAll(/"(host_[a-z0-9_]+)"/g)].map((m) => m[1]!);
      expect(entries.length, `${name} 解析出 0 条命令（正则失配）`).toBeGreaterThan(0);
      return entries;
    };
    const substrate = parse('SUBSTRATE_COMMANDS');
    const plugin = parse('PLUGIN_RUNTIME_COMMANDS');
    const optionalInstall = parse('PLUGIN_INSTALL_COMMANDS');
    const handlers = rustHandlerFamilies();
    expect(substrate.length).toBeGreaterThan(30);
    expect(plugin.length).toBeGreaterThan(10);
    expect(new Set(substrate).size).toBe(substrate.length);
    expect(new Set(plugin).size).toBe(plugin.length);
    expect(substrate.sort()).toEqual(handlers.substrate.sort());
    expect([...substrate, ...plugin, ...optionalInstall].sort()).toEqual(handlers.full.sort());
  });

  // 0.4-A2：Rust 侧 feature-gated 的命令，TS 侧不得混进静态能力表。
  //
  // 断链原型：`host_registry_install*` 挂 `#[cfg(feature = "plugin-install")]`，
  // 编译期可关（`default-features = false`）；此前它们被无条件列进
  // `FRAMEWORK_COMMANDS`，于是 `capabilities()` 对它们误报已注册——调用方按能力表
  // 判断"能不能装插件"拿到 `true`，直到 invoke 才 `command not found`。
  // （注：`plugin-install` 是**opt-in** 特性（`default = []`），默认装配不注册；
  //  「静态全集不得硬编码」这条纪律与 feature 的默认值无关——开或关都可能漂移，
  //  真相只允许来自运行期 `host_capabilities`。）
  // 本门禁把「哪些命令是可选的」这份真相钉在两侧之间：集合必须对得上，
  // 且可选命令**不得**出现在静态全集里（只能由 host_capabilities 运行期开门）。
  it('TS 可选命令集与 Rust feature-gated 命令集一致（默认构建不得误报已注册）', () => {
    const rust = read('crates/tauron-adapter/src/lib.rs');
    const installBlock = /pub const PLUGIN_INSTALL_COMMANDS[\s\S]*?\];/.exec(rust)?.[0] ?? '';
    const rustOptional = [...installBlock.matchAll(/"(host_[a-z0-9_]+)"/g)].map((m) => m[1]!);
    expect(rustOptional.length, '未解析出 Rust 侧可选命令（正则失配）').toBeGreaterThan(0);

    const ts = read('packages/tauron-host/src/tauri-backend.ts');
    const tsOptional = [
      ...(
        /export const OPTIONAL_FRAMEWORK_COMMANDS = \[([\s\S]*?)\] as const;/.exec(ts)?.[1] ?? ''
      ).matchAll(/'(host_[a-z0-9_]+)'/g),
    ].map((m) => m[1]!);
    expect(tsOptional.length, '未解析出 TS 侧可选命令（正则失配）').toBeGreaterThan(0);
    expect(tsOptional.sort()).toEqual(rustOptional.sort());

    // 静态全集里不得出现可选命令——出现即回退成"能力表说有、invoke 说没有"。
    const staticList = /const FRAMEWORK_COMMANDS = \[([\s\S]*?)\] as const;/.exec(ts)?.[1] ?? '';
    const staticCmds = [...staticList.matchAll(/'(host_[a-z0-9_]+)'/g)].map((m) => m[1]!);
    expect(staticCmds.length, '未解析出 TS 静态命令全集（正则失配）').toBeGreaterThan(30);
    const leaked = staticCmds.filter((c) => rustOptional.includes(c));
    expect(
      leaked,
      `feature-gated 命令不得出现在静态能力表（否则默认构建误报已注册）：${leaked.join(', ')}`,
    ).toEqual([]);
  });

  it('存在运行期回填能力集的入口（adoptCapabilities ↔ refreshCapabilities）', () => {
    const tb = read('packages/tauron-host/src/tauri-backend.ts');
    expect(/adoptCapabilities\(commands: Iterable<string>\)/.test(tb)).toBe(true);
    const sc = read('packages/tauron-host/src/shell-client.ts');
    expect(/refreshCapabilities\(\)/.test(sc)).toBe(true);
    // 回填必须走 host_capabilities 的返回值，不得回到硬编码清单。
    expect(/adoptRuntimeCapabilities\(this\.backend, caps\.commands\)/.test(sc)).toBe(true);
  });
});

describe('门禁：启动恢复阶段线值（Rust BootPhase::as_str ↔ TS RecoveryBootResult.phase）', () => {
  it('两侧取值集合一致（改名即失败，避免前端按 phase 分支静默失效）', () => {
    const body =
      /impl BootPhase \{([\s\S]*?)\n\}/.exec(read('crates/tauron-recovery/src/lib.rs'))?.[1] ?? '';
    const rustValues = [...body.matchAll(/BootPhase::\w+ => "([a-z]+)"/g)].map((m) => m[1]!);
    // 正则失配保护：解析不到就"假绿"。
    expect(rustValues.length, '未解析出 BootPhase::as_str 取值（正则失配）').toBe(3);

    const union =
      /phase: ([^;]+);/.exec(read('packages/tauron-host/src/shell-client.ts'))?.[1] ?? '';
    const tsValues = [...union.matchAll(/'([a-z]+)'/g)].map((m) => m[1]!);
    expect(tsValues.length, '未解析出 TS phase 联合类型（正则失配）').toBe(3);

    expect([...tsValues].sort()).toEqual([...rustValues].sort());
  });

  it('loadSource 取值集合两侧一致（Rust LoadSource::as_str ↔ TS LoadSource）', () => {
    // 与 phase 同理：前端按 loadSource 区分「持久化没开 / 首次启动 / 恢复 / 损坏」，
    // 改名会静默失效。
    const body =
      /impl LoadSource \{([\s\S]*?)\n\}/.exec(read('crates/tauron-adapter/src/recovery.rs'))?.[1] ??
      '';
    const rustValues = [...body.matchAll(/LoadSource::\w+ => "([a-z]+)"/g)].map((m) => m[1]!);
    expect(rustValues.length, '未解析出 LoadSource::as_str 取值（正则失配）').toBe(4);

    const union =
      /export type LoadSource = ([^;]+);/.exec(
        read('packages/tauron-host/src/shell-client.ts'),
      )?.[1] ?? '';
    const tsValues = [...union.matchAll(/'([a-z]+)'/g)].map((m) => m[1]!);
    expect(tsValues.length, '未解析出 TS LoadSource 联合类型（正则失配）').toBe(4);

    expect([...tsValues].sort()).toEqual([...rustValues].sort());
  });

  it('宿主入口必须打开恢复持久化（否则崩溃检测只在进程内有效，安全模式永不触发）', () => {
    // 断链回归：init()/state_init() 曾用 CommandState::new()（刻意关持久化），
    // 引擎的跨进程计数因此完全失活。两个入口都必须走 with_adapter_config。
    //
    // 轮 7 更新：装配点改名为 `command_state_with_dir_and_config`（因为入口现在
    // 还要承载 AdapterConfig——origin 允许清单等此前对宿主完全不可达）。
    // 断言意图不变：仍在同一个装配点配置、仍读 app_config_dir、仍禁止 new()。
    const src = read('crates/tauron-adapter/src/tauri.rs');
    expect(src).toMatch(/fn command_state_with_dir_and_config/);
    expect(src).toMatch(/with_adapter_config\(/);
    expect(src).toMatch(/app_config_dir\(\)\.ok\(\)/);
    // 轮 8（R1b）更新：装配改为经 `manage_states` 注册**两个** managed 状态
    // （底座 + 插件运行时，共享同一份底座 Arc）。断言意图不变：宿主入口必须走
    // 同一个装配函数（它才打开恢复持久化），而不是各自 manage。
    expect(src).toMatch(/manage_states\(/);
    // 轮 22（V7 §9 `runtime_assembly_singleton`）：装配现在返回
    // `Result<CommandState, AssemblyError>`，所以顺序**反过来**了——结果必须先 `?`
    // 传播，再交给 manage。把装配调用塞进 `manage_states(..)` 里等于让冲突重新回到
    // Tauri 的 state 表（那里只会 panic，不会给接入方一个结构化错误）。
    expect(src).toMatch(
      /command_state_with_dir_and_config\([\s\S]{0,140}\?;\s*manage_states\(app, state\);/,
    );
    expect(src).not.toMatch(/manage_states\([\s\S]{0,80}command_state_with_dir_and_config\(/);
    // 单测用的 new() 不允许出现在宿主入口。
    expect(src).not.toMatch(/app\.manage\(CommandState::new\(\)\)/);
    // 轮 7 收口：`init_with_config` 曾 `manage(CommandState::with_config(..))` 自建
    // 装配，绕过唯一装配点 → 恢复持久化在该入口静默失活。任何入口都不得自建。
    expect(src).not.toMatch(/app\.manage\(CommandState::with_config\(/);
  });

  it('宿主入口必须注册全部恢复/试验/i18n 命令（缺一个 = 前端 command not found）', () => {
    const rust = new Set(rustHostCommands());
    for (const cmd of [
      'host_recover_boot',
      'host_recover_report',
      'host_recover_trial_enable',
      'host_i18n_t',
      'host_i18n_t_params',
      'host_i18n_set_locale',
      'host_i18n_load',
      'host_i18n_stats',
      'host_i18n_cleanup_plugin',
    ]) {
      expect(rust.has(cmd), `Rust 注册宏缺少 ${cmd}`).toBe(true);
    }
  });

  it('0.4-W1：host 不得依赖 @tauron/core，且死编排器 bootstrap() 保持删除', () => {
    // 断链回归（P1-1「活线依赖死线」）：`@tauron/host` 的 `bootstrap.ts` 曾在
    // **真实运行时** import `@tauron/core` 的 PluginRegistry / ConfigManager /
    // EventBus 三个运行时类；而本包声明 `sideEffects: false`，真实构建里这三个类
    // 根本不存在——一旦被用就会崩。处置按 W9-3 取**删除**（它全仓零生产消费者）。
    const pkg = read('packages/tauron-host/package.json');
    expect(pkg, '@tauron/host 不得依赖 @tauron/core（P1-1 回归）').not.toMatch(/"@tauron\/core"/);
    // 全包源码（含测试）都不得再 import core 运行时类。
    const offenders: string[] = [];
    for (const file of hostSrcFiles()) {
      if (/from '@tauron\/core'/.test(read(`packages/tauron-host/src/${file}`))) {
        offenders.push(file);
      }
    }
    expect(offenders, '@tauron/host 源码仍 import @tauron/core（活线依赖死线）').toEqual([]);
    // 死编排器不得悄悄回来：入口不导出 bootstrap，源文件也不存在。
    expect(read('packages/tauron-host/src/index.ts')).not.toMatch(/\bbootstrap\b\s*,/);
    expect(hostSrcFiles(), 'bootstrap.ts 是已删除的死导出，不得回归').not.toContain('bootstrap.ts');
    // 启动上报的真实落点（示例）由下一条门禁钉住；这里只确认它仍在。
    expect(read('examples/minimal-app/src/main.ts')).toMatch(/recoverReport\('success'\)/);

    // P1-5：不得再出现**硬编码的**插件数上限字面量。历史上的第二事实源是
    // `bootstrap.ts` 的 `new PluginRegistry({ maxPlugins: 32 })`——与 Rust
    // `registry.rs` 的 `max_plugins: 8` 互相矛盾（32 ≥ 8 纯属巧合）。
    // 上限的唯一定义在 Rust `RegistryConfig::default()`；TS 只通过
    // `host_capabilities` 读回真实值。
    const hardcoded: string[] = [];
    for (const file of hostSrcFiles()) {
      // 去注释后再查：说明文字里提到 `maxPlugins: 32` 是**记录历史缺陷**，
      // 不是硬编码。代码里的字面量才是第二事实源。
      if (/maxPlugins:\s*\d/.test(stripComments(read(`packages/tauron-host/src/${file}`)))) {
        hardcoded.push(file);
      }
    }
    expect(hardcoded, 'TS 出现硬编码 maxPlugins 字面量（第二事实源回归，P1-5）').toEqual([]);
  });

  it('参考集成示例必须上报启动结果（它是接入方的唯一样板）', () => {
    // 启动上报的唯一落点是示例（`bootstrap()` 已按 0.4-W1 删除）——漏报会让
    // 抄这个示例的接入方把「两次重启进安全模式」当成框架行为。
    const src = read('examples/minimal-app/src/main.ts');
    expect(src, '示例必须上报 host_recover_report').toMatch(/recoverReport\('success'\)/);
    expect(src, '示例必须消费阶段决策而非只发后不管').toMatch(/r\.phase/);
  });

  it('参考集成示例必须实例化 ShellController（壳组件动作的唯一宿主侧消费者）', () => {
    // 断链回归：`ShellController` 曾在生产代码里**零实例化**——只存在于自己的
    // 单测中。后果是 1.0-W3 宣称的「命令面板选中 → 跨主体投递」在唯一可运行的
    // app 里没有入口，而示例自己又写了一份更弱的 `document.addEventListener`
    // 重复接线。本门禁把「控制器必须被示例真实使用」钉死。
    const src = stripComments(read('examples/minimal-app/src/main.ts'));
    expect(src, '示例必须 new ShellController').toMatch(/new ShellController\(/);
    expect(src, '示例必须 start() 控制器（否则监听不生效）').toMatch(/controller\.start\(\)/);
    // 重复接线不得回归：壳组件事件只能由控制器消费。
    expect(
      src,
      '示例不得自行 addEventListener 壳层事件（会与 ShellController 形成第二指挥链）',
    ).not.toMatch(/addEventListener\(\s*'oc-(plugin|command)-/);
    // 命令面板的真实落点：示例必须渲染该组件并喂数。
    expect(read('examples/minimal-app/index.html'), '示例必须渲染 <oc-command-palette>').toMatch(
      /<oc-command-palette>/,
    );
    expect(src, '示例必须用 contributesList 喂命令面板').toMatch(/contributesList\('command'\)/);
  });
});

describe('门禁：断链回归（贡献身份绑定 / 事件取件泵 / 声明式贡献）', () => {
  it('contributes 注册必须绑 label 身份（self 档线形不收 pluginId）', () => {
    // 断链回归：命令曾收 plugin_id 入参、核心盲信 entry.plugin_id——署名可
    // 伪造（并在目标插件卸载时被 clear_plugin 误删），且注册方法挂在主窗
    // 客户端上（self 档命令挂错客户端 = 要么永不成功、要么绕过绑定）。
    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    expect(tauri).toMatch(/fn wire_contributes_register/);
    expect(tauri).toMatch(/fn wire_contributes_register[\s\S]{0,700}resolve_self_identity/);
    expect(tauri).toMatch(/struct ContributeEntryInput/);
    // 注册入口必须在插件侧客户端，主窗客户端只保留读取。
    const host = read('packages/tauron-host/src/host.ts');
    expect(host).toMatch(/async contributesRegister\(/);
    const shell = read('packages/tauron-host/src/shell-client.ts');
    expect(shell).not.toMatch(/async contributesRegister\(/);
  });

  it('0.4-W3：贡献对账闭环（声明 vs 注册必须可检出，且 SDK 真的消费它）', () => {
    // 断链回归（P0-6）：`contributes` 是插件对外承诺的扩展点清单，但过去
    // **没有任何东西比对声明与事实**——声明了却漏注册（入口点了没反应）与
    // 注册了却没声明（来源不明的入口）都无人发现。这条门禁钉住四件事：
    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    const lib = read('crates/tauron-adapter/src/lib.rs');

    // ① 命令真的注册进 handler 宏（否则线上不可达）。
    expect(tauri, 'host_contributes_reconcile 未进注册宏').toMatch(
      /tauri::host_contributes_reconcile/,
    );
    // ② 身份只从 label 解析（对账的是"我自己"，不接受入参 pluginId）。
    expect(tauri).toMatch(/fn host_contributes_reconcile[\s\S]{0,600}resolve_self_identity/);
    // ③ 分叉必须报**专用**错误码，且该码两侧词表都有（追加在末尾）。
    expect(lib, '未定义 E_CONTRIBUTES_DRIFT').toMatch(/ErrorCode::E_CONTRIBUTES_DRIFT/);
    expect(lib, '对账必须比对声明与注册两侧').toMatch(
      /fn cmd_contributes_reconcile[\s\S]{0,2000}difference/,
    );
    const rustErr = read('crates/tauron-host/src/error.rs');
    const tsErr = read('packages/tauron-host/src/errors.ts');
    expect(rustErr, 'Rust 错误码枚举缺 E_CONTRIBUTES_DRIFT').toContain('E_CONTRIBUTES_DRIFT');
    expect(tsErr, 'TS 错误码词表缺 E_CONTRIBUTES_DRIFT').toContain('E_CONTRIBUTES_DRIFT');
    // ④ 必须有**真实消费方**：SDK 激活期对账（否则又是一个"有命令没入口"）。
    const create = read('packages/tauron-app-plugin-sdk/src/createPlugin.ts');
    expect(create, 'SDK 必须在对账命令上留痕（消费它）').toMatch(/contributesReconcile\(/);
    expect(create, '对账失败必须留痕而不是静默').toMatch(/对账发现分叉/);
  });

  it('PluginContext 必须内置取件泵（只订阅不取件 = 投递断链）', () => {
    // 断链回归：宿主总线是拉取模型（host_events_drain）。此前 subscribe 侧
    // 建立后无人取件——帧只进队列，本地订阅者永远收不到。
    const ctx = read('packages/tauron-app-plugin-sdk/src/context.ts');
    expect(ctx, '泵必须取可靠通道（发布落点）').toMatch(/eventsDrain\('request'\)/);
    expect(ctx, '泵必须取 event 通道（深链等框架投递落点）').toMatch(/eventsDrain\('event'\)/);
    expect(ctx, '订阅建立必须起泵').toMatch(/startPump\(\)/);
    expect(ctx, '退订/销毁必须收泵（否则空转 IPC）').toMatch(/stopPump\(\)/);
  });

  it('0.4-A1：入站调用帧词表 TS ↔ Rust 同源，且执行泵必须回填结果', () => {
    // 跨主体调用的取件侧：SDK 泵识别 `:__call` 帧自动执行，后缀必须与 Rust
    // `call_delivery::CALL_TOPIC_SUFFIX` 逐字一致——否则调用帧永远不被识别，
    // 发起方只能等到 TTL（另一类「受理了但永不回帧」断链）。
    const rust = read('crates/tauron-host/src/call_delivery.rs');
    const suffix = /CALL_TOPIC_SUFFIX:\s*&str\s*=\s*"([^"]+)"/.exec(rust)?.[1];
    expect(suffix, 'Rust 侧 CALL_TOPIC_SUFFIX 解析失败').toBeTruthy();

    const ctx = read('packages/tauron-app-plugin-sdk/src/context.ts');
    expect(ctx, `SDK 泵必须识别 \`${suffix}\` 调用帧`).toContain(`'${suffix}'`);
    // 识别之后必须真的回填：不回填 = 发起方等待 TTL，链路只通一半。
    expect(ctx, '执行泵必须调用 reportCallResult 回填结果').toMatch(/reportCallResult\(/);
    // 失败也必须回填（命令不存在 / handler 抛异常），不得静默。
    expect(ctx, '执行失败必须有回填分支').toMatch(/CALL_EXEC_FAILED/);
  });

  it('0.4-轮21：sidecar stdout 必须有读线程在排水（piped 而不读 = sidecar 堵死）', () => {
    // 断链回归：历史实现用 `Stdio::null()` 丢弃 sidecar 输出——宿主永远收不到
    // 任何回帧，「宿主 → sidecar → 回帧 → 结算」这条链断在最后一跳（0.4-A1 修复）。
    // 这条门禁防止有人"简化"回 null，或者只 piped 不排水——后者会让 sidecar 在
    // stdout 缓冲区写满时永久阻塞。注意：注释里提到这些符号是允许的（诚实边界
    // 注释必须能引用历史），故先剥注释再断言。
    const raw = read('crates/tauron-proc/src/spawner.rs');
    const code = raw.replace(/\/\/[^\n]*/g, '');

    // ① stdin 与 stdout 必须 piped（写帧 + 读帧双通道，各至少一次）。
    const pipedCount = [...code.matchAll(/Stdio::piped\(\)/g)].length;
    expect(pipedCount, 'spawn 必须 piped stdin 与 stdout 双通道').toBeGreaterThanOrEqual(2);
    expect(code, '不得用 Stdio::null() 丢弃 sidecar 输出（回帧断链）').not.toMatch(
      /Stdio::null\(\)/,
    );

    // ② 必须有读线程在持续排水（BufReader 逐行 → on_frame 送 sink）。
    expect(code, '必须有读线程（thread::spawn）持续排水 stdout').toMatch(/thread::spawn/);
    expect(code, '读线程必须用 BufReader 逐行排水').toMatch(/BufReader/);
    expect(code, '读到帧必须交给 ProcessFrameSink（on_frame）').toMatch(/on_frame\(/);

    // ③ EOF 必须回收 sink 与 stdin 句柄（句柄表不得随进程退出单调增长）。
    expect(code, 'EOF 必须回收 sink 与 stdin 句柄表').toMatch(/remove\(&pid\)/);
  });

  it('0.4-A2：主推 SDK 必须有非测试的消费方（示例 app 端到端证据）', () => {
    // 断链回归：`@tauron/app-plugin-sdk` 此前只有测试在用（「有 SDK、没用户」）。
    // A2 把示例 app 的插件页切到主推 SDK——这条门禁防止它再退回孤儿状态。
    const pkg = read('examples/minimal-app/package.json');
    expect(pkg, '示例 app 必须声明 app-plugin-sdk 依赖').toMatch(
      /"@tauron\/app-plugin-sdk":\s*"workspace:\*"/,
    );

    // 主推入口 first.ts 必须真用 SDK 的两个核心面：createPlugin（插件定义）
    // + createPluginContext（接到宿主 RPC 面，含执行泵）。
    const first = read('examples/minimal-app/src/plugin/first.ts');
    expect(first, '插件页必须用主推 SDK 的 createPlugin').toMatch(/from '@tauron\/app-plugin-sdk'/);
    expect(first, '必须经 createPluginContext 接宿主（含执行泵）').toMatch(/createPluginContext\(/);
    expect(first, '必须注册声明式命令（注册即开执行泵）').toMatch(/commands:\s*\{/);
    // 主推页不得再回头用 legacy iframe SDK（两套模型混写 = 读者不知道哪条是主路）。
    expect(first, '主推页不得 import legacy 的 @tauron/plugin-sdk').not.toMatch(
      /from '@tauron\/plugin-sdk'/,
    );

    // legacy 实现保留但必须显式标注 deprecated（防止新插件照着旧样子写）。
    const legacy = read('examples/minimal-app/src/plugin/legacy-first.ts');
    expect(legacy, 'legacy 实现必须标注 deprecated').toMatch(/deprecated/);

    // 插件面板窗口页必须存在且进了构建入口（执行泵的宿主载体）。
    read('examples/minimal-app/plugin-window.html');
    const vite = read('examples/minimal-app/vite.config.ts');
    expect(vite, 'plugin-window.html 必须是 vite 构建入口').toMatch(/plugin-window\.html/);

    // 安装入口、窗口入口、Vite 多页产物必须指向同一条插件链。此前示例只登记
    // entry.js，host_window_create 因缺 entry.ui 必然拒绝，执行泵页面永远打不开。
    const app = read('examples/minimal-app/src-tauri/src/main.rs');
    expect(app, '示例安装的插件必须声明可打开的插件窗口页面').toMatch(
      /"entry"\s*:\s*\{\s*"js"\s*:\s*"plugin\.html"\s*,\s*"ui"\s*:\s*"plugin-window\.html"/,
    );
    expect(app, '窗口页面必须进 Vite 构建产物').toContain('plugin-window.html');
    const windowSink = read('crates/tauron-adapter/src/tauri.rs');
    expect(windowSink, '无外部插件安装根时必须从 app asset 加载内置插件 UI').toMatch(
      /if let Some\(install_root\) = state\.install_config_root\(\)[\s\S]*?CustomProtocol[\s\S]*?else\s*\{\s*tauri::WebviewUrl::App\(std::path::PathBuf::from\(&spec\.url\)\)/,
    );
    expect(windowSink, '未启用插件磁盘安装时也必须支持内置 app asset 插件').toMatch(
      /cfg\(not\(feature = "plugin-install"\)\)[\s\S]*?WebviewUrl::App\(std::path::PathBuf::from\(&spec\.url\)\)/,
    );

    // 命令面板需要「插件声明贡献 → 宿主收录 → UI 列出 → 控制器用同一 id 投递 →
    // 插件执行泵找到处理器」闭环；只接上 UI 事件但插件没注册贡献会一直显示空表。
    const main = read('examples/minimal-app/src/main.ts');
    expect(first, '示例插件必须声明命令面板贡献').toMatch(
      /contributes:\s*\{[\s\S]*?commands:\s*\[\{\s*id:\s*FORMAT_COMMAND_ID/,
    );
    expect(first, '贡献命令 id 必须有同名执行处理器').toMatch(
      /commands:\s*\{[\s\S]*?\[FORMAT_COMMAND_ID\]:\s*async/,
    );
    expect(main, '演示跨主体调用必须使用命令面板登记的完整 id').toMatch(
      /callPlugin\('com\.example\.formatter',\s*'formatter\.format'/,
    );
  });

  it('声明式 contributes 必须有激活期注册路径（有类型无调用 = 孤儿配置）', () => {
    // 断链回归：PluginDefinition.contributes 有类型、有文档，此前 activate
    // 的四步全部绕过它——插件作者声明了贡献，宿主永远收不到。
    const create = read('packages/tauron-app-plugin-sdk/src/createPlugin.ts');
    expect(create).toMatch(/def\.contributes/);
    expect(create).toMatch(/contributesRegister\(/);
    // best-effort：重复 id 等宿主错误不得阻断激活。
    expect(create).toMatch(/ctx\.log\.warn\('contributes 注册失败/);
  });
});

describe('门禁：写入侧回扫（通知读写成对 / 能力表全集 / 设置 Tab 喂宿主）', () => {
  it('TauriBackend 能力表必须覆盖 Rust 注册宏全集（否则生产 available() 误报 false）', () => {
    // 断链回归：tauri-backend 此前只列 10 条框架命令——真实宿主上
    // available('host_window_minimize') 等全部误报未注册。
    const rust = rustHostCommands();
    expect(rust.length, '注册宏解析失败（0 条）').toBeGreaterThan(0);
    const backend = read('packages/tauron-host/src/tauri-backend.ts');
    for (const cmd of rust) {
      expect(backend.includes(`'${cmd}'`), `tauri-backend 缺少 ${cmd}`).toBe(true);
    }
  });

  it('通知链必须读写成对，卸载必须回收通知（只写不读 = 通知中心无处取数）', () => {
    const lib = read('crates/tauron-adapter/src/lib.rs');
    expect(lib).toMatch(/fn cmd_notifications_list/);
    expect(lib).toMatch(/fn cmd_notifications_read/);
    // 卸载/清除必须回收该插件的通知（未读计数不得被死条目永久膨胀）。
    const admin = lib.slice(
      lib.indexOf('pub fn cmd_registry_admin('),
      lib.indexOf('pub fn cmd_registry_admin_as('),
    );
    expect(admin).toMatch(/notify_store[\s\S]{0,200}cleanup_plugin/);
    const ts = read('packages/tauron-host/src/shell-client.ts');
    expect(ts).toMatch(/host_notifications_list/);
    expect(ts).toMatch(/host_notifications_read/);
  });

  it('ctx.settings.registerTab 必须同步喂给宿主贡献表（否则设置 Tab 只进本地 Map）', () => {
    // 断链回归：应用设置中心读的是宿主贡献表，本地 Map 只是去重账本。
    const ctx = read('packages/tauron-app-plugin-sdk/src/context.ts');
    expect(ctx).toMatch(/registerTab[\s\S]{0,1200}contributesRegister/);
    expect(ctx).toMatch(/kind: 'settings'/);
  });

  it('autoDownload 后台下载必须吸收 rejection（否则 unhandled rejection 炸进程）', () => {
    const src = read('packages/tauron-host/src/auto-update-client.ts');
    // 只断言**语义**（后台下载挂了 .catch 吸收 rejection），不断言排版：
    // prettier 会把链式调用换行缩进，固定字符窗口会因此假红。
    // `(?!;)` 把搜索限制在同一条语句内——删掉 .catch 后窗口里不会出现别的
    // .catch 来蒙混过关，只会因为撞上语句结尾的 `;` 而变红。
    expect(src).toMatch(/this\.downloadUpdate\(\)(?:(?!;)[\s\S])*\.catch\(/);
    // 桩阶段进度回调不得假装工作：必须如实声明不会被调用。
    expect(src).toMatch(/onProgress[\s\S]{0,240}被回调/);
  });

  it('CLI 帮助不得声称实现里没有的密码学（Ed25519 未接线）', () => {
    // plugin sign 当前是简化 SHA-256 摘要（plugin-lifecycle.ts 自述），
    // 帮助文本曾自称 Ed25519——工具对开发者说谎比缺功能更糟。
    const cli = read('packages/tauron-cli/src/cli.ts');
    expect(cli).not.toMatch(/Ed25519/);
  });

  // ────────────────────────────────────────────────────────────────────────
  // R3：UI / 宿主依赖反转 + 事件契约显式化
  // ────────────────────────────────────────────────────────────────────────

  /** 从契约源码解析 `SHELL_EVENTS` 的 key → 线上事件名映射。 */
  function shellEventContract(): Map<string, string> {
    const src = read('packages/tauron-shell-events/src/index.ts');
    const map = new Map<string, string>();
    for (const m of src.matchAll(/^\s{2}([a-zA-Z]+): '(oc-[a-z-]+)',$/gm)) {
      map.set(m[1]!, m[2]!);
    }
    return map;
  }

  /**
   * 提取某文件里以契约常量派发的所有事件 key。
   *
   * 两种形态都算派发（轮 32）：
   * - `new CustomEvent(SHELL_EVENTS.<key>, …)`：直接按常量派发；
   * - `new CustomEvent(UPDATER_PRIMARY_ACTION[…], …)`：从契约的「状态 → 主按钮
   *   动作」映射派发（更新对话框）。此时真实事件名是那张映射表的值域，因此
   *   **只有在本文件确实这样派发时**才并入映射表的 key——无条件并入等于替映射表
   *   里的动作凭空发明派发方，反向门禁就会放过真孤儿监听。
   */
  function dispatchedKeys(rel: string): string[] {
    const src = read(rel);
    const keys = [...src.matchAll(/new CustomEvent\(SHELL_EVENTS\.([a-zA-Z]+)/g)].map((m) => m[1]!);
    if (/new CustomEvent\(UPDATER_PRIMARY_ACTION\[/.test(src)) {
      keys.push(
        ...[
          ...read('packages/tauron-shell-events/src/index.ts').matchAll(
            /^  [a-z]+: SHELL_EVENTS\.([a-zA-Z]+),$/gm,
          ),
        ].map((m) => m[1]!),
      );
    }
    return keys;
  }

  it('@tauron/ui-primitives 不得依赖 @tauron/host（底座可取组件而不取宿主）', () => {
    // R3 / C1：此前 `@tauron/ui` 依赖 `@tauron/host`，「哑组件靠 props 喂数」是
    // 假象——只想要标题栏的底座项目会把整个宿主客户端层拖进来。
    const pkg = JSON.parse(read('packages/tauron-ui-primitives/package.json')) as {
      dependencies?: Record<string, string>;
      peerDependencies?: Record<string, string>;
    };
    expect(
      pkg.dependencies?.['@tauron/host'],
      'ui-primitives 不得依赖 @tauron/host',
    ).toBeUndefined();
    expect(
      pkg.peerDependencies?.['@tauron/host'],
      'ui-primitives 不得以 peer 依赖 @tauron/host',
    ).toBeUndefined();
    // 运行时依赖白名单：只有零依赖的契约包。
    expect(Object.keys(pkg.dependencies ?? {})).toEqual(['@tauron/shell-events']);
  });

  it('UI 组件不得写字面量 oc-* 事件名（唯一事实源是 @tauron/shell-events）', () => {
    // 隐式字符串契约有先例：三个按钮长期零监听、`oc-updater-check` 是零派发的
    // 孤儿监听。字面量一律禁止，必须走契约常量。
    for (const f of [
      'wc-shell.ts',
      'wc.ts',
      'theme-picker.ts',
      'title-bar.ts',
      'toast.ts',
      'updater-dialog.ts',
      'command-palette.ts',
      'shortcut-recorder.ts',
    ]) {
      const src = read(`packages/tauron-ui-primitives/src/${f}`);
      expect(src, `${f} 出现字面量 oc-* 事件名（应改为 SHELL_EVENTS.*）`).not.toMatch(
        /new CustomEvent\('oc-/,
      );
    }
  });

  it('契约包被派发方与监听方共同依赖（同源才叫契约）', () => {
    expect(read('packages/tauron-ui-primitives/src/wc-shell.ts')).toMatch(
      /from '@tauron\/shell-events'/,
    );
    expect(read('packages/tauron-host/src/shell-controller.ts')).toMatch(
      /from '@tauron\/shell-events'/,
    );
  });

  it('wc-shell 派发的每个契约事件必须在 ShellController 有归属（真实监听或显式登记未接线）', () => {
    // 断链回归：更新对话框的「开始更新/立即重启」与插件管理器的启用开关曾零监听。
    //
    // 判据与 `packages/tauron-host/src/unwired-events.test.ts` **同源**，而不是
    // 「名字在 shell-controller.ts 的文本里出现过」——后者连注释里的名字都算数，
    // 于是 `oc-command-select` 曾靠一句「属接入方域」的注释蒙混过关（命令面板
    // 点了没反应，且没有任何出口可查）。现在只认两种归属：
    //   1) 真实接线：`_listen(elements, SHELL_EVENTS.<key>, …)`
    //   2) 在 `unwired-events.ts` 的 `UNWIRED_EVENTS` 里显式登记（附原因）
    const contract = shellEventContract();
    expect(contract.size, 'ShellController 契约事件解析失败').toBeGreaterThan(8);

    const dispatched = dispatchedKeys('packages/tauron-ui-primitives/src/wc-shell.ts');
    expect(dispatched.length, 'wc-shell 契约派发提取失败').toBeGreaterThan(4);

    // 去掉注释再找接线：注释不算接线。
    const controller = stripComments(read('packages/tauron-host/src/shell-controller.ts'));
    const listenedKeysIn = new Set(
      [...controller.matchAll(/_listen\(elements, SHELL_EVENTS\.([a-zA-Z]+)/g)].map((m) => m[1]!),
    );
    expect(listenedKeysIn.size, 'ShellController 接线解析失败').toBeGreaterThan(3);

    const unwiredKeys = new Set(
      [
        ...read('packages/tauron-host/src/unwired-events.ts').matchAll(
          /name:\s*SHELL_EVENTS\.([a-zA-Z]+)/g,
        ),
      ].map((m) => m[1]!),
    );
    expect(unwiredKeys.size, 'UNWIRED_EVENTS 解析失败').toBeGreaterThan(0);

    for (const key of dispatched) {
      const name = contract.get(key);
      expect(name, `契约里没有 ${key} 这个 key`).toBeDefined();
      expect(
        listenedKeysIn.has(key) || unwiredKeys.has(key),
        `事件 ${name} 在 ShellController 无归属（既没真实接线也没在 UNWIRED_EVENTS 登记）`,
      ).toBe(true);
    }
  });

  it('ShellController 监听的每个契约事件必须有派发方（防孤儿监听）', () => {
    // 反向门禁：`oc-updater-check` 曾反过来是孤儿——控制器监听了它，但更新
    // 对话框根本没有「检查更新」按钮。监听方不得多于派发方。
    const contract = shellEventContract();
    const listened = [
      ...read('packages/tauron-host/src/shell-controller.ts').matchAll(
        /_listen\(elements, SHELL_EVENTS\.([a-zA-Z]+)/g,
      ),
    ].map((m) => m[1]!);
    expect(listened.length, 'Controller 监听提取失败').toBeGreaterThan(3);

    const dispatchedAnywhere = new Set([
      ...dispatchedKeys('packages/tauron-ui-primitives/src/wc-shell.ts'),
      ...dispatchedKeys('packages/tauron-ui-primitives/src/wc.ts'),
      ...dispatchedKeys('packages/tauron-ui-primitives/src/theme-picker.ts'),
    ]);
    for (const key of listened) {
      expect(
        dispatchedAnywhere.has(key),
        `ShellController 监听了 ${contract.get(key) ?? key}，但没有任何组件派发它（孤儿监听）`,
      ).toBe(true);
    }
  });

  it('更新对话框的「稍后」不得复用 oc-close（点一下会把主窗口关掉）', () => {
    // 回归锁：`OcUpdaterDialog` 的「稍后」曾派发 `SHELL_EVENTS.close`，而
    // ShellController 把 `oc-close` 无条件路由到 `windowClose()`——用户点
    // 「稍后」把主窗口关掉了。「稍后」语义是**收起对话框**，走 oc-updater-dismiss。
    const shell = stripComments(read('packages/tauron-ui-primitives/src/wc-shell.ts'));
    // 组件里 `SHELL_EVENTS.close` 只允许出现在标题栏 ✕ 那一处。
    const closeUses = [...shell.matchAll(/SHELL_EVENTS\.close/g)].length;
    expect(closeUses, `wc-shell 里有 ${closeUses} 处 SHELL_EVENTS.close，应只有标题栏 ✕ 一处`).toBe(
      1,
    );
    expect(shell, '「稍后」按钮的点击处理必须是 _dismiss').toMatch(
      /_dismiss\(\)\}\s*>\s*稍后\s*<\/button>/,
    );
    expect(shell, '必须存在 _dismiss 实现').toMatch(/private _dismiss\(\): void \{/);
    expect(shell, '_dismiss 必须派发 oc-updater-dismiss').toMatch(
      /_dismiss\(\): void \{[\s\S]{0,300}SHELL_EVENTS\.updaterDismiss/,
    );
    // 契约里必须真有这条事件，且控制器**不**监听它（收起是组件自身行为）。
    const contract = shellEventContract();
    expect(contract.get('updaterDismiss')).toBe('oc-updater-dismiss');
    expect(
      read('packages/tauron-host/src/shell-controller.ts'),
      '控制器不得监听 oc-updater-dismiss（收起对话框不是宿主命令）',
    ).not.toMatch(/_listen\(elements, SHELL_EVENTS\.updaterDismiss/);
  });

  it('更新状态词表只有一个事实源，「立即重启」只能由 ready 触发（轮 32）', () => {
    // 断链回归：轮 32 之前「更新到哪一步了」有三套平行字面量——契约事件名、
    // `@tauron/host` 的 `UpdateStatus`（安装成功终态 `'ready'`）、`<oc-updater-dialog>`
    // 的渲染分支（安装成功终态 `'done'`）。全仓没有任何生产者产出 `'done'`，于是
    // 「立即重启」在真装配里**永远点不出来**，`oc-restart` → `host_window_relaunch`
    // 整条腿是死的；更糟的是 `'ready'` 落进兜底分支显示「开始更新」，用户点一下会把
    // 下载+安装重跑一遍。组件的 `status` 当时是裸 `string`，漂移无处显形。
    const events = read('packages/tauron-shell-events/src/index.ts');

    // ① 词表：事实源在契约包，取值必须与客户端对外 API 完全一致。
    const list = /export const UPDATER_STATUSES = \[([\s\S]*?)\] as const;/.exec(events);
    expect(list, '契约里没有 UPDATER_STATUSES（状态词表必须有事实源）').not.toBeNull();
    const statuses = [...list![1]!.matchAll(/'([a-z]+)'/g)].map((m) => m[1]!);
    expect(statuses, '状态词表变了：它同时是 host 客户端的对外 API').toEqual([
      'idle',
      'checking',
      'available',
      'downloading',
      'downloaded',
      'installing',
      'ready',
      'error',
    ]);

    // ② 映射：每个状态一个主按钮动作，且重启只属于 ready。
    const body = /export const UPDATER_PRIMARY_ACTION:[\s\S]*?\{([\s\S]*?)\n\};/.exec(events);
    expect(body, '契约里没有 UPDATER_PRIMARY_ACTION（主按钮动作必须有唯一映射）').not.toBeNull();
    const mapped = [...body![1]!.matchAll(/([a-z]+): SHELL_EVENTS\.([a-zA-Z]+)/g)];
    expect(mapped.map((m) => m[1]!).sort(), '映射覆盖的状态与词表不一致').toEqual(
      [...statuses].sort(),
    );
    expect([...new Set(mapped.map((m) => m[2]!))].sort()).toEqual([
      'restart',
      'updateStart',
      'updaterCheck',
    ]);
    expect(
      mapped.filter((m) => m[2] === 'restart').map((m) => m[1]),
      'ready 是唯一能点亮「立即重启」的状态，别的状态点它就是重装一遍',
    ).toEqual(['ready']);

    // ③ 写侧：host 的 UpdateStatus 必须是契约类型的**别名**，不是第二套字面量。
    const client = read('packages/tauron-host/src/auto-update-client.ts');
    expect(client, 'UpdateStatus 必须引自契约词表（UPDATER_STATUSES 派生类型）').toMatch(
      /export type UpdateStatus = UpdaterStatus;/,
    );
    expect(
      client,
      'auto-update-client 里重新声明了状态联合字面量——这就是第二套词表的开端',
    ).not.toMatch(/export type UpdateStatus\s*=[^;]*'/);

    // ④ 读侧：组件状态必须带类型，动作必须从映射取，且派发点唯一。
    const shell = stripComments(read('packages/tauron-ui-primitives/src/wc-shell.ts'));
    expect(shell, '<oc-updater-dialog> 的 status 又变回裸 string 了').toMatch(
      /private _status: UpdaterStatus = 'idle';/,
    );
    expect(
      [...shell.matchAll(/new CustomEvent\(UPDATER_PRIMARY_ACTION\[/g)].length,
      '主按钮派发点必须唯一：多处各写一遍事件名，就会再次各写一遍状态',
    ).toBe(1);
    expect(
      shell,
      '渲染分支不得再写字面量状态——「done/updating」那一版就是没人产出的死分支',
    ).not.toMatch(/'(done|updating)'/);
    expect(
      [...shell.matchAll(/SHELL_EVENTS\.restart/g)].length,
      '组件里 SHELL_EVENTS.restart 只允许出现在「标签由动作推导」那一处',
    ).toBe(1);

    // ⑤ 真消费者：示例页写回的是契约状态，而不是自己发明一套。
    const example = read('examples/minimal-app/src/main.ts');
    expect(example, '示例对话框的 status 又退回裸 string').toMatch(/status: UpdateStatus;/);
    expect(
      example,
      '检查失败必须写契约的 error：显示成「已是最新版本」是把答不了说成没有更新',
    ).toMatch(/info\.degraded === true \? 'error'/);

    // ⑥ 第三套词表（`UpdaterStore` 状态机）没有生产消费者：必须留在孤儿账上。
    // 不静默删除——它随 v1.0.0 发布过，收口或对齐属破坏性变更，需单独批准。
    expect(
      read('contracts/orphan-public-api.json'),
      'UpdaterStore 从孤儿账上消失了（要么真接线，要么重新登记，不能静默失踪）',
    ).toContain('UpdaterStore');
  });

  it('@tauron/host 不得静态引入 UI 包（保持宿主入口 DOM/lit 无关）', () => {
    // host 是轻量客户端层（`sideEffects: false`，node 测试环境可用）。
    // 静态 import/re-export UI 包会把 lit 与全部 DOM 组件拖进每个消费者。
    // 全目录断言（不只查 index.ts）：新增文件里的静态 import 同样违规。
    const offenders = hostSrcFiles().filter((file) => {
      const src = read(`packages/tauron-host/src/${file}`);
      return /^\s*(?:import|export)[^\n]*from '@tauron\/ui(-primitives)?'/m.test(src);
    });
    expect(offenders, '@tauron/host 静态引入 UI 包（会把 lit/DOM 拖进每个消费者）').toEqual([]);
  });

  it('@tauron/host 不得导出无生产调用点的 PluginJsRuntime', () => {
    const idx = read('packages/tauron-host/src/index.ts');
    expect(idx).not.toMatch(/PluginJsRuntime/);
  });

  // ────────────────────────────────────────────────────────────────────────
  // R4：身份主体模型（Principal）
  // ────────────────────────────────────────────────────────────────────────

  it('Backend 契约必须声明 principal()，pluginId() 保留为派生便利方法', () => {
    // R4 / D1：此前身份只有 `pluginId(): string | null`——主窗只能表达为
    // 「null 身份」，它的一等特权来自「没有身份」。主体必须是一等概念。
    const backend = read('packages/tauron-host/src/backend.ts');
    expect(backend, 'Backend 接口缺 principal()').toMatch(/^\s{2}principal\(\): Principal;$/m);
    expect(backend, 'pluginId() 必须保留为派生便利方法').toMatch(
      /^\s{2}pluginId\(\): string \| null;$/m,
    );
    // TS 主体三态必须与 Rust `Principal` 同名对应。
    for (const kind of ["'plugin'", "'main-window'", "'invalid'"]) {
      expect(backend, `Principal 缺 ${kind} 态`).toContain(kind);
    }
    // HostClient 必须把主体与派生 id 都暴露出来。
    const host = read('packages/tauron-host/src/host.ts');
    expect(host, 'HostClient 缺 principal 访问器').toMatch(/get principal\(\): Principal \{/);
    expect(host, 'HostClient.pluginId 必须由 principal 派生').toMatch(
      /get pluginId\(\): string \| null \{[\s\S]{0,200}this\.backend\.principal\(\)/,
    );
  });

  it('pluginId() 实现必须由 principal() 派生（不得各存一份身份）', () => {
    // MockBackend：身份只存一个 Principal 字段，pluginId() 从中派生。
    const backend = read('packages/tauron-host/src/backend.ts');
    expect(backend, 'MockBackend 不得再单存 plugin 字符串').not.toMatch(
      /private readonly plugin: string \| null;/,
    );
    expect(backend).toMatch(/private readonly principalValue: Principal;/);
    // TauriBackend：pluginId() 必须经 principal() 派生，不得绕过它直读 label。
    const tauriBackend = read('packages/tauron-host/src/tauri-backend.ts');
    expect(tauriBackend, 'TauriBackend.pluginId 未由 principal() 派生').toMatch(
      /pluginId\(\): string \| null \{[\s\S]{0,200}this\.principal\(\)/,
    );
    expect(tauriBackend, 'pluginId() 不得绕过 principal() 直接解析 label').not.toMatch(
      /pluginId\(\): string \| null \{[\s\S]{0,200}pluginIdFromLabel/,
    );
  });

  it('Rust Principal 三态齐备，畸形 label 有「不降级为主窗」的回归测试', () => {
    const authz = read('crates/tauron-host/src/authz.rs');
    expect(authz).toMatch(/pub enum Principal/);
    for (const v of ['Plugin(', 'MainWindow,', 'Invalid(String)']) {
      expect(authz, `Principal 缺 ${v}`).toContain(v);
    }
    expect(authz).toMatch(/pub fn resolve_principal\(/);
    // 提权防线必须被测试锁死：畸形 `plugin-` label 归 Invalid 而非 MainWindow。
    expect(authz, '缺「畸形 label 不得降级为主窗」的回归测试').toMatch(
      /fn malformed_plugin_label_is_invalid_not_main_window/,
    );
  });

  it('命令包装器必须经 resolve_principal 解析身份，Invalid 一律拒绝', () => {
    // 回退防线：此前 `host_events_drain` / `host_registry_list` 用裸
    // `strip_prefix("plugin-")`——畸形 label 被当未知身份/主窗放行。
    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    expect(tauri).toMatch(/tauron_host::authz::resolve_principal/);
    const denials = [...tauri.matchAll(/Principal::Invalid/g)].length;
    expect(denials, '命令包装器必须对 Invalid 显式拒绝（≥2 处）').toBeGreaterThanOrEqual(2);
    // 命令体里不得再出现裸前缀解析（`plugin_id_of` 是窗口回收专用，语义不同，
    // 且位于命令区之前，因此这里从 `host_events_drain` 起切片）。
    const commandRegion = tauri.slice(tauri.indexOf('pub fn host_events_drain'));
    expect(commandRegion, '命令体里回退成了裸 strip_prefix 解析').not.toMatch(
      /\.strip_prefix\("plugin-"\)/,
    );
  });

  it('origin ACL 必须落在命令分发的单一咽喉点（而不是逐命令补丁）', () => {
    // R4-D2：45 条命令逐条加校验必然漏，且漏掉的那条不会有任何编译期或门禁期
    // 提示——只会静默裸奔。因此判定必须在唯一分发入口上。
    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    expect(tauri, '缺 origin_gate').toMatch(/fn origin_gate</);
    expect(tauri, '缺 origin_gated_handler 包裹器').toMatch(/pub fn origin_gated_handler</);
    // 两组注册宏（底座-only / 全量）都必须整体经包裹器——这是「新增命令自动受管」
    // 的唯一保证。（R1a 后 `tauron_generate_handler!` 只是别名，本体在
    // `tauron_plugin_handler!`；`tauron_generate_handler!` 自身也必须委托到位。）
    for (const macroName of ['tauron_substrate_handler', 'tauron_plugin_handler']) {
      const macroBody = tauri.slice(tauri.indexOf(`macro_rules! ${macroName}`));
      expect(macroBody.slice(0, 400), `${macroName} 未经 origin_gated_handler 包裹`).toMatch(
        /origin_gated_handler\(tauri::generate_handler!\[/,
      );
    }
    // 判定素材必须全部取自宿主侧（前端自报不参与判定）。
    // 用 `\s*` 容忍链式调用被格式化换行（本门禁锁语义，不锁排版）。
    expect(tauri).toMatch(/invoke\s*\.\s*message\s*\.\s*command\(\)/);
    expect(tauri).toMatch(/invoke\s*\.\s*message\s*\.\s*webview\(\)/);
    expect(tauri).toMatch(/invoke\s*\.\s*message\s*\.\s*state\(\)/);
    // 装配链路：宿主只配置一次（AdapterConfig → shell_ext）。
    expect(read('crates/tauron-adapter/src/lib.rs')).toMatch(
      /origin_allowlist: cfg\.origin_allowlist/,
    );
    // 配置必须**可达**：官方入口若硬编码默认配置，origin 允许清单就是死配置。
    expect(tauri, '缺可配置入口 init_with_adapter_config').toMatch(
      /pub fn init_with_adapter_config\(/,
    );
    expect(tauri, '缺可配置入口 state_init_with_adapter_config').toMatch(
      /pub fn state_init_with_adapter_config\(/,
    );
    expect(tauri, '缺省入口必须复用同一装配点（否则两套装配会漂移）').toMatch(
      /pub fn init\(\)[\s\S]{0,160}init_with_adapter_config\(AdapterConfig::default\(\)\)/,
    );
    expect(tauri, '缺省入口必须复用同一装配点（state_init）').toMatch(
      /pub fn state_init\(\)[\s\S]{0,200}state_init_with_adapter_config\(AdapterConfig::default\(\)\)/,
    );
    // 历史入口（只接注册表配置）必须委托到同一个可配置入口，不得自建装配。
    expect(tauri, 'init_with_config 未委托到 init_with_adapter_config').toMatch(
      /pub fn init_with_config\([\s\S]{0,400}init_with_adapter_config\(/,
    );
    // 策略唯一实现点在 authz：空清单=不启用；哨兵不得被清单绕过。
    const authz = read('crates/tauron-host/src/authz.rs');
    expect(authz).toMatch(/pub fn origin_allowed\(/);
    expect(authz).toMatch(/pub const ORIGIN_UNKNOWN/);
    expect(authz, '缺「哨兵写入清单也不得放行」的回归测试').toMatch(
      /fn unknown_origin_is_rejected_even_if_listed/,
    );
  });

  // 轮 10 / Batch 0-1 + 0-2（F2 + F1）：origin 门在 Production 必须**真的装弹**，
  // 且「声明了身份策略」不等于「门可用」。这三条链路各自都曾被验证为断点，
  // 因此按语义钉死——回归时不需要人记得去看。
  it('Production 的 origin 门必须装弹，且身份策略与门装弹是两个独立事实', () => {
    // ① 策略实现点仍在 host（默认特性就编译，CI 的默认 job 才测得到它）。
    const authz = read('crates/tauron-host/src/authz.rs');
    expect(authz, '缺 production_caller_allowed').toMatch(/pub fn production_caller_allowed\(/);
    expect(authz, '缺「清单为空 = 门未装弹」的拒绝码').toMatch(/ORIGIN_GATE_NOT_ARMED/);
    expect(authz, '缺「未声明 label 不得等同主窗」的拒绝码').toMatch(
      /MAIN_WINDOW_LABEL_NOT_DECLARED/,
    );
    expect(authz, '缺主窗 label 缺省展开').toMatch(/pub fn default_main_window_labels\(/);

    // ② 分发咽喉点必须在 Production 走这条策略，并把宿主侧真实 label 交给它。
    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    const gate = tauri.slice(tauri.indexOf('fn origin_gate<'));
    expect(gate, 'origin 门未按 DeploymentMode 分档').toMatch(
      /DeploymentMode::Production[\s\S]{0,600}production_caller_allowed/,
    );
    expect(gate, 'origin 门未把真实 webview label 交给策略').toMatch(
      /invoke\s*\.\s*message\s*\.\s*webview\(\)\s*\.\s*label\(\)|webview\s*\.\s*label\(\)/,
    );

    // ③ readiness 必须有「门装弹」这个**独立**事实，且 Production 缺它即 fail closed。
    const production = read('crates/tauron-host/src/production.rs');
    expect(production, 'readiness 缺 origin_gate_armed 事实').toMatch(
      /pub origin_gate_armed: bool/,
    );
    expect(production, '缺 ORIGIN_GATE_ARMED_REQUIRED 违规码').toMatch(
      /ORIGIN_GATE_ARMED_REQUIRED/,
    );
    expect(production, 'doctor 缺 origin-gate 检查项').toMatch(/id: "origin-gate"/);

    // ④ 适配器必须由**真实配置**推导该事实，而不是让宿主自己声明。
    const adapter = read('crates/tauron-adapter/src/lib.rs');
    expect(adapter, 'origin_gate_armed 必须由允许清单非空推导').toMatch(
      /origin_gate_armed:\s*!self\.origin_allowlist\.is_empty\(\)/,
    );
    expect(adapter, '主窗 label 集合未从配置装配').toMatch(
      /main_window_labels: cfg\.effective_main_window_labels\(\)/,
    );

    // ⑤ 声明身份策略但未装弹必须有回归测试兜着（否则这条链只靠人记）。
    expect(production, '缺「声明策略≠门装弹」的回归测试').toMatch(
      /fn declared_identity_policy_does_not_arm_the_origin_gate/,
    );
    expect(adapter, '缺适配器侧同口径回归测试').toMatch(
      /fn production_declared_identity_policy_without_allowlist_is_not_ready/,
    );
  });

  // 轮 10 / Batch 0-4（F4）：额度算术只有一个原语，且它有真实生产消费点。
  it('stream credit 必须由 admission::CreditWindow 单一裁决（不得第二套算术）', () => {
    const stream = read('crates/tauron-host/src/stream.rs');
    expect(stream, 'StreamHandle 未使用 CreditWindow').toMatch(/credit:\s*CreditWindow/);
    expect(stream, '流侧残留第二套额度算术').not.toMatch(/credit_bytes/);
    expect(stream, '写帧未经 CreditWindow 扣额').toMatch(/credit\.consume\(/);
    expect(stream, '补额未经 CreditWindow').toMatch(/credit\.grant\(/);
    expect(stream, '缺消费点边界回归测试').toMatch(
      /fn stream_credit_is_arbitrated_by_the_shared_window_at_its_exact_boundary/,
    );
    // FairQueue 是零消费者的死类型，已按「未接线公开 API 台账」删除（A80 公平调度
    // 仍未落，缺额记在方案里而不是记在没人调用的泛型上）。
    const admission = read('crates/tauron-host/src/admission.rs');
    expect(admission, '死类型 FairQueue 复活').not.toMatch(/struct FairQueue/);
    expect(read('crates/tauron-host/src/lib.rs'), 'FairQueue 仍被当作公开 API 导出').not.toMatch(
      /FairQueue/,
    );
  });

  // ────────────────────────────────────────────────────────────────────────
  // 轮 11 / Batch 0-3（F3）：特权管理操作的审计事实，而不是审计开关位
  // ────────────────────────────────────────────────────────────────────────

  it('特权管理操作必须在唯一咽喉点产出结构化审计事实', () => {
    // 旧实现是 `AdapterConfig.admin_audit_available: bool`——宿主自己写一个布尔位
    // 就能让 production 的 `admin-audit` 检查项变绿，而实际一条记录都没有。现在
    // 真值只有一个来源：装配出来的 sink（能落盘 + 哈希链完整 + 无写失败）。
    const audit = read('crates/tauron-host/src/admin_audit.rs');
    expect(audit, '缺审计命令登记表').toMatch(/pub const AUDITED_ADMIN_COMMANDS: &\[&str\] = &\[/);
    expect(audit, '缺 durable schema 常量').toMatch(/ADMIN_AUDIT_SCHEMA: &str = "admin-audit\/1"/);
    expect(audit, '审计日志必须有界').toMatch(/pub const MAX_ADMIN_AUDIT_RECORDS: usize/);
    expect(audit, 'healthy 必须同时要求落盘、链完整、零写失败').toMatch(
      /fn healthy\(&self\) -> bool \{\s*self\.durable && self\.chain_intact && self\.write_failures == 0/,
    );
    expect(audit, '缺链式校验的负向回归').toMatch(/fn tampering_with_a_record_breaks_the_chain/);
    expect(audit, '缺裁剪后仍可验证的回归').toMatch(
      /fn ring_keeps_the_newest_records_and_still_verifies/,
    );

    // 写入点：唯一咽喉函数，且**路由的命令名集合与登记表全等**（多一个=白审计，
    // 少一个=漏审计，两者都必须红）。
    const adapter = read('crates/tauron-adapter/src/lib.rs');
    expect(adapter, '缺 admin_gate 咽喉点').toMatch(/pub fn admin_gate\(/);
    expect(adapter, 'admin_gate 必须复用既有特权判定后再留痕').toMatch(
      /pub fn admin_gate\([\s\S]{0,500}require_main_window\(caller, command\)[\s\S]{0,200}record_admin_audit\(/,
    );
    expect(adapter, '开关位复活：适配器不得再持有 admin_audit_available').not.toMatch(
      /admin_audit_available/,
    );
    const registryStart = audit.indexOf('AUDITED_ADMIN_COMMANDS');
    const registered = [
      ...audit
        .slice(registryStart, audit.indexOf('];', registryStart))
        .matchAll(/"(host_[a-z_]+)"/g),
    ]
      .map((m) => m[1]!)
      .sort();
    const routed = [
      ...adapter.matchAll(/admin_gate\((?:&state\.substrate|state), caller, "(host_[a-z_]+)"\)/g),
    ].map((m) => m[1]!);
    // 一名多点是允许的（`host_registry_install` 有 legacy 与 reviewed 两个入口），
    // 但两侧集合必须全等；且**任何**审计命令都不得再走裸判定——一个入口绕开咽喉点
    // 就等于那条路径不留痕，而集合比对看不出来，所以按出现次数逐条钉。
    expect([...new Set(routed)].sort(), '审计登记表与咽喉点路由的命令必须同源').toEqual(registered);
    expect(registered.length, '审计集不该被清空').toBeGreaterThanOrEqual(4);
    for (const name of registered) {
      expect(adapter, `${name} 有入口绕过 admin_gate（裸 require_main_window）`).not.toMatch(
        new RegExp(`require_main_window\\(caller, "${name}"\\)`),
      );
      expect(
        routed.filter((routedName) => routedName === name).length,
        `${name} 的入口数为 0`,
      ).toBeGreaterThan(0);
    }

    // 读取点：doctor 的检查项由 sink 健康态推导，快照本身也上线。
    expect(adapter).toMatch(
      /audit_for_admin_operations_available =\s*\n?\s*audit\.as_ref\(\)\.is_some_and\(\s*\n?\s*tauron_host::AdminAuditFacts::healthy/,
    );
    expect(adapter, 'doctor 报告未附带审计快照').toMatch(/report\.admin_audit = audit/);
    expect(adapter, '审计目录必须在启动时真的打得开').toMatch(
      /fn validate_for_start\([\s\S]{0,900}AdminAuditSink::open\(dir\)/,
    );
    expect(adapter, '装配期必须打开 sink（不是记一个路径）').toMatch(
      /let admin_audit = cfg\s*\n?\s*\.admin_audit_dir[\s\S]{0,300}AdminAuditSink::open\(dir\)/,
    );
    for (const name of [
      'an_admin_call_leaves_a_structured_audit_fact',
      'a_denied_admin_attempt_is_audited_without_side_effects',
      'doctor_derives_the_admin_audit_check_from_the_live_sink',
      'production_without_an_audit_directory_is_not_ready',
      'admin_audit_survives_a_restart_of_the_host',
      'audited_admin_commands_are_real_dispatched_privileged_commands',
    ]) {
      expect(adapter, `缺审计消费点回归测试 ${name}`).toMatch(new RegExp(`fn ${name}\\(`));
    }

    expect(read('crates/tauron-host/src/production.rs'), '报告缺 admin_audit 快照字段').toMatch(
      /pub admin_audit: Option<crate::admin_audit::AdminAuditFacts>/,
    );
    const host = read('packages/tauron-host/src/host.ts');
    expect(host, 'TS 侧缺 AdminAuditFacts 线形').toMatch(/export interface AdminAuditFacts \{/);
    expect(host, 'TS 报告缺 adminAudit').toMatch(/adminAudit: AdminAuditFacts \| null;/);
  });

  // ────────────────────────────────────────────────────────────────────────
  // 轮 11 / Batch 4'（A84）：已安装内容的摘要必须复核到**每一次服务**
  // ────────────────────────────────────────────────────────────────────────

  it('已安装插件的 asset 读侧只能服务摘要复核通过的字节', () => {
    // 旧状态：入口页在 `installed_plugin_ui` 里做过全目录摘要复核，但入口页加载后
    // 浏览器逐个 GET 的 js/css/图片走的是 asset 协议，那条路径只做 `canonicalize` +
    // `starts_with` 就 `fs::read` 返回——安装完成后篡改磁盘上任意资产，宿主照原样
    // 服务；`ActivationRecord::verify_bytes` 因此是零消费者的死 API。
    const host = read('crates/tauron-host/src/activation.rs');
    expect(host, 'verify_bytes 必须是摘要判定的唯一仲裁者').toMatch(
      /pub fn verify_bytes\(&self, bytes: &\[u8\]\) -> Result<\(\), ActivationError>/,
    );

    const adapter = read('crates/tauron-adapter/src/lib.rs');
    // 密封记录集只能经**一条**认证出口取出（文件形态 + HMAC + generation 同判），
    // 加载侧与读侧共用；再开一条解析路径就是给读侧发一个更松的口径。
    expect(adapter, '缺密封记录集的唯一认证出口').toMatch(/fn load_sealed_activation\(/);
    expect(adapter, 'verify_plugin_ui_activation 必须复用同一出口').toMatch(
      /fn verify_plugin_ui_activation\([\s\S]{0,400}load_sealed_activation\(plugin_dir, config\.acl_signing_key\.as_deref\(\)\)/,
    );
    expect(adapter, 'asset 读侧缺信任根类型').toMatch(/pub struct PluginAssetTrust \{/);
    expect(adapter, '读侧最终判定必须是 verify_bytes').toMatch(/record\.verify_bytes\(bytes\)/);
    expect(adapter, '无密封记录必须拒绝而非放行').toMatch(/没有密封的 activation 记录/);
    expect(adapter, '记录归属必须绑定被请求的插件（挡跨插件重放）').toMatch(
      /record\.resource\.starts_with\(&owner\)/,
    );
    expect(adapter, '宿主密钥不得出现在 Debug 输出').toMatch(
      /fn fmt\(&self, f: &mut std::fmt::Formatter<'_>\)[\s\S]{0,400}bytes redacted/,
    );
    for (const name of [
      'asset_read_path_serves_only_sealed_and_untampered_content',
      'asset_read_path_refuses_a_forged_activation_record',
      'asset_read_path_refuses_unsealed_and_foreign_plugin_content',
      'asset_trust_requires_the_host_sized_key',
    ]) {
      expect(adapter, `缺读侧回归测试 ${name}`).toMatch(new RegExp(`fn ${name}\\(`));
    }

    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    expect(tauri, 'asset 协议必须持有信任根').toMatch(
      /pub fn with_plugin_asset_protocol<R: tauri::Runtime>\(\s*\n\s*builder: tauri::Builder<R>,\s*\n\s*trust: crate::PluginAssetTrust,/,
    );
    expect(tauri, '旧的裸目录入口不得复活').not.toMatch(
      /builder: tauri::Builder<R>,\s*\n\s*root: std::path::PathBuf/,
    );
    expect(tauri, 'read_installed_plugin_asset 必须以信任根为入口').toMatch(
      /fn read_installed_plugin_asset\(\s*\n\s*trust: &crate::PluginAssetTrust,/,
    );
    expect(tauri, 'fs::read 之后缺摘要复核').toMatch(
      /let bytes = std::fs::read\(&target\)[\s\S]{0,400}trust\s*\n\s*\.verify_asset\(plugin_id\.as_str\(\), &rest, &bytes\)/,
    );
    for (const name of [
      'refuses_to_serve_bytes_that_do_not_match_the_sealed_digest',
      'refuses_to_serve_content_without_a_sealed_activation_record',
    ]) {
      expect(tauri, `缺协议级回归测试 ${name}`).toMatch(new RegExp(`fn ${name}\\(`));
    }

    // 装配点：两个真实宿主都必须「没有信任根就不注册协议」，temp-dir 兜底已删。
    for (const file of [
      'examples/minimal-app/src-tauri/src/main.rs',
      'packages/tauron-app-cli/src/scaffold.ts',
    ]) {
      const source = read(file);
      expect(source, `${file} 必须以信任根注册 asset 协议`).toMatch(
        /with_plugin_asset_protocol\(tauri::Builder::default\(\), trust\)/,
      );
      expect(source, `${file} 缺信任根构造入口`).toMatch(
        /fn plugin_asset_trust\(\) -> Option<tauron_adapter::PluginAssetTrust>/,
      );
      expect(source, `${file} 不得再用临时目录兜底安装根`).not.toMatch(
        /std::env::temp_dir\(\)\.join\("tauron-plugins"\)/,
      );
    }

    // CI：这条链只在 tauri + plugin-install 同时开启时才存在，两个分开的 job 都
    // 跑不到它——不补组合步骤，它就会退化成"编译过、没跑过"。
    expect(read('.github/workflows/ci.yml'), 'CI 必须跑 tauri+plugin-install 组合').toMatch(
      /cargo test -p tauron-adapter --features tauri,plugin-install --locked/,
    );
  });

  // ────────────────────────────────────────────────────────────────────────
  // 轮 11 / Batch 4'（A75）：ServiceGraph 只认它真的能判定的那件事
  // ────────────────────────────────────────────────────────────────────────

  it('服务拓扑图不得再抄两份无人执行的序', () => {
    // 旧状态：装配算出 startup/shutdown 两个序存进 `SubstrateState` 的公开字段，
    // 唯一消费者是测试自己——"按拓扑序装配/回收"因此是**读起来像、跑起来不是**。
    // 图的边描述运行期能力依赖（`message` 依赖 `capability` 的审批面），不是构造
    // 顺序；本仓也没有可排序的服务级回收动作。处置：装配期只校验图的有效性，
    // 装饰字段删除。
    const adapter = read('crates/tauron-adapter/src/lib.rs');
    expect(adapter, '拓扑序字段复活（没有执行点的声明）').not.toMatch(
      /service_startup_order|service_shutdown_order/,
    );
    expect(adapter, '装配期必须真的校验图').toMatch(
      /if let Err\(error\) = canonical_substrate_service_graph\(\)\.startup_order\(\) \{\s*\n\s*panic!\("\[tauron\] service dependency graph invalid/,
    );
    expect(adapter, '缺装配期图校验回归测试').toMatch(
      /fn substrate_assembly_validates_the_canonical_service_graph\(/,
    );

    // 唯一真有外部副作用的退出动作（回收 sidecar）在 tauron-proc，且必须走同一条
    // kill 路径——不能再开第二条"只摘表不杀进程"的出口。
    const spawner = read('crates/tauron-proc/src/spawner.rs');
    expect(spawner, '缺退出回收').toMatch(
      /impl Drop for CommandSpawner \{\s*\n[\s\S]{0,500}ProcSpawner::kill\(self, pid\)/,
    );
    expect(spawner, '已失效的"丢弃时不 kill"说明不得复活').not.toMatch(
      /本类型被\*\*丢弃\*\*时不会 `wait`\/`kill`/,
    );
  });

  // ────────────────────────────────────────────────────────────────────────
  // 轮 11 / Batch 3（A81）：撤销的效力必须落到「还在收帧的订阅」
  // ────────────────────────────────────────────────────────────────────────

  it('事件授权撤销必须同时作废既有订阅与已入队帧', () => {
    // 旧状态：`revoke` 只删授权表里的一行，注释还理直气壮地写「撤销只阻止后续新
    // 订阅」。于是管理面点下撤销、宿主返回成功，数据面却继续给同一个订阅者投帧，
    // 队列里撤销前落下的帧也照样被 drain 走——授权与投递完全脱钩。
    const bus = read('crates/tauron-host/src/eventbus.rs');
    expect(bus, '「撤销只阻止后续新订阅」的旧口径不得复活').not.toMatch(
      /撤销只阻止\*\*后续新订阅\*\*/,
    );
    expect(bus, 'revoke 必须在 approvals 临界区内退订既有订阅').toMatch(
      /pub fn revoke\([\s\S]{0,700}approvals\.remove\(&key\)[\s\S]{0,700}self\.unsubscribe\(&token\)/,
    );
    // 只有「他人声明且非公共」的订阅才归审批表管——公共 topic 上的冗余审批被撤销
    // 时不得顺手掐掉合法订阅（与 subscribe 的授权判定同一口径）。
    expect(bus, '撤销级联必须限定在可撤销授权档').toMatch(
      /meta\.publisher != subscriber && !meta\.is_public/,
    );
    expect(bus, '退订后必须作废队列里该 topic 的待取帧').toMatch(
      // 轮 17：`drop_queued` 与返回的 `had_grant` 之间现在夹了一句 grants 回收。
      // 断言的实质是**顺序**（作废必须在返回之前），不是相邻两行——故给出 120 字符
      // 的上限，既容得下插入的一句，也不让文件后段无关的 `had_grant` 蒙过去。
      /self\.drop_queued\(subscriber, topic\)\s*;[\s\S]{0,120}?\n\s*had_grant/,
    );
    expect(
      (bus.match(/self\.reclaim_grant_when_unapproved\(subscriber\);/g) ?? []).length,
      'grants 版本行的回收必须同时挂在 revoke 的两个出口（幂等重试 + 正常撤销）',
    ).toBe(2);
    expect(bus, '幂等重放的 revoke 也必须作废竞态残留帧').toMatch(
      /即使授权行已不存在（幂等重试）也再作废一次队列/,
    );
    expect(bus, '作废帧必须回销字节预算').toMatch(
      /fn remove_topic\(&mut self, topic: &str\) \{[\s\S]{0,400}self\.bytes = self\.bytes\.saturating_sub\(freed\)/,
    );
    for (const name of [
      'revoke_cascades_to_existing_subscriptions_and_queued_frames',
      'revoke_racing_publish_leaves_no_revoked_content_behind',
      'revoke_of_a_redundant_public_approval_leaves_the_subscription_intact',
    ]) {
      expect(bus, `缺撤销效力回归测试 ${name}`).toMatch(new RegExp(`fn ${name}\\(`));
    }

    // 消费点：撤销是主窗特权命令，管理面拿到 `true` 就必须真的停流。
    const adapter = read('crates/tauron-adapter/src/lib.rs');
    expect(adapter, 'host_events_revoke 未走特权咽喉点').toMatch(
      /admin_gate\(state, caller, "host_events_revoke"\)/,
    );
    expect(adapter, '缺订阅侧的撤销效力回归测试').toMatch(
      /subscribed_topics_of\("com\.b", "w1"\)\.is_empty\(\)/,
    );
    expect(adapter, '缺「撤销后提交不再镜像」断言').toMatch(
      /撤销后的提交不得再镜像给已撤销的观察方/,
    );
    expect(adapter, '命令面文档未交代撤销效力').toMatch(/撤销即失效[\s\S]{0,200}待取帧一并作废/);

    // 前端契约：TS SDK 的方法注释必须把「撤销有实效」讲清楚，否则调用方会以为
    // 还得自己补一次 unsubscribe。
    const host = read('packages/tauron-host/src/host.ts');
    expect(host, 'TS 侧未同步撤销语义').toMatch(/既有订阅当场退订[\s\S]{0,120}一并作废/);
    expect(host, 'TS 侧撤销方法签名漂移').toMatch(
      /async eventsRevoke\(subscriber: string, topic: string\): Promise<boolean>/,
    );
  });

  // ────────────────────────────────────────────────────────────────────────
  // 轮 11 / Batch 6（A102）：顺序元数据必须有人在接收端用起来
  // ────────────────────────────────────────────────────────────────────────

  it('A102 顺序契约必须有接收端判定，且两侧语义差异写进文档', () => {
    // 旧状态：宿主在每帧上铸造 `seq`/`sender`/`receiver`，Rust 有 `observe`，TS 有
    // 字段——但**没有任何生产调用方**读它们。于是 at-least-once 的重投与丢帧在业务
    // 视角完全静默，契约只落地了一半。
    const ordering = read('crates/tauron-host/src/ordering.rs');
    expect(ordering, 'Rust oracle 的 observe 不得消失').toMatch(
      /pub fn observe\(&mut self, meta: &OrderedEventMeta\) -> Result<\(\), OrderingError>/,
    );
    expect(ordering, 'oracle 与运行视图的差异必须成文（否则两边会各自"修"成一样）').toMatch(
      /EventOrderingWatcher[\s\S]{0,200}seq \+ 1/,
    );

    const events = read('packages/tauron-host/src/events.ts');
    // 三种判据与 Rust `OrderingError` 一一对应；少一种就是异常被静默归类。
    for (const kind of ['duplicate', 'gap', 'revision-regression']) {
      expect(events, `TS 侧缺 A102 判据 ${kind}`).toMatch(new RegExp(`kind: '${kind}'`));
    }
    expect(events, '缺接收端守卫').toMatch(/export class EventOrderingWatcher \{/);
    expect(events, '守卫必须按帧判定并显式表达正常路径').toMatch(
      /observe\(frame: EventFrame\): EventOrderingViolation \| null \{/,
    );
    // 流键必须带分隔符，否则 ("ab","c") 与 ("a","bc") 会混成同一条流。
    expect(events, '流键分隔符缺失（principal 拼接会串流）').toMatch(
      /`\$\{sender\}\\u0000\$\{receiver\}`/,
    );
    expect(events, '期望序号必须从 1 起（与 Rust issue 同一口径）').toMatch(
      /this\.expectedByStream\.get\(key\) \?\? 1/,
    );
    expect(events, 'gap 之后必须重同步（运行视图不掩后续帧）').toMatch(
      /this\.expectedByStream\.set\(key, frame\.seq \+ 1\)/,
    );
    expect(events, '缺会话重置出口').toMatch(/reset\(\): void \{/);
    expect(events, '判定不得回退已记录的 revision').toMatch(
      /const previous = this\.latestRevisionByStream\.get\(key\);/,
    );

    // 消费点 ①：`toHostRpc` 的取件泵（宿主/业务代码走的传输无关面）。
    const rpc = read('packages/tauron-host/src/rpc.ts');
    expect(rpc, '取件泵没有建立顺序守卫').toMatch(/const ordering = new EventOrderingWatcher\(\)/);
    expect(rpc, '取件泵没有判定帧顺序').toMatch(/const violation = ordering\.observe\(frame\)/);
    expect(rpc, '异常必须有无副作用的默认出口').toMatch(
      /console\.error\(\s*\n?\s*`\[tauron\] 事件顺序契约异常/,
    );
    expect(rpc, '宿主可注入自己的聚合出口').toMatch(/onOrderingViolation\??:/);
    expect(rpc, '收泵时必须清顺序状态').toMatch(/ordering\.reset\(\)/);
    // 只上报不丢帧：一旦有人"顺手"在异常分支里 continue/throw/return，at-least-once
    // 的重投就变成静默丢投递，而丢帧本来就是我们要报的异常——门必须钉在异常分支本体上。
    const violationBranch = (rel: string): string => {
      const src = read(rel);
      const at = src.indexOf('if (violation) {');
      expect(at, `${rel} 缺 \`if (violation)\` 异常分支`).toBeGreaterThan(-1);
      return balanced(src, src.indexOf('{', at));
    };
    expect(
      violationBranch('packages/tauron-host/src/rpc.ts'),
      '取件泵的异常分支不得吞掉投递',
    ).not.toMatch(/\b(continue|throw|return)\b/);
    expect(
      violationBranch('packages/tauron-app-plugin-sdk/src/context.ts'),
      'SDK 泵的异常分支不得吞掉投递',
    ).not.toMatch(/\b(continue|throw|return)\b/);

    // 消费点 ②：插件 SDK 内置泵（插件侧唯一的 host_events_drain 出口）。
    const ctx = read('packages/tauron-app-plugin-sdk/src/context.ts');
    expect(ctx, 'SDK 泵没有顺序守卫').toMatch(/const ordering = new EventOrderingWatcher\(\)/);
    expect(ctx, 'SDK 泵没有判定帧顺序').toMatch(/const violation = ordering\.observe\(frame\)/);
    expect(ctx, 'SDK 侧异常必须走插件日志').toMatch(/ctx\.log\.warn\(\s*\n?\s*`事件顺序契约异常/);
    expect(ctx, 'SDK 收泵必须清顺序状态').toMatch(
      /const stopPump = \(\): void => \{[\s\S]{0,400}ordering\.reset\(\)/,
    );

    // 回归测试面：判定语义 + 两个消费点各自都要有测试，缺一个就等于没接线。
    expect(read('packages/tauron-host/src/events.test.ts'), '缺守卫单测').toMatch(
      /describe\('EventOrderingWatcher（A102 接收端）'/,
    );
    expect(read('packages/tauron-host/src/rpc.test.ts'), '缺取件泵 A102 测试').toMatch(
      /describe\('HostRpc 取件泵的 A102 判定'/,
    );
    expect(
      read('packages/tauron-app-plugin-sdk/src/events.test.ts'),
      '缺 SDK 泵 A102 测试',
    ).toMatch(/取件泵判定 A102 丢帧并经 ctx\.log\.warn 暴露/);
  });

  // ────────────────────────────────────────────────────────────────────────
  // 轮 11 / Batch 7（A91 × A104）：故障闸门不得替事务代持锁
  // ────────────────────────────────────────────────────────────────────────

  it('设置族闸门必须只判就绪，单写者边界归写租约且有行为证明', () => {
    // 旧状态：`run_settings_boundary` 用 `FaultBoundary::run`，边界锁整段扣在闭包上，
    // 而闭包含磁盘写。后果是**文档与实现互相矛盾**：`settings_write_lock` 的注释说它
    // 是单写者事务边界，实测把它的 `_write` 换成 `None`，并发写测试仍然全绿——闸门替它
    // 把一切串好了，只读命令也被排在别人的磁盘写之后。
    const lib = read('crates/tauron-adapter/src/lib.rs');
    expect(lib, '闸门不得再把边界锁跨在闭包上（那等于用故障状态当串行化点）').not.toMatch(
      /settings_fault\.lock\(\)\.run\(/,
    );
    expect(lib, '闸门缺就绪判定').toMatch(
      /state\.settings_fault\.lock\(\)\.ensure_ready\(\)\.map_err\(settings_fault_to_host_error\)\?;/,
    );
    expect(lib, 'panic 必须事后登记（否则 A91 的隔离语义丢失）').toMatch(
      /state\.settings_fault\.lock\(\)\.record_panic\(operation, payload\)/,
    );

    // 写租约是**唯一**的事务边界，且四个写点都得持它。
    const leaseSites = lib.match(/let _write = state\.settings_write_lock\.lock\(\);/g) ?? [];
    expect(leaseSites.length, '写租约消费点数量漂移（set/adopt/migrate/reconcile）').toBe(4);
    expect(lib, '未说明租约为何故意横跨落盘').toMatch(/写租约故意横跨落盘/);
    expect(lib, '未交代 store 互斥量的临界区边界').toMatch(/互斥量\*\*绝不\*\*进入/);
    expect(lib, '原子写的共享 tmp 名是租约存在的理由，必须成文').toMatch(
      /单写者不是性能选项，是这段代码的正确性前提/,
    );
    expect(lib, '缺单写者事务的行为证明测试').toMatch(
      /fn concurrent_settings_writes_never_lose_a_key_or_share_a_generation\(\)/,
    );

    // 拆 API 的一方也要有理由：`run` 的锁跨闭包是类型层面强制的，只能新增两段式出口。
    const fault = read('crates/tauron-host/src/fault.rs');
    expect(fault, '缺 record_panic 出口').toMatch(/pub fn record_panic\(/);
    expect(fault, 'run 必须复用 record_panic（两份记账迟早漂移）').toMatch(
      /Err\(self\.record_panic\(operation, payload\)\)/,
    );
    expect(fault, '未写明锁跨 I/O 会把闸门变成串行化点').toMatch(/事实上的串行化点/);
  });

  // ────────────────────────────────────────────────────────────────────────
  // 轮 11 / Batch 4（A101）：迁移回滚快照必须是磁盘事实，且有人在重启后消费它
  // ────────────────────────────────────────────────────────────────────────

  it('A101 迁移回滚镜像必须落盘、先于新文档写、并在重启时被消费', () => {
    // 旧状态：`MigrationContract.requires_snapshot` 只是一个形容词。快照只活在内存里
    // （`MigrationReceipt.before`，私有字段且不覆盖其它命名空间），进程重启即消失；
    // `durable.rs` 那个通用 `MigrationSnapshot<T>` 零消费者。于是「迁移可回滚」在最需要
    // 它的场景——迁移后正式文档写坏、下一次启动才暴露——一条都不成立。
    const lib = read('crates/tauron-adapter/src/lib.rs');
    // 定义锚定在列 0：装配段（`load_settings_rollback_image(&…)`）在文件里出现在定义
    // 之前，不锚定就会把调用点当成函数体。
    const fnBody = (src: string, name: string): string => {
      const at = src.search(new RegExp(`^(?:pub )?fn ${name}\\(`, 'm'));
      expect(at, `找不到函数 ${name}`).toBeGreaterThan(-1);
      const end = src.indexOf('\n}\n', at);
      expect(end, `${name} 的函数体边界未找到`).toBeGreaterThan(at);
      return src.slice(at, end);
    };

    // ① 镜像是**独立的磁盘事实**：自己的文件名 + 自己的 durable schema。
    expect(lib, '缺 A101 回滚镜像文件名常量').toMatch(
      /pub const HOST_SETTINGS_ROLLBACK_FILE: &str = "host-settings\.rollback\.json";/,
    );
    expect(lib, '缺 A101 回滚镜像 schema').toMatch(
      /const HOST_SETTINGS_ROLLBACK_SCHEMA: &str = "tauron\.host-settings-rollback\/1";/,
    );

    // ② 迁移链的顺序即正确性：取快照 → 迁移 → 先写镜像 → 再写正式文档。
    const migrate = fnBody(lib, 'cmd_settings_migrate');
    expect(migrate, '迁移前没有取用户层快照').toMatch(
      /let before = state\.settings\.lock\(\)\.snapshot_all\(\);/,
    );
    expect(migrate.indexOf('snapshot_all'), '快照必须在迁移事务之前取').toBeLessThan(
      migrate.indexOf('host_settings_migrate_transaction'),
    );
    const stageAt = migrate.indexOf('stage_settings_rollback_image(');
    const persistAt = migrate.indexOf('persist_settings_doc(');
    expect(stageAt, '迁移没有把回滚镜像落盘').toBeGreaterThan(-1);
    expect(persistAt, '迁移没有落盘正式文档').toBeGreaterThan(-1);
    expect(stageAt, '回滚镜像必须先于正式文档落盘').toBeLessThan(persistAt);
    expect(migrate, '未说明这个顺序为什么不能反').toMatch(/顺序不能反/);
    expect(migrate, 'requires_snapshot 没有消费点（合同仍是形容词）').toMatch(
      /receipt\.contract\(\)\.requires_snapshot/,
    );
    // 三条失败路径都必须 rewind 内存；落盘失败那条还得作废镜像。
    // （轮 59 加了第三条出口：容量键解释不了时在任何写动作之前退回。）
    expect(
      (migrate.match(/rollback_migration\(receipt\)/g) ?? []).length,
      '迁移失败路径的 rewind 出口漂移（容量校验失败 / 镜像失败 / 落盘失败）',
    ).toBe(3);
    expect(migrate, '落盘失败后仍留着镜像 = 在磁盘上伪造一次没发生过的迁移').toMatch(
      /clear_settings_rollback_image\(&path\)/,
    );

    // ③ 镜像自带信封：恢复只消费**已验证**的数据，这也是生产档敢用它启动的理由。
    const stage = fnBody(lib, 'stage_settings_rollback_image');
    expect(stage, '镜像未密封进 durable 信封').toMatch(
      /DurableEnvelope::seal\(\s*HOST_SETTINGS_ROLLBACK_SCHEMA/,
    );
    expect(stage, '镜像未走原子写').toMatch(
      /atomic_write_settings_file\(&path, &bytes, "设置回滚镜像"\)/,
    );
    const load = fnBody(lib, 'load_settings_rollback_image');
    expect(load, '镜像读取未做完整性校验').toMatch(/decode_durable/);
    expect(load, '镜像 schema 未校验').toMatch(/envelope\.schema != HOST_SETTINGS_ROLLBACK_SCHEMA/);
    expect(load, '「没有镜像」与「镜像坏了」必须是两种答案').toMatch(
      /ErrorKind::NotFound => return Ok\(None\)/,
    );

    // ④ 消费点在装配路径上，且**一次性**：消费即作废；健康启动也必须回执掉镜像。
    const askAt = lib.indexOf('path.with_file_name(HOST_SETTINGS_ROLLBACK_FILE)');
    expect(askAt, '启动装配没有问回滚镜像（磁盘快照仍是孤儿）').toBeGreaterThan(-1);
    expect(lib.slice(askAt, askAt + 1200), '镜像消费后未作废（一次性语义丢失）').toMatch(
      /clear_settings_rollback_image\(&rollback_path\)/,
    );
    expect(lib, '按镜像恢复必须如实记录，不能静默改写用户数据').toMatch(/已按 A101 从迁移回滚镜像/);
    // 轮 16 R3 改判后镜像有三个作废出口：装配期按镜像恢复之后、迁移落盘失败时、
    // **健康启动读到好数据时的回执**。第三个出口是语义变更的核心——镜像的保护窗
    // 是「迁移提交 → 首次证明可读」，一旦读到就说明新状态可用，再留着它只会在
    // 下次损坏时把用户回滚到迁移前的旧版本。
    // 轮 17：判定面限定在**生产区**（`mod tests` 之前）。轮 16 的口径把整文件都数进
    // 去，于是「给作废函数补一个单测」会让消费点计数从 3 变 5 —— 门禁红得毫无道理，
    // 而这种红教人的是「别给生产语义加测试」。测试里的调用不是消费点。
    const testsAt = lib.search(/^#\[cfg\(test\)\]\r?\nmod tests \{$/m);
    if (testsAt < 0) throw new Error('adapter lib.rs 的测试模块锚点消失，消费点计数失去边界');
    const productionLib = lib.slice(0, testsAt);
    expect(
      (productionLib.match(/clear_settings_rollback_image\(&/g) ?? []).length,
      '作废出口的消费点漂移（装配消费 + 迁移落盘失败 + 健康启动回执）',
    ).toBe(3);

    // ⑤ 救不回来时仍是原来的 fail-closed 口径：生产档 panic 前缀逐字不改（外部日志/监控
    // 按它聚合），且镜像也坏时两半原因都要报。
    const report = fnBody(lib, 'report_settings_load_failure');
    expect(report, '生产档 panic 前缀漂移').toMatch(
      /\[tauron\] production settings durable-state validation failed for/,
    );
    expect(report, '镜像也不可用时原因被吞掉（半份诊断）').toMatch(/回滚镜像也不可用/);
    expect(
      (lib.match(/report_settings_load_failure\(/g) ?? []).length,
      '降级出口漂移（定义 + 无镜像 + 镜像也坏）',
    ).toBe(3);

    // ⑥ 死结构不得复活，去向必须在模块 doc 里说清。
    expect(lib, '通用 MigrationSnapshot 复活了').not.toMatch(/MigrationSnapshot/);
    const durable = read('crates/tauron-host/src/durable.rs');
    expect(durable, 'durable.rs 的 MigrationSnapshot 复活了').not.toMatch(
      /pub struct MigrationSnapshot/,
    );
    expect(durable, '删除未登记去向（下一轮会再写一遍）').toMatch(/零消费者/);
    expect(read('crates/tauron-host/src/lib.rs'), 'MigrationSnapshot 仍在 re-export').not.toMatch(
      /MigrationSnapshot/,
    );

    // ⑦ 重启级 E2E：光有单元测试证明不了「下一次启动」这条路。
    expect(lib, '缺进程重启级回滚 E2E').toMatch(
      /fn a101_rollback_image_restores_pre_migration_state_after_a_restart\(\)/,
    );
  });

  // ────────────────────────────────────────────────────────────────────────
  // 轮 11 / 发布链：版本号只有校验器、没有生产者
  // ────────────────────────────────────────────────────────────────────────

  it('版本号的写入侧必须存在，且与 release.yml 的 version-check 读同一组落点', () => {
    // 旧状态：`release.yml` 会比对六处版本号并要求它们等于 tag，但仓内**没有任何一手**
    // 把它们写齐（`scripts/version-bump.mjs` 与 `tauron-build-tools` 都已不在仓里）。
    // 校验器读得到漂移、生产者不存在，就等价于「发版靠人肉记住六个文件」——
    // release.yml 自己的注释里就记着那次「tag v0.2.0、安装包却写 0.1.0」的事故。
    const release = read('.github/workflows/release.yml');
    const sync = read('scripts/version-sync.mjs');
    const pkg = JSON.parse(read('package.json')) as { scripts: Record<string, string> };
    expect(pkg.scripts['version:check'], '缺 pnpm version:check（CI 可用的只判定入口）').toBe(
      'node scripts/version-sync.mjs --check',
    );
    expect(pkg.scripts['version:sync'], '缺 pnpm version:sync（发版时的写入口）').toBe(
      'node scripts/version-sync.mjs --set',
    );

    // 落点集合两侧必须同源：少一类就是「校验器看得见、生产者写不到」。
    for (const rel of [
      'package.json',
      'Cargo.toml',
      'examples/minimal-app/package.json',
      'examples/minimal-app/src-tauri/Cargo.toml',
      'examples/minimal-app/src-tauri/tauri.conf.json',
      'packages',
    ]) {
      expect(release, `release.yml 的 version-check 不再读 ${rel}`).toContain(rel);
      expect(sync, `version-sync 不再写 ${rel}`).toContain(rel);
    }
    // `packages/*` 走目录枚举，新增包不需要有人记得改脚本。
    expect(sync, 'version-sync 未枚举 packages/*').toMatch(
      /readdirSync\(join\(ROOT, 'packages'\)\)/,
    );
    expect(release, 'release.yml 未枚举 packages/*').toMatch(/readdirSync\("packages"\)/);
    // 事实源口径必须成文：否则下一个人会去改 Cargo.toml，而校验器按根 package.json 比对。
    expect(sync, '未写明唯一事实源').toMatch(/根 package\.json 的 version 是唯一事实源/);
  });

  it('文档里的行号引用是可执行断言，且核对脚本接进了 CI', () => {
    // 断链形态：文档写 `eventbus.rs:597`／`drop_queued:634` 这类定位，代码一改数字就漂。
    // 漂掉的引用比不写引用更糟——读者跳过去看到的是无关代码，于是文档宣称的"证据"
    // 悄悄变成虚构。轮 11 实测：一次 +1330 行的改动让缺口方案里 5 条引用指错位置，
    // 其中 A97/F3 两行连文件都对不上（`adapter/lib.rs` 这种缩写路径不在任何真实路径里）。
    const ci = read('.github/workflows/ci.yml');
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(
      existsSync(resolve(workspaceRoot, 'scripts/check-doc-line-refs.mjs')),
      '缺核对脚本',
    ).toBe(true);
    const pkg = JSON.parse(read('package.json')) as { scripts: Record<string, string> };
    expect(pkg.scripts['docs:check'], '缺 pnpm docs:check').toBe(
      'node scripts/check-doc-line-refs.mjs',
    );
    expect(ci, 'CI 未调用 docs:check').toContain('pnpm docs:check');
    // 脚本必须核对**符号级**引用（只查行数边界等于没查：漂移绝大多数是语义漂移）。
    const checker = read('scripts/check-doc-line-refs.mjs');
    expect(checker, '核对脚本不再验证符号落在被指行上').toMatch(/指不到该符号/);
    expect(checker, '核对脚本丢失默认文件集').toMatch(/v4-industrial-gap-closure-plan\.md/);
    // 修法口径成文：否则下一个人只会把数字改对，下次改动又漂。
    expect(plan, '未成文「引用优先符号锚定」口径').toMatch(/符号锚定/);
    expect(plan, '缺口方案未把 docs:check 写进复核命令').toMatch(/pnpm docs:check/);
  });

  // ────────────────────────────────────────────────────────────────────────
  // 轮 22（V7 §9）：五条语义门禁自己也需要链路证明
  // ────────────────────────────────────────────────────────────────────────

  it('V7 §9 语义门禁必须存在、有 pnpm 入口、且被 CI 调用', () => {
    // 门禁脚本最容易烂成的形态不是「报错」，而是**没人调用**：源码里那条不变量
    // 被改回去时，CI 依然是绿的。这里钉的是「脚本存在 + pnpm 入口 + CI 步骤 +
    // 聚合入口」四件，缺任何一件即红。
    const gates: Array<[string, string, string]> = [
      [
        'gate:assembly-singleton',
        'scripts/check-runtime-assembly-singleton.mjs',
        'V7 §9 runtime_assembly_singleton',
      ],
      [
        'gate:wasm-generation',
        'scripts/check-wasm-loaded-generation.mjs',
        'V7 §9 wasm_loaded_generation',
      ],
      ['gate:upgrade-hash', 'scripts/check-upgrade-file-hash.mjs', 'V7 §9 upgrade_file_hash'],
      [
        'gate:success-effect',
        'scripts/check-success-requires-effect.mjs',
        'V7 §9 success_requires_effect',
      ],
      [
        'gate:simulated-commits',
        'scripts/check-simulated-never-commits.mjs',
        'V7 §9 simulated_never_commits',
      ],
      [
        'gate:domain-ownership',
        'scripts/check-adapter-domain-ownership.mjs',
        'V7 §10-9 adapter domain ownership 冻结',
      ],
    ];
    const pkg = JSON.parse(read('package.json')) as { scripts: Record<string, string> };
    const ci = read('.github/workflows/ci.yml');
    const aggregate = pkg.scripts['gates:check'] ?? '';
    for (const [script, file, label] of gates) {
      expect(existsSync(resolve(workspaceRoot, file)), `缺门禁脚本 ${file}`).toBe(true);
      expect(pkg.scripts[script], `缺 pnpm ${script}（${label}）`).toBe(`node ${file}`);
      expect(ci, `CI 未调用 ${file}（${label}）`).toContain(`run: node ${file}`);
      expect(aggregate, `${script} 未纳入 gates:check 聚合入口`).toContain(file);
    }
    // 所有权台账必须**两类形状都登记**：只钉 `impl` 的话，往 `lib.rs` 追加自由函数
    // （本仓 `cmd_*` 的主体形状）完全不受约束——这条断言是轮 24 落地时实测出来的。
    const ownership = JSON.parse(read('contracts/adapter-domain-ownership.json')) as {
      entries: Array<{ kind: string; key: string; domain: string; owners: string[] }>;
      fileBudgets: Array<{ file: string; maxItems: number }>;
    };
    const kinds = new Set(ownership.entries.map((e) => e.kind));
    expect(kinds.has('impl'), '台账缺少 impl 类条目').toBe(true);
    expect(kinds.has('fn'), '台账缺少顶层 fn 类条目（cmd_* 就是这一类）').toBe(true);
    expect(ownership.fileBudgets.length, '台账没有文件级条目预算').toBeGreaterThan(0);
    // 门禁自证（变异夹具）也得真的被调用，否则「规则空转」只能靠人记得去验。
    expect(aggregate, '门禁自证未纳入 gates:check').toContain(
      'node scripts/check-adapter-domain-ownership.mjs --self-test',
    );
    expect(ci, 'CI 未调用门禁自证').toContain(
      'run: node scripts/check-adapter-domain-ownership.mjs --self-test',
    );
    // 共享扫描原语必须抹平**模板字面量**：轮 22 的反向验证里，CLI 那句
    // `` `…simulated: true…` `` 提示语让诚实性门禁自己变绿了——文档字符串不能充当代码证据。
    expect(read('scripts/lib/gate-scan.mjs'), 'gate-scan 不再抹平模板字面量').toMatch(/inTemplate/);
  });

  // ────────────────────────────────────────────────────────────────────────
  // 轮 12：命令面的**全量**接口参考必须是生成物
  // ────────────────────────────────────────────────────────────────────────

  it('命令面接口参考必须由代码生成并覆盖全部命令（文档覆盖＝代码集合）', () => {
    // 断链形态（轮 12 实测）：`app-layer-wire.md` 只有 49/85 条命令的线格式，
    // 而插件指南把那句"完整线格式见 …"当作事实源指过去——读者按文档找不到 36 条命令
    // 的形参与返回，等于接口文档缺一半却没人知道。**手写清单挡不住这种缺**，
    // 因为只有"生成物 + 复算"才能让"漏一条"变成红灯。
    const gen = 'scripts/generate-command-surface.mjs';
    const doc = 'docs/api/command-surface.md';
    expect(existsSync(resolve(workspaceRoot, gen)), `缺生成器 ${gen}`).toBe(true);
    expect(existsSync(resolve(workspaceRoot, doc)), `缺生成物 ${doc}`).toBe(true);

    const pkg = JSON.parse(read('package.json')) as { scripts: Record<string, string> };
    expect(pkg.scripts['command-surface:gen'], '缺 pnpm command-surface:gen').toBe(`node ${gen}`);
    expect(
      pkg.scripts['command-surface:check'],
      '缺 pnpm command-surface:check（CI 复算入口）',
    ).toBe(`node ${gen} --check`);
    expect(read('.github/workflows/ci.yml'), 'CI 未调用 command-surface:check').toContain(
      'pnpm command-surface:check',
    );

    // ① 表里的命令集合必须与代码**按名集合**相等：多一条 = 文档写了不存在的命令，
    //    少一条 = 接入方查不到形参/返回。两条都是断链（口径同 A69：比对集合不比对顺序）。
    const defined = [
      ...new Set(
        [
          ...read('crates/tauron-adapter/src/tauri.rs').matchAll(
            /#\[tauri::command\]\s*\npub fn (host_[a-z0-9_]+)/g,
          ),
        ].map((m) => m[1]),
      ),
    ].sort();
    const surface = read(doc);
    const documented = [
      ...new Set(
        surface
          .split(/\r?\n/)
          .filter((l) => l.startsWith('| `host_'))
          .map((l) => l.match(/^\| `(host_[a-z0-9_]+)`/)![1]),
      ),
    ].sort();
    expect(documented.length, '生成的表里一条命令都没解析到').toBeGreaterThan(80);
    const missing = defined.filter((c) => !documented.includes(c));
    const ghost = documented.filter((c) => !defined.includes(c));
    expect(missing, `命令面参考缺这些命令：${missing.join(' ')}`).toEqual([]);
    expect(ghost, `命令面参考写了不存在的命令：${ghost.join(' ')}`).toEqual([]);

    // ② 三个编译期集合的条数必须写进文档，且与代码一致——否则"全量"又是宣称。
    const lib = read('crates/tauron-adapter/src/lib.rs');
    const count = (name: string) => {
      const m = lib.match(new RegExp(`pub const ${name}:[^=]*=\\s*&?\\[([\\s\\S]*?)\\];`));
      expect(m, `找不到命令数组 ${name}`).toBeTruthy();
      return [...(m![1] ?? '').matchAll(/"([^"]+)"/g)].length;
    };
    for (const [name, label] of [
      ['SUBSTRATE_COMMANDS', '底座'],
      ['PLUGIN_RUNTIME_COMMANDS', '插件运行时'],
      ['PLUGIN_INSTALL_COMMANDS', '插件安装'],
    ] as const) {
      expect(surface, `计数口径表缺「${label} ${name}」`).toContain(`${name}\` | ${count(name)} |`);
    }

    // ③ 全量参考必须**反向**成为链路证明：任何一行命令没有前端落点即红。
    //    （判定只看表格行——"读表须知"里解释这个标记的含义时也会提到它。）
    const orphans = documented.filter((c) => {
      const row = surface.split(/\r?\n/).find((l) => l.startsWith(`| \`${c}\``));
      return !!row && row.includes('**缺（孤儿命令）**');
    });
    expect(orphans, `这些命令后端有注册、packages 无人调用：${orphans.join(' ')}`).toEqual([]);
    expect(surface).toContain('前端无人调用**的孤儿命令：**0');

    // ④ 两份人写文档必须把"全量"这个期望交给生成物，不再把 49/85 的文档称作完整清单。
    const guide = read('docs/api/plugin-development-guide.md');
    const wire = read('docs/architecture/app-layer-wire.md');
    expect(guide, '指南未指向 command-surface.md').toContain('docs/api/command-surface.md');
    expect(guide, '指南仍把 app-layer-wire 称作完整线格式').not.toContain(
      '完整线格式见 `docs/architecture/app-layer-wire.md`',
    );
    expect(wire, 'wire 文档未声明本节只覆盖关键命令').toMatch(/只覆盖「关键」命令|不是全量清单/);
    expect(wire, 'wire 文档未指向 command-surface.md').toContain('docs/api/command-surface.md');

    // ⑤ 判定列必须与代码同源，且**不许把没判定写成有判定**。
    //    轮 12 的真实缺陷方向是反的：文档 fallback 写「主窗专属（capability `windows`）」，
    //    而示例能力文件 `windows` 覆盖 `plugin-*`、`permissions` 只有 `core:default`，
    //    root 注册的 `host_*` 根本不被 ACL 按命令名管辖——那句口径在代码里零对应。
    const rowOf = (cmd: string) => surface.split(/\r?\n/).find((l) => l.startsWith(`| \`${cmd}\``));
    const cellsOf = (cmd: string) => {
      const row = rowOf(cmd);
      expect(row, `生成物缺命令行：${cmd}`).toBeTruthy();
      return row!
        .replace(/^\|\s*/, '')
        .replace(/\s*\|$/, '')
        .split(' | ');
    };
    for (const cmd of [
      'host_window_quit',
      'host_clipboard_read',
      'host_clipboard_write',
      'host_dialog_open',
      'host_dialog_save',
      'host_dialog_message',
      'host_dialog_confirm',
      'host_recover_boot',
      'host_i18n_stats',
    ]) {
      expect(cellsOf(cmd)[1], `${cmd} 的表里必须有 require_main_window 判定`).toContain(
        'require_main_window',
      );
    }
    expect(surface, '生成物仍用 capability 冒充判定').not.toContain('主窗专属（capability');
    expect(surface, '档位出现未解析的 `?`（authz 表读不到 = 档位承诺不可信）').not.toContain(
      '`?`（',
    );

    // ⑥ feature 门列必须与 tauri.rs 的属性逐条同源（写在 `#[tauri::command]` 上方也算）。
    //    轮 12 实测的缺陷形态就是 85 行全填「—」：opt-in 命令被谎报成无门，接入方按
    //    默认构建找不到那 2 条时没人能解释为什么。
    const tauriLines = read('crates/tauron-adapter/src/tauri.rs').split(/\r?\n/);
    const expectedGates = new Map<string, string>();
    for (let i = 0; i < tauriLines.length; i++) {
      const fn = (tauriLines[i] ?? '').match(/^\s*pub fn (host_[a-z0-9_]+)/)?.[1];
      if (!fn) continue;
      const feats: string[] = [];
      for (let k = i - 1; k >= 0 && /^\s*#/.test(tauriLines[k] ?? ''); k--) {
        const attr = (tauriLines[k] ?? '').trim();
        if (/^#\[cfg\(test/.test(attr) || attr.includes('not(feature')) continue;
        for (const m of attr.matchAll(/feature\s*=\s*"([^"]+)"/g)) feats.push(m[1] ?? '');
      }
      if (feats.length) expectedGates.set(fn, [...new Set(feats)].join(' + '));
    }
    expect(
      [...expectedGates.keys()].sort(),
      '本用例没解析出任何 feature 门（解析器或代码变了）',
    ).toEqual(['host_registry_install', 'host_registry_install_preview']);
    for (const [cmd, feat] of expectedGates) {
      expect(cellsOf(cmd)[4], `${cmd} 的 feature 门应是 ${feat}`).toContain(feat);
    }
    const gatedRows = surface
      .split(/\r?\n/)
      .filter((l) => l.startsWith('| `host_'))
      .filter((l) => {
        const cells = l
          .replace(/^\|\s*/, '')
          .replace(/\s*\|$/, '')
          .split(' | ');
        return cells[4] !== '—';
      })
      .map((l) => l.match(/^\| `(host_[a-z0-9_]+)`/)![1]);
    expect(gatedRows.sort(), 'feature 门列与 tauri.rs 的属性不同源').toEqual(
      [...expectedGates.keys()].sort(),
    );

    // ⑦ 「代码层无判定」是一份**逐名**清单，不是模糊的"少数"。新增命令漏判定即红；
    //    留在清单里的每条都必须在代码注释里给出理由（生成的登记语义随之携带它）。
    const guardlessLine = surface.match(/\*\*(\d+)\*\*（(host_[a-z0-9_、]+)）/);
    expect(guardlessLine, '计数口径没输出「无判定」的逐名清单').toBeTruthy();
    const listed = (guardlessLine![2] ?? '').split('、').filter(Boolean);
    expect(listed.length, '清单条数与正则捕获的条数不一致').toBe(Number(guardlessLine![1]));
    expect(listed.sort()).toEqual(
      [
        'host_brand_info',
        'host_i18n_t',
        'host_i18n_t_params',
        'host_window_close',
        'host_window_maximize',
        'host_window_minimize',
        'host_window_restore',
        'host_window_set_position',
        'host_window_set_size',
      ].sort(),
    );
    // 每条都要在**它自己的** `///` 注释里给出理由（生成的登记语义随之携带它）；
    // 只断言「全文出现过这句话」等于没查。（`tauriLines` 在 ⑥ 处已读入。）
    const docCommentAbove = (cmd: string) => {
      const i = tauriLines.findIndex((l) => /^\s*pub fn /.test(l) && l.includes(cmd));
      expect(i, `tauri.rs 找不到 ${cmd}`).toBeGreaterThan(-1);
      const lines: string[] = [];
      for (let k = i - 1; k >= 0; k--) {
        const l = (tauriLines[k] ?? '').trim();
        if (l.startsWith('#') || l === '') continue;
        if (!l.startsWith('///')) break;
        lines.unshift(l);
      }
      return lines.join('\n');
    };
    for (const cmd of listed) {
      expect(
        docCommentAbove(cmd),
        `${cmd} 既无判定、自己的注释里又没有「无需身份判定（轮 12 复核）」—— 文档只能猜`,
      ).toContain('无需身份判定（轮 12 复核）');
      expect(cellsOf(cmd)[1], `${cmd} 必须如实标成「代码层无判定」`).toContain('代码层无判定');
    }
  });

  // 轮 12：示例能力文件是「谁调得到 host_*」最容易被误读的地方——它曾被文档当成
  // 按命令的授权机制。这里把它自己的描述与 adapter 的 opt-in 事实钉住，免得下一个
  // 接入方复制示例时又把「windows 含 plugin-*」读成「这条命令插件调不到」。
  it('示例能力文件不得冒充按命令授权，且必须与 adapter 的 opt-in 口径同源', () => {
    const cap = read('examples/minimal-app/src-tauri/capabilities/default.json');
    expect(cap).toContain('"windows": ["main", "plugin-*"]');
    expect(cap).toContain('"permissions": ["core:default"]');
    expect(cap, '能力文件描述把 host_* 说成受 ACL 按命令管辖').toContain('不按命令名管辖');
    expect(cap, '能力文件仍把 plugin-install 说成 adapter 默认特性').not.toContain('已在默认特性');

    // 描述里的两个数必须真的来自 Cargo.toml，而不是抄自旧文档。
    const adapterToml = read('crates/tauron-adapter/Cargo.toml');
    expect(adapterToml, 'adapter 的 default 不再是空集（opt-in 口径失效）').toMatch(
      /^default\s*=\s*\[\]/m,
    );
    const exampleToml = read('examples/minimal-app/src-tauri/Cargo.toml');
    expect(exampleToml, '示例不再显式开 plugin-install，85 条的口径就错了').toContain(
      'default = ["plugin-install", "runtime-wasm-broker"]',
    );
    expect(read('examples/minimal-app/README.md'), '示例 README 的注册条数口径漂了').toContain(
      '故实际注册 **85** 条',
    );
  });

  // ────────────────────────────────────────────────────────────────────────
  // R1a：命令族的编译期可选集合（底座-only vs 全量）
  // ────────────────────────────────────────────────────────────────────────

  it('命令族必须是编译期可选集合，且底座集合不含插件域命令', () => {
    // 为什么是「两组集合」而不是「N 个可嵌套的族宏」：tauri::generate_handler!
    // 是 proc macro，只吃字面 path 列表（实测传 family!() → error: expected ','）。
    // 因此族以两组**编译期**可选集合表达，宿主二选一。
    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    const { substrate, full } = rustHandlerFamilies();
    expect(substrate.length, '底座集合解析失败').toBeGreaterThan(30);
    expect(full.length, '全量集合解析失败').toBeGreaterThan(40);

    // ① 全量集合 == 全部 `#[tauri::command] pub fn host_*` 定义
    //    （漏注册 = 前端 command not found；多注册 = 幽灵命令）。
    const defined = [...tauri.matchAll(/#\[tauri::command\]\s*\npub fn (host_[a-z0-9_]+)/g)].map(
      (m) => m[1]!,
    );
    expect(defined.length, '未解析出命令定义').toBeGreaterThan(40);
    expect([...full].sort()).toEqual([...defined].sort());
    // 无重复注册（重复会让分发出现不可达分支）。
    expect(new Set(full).size).toBe(full.length);

    // ② 底座集合 ⊂ 全量集合，且**不含任何插件域命令**。
    //    `host_recover_trial_enable` 属插件域：它要读注册表里插件的当前状态并补发
    //    `TrialEnable`（R1b 收窄签名时发现，R1a 首版误归底座）。
    //    `host_window_create` 同属插件域：它必须查注册表确认 `plugin-<id>` 存在，
    //    故绑 `PluginRuntimeState`（轮 11 R8 加入）。判据始终是 **State bound**，
    //    这个名字正则只是它的可读投影——加命令时两者必须一起改。
    //    `host_call_` 族（0.4-A1 跨主体调用：plugin/result/take + 既有 end）
    //    同属插件域：发起/回填/取件都要查 pending 表。
    const PLUGIN_DOMAIN =
      /^(host_lifecycle_report|host_plugin_call|host_call_|host_cancel|host_registry_|host_contributes_|host_recover_trial_enable$|host_stream_|host_runtime_|host_resource_stats$|host_window_create$)/;
    for (const cmd of substrate) {
      expect(full, `底座集合里的 ${cmd} 不在全量集合里`).toContain(cmd);
      expect(
        PLUGIN_DOMAIN.test(cmd),
        `底座集合混入插件域命令 ${cmd}（底座宿主会白拿插件命令面）`,
      ).toBe(false);
    }
    // 差值必须**恰好**等于插件域命令数：少减=白拿，多减=底座宿主漏功能。
    // 24 = 19（0.4-A1 之前）+ host_call_plugin / host_call_result / host_call_take
    //      + host_contributes_reconcile（0.4-W3）+ host_stream_grant（V4 A79）。
    const pluginOnly = full.filter((c) => PLUGIN_DOMAIN.test(c));
    expect(pluginOnly.length, '插件域命令数量异常').toBe(24);
    expect(full.length - substrate.length).toBe(pluginOnly.length);

    // ③ 两组集合都必须经 origin 门（收窄命令面不得绕过 R4-D2）。
    const gated = [...tauri.matchAll(/origin_gated_handler\(tauri::generate_handler!\[/g)].length;
    expect(gated, '两组 handler 都必须经 origin_gated_handler').toBeGreaterThanOrEqual(2);

    // ④ 历史名称仍等价全量（否则既有宿主装配会静默变成空 handler）。
    expect(tauri).toMatch(
      /macro_rules! tauron_generate_handler[\s\S]{0,240}tauron_plugin_handler!\[\]/,
    );
  });

  // ────────────────────────────────────────────────────────────────────────
  // R1b：底座 / 插件运行时状态拆分（编译期隔离）
  // ────────────────────────────────────────────────────────────────────────

  it('R1b：god object 拆成底座态 + 插件运行时态，且 State bound 与命令族一致', () => {
    const lib = read('crates/tauron-adapter/src/lib.rs');
    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    const { substrate: substrateCmds, full: fullCmds } = rustHandlerFamilies();
    const toFn = (c: string): string => c.replace(/^host_/, '');
    const baseSet = new Set(substrateCmds.map(toFn));
    const pluginSet = new Set(fullCmds.map(toFn).filter((c) => !baseSet.has(c)));

    // ① god object 必须消失：CommandState 只能是别名。
    expect(lib, 'CommandState 必须仍是 PluginRuntimeState 的别名').toMatch(
      /pub type CommandState = PluginRuntimeState;/,
    );
    expect(lib, '不得再出现 pub struct CommandState（god object 回归）').not.toMatch(
      /pub struct CommandState\b/,
    );
    expect(lib, 'PluginRuntimeState 必须 Deref 到 SubstrateState（底座字段单一来源）').toMatch(
      /impl core::ops::Deref for PluginRuntimeState[\s\S]{0,200}type Target = SubstrateState;/,
    );

    // ② 字段归属：注册表 / 贡献只能在插件运行时态。
    const structBody = (name: string): string => {
      const start = lib.indexOf(`pub struct ${name} {`);
      expect(start, `未找到 pub struct ${name}`).toBeGreaterThan(-1);
      const end = lib.indexOf('\n}\n', start);
      return lib.slice(start, end > start ? end : undefined);
    };
    const substrateStruct = structBody('SubstrateState');
    const pluginStruct = structBody('PluginRuntimeState');
    expect(substrateStruct, '底座态不得含 registry（否则编译期隔离无从谈起）').not.toMatch(
      /pub registry\s*:/,
    );
    expect(substrateStruct, '底座态不得含 contributes').not.toMatch(/pub contributes\s*:/);
    expect(pluginStruct, '插件运行时态必须含 registry').toMatch(/pub registry\s*:/);
    expect(pluginStruct, '插件运行时态必须含 contributes').toMatch(/pub contributes\s*:/);
    expect(pluginStruct, '插件运行时态必须持同一份底座 Arc').toMatch(
      /pub substrate: Arc<SubstrateState>/,
    );

    // ③ 命令的 State bound 必须与命令族一一对应（方案 R1 的核心不变量）。
    //    窗口必须**止于签名末尾**（首个 `{`）：否则会扫进下一个函数的签名，
    //    把底座命令的类型误配给插件命令（本门禁首版即因此假红）。
    const boundOf = (fn: string): string | null => {
      const idx = tauri.indexOf(`pub fn ${fn}(`);
      if (idx < 0) return null;
      const rel = tauri.slice(idx, idx + 600);
      const brace = rel.indexOf('{');
      const head = brace > 0 ? rel.slice(0, brace) : rel;
      if (/State<'_,\s*SubstrateState>/.test(head)) return 'substrate';
      if (/State<'_,\s*PluginRuntimeState>/.test(head)) return 'plugin';
      return null;
    };
    for (const fn of baseSet) {
      expect(
        boundOf(`host_${fn}`),
        `底座命令 host_${fn} 的 State bound 必须是 SubstrateState`,
      ).toBe('substrate');
    }
    for (const fn of pluginSet) {
      expect(
        boundOf(`host_${fn}`),
        `插件命令 host_${fn} 的 State bound 必须是 PluginRuntimeState`,
      ).toBe('plugin');
    }

    // ④ 底座命令实现体不得访问 registry / contributes（方案 R1 门禁一）。
    //    判定：生产代码里每个 `state.registry` / `state.contributes` 都必须落在
    //    形参为 `&PluginRuntimeState` 的函数体内（测试模块不计）。
    const lines = lib.split('\n');
    const testStart = lines.findIndex((l) => l.startsWith('#[cfg(test)]'));
    const prodEnd = testStart < 0 ? lines.length : testStart;
    const sigKind = new Map<string, string>();
    {
      let name = '';
      for (let i = 0; i < prodEnd; i += 1) {
        const m = /^\s*(?:pub )?fn (\w+)/.exec(lines[i]!);
        if (!m) continue;
        name = m[1]!;
        if (sigKind.has(name)) continue;
        // 只取签名区：从 fn 行到首个含 `{` 的行为止（免得把函数体里的类型名
        // 当成形参类型）。
        const head: string[] = [];
        for (let j = i; j < Math.min(i + 20, prodEnd); j += 1) {
          head.push(lines[j]!);
          if (lines[j]!.includes('{')) break;
        }
        const text = head.join('\n');
        if (/&\s*PluginRuntimeState/.test(text)) sigKind.set(name, 'plugin');
        else if (/&\s*SubstrateState/.test(text)) sigKind.set(name, 'substrate');
      }
    }
    const offenders: string[] = [];
    let owner = '';
    for (let i = 0; i < prodEnd; i += 1) {
      const m = /^\s*(?:pub )?fn (\w+)/.exec(lines[i]!);
      if (m) owner = m[1]!;
      if (/state\.(registry|contributes)\b/.test(lines[i]!)) {
        if (sigKind.get(owner) !== 'plugin') offenders.push(`${owner}:${i + 1}`);
      }
    }
    expect(
      offenders,
      `以下位置在非插件态函数里访问 registry/contributes：${offenders.join(', ')}`,
    ).toEqual([]);

    // ⑤ origin 门必须读底座状态：读 CommandState 会让底座-only 宿主被判「未装配」
    //    而**静默跳过鉴权**——恰好在最需要鉴权的那类宿主上失效。
    expect(tauri, 'origin 门必须从 SubstrateState 读允许清单').toMatch(
      /try_get::<SubstrateState>\(\)/,
    );
    expect(tauri, 'origin 门不得改回读 CommandState').not.toMatch(/try_get::<CommandState>\(\)/);

    // ⑥ 恢复对账的跨域解耦：底座只依赖写回口 trait，插件装配时注入。
    expect(lib, '缺 PluginFlagSink 写回口抽象').toMatch(/pub trait PluginFlagSink: Send \+ Sync/);
    expect(lib, '底座缺 plugin_flags 写回口字段').toMatch(
      /pub plugin_flags: Arc<std::sync::OnceLock<Arc<dyn PluginFlagSink>>>/,
    );
    expect(lib, '对账必须对「无写回口」显式短路').toMatch(
      /let Some\(sink\) = state\.plugin_flags\.get\(\) else/,
    );
    expect(lib, '插件装配必须注入写回口').toMatch(/plugin_flags[\s\S]{0,80}\.set\(/);

    // ⑦ 宿主装配必须同时注册两个状态，且共享同一份底座。
    expect(tauri, '装配必须注册底座状态').toMatch(
      /manager\.manage\(\(\*state\.substrate\)\.clone\(\)\)/,
    );
    expect(tauri, '装配必须注册插件运行时状态').toMatch(/manager\.manage\(state\)/);
  });

  // ────────────────────────────────────────────────────────────────────────
  // R5：HostRpc 统一 + 流式一等公民（P0-1）
  // ────────────────────────────────────────────────────────────────────────

  it('R5：流式帧词表两端同构，且旧词表已清除', () => {
    const rust = read('crates/tauron-host/src/stream.rs');
    const ts = read('packages/tauron-host/src/stream.ts');

    // ① 字段名：Rust snake_case（serde rename 成 camelCase）↔ TS camelCase。
    const rustStruct = rust.slice(
      rust.indexOf('pub struct StreamFrame {'),
      rust.indexOf('impl StreamFrame'),
    );
    expect([...rustStruct.matchAll(/pub (\w+):/g)].map((m) => m[1])).toEqual([
      'seq',
      'kind',
      'args_json',
      'args_raw',
    ]);
    const tsStruct = ts.slice(
      ts.indexOf('export interface StreamFrame {'),
      ts.indexOf('export function isStreamFrame'),
    );
    expect([...tsStruct.matchAll(/^\s{2}(\w+)\??:/gm)].map((m) => m[1])).toEqual([
      'seq',
      'kind',
      'argsJson',
      'argsRaw',
    ]);

    // ② 种类词表：data | end | error，两端一致且大小写敏感。
    expect([...rust.matchAll(/Self::\w+ => "(\w+)"/g)].map((m) => m[1])).toEqual([
      'data',
      'end',
      'error',
    ]);
    expect(ts).toMatch(/STREAM_KINDS[^=]*=\s*\['data', 'end', 'error'\]/);

    // ③ 旧词表必须清除：`CallFrame`（response/progress/cancel + value/data）与
    //    Rust 侧对不上，两边都「有类型」却互相不认识。
    const hostTs = read('packages/tauron-host/src/host.ts');
    const eventsTs = read('packages/tauron-host/src/events.ts');
    const indexTs = read('packages/tauron-host/src/index.ts');
    expect(eventsTs).not.toMatch(/interface CallFrame\b|type CallFrameKind/);
    expect(hostTs).not.toMatch(/\bCallFrame\b/);
    expect(indexTs).not.toMatch(/\bCallFrame\b/);
  });

  it('R5：流式三命令成组注册，且 plugin_call 真正接上帧载体', () => {
    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    const lib = read('crates/tauron-adapter/src/lib.rs');
    const registry = read('crates/tauron-host/src/registry.rs');
    const { substrate, full } = rustHandlerFamilies();

    // ① 三命令成组：只注册一两条会让流无法开/无法终结。
    const STREAM_CMDS = ['host_stream_open', 'host_stream_write', 'host_stream_close'];
    expect(
      STREAM_CMDS.filter((c) => !full.includes(c)),
      '流式命令未成组注册',
    ).toEqual([]);
    // 流属插件运行时域（句柄表与 pending-call 同锁域），底座集合不得含。
    expect(
      STREAM_CMDS.filter((c) => substrate.includes(c)),
      '底座集合不得含流式命令（它们依赖插件运行时状态）',
    ).toEqual([]);

    // ② 载体必须**真的**登记到调用上：R5 之前 channel 收下即丢，帧进虚空且不报错。
    expect(tauri, 'wire 层必须把 sink 登记到调用上').toMatch(
      /stream_bind\(&call\.call_id, &call\.plugin_id, sink\)/,
    );
    expect(tauri, '登记失败必须撤掉调用条目，不留悬挂').toMatch(/call_cancel\(&call\.call_id\)/);
    expect(tauri, '平台细节只在这一层：Channel → StreamSink').toMatch(/struct ChannelSink/);
    expect(tauri).toMatch(/impl StreamSink for ChannelSink/);
    expect(tauri, 'channel 线值必须解析成 Channel').toMatch(/JavaScriptChannelId/);
    // `Option<Channel<T>>` 编译不过（只有裸 Channel 有 CommandArg 实现）——锁住这个坑。
    expect(tauri, 'channel 参数必须是 Option<String>').toMatch(/channel: Option<String>/);

    // ③ 三命令实现体必须真调 Registry 的流 API（不是空壳）。
    expect(lib).toMatch(/\.stream_open\(call_id, subscriber\)/);
    expect(lib).toMatch(/\.stream_write\(stream_id, subscriber, args_json, args_raw\)/);
    expect(lib).toMatch(/\.stream_close\(stream_id, subscriber, parsed\)/);
    // `seq` 由宿主铸：不接受前端传入的 seq 参数。
    expect(lib, '流式命令不得接受前端 seq').not.toMatch(/seq: u64[\s\S]{0,80}stream/);

    // ④ 调用结束必须补终帧（handler 忘了关流时的唯一兜底），否则接收方永远等下去。
    expect(registry).toMatch(/fn end_call\(/);
    expect(registry).toMatch(/close_for_call\(call_id, terminal, reason\)/);
    expect(registry).toMatch(/close_for_subscriber\(id\.as_str\(\)\)/);
    expect(registry, 'GC 过期也要补终帧').toMatch(/json!\(\{ "reason": "call_timeout" \}\)/);
  });

  it('S1：传输身份通过 CallerSource 注入，host 命令签名不耦合 WebviewWindow', () => {
    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    const signatures = [...tauri.matchAll(/pub fn (host_\w+)\([\s\S]*?\) -> [^{]+\{/g)];
    expect(signatures.length).toBeGreaterThan(40);
    for (const signature of signatures) {
      expect(signature[0], `${signature[1]} 不应直接依赖 Tauri 窗口参数`).not.toMatch(
        /\bWebviewWindow\b/,
      );
    }
    expect(tauri).toMatch(
      /pub trait CallerSource\s*\{[\s\S]*?fn caller\(&self\) -> HostResult<crate::Caller>/,
    );
    expect(tauri).toMatch(/impl CallerSource for TauriCallerSource/);
    expect(tauri).toMatch(/impl<'de> CommandArg<'de, tauri::Wry> for TauriCallerSource/);
  });

  it('R5：TS 侧把**真实** channel 传进 invoke，且 SDK 不碰传输细节', () => {
    const hostTs = read('packages/tauron-host/src/host.ts');
    const backendTs = read('packages/tauron-host/src/backend.ts');
    const tauriBackend = read('packages/tauron-host/src/tauri-backend.ts');
    const rpc = read('packages/tauron-host/src/rpc.ts');
    const ctx = read('packages/tauron-app-plugin-sdk/src/context.ts');

    // ① FrameSink 必须把通道对象本身传进去（旧实现传的是 FrameSink 自己，
    //    Rust 侧只收到一个普通对象 → 帧永远送不出去且不报错）。
    expect(hostTs).toMatch(/channel: channel\.port/);
    expect(hostTs).not.toMatch(/^\s*channel,\s*$/m);
    // ② ChannelPort 只声明真实 Tauri Channel 拥有的成员。
    //    先剥注释：旧形状在注释里被解释过，门禁不该被自己的说明文字绊倒。
    const backendCode = backendTs.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '');
    expect(backendCode).toMatch(/onmessage\?:/);
    expect(backendCode).not.toMatch(/message: MessagePort/);
    // ③ 适配层不得再用 as unknown as 掩盖形状不一致。
    expect(tauriBackend).toMatch(/return new Channel<T>\(\);/);
    expect(tauriBackend).not.toMatch(/as unknown as ChannelPort/);
    // ④ HostRpc 四方法齐备，且 stream 落到 HostClient.openStream。
    for (const m of ['request', 'stream', 'event', 'subscribe']) {
      expect(rpc, `HostRpc 缺少 ${m}`).toMatch(new RegExp(`\\b${m}[<(]`));
    }
    expect(hostTs).toMatch(/openStream\(callId: string/);
    // ⑤ SDK 不得直接碰传输细节；只用事件域 4 原语。
    expect(ctx).not.toMatch(/@tauri-apps|TauriBackend|\binvoke\(/);
    // 注意：调用点可能是多行的（`host\n  .eventsSubscribe(...)`），因此不能用
    // `host\.` 这种紧邻写法——否则会漏掉换行的那一处，门禁假绿。
    const used = [...ctx.matchAll(/host\s*\.\s*(\w+)\(/g)].map((m) => m[1]!);
    expect([...new Set(used)].sort()).toEqual([
      'contributesRegister',
      'eventsDrain',
      'eventsPublish',
      'eventsSubscribe',
      'eventsUnsubscribe',
      // 0.4-A1：执行泵自动回填跨主体调用结果（仍是 HostClient 方法，不碰传输）。
      'reportCallResult',
    ]);
  });

  // ────────────────────────────────────────────────────────────────────────
  // R2-c：错误词表边界显式化（B3）
  // ────────────────────────────────────────────────────────────────────────

  it('R2-c：错误穿越边界必须经 translate_at_boundary（不得就地 new）', () => {
    const hostSrc = read('packages/tauron-host/src/host.ts');
    const shellSrc = read('packages/tauron-host/src/shell-client.ts');
    const errorsTs = read('packages/tauron-host/src/errors.ts');

    // ① 边界归一函数存在，且携带「哪个边界 + 是否翻译过 + 原始码所属词表」。
    expect(errorsTs).toMatch(/export function translate_at_boundary\(/);
    for (const f of ['boundary', 'translated', 'foreignVocabulary', 'rawCode']) {
      expect(errorsTs, `边界结果缺少 ${f}`).toMatch(new RegExp(`\\b${f}:`));
    }
    expect(errorsTs).toMatch(/'plugin-webview→host'/);
    expect(errorsTs).toMatch(/'host→plugin-webview'/);
    expect(errorsTs).toMatch(/'plugin-internal'/);

    // ② 穿越点必须走它：`new HostException(normalizeError(` 是**隐式**边界——
    //    它丢掉了边界身份与"码被改写"这一事实。
    for (const [name, src] of [
      ['host.ts', hostSrc],
      ['shell-client.ts', shellSrc],
    ] as const) {
      expect(src, `${name} 仍在就地构造边界异常`).not.toMatch(
        /new HostException\(normalizeError\(/,
      );
      expect(src, `${name} 未经 translate_at_boundary`).toMatch(
        /translate_at_boundary\(err, 'plugin-webview→host'\)/,
      );
    }

    // ③ 两套词表**不得**合并（合并会静默改变调用方的分支语义）。
    const hostCodes = errorsTs.slice(
      errorsTs.indexOf('HOST_ERROR_CODES = ['),
      errorsTs.indexOf('] as const;'),
    );
    expect(hostCodes, '宿主码表混入框架层码').not.toMatch(/SC-\d{4}/);
    const scTable = read('packages/types/src/errors.ts');
    expect(scTable, '框架层码表混入宿主码').not.toMatch(/'E_[A-Z_]+'/);
    // 形态识别 ≠ 语义接受：非宿主码仍归 E_UNKNOWN，只保留原始码。
    // 轮 35：`isAppLayerErrorCode` 的本体搬到 `@tauron/types`（两套词表共用的形态判据
    // 只能有一份），宿主侧改成再导出。公开面不变（1.0 兼容），所以门禁认两种形态：
    // 自家定义 或 再导出——但**只允许一处定义**（见轮 35 那条门禁）。
    expect(errorsTs).toMatch(
      /export function isAppLayerErrorCode\(|export \{[^}]*isAppLayerErrorCode/,
    );
    expect(errorsTs).toMatch(/export function isCodeLike\(|export \{[^}]*isCodeLike/);
  });

  // ────────────────────────────────────────────────────────────────────────
  // R2-a：canonical 归属（B1）
  // ────────────────────────────────────────────────────────────────────────

  it('R2-a：canonical 归属成文，且 legacy 侧被冻结（不得再长功能）', () => {
    const doc = read('docs/architecture/canonical-owners.md');
    // ① 决策必须成文且可定位（改这张表 = 改架构决策）。
    expect(doc, 'canonical 归属表缺失').toContain('canonical = `tauron-host`');
    const placement = doc.slice(doc.indexOf('## 0.3 crate 归置决策'), doc.indexOf('## 依据'));
    expect(placement, '0.3 孤儿 crate 归置表缺失').toContain('| `tauron-acl` | 迁移中 |');
    for (const crate of [
      'tauron-brand',
      'tauron-market',
      'tauron-wasm',
      'tauron-theme',
      'tauron-distribute',
      'tauron-shell',
    ]) {
      expect(placement, `归置表缺少 ${crate}`).toContain(`| \`${crate}\` |`);
    }
    expect(placement.match(/^\| `tauron-[^`]+` \|/gm)?.length).toBe(7);
    for (const owner of [
      'crates/tauron-host/src/eventbus.rs',
      'crates/tauron-host/src/registry.rs',
    ]) {
      expect(doc, `归属表未写明 ${owner}`).toContain(owner);
    }

    // ② 两侧代码必须自述归属（读代码的人不该靠猜）。
    const legacyBus = read('crates/tauron-shell/src/eventbus.rs');
    const legacyReg = read('crates/tauron-shell/src/registry.rs');
    const legacyDispatch = read('crates/tauron-shell/src/dispatch.rs');
    for (const [name, src] of [
      ['shell/eventbus.rs', legacyBus],
      ['shell/registry.rs', legacyReg],
      ['shell/dispatch.rs', legacyDispatch],
    ] as const) {
      expect(src, `${name} 未标注 legacy/非 canonical`).toMatch(/legacy \/ 非 canonical/);
      expect(src, `${name} 未指向归属表`).toContain('canonical-owners.md');
    }

    // ③ 冻结：legacy 侧方法集**指纹**不得变化。
    //    这是"非 canonical 侧不得继续分叉"的机械保证——想加功能必须改本门禁
    //    并在归属表里说明，不能悄悄长。
    const methodsOf = (src: string) => [...src.matchAll(/^\s{4}pub fn (\w+)/gm)].map((m) => m[1]!);
    expect(methodsOf(legacyBus), 'shell EventBus 是 legacy，方法集被冻结').toEqual([
      'namespace',
      'new',
      'with_queue_size',
      'emit',
      'subscribe',
      'unsubscribe',
      'clear_plugin',
      'queue_stats',
      'clear',
    ]);
    expect(methodsOf(legacyReg), 'shell PluginRegistry 是 legacy，方法集被冻结').toEqual([
      'new',
      'register',
      'transition',
      'get_state',
      'get_entry',
      'list_plugin_ids',
      'unregister',
      'clear',
      'len',
      'is_empty',
    ]);

    // ④ canonical 侧只允许**增长**（能力必须留在这一侧）。
    const hostBus = methodsOf(read('crates/tauron-host/src/eventbus.rs'));
    const hostReg = methodsOf(read('crates/tauron-host/src/registry.rs'));
    expect(hostBus.length, 'canonical 总线能力被削减').toBeGreaterThanOrEqual(19);
    expect(hostReg.length, 'canonical 注册表能力被削减').toBeGreaterThanOrEqual(32);
    // 关键能力必须在 canonical 侧（而不是 legacy 侧）。
    for (const fn of ['declare_topics', 'drain', 'dispose_subscriber']) {
      expect(hostBus, `canonical 总线缺少 ${fn}`).toContain(fn);
    }
    for (const fn of ['call_begin', 'gc_expired', 'stream_bind', 'install']) {
      expect(hostReg, `canonical 注册表缺少 ${fn}`).toContain(fn);
    }
    expect(methodsOf(legacyBus)).not.toContain('drain');
    expect(methodsOf(legacyReg)).not.toContain('stream_bind');

    // ⑤ 两个 crate 不得互相依赖：半合并会让归属重新变得含糊
    //    （一边引用另一边的类型，却又各自留着实现）。
    expect(read('crates/tauron-shell/Cargo.toml')).not.toMatch(/tauron-host/);
    expect(read('crates/tauron-host/Cargo.toml')).not.toMatch(/tauron-shell/);
  });

  // ────────────────────────────────────────────────────────────────────────
  // P0-2：进程插件运行时（激活孤儿 crate `tauron-proc`）
  // ────────────────────────────────────────────────────────────────────────

  it('P0-2：runtime 两命令成组注册、真调实现，且失败码语义分明', () => {
    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    const lib = read('crates/tauron-adapter/src/lib.rs');
    const { substrate, full } = rustHandlerFamilies();

    // ① 两命令成组：只有 spawn 没有 health 会让租约永远无法对账（进程死了没人知道）。
    const RT = ['host_runtime_spawn', 'host_runtime_health'];
    expect(
      RT.filter((c) => !full.includes(c)),
      '运行时命令未成组注册',
    ).toEqual([]);
    expect(
      RT.filter((c) => substrate.includes(c)),
      '底座-only 宿主不得注册进程运行时命令（它需要注册表与运行时状态）',
    ).toEqual([]);

    // ② 包装层必须真调平台无关实现（空壳 = 前端以为起了进程）。
    expect(tauri).toMatch(/crate::cmd_runtime_spawn_as\(&caller, &state, &plugin_id, &profile\)/);
    expect(tauri).toMatch(/crate::cmd_runtime_health_as\(&caller, &state, &lease\)/);

    // ③ 孤儿 crate 必须真的被用上（否则"激活"只是文档说法）。
    expect(lib, 'tauron-proc 未被接入').toMatch(/use tauron_proc::\{/);
    expect(lib, '未复用 tauron-proc 的 spawn 校验（两份规则 = 迟早不一致）').toMatch(
      /validate_spawn_config/,
    );
    expect(lib, '崩溃计数未复用 tauron-proc 的 CrashTracker').toMatch(/CrashTracker/);

    // ④ 失败码语义分明：类型不对 vs 租约失效，是两种不同的下一步动作。
    expect(lib, '非 Process 插件必须诚实失败').toMatch(/E_PLUGIN_TYPE_NO_RUNTIME/);
    expect(lib, '租约失效必须用租约语义的码').toMatch(/E_LEASE_EXPIRED/);
    expect(
      /runtime_lease\([^)]*\)[\s\S]{0,200}E_CALL_NOT_FOUND/.test(lib),
      '租约失效不得复用 E_CALL_NOT_FOUND（调用方会做错动作）',
    ).toBe(false);

    // ⑤ 进程启动必须可注入：测试真起 sidecar 会让 CI flaky。
    const procSpawn = read('crates/tauron-proc/src/spawner.rs');
    expect(procSpawn, '缺少可注入的 spawner 抽象').toMatch(/trait ProcSpawner/);
    expect(procSpawn, '生产实现必须显式命名（否则真实路径无处可查）').toMatch(/CommandSpawner/);
    expect(lib, '适配层未通过 trait 注入 spawner').toMatch(/ProcSpawner/);

    // ⑥ 运行时租约表必须与 pending/streams 同域并写明锁序。
    const registry = read('crates/tauron-host/src/registry.rs');
    expect(registry, '租约表不在 Registry 域锁内').toMatch(/fn runtime_lease\(/);
    expect(registry, '锁序契约未覆盖 runtime 表').toMatch(/pending → streams → runtime/);
  });

  it('P0-2：运行时线类型 TS ↔ Rust 逐字段同构（camelCase）', () => {
    const rust = read('crates/tauron-adapter/src/lib.rs');
    // `RuntimeHandle`/`RuntimeLease` 是宿主侧类型（租约表的行形态）。
    const rustHost = read('crates/tauron-host/src/runtime.rs');
    // 代际台账的行形态在 `generation.rs`（leak gate 的读数），同样要逐字段同构。
    const rustGen = read('crates/tauron-host/src/generation.rs');
    const ts = read('packages/tauron-host/src/shell-client.ts');

    const rustFields = (structName: string): string[] => {
      const m =
        new RegExp(`pub struct ${structName} \\{([\\s\\S]*?)\\n\\}`).exec(rustHost) ??
        new RegExp(`pub struct ${structName} \\{([\\s\\S]*?)\\n\\}`).exec(rustGen) ??
        new RegExp(`pub struct ${structName} \\{([\\s\\S]*?)\\n\\}`).exec(rust);
      expect(m, `Rust 侧缺 ${structName}`).not.toBeNull();
      const body = m![1]!.replace(/\/\/[^\n]*/g, '');
      const snake = [...body.matchAll(/pub (\w+):/g)].map((x) => x[1]!);
      // `#[serde(rename_all = "camelCase")]` 的线形：字段名即线名（转 camelCase）。
      return snake.map((f) => f.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase())).sort();
    };
    const tsFields = (ifaceName: string): string[] => {
      const m = new RegExp(`export interface ${ifaceName} \\{([\\s\\S]*?)\\n\\}`).exec(ts);
      expect(m, `TS 侧缺 ${ifaceName}`).not.toBeNull();
      const body = m![1]!.replace(/\/\/[^\n]*/g, '').replace(/\/\*[\s\S]*?\*\//g, '');
      return [...body.matchAll(/^\s+(\w+)\??:/gm)].map((x) => x[1]!).sort();
    };

    for (const [rustName, tsName] of [
      ['RuntimeSpawnProfile', 'RuntimeSpawnProfile'],
      ['RuntimeHandle', 'RuntimeHandle'],
      ['RuntimeHealth', 'RuntimeHealth'],
      ['RuntimeBinarySignature', 'RuntimeBinarySignature'],
      ['RuntimeAbiFingerprint', 'RuntimeAbiFingerprint'],
      ['ReapStats', 'ReapStats'],
      ['TerminalReapRecord', 'TerminalReapRecord'],
      ['GenerationStats', 'GenerationStats'],
    ] as const) {
      expect(tsFields(tsName), `${tsName} 字段与 Rust ${rustName} 不一致`).toEqual(
        rustFields(rustName),
      );
    }

    // `generatedAt` 故意不存在：校验时刻由宿主时钟决定，前端时间戳不可信。
    for (const name of ['RuntimeAbiFingerprint', 'RuntimeSpawnProfile']) {
      expect(tsFields(name)).not.toContain('generatedAt');
      expect(rustFields(name)).not.toContain('generatedAt');
    }
  });

  it('V7 §9 leak gate：abandoned generation 必须有界，且读数跨 IPC 可查', () => {
    // 泄漏形态（轮 25 翻出来的隐患）：`GenerationRegistry::active` 曾按资源各自记高点、
    // **只进不出**，于是内存占用由"曾起过运行期的插件数"决定——那个上界只写在注释里，
    // 没有代码把它钉住、没有读数可查、没有测试跑一遍 churn。这里钉四件：号源全局单调
    // （摘除跟踪后仍不得复用号）、回收连带摘除跟踪、有界读数一路走到线上、churn 测试存在。
    const gen = read('crates/tauron-host/src/generation.rs');
    const runtime = read('crates/tauron-host/src/runtime.rs');
    const registry = read('crates/tauron-host/src/registry.rs');
    const adapter = read('crates/tauron-adapter/src/lib.rs');
    const ts = read('packages/tauron-host/src/shell-client.ts');

    // ① 号源必须是全局单调计数器，不是每资源序号：否则摘除跟踪后重装会退回旧号。
    expect(gen, '代际号源已退回每资源序号（forget 之后会复用号）').toMatch(
      /self\.high_water = self\.high_water\.saturating_add\(1\)/,
    );
    expect(gen, '缺少摘除跟踪的出口').toMatch(/pub fn forget\(&mut self, resource: &str\) -> bool/);
    const forgetBody = /pub fn forget[\s\S]*?\n    \}/.exec(gen)?.[0] ?? '';
    expect(
      forgetBody,
      'forget 未拒绝仍有活跃租约的资源（会让 validate 只剩 token 一道判据）',
    ).toContain('return false');
    expect(forgetBody, 'forget 没有真的摘除 active 条目').toContain('self.active.remove(resource)');

    // ② 回收链路：先释放代际租约、再摘除跟踪，顺序反了就会被自己的闸门拒绝。
    const removeBody = /pub fn remove_plugin[\s\S]*?\n    \}/.exec(runtime)?.[0] ?? '';
    const releaseAt = removeBody.indexOf('generations.release(&lease)');
    const forgetAt = removeBody.indexOf('generations.forget(plugin_id)');
    expect(releaseAt, '回收路径不再释放代际租约').toBeGreaterThan(-1);
    expect(forgetAt, '回收路径未连带摘除代际跟踪（abandoned generation 会累积）').toBeGreaterThan(
      releaseAt,
    );

    // ③ 有界读数必须一路走到线上：台账 → 租约表 → 注册表 → host_resource_stats → TS 类型。
    expect(gen, '台账没有读数出口').toMatch(/pub fn stats\(&self\) -> GenerationStats/);
    expect(runtime, 'RuntimeTable 未暴露代际读数').toMatch(
      /pub fn generation_stats\(&self\) -> GenerationStats/,
    );
    expect(registry, 'Registry 未暴露代际读数').toMatch(/pub fn runtime_generation_stats\(&self\)/);
    expect(adapter, 'host_resource_stats 的读数不是来自注册表（另造计数器＝第二事实源）').toMatch(
      /let generations = state\.registry\.runtime_generation_stats\(\)/,
    );
    expect(adapter, 'host_resource_stats 未带上代际读数').toMatch(/"generations": generations/);
    expect(ts, 'TS 侧 ResourceStats 缺 generations 字段').toMatch(/generations: GenerationStats/);

    // ④ churn 证明：「有界」这件事只有真跑一遍大量 id 才钉得住，注释里的上界不算。
    expect(gen, '缺台账级 churn 测试').toMatch(
      /tracked_resources_stay_bounded_under_churn_of_distinct_ids/,
    );
    expect(gen, '缺句柄复用测试').toMatch(
      /abandoned_generation_handle_stays_rejected_after_forget_and_reactivation/,
    );
    expect(runtime, '缺租约表级 churn 测试').toMatch(
      /generation_tracking_stays_bounded_under_plugin_churn/,
    );
    expect(registry, '缺注册表真实路径 churn 测试').toMatch(
      /generation_tracked_resources_do_not_accumulate_across_plugin_churn/,
    );
    expect(adapter, 'host_resource_stats 的测试没核对代际读数').toMatch(
      /\["global"\]\["generations"\]/,
    );

    // 口径必须成文：否则下一个人只会看见 `forget` 这个新出口，把它当成"可以随便摘"。
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案未成文轮 25 的 abandoned generation 口径').toMatch(
      /abandoned generation/,
    );
    expect(plan, '缺口方案未成文 forget 的拒绝语义').toMatch(/仍有活跃租约时 `forget` 拒绝/);
    expect(plan, '复核命令未给出三条变异配方').toMatch(/摘除跟踪与每资源序号不可共存/);
  });

  it('V7 §7/§9：kill 失败必须有 retry 或终态证据，两者都有上界，且重试不得误杀同号活进程', () => {
    // 泄漏/断链形态（轮 26 翻出来的）：`tauron-proc::kill` 失败时把子进程句柄放回自己的
    // 跟踪表、错误里写「句柄已保留，可重试」，而 `RuntimeTable::terminate` 以前只记一笔
    // `failures` 就把租约摘掉——下层留着可重试的句柄，上层没有任何重试方。V7 §9 要的是
    // "retry/terminal evidence，不能只留日志"，这里钉四件：有人重试、重试有上界、到顶有
    // 查得动的证据、证据跨 IPC 可查。
    const runtime = read('crates/tauron-host/src/runtime.rs');
    const registry = read('crates/tauron-host/src/registry.rs');
    const adapter = read('crates/tauron-adapter/src/lib.rs');
    const proc = read('crates/tauron-proc/src/spawner.rs');
    const ts = read('packages/tauron-host/src/shell-client.ts');

    // ① 下层那句"可重试"必须兑现：失败入队 + 驱动出口存在。
    expect(proc, '进程侧不再声明"可重试"（这一侧的队列就失去理由了）').toMatch(/可重试/);
    const terminateBody = /fn terminate\(&mut self[\s\S]*?\n    \}/.exec(runtime)?.[0] ?? '';
    expect(terminateBody, 'terminate 失败后不再排队（kill 失败又只剩日志了）').toContain(
      'self.enqueue_pending(plugin_id, pid, reason)',
    );
    expect(runtime, '缺少驱动出口').toMatch(/pub fn retry_pending_reaps\(&mut self\) -> usize/);

    // ② 三重上界：队列长度、每条尝试次数、证据条数。少一个就是无限增长。
    expect(runtime, '待重试队列失去上界').toMatch(/pub const MAX_PENDING_REAPS: usize = \d+;/);
    expect(runtime, '重试次数失去上界').toMatch(/pub const MAX_REAP_RETRY_ATTEMPTS: u32 = \d+;/);
    expect(runtime, '终态证据台账失去上界').toMatch(/pub const MAX_TERMINAL_REAPS: usize = \d+;/);
    const enqueueBody = /fn enqueue_pending\(&mut self[\s\S]*?\n    \}/.exec(runtime)?.[0] ?? '';
    expect(enqueueBody, '队列满时不再计数（静默丢弃＝另一种泄漏）').toContain(
      'self.reap.overflow += 1',
    );
    expect(enqueueBody, '同一 pid 可以重复排队（队列会随失败次数增长）').toContain(
      'self.pending_reaps.iter().any(|p| p.pid == pid)',
    );

    // ③ 读数必须一路走到线上：表 → 注册表 → host_resource_stats → TS，且顺序不能反
    //    （先重试、后取快照，读到的才与刚发生的回收一致）。
    const statsBody = /pub fn reap_stats\(&self\)[\s\S]*?\n    \}/.exec(runtime)?.[0] ?? '';
    expect(statsBody, 'reap_stats 不再合成队列读数').toContain(
      's.pending = self.pending_reaps.len()',
    );
    expect(statsBody, 'reap_stats 不再带出终态证据').toContain(
      's.terminal_records = self.terminal_reaps',
    );
    expect(registry, 'Registry 未暴露重试驱动').toMatch(
      /pub fn runtime_retry_pending_reaps\(&self\)/,
    );
    const cmdBody =
      /pub fn cmd_resource_stats\(state: &PluginRuntimeState\)[\s\S]*?\n    \}/.exec(
        adapter,
      )?.[0] ?? '';
    const retryAt = cmdBody.indexOf('state.registry.runtime_retry_pending_reaps()');
    const readAt = cmdBody.indexOf('let reap = state.registry.runtime_reap_stats()');
    expect(retryAt, 'host_resource_stats 不再驱动重试（断链回来）').toBeGreaterThan(-1);
    expect(readAt, 'host_resource_stats 取读数不再经过注册表（第二事实源）').toBeGreaterThan(
      retryAt,
    );
    expect(adapter, 'host_resource_stats 未带上回收留痕').toMatch(/"reap": reap/);
    expect(ts, 'TS 侧 ResourceStats.global 缺 reap 字段').toMatch(
      /generations: GenerationStats;[\s\S]{0,600}reap: ReapStats;/,
    );
    expect(ts, 'TS 侧 ReapStats 缺重试腿字段').toMatch(
      /pending: number;[\s\S]*retries: number;[\s\S]*recovered: number;[\s\S]*terminal: number;[\s\S]*overflow: number;[\s\S]*skippedLivePid: number;[\s\S]*sweepResolved: number;[\s\S]*sweepSurvivors: number;[\s\S]*sweepUnknown: number;[\s\S]*ledgerWriteFailures: number;[\s\S]*terminalRecords: TerminalReapRecord\[\];/,
    );

    // ⑤ 安全封口（轮 27）：重试打第二枪之前必须先问"这个 pid 还有没有主人"。
    //    pid 会被 OS 重用，而终止器按 pid 查自己的跟踪表——旧回收记录与刚起来的活进程
    //    在它眼里是同一个号。少了这道判定，"清理孤儿"就会变成"误杀 sidecar"。
    const retryBody =
      /pub fn retry_pending_reaps\(&mut self\)[\s\S]*?\n    \}/.exec(runtime)?.[0] ?? '';
    expect(runtime, '缺少 pid 归属判定函数').toMatch(
      /fn pid_owner\(&self, pid: u32\) -> Option<String>/,
    );
    expect(retryBody, '重试不再核对 pid 归属（会误杀同号活进程）').toContain(
      'self.pid_owner(entry.pid)',
    );
    expect(retryBody, '让位不计数（"没误杀"与"没重试"在读数上同形）').toContain(
      'self.reap.skipped_live_pid += 1',
    );
    const guardAt = retryBody.indexOf('self.pid_owner(entry.pid)');
    const retryCountAt = retryBody.indexOf('self.reap.retries += 1');
    expect(retryCountAt, '重试计数在归属判定之前（让位会被误计成试过）').toBeGreaterThan(guardAt);
    expect(ts, 'TS 侧读不到让位次数').toContain('skippedLivePid: number;');
    expect(adapter, '线上没有 skippedLivePid').toMatch(/\["reap"\]\["skippedLivePid"\]/);

    // ④ 证明必须真跑：恢复、固化、风暴下的上界、重复失败去重，外加命令侧的活值核对。
    for (const name of [
      'driver_retries_a_failed_reap_and_recovers_it',
      'exhausted_reap_retries_become_terminal_evidence',
      'reap_failure_storm_keeps_queue_and_evidence_bounded',
      'repeated_failure_for_one_pid_queues_it_once',
      'reap_retry_yields_when_a_live_lease_owns_the_pid',
      'reap_retry_still_runs_when_the_live_lease_has_a_different_pid',
    ]) {
      expect(runtime, `缺租约表级测试 ${name}`).toMatch(new RegExp(name));
    }
    expect(registry, '缺注册表卸载路径的重试驱动测试').toMatch(
      /uninstall_kill_failure_is_retried_by_the_registry_driver/,
    );
    expect(registry, '缺"重装复用同号 ⇒ 驱动让位"的生产路径测试').toMatch(
      /reap_retry_never_kills_the_restarted_process_that_reused_the_pid/,
    );
    expect(adapter, 'host_resource_stats 的测试没核对回收留痕').toMatch(/\["global"\]\["reap"\]/);

    // 口径必须成文：否则下一个人会把重试队列当成"永远兜底的回收器"。
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案未成文 kill failure 的 retry/terminal 口径').toMatch(
      /kill failure 必须有 retry\/terminal evidence/,
    );
    expect(plan, '缺口方案未登记 startup orphan sweep 的落地（轮 41）').toMatch(
      /startup orphan sweep/,
    );
    expect(plan, '缺口方案未成文"重试不得误杀同号活进程"的口径（轮 27）').toMatch(
      /轮 27 重试的第二枪/,
    );
  });

  it('轮 41：跨重启孤儿扫描只探测不杀，台账可校验且读不开不洗白', () => {
    // 形态（V7 §7「未落：startup orphan sweep」的收口）：宿主上一轮"杀不掉"的 pid
    // 必须跨重启有下文。收口的关键安全边界是**跨重启 pid 身份不可验**——同一号可能
    // 已被无关新进程复用，因此扫描只探测定性（Gone 销账 / Alive、Unknown 留证），
    // **任何**按 pid 的终止都不允许出现在扫描体里；台账走 durable 校验和，撕裂/篡改
    // 拒绝装配（不静默重置成空账）。
    const runtime = read('crates/tauron-host/src/runtime.rs');
    const registry = read('crates/tauron-host/src/registry.rs');
    const adapter = read('crates/tauron-adapter/src/lib.rs');
    const ts = read('packages/tauron-host/src/shell-client.ts');

    // ① 分池铁律：读回的条目进 restart_reaps，杀进程重试腿（pending_reaps）不许碰它。
    expect(runtime, '缺少独立的重启待扫描池').toMatch(/restart_reaps: VecDeque<ReapLedgerEntry>/);
    const retryBody =
      /pub fn retry_pending_reaps\(&mut self\)[\s\S]*?\n    \}/.exec(runtime)?.[0] ?? '';
    expect(retryBody, '重试腿触碰重启条目（跨重启盲杀风险回来了）').not.toContain('restart_reaps');

    // ② 只探测不杀：扫描体里不许出现终止调用；三分支各自计数并留证。
    const sweepBody =
      /pub fn sweep_restart_reaps\(&mut self\)[\s\S]*?\n    \}/.exec(runtime)?.[0] ?? '';
    expect(sweepBody.length, '定位不到 sweep_restart_reaps（改签名要同步门禁）').toBeGreaterThan(
      100,
    );
    expect(sweepBody, '扫描体出现终止调用：跨重启盲杀封口被打开').not.toContain('.kill(');
    expect(sweepBody, '存活/不可判定条目未留证（"还在跑"必须有人查得动）').toContain('不盲杀');
    expect(sweepBody, '销账路径未计 sweep_resolved').toContain('self.reap.sweep_resolved += 1');
    expect(sweepBody, '存活路径未计 sweep_survivors').toContain('self.reap.sweep_survivors += 1');
    expect(sweepBody, '不可判定路径未计 sweep_unknown').toContain('self.reap.sweep_unknown += 1');

    // ③ 台账：durable 校验和 + 读不开拒绝 + 扫描后自动落盘（不等别的动作）。
    expect(runtime, '台账未走 durable 信封（撕裂无法发现）').toMatch(/REAP_LEDGER_SCHEMA/);
    const configureBody =
      /pub fn configure_reap_ledger\(&mut self[\s\S]*?\n    \}/.exec(runtime)?.[0] ?? '';
    expect(configureBody, '台账读取不做校验（撕裂会被当空账）').toContain('decode_durable');
    expect(configureBody, '读取失败未按 Integrity 拒绝').toContain('ReapLedgerError::Integrity');
    expect(registry, '注册表未提供开账入口').toMatch(
      /pub fn open_reap_ledger\(&self, dir: &Path\)/,
    );
    expect(registry, '开账入口没有扫描').toMatch(/runtime\.sweep_restart_reaps\(\)/);
    expect(registry, '落盘腿未走 durable 原子写').toMatch(
      /crate::durable::write_durable\(&path, &bytes\)/,
    );
    // 装配腿：探测注入 + 开账都在生产装配路径上（不是只在测试里接的）。
    expect(adapter, '装配层未注入平台探测').toContain('registry.set_system_pid_probe()');
    expect(adapter, '装配层未在恢复数据目录开账').toMatch(/\.open_reap_ledger\(dir\)/);
    expect(adapter, '台账被拒未按装配错误上抛（静默重置成空账）').toMatch(
      /AssemblyError::ReapLedgerRejected/,
    );

    // ④ 线上四字段 + 证据闭环。
    expect(ts, 'TS 侧读不到重启扫描销账数').toContain('sweepResolved: number;');
    expect(ts, 'TS 侧读不到重启扫描存活数').toContain('sweepSurvivors: number;');
    expect(ts, 'TS 侧读不到重启扫描不可判定数').toContain('sweepUnknown: number;');
    expect(ts, 'TS 侧读不到台账写失败计数').toContain('ledgerWriteFailures: number;');
    expect(adapter, '线上没有 sweepResolved').toMatch(/\["reap"\]\["sweepResolved"\]/);
    expect(adapter, '线上没有 ledgerWriteFailures').toMatch(/\["reap"\]\["ledgerWriteFailures"\]/);

    // ⑤ 证明必须真跑：往返、不盲杀、不可判定、撕裂拒绝、注册表全链、装配两例。
    for (const name of [
      'reap_ledger_round_trips_across_a_restart_scan',
      'restart_sweep_never_kills_even_when_the_probe_says_alive',
      'unknown_or_missing_probe_is_evidence_not_a_guess',
      'torn_or_tampered_ledger_is_rejected_not_reset',
      'empty_sweep_still_records_a_scanned_ledger',
      'unconfigured_table_yields_no_snapshot_and_no_fake_failure',
    ]) {
      expect(runtime, `缺租约表级测试 ${name}`).toMatch(new RegExp(name));
    }
    expect(registry, '缺注册表跨重启全链测试').toMatch(
      /reap_ledger_is_flushed_and_swept_across_registry_restarts/,
    );
    expect(adapter, '缺装配开账测试').toMatch(/assembly_opens_the_reap_ledger_on_the_recovery_dir/);
    expect(adapter, '缺撕裂台账拒绝装配测试').toMatch(/assembly_rejects_a_torn_reap_ledger/);

    // 口径必须成文：边界（活的只留证、不许自动清理）是结论的一部分。
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案未成文轮 41 的跨重启扫描口径').toMatch(/轮 41/);
    expect(plan, '缺口方案未写明"只探测不杀"的边界').toMatch(/只探测不杀|不盲杀/);
    // V7 §7 的 Process 行是这条链的**对外宣称**：轮 41 的收口与"不盲杀"边界必须写进行文，
    // 且不许被改写成"已自动清理"（该行自称的落地门禁就是本段）。锁措辞本身，不锁行号。
    const v7 = read('docs/Tauron-Deep-Audit-and-Optimization-Plan-V7.md');
    const v7ProcessRow = /^\| Process \|.*$/m.exec(v7)?.[0] ?? '';
    expect(v7ProcessRow, 'V7 §7 缺 Process 行（行名改了就要重新复核）').not.toBe('');
    expect(v7ProcessRow, 'V7 §7 Process 行未登记轮 41 的收口').toMatch(/轮 41/);
    expect(v7ProcessRow, 'V7 §7 Process 行丢了"不盲杀"边界（假宣称已自动清理？）').toMatch(
      /不盲杀|只探测不杀/,
    );
  });

  it('轮 42：便携路径五维各自具名，zip 侧与宿主共用同一份判定', () => {
    // 形态（A85 收口）：五维判定必须可指认、可变异——每维一个具名错误面，判定是
    // 词典序、平台中立的（不依赖运行宿主 std::path 方言）；第 5 维是集合级的，归宿主的
    // 逐条校验管不到。zip 侧（市场 signing 生产面）不许抄第二份实现，必须委派同一份。
    const pp = read('crates/tauron-host/src/portable_path.rs');
    const market = read('crates/tauron-market/src/lib.rs');

    // ① 五维各自具名：保留名 / 非 NFC / 尾点尾空格 / 超长 / 集合级折叠冲突。
    for (const variant of [
      'ReservedName',
      'NotNfc',
      'TrailingDotOrSpace',
      'TooLong',
      'CaseCollision',
    ]) {
      expect(pp, `缺具名失败面 ${variant}（五维退回一个泛型错误？）`).toMatch(
        new RegExp(`\\b${variant}\\b`),
      );
    }

    // ② 判定本身：设备名表、NFC 归一器、尾缀、长度上界、双分隔符切分、ASCII 折叠。
    expect(pp, '缺 22 个 Windows 保留设备名表').toMatch(/WINDOWS_RESERVED_STEMS: \[&str; 22\]/);
    expect(pp, 'NFC 判定未用 icu 归一器（自己写近似判定？）').toMatch(
      /ComposingNormalizerBorrowed::new_nfc\(\)/,
    );
    expect(pp, '尾点/尾空格判定被改写（Windows 会静默剥掉尾缀）').toMatch(
      /segment\.ends_with\('\.'\) \|\| segment\.ends_with\(' '\)/,
    );
    expect(pp, '段长上界常量被动').toMatch(/const MAX_SEGMENT_BYTES: usize = 255;/);
    expect(pp, '整条相对路径上界常量被动').toMatch(/const MAX_RELATIVE_BYTES: usize = 1023;/);
    expect(pp, '段切分未按双分隔符（换了宿主平台结论就变）').toContain("path.split(['/', '\\\\'])");
    const setBody = /pub fn validate_portable_entry_set[\s\S]*?\n\}/.exec(pp)?.[0] ?? '';
    expect(setBody, '定位不到集合级判定（改签名要同步门禁）').toContain('to_ascii_lowercase()');

    // ③ zip 侧委派：两处调用都必须在各自函数体内，且常量腿明确在 signing 门后
    // （不是抄第二份判定，也不是缺省构建里炸的裸调用）。
    const sanitizeBody = /pub fn sanitize_entry_path[\s\S]*?\n\}/.exec(market)?.[0] ?? '';
    expect(sanitizeBody, '逐条校验未委派宿主的 portable_path').toContain(
      'tauron_host::portable_path::validate_portable_relative(name)',
    );
    const constantsBody = /pub fn validate_zip_constants[\s\S]*?\n\}/.exec(market)?.[0] ?? '';
    expect(constantsBody, '集合级校验未委派宿主的 portable_path').toContain(
      'tauron_host::portable_path::validate_portable_entry_set(&names)',
    );
    expect(market, 'zip 侧集合级委派不在 signing 门后').toMatch(
      /#\[cfg\(feature = "signing"\)\]\s*\n\s*\{\s*\n\s*let names: Vec<&str> = entries/,
    );

    // ④ 证明必须真跑：宿主五条 + 市场两条（signing 门后）。
    for (const name of [
      'rejects_windows_reserved_device_names_in_any_case_or_extension',
      'rejects_trailing_dot_or_space_that_windows_would_strip',
      'rejects_non_nfc_forms_but_accepts_the_precomposed_one',
      'rejects_segments_or_paths_over_the_portable_length_bounds',
      'entry_set_rejects_ascii_case_collisions_and_duplicates',
    ]) {
      expect(pp, `缺宿主五维测试 ${name}`).toMatch(new RegExp(name));
    }
    for (const name of [
      'zip_side_rejects_five_dim_paths_before_extraction',
      'zip_side_rejects_case_colliding_entry_sets',
    ]) {
      expect(market, `缺 zip 侧委派测试 ${name}`).toMatch(new RegExp(name));
    }

    // ⑤ 口径必须成文：A85 台账行已翻篇（不许回写"未落"），轮 42 小节在场。
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案未登记轮 42').toMatch(/轮 42/);
    const a85Row = /^\| A85 .*$/m.exec(plan)?.[0] ?? '';
    expect(a85Row, 'A85 台账行消失（行名改了要重新复核）').not.toBe('');
    expect(a85Row, 'A85 台账行未登记轮 42 落地').toMatch(/轮 42/);
    expect(a85Row, 'A85 台账行仍写"未落"（代码与宣称脱节）').not.toMatch(/未落/);
  });

  it('轮 43：破坏性管理操作的令牌链两侧同形，TTL 走 A100 可信时间', () => {
    // 形态（A83 尾段 + A100 收口）。五段钉住：
    // ① Rust——令牌绑事实、一次消费（先摘除再比对）、两域时间源与过期判定共用
    //    review_now/review_unexpired、旧面无令牌入口在生产档拒破坏性操作、预览零副作用；
    // ② wire——HostAdminOp 两字段 serde(default)（旧载荷仍合法）、wire 层转调 reviewed
    //    入口、返回判别形且 executed 分支老字段留顶层；
    // ③ TS——token/判别联合镜像、管理面与壳层两条腿的预览→确认→提交链、无 DOM 失败关闭；
    // ④ 测试在场（Rust 定向 + TS 两条腿）；⑤ 台账行翻篇 + 轮 43 小节在场。
    const lib = read('crates/tauron-adapter/src/lib.rs');
    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    const events = read('packages/tauron-host/src/events.ts');
    const host = read('packages/tauron-host/src/host.ts');
    const manager = read('packages/tauron-ui/src/plugin-manager.ts');
    const shell = read('packages/tauron-host/src/shell-controller.ts');
    const bodyOf = (src: string, pattern: RegExp): string => pattern.exec(src)?.[0] ?? '';

    // ① Rust 形状。
    const tokenBody = bodyOf(lib, /pub struct AdminReviewToken \{[\s\S]*?\n\}/);
    expect(tokenBody, 'AdminReviewToken 消失（行名改了要重新复核）').not.toBe('');
    for (const field of ['plugin_id', 'op', 'version', 'issued_at', 'expires_at', 'nonce']) {
      expect(tokenBody, `令牌缺事实绑定字段 ${field}（重核会退化成只认 nonce）`).toMatch(
        new RegExp(`\\b${field}\\b`),
      );
    }
    expect(lib, '令牌缺 deny_unknown_fields（未知字段被静默吞掉？）').toMatch(
      /#\[serde\(rename_all = "camelCase", deny_unknown_fields\)\]\s*\n\s*pub struct AdminReviewToken/,
    );
    expect(lib, 'TTL 常量被动（600s 是台账成文承诺）').toMatch(
      /const ADMIN_REVIEW_TTL_SECS: u64 = 10 \* 60;/,
    );
    expect(lib, '令牌容量上界被动（无界 pending 集）').toMatch(
      /const MAX_ADMIN_REVIEWS: usize = 64;/,
    );

    // 时间源与过期判定：两域共用同一对咽喉（provider 在 → trusted_time；None+Production 拒）。
    const reviewNow = bodyOf(lib, /fn review_now\([\s\S]*?\n\}/);
    expect(reviewNow, 'review_now 未走 trusted_time（TTL 建立在墙钟上？）').toContain(
      'tauron_host::TrustedTimeProvider::trusted_time(provider.as_ref())',
    );
    expect(reviewNow, 'review_now 未对非 Trusted 失败关闭').toContain(
      'trusted.state != tauron_host::TimeTrustState::Trusted',
    );
    expect(reviewNow, 'review_now 丢了 None+Production 拒绝分支').toMatch(
      /state\.deployment_mode == tauron_host::DeploymentMode::Production/,
    );
    const reviewUnexpired = bodyOf(lib, /fn review_unexpired\([\s\S]*?\n\}/);
    expect(reviewUnexpired, '过期判定未复用 A100 的 require_unexpired').toContain(
      'tauron_host::require_unexpired(provider.as_ref(), expires)',
    );
    expect(reviewUnexpired, 'review_unexpired 丢了 None+Production 拒绝分支').toMatch(
      /state\.deployment_mode == tauron_host::DeploymentMode::Production/,
    );
    const mintAdmin = bodyOf(lib, /fn mint_admin_review\([\s\S]*?\n\}/);
    expect(mintAdmin, '管理域铸发未走 review_now（墙钟回退到铸发了？）').toContain(
      'review_now(state)?',
    );
    expect(mintAdmin, '铸发未做容量上界处理').toContain('reviews.len() >= MAX_ADMIN_REVIEWS');
    const mintInstall = bodyOf(lib, /fn mint_install_review\([\s\S]*?\n\}/);
    expect(mintInstall, 'install 域铸发未走 review_now（A100 只覆盖一半？）').toContain(
      'review_now(state)?',
    );
    expect(lib, 'install 域消费未走 review_unexpired（两域判定分家）').toMatch(
      /review_unexpired\(state, review_token\.expires_at\)\?/,
    );
    const validateBody = bodyOf(lib, /fn validate_admin_review\([\s\S]*?\n\}/);
    expect(validateBody, '消费顺序不是先摘除（伪造令牌能留下可用条目？）').toContain(
      '.remove(&token.nonce)',
    );
    expect(validateBody, '消费未比对存储值').toContain('stored != *token');
    expect(validateBody, '消费未做过期判定').toContain(
      'review_unexpired(state, token.expires_at)?',
    );
    expect(validateBody, '消费未重核 op/plugin_id').toMatch(
      /token\.op != op \|\| token\.plugin_id != plugin_id/,
    );
    expect(validateBody, '消费未重核预览版本（预览后换版本仍放行？）').toContain(
      'entry.manifest.version.to_string() != token.version',
    );

    // 生产档强制条件：feature+Production；旧面拒、新面 preview 零副作用。
    const reviewRequired = bodyOf(lib, /pub fn admin_review_required\([\s\S]*?\n\}/);
    expect(reviewRequired, 'admin_review_required 丢了 feature 门').toContain(
      'cfg!(feature = "plugin-install")',
    );
    expect(reviewRequired, 'admin_review_required 丢了 Production 条件').toContain(
      'DeploymentMode::Production',
    );
    expect(lib, '旧面无令牌入口未拒生产档破坏性操作').toMatch(
      /admin_review_required\(state\) && is_destructive_admin_op\(op\)/,
    );
    const reviewed = bodyOf(lib, /pub fn cmd_registry_admin_reviewed_as\([\s\S]*?\n\}/);
    expect(reviewed, 'reviewed 入口丢了 preview 铸发').toContain(
      'mint_admin_review(state, plugin_id, op, &version)?',
    );
    expect(reviewed, 'reviewed 入口丢了令牌消费').toContain(
      'validate_admin_review(state, plugin_id, op, token)?',
    );
    expect(reviewed, 'reviewed 入口未把执行结果包进 Executed').toContain(
      'RegistryAdminResponse::Executed(outcome)',
    );
    const responseEnum = bodyOf(lib, /pub enum RegistryAdminResponse \{[\s\S]*?\n\}/);
    expect(responseEnum, '返回未判别为 Executed/Review 两态').toMatch(
      /Executed\(TransitionOutcome\)/,
    );
    expect(responseEnum, '返回缺 Review 分支').toMatch(/Review\(RegistryAdminReview\)/);
    expect(lib, '判别形未用 internally-tagged kind（老读者凭 kind 分流）').toMatch(
      /#\[serde\(tag = "kind", rename_all = "camelCase"\)\]\s*\n\s*pub enum RegistryAdminResponse/,
    );

    // ② wire 形状。
    const adminOpBody = bodyOf(tauri, /pub struct HostAdminOp \{[\s\S]*?\n\}/);
    expect(
      adminOpBody,
      '旧载荷断供：preview 无 serde(default)（老客户端不带新键即反序列化失败）',
    ).toMatch(/#\[serde\(default\)\]\s*\n\s*pub preview: Option<bool>/);
    expect(adminOpBody, '旧载荷断供：reviewToken 无 serde(default)').toMatch(
      /#\[serde\(default\)\]\s*\n\s*pub review_token: Option<crate::AdminReviewToken>/,
    );
    const wire = bodyOf(tauri, /pub fn wire_registry_admin\([\s\S]*?\n\}/);
    expect(wire, 'wire 层未转调 reviewed 入口（预览/令牌面绕行）').toContain(
      'cmd_registry_admin_reviewed_as(',
    );
    expect(wire, 'wire 层丢了 preview 默认 false').toContain('op.preview.unwrap_or(false)');
    expect(wire, 'wire 层丢了令牌透传').toContain('op.review_token.as_ref()');
    expect(tauri, 'host_registry_admin 返回类型未换判别形').toMatch(
      /Result<crate::RegistryAdminResponse, TauriError>/,
    );

    // ③ TS 两侧。
    expect(events, 'TS 缺 AdminReviewToken 镜像').toMatch(/export interface AdminReviewToken \{/);
    expect(events, 'RegistryAdminOp 缺 preview 可选字段').toMatch(/preview\?: boolean;/);
    expect(events, 'RegistryAdminOp 缺 reviewToken 可选字段').toMatch(
      /reviewToken\?: AdminReviewToken;/,
    );
    expect(events, '返回判别联合缺 executed 分支').toMatch(/kind: 'executed';/);
    expect(events, '返回判别联合缺 review 分支').toMatch(/kind: 'review';/);
    expect(host, 'HostClient.registryAdmin 未返回判别联合').toMatch(
      /async registryAdmin\(op: RegistryAdminOp\): Promise<RegistryAdminOutcome>/,
    );
    const destructive = bodyOf(manager, /private async _destructiveAdminOp\([\s\S]*?\n  \}/);
    expect(destructive, '管理面破坏性操作未先预览').toMatch(/preview: true/);
    expect(destructive, '管理面未消费确认钩子').toContain('approved = await this._confirm(');
    expect(destructive, '管理面取消未与失败区分').toContain(
      'return { ok: false, cancelled: true };',
    );
    expect(destructive, '管理面未把令牌原样带回').toContain('reviewToken: preview.reviewToken');
    expect(destructive, '老宿主（预览即执行）被重复提交').toContain(
      "if (preview.kind !== 'review') return { ok: true };",
    );
    const defaultConfirm = bodyOf(manager, /function defaultAdminConfirm\([\s\S]*?\n\}/);
    expect(defaultConfirm, '无 DOM 默认钩子未失败关闭（静默放行破坏性操作？）').toMatch(
      /typeof window === 'undefined' \|\| typeof window\.confirm !== 'function'\)\s*return false/,
    );
    const uninstall = bodyOf(shell, /private async _uninstallPlugin\([\s\S]*?\n  \}/);
    expect(uninstall, '壳层卸载腿未先预览').toMatch(/preview: true/);
    expect(uninstall, '壳层未按判别形分流').toContain("outcome.kind === 'review'");
    expect(uninstall, '壳层未带令牌提交').toContain('reviewToken: outcome.reviewToken');

    // ④ 测试在场。
    for (const name of [
      'admin_review_preview_binds_facts_and_commits_once',
      'admin_review_rejects_op_mismatch_and_stays_consumed',
      'admin_review_version_drift_forces_a_fresh_preview',
      'admin_review_protocol_rejects_non_destructive_mixing',
      'production_destructive_admin_op_requires_the_review_path',
      'admin_review_ttl_follows_the_trusted_provider',
      'demoted_clock_after_startup_fails_closed_for_admin_review',
      'install_review_ttl_follows_the_trusted_provider',
      'production_without_trusted_time_is_rejected_at_startup',
      'production_with_suspicious_clock_is_rejected_at_startup',
      'production_preview_fails_closed_without_trusted_time_in_substrate_builds',
    ]) {
      expect(lib, `缺轮 43 定向测试 ${name}`).toMatch(new RegExp(name));
    }
    const managerTests = read('packages/tauron-ui/src/plugin-manager.test.ts');
    const shellTests = read('packages/tauron-host/src/shell-controller.test.ts');
    for (const name of [
      'uninstall 走预览 → 确认 → 提交令牌（轮 43 / A83）',
      'uninstall 用户取消 → cancelled，无提交调用（轮 43 / A83）',
      'uninstall 默认确认在无 DOM 环境失败关闭（拒绝而非静默放行）',
      'purge 走预览 → 确认 → 提交令牌（轮 43 / A83）',
    ]) {
      expect(managerTests, `缺管理面轮 43 测试「${name}」`).toContain(name);
    }
    expect(shellTests, '缺壳层两步链测试').toContain(
      'oc-plugin-uninstall 两步链：预览（无副作用）→ 确认 → 以令牌提交（轮 43 / A83）',
    );

    // ⑤ 口径成文：A83/A100 台账行翻篇，轮 43 小节在场。
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案缺轮 43 小节').toMatch(/### 轮 43：/);
    const a83Row = /^\| A83 .*$/m.exec(plan)?.[0] ?? '';
    expect(a83Row, 'A83 台账行消失（行名改了要重新复核）').not.toBe('');
    expect(a83Row, 'A83 台账行未登记轮 43 落地').toMatch(/轮 43/);
    expect(a83Row, 'A83 台账行仍只写 install 域（代码与宣称脱节）').not.toMatch(/仅 install 域/);
    const a100Row = /^\| A100 .*$/m.exec(plan)?.[0] ?? '';
    expect(a100Row, 'A100 台账行消失（行名改了要重新复核）').not.toBe('');
    expect(a100Row, 'A100 台账行未登记两域 TTL 收口').toMatch(/轮 43/);
    expect(a100Row, 'A100 台账行仍写"A83 的 token TTL 仍用墙钟"（已在代码中收口）').not.toMatch(
      /仍用墙钟/,
    );
  });

  it('轮 44：安装流内存门禁落在真实安装链上，整包内存入口已删', () => {
    // A94 收口。五段钉住：
    // ① budgets——`process.maxPeakRssKb` 被 `installStream` 取代（载荷下限 + 堆/VmHWM 双上界）；
    // ② 门禁脚本——`--install-probe` 必需、三道安装断言在场、`budgets.process` 不再被读；
    // ③ CI——release 构建探针并把它喂给门禁（VmHWM 只在 Linux runner 上有读数）；
    // ④ 代码形状——探针走生产提交链、计数分配器 + VmHWM 双读数且载荷下限被结构保证；
    //    整包入口删净；激活摘要流式（本轮门禁抓出的真实缺口）；perf_probe 不再装作测过内存；
    // ⑤ 台账行翻篇 + 轮 44 小节在场。
    const budgets = JSON.parse(read('contracts/performance-budgets.json'));
    expect(budgets.installStream, 'installStream 预算块消失（内存门禁失去锚点）').toBeTruthy();
    expect(
      budgets.installStream.minPayloadBytes,
      '载荷下限被改（小包跑低内存什么都证明不了）',
    ).toBe(67108864);
    expect(budgets.installStream.maxPeakHeapKib, '堆峰值上界被改（计数分配器断言失锚）').toBe(
      16384,
    );
    expect(budgets.installStream.maxPeakRssKb, 'VmHWM 上界被改（Linux 断言失锚）').toBe(32768);
    expect(budgets.process, '旧 process 预算块仍在（两条读数链并存会分家）').toBeUndefined();

    const script = read('scripts/check-performance-size.mjs');
    expect(script, '门禁未要求 --install-probe（安装流读数可缺席）').toContain(
      "required('install-probe')",
    );
    expect(script, '门禁丢了载荷下限断言').toContain('budgets.installStream.minPayloadBytes');
    expect(script, '门禁丢了堆峰值断言').toContain('budgets.installStream.maxPeakHeapKib');
    expect(script, '门禁丢了 VmHWM 断言').toContain('budgets.installStream.maxPeakRssKb');
    expect(script, '缺 RSS 读数被当作跳过（必须失败关闭）').toContain(
      'install peak RSS was not observed on the Linux performance runner',
    );
    expect(script, '旧 budgets.process 读取仍在（影子预算）').not.toContain('budgets.process');
    expect(script, 'wire 轮数断言被拆（回归天花板必须还在）').toContain(
      'budgets.wireRoundTrip.iterations',
    );
    expect(script, 'wire 耗时断言被拆').toContain('budgets.wireRoundTrip.maxElapsedMs');

    const ci = read('.github/workflows/ci.yml');
    expect(ci, 'CI 未构建安装流探针（门禁读不到真实读数）').toContain(
      'cargo build -p tauron-adapter --example install_stream_probe --release --locked --features plugin-install',
    );
    expect(ci, 'CI 未运行安装流探针').toContain(
      'target/release/examples/install_stream_probe > /tmp/tauron-install-probe.json',
    );
    expect(ci, 'CI 未把探针输出喂给门禁').toContain(
      '--install-probe=/tmp/tauron-install-probe.json',
    );

    // ④ 代码形状。
    const probeSrc = read('crates/tauron-adapter/examples/install_stream_probe.rs');
    expect(probeSrc, '探针缺计数全局分配器（跨平台堆读数）').toContain('#[global_allocator]');
    expect(probeSrc, '探针堆读数为普通 load（高水位只能 fetch_max 记录）').toContain(
      'PEAK.fetch_max',
    );
    expect(probeSrc, '探针未读 VmHWM（Linux 侧读数）').toContain('/proc/self/status');
    expect(probeSrc, '探针未走 install 预览（绕过审批链的内存什么都不代表）').toContain(
      'cmd_registry_install_preview_as',
    );
    expect(probeSrc, '探针未走 reviewed 提交（真实提交链缺席）').toContain(
      'cmd_registry_install_reviewed_as',
    );
    expect(probeSrc, '缺省特性下的空壳退出口消失（会静默成功？）').toContain(
      'std::process::exit(2)',
    );
    // 载荷下限的结构保证：探针自报的常量乘积必须 ≥ 预算下限——不然「探针包变小、
    // 门禁按自报数通过」会成为绕过路径。
    const blobEntries = Number(/const BLOB_ENTRIES: usize = (\d+);/.exec(probeSrc)?.[1]);
    const blobBytes = Number(/const BLOB_BYTES: u64 = (\d+) \* 1024 \* 1024;/.exec(probeSrc)?.[1]);
    expect(blobEntries, '探针载荷条目常量消失（下限结构保证失效）').toBeGreaterThan(0);
    expect(blobBytes, '探针载荷块常量消失').toBeGreaterThan(0);
    expect(
      blobEntries * blobBytes * 1024 * 1024,
      '探针载荷上限外于预算下限（门禁载荷断言会空转）',
    ).toBeGreaterThanOrEqual(budgets.installStream.minPayloadBytes);
    expect(probeSrc, '探针包改用压缩（载荷/包装关系不再简单可核）').toContain(
      'CompressionMethod::Stored',
    );

    const signature = read('crates/tauron-market/src/package_signature.rs');
    expect(signature, '整包入口 verify_tpkg 复活（零调用者整包读）').not.toMatch(
      /pub fn verify_tpkg\s*\(/,
    );
    expect(signature, 'verify_tpkg_file 复活').not.toMatch(/pub fn verify_tpkg_file\s*\(/);
    expect(signature, '流式入口 verify_tpkg_reader 消失（验签整读回来了？）').toContain(
      'pub fn verify_tpkg_reader<',
    );
    expect(signature, '流式入口 verify_tpkg_reader_with_time 消失').toContain(
      'pub fn verify_tpkg_reader_with_time<',
    );

    const activation = read('crates/tauron-host/src/activation.rs');
    expect(activation, 'ContentIdentity 缺流式构造').toContain(
      'pub fn from_reader(mut reader: impl std::io::Read)',
    );
    expect(activation, 'from_bytes 不再委托流式构造（内存切片也要走同一咽喉）').toContain(
      'Self::from_reader(bytes)',
    );

    const lib = read('crates/tauron-adapter/src/lib.rs');
    const collectBody = /fn collect_plugin_activation_records\([\s\S]*?\n\}/.exec(lib)?.[0] ?? '';
    expect(collectBody, 'collect_plugin_activation_records 消失（行名改了要重新复核）').not.toBe(
      '',
    );
    expect(collectBody, '激活摘要未走流式（整读回归会让堆门禁重新变红）').toContain(
      'ContentIdentity::from_reader',
    );
    expect(collectBody, '激活摘要仍在整读（std::fs::read 回归）').not.toContain('std::fs::read(');
    expect(collectBody, '激活摘要未经文件句柄流入').toContain('std::fs::File::open');

    const hostProbe = read('crates/tauron-host/examples/perf_probe.rs');
    expect(hostProbe, 'perf_probe 仍在读 VmHWM（装饰性未断言读数）').not.toContain(
      '/proc/self/status',
    );
    expect(hostProbe, 'perf_probe 仍在输出 peakRssKb（门禁已不读它）').not.toContain('peakRssKb');

    // ⑤ 口径成文。
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案缺轮 44 小节').toMatch(/### 轮 44：/);
    const a94Row = /^\| A94 .*$/m.exec(plan)?.[0] ?? '';
    expect(a94Row, 'A94 台账行消失（行名改了要重新复核）').not.toBe('');
    expect(a94Row, 'A94 台账行未登记轮 44 落地').toMatch(/轮 44/);
    expect(a94Row, 'A94 台账行仍写"部分"（代码与宣称脱节）').not.toMatch(/\*\*部分\*\*/);
  });

  it('轮 45：Tier↔Bundle↔命令子集三张表与命令面同源（A65 Profile V2）', () => {
    // 轮 45 立的规矩：Profile V2 的「深度档 × 能力捆绑」从修辞变成三张可判定表。
    // 这条门禁钉表形状本身（键集 / requires 字面 / 状态诚实）与布线（脚本、链、CI、
    // 台账行）；语义一致性（85 条命令单属划分、§87 正交性）由
    // scripts/check-tier-bundle.mjs 在本地 gates:check 与 CI 上实跑判定。
    const tiers = JSON.parse(read('contracts/tier-bundles.json')) as {
      tiers: Record<string, { requires: string[] }>;
      surfaceBundles: string[];
      bundles: Record<string, { kind: string; status: string; commands: string[] }>;
    };
    expect(Object.keys(tiers.tiers).sort(), '三档必须恰为 S/E/P').toEqual(['E', 'P', 'S']);
    expect(
      Object.keys(tiers.bundles).sort(),
      'bundles 键必须恰为 §87.2 的 10 个（改名/别名都得同时改门禁）',
    ).toEqual([
      'core',
      'desktop-ui',
      'enterprise-policy',
      'headless',
      'marketplace',
      'mobile-ui',
      'observability',
      'plugin-runtime',
      'remote-client',
      'service',
    ]);
    expect([...tiers.surfaceBundles].sort(), '形态捆绑清单被改动（正交性断言的分母）').toEqual([
      'desktop-ui',
      'headless',
      'mobile-ui',
      'remote-client',
      'service',
    ]);
    // requires 字面锁死：S⊂E⊂P 的单调链是 Profile V2 的核心语义，改链必须同时改这里。
    const requiresOf = (tier: string): string[] => tiers.tiers[tier]?.requires ?? [];
    expect(requiresOf('S'), 'S 档必备捆绑变了').toEqual(['core']);
    expect(requiresOf('E'), 'E 档必备捆绑变了').toEqual(['core', 'plugin-runtime']);
    expect(requiresOf('P'), 'P 档必备捆绑变了').toEqual(['core', 'plugin-runtime', 'marketplace']);
    // 状态诚实：已落 ⇔ 非空（门禁脚本另有一份运行时判定，这里钉的是形状不可空转）。
    for (const [bundle, spec] of Object.entries(tiers.bundles)) {
      if (spec.status === '已落') {
        expect(spec.commands.length, `${bundle} 标已落却零命令`).toBeGreaterThan(0);
      } else {
        expect(spec.status, `${bundle} 状态不在口径内`).toBe('roadmap');
        expect(spec.commands.length, `${bundle} 标 roadmap 却挂命令`).toBe(0);
      }
    }

    // 布线三件：脚本在场（含关键判定文本）→ pnpm 入口在 gates:check 链上 → CI 独立步骤。
    const tierGate = read('scripts/check-tier-bundle.mjs');
    for (const needle of ['tier-bundles.json', 'command-surface.md', 'Tier-Bundle gate FAILED']) {
      expect(tierGate, `门禁脚本缺 ${needle}（判定被掏空）`).toContain(needle);
    }
    const pkg = read('package.json');
    const gatesChain = /"gates:check":\s*"([^"]+)"/.exec(pkg)?.[1] ?? '';
    expect(gatesChain, 'gates:check 链缺 check-tier-bundle（本地不再跑它）').toContain(
      'check-tier-bundle.mjs',
    );
    const ci = read('.github/workflows/ci.yml');
    expect(ci, 'CI 缺 Tier-Bundle 步骤').toContain('node scripts/check-tier-bundle.mjs');

    // 口径成文：A65 台账行翻篇 + 轮 45 小节在场 + 轮 11「未做」清单已划掉 A65。
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案缺轮 45 小节').toMatch(/### 轮 45：/);
    const a65Row = /^\| A65 .*$/m.exec(plan)?.[0] ?? '';
    expect(a65Row, 'A65 台账行消失（行名改了要重新复核）').not.toBe('');
    expect(a65Row, 'A65 台账行未登记轮 45 落地').toMatch(/轮 45/);
    expect(a65Row, 'A65 台账行仍写"未落"（代码与宣称脱节）').not.toMatch(/未落/);
    expect(plan, '轮 11 未做清单未划掉 A65').toContain('~~A65');
  });

  it('轮 46：零 Surface 就绪集合与真跑 conformance 同形（A66 ReadinessSet）', () => {
    // 轮 46 立的规矩：「headless 可零 Surface 运行」从判词变成可求值集合 + 真跑用例。
    // 这条门禁钉模块形状（六组事实 / 具名 blockers 入口 / headless 构造）、三条
    // conformance 测试名在场、导出行、台账行翻篇与 CHANGELOG；语义（空集不阻塞 /
    // 声明即须兑现 / kernel 不豁免）由 v4_host_conformance.rs 三条用例在 cargo test
    // 与本地电池上实跑判定——删掉任一用例必须同时改这里。
    const readiness = read('crates/tauron-host/src/readiness.rs');
    for (const needle of [
      'pub struct ReadinessSet',
      'pub fn headless(',
      'pub required_surfaces: Vec<ReadinessFact>,',
      'pub blockers: Vec<String>,',
    ]) {
      expect(readiness, `readiness.rs 缺 ${needle}（A66 形状被掏空）`).toContain(needle);
    }
    const hostLib = read('crates/tauron-host/src/lib.rs');
    expect(hostLib, 'readiness 未导出（消费者拿不到）').toContain(
      'pub use readiness::{ApplicationReadiness, ReadinessFact, ReadinessSet};',
    );
    const conformance = read('crates/tauron-host/tests/v4_host_conformance.rs');
    for (const name of [
      'conform_zero_surface_host_reaches_ready_without_any_surface',
      'conform_declared_surface_blocks_readiness_with_named_reason',
      'conform_unready_kernel_blocks_even_without_surfaces',
    ]) {
      expect(conformance, `conformance 缺 ${name}（零 Surface 就绪判定被删）`).toContain(name);
    }

    // 口径成文：A66 台账行翻篇 + 轮 46 小节在场 + 轮 11「未做」清单已划掉 A66 + CHANGELOG 条目。
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    // 行首锚定：变异证明第一遍暴露过 `/### 轮 46：/` 会被 `#### 轮 46：` 子串命中的假绿。
    expect(plan, '缺口方案缺轮 46 小节').toMatch(/^### 轮 46：/m);
    const a66Row = /^\| A66 .*$/m.exec(plan)?.[0] ?? '';
    expect(a66Row, 'A66 台账行消失（行名改了要重新复核）').not.toBe('');
    expect(a66Row, 'A66 台账行未登记轮 46 落地').toMatch(/轮 46/);
    expect(a66Row, 'A66 台账行仍写"未落"（代码与宣称脱节）').not.toMatch(/未落/);
    expect(plan, '轮 11 未做清单未划掉 A66').toContain('~~A66');
    const changelog = read('CHANGELOG.md');
    expect(changelog, 'CHANGELOG 缺轮 46 条目').toMatch(/轮 46，A66/);
  });

  it('轮 47：事件/审批/注册表三条故障边界各带确定性修复语义与实测（A91）', () => {
    // 轮 47 立的规矩：A91 的「一个子系统 panic 不得升级为全宿主风险」从 settings
    // 一条边界扩到三条，且每条修复语义必须诚实——events 保留 topic 声明（发布端对
    // 未声明 topic 只静默丢弃不报错，清声明=把可工作的消息面悄悄打哑）、approval
    // 重新 preview 清令牌、registry 无会话内重建就如实 Quarantined。这条门禁钉
    // 形状/接线数/修复文案/负向回潮/台账翻篇；语义（清零后声明仍在、Quarantined 不
    // 假 Ready、修复后旧令牌失效）由 adapter 三条测试在 cargo test 与电池上实跑判定。
    const eventbus = read('crates/tauron-host/src/eventbus.rs');
    for (const needle of [
      // 带签名收尾：裸名是 `..._x` 改名的字符串前缀，`.includes()` 会放过（轮 47 变异
      // W4 实测假绿，同轮 46 未锚定 regex 一类）。
      'pub fn reset_session_state(&self)',
      'session_reset_clears_subscriptions_but_keeps_topic_declarations',
    ]) {
      expect(eventbus, `eventbus.rs 缺 ${needle}（会话态重置/声明存活被掏空）`).toContain(needle);
    }

    const adapter = read('crates/tauron-adapter/src/lib.rs');
    for (const needle of [
      'fn run_subsystem_boundary<T>(',
      'fn run_events_boundary<T>(',
      'fn run_review_boundary<T>(',
      'fn run_registry_boundary<T>(',
      'fn begin_subsystem_reconcile(',
      'fn reconcile_events_boundary(',
      'fn reconcile_review_boundary(',
      'fn reconcile_registry_boundary(',
      'events_fault: Arc<Mutex<tauron_host::FaultBoundary>>',
      'review_fault: Arc<Mutex<tauron_host::FaultBoundary>>',
      'registry_fault: Arc<Mutex<tauron_host::FaultBoundary>>',
      'tauron_host::FaultBoundary::new("events")',
      'tauron_host::FaultBoundary::new("approval")',
      'tauron_host::FaultBoundary::new("registry")',
      'events_fault_boundary_rejects_bus_work_and_recover_boot_resets_session_state',
      'approval_fault_boundary_blocks_tokens_and_preview_reconcile_clears_them',
      'registry_fault_boundary_quarantines_until_reassembly_not_faked_ready',
      // 三条修复文案：各带具名的下一步动作，操作者读错误就知道怎么修
      'topic declarations are kept',
      'clears pending approval tokens',
      'no in-session rebuild exists',
    ]) {
      expect(adapter, `lib.rs 缺 ${needle}（轮 47 的边界形状/修复语义被掏空）`).toContain(needle);
    }

    // 接线数：定义在场还不够，闸门要真的包在命令面上（定义与测试调用不匹配该计数模式）。
    const countSites = (re: RegExp): number => [...adapter.matchAll(re)].length;
    expect(countSites(/run_events_boundary\(state, "/g), '事件命令闸门接线数变了（7 条）').toBe(7);
    expect(countSites(/run_review_boundary\(state, "/g), '令牌消费点闸门接线数变了（4 个）').toBe(
      4,
    );
    expect(countSites(/run_registry_boundary\(state, "/g), '注册表命令闸门接线数变了（5 条）').toBe(
      5,
    );
    expect(
      countSites(/reconcile_review_boundary\(state\)\?;/g),
      '审批修复不在两条 preview 上骑车了',
    ).toBe(2);

    // 负向钉：禁止整重置回潮——`EventBus::default()` 连 topic 声明一起清，
    // 发布端静默丢弃不报错，等于让修复动作把可工作的消息面打哑。
    expect(adapter, '事件修复重新用整重置（topic 声明会被清掉，发布静默丢弃）').not.toMatch(
      /\*state\.bus\.lock\(\)\s*=\s*EventBus::default\(\);/,
    );
    // 修复驱动：cmd_recover_boot 必须驱动 events+registry 两条修复（审批修复另有骑点）。
    const boot = adapter.slice(adapter.indexOf('pub fn cmd_recover_boot('));
    expect(boot, 'cmd_recover_boot 不再驱动事件修复').toContain('reconcile_events_boundary(state)');
    expect(boot, 'cmd_recover_boot 不再驱动注册表修复').toContain(
      'reconcile_registry_boundary(state)',
    );

    // 口径成文：A91 台账行翻篇 + 轮 47 小节在场 + 轮 11「未做」清单已划掉 A91 + CHANGELOG。
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案缺轮 47 小节').toMatch(/^### 轮 47：/m);
    const a91Row = /^\| A91 .*$/m.exec(plan)?.[0] ?? '';
    expect(a91Row, 'A91 台账行消失（行名改了要重新复核）').not.toBe('');
    expect(a91Row, 'A91 台账行未登记轮 47 落地').toMatch(/轮 47/);
    expect(a91Row, 'A91 台账行仍写"未落"（代码与宣称脱节）').not.toMatch(/未落/);
    expect(plan, '轮 11 未做清单未划掉 A91').toContain('~~A91');
    const changelog = read('CHANGELOG.md');
    expect(changelog, 'CHANGELOG 缺轮 47 条目').toMatch(/轮 47，A91/);
  });

  it('轮 48：HTTP 重定向/DNS 复检接生产——单跳契约 + 宿主侧逐跳授权（A96）', () => {
    // 轮 48 立的规矩：A96 的「重定向/解析尊重 scope」此前只有策略实现（模块自测），
    // 生产命令只做首跳授权。这条门禁钉：单跳 sink 契约（不得自行跟随 3xx、必须回报
    // 解析地址）、宿主侧逐跳链（resolve_redirect_location → authorize_redirect →
    // authorize_resolution）真的在 cmd_http_request 里、跨源凭据剥离在场、能力闸把
    // 非 RedirectAndDns 档挡住；语义（RFC 3986 归一转、被拒目标不发、hop 上界、
    // 私网复检）由 adapter 四条测试在 cargo test 与电池上实跑判定。
    const netpol = read('crates/tauron-host/src/network_policy.rs');
    for (const needle of [
      // 带签名收尾：裸名可能成为改名后的字符串前缀（轮 47 变异 W4 实测假绿一类）。
      'pub fn resolve_redirect_location(base: &str, location: &str)',
      'base.join(location)',
    ]) {
      expect(netpol, `network_policy.rs 缺 ${needle}（RFC 3986 相对解析被掏空）`).toContain(needle);
    }
    const hostLib = read('crates/tauron-host/src/lib.rs');
    const hostExport = hostLib.slice(hostLib.indexOf('pub use network_policy::{'));
    expect(hostExport, 'tauron-host 未导出 resolve_redirect_location').toContain(
      'resolve_redirect_location',
    );

    const adapter = read('crates/tauron-adapter/src/lib.rs');
    for (const needle of [
      'fn http_policy_denied(',
      'pub fn with_http_sink(mut self, sink: Arc<dyn HttpSink>) -> Self',
      'pub resolved_addrs: Vec<String>',
      // 单跳契约成文（provider 责任 + 宿主职责）
      '发起**单跳**请求',
      '实现**不得**自行跟随 redirect',
      '实现**必须**在响应里回报本跳解析地址',
      // 宿主逐跳链：归一转址 → 授权 → 解析复检 → 凭据剥离
      'tauron_host::resolve_redirect_location(&current_url, location)',
      '.authorize_redirect(&current, &next_url, hop, has_credentials)',
      '.authorize_resolution(&current, &resolved)',
      'raw.parse::<std::net::IpAddr>()',
      'if !authorization.forward_credentials',
      'http_sink: cfg.http_sink.clone().unwrap_or_else(|| Arc::new(UnavailableHttpSink))',
      // 五条行为测试
      'http_redirect_flow_follows_hops_with_rfc_3986_resolution',
      'http_redirect_cross_origin_strips_credentials_and_denied_target_never_sent',
      'http_redirect_loop_is_bounded_by_policy_max_redirects',
      'http_provider_resolution_is_rechecked_against_private_network_policy',
      'http_provider_without_redirect_enforcement_is_rejected_before_any_request',
    ]) {
      expect(adapter, `lib.rs 缺 ${needle}（轮 48 的单跳契约/逐跳授权被掏空）`).toContain(needle);
    }

    // 接线数：三处策略调用必须各有一段——定义在场 ≠ 宿主在跑这条链。
    const countSites = (re: RegExp): number => [...adapter.matchAll(re)].length;
    expect(countSites(/\.authorize_redirect\(/g), 'authorize_redirect 调用点变了（应 1 处）').toBe(
      1,
    );
    expect(
      countSites(/\.authorize_resolution\(/g),
      'authorize_resolution 调用点变了（应 1 处）',
    ).toBe(1);
    expect(
      countSites(/"authorization" \| "cookie" \| "proxy-authorization"/g),
      '跨源凭据剥离三头不在（判定 + 剥离应各一处）',
    ).toBe(2);

    // 能力闸：非 RedirectAndDns 档不得放行（策略不可执行时拒绝发请求，而非裸发）。
    const cmd = adapter.slice(adapter.indexOf('pub fn cmd_http_request('));
    expect(cmd, '能力闸被拆（非 RedirectAndDns 档会裸发请求）').toContain(
      'tauron_host::NetworkEnforcement::RedirectAndDns',
    );

    const shell = read('packages/tauron-host/src/shell-client.ts');
    expect(shell, 'TS 线形缺 resolvedAddrs（provider 回报地址的线形出口）').toContain(
      'resolvedAddrs?: string[];',
    );

    // 口径成文：A96 台账行翻篇 + 轮 48 小节在场 + 轮 11「未做」清单已划掉 A96 + CHANGELOG。
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案缺轮 48 小节').toMatch(/^### 轮 48：/m);
    const a96Row = /^\| A96 .*$/m.exec(plan)?.[0] ?? '';
    expect(a96Row, 'A96 台账行消失（行名改了要重新复核）').not.toBe('');
    expect(a96Row, 'A96 台账行未登记轮 48 落地').toMatch(/轮 48/);
    expect(a96Row, 'A96 台账行仍写"部分"（代码与宣称脱节）').not.toMatch(/\*\*部分\*\*/);
    expect(
      (plan.match(/~~A96/g) ?? []).length,
      'A96 未做清单划线数变了（轮 11 与 Batch 5 应各 1 处）',
    ).toBe(2);
    expect(plan, '能力矩阵行未翻篇').toMatch(
      /^\| network redirects\/resolution respect scope \| ✅ \|/m,
    );
    const surface = read('docs/api/command-surface.md');
    expect(surface, '命令面文档未登记单跳契约').toContain('resolvedAddrs');
    const changelog = read('CHANGELOG.md');
    expect(changelog, 'CHANGELOG 缺轮 48 条目').toMatch(/轮 48，A96/);
  });

  it('轮 49：A71 变体过滤接进安装/更新路径——resolve_best 从在册孤儿变为消费者', () => {
    // 轮 49 立的规矩：`resolve_best` 此前是台账在册孤儿（「安装/更新路径不过滤变体」），
    // `ArtifactVariantResolver` 只存在于注释。这条门禁钉：resolver 实现在场、装配腿在
    // **任何下载/解压/OS 加载之前**做变体解析、无兼容即硬拒（不静默回退单清单）、
    // 台账条目已删除（仍留着 = 文档在说「未接线」而代码已接）；语义（选中最具体、
    // 无兼容失败关闭、download/install 同口径）由 host/adapter 四条测试在 cargo 上实跑判定。
    const target = read('crates/tauron-host/src/target.rs');
    for (const needle of [
      // 带签名收尾：改名或泛型收窄都会破配（子串针会放过后缀式改名）。
      "pub fn resolve_best<'a, T: AsRef<TargetSpec>>(",
      "pub fn resolve<'a, T: AsRef<TargetSpec>>(",
      // 测试名带 `fn ` 与左括号收尾：后缀式改名会破配（裸名会被 `_x` 后缀骗过）。
      'fn artifact_variant_resolver_returns_none_when_no_variant_is_runnable(',
    ]) {
      expect(target, `target.rs 缺 ${needle}（resolve_best 泛型化/Resolver 入口被掏空）`).toContain(
        needle,
      );
    }
    const hostLib = read('crates/tauron-host/src/lib.rs');
    // 带逗号分界：裸名针会被 `ArtifactVariantResolverX` 这类后缀改名骗过（轮 49 变异 W11 实测）。
    expect(hostLib, 'tauron-host 未导出 ArtifactVariantResolver').toContain(
      'ArtifactVariantResolver, TargetAbi,',
    );

    const adapter = read('crates/tauron-adapter/src/lib.rs');
    for (const needle of [
      'pub struct UpdateArtifactVariant {',
      // 带 ` {` 收尾：`UpdateArtifactVariantX {` 后缀式改名必须破配（轮 49 变异 W8 实测）。
      'impl AsRef<tauron_host::TargetSpec> for UpdateArtifactVariant {',
      'pub fn with_variants(mut self, variants: Vec<UpdateArtifactVariant>) -> Self',
      'tauron_host::ArtifactVariantResolver::resolve(&self.variants, &host)',
      // 全量文案针：只有「无一兼容本机目标」短语会被测试文件的断言文本喂饱（轮 49 变异 C2 实测）。
      '更新清单提供 {} 个变体，无一兼容本机目标（os={:?} arch={:?}）：拒绝下载，不静默回退单包',
      'fn variant_list_without_compatible_target_is_rejected_before_any_download(',
      'fn download_and_install_both_use_the_variant_compatible_with_this_host(',
    ]) {
      expect(adapter, `lib.rs 缺 ${needle}（轮 49 的变体过滤被掏空）`).toContain(needle);
    }
    // 计数钉：resolver 调用点恰一处（select_manifest），download/install 都经它。
    const countSites49 = (re: RegExp): number => [...adapter.matchAll(re)].length;
    expect(
      countSites49(/ArtifactVariantResolver::resolve\(/g),
      'ArtifactVariantResolver::resolve 调用点变了（应 1 处：select_manifest）',
    ).toBe(1);

    // 台账：条目必须删除（仍在 = 文档还在宣称「resolve_best 未接线」而代码已接）。
    const ledger = read('contracts/orphan-public-api.json');
    expect(
      (ledger.match(/resolve_best/g) ?? []).length,
      '孤儿台账仍登记 resolve_best（A71 已接线，条目必须删除）',
    ).toBe(0);

    // 口径成文：A71 台账行翻篇（不得再写「部分」）+ 轮 49 小节在场 + CHANGELOG。
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案缺轮 49 小节').toMatch(/^### 轮 49：/m);
    const a71Row = /^\| A71 .*$/m.exec(plan)?.[0] ?? '';
    expect(a71Row, 'A71 台账行消失（行名改了要重新复核）').not.toBe('');
    expect(a71Row, 'A71 台账行未登记轮 49 落地').toMatch(/轮 49/);
    expect(a71Row, 'A71 台账行仍写「部分」（代码与宣称脱节）').not.toMatch(/\*\*部分\*\*/);
    const changelog = read('CHANGELOG.md');
    expect(changelog, 'CHANGELOG 缺轮 49 条目').toMatch(/轮 49，A71/);
  });
  it('轮 54：降级门禁接进升级执行器——is_downgrade 从在册孤儿变为 distribute 侧的单一算术消费者', () => {
    // 轮 54 立的规矩：此前文档敢写「降级门禁另有实现（distribute 侧）」，而 `UpgradeRunner::validate`
    // 里一个版本比较都没有 ⇒ 一条低于已装版本的清单会被当正常升级走完 download→extract→swap。
    // 这条门禁钉四件事：①版本序的**唯一算术源**仍是 market 的 `cmp_version`/`is_downgrade`
    // （distribute 靠依赖边复用，不许自己再写一份解析）；②门禁在 `validate` 这个零副作用段里；
    // ③装配腿 download/install 共用同一判据（轮 49 口径）；④台账把 `is_downgrade` 换成它非对偶的
    // `is_monotonic`。语义（数字序不是字符串序、相等放行、无基线如实放行、拒绝先于任何文件效果）
    // 由 distribute/adapter 三条测试在 cargo 上实跑判定。
    const cargo = read('crates/tauron-distribute/Cargo.toml');
    // 行锚定（不带 `#` 前缀也算命中是轮 54 变异 W1 实测的假绿：把依赖行注释掉，
    // `toContain` 照样满意，而 crate 早就没有那条边了）。
    expect(cargo, 'distribute 不再依赖 tauron-market（版本序要变成第二套实现了）').toMatch(
      /^tauron-market = \{ path = "\.\.\/tauron-market", version = "1\.1\.0" \}/m,
    );

    const upgrade = read('crates/tauron-distribute/src/upgrade.rs');
    for (const needle of [
      // 带签名收尾：改名或收窄参数形状都会破配（后缀式改名会骗过裸名针）。
      'pub fn ensure_not_downgrade(installed: Option<&str>, target: &str) -> DistributeResult<()> {',
      'tauron_market::is_downgrade(current, target)',
      'ensure_not_downgrade(self.options.installed_version.as_deref(), &manifest.version)?;',
      // 测试名带 `fn ` 与左括号收尾。
      'fn validate_rejects_downgrade_with_zero_file_effect(',
      'fn downgrade_gate_uses_numeric_version_order_not_strings(',
    ]) {
      expect(upgrade, `upgrade.rs 缺 ${needle}（轮 54 的降级门禁被掏空）`).toContain(needle);
    }
    // 计数钉：执行器侧生产调用点恰一处（validate）。
    expect(
      [...upgrade.matchAll(/ensure_not_downgrade\(self\.options\.installed_version/g)].length,
      'ensure_not_downgrade 在 upgrade.rs 的生产调用点变了（应 1 处：validate）',
    ).toBe(1);
    expect(
      [...upgrade.matchAll(/\bDistributeError::DowngradeRejected\b/g)].length,
      'DowngradeRejected 在 upgrade.rs 出现次数变了（应 2 处：门禁构造 + 测试断言）',
    ).toBe(2);

    const errors = read('crates/tauron-distribute/src/error.rs');
    expect(errors, 'distribute 缺具名降级错误').toContain('DowngradeRejected { current: String');

    const distLib = read('crates/tauron-distribute/src/lib.rs');
    expect(distLib, 'tauron-distribute 未导出 ensure_not_downgrade').toContain(
      'download_bounded, ensure_not_downgrade,',
    );

    const adapter = read('crates/tauron-adapter/src/lib.rs');
    for (const needle of [
      'tauron_distribute::ensure_not_downgrade(',
      '| D::DowngradeRejected { .. }',
      'fn downgrade_manifest_is_rejected_before_any_download(',
    ]) {
      expect(adapter, `lib.rs 缺 ${needle}（装配腿的降级门禁被掏空）`).toContain(needle);
    }
    // 计数钉：download/install 共用 select_manifest，调用点恰一处。
    expect(
      [...adapter.matchAll(/tauron_distribute::ensure_not_downgrade\(/g)].length,
      'ensure_not_downgrade 在 adapter 的调用点变了（应 1 处：select_manifest，两腿同判据）',
    ).toBe(1);

    // 台账：`is_downgrade` 条目必须删除（仍在 = 文档还在宣称它未接线），
    // 且必须留下非对偶的 `is_monotonic`（拿它当替身会放行等版本降级）。
    const ledger = read('contracts/orphan-public-api.json');
    expect(
      (ledger.match(/"symbol": "is_downgrade"/g) ?? []).length,
      '孤儿台账仍登记 is_downgrade（轮 54 已接线，条目必须删除）',
    ).toBe(0);
    expect(
      (ledger.match(/"symbol": "is_monotonic"/g) ?? []).length,
      '孤儿台账缺 is_monotonic 条目（它与门禁的相等判定不同，不得一起销账）',
    ).toBe(1);

    // 宣称链：market 侧文档注释必须跟着翻篇，缺口方案有轮 54 小节，CHANGELOG 有对应条目。
    const market = read('crates/tauron-market/src/lib.rs');
    expect(market, 'market 的 is_downgrade 注释仍宣称未接线').toContain('已接线（轮 54）');
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案缺轮 54 小节').toMatch(/^### 轮 54：/m);
    const changelog = read('CHANGELOG.md');
    expect(changelog, 'CHANGELOG 缺轮 54 条目').toMatch(/轮 54/);
  });
  it('轮 55：安装预览的审批行改由 acl 审批构造器单源生成（build_approval_rows 不再是孤儿）', () => {
    // 轮 55 立的规矩：预览链路过去自己内联造行，把「高危档默认不勾」这条 §4.5 规则抄了第二份
    // （acl 里已有），并且**丢掉了 scope 与确认词**——前端拿不到高危确认文案，只能自己硬编码，
    // 而那正是 §4.5 禁止的。门禁钉：①行由 `build_approval_rows` 生成；②内联副本不许回来；
    // ③两个新可选字段带出去且缺席时整字段省略（线形与既有 TS 镜像一致）；④TS 类型同步；
    // ⑤台账条目已删除。语义（顺序=manifest 顺序、文案=词表原文、高危不勾+确认词、无 scope 为 None）
    // 由适配层测试在 cargo 上实跑判定。
    const adapter = read('crates/tauron-adapter/src/lib.rs');
    for (const needle of [
      'tauron_acl::build_approval_rows(&draft, &index)',
      'let confirmation_hint = row.confirmation_hint().map(str::to_string);',
      'pub confirmation_hint: Option<String>,',
      'fn preview_approval_rows_come_from_the_acl_builder(',
    ]) {
      expect(adapter, `lib.rs 缺 ${needle}（轮 55 的审批行单源接线被掏空）`).toContain(needle);
    }
    // 反向钉：内联抄的那份默认勾选规则不许回来。
    expect(
      adapter,
      'lib.rs 又出现了内联的「risk != High」默认勾选副本（§4.5 规则必须只有 acl 一份）',
    ).not.toContain('default_checked: grant.risk != tauron_host::manifest::Risk::High');
    // 计数钉：两个可选审批字段各一根 skip_serializing_if（缺席写成 null 会破既有线形）。
    expect(
      [...adapter.matchAll(/#\[serde\(skip_serializing_if = "Option::is_none"\)\]/g)].length,
      'PluginPermissionReview 的可选字段省略标记数量变了（应 2 处：scope / confirmationHint）',
    ).toBe(2);

    const ledger = read('contracts/orphan-public-api.json');
    expect(
      (ledger.match(/"symbol": "build_approval_rows"/g) ?? []).length,
      '孤儿台账仍登记 build_approval_rows（轮 55 已接线，条目必须删除）',
    ).toBe(0);

    const acl = read('crates/tauron-acl/src/approval.rs');
    expect(acl, 'acl 的 build_approval_rows 注释仍宣称无人调用').toContain('已接线（轮 55）');

    // （轮 56 更正：TS 侧行形状收敛成 `grants.ts` 的 `ApprovalRow` 单处声明，host.ts 只引用。
    // 因此「host.ts 把三个字段带出来」这根针检查的对象已经不在了，改由轮 56 段
    // 直接钉「引用单源类型 + 不再内联字段列表」。台账/勾选规则/确认词三根针仍属本轮。）

    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案缺轮 55 小节').toMatch(/^### 轮 55：/m);
    const changelog = read('CHANGELOG.md');
    expect(changelog, 'CHANGELOG 缺轮 55 条目').toMatch(/轮 55/);
  });

  it('轮 56：壳层安装流真的消费宿主的审批判定（defaultChecked/scope/confirmationHint）', () => {
    // 轮 55 把判定收到宿主单源，但线上事实到壳层就被丢掉：`_installPlugin` 对每一行都问
    // 同一句「是否授予」，`defaultChecked`/`scope`/`confirmationHint` 三个字段零读者——
    // 「后端判了、前端没接」正是本仓库要的断链。门禁钉：①循环按 `defaultChecked` 分叉；
    // ②高危行走宿主确认词，且**比对是逐字相等**（不是包含、不是本地另拟文案）；
    // ③`scope` 进显示文本；④TS 行类型只在 `grants.ts` 声明一次，host.ts 只引用；
    // ⑤确认词字面量在 TS 侧零出现（唯一来源是 Rust `tauron-acl`）。
    const shell = read('packages/tauron-host/src/shell-controller.ts');
    for (const needle of [
      'if (permission.defaultChecked) {',
      'if (!this._confirmHighRiskRow(detail, permission)) return;',
      'const hint = permission.confirmationHint;',
      'if (answer !== hint) {',
      "(permission.scope ? `\\nscope: ${permission.scope}` : '')",
    ]) {
      expect(shell, `shell-controller.ts 缺 ${needle}（宿主判定又被丢回了）`).toContain(needle);
    }
    // 反向钉：确认词文案的唯一来源是宿主，壳层不得内联一份。
    expect(
      shell,
      '壳层内联了确认词文案（§4.5 规定其唯一来源是 acl 的常量，前端只能引用宿主给的字段）',
    ).not.toContain('我理解该权限的能力边界并显式批准');
    // 反向钉：壳层不自行按 risk 重算默认勾态。
    expect(shell, '壳层又出现本地 risk→勾选判定（该结论由宿主的 defaultChecked 给出）').not.toMatch(
      /risk\s*===\s*'high'/,
    );

    const grants = read('packages/tauron-host/src/grants.ts');
    for (const needle of [
      'confirmationHint?: string;',
      'defaultChecked: boolean;',
      'scope?: string;',
    ]) {
      expect(grants, `grants.ts 的 ApprovalRow 少了 ${needle}`).toContain(needle);
    }
    // 反向钉：`humanText` 是轮 56 删掉的那个平行字段名（线上真名是 `description`）。
    expect(grants, 'grants.ts 还留着与线上不同名的 humanText 字段').not.toContain('humanText');

    // 单源声明计数：行形状在 TS 侧只许有一处字段列表（grants.ts），host.ts 只引用类型。
    const hostTs = read('packages/tauron-host/src/host.ts');
    expect(
      [...hostTs.matchAll(/permissions: ApprovalRow\[\];/g)].length,
      'host.ts 的预览行形状没收敛到单源类型（应 2 处引用：返回声明 + invoke 泛型）',
    ).toBe(2);
    expect(
      hostTs,
      'host.ts 又内联了一遍确认词字段（字段列表只能在 grants.ts 存在一份）',
    ).not.toContain('confirmationHint?: string;');
    expect(
      [...grants.matchAll(/confirmationHint\?: string;/g)].length,
      'grants.ts 的确认词字段声明数量变了（应 1 处：ApprovalRow）',
    ).toBe(1);

    const acl = read('crates/tauron-acl/src/approval.rs');
    expect(acl, 'acl 的确认词常量不见了（壳层就没有可比对的真源）').toContain(
      '我理解该权限的能力边界并显式批准',
    );

    const shellTest = read('packages/tauron-host/src/shell-controller.test.ts');
    expect(shellTest, '缺轮 56 的行为测试（不一致即中止）').toContain(
      '轮 56：宿主判为高危的行必须逐字输入确认词',
    );

    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案缺轮 56 小节').toMatch(/^### 轮 56：/m);
    const changelog = read('CHANGELOG.md');
    expect(changelog, 'CHANGELOG 缺轮 56 条目').toMatch(/轮 56/);
  });

  it('轮 57：通知容量设置键接通 NotifyStore::trim_to（判定一处、生效两点、先判后写）', () => {
    // 轮 57 立的规矩：`trim_to` 此前是「库内 API + 单测」的孤儿——用户侧没有任何入口能让
    // 通知环形缓冲收缩，容量在宿主里是装配期的一个字面量。命令面 85 条冻结 ⇒ 只能挂设置键。
    // 门禁钉：①容量判定只有一处实现（`parse_notify_capacity`）；②生效点恰两处（装配期读盘 +
    // `host_settings_set` 落盘提交后），且校验发生在**落盘之前**；③内建默认是常量、不是被抄的
    // 字面量；④磁盘坏值必须留痕（降级不许静默）；⑤孤儿条目已删、声明侧注释改口；
    // ⑥四条行为测试在。语义（驱逐最旧、unread 记账、非法值零副作用、重启仍生效、坏值不拒启）
    // 由 cargo 实跑判定。
    const adapter = read('crates/tauron-adapter/src/lib.rs');
    const capacity = read('crates/tauron-adapter/src/notify_capacity.rs');
    // 判定与生效的**实现**都在域文件里（轮 57：`lib.rs` 顶层条目已封顶，新能力按台账建议开新域文件）。
    for (const needle of [
      'pub(crate) const NOTIFICATIONS_CAPACITY_KEY: &str = "notifications.capacity";',
      'pub(crate) const NOTIFICATIONS_CAPACITY_MAX: usize = 4096;',
      'pub(crate) const NOTIFICATIONS_CAPACITY_DEFAULT: usize = 512;',
      'pub(crate) fn parse_notify_capacity(value: &serde_json::Value) -> HostResult<usize>',
      'pub(crate) fn apply_notify_capacity(state: &SubstrateState) -> HostResult<Option<usize>>',
      '.trim_to(capacity)',
    ]) {
      expect(capacity, `notify_capacity.rs 缺 ${needle}（容量的单源实现被搬空）`).toContain(needle);
    }
    // `lib.rs` 只许留**调用点**：装配期读盘一次、写前判定一次、落盘提交后生效一次。
    for (const needle of [
      'mod notify_capacity;',
      'match notify_capacity_from_settings(&settings) {',
      'NotifyStore::new(NOTIFICATIONS_CAPACITY_DEFAULT)',
      'if key == NOTIFICATIONS_CAPACITY_KEY {\n            parse_notify_capacity(&value)?;',
      'if key == NOTIFICATIONS_CAPACITY_KEY {\n            apply_notify_capacity(state)?;',
    ]) {
      expect(adapter, `lib.rs 缺 ${needle}（轮 57 的容量接线被掏空）`).toContain(needle);
    }
    // 行锚定：`toContain` 只认子串，把整行注释掉仍算命中——域文件必须真的被挂上。
    expect(adapter, 'lib.rs 的 mod notify_capacity; 只是作为注释存在（域文件没真接上）').toMatch(
      /^mod notify_capacity;$/m,
    );
    // 计数钉（跨两个文件）：裁剪调用点恰 2、判定/生效各只有 1 份实现、调用点各 1 处。
    expect(
      [...`${adapter}${capacity}`.matchAll(/\.trim_to\(/g)].length,
      '环形缓冲裁剪的调用点变了（应 2：装配期 + 落盘后；别处不得再抄一份裁剪）',
    ).toBe(2);
    expect(
      [...`${adapter}${capacity}`.matchAll(/fn parse_notify_capacity\(/g)].length,
      '容量判定函数长了第二份实现（判定必须只有一处）',
    ).toBe(1);
    expect(
      [...`${adapter}${capacity}`.matchAll(/fn apply_notify_capacity\(/g)].length,
      '容量生效函数长了第二份实现（生效必须只有一处）',
    ).toBe(1);
    expect(
      [...`${adapter}${capacity}`.matchAll(/parse_notify_capacity\(/g)].length,
      '判定的定义+调用点数量变了（轮 59 后应 4：定义 + 读盘判定 + 写前判定 + 整份文档校验）',
    ).toBe(4);
    expect(
      [...`${adapter}${capacity}`.matchAll(/notify_capacity_from_settings\(/g)].length,
      '读盘事实的定义+调用点数量变了（轮 59 后应 4：定义 + 生效内 + 装配期 + 迁移落盘前）',
    ).toBe(4);
    // 反向钉：内建默认只能是常量。
    expect(
      adapter,
      '装配处又把内建默认容量抄成了字面量（「默认」与「键缺席」必须是同一个事实）',
    ).not.toContain('NotifyStore::new(512)');
    // 反向钉：实现不许被搬回 `lib.rs`（那是本仓的 god file，条目预算已封顶）。
    expect(
      adapter,
      '容量的判定/生效实现又搬回了 lib.rs（应留在 notify_capacity.rs 域文件里）',
    ).not.toContain('const NOTIFICATIONS_CAPACITY_KEY');
    // 反向钉：坏值降级必须留痕（分支必须是 `eprintln!`，消息里点明键名）。
    expect(adapter, '磁盘上的坏容量不再留痕了——启动降级必须是诚实报告，不能静默沿用默认').toContain(
      'Err(error) => eprintln!(',
    );
    expect(adapter, '降级留痕没点明是哪个键坏了').toContain(
      '[tauron] 磁盘上的 {NOTIFICATIONS_CAPACITY_KEY} 非法',
    );

    const ledger = read('contracts/orphan-public-api.json');
    expect(
      (ledger.match(/"symbol": "NotifyStore::trim_to"/g) ?? []).length,
      '孤儿台账仍登记 NotifyStore::trim_to（轮 57 已接线，条目必须删除）',
    ).toBe(0);

    const notify = read('crates/tauron-notify/src/lib.rs');
    expect(notify, 'trim_to 的注释仍宣称无人调用').toContain('已接线（轮 57）');

    for (const name of [
      'fn notify_capacity_setting_shrinks_the_ring_and_reports_it_on_the_wire(',
      'fn notify_capacity_setting_rejects_bad_values_before_they_reach_disk_or_ring(',
      'fn notify_capacity_is_reapplied_from_disk_at_assembly_after_restart(',
      'fn a_bad_capacity_on_disk_never_refuses_startup(',
    ]) {
      expect(adapter, `缺轮 57 的行为测试 ${name}`).toContain(name);
    }

    const adoption = read('docs/integration/incremental-adoption.md');
    expect(adoption, '孤儿行的删除清单没登记轮 57').toContain('`trim_to` 轮 57 已接线');

    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案缺轮 57 小节').toMatch(/^### 轮 57：/m);
    const changelog = read('CHANGELOG.md');
    expect(changelog, 'CHANGELOG 缺轮 57 条目').toMatch(/轮 57/);
  });

  it('轮 58：内层权限词表不留零读者镜像（风险档位 TS 只有一处声明）', () => {
    // 轮 58 立的规矩：`@tauron/types` 的 ACL 面只留**真有读者**的导出；零读者的形状类型/工具
    // 一律删。理由不是「没人用就删」，而是它的注释与 `app-layer-wire.md` §6 在**替它宣传读者**
    // （「审批 UI 文案与 isValidPermission 都用它」）——文案自轮 55 起来自 Rust 单源的线上行。
    const acl = read('packages/types/src/acl.ts');
    const typesIndex = read('packages/types/src/index.ts');
    const grants = read('packages/tauron-host/src/grants.ts');
    const core = read('packages/tauron-core/src/acl.ts');
    const typesTest = read('packages/types/src/acl.test.ts');
    for (const gone of [
      'PermissionMeta',
      'PermissionRisk',
      'hasPermission',
      'hasAllPermissions',
      'isValidPermission',
    ]) {
      expect(acl, `ACL 面又长回零读者的 ${gone}（轮 58 已删，要回来先带读者）`).not.toContain(gone);
      expect(typesIndex, `${gone} 又被转出（轮 58 收口的公共导出面被破）`).not.toContain(gone);
    }
    // 保留面：有读者的三件必须还在，缺任何一件就是「把能力也删了」。
    for (const needle of [
      'export interface PluginPermissionGrant {',
      'export const PERMISSION_GRANULARITY',
      'export function missingPermissions(',
    ]) {
      expect(acl, `轮 58 之后 ACL 面少了真有读者的 ${needle}`).toContain(needle);
    }
    expect(
      core,
      '@tauron/core 不再真的消费 missingPermissions（那才是这些工具留在包里的唯一理由）',
    ).toContain('return missingPermissions(grant, requiredPerms);');
    // 单源：TS 侧风险档位只许有 `@tauron/host` 的一份声明。
    expect(grants, 'grants.ts 的风险档位单源声明被搬走').toContain(
      "export const RISKS = ['low', 'elevated', 'high'] as const;",
    );
    const riskLiterals = [...`${acl}${typesIndex}${grants}`.matchAll(/'elevated'/g)];
    expect(
      riskLiterals.length,
      `TS 侧风险档位字面量有 ${riskLiterals.length} 处（只许 1 处）`,
    ).toBe(1);
    // 注释与文档改口：词表「无人执行」是事实，不许再假装有人读。
    expect(acl, '内层词表的注释又宣称自己被 UI/校验函数读').toContain('在仓库内没有任何执行者');
    const wire = read('docs/architecture/app-layer-wire.md');
    expect(wire, '§6 又把内层词表宣传成有执行者的描述词表').toContain('仓库内**没有它的执行者**');
    expect(wire, '§6 没登记轮 58 删掉的零读者工具').toContain('已删');
    // 删除要由运行时断言证明，不能只靠文本针。
    expect(typesTest, '轮 58 的导出面行为测试被删').toContain("(await import('./index.js'))");
    expect(typesTest, '轮 58 的导出面行为测试不再断言被删工具缺席').toContain(
      '又导出了一份没人用的',
    );
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案缺轮 58 小节').toMatch(/^### 轮 58：/m);
    expect(plan, '轮 56 的遗留句没被轮 58 就地改口').toMatch(/轮 58 已落地/);
    expect(read('CHANGELOG.md'), 'CHANGELOG 缺轮 58 条目').toMatch(/轮 58/);
  });

  it('轮 59：整份文档写路径也按同一判定解释容量键（不再有「按键写才校验」的双标）', () => {
    // 轮 57 只给 `host_settings_set` 加了先判后写，`cmd_settings_adopt_legacy` /
    // `cmd_settings_migrate` 两条整份文档路径仍不校验——同一个键两套标准，坏值经旧文档
    // 就能进磁盘（当时的注释还把它写成坏值的「真实来路」）。轮 59 把三条路径收到同一判定上。
    const adapter = read('crates/tauron-adapter/src/lib.rs');
    const capacity = read('crates/tauron-adapter/src/notify_capacity.rs');

    for (const needle of [
      'pub(crate) fn validate_notify_capacity_document(doc: &serde_json::Value) -> HostResult<()> {',
      'for spelling in [NOTIFICATIONS_CAPACITY_KEY, settings_path(NOTIFICATIONS_CAPACITY_KEY).as_str()]',
      'parse_notify_capacity(value)?;',
      '整份文档写走 [`validate_notify_capacity_document`]',
    ]) {
      expect(capacity, `notify_capacity.rs 缺 ${needle}（整份文档校验被搬空）`).toContain(needle);
    }
    for (const needle of [
      'validate_notify_capacity_document(&doc)?;',
      'let capacity = notify_capacity_from_settings(&state.settings.lock());',
      'if let Err(error) = capacity {',
      '轮 59 起三条命令路径',
      '剩下的真实来路是**用户手改文件**',
    ]) {
      expect(adapter, `lib.rs 缺 ${needle}（轮 59 的接线或改口被掏空）`).toContain(needle);
    }

    // 计数钉：校验器一份实现 + 一处调用；生效点恰三处（set / adopt_legacy / migrate）。
    expect(
      [...`${adapter}${capacity}`.matchAll(/validate_notify_capacity_document\(/g)].length,
      '整份文档校验的定义+调用点数量变了（应 2：定义 + adopt_legacy 落盘前）',
    ).toBe(2);
    expect(
      [...`${adapter}${capacity}`.matchAll(/apply_notify_capacity\(state\)\?;/g)].length,
      '容量的运行期生效点数量变了（应 3：按键写 + 整份接手 + 迁移，别处不得再多一份也不得少一份）',
    ).toBe(3);

    // 反向钉：轮 57 那句「坏值的真实来路是整份接手链」必须已经改口，不许悄悄回退成
    // 宣称旧文档还能把坏容量带上盘（那会让先判后写重新变成注释里的假话）。
    expect(
      adapter,
      'cmd_settings_set 的文档又把整份接手/迁移说成坏值来路（轮 59 起这两条门都先判后写）',
    ).not.toContain(
      '（例如旧文档经 `host_settings_adopt_legacy` + `host_settings_migrate` 带进来的）',
    );
    expect(
      adapter,
      '轮 57 的降级用例又改用 adopt_legacy + migrate 播坏值（那正是轮 59 关掉的门）',
    ).not.toContain('那条链不经过它——旧文档里的裸键被 v1→v2 迁移转义后照样落盘');

    for (const name of [
      'fn adopting_a_legacy_document_with_a_bad_capacity_is_rejected_before_any_mutation(',
      'fn adopting_and_migrating_a_valid_capacity_applies_it_without_a_restart(',
      'fn migrate_refuses_to_persist_a_capacity_it_cannot_interpret(',
    ]) {
      expect(adapter, `缺轮 59 的行为测试 ${name}`).toContain(name);
    }

    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案缺轮 59 小节').toMatch(/^### 轮 59：/m);
    expect(plan, '轮 57 的遗留句没被轮 59 就地改口').toMatch(/降级兜住（\*\*轮 59 已落地\*\*/);
    expect(read('CHANGELOG.md'), 'CHANGELOG 缺轮 59 条目').toMatch(/轮 59/);
  });

  it('轮 60：三套插件生命周期词表的分歧必须逐名钉死（mock 有小写→线名映射，SDK 假注释已改口）', () => {
    // 事实面：宿主线名只有一个权威（Rust `tauron-host::lifecycle::State::as_str()`，
    // TS 镜像 = 本文件已 import 的 `LIFECYCLE_STATES`）。另有两份**已发布**的 TS 词表：
    // ①`@tauron/types` 的 §4.3 设计模型名（10 个，与线名只重合 5 个）；
    // ②`@tauron/app-contract-kit` mock 的 5 个小写名（作者被 README 引导用它测生命周期）。
    // 轮 60 前两者与线名之间零可证一致性：改名、加名、把 mock 的小写名喂给真实宿主都不红。
    const wire: readonly string[] = LIFECYCLE_STATES;
    expect(wire.length, 'LIFECYCLE_STATES 读数异常（构建产物过期？先 pnpm build）').toBe(10);

    // ── ①mock 词表 → 线名：整表逐名核对，两侧都不许悄悄动 ─────────────
    const kit = read('packages/tauron-app-contract-kit/src/mock-registry.ts');
    const unionMatch = /export type PluginState =\s*([^;]+);/.exec(kit);
    expect(unionMatch, '未解析到 mock 的 PluginState union（门禁定位失败）').not.toBeNull();
    const mockStates = [...unionMatch![1]!.matchAll(/'([a-z_]+)'/g)].map((m) => m[1]!);
    expect(mockStates.length, 'mock PluginState 解析到 0 个成员（正则失配）').toBeGreaterThan(0);

    const tableMatch = /MOCK_STATE_TO_WIRE[^=]*=\s*\{([\s\S]*?)\};/.exec(kit);
    expect(
      tableMatch,
      'mock 缺 MOCK_STATE_TO_WIRE（小写名→线名的唯一映射表被搬空）',
    ).not.toBeNull();
    const rows = [...tableMatch![1]!.matchAll(/(\w+):\s*'([A-Z_]+)'/g)].map((m) => ({
      from: m[1]!,
      to: m[2]!,
    }));
    expect(rows.length, 'MOCK_STATE_TO_WIRE 解析到 0 行（表形制变了）').toBeGreaterThan(0);

    // 定义域 = union 全集：漏一个名字（或表里多出一个 union 里没有的名字）都红。
    expect(
      [...rows.map((r) => r.from)].sort(),
      '映射表定义域与 mock PluginState union 不同集',
    ).toEqual([...mockStates].sort());
    // 值域必须是**真实线名**且互不重复：映射到不存在的名字 = mock 测得出的状态线上发不出。
    for (const row of rows) {
      expect(
        wire,
        `mock 的 ${row.from} 映射到 ${row.to}，而它不是 LIFECYCLE_STATES 的成员`,
      ).toContain(row.to);
    }
    expect(new Set(rows.map((r) => r.to)).size, '两枚 mock 状态映射到同一线名（词表被压平）').toBe(
      rows.length,
    );

    // ── ②SDK 设计模型名 ↔ 线名：分歧集合本身被钉死，收敛/改名必须走批准 ──
    const modelStates = Object.keys(TRANSITIONS);
    expect(modelStates.length, '@tauron/types 的 TRANSITIONS 表条目数异常（构建产物过期？）').toBe(
      10,
    );
    const shared = modelStates.filter((s) => wire.includes(s));
    const modelOnly = modelStates.filter((s) => !wire.includes(s));
    const wireOnly = wire.filter((s) => !modelStates.includes(s));
    expect(shared.sort(), '设计模型与线名的重合集变了（任一侧改名都会到这里来）').toEqual([
      'DISABLED',
      'DISCOVERED',
      'ENABLED',
      'INSTALLED',
      'INSTALLING',
    ]);
    expect(
      modelOnly.sort(),
      'SDK 词表冒出新的非线名状态（本表已发布，不得再加不可表达的名）',
    ).toEqual(['ENABLING', 'ERRORED', 'DISABLING', 'UNINSTALLING', 'UPGRADING'].sort());
    expect(
      wireOnly.sort(),
      '线名冒出 SDK 词表表达不了的状态（宿主加了态而 @tauron/types 没跟上，须单独批准收敛）',
    ).toEqual([
      'ERRORED_RETRYABLE',
      'ERRORED_USER_CONFIRM',
      'INSTALL_FAILED',
      'RUNNING',
      'UNINSTALLED',
    ]);

    // ── ③假注释不得复活：@tauron/types 曾自称「表驱动 + 单点收口」 ───────
    const typesPlugin = read('packages/types/src/plugin.ts');
    expect(
      typesPlugin,
      'plugin.ts 又把设计模型表说成「单点收口」（真收口点是 Rust lifecycle::TRANSITIONS）',
    ).not.toContain('DSH 模式：表驱动 + 单点收口');
    expect(typesPlugin, 'plugin.ts 没交代设计模型名与线名的关系').toContain('这不是宿主线名');
    for (const needle of [
      'export const MOCK_STATE_TO_WIRE: Readonly<Record<PluginState, MockWireState>> = {',
      'getWireState(pluginId: string): MockWireState | null',
      '是本 mock 的小写私有词表，不是宿主线名',
    ]) {
      expect(kit, `mock-registry.ts 缺 ${needle}（轮 60 的桥被掏空）`).toContain(needle);
    }
    expect(
      read('packages/tauron-app-contract-kit/src/index.ts'),
      'MOCK_STATE_TO_WIRE 没对外导出（作者拿不到唯一的映射表）',
    ).toContain('export { MOCK_STATE_TO_WIRE }');
    for (const name of [
      'getWireState 随生命周期操作返回线名',
      'getWireState 对未知与已卸载插件返回 null',
    ]) {
      expect(
        read('packages/tauron-app-contract-kit/src/mock-registry.test.ts'),
        `缺轮 60 的行为测试 ${name}`,
      ).toContain(name);
    }

    const guide = read('docs/api/plugin-development-guide.md');
    expect(guide, '接口文档没交代 mock 小写名 / SDK 设计模型名 / 线名三者关系').toMatch(
      /词表：哪份状态名在线上/,
    );
    expect(
      read('docs/architecture/app-layer-wire.md'),
      '线格式文档没登记「不在线上」的那两套 TS 词表（读者只看得见镜像表就会以为小写名也是线名）',
    ).toMatch(/另外两套\*\*不在线上\*\*的 TS 词表（轮 60 登记）/);
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案缺轮 60 小节').toMatch(/^### 轮 60：/m);
    expect(plan, '轮 59 的遗留②（SDK 面未审）没被轮 60 就地改口（做了一半也要标出来）').toMatch(
      /导出未审（轮 58 遗留②）（\*\*轮 60 已落地\*\*/,
    );
    expect(read('CHANGELOG.md'), 'CHANGELOG 缺轮 60 条目').toMatch(/轮 60/);
  });

  it('V7 §7：风险表的复核结论必须与代码同形（宣称链不得悄悄升级）', () => {
    // 轮 28 立的规矩：审计风险表一旦被逐行复核，"结论"就成了对外宣称。这条门禁挡两种漂移：
    // ①代码没动、文档把「部分／仍成立」改成「已闭合」（假宣称）；②文档没动、代码悄悄多了
    // 引用点（假孤儿）。两侧任一变动都必须同时改另一侧。
    const v7 = read('docs/Tauron-Deep-Audit-and-Optimization-Plan-V7.md');
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(v7, 'V7 §7 缺轮 28 的逐行复核表').toMatch(/轮 28 复核结论/);
    expect(plan, '缺口方案未登记轮 28 的逐行复核').toMatch(/轮 28 逐行复核/);

    // 只在复核表里找行：§7 上面还留着"审计当时的事实"原表，同名行的措辞是**历史句**，
    // 拿它当现状判据会让这条门禁查错对象（第一次跑就是这么红的）。
    const review = v7.slice(v7.indexOf('轮 28 复核结论'));
    expect(review.length, '复核表之后没有内容（定位失败）').toBeGreaterThan(200);

    // Upgrade / ServiceGraph 两行是本轮判为"未闭合"的：措辞是结论的一部分。
    const row = (name: string): string => {
      const m = new RegExp(`^\\| ${name} \\|.*$`, 'm').exec(review);
      expect(m, `V7 §7 复核表找不到 ${name} 行（行名改了就要重新复核）`).not.toBeNull();
      return m![0]!;
    };
    const upgrade = row('Upgrade');
    expect(upgrade, 'Upgrade 行未写明"部分"（清理腿已闭合≠整行闭合）').toContain('**部分**');
    expect(upgrade, 'Upgrade 行删掉了真执行器的符号证据').toContain('UpgradeRunner::run_phases');
    // 轮 40：装配腿落地到适配层 seam——行文必须点明消费者与仍成立的边界
    // （网络下载/证书验签仍由装配方注入、check 仍是桩），不能改写成"已闭合"。
    expect(upgrade, 'Upgrade 行未写明轮 40 的仓内消费者').toMatch(/DistributeUpgradeInstaller/);
    expect(upgrade, 'Upgrade 行丢了"网络/验签仍由装配方注入"的边界').toMatch(/装配方注入/);
    expect(row('ServiceGraph'), 'ServiceGraph 行不得伪装成已闭合').toContain('**仍成立**');

    // 代码侧核对：`UpgradeRunner` 的仓内消费者**恰好一处**——适配层的装配腿 seam。
    // （轮 28 复核时这里断言的是"零引用点"；轮 40 结论变了，门禁换断言而不是删门禁。）
    const refs = ((): string[] => {
      const hits: string[] = [];
      const walk = (dir: string): void => {
        for (const e of readdirSync(dir, { withFileTypes: true })) {
          const p = join(dir, e.name);
          if (e.isDirectory()) walk(p);
          else if (e.name.endsWith('.rs')) {
            const src = readFileSync(p, 'utf8');
            // 注释里"引用"它不算消费者（这里要的是装配点）。
            const code = src.replace(/^\s*\/\/.*$/gm, '').replace(/^\s*\/\/\/.*$/gm, '');
            if (
              /\bUpgradeRunner\b|create_default_upgrade_runner|create_upgrade_runner/.test(code)
            ) {
              hits.push(p.slice(workspaceRoot.length + 1).replace(/\\/g, '/'));
            }
          }
        }
      };
      for (const crate of readdirSync(join(workspaceRoot, 'crates'), { withFileTypes: true })) {
        if (!crate.isDirectory() || crate.name === 'tauron-distribute') continue;
        const src = join(workspaceRoot, 'crates', crate.name, 'src');
        if (existsSync(src)) walk(src);
      }
      return hits;
    })();
    expect(refs, `UpgradeRunner 的仓内消费者集合变了：V7 §7 的结论必须同步改写`).toEqual([
      'crates/tauron-adapter/src/lib.rs',
    ]);
    // 消费者必须**恰好是一条装配腿**：runner 构造点只有一处（`create_upgrade_runner`），
    // 且注入面是 `UpgradeInstaller` seam。命令层绕过 seam 直连 runner（第二个构造点）
    // 或增删消费者都会在这里红。
    const adapterSrc = read('crates/tauron-adapter/src/lib.rs');
    const runnerBuilds = [...adapterSrc.matchAll(/create_upgrade_runner\(/g)];
    expect(runnerBuilds.length, '装配腿的 runner 构造点必须恰好一处').toBe(1);
    expect(adapterSrc, '装配腿 seam 丢失（UpgradeInstaller 是唯一注入面）').toMatch(
      /pub trait UpgradeInstaller/,
    );
  });

  it('轮 29：更新可用性只有一条权威通道（SDK 不得再拿宿主桩当结论）', () => {
    const sdk = read('packages/tauron-host/src/auto-update-client.ts');
    const adapter = read('crates/tauron-adapter/src/lib.rs');

    // ① 检查腿：真通道。`host_updater_check` 是被调的一方，`host_market_check` 不得再出现。
    expect(sdk, 'SDK 的检查没有走真通道 host_updater_check').toContain("'host_updater_check'");
    expect(sdk, 'SDK 又回到宿主桩 host_market_check 取可用性结论').not.toContain(
      "'host_market_check'",
    );
    // 版本是入参而不是猜测：宿主不给 currentVersion 就不检查。
    expect(sdk, 'currentVersion 不再作为入参下发').toMatch(
      /'host_updater_check',\s*\{ currentVersion \}/,
    );
    // 缺参/无通道两种"答不了"都必须与"没有更新"分家。
    expect(sdk, '缺 currentVersion 时必须抛错而不是返回 available:false').toMatch(
      /if \(currentVersion === ''\) \{[\s\S]*?throw new Error/,
    );
    expect(sdk, 'Unsupported 不再落 degraded').toMatch(
      /isUnsupportedBody\(result\)[\s\S]*?degraded: true/,
    );
    expect(sdk, '降级路径不再被当成 idle（"查过了，没更新"）').toMatch(
      /degraded === true \? 'error' : 'idle'/,
    );

    // ② 下载/安装腿：轮 40 起装配腿注入后为真；**缺省装配仍是有意的模拟桩**，
    // 且分派必须问 `native_supported()`——装好的腿不得被悄悄钉回桩，桩不得被
    // 悄悄当成真。SDK 侧守卫不变：见 `simulated` 一律抛错。
    for (const cmd of ['host_market_download', 'host_market_install']) {
      expect(sdk, `SDK 不再调 ${cmd}（下载/安装腿被悄悄摘掉）`).toContain(`'${cmd}'`);
    }
    expect(sdk, 'simulated 守卫丢失（桩结果可能被推进为已下载）').toMatch(
      /result\?\.simulated[\s\S]{0,200}?throw new Error/,
    );
    expect(adapter, 'market 模拟腿不再如实标 simulated').toMatch(
      /fn cmd_market_download_simulated\([\s\S]*?simulated: true/,
    );
    expect(adapter, 'market 分派没有问装配腿是否注入（装好的腿会被钉回桩）').toMatch(
      /fn cmd_market_download\([\s\S]{0,400}?native_supported\(\)[\s\S]{0,200}?cmd_market_download_wired/,
    );

    // ③ 桩必须把"真通道在哪"说清楚：两条命令对同一问题给相反答案，就是轮 28 的分裂。
    expect(adapter, 'market 桩的 reason 不再指向真通道 host_updater_check').toMatch(
      /fn cmd_market_check\([\s\S]*?host_updater_check/,
    );
    expect(adapter, 'capabilities 的 market-update 域不再说明真通道在哪').toMatch(
      /"market-update",\s*\n?\s*"缺省装配无更新通道[\s\S]*?updater 域/,
    );
  });

  it('轮 31：更新对话框的检查腿与 SDK 同源（壳层不读宿主桩，也不自造判定）', () => {
    const shell = read('packages/tauron-host/src/shell-controller.ts');
    const sdk = read('packages/tauron-host/src/auto-update-client.ts');
    const example = read('examples/minimal-app/src/main.ts');
    const shellTest = read('packages/tauron-host/src/shell-controller.test.ts');

    // ① 检查腿打真通道；桩命令名在控制器里**一次都不该出现**。
    expect(shell, '壳层检查腿没走真通道 host_updater_check').toContain('this.client.updaterCheck(');
    expect(shell, '壳层又回到宿主桩 marketCheck 取可用性结论').not.toContain('marketCheck');
    // ② 判定同源：同一个用户动作的两条入口必须共用一份"怎么算答不了"。
    expect(sdk, 'toUpdateInfo 不再是导出的共享判据').toMatch(/^export function toUpdateInfo/m);
    expect(shell, '壳层自己造判定（双镜像点：两处会各自漂移，轮 29 修的就是这种分叉）').toMatch(
      // 不锁单行写法：轮 33 之后这条导入是 prettier 折的多行块，锁字面量等于给格式化下赌注。
      /import \{[^}]*\btoUpdateInfo\b[^}]*\} from '\.\/auto-update-client\.js'/,
    );
    // ③ 缺必填入参时必须**先返回**，不发任何命令——回落桩会拿到形似结论的 available:false。
    expect(shell, '缺 currentVersion 的处理没有先返回').toMatch(
      /if \(currentVersion === ''\) \{[\s\S]*?return;\n {4}\}/,
    );
    // ④ 示例是这条链的真生产消费者：版本有事实源、展示把"答不了"和"没有更新"分家。
    expect(example, '示例没声明当前版本（检查腿只会恒报缺参）').toMatch(
      /currentVersion: APP_VERSION/,
    );
    expect(example, '示例的版本号不是从 package.json 取的（写死字符串会随版本漂移）').toMatch(
      /import \{ version as APP_VERSION \} from '\.\.\/package\.json'/,
    );
    expect(example, '示例把 degraded 与"没有更新"合并成同一句话').toMatch(
      /if \(info\.degraded === true\) \{[\s\S]*?return info\.available/,
    );
    // ④′ 对话框必须**真的挂在页面上**：控制器那条腿若只有单测里没有用户，就是"接口有、
    //     行为无"——`oc-updater-check` 在这个仓库里正因为零派发被记过一次孤儿账。
    const page = read('examples/minimal-app/index.html');
    expect(page, '示例页面不再挂载 <oc-updater-dialog>（壳层检查腿没有真消费者）').toMatch(
      /<oc-updater-dialog id="updater-dialog"><\/oc-updater-dialog>/,
    );
    expect(page, '页面上没有打开对话框的入口（挂了元素但没人能点开）').toMatch(
      /id="btn-updater-dialog"/,
    );
    expect(example, '检查结果不再写回对话框（结论只进了日志）').toMatch(
      /updaterDialog\.message = describeUpdate\(info\)/,
    );
    // ⑤ 测试侧后门封掉：控制器能力表里不该再声明宿主桩命令（用例正文引用它是允许的，
    // 能力表声明它才是要防的事——mock 会照样把桩答案返回给回归后的检查腿）。
    const capsBlock = shellTest.match(/const CONTROLLER_CAPS = \[[\s\S]*?\];/);
    expect(capsBlock, '测试里找不到控制器能力表').not.toBeNull();
    expect(
      capsBlock![0],
      '测试能力表重新声明 host_market_check——那样即使检查腿退回桩，mock 也会照样答绿',
    ).not.toContain("'host_market_check'");
  });

  it('轮 33：更新通道诊断只有一个出口，账本的模拟推进不得被读成真实状态', () => {
    const adapter = read('crates/tauron-adapter/src/lib.rs');
    const client = read('packages/tauron-host/src/shell-client.ts');
    const shell = read('packages/tauron-host/src/shell-controller.ts');
    const shellTest = read('packages/tauron-host/src/shell-controller.test.ts');

    // 断链形态：`host_updater_status` 是灰度百分比 / 崩溃门禁 / 进程内账本的**唯一**出口，
    // 而它在 TS 侧零消费者——"你不在灰度批次""被崩溃门禁停发""宿主没装端点"在 UI 上
    // 全是同一句"没有更新"。另一半：账本 `state` 的两个写入方今天都是宿主桩，
    // `installed:2.0.0` 这个字符串自己不带出处，只看它就等于把"点了一下模拟安装"
    // 显示成"已安装 2.0.0"。

    // ① 线形：Rust 字段集与 TS 接口逐字段一致，且必须 camelCase（漏一半就是前端读 `undefined`）。
    const rs = rustStruct('crates/tauron-adapter/src/lib.rs', 'UpdaterStatus');
    expect(rs.camelCase, 'UpdaterStatus 必须 rename_all = "camelCase"').toBe(true);
    expect(
      rs.fields.has('state_simulated'),
      '线字段 state_simulated 不见了（出处信息无从传递）',
    ).toBe(true);
    const tsBody = /export interface UpdaterStatus \{([\s\S]*?)\n\}/.exec(client);
    expect(tsBody, 'TS 侧找不到 UpdaterStatus 接口').not.toBeNull();
    const tsFields = new Set(
      [...tsBody![1]!.matchAll(/^\s{2}(\w+)\??:/gm)]
        .map((m) => m[1]!)
        .filter((k) => k !== 'readonly'),
    );
    expect(
      [...rs.fields].map(snakeToCamel).sort(),
      'Rust ↔ TS 的 UpdaterStatus 字段集漂移（两侧各写一份就会漂，轮 32 的三面镜像同理）',
    ).toEqual([...tsFields].sort());

    // ② 读侧成对，且**成对到签名上**：账本与其出处只能由命令层从 `shell_ext` 一次性
    //    交给 sink，sink 依据"有没有账本"推导出处。这里不加一条 `state_simulated: false`
    //    字面量的容错——`check-simulated-never-commits.mjs` 的 cross-check 就是禁止被网关源
    //    出现 `simulated: false` 断言（无中生有地说"这条状态是真的"）。本轮的修法是让这种
    //    断言**写不出来**（sink 必须收到账本对），不是把门禁的needle放宽。
    expect(
      adapter,
      'cmd_updater_status 不再把账本与出处成对交给 sink（分开传就能只传 state）',
    ).toMatch(/status\(\s*ext\.update_state\.clone\(\),\s*ext\.update_state_simulated\s*\)/);
    expect(
      adapter,
      'UpdaterSink::status 的账本对参数消失了：sink 又能只报 state、或自己断言出处',
    ).toMatch(/fn status\(&self, ledger_state: Option<String>, ledger_simulated: bool\)/);
    expect(
      adapter,
      'state_simulated 必须由"账本有无"推导（`state: None` 时不得报"这是模拟来的"，也不得报"这是真的"）',
    ).toMatch(/state_simulated = ledger_state\.is_some\(\) && ledger_simulated;/);
    expect(
      adapter,
      '出现硬写的 `state_simulated: false`：无账本时的"真实"断言又回来了',
    ).not.toMatch(/state_simulated: false/);

    // ③ 写侧逐条登记：每一个推进账本的写入方都必须同时写下它的出处标注。
    //    轮 40 起标注有两种：模拟腿 `= true`（本次没有真实效果），装配腿 `= false`
    //    （真实效果已发生——`check-simulated-never-commits.mjs` 的 A2b 段把这两条
    //    `= false` 钉在 wired 双胞胎内、且必须排在 seam 调用之后，这里只核**每一处
    //    写入都紧跟一条标注**：4 处写入 == 2×true + 2×false）。
    for (const phase of ['downloaded', 'installed'] as const) {
      for (const marker of ['true', 'false'] as const) {
        expect(
          adapter,
          `${phase} 账本写入方没有紧跟 \`update_state_simulated = ${marker};\`——新增写入方漏标这条必红`,
        ).toMatch(
          new RegExp(
            `ext\\.update_state = Some\\(format!\\("${phase}:[\\s\\S]{0,80}?ext\\.update_state_simulated = ${marker};`,
          ),
        );
      }
    }
    expect(
      [...adapter.matchAll(/ext\.update_state = Some\(/g)].length,
      '账本写入方数量变了：门禁要求每条写入都带出处登记（true / false 各算一种），请同步本轮',
    ).toBe([...adapter.matchAll(/ext\.update_state_simulated = (?:true|false);/g)].length);

    // ④ 诊断有人消费，且**两条入口同一口径**：SDK 的 `checkUpdate()` 与对话框的检查按钮
    //    必须共用一份 `enrichUpdaterInfoWithChannel`。轮 29 修的是判定分叉、轮 31 修的是
    // 检查腿分叉，展示口径若再各写一份，同一个用户动作就会给出两种"为什么没更新"。
    const sdk = read('packages/tauron-host/src/auto-update-client.ts');
    expect(
      sdk,
      'host_updater_status 回到零消费者：灰度 / 崩溃门禁 / 宿主账本再次是 UI 上的隐形事实',
    ).toMatch(/enrichUpdaterInfoWithChannel\(/);
    expect(sdk, 'SDK 补诊断时读的不是 host_updater_status').toMatch(
      /invoke<UpdaterChannelStatus>\('host_updater_status'\)/,
    );
    expect(shell, '对话框检查腿没走同一份诊断口径（两条入口各拼一句 = 第三面镜像）').toMatch(
      /enrichUpdaterInfoWithChannel\(info, \(\) => this\.client\.updaterStatus\(\)\)/,
    );
    expect(shell, '控制器自己又拼了一份诊断文案').not.toMatch(/grayscalePercent/);
    expect(
      [...sdk.matchAll(/function describeUpdaterChannel/g)].length,
      '诊断文案必须只有一份定义',
    ).toBe(1);
    expect(sdk, '成对读 state / stateSimulated 的那句被拆开了（模拟推进又能冒充真实）').toMatch(
      /status\.state[\s\S]{0,220}status\.stateSimulated/,
    );
    // 失败边界：命令缺席 / 报错 / 答空都不得改结论。
    expect(sdk, '诊断失败会把「检查更新」整条腿带崩').toMatch(
      /await readChannel\(\)\.catch\(\(\) => null\)/,
    );
    expect(sdk, '空结果被当成有效诊断（undefined 会渲染成"通道已装配、灰度 NaN%"）').toMatch(
      /if \(!channel\) return info;/,
    );
    expect(sdk, '有更新时也去问通道状态（一次点击为附加读数多付一条 IPC）').toMatch(
      /if \(info\.degraded !== true && info\.available !== false\) return info;/,
    );
    // ④′ 补进来的话必须真上屏：示例把 reason 吞掉，这轮就只是"接口有、行为无"。
    const example = read('examples/minimal-app/src/main.ts');
    expect(
      example,
      '示例在"没有更新"分支把宿主给的依据吞了（那句"已是最新版本"没有依据支撑就说出口）',
    ).toMatch(/if \(!info\.available && info\.reason != null\) \{[\s\S]{0,200}依据/);

    // ⑤ 测试侧不留后门：诊断命令必须在诊断后端的能力表里——只写 `[...CONTROLLER_CAPS]`
    //    的话它永远走 command-not-found 分支，上面那几条"消费者"断言就是空转。
    //    反向也要钉住：默认后端**不**声明它，缺席路径（宿主没这条命令）才有用例覆盖。
    const caps = /const CONTROLLER_CAPS = \[[\s\S]*?\];/.exec(shellTest);
    expect(caps, '测试里找不到控制器能力表').not.toBeNull();
    expect(
      caps![0],
      '默认能力表声明了 host_updater_status——诊断命令缺席这条真实路径就没有用例了',
    ).not.toContain("'host_updater_status'");
    const diagCaps = /const diagnosisBackend[\s\S]*?capabilities: \[[\s\S]*?\]/.exec(shellTest);
    expect(diagCaps, '测试里找不到诊断腿的后端工厂').not.toBeNull();
    expect(
      diagCaps![0],
      '诊断后端没声明 host_updater_status：轮 33 那条腿在测试里根本跑不到',
    ).toContain("'host_updater_status'");
    expect(
      diagCaps![0],
      '诊断后端没声明 host_updater_check：检查腿与诊断腿必须同处装配才测得出先后顺序',
    ).toContain("'host_updater_check'");
    expect(shellTest, '没有用例把"模拟推进"的读数钉死').toContain('模拟推进，未真的装上');

    // ⑥ 文档同步：这一轮的口径必须成文，否则下一个人又会只读 `state`。
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    expect(plan, '缺口方案未成文轮 33').toMatch(/轮 33/);
    const api = read('docs/api/command-surface.md');
    expect(api, '命令面文档未同步 stateSimulated：接口文档与线形相反就是宣称漂移').toMatch(
      /stateSimulated|state_simulated/,
    );
  });

  it('轮 34：施工单不得再把已删除、且被门禁反向钉住的形状写成待办', () => {
    const plan = read('docs/architecture/v4-industrial-gap-closure-plan.md');
    const roadmap = read('docs/architecture/multi-plugin-substrate-roadmap.md');
    const ledger = read('contracts/orphan-public-api.json');

    // 断链形态：方案文档是**给下一个人看的施工单**。它指路的形状若已被删掉并被门禁反向钉住，
    // 照着做的人一定撞红——这不是"文档旧了"，是文档与代码互相矛盾，本仓口径里那本身就是一条断链。
    // A75 是本轮抓到的一条：轮 11 删了拓扑序的两个字段并立了「服务拓扑图不得再抄两份无人执行的
    // 序」，Batch 2' 那条却仍叫人来一份新方案，且没说明为什么不能照字面做。

    // ① A75：旧处方不得留在待办里，且必须写明"已删除 + 反向门禁 + 诚实前置"。
    expect(plan, 'A75 又写成"装配走那两个字段"：那是轮 11 删除并被门禁钉死的形状').not.toMatch(
      /让拓扑序\*\*真的执行\*\*——装配走/,
    );
    expect(plan, 'A75 未说明两个字段已在轮 11 删除').toMatch(/这两个字段已在轮 11 删除/);
    expect(plan, 'A75 未点名反向门禁：下一个人不会知道它会撞红').toMatch(
      /服务拓扑图不得再抄两份无人执行的序/,
    );
    // 复核命令必须自带期望值——零命中的命令不写"期望零命中"，跑的人读不出成败。
    expect(plan, 'A75 的 rg 复核命令没写期望零命中（命不命中都判不出结论）').toMatch(
      /期望零命中[\s\S]{0,160}rg -n 'service_startup_order/,
    );

    // ② A67/A68：新增根命令与 85 条冻结门禁冲突，必须先给"加字段"这条路，再谈显式解冻。
    const a67 = /^- A67\/A68：([\s\S]*?)(?=\r?\n- )/m.exec(plan);
    expect(a67, '方案里找不到 A67/A68 那条待办').not.toBeNull();
    expect(a67![1]!, 'A67/A68 仍把新增 host_* 命令当成无门槛动作').toContain('host_protocol_info');
    expect(a67![1]!, 'A67/A68 未点名命令面冻结门禁').toMatch(/§46\.3 冻结[\s\S]{0,80}85/);
    expect(a67![1]!, 'A67/A68 没给出"并入既有命令返回字段"这条不破冻结的路').toMatch(
      /返回字段[\s\S]{0,120}轮 33/,
    );

    // ③ 台账与文档必须同批移动：新写进文档的"零消费者"宣称要能在孤儿账里找到条目，
    //    否则那句话只是另一条无人复核的文字。
    expect(ledger, 'PluginRegistry 的零消费者宣称未进台账').toMatch(/"symbol": "PluginRegistry"/);
    expect(ledger, 'ConfigManager 的零消费者宣称未进台账').toMatch(/"symbol": "ConfigManager"/);

    // ④ 历史方案的过期行必须**改判并注明依据**，不能留成"待办的活线"。
    expect(roadmap, 'M-9 仍把两份注册表当成活的冲突：矛盾载体 bootstrap.ts 已删除').toMatch(
      /M-9 \| ~~[\s\S]{0,80}已消除（轮 34 复核）/,
    );
    expect(
      roadmap,
      'roadmap M-9 未点名 bootstrap.ts 已随 W1-b 删除（读者会把死线当活线去"解 P1-1"）',
    ).toMatch(/bootstrap\.ts[\s\S]{0,140}已随 W1-b 删除/);

    // ⑤ 现状表与待办必须同向：Batch 2 的 A75 行此前把轮 11 删掉的两个字段**当成今天的缺口**
    //    描述（待办那条教人去做，现状表教人相信它还活着）——同一矛盾的两侧，只钉一侧下一轮
    //    就会只改一侧。
    expect(plan, 'A75 现状行又写回"两个序只存字段"：那个载体轮 11 已删').not.toMatch(
      /两个序只存字段（/,
    );
    expect(plan, 'A75 现状行未标轮 11 删除，读者会以为字段仍在').toMatch(
      /A75 ServiceGraph[\s\S]{0,80}轮 11 删[\s\S]{0,40}轮 34 复核/,
    );
    expect(
      plan,
      'A75 现状行没写真实缺口（服务级回收动作），只剩一句无对象的"拓扑序不执行"',
    ).toMatch(/A75 ServiceGraph[\s\S]{0,700}服务级回收动作/);
  });

  it('轮 35：iframe 桥两侧都必须把错误码带过去，不得塌成通用码', () => {
    // 断链形态：轮 30 把投递写侧的失败按事实分成四个码，代价只在**调用方能按码分支**
    // 时才回本。跨 iframe 桥是这条链的最后一跳，此前两半各丢一次——写侧把 handler
    // 抛出的码硬换成 `SC-9001`，读侧声明了 `error.code` 却只取 `message` 造裸 Error。
    const bridge = read('packages/tauron-plugin-sdk/src/bridge.ts');
    const ctx = read('packages/tauron-plugin-sdk/src/plugin-context.ts');
    const sdkErrors = read('packages/tauron-plugin-sdk/src/errors.ts');
    const typesErrors = read('packages/types/src/errors.ts');
    const hostErrors = read('packages/tauron-host/src/errors.ts');
    const guide = read('docs/api/plugin-development-guide.md');
    const wire = read('docs/architecture/app-layer-wire.md');
    const sdkPkg = JSON.parse(read('packages/tauron-plugin-sdk/package.json')) as {
      dependencies?: Record<string, string>;
      devDependencies?: Record<string, string>;
    };

    // ① 写半：先取错误自带码，再退消息里的码形态，两处都没有才落通用码。
    expect(bridge, '桥的失败分支又写回通用码字面量').not.toMatch(/code:\s*'SC-9001'/);
    expect(bridge, '桥不再从抛出的值取码（handler 的码会被抹掉）').toMatch(
      /const preserved = codeFromThrown\(err\)/,
    );
    expect(bridge, '取不到码时必须有显式兜底，而不是 undefined').toMatch(
      /preserved \?\? PluginErrorCode\.INTERNAL/,
    );
    expect(bridge, 'retryable 又变成硬编码常量（与已发布码表冲突）').toMatch(
      /retryable: isAppRetryable\(code\)/,
    );

    // ② 读半：桥送回来的码必须交到插件作者手上，不能只留一句消息。
    expect(ctx, '插件侧又用裸 Error 丢掉 code（声明了字段却没人读）').not.toMatch(
      /pending\.reject\(\s*new Error\(result\.error\?\.message/,
    );
    expect(ctx, '插件侧不再校验码形态就接受任意 code 字段').toMatch(/isCodeLike\(raw\)/);
    expect(ctx, '拒绝值不带 PluginBridgeError（调用方拿不到 code/fallbackApplied）').toMatch(
      /new PluginBridgeError\(/,
    );

    // ③ 形态判据只有一份：定义在 `@tauron/types`，宿主侧只做再导出。
    //    第二份 `/^SC-\d{4}$/` 会随时间漂移（一侧放宽一侧没放宽），那才是真断链。
    expect(typesErrors, 'SC-#### 形态定义不在 @tauron/types').toMatch(
      /export const APP_LAYER_ERROR_CODE_PATTERN = \/\^SC-\\d\{4\}\$\/;/,
    );
    expect(hostErrors, '宿主侧又自己定义了一份形态（两份判据必然漂移）').not.toMatch(
      /const APP_LAYER_ERROR_CODE_PATTERN\s*=/,
    );
    expect(hostErrors, '宿主侧没有再导出 @tauron/types 的那份判据').toMatch(
      /export \{ APP_LAYER_ERROR_CODE_PATTERN, isAppLayerErrorCode, isCodeLike \};/,
    );
    expect(sdkErrors, 'SDK 侧出现第二份 SC 形态正则').not.toMatch(/SC-\\d\{4\}/);
    expect(sdkErrors, 'SDK 没有复用形态判据').toMatch(/isCodeLike/);

    // ④ 为什么判据放 types 而不是 host：依赖方向没变，这条必须一直为真，
    //    否则"插件包依赖宿主客户端"会把整层宿主实现打进 iframe 产物。
    for (const field of ['dependencies', 'devDependencies'] as const) {
      expect(
        sdkPkg[field]?.['@tauron/host'],
        `插件 SDK 的 ${field} 出现 @tauron/host：整座宿主客户端会进插件包`,
      ).toBeUndefined();
    }
    for (const f of ['bridge.ts', 'plugin-context.ts', 'errors.ts', 'contract-context.ts']) {
      expect(
        read(`packages/tauron-plugin-sdk/src/${f}`),
        `插件 SDK 的 ${f} 直接 import 了 @tauron/host`,
      ).not.toMatch(/from '@tauron\/host'/);
    }

    // ⑤ 重试语义仍各归各表：SDK 只判框架层码（查 `RETRYABLE_ERROR_CODES`），
    //    宿主 `E_*` 一律不猜——把那份表复制过来就是第三个事实源。
    expect(sdkErrors, '框架层重试判断没有走词表').toMatch(/isRetryable\(code/);
    expect(sdkErrors, '宿主 E_* 的重试语义被 SDK 复制了一份（应交给调用方按码决定）').not.toMatch(
      /E_[A-Z_]+':\s*true|startsWith\('E_'\)\s*\?\s*true/,
    );

    // ⑥ 层归属用词必须与文档口径同向（`SC-####` 属框架层、`E_*` 属应用层宿主底座）：
    //    同一个词在两层上反着用，下一个人一定会把码表接错。
    for (const [name, src] of [
      ['types/errors.ts', typesErrors],
      ['tauron-host/errors.ts', hostErrors],
      ['tauron-plugin-sdk/errors.ts', sdkErrors],
      ['app-layer-wire.md', wire],
      ['plugin-development-guide.md', guide],
    ] as const) {
      expect(src, `${name} 把 SC-#### 称作应用层（与 overview.md 口径相反）`).not.toMatch(
        /应用层(错误码|词表|形态|码| `?SC-)/,
      );
      expect(src, `${name} 把宿主 E_* 称作框架层（E_* 属应用层宿主底座）`).not.toMatch(
        /框架层? [`']*E_\*/,
      );
    }

    // ⑦ 两半各有一条真实测试：只测写侧等于没测（轮 30 的教训就是"送出桥没人接"）。
    expect(read('packages/tauron-plugin-sdk/src/bridge.test.ts')).toMatch(
      /轮 35：handler 抛出的宿主码原样过桥/,
    );
    expect(read('packages/tauron-plugin-sdk/src/plugin-context.test.ts')).toMatch(
      /宿主 24 码原样保留在 reject 上/,
    );

    // ⑧ 面向插件作者的使用文档必须写明这份形状（码原样、兜底标记、重试判定边界）。
    expect(guide, '插件开发指南没有记录桥的保码行为（轮 35）').toMatch(
      /PluginBridgeError[\s\S]{0,700}fallbackApplied/,
    );
    expect(wire, '线协议文档未记录 bridge.ts 的保码收口').toMatch(/bridge\.ts[\s\S]{0,200}轮 35/);
  });

  it('轮 30：投递写侧按事实分码，不得塌成"非法状态迁移"', () => {
    const leg = read('crates/tauron-adapter/src/process_delivery.rs');

    // 写失败必须交回 `stdin_write_failure` 分流，而不是在 `deliver()` 里就地造一个码——
    // 就地 map_err 正是上一版把四种事实压成一种的写法。
    expect(leg, 'deliver() 的写失败又被就地塌成一个码').toMatch(
      /Err\(e\) => Err\(stdin_write_failure\(pid, &e\)\)/,
    );
    expect(leg, '写队列满不再冒充状态错误').toMatch(
      /ErrorKind::WouldBlock => \(\s*ErrorCode::E_CALL_PENDING_FULL/,
    );
    // 通路没了（pid 不在了 / 管道断了）= 租约没了：`E_LEASE_EXPIRED` 的
    // `AfterReconnect` 是这条事实唯一的机器可读处置，丢掉它调用方只会瞎重试或放弃。
    expect(leg, 'stdin 通路消失不再报告为状态迁移问题').toMatch(
      /ErrorKind::NotFound \| std::io::ErrorKind::BrokenPipe => \(\s*ErrorCode::E_LEASE_EXPIRED/,
    );
    expect(leg, '帧超协议上界不再冒充状态迁移').toMatch(/_ => \(\s*ErrorCode::E_INVALID_MANIFEST/);
    // 「这个启动器不能写 sidecar stdin」不是失败，是没有通路：必须落 delivered:false，
    // 让上层转成 `Unsupported`，与"没有 runtime 不投递"同一个形状。
    expect(leg, '没有写通路又变成错误码（调用方看不见"这条链路压根没接"）').toMatch(
      /ErrorKind::Unsupported => Ok\(DeliveryReceipt \{\s*delivered: false/,
    );

    // deliver() 这一段里不得再出现 `E_STATE_INVALID_TRANSITION`：走到这里状态机**已经
    // 放行**（调用已进 pending 表），报"非法状态迁移"是对系统的假陈述。
    const from = leg.indexOf('fn deliver(');
    expect(from, '找不到 deliver()（门禁定位失败）').toBeGreaterThanOrEqual(0);
    const body = leg.slice(from, leg.indexOf('fn settle(', from));
    expect(body.length, 'deliver() 与 settle() 之间没有内容（定位失败）').toBeGreaterThan(0);
    expect(body, 'deliver() 仍把写侧事实报成非法状态迁移').not.toContain(
      'ErrorCode::E_STATE_INVALID_TRANSITION',
    );
  });

  it('R2-c：每个 HostBoundary 值要么有生产使用点，要么在诚实清单里（无孤儿值）', () => {
    const errorsTs = read('packages/tauron-host/src/errors.ts');
    const unionBlock = /export type HostBoundary =([\s\S]*?);/.exec(errorsTs);
    expect(unionBlock, '未找到 HostBoundary 联合定义').not.toBeNull();
    // 只在 HostBoundary 的联合体内取值：别的联合类型若也用 `| 'x'` 写法不应被误判为边界。
    const union = [...unionBlock![1]!.matchAll(/'([\w→-]+)'/g)].map((m) => m[1]!);
    expect(union.length, 'HostBoundary 联合成员').toBeGreaterThanOrEqual(3);

    // 生产调用点：排除测试与 errors.ts 自身（那里是定义与实现）。
    const prod = [
      'packages/tauron-host/src/host.ts',
      'packages/tauron-host/src/shell-client.ts',
      'packages/tauron-host/src/rpc.ts',
      'packages/tauron-host/src/shell-controller.ts',
    ].map(read);
    const wired = new Set<string>();
    for (const src of prod) {
      for (const m of src.matchAll(/translate_at_boundary\([^,]+,\s*'([\w→-]+)'/g)) {
        wired.add(m[1]!);
      }
    }

    const listed = [...errorsTs.matchAll(/UNWIRED_BOUNDARIES[\s\S]*?\];/g)].flatMap((m) =>
      [...m[0]!.matchAll(/'([\w→-]+)'/g)].map((x) => x[1]!),
    );
    const orphans = union.filter((b) => !wired.has(b) && !listed.includes(b));
    expect(orphans, `既没接线也没登记为未接线的边界值: ${orphans.join(', ')}`).toEqual([]);

    // 清单里的值一旦被接线就必须从清单删掉（否则清单会长期说谎）。
    const staleListed = listed.filter((b) => wired.has(b));
    expect(staleListed, `已接线却仍留在 UNWIRED_BOUNDARIES: ${staleListed.join(', ')}`).toEqual([]);

    // 已定位的缺口必须写清消费者与理由（"暂时没接线"与"不知道谁该接"不是一回事）：
    // 把清单本体剔掉后，每个值都必须仍在文档散文里出现，且写明未接线原因。
    const prose = errorsTs.replace(/UNWIRED_BOUNDARIES[\s\S]*?\];/, '');
    for (const b of listed) {
      expect(prose, `文档未说明 ${b} 为何未接线`).toContain(b);
    }
    expect(prose, '未记录 bridge.ts 的保码收口（轮 35）与它为何仍算未接线').toMatch(
      /bridge\.ts[\s\S]{0,260}轮 35[\s\S]{0,300}依赖方向/,
    );
    expect(prose, '未说明依赖方向这一未接线原因').toMatch(/依赖方向/);
  });

  it('R7：RecoveryStore 持久化 last_context，且 boot 线形回传它', () => {
    // 方案 R7 门禁原文：「RecoveryStore 必须持久化 last_context」。
    const recovery = read('crates/tauron-adapter/src/recovery.rs');
    expect(recovery, 'RecoveryStore 缺 last_context 字段').toMatch(/last_context/);
    // 必须真进落盘 payload（只放内存 = 崩溃后什么都没有，门禁要挡的正是这种）。
    expect(recovery, 'last_context 没有进入落盘序列化').toMatch(
      /"lastContext"|lastContext[\s\S]{0,120}to_json/,
    );
    // 读取容错：文件里没有该字段必须是"无上下文"，而不是把整份标记判成损坏。
    expect(recovery, 'last_context 读取缺容错/默认值').toMatch(
      /last_context[\s\S]{0,200}unwrap_or/,
    );
    // 线形：boot 返回必须带上它。
    const lib = read('crates/tauron-adapter/src/lib.rs');
    expect(lib, 'host_recover_boot 线形未回传 lastContext').toMatch(/"lastContext"/);
    // 引擎侧容量常量必须存在且被测试钉住（N 是设计决定，不能是魔数）。
    const engine = read('crates/tauron-recovery/src/lib.rs');
    expect(engine, '缺少 context 容量常量').toMatch(/CONTEXT_CAPACITY/);
    expect(engine, 'CONTEXT_CAPACITY 没有测试引用').toMatch(
      /CONTEXT_CAPACITY[\s\S]{0,400}assert|assert[\s\S]{0,400}CONTEXT_CAPACITY/,
    );
  });

  it('R7：settings 经 SettingsStore（不是裸 HashMap），且迁移有线上入口', () => {
    // 方案 R7 门禁原文：「settings 必须经 Store（非裸 HashMap）」。
    const cargo = read('crates/tauron-adapter/Cargo.toml');
    expect(cargo, 'adapter 未依赖 tauron-settings（孤儿 crate 未激活）').toMatch(/tauron-settings/);
    const lib = read('crates/tauron-adapter/src/lib.rs');
    expect(lib, 'settings 未走 SettingsStore').toMatch(/SettingsStore/);
    expect(lib, 'settings_set 未走 V4 deferred transaction 写入').toMatch(
      /set_deferred\(HOST_SETTINGS_NAMESPACE,\s*HOST_SETTINGS_NAMESPACE/,
    );
    // 提交口收成一个函数（§33 R2-4 轮 4）：watcher 与消息面镜像必须同处一地发生，
    // 所以这里断的是「写路径 → 单一提交口」的接线，而不是「写命令里能看到 publish」。
    // 若有人新增写路径绕过 commit_settings_change，第二条断言就会失配（镜像帧只有一处产生点）。
    expect(lib, 'settings_set 未在持久化成功后经单一提交口发布变更').toMatch(
      /persist_settings_doc\(state\)[\s\S]{0,400}commit_settings_change\(state,\s*event\)/,
    );
    expect(lib, 'committed revision 的发布口必须是 commit_settings_change（且只有一处）').toMatch(
      /fn commit_settings_change\([\s\S]{0,700}publish_committed_change\(event\)[\s\S]{0,700}HOST_SETTINGS_CHANGED_TOPIC/,
    );

    // 断链回归（本轮实测）：`host_settings_adopt_legacy` / `host_settings_migrate`
    // 曾是**没有注册成命令**的纯函数 → 迁移只有单元测试能碰到。命令名必须出现在
    // handler 宏里，`ShellClient` 也必须有对应方法（两侧任一改名即红）。
    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    const macroBody = tauri.slice(
      tauri.indexOf('macro_rules! tauron_substrate_handler'),
      tauri.indexOf('macro_rules! tauron_plugin_handler'),
    );
    for (const cmd of ['host_settings_adopt_legacy', 'host_settings_migrate']) {
      expect(macroBody, `底座宏未注册 ${cmd}（迁移能力线上不可达）`).toContain(cmd);
    }
    const shell = read('packages/tauron-host/src/shell-client.ts');
    expect(shell, 'ShellClient 缺 settingsAdoptLegacy').toContain('host_settings_adopt_legacy');
    expect(shell, 'ShellClient 缺 settingsMigrate').toContain('host_settings_migrate');
  });

  it('R7：cmd_notify 必须经 dispatch（顺序：先入缓冲再推系统），且 DispatchSink 有 Tauri 实现', () => {
    // 方案 R7 门禁原文：「cmd_notify 必须调 dispatch」「DispatchSink 必须有 Tauri 实现」。
    const lib = read('crates/tauron-adapter/src/lib.rs');
    expect(lib, 'cmd_notify 未接 dispatch').toMatch(/tauron_notify::dispatch|notify::dispatch/);
    const notify = read('crates/tauron-notify/src/lib.rs');
    // 顺序不变量：push 必须早于 send（反过来 = dispatch 失败就丢通知）。
    const dispatchBody = notify.slice(notify.indexOf('pub fn dispatch('));
    const pushAt = dispatchBody.indexOf('push');
    const sendAt = dispatchBody.indexOf('send');
    expect(pushAt, 'dispatch 里没有 push（通知不入缓冲）').toBeGreaterThan(-1);
    expect(sendAt, 'dispatch 里没有 send（没推系统）').toBeGreaterThan(-1);
    expect(pushAt, '顺序错误：必须先 push 再 send（否则系统通知失败会丢通知）').toBeLessThan(
      sendAt,
    );

    // Tauri 实现必须是真类型实现 trait，不能只是注释里提一句。
    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    expect(tauri, 'DispatchSink 无 Tauri 实现').toMatch(
      /impl\s+tauron_notify::DispatchSink\s+for\s+\w+/,
    );
    // 且必须真被注入（有实现但没人 set = 生产上永远没有系统通道）。
    expect(tauri, 'DispatchSink 实现未被注入').toMatch(/notify_sink[\s\S]{0,200}\.set\(/);
  });

  it('R7：通知读取端暴露 dispatchLog（线形字段，不是只写日志）', () => {
    const lib = read('crates/tauron-adapter/src/lib.rs');
    expect(lib, 'host_notifications_list 未返回 dispatchLog').toMatch(/"dispatchLog"/);
    const shell = read('packages/tauron-host/src/shell-client.ts');
    expect(shell, 'TS 侧 NotificationsListResult 缺 dispatchLog').toMatch(
      /NotificationsListResult[\s\S]{0,600}dispatchLog/,
    );
    const notify = read('crates/tauron-notify/src/lib.rs');
    expect(notify, 'dispatchLog outcome 词表缺失').toMatch(/DispatchOutcome/);
  });

  it('R8：窗口/商城的"降级与模拟"必须是线字段，不是注释里的承诺', () => {
    const lib = read('crates/tauron-adapter/src/lib.rs');
    // ① 商城返回必须是有类型的结构体（此前是裸 json! 字面量）且带 simulated。
    expect(lib, 'MarketCheckResult 不是类型化结构体').toMatch(
      /pub struct MarketCheckResult[\s\S]{0,400}pub simulated: bool/,
    );
    expect(lib, 'MarketUpdateResult 不是类型化结构体').toMatch(
      /pub struct MarketUpdateResult[\s\S]{0,400}pub simulated: bool/,
    );
    // `check` 此前是 `{ "available": false }`，连 simulated 都没有 —— 回归锁。
    expect(lib, 'host_market_check 未返回 simulated').toMatch(
      /MarketCheckResult[\s\S]{0,300}simulated: true/,
    );

    // ② 两条新命令的结果必须自带"真的做了吗"的判据。
    expect(lib, 'WindowRelaunchOutcome 缺 relaunch_requested').toMatch(
      /pub struct WindowRelaunchOutcome[\s\S]{0,1200}relaunch_requested: bool/,
    );
    expect(lib, 'WindowCreateOutcome 缺 created').toMatch(
      /pub struct WindowCreateOutcome[\s\S]{0,1200}created: bool/,
    );
    // ③ 先对账、后重启：顺序在源码里必须是这两行的次序（反序 = 重启循环）。
    const core = lib.slice(
      lib.indexOf('pub fn cmd_window_relaunch_as'),
      lib.indexOf('pub fn cmd_window_create_as'),
    );
    const iReconcile = core.indexOf('reconcile_recovery_phase');
    const iRelaunch = core.indexOf('.relaunch()');
    expect(iReconcile, 'relaunch 核心未先做阶段对账').toBeGreaterThan(-1);
    expect(iRelaunch, 'relaunch 核心未调用 sink').toBeGreaterThan(-1);
    expect(iReconcile, '对账必须发生在请求重启之前').toBeLessThan(iRelaunch);

    // ④ TS 侧必须是**必填**字段：可选 = 平台可以静默省略 = 信息再次丢失。
    const shell = read('packages/tauron-host/src/shell-client.ts');
    expect(shell, 'TS MarketCheckResult.simulated 不是必填').toMatch(
      /interface MarketCheckResult[\s\S]{0,300}simulated: boolean;/,
    );
    expect(shell, 'TS MarketUpdateResult.simulated 不是必填').toMatch(
      /interface MarketUpdateResult[\s\S]{0,300}simulated: boolean;/,
    );
    expect(shell, 'TS WindowRelaunchOutcome.relaunchRequested 不是必填').toMatch(
      /interface WindowRelaunchOutcome[\s\S]{0,600}relaunchRequested: boolean;/,
    );
    expect(shell, 'TS WindowCreateOutcome.created 不是必填').toMatch(
      /interface WindowCreateOutcome[\s\S]{0,600}created: boolean;/,
    );
    // ⑤ ShellClient 必须真的暴露这两条新命令（Rust 有、前端没入口 = 断链）。
    expect(shell, 'ShellClient 缺 windowRelaunch').toMatch(/windowRelaunch\(\)/);
    expect(shell, 'ShellClient 缺 windowCreate').toMatch(/windowCreate\(/);

    // ⑥ 「立即重启」必须走 relaunch 而不是 quit：quit 只退出、且跳过恢复对账
    //    （R8 之前宿主面确实只有 quit，注释与实现都过时了，这条防它退回去）。
    const controller = read('packages/tauron-host/src/shell-controller.ts');
    const at = controller.indexOf('SHELL_EVENTS.restart');
    expect(at, 'shell-controller 未监听 oc-restart').toBeGreaterThan(-1);
    const restartHandler = controller.slice(at, at + 400);
    expect(restartHandler, 'oc-restart 未走 windowRelaunch').toMatch(/windowRelaunch\(\)/);
    expect(restartHandler, 'oc-restart 退回了 windowQuit（只退不重启且跳过对账）').not.toMatch(
      /windowQuit\(\)/,
    );
  });

  it('P0-3：CLI 签名必须真调 @tauron/market（同生态消费，不许自己实现一遍）', () => {
    // 方案 R8 门禁原文：「CLI sign 必须调用 `@tauron/market`」。
    const pack = read('packages/tauron-app-cli/src/pack.ts');
    expect(pack, 'app-cli 未消费 @tauron/market').toMatch(/from\s+'@tauron\/market'/);
    // 真调用（不是只 import）：必须出现 market 的签名/验签函数名。
    expect(pack, 'app-cli 未调用 market 的签名函数').toMatch(/\bsign\s*\(|\bverify\s*\(/);

    // 反向：包内不得再自造密码学实现（此前 `hash*31` 却谎报 ed25519 的事就是这里出的）。
    const cryptoish = /\bcreateHmac\b|\bcreateSign\b|\bhash\s*\*\s*31\b|\bgenerateSignature\b/;
    for (const f of ['pack.ts', 'cli.ts', 'index.ts']) {
      const src = read(`packages/tauron-app-cli/src/${f}`);
      expect(cryptoish.test(src), `app-cli/${f} 出现自造签名实现`).toBe(false);
    }
    // 依赖是真的 workspace 依赖（同生态消费，零新外部依赖）。
    const pkg = JSON.parse(read('packages/tauron-app-cli/package.json')) as {
      dependencies?: Record<string, string>;
    };
    expect(pkg.dependencies?.['@tauron/market'], 'app-cli 未声明 @tauron/market 依赖').toBeTruthy();

    // 另一条 CLI（@tauron/cli）此前也在造假签名并谎报 ed25519（轮 11 审计实测）。
    // 现在它必须算**真** SHA-256 摘要并如实标注算法，不得再声称 ed25519。
    // 负向断言必须先剥注释——本仓库习惯把"修正前的错误写法"写进注释里，不剥就会自己踩自己。
    const legacy = read('packages/tauron-cli/src/plugin-lifecycle.ts')
      .replace(/\/\*[\s\S]*?\*\//g, '')
      .replace(/^\s*\/\/.*$/gm, '');
    expect(legacy, '@tauron/cli 又回到手写伪摘要').not.toMatch(/hash\s*\*\s*31/);
    expect(legacy, '@tauron/cli 未用真实 SHA-256').toMatch(/createHash\('sha256'\)/);
    expect(legacy, '@tauron/cli 不得再谎报 ed25519').not.toMatch(/algorithm:\s*'ed25519'/);
    expect(legacy, '摘要实现必须如实标 simulated').toMatch(/simulated:\s*true/);
  });

  it('轮 11 身份判定：主窗专属命令与设置键空间在代码层收口（不只是 ACL）', () => {
    // 这一条把 R7 报告里"硬编码 Caller::MainWindow 只能靠代码审查"的未验证面
    // 变成可执行检查：包装器**必须**从真实窗口 label 取主体。
    const lib = read('crates/tauron-adapter/src/lib.rs');
    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    const fnBody = (src: string, name: string): string => {
      const at = src.search(new RegExp(`(?:pub )?fn ${name}\\b`));
      expect(at, `找不到函数 ${name}`).toBeGreaterThan(-1);
      const end = src.indexOf('\n}\n', at);
      return src.slice(at, end === -1 ? undefined : end);
    };

    // 主体类型**刻意没有** Invalid 变体：让"未知主体"连构造都构造不出来，
    // 否则它迟早被当作可传递值漏判（R7 的设计决定，这里锁死）。
    const enumAt = lib.search(/pub enum Caller\b/);
    expect(enumAt, 'Caller 类型缺失').toBeGreaterThan(-1);
    const enumBody = lib.slice(enumAt, lib.indexOf('\n}\n', enumAt));
    expect(enumBody, 'Caller 缺 MainWindow 变体').toMatch(/MainWindow/);
    expect(enumBody, 'Caller 缺 Plugin 变体').toMatch(/Plugin/);
    expect(enumBody, 'Caller 不得有 Invalid 变体（可传递的未知主体迟早被漏判）').not.toMatch(
      /Invalid/,
    );

    // 解析必须复用既有 authz 解析器——造第二套解析 = 迟早两份规则不一致。
    expect(fnBody(lib, 'from_label'), 'from_label 未复用 resolve_principal').toMatch(
      /resolve_principal/,
    );

    // 主窗专属命令：(包装器, 真正执行判定的核心函数)。
    // 轮 11 R8 追加 `host_window_relaunch` / `host_window_create`（都仅主窗）。
    const privileged: Array<[string, string]> = [
      ['host_registry_list_all', 'cmd_registry_list_all_as'],
      ['host_registry_admin', 'cmd_registry_admin_as'],
      ['host_runtime_spawn', 'cmd_runtime_spawn_as'],
      ['host_runtime_health', 'cmd_runtime_health_as'],
      ['host_resource_stats', 'cmd_resource_stats_as'],
      ['host_production_doctor', 'cmd_production_doctor_as'],
      ['host_settings_adopt_legacy', 'cmd_settings_adopt_legacy_as'],
      ['host_settings_migrate', 'cmd_settings_migrate_as'],
      ['host_window_relaunch', 'cmd_window_relaunch_as'],
      ['host_window_create', 'cmd_window_create_as'],
    ];
    for (const [cmd, core] of privileged) {
      const wrapper = fnBody(tauri, cmd);
      expect(
        wrapper,
        `${cmd} 没从受信传输上下文取得主体（硬编码 Caller::MainWindow 即可绕过判定）`,
      ).toMatch(/(?:window\.caller\(\)|Caller::from_label\(window\.label\(\)\))/);
      expect(wrapper, `${cmd} 未把解析出的主体交给核心`).toMatch(
        new RegExp(`(?:crate::)?(?:${core}|wire_registry_admin)\\(&?caller`),
      );
      // 判定可以是裸 `require_main_window`，也可以是它的**审计版** `admin_gate`
      // （内部就是同一条判定 + 留痕；见「特权管理操作必须在唯一咽喉点产出结构化
      // 审计事实」那条门禁，它钉死了 admin_gate 体内必须调 require_main_window）。
      expect(fnBody(lib, core), `${core} 收了主体却不判定`).toMatch(
        /require_main_window|admin_gate\(/,
      );
    }

    // 第二批（轮 11）：身份绑定参数的命令——插件只能以**自己**的名义调，
    // 主窗任意。判定函数是 `require_self_plugin_scope`（与 require_main_window
    // 同族、同码：E_AUTH_DENIED）。
    const selfScoped: Array<[string, string]> = [
      ['host_notify', 'cmd_notify_as'],
      ['host_recover_report', 'cmd_recover_report_as'],
      ['host_i18n_load', 'cmd_i18n_load_as'],
      ['host_i18n_cleanup_plugin', 'cmd_i18n_cleanup_plugin_as'],
    ];
    for (const [cmd, core] of selfScoped) {
      const wrapper = fnBody(tauri, cmd);
      expect(
        wrapper,
        `${cmd} 没从受信传输上下文取得主体（硬编码 Caller::MainWindow 即可绕过判定）`,
      ).toMatch(/(?:window\.caller\(\)|Caller::from_label\(window\.label\(\)\))/);
      expect(wrapper, `${cmd} 未把解析出的主体交给核心`).toMatch(
        new RegExp(`(?:crate::)?${core}\\(&caller`),
      );
      expect(fnBody(lib, core), `${core} 收了主体却不做自身范围判定`).toMatch(
        /require_self_plugin_scope/,
      );
    }
    const selfScope = fnBody(lib, 'require_self_plugin_scope');
    expect(selfScope, '自身范围判定的拒绝码必须是既有的 E_AUTH_DENIED').toMatch(/E_AUTH_DENIED/);
    // 判据必须落在"插件主体"上（主窗放行 = 非 Plugin 分支直接 Ok），
    // 且**必须真比较**自称 id 与自己（只判"是插件"等于没判），
    // `None`（宿主级命名空间 / 应用级上报）必须单独拒绝。
    expect(selfScope, '自身范围判定未识别插件主体').toMatch(/Caller::Plugin\(/);
    expect(selfScope, '自身范围判定未比较自称 id 与自己').toMatch(/id\s*==\s*me/);
    expect(selfScope, '缺少 None（宿主级/应用级）拒绝分支').toMatch(/None\s*=>/);

    // 第二批的四条主窗专属命令（试启别人的插件、宿主级更新操作）。判定可以是裸
    // `require_main_window`，也可以是它的**审计版** `admin_gate`（轮 40 起
    // download/install 走后者：判定 + 结构化审计留痕，内部仍调 require_main_window）。
    for (const [cmd, core] of [
      ['host_recover_trial_enable', 'cmd_recover_trial_enable_as'],
      ['host_market_check', 'cmd_market_check_as'],
      ['host_market_download', 'cmd_market_download_as'],
      ['host_market_install', 'cmd_market_install_as'],
    ] as Array<[string, string]>) {
      expect(fnBody(tauri, cmd), `${cmd} 未从受信传输上下文取得主体`).toMatch(
        /(?:window\.caller\(\)|Caller::from_label\(window\.label\(\)\))/,
      );
      expect(fnBody(lib, core), `${core} 未做主窗判定`).toMatch(/require_main_window|admin_gate\(/);
    }

    // 设置族不是"整条命令主窗专属"（插件本来就需要读自己的设置），而是**按键**判定。
    for (const core of ['cmd_settings_get_as', 'cmd_settings_set_as']) {
      expect(fnBody(lib, core), `${core} 未做键空间判定`).toMatch(/require_settings_key_scope/);
    }
    const scope = fnBody(lib, 'require_settings_key_scope');
    // 判定写法锁死：必须 `strip_prefix` 后要求"剩下为空或以 `.` 开头"。
    // 裸 `key.starts_with(ns)` 会让 `plugin:p.a` 与 `plugin:p.ab` 互相穿透（双向都出事）。
    expect(scope, '命名空间判定必须剥前缀（strip_prefix）').toMatch(/strip_prefix/);
    expect(scope, '剥前缀后必须要求为空或以 . 开头').toMatch(/is_empty\(\)/);
    expect(scope, '剥前缀后必须校验分隔符 `.`').toMatch(/starts_with\('\.'\)/);
    expect(scope, '命名空间判定不得用裸前缀比较（plugin:p.a 会穿透 plugin:p.ab）').not.toMatch(
      /starts_with\((?:&)?(?:ns|namespace|prefix)\b/,
    );
    // 拒绝码必须是既有身份/越权码：不得为"身份/越权"这类判定另造新码。
    for (const core of ['require_main_window', 'require_settings_key_scope']) {
      expect(fnBody(lib, core), `${core} 的拒绝码必须是既有的 E_AUTH_DENIED`).toMatch(
        /E_AUTH_DENIED/,
      );
    }

    // 档位表 ↔ 代码判定同源：新增一条特权命令却漏接线时，这条遍历测试会红。
    expect(lib, '缺少"档位表 ↔ 代码判定同源"的遍历测试').toMatch(
      /every_privileged_command_in_the_authz_table_denies_plugins/,
    );
    expect(lib, '同源测试必须遍历 authz 的特权表').toMatch(/ADMIN_COMMANDS/);
  });

  it('P1-10：授权档位表自检必须是生产调用（不得只在测试里调用）', () => {
    // 断链回归：`validate_command_registry` 曾经**有实现、有单测、生产零调用**——
    // 档位表写错（空字段 / 重复命令）要等到有人写测试才暴露。现在底座构造会跑它。
    const lib = read('crates/tauron-adapter/src/lib.rs');
    // ① 生产代码里必须真的调用（`tauron_host::authz::validate_command_registry()`）。
    expect(lib, 'tauron-adapter 未在装配期调用 validate_command_registry（P1-10 回归）').toMatch(
      /tauron_host::authz::validate_command_registry\(\)/,
    );
    // ② 调用点必须在**非测试**区域（`#[cfg(test)]` 之前），否则等于没接生产。
    const testModule = lib.indexOf('#[cfg(test)]\nmod substrate_only_tests');
    expect(testModule, '找不到 substrate_only_tests 模块（门禁定位失败）').toBeGreaterThan(-1);
    const production = lib.slice(0, testModule);
    expect(
      production,
      'validate_command_registry 只在测试区域出现，生产装配没接（P1-10 未解）',
    ).toMatch(/tauron_host::authz::validate_command_registry\(\)/);
    // ③ 自检结果必须可读（OnceLock 缓存 + 可断言），不是「调了就丢」。
    expect(production, '自检结果必须被缓存并可断言').toMatch(/fn authz_table_selfcheck\(\)/);
  });

  it('无孤儿命令：每条 #[tauri::command] 都必须真的被某个 handler 宏注册', () => {
    // 轮 11 审计抓到的断链类：`cmd_settings_adopt_legacy` / `cmd_settings_migrate`
    // 曾经**有实现、有包装器、有单测，但没进任何宏**——真实宿主里这两条命令根本不存在，
    // 只有 Rust 单测能碰到。这类"定义了却没注册"的断链，编译器不会报、单测也不会报。
    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    const defined = new Set([...tauri.matchAll(/^\s*pub fn (host_\w+)\(/gm)].map((m) => m[1]!));
    const subStart = tauri.indexOf('macro_rules! tauron_substrate_handler');
    const plugStart = tauri.indexOf('macro_rules! tauron_plugin_handler');
    expect(subStart, '找不到底座宏').toBeGreaterThan(-1);
    expect(plugStart, '找不到全量宏').toBeGreaterThan(-1);
    const substrate = tauri.slice(subStart, plugStart);
    const plugin = tauri.slice(plugStart);
    const inSub = new Set([...substrate.matchAll(/\$crate::tauri::(host_\w+)/g)].map((m) => m[1]!));
    const inPlug = new Set([...plugin.matchAll(/\$crate::tauri::(host_\w+)/g)].map((m) => m[1]!));

    const orphans = [...defined].filter((c) => !inSub.has(c) && !inPlug.has(c));
    expect(orphans, '这些命令定义了但没被任何宏注册（线上不可达）').toEqual([]);

    // 反向也要成立：宏里引用的名字必须真有定义（笔误/删实现留下空引用）。
    const ghosts = [...new Set([...inSub, ...inPlug])].filter((c) => !defined.has(c));
    expect(ghosts, '宏引用了不存在的命令实现').toEqual([]);

    // 结构比例：底座 ⊆ 全量，且差集 = 插件运行时域命令（绑 PluginRuntimeState 的那些）。
    // 16 → 17（M8）；可选 plugin-install feature 另外增加 install + preview 两条主窗命令。
    // 17 → 20（0.4-A1：跨主体调用三命令 host_call_plugin/result/take 入插件域）。
    // 20 → 21（0.4-W3：host_contributes_reconcile 入插件域）。
    // 21 → 22（V4 A79：host_stream_grant 入插件域）。
    const PLUGIN_RUNTIME_DOMAIN_SIZE = 22;
    const notInSub = [...inPlug].filter((c) => !inSub.has(c));
    expect(
      [...inSub].filter((c) => !inPlug.has(c)),
      '底座宏里出现了不在全量宏里的命令（不是子集）',
    ).toEqual([]);
    expect(notInSub.length, '插件运行时域命令数（差集）').toBe(PLUGIN_RUNTIME_DOMAIN_SIZE + 2);
  });

  it('ShellClient 调用的命令必须都在宿主注册表内（名字打错 = 前端永远 reject）', () => {
    // 与上一条互补：上一条管"宿主注册了但 backend 不认识"（available 误报 false），
    // 这条管"客户端调了宿主没有的"（拼错/命令被删/改名漏改）。两条都红才算闭链。
    const shell = read('packages/tauron-host/src/shell-client.ts');
    const called = [
      ...new Set([...shell.matchAll(/'(host_[a-z0-9_]+)'/g)].map((m) => m[1]!)),
    ].sort();
    // 数量下限是"解析没退化成空集"的自我保护，不是业务常量。
    expect(called.length, 'ShellClient 的命令解析疑似失败').toBeGreaterThan(25);
    const registered = new Set(rustHostCommands());
    const missing = called.filter((c) => !registered.has(c));
    expect(missing, 'ShellClient 调用了宿主未注册的命令').toEqual([]);
  });

  it('诚实性：仿真不得冒充成功（三处已修正的伪造点不许回归）', () => {
    // 剥注释：本仓库习惯把"修正前的错误写法"写进注释，负向断言不剥就会自己踩自己。
    const strip = (s: string): string =>
      s.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');

    // ① `@tauron/cli` publish：不许再声称已上传，也不许编造商城 URL。
    const lifecycle = strip(read('packages/tauron-cli/src/plugin-lifecycle.ts'));
    expect(lifecycle, 'publish 又谎报已发布').not.toMatch(/`Published |Published plugin/);
    expect(lifecycle, 'publish 又编造商城地址').not.toMatch(/marketplace\.tauron\.dev/);
    expect(lifecycle, 'publish 必须如实标 published: false').toMatch(/published:\s*false/);
    expect(lifecycle, 'publish 必须如实标 simulated: true').toMatch(/simulated:\s*true/);

    // ② `@tauron/dual-world` sandbox：未接运行时前必须 fail closed。
    const sandbox = strip(read('packages/tauron-dual-world/src/sandbox.ts'));
    expect(sandbox, 'sandbox 又伪报 executed: true').not.toMatch(/executed:\s*true/);
    expect(sandbox, 'sandbox 必须如实报不可用').toMatch(/SANDBOX_UNAVAILABLE/);

    // ③ `@tauron/shell-matrix`：模拟启动必须带 simulated 标记。
    const manager = read('packages/tauron-shell-matrix/src/manager.ts');
    expect(manager, 'shell-matrix 丢了 simulated 诚实标记').toMatch(/simulated:\s*true/);
    const types = read('packages/tauron-shell-matrix/src/types.ts');
    expect(types, 'ShellInstance 缺 simulated 字段').toMatch(/simulated:\s*boolean/);

    // ⑤ 同一文件里的 dev / test / pack：**同类伪造成功，轮 12 修正**，同样不许回归。
    //    这三条此前返回 `success: true` 并声称做了事：dev 说"已启动 dev server
    //    (watch mode)"还编了 `port: 8080`；test 把"发现的测试文件数"当成
    //    `passed` 上报（CI 会因此变绿）；pack 说"已打包进 x.tgz"却没写一个字节。
    //    按函数切段断言，避免"文件里某处有 success: false"就骗过整体匹配。
    const bodyOf = (name: string): string => {
      const start = lifecycle.indexOf(`export function ${name}(`);
      expect(start, `plugin-lifecycle.ts 找不到 ${name}`).toBeGreaterThanOrEqual(0);
      const next = lifecycle.indexOf('\nexport function ', start + 1);
      return lifecycle.slice(start, next < 0 ? undefined : next);
    };
    for (const fn of ['pluginDev', 'pluginTest', 'pluginPack']) {
      const body = bodyOf(fn);
      expect(body, `${fn} 又谎报成功（这几条命令都未实现）`).not.toMatch(/success:\s*true/);
      expect(body, `${fn} 缺 simulated 诚实标记`).toMatch(/simulated:\s*true/);
    }
    expect(lifecycle, 'dev 又谎报已启动 dev server').not.toMatch(/Dev server started/);
    expect(lifecycle, 'dev 又编造端口号').not.toMatch(/port:\s*8080/);
    expect(lifecycle, 'test 又谎报"正在跑测试"').not.toMatch(/Running tests/);
    expect(lifecycle, 'test 又把文件数当通过数').not.toMatch(/passed:\s*testFiles\.length/);
    expect(lifecycle, 'pack 又声称已产出归档').not.toMatch(/Packed plugin/);
    // 正面锚：如实标注"未执行"，且不给出不存在的执行结果字段。
    expect(bodyOf('pluginTest'), 'test 必须标 executed: false').toMatch(/executed:\s*false/);
    expect(bodyOf('pluginPack'), 'pack 必须把 package 置空').toMatch(/package:\s*null/);

    // ⑥ 文档侧：插件开发指南不得再回到"宣称隔离性"的口径（验收 9：无 aspirational 谎言）。
    const guide = read('docs/api/plugin-development-guide.md');
    expect(guide, '指南的类型表又回到"宣称隔离性"口径').not.toMatch(/\| 类型 \| 语言 \| 隔离性 \|/);
    expect(guide, '指南必须点明 wasm 的诚实失败码').toMatch(/E_PLUGIN_TYPE_NO_RUNTIME/);
    expect(guide, '指南必须点明双世界沙箱未接线').toMatch(/SANDBOX_UNAVAILABLE/);

    // ⑦ 文档完整性：轮 12 出过一次"整篇静默变形"——用 PowerShell 批量替换时数组
    //    展平，把文档里**所有**反引号换成了 `t`（`tauron-host` → `ttauron-hostt`），
    //    而该文件未入版本控制、无备份可回滚，只能按残文重建。这类损坏编译期、单测、
    //    甚至"文件存在且非空"的检查都抓不到，只有形状锁能抓。
    for (const f of [
      'docs/architecture/canonical-owners.md',
      'README.md',
      'docs/architecture/app-layer-wire.md',
      'docs/integration/incremental-adoption.md',
    ]) {
      const doc = read(f);
      expect(doc, `${f} 出现反引号被替换成 t 的痕迹`).not.toMatch(/ttauron-/);
      expect(
        (doc.match(/`/g) ?? []).length,
        `${f} 反引号数量异常偏少（疑似被批量替换）`,
      ).toBeGreaterThan(50);
    }

    // ⑧ 反向守卫：Rust 侧不得出现未实现占位（本轮扫描为 0，锁住不让它长回来）。
    for (const f of ['crates/tauron-adapter/src/lib.rs', 'crates/tauron-adapter/src/tauri.rs']) {
      const src = read(f);
      expect(src, `${f} 出现 todo!/unimplemented! 占位`).not.toMatch(/todo!\(|unimplemented!\(/);
    }
  });

  it('轮 13：module-maturity 台账与文件系统/Cargo/文档三方同源（V5 §46.2）', () => {
    type CrateEntry = {
      adapterDependency: string | null;
      overviewWiring: 'wired' | 'partial' | 'unwired' | null;
      ownersTable: string | null;
    };
    const ledger = JSON.parse(read('contracts/module-maturity.json')) as {
      schemaVersion: number;
      crates: Record<string, CrateEntry>;
      packages: Record<string, { name: string; private?: boolean; layer: 'framework' | 'app' }>;
    };
    expect(ledger.schemaVersion, 'module-maturity.json 缺 schemaVersion').toBe(1);

    // ① 收录集合与文件系统逐一对账——「15 crates / 20 packages」这类计数谎报
    //    从此刻意不可再犯：加一个目录不改台账就红。
    const crateDirs = readdirSync(join(workspaceRoot, 'crates')).sort();
    expect(Object.keys(ledger.crates).sort(), 'crates/ 目录集合与台账不一致').toEqual(crateDirs);
    const pkgDirs = readdirSync(join(workspaceRoot, 'packages')).sort();
    expect(Object.keys(ledger.packages).sort(), 'packages/ 目录集合与台账不一致').toEqual(pkgDirs);

    // ② package.json 的 name/private 与台账一致（README 树与两层表靠它对齐）。
    for (const [dir, meta] of Object.entries(ledger.packages)) {
      const pkg = JSON.parse(read(`packages/${dir}/package.json`)) as {
        name: string;
        private?: boolean;
      };
      expect(pkg.name, `${dir} 的包名与台账不一致`).toBe(meta.name);
      expect(Boolean(pkg.private), `${dir} 的 private 与台账不一致`).toBe(Boolean(meta.private));
    }

    // ③ adapterDependency 从 tauron-adapter/Cargo.toml **反推**：默认依赖 /
    //    optional+具体 feature / 不在表内。文档再声称「已接线」也不作数，以表为准。
    const toml = read('crates/tauron-adapter/Cargo.toml');
    const depsSection = /\[dependencies\][\s\S]*?(?=\n\[|$)/.exec(toml)?.[0] ?? '';
    expect(depsSection.length, '未解析到 tauron-adapter 的 [dependencies]').toBeGreaterThan(0);
    for (const [crate, meta] of Object.entries(ledger.crates)) {
      if (crate === 'tauron-adapter') continue;
      const line = new RegExp(`^${crate}\\s*=\\s*[^\\n]*`, 'm').exec(depsSection)?.[0] ?? null;
      let derived: string | null = null;
      if (line) {
        if (/optional\s*=\s*true/.test(line)) {
          let feat: string | null = null;
          for (const m of toml.matchAll(/^([\w-]+)\s*=\s*\[([^\]]*)\]/gm)) {
            if ((m[2] ?? '').includes(`dep:${crate}`)) feat = m[1] ?? null;
          }
          expect(feat, `${crate} 是 optional 但没有 feature 引用 dep:${crate}`).not.toBeNull();
          derived = `feature:${feat}`;
        } else {
          derived = 'default';
        }
      }
      expect(
        meta.adapterDependency,
        `${crate} 的依赖形态与 tauron-adapter/Cargo.toml 不一致（账面/代码漂移）`,
      ).toBe(derived);
    }

    // ④ overview.md 接线表的逐 crate 判定必须等于台账（轮 13 的漂移就在这张表）。
    //    解析按**中文标签**取判定，不按 emoji——emoji 的变体选择符（U+FE0F）在
    //    不同编辑路径下时有时无，把它写进正则会造出「行明明在表里、解析说没有」
    //    的假红（本轮实测踩过）。
    const overview = read('docs/architecture/overview.md');
    const labelMap = new Map<string, string>();
    for (const line of overview.split(/\r?\n/)) {
      if (!line.startsWith('> |')) continue;
      const cell =
        /\*\*(已接线|部分接线|未接线)\*\*\s*\|\s*((?:`tauron-[a-z0-9-]+`)(?:\s*\/\s*`tauron-[a-z0-9-]+`)*)\s*\|/.exec(
          line,
        );
      if (!cell) {
        // 防假绿：行首是表格、单元格里有判定词、却没解析出 crate 列——说明形状变了。
        if (line.includes('接线') && line.includes('`tauron-')) {
          throw new Error(`接线表行无法解析 crate 列：${line.slice(0, 60)}…`);
        }
        continue;
      }
      for (const c of (cell[2] ?? '').split('/')) {
        labelMap.set(c.trim().replace(/`/g, ''), (cell[1] ?? '').trim());
      }
    }
    expect(labelMap.size, 'overview 接线表解析为 0 行（正则失配，防假绿）').toBeGreaterThanOrEqual(
      10,
    );
    const sym = { wired: '已接线', partial: '部分接线', unwired: '未接线' } as const;
    for (const [crate, meta] of Object.entries(ledger.crates)) {
      const expected = meta.overviewWiring ? sym[meta.overviewWiring] : null;
      expect(labelMap.get(crate) ?? null, `overview.md 接线表与台账对 ${crate} 判定不一致`).toBe(
        expected,
      );
    }

    // ⑤ canonical-owners 0.3 归置表的状态必须等于台账。
    const owners = read('docs/architecture/canonical-owners.md');
    const placement = owners.slice(
      owners.indexOf('## 0.3 crate 归置决策'),
      owners.indexOf('## 依据'),
    );
    expect(placement.length, '未定位到 0.3 归置表').toBeGreaterThan(100);
    for (const [crate, meta] of Object.entries(ledger.crates)) {
      const row = new RegExp('^\\| `' + crate + '` \\| ([^|]+) \\|', 'm').exec(placement);
      expect(!!row, `0.3 归置表对 ${crate} 的收录与台账不一致`).toBe(meta.ownersTable !== null);
      if (meta.ownersTable !== null) {
        expect(row![1]!.trim(), `0.3 归置表 ${crate} 状态与台账不同步`).toBe(meta.ownersTable);
      }
    }

    // ⑥ README 结构树与竞品分析的计数必须与台账同源（两处本轮刚错过账）。
    const readme = read('README.md');
    const total = pkgDirs.length;
    const appCount = Object.values(ledger.packages).filter((p) => p.layer === 'app').length;
    expect(readme, 'README 结构树的包总数与台账不一致').toContain(`npm 包（${total} 个目录`);
    expect(readme, 'README 框架层计数与台账不一致').toContain(
      `# ── 框架层（${total - appCount} 个）──`,
    );
    expect(readme, 'README 应用层计数与台账不一致').toContain(`# ── 应用层（${appCount} 个）──`);
    for (const dir of [...pkgDirs, ...crateDirs]) {
      expect(readme, `README 结构树漏了目录 ${dir}（新增模块必须同步结构树与台账）`).toMatch(
        new RegExp(`[├└]── ${dir}/`),
      );
    }
    const comp = read('docs/competitive-analysis/competitive-analysis.md');
    expect(comp, '竞品分析的技术架构 crate 计数与台账不一致').toContain(
      `${crateDirs.length} crates`,
    );
    expect(comp, '竞品分析的包目录计数与台账不一致').toContain(`${total} 个包目录`);

    // ⑦ V5 §46.3：Universal Protocol 之前**冻结**根 `host_*` 命令面（85 = 底座 61 +
    //    运行时 22 + 安装 2）。新增命令只允许「修复 / 补 guard / 必要 handshake」，
    //    且必须显式改这条断言——让「85 悄悄长到 100+」在 CI 里不可能无声发生。
    //    数字本身的真伪由 CI `command-surface:check` 对代码复算，这里冻结的是**上限**。
    const surface = read('docs/api/command-surface.md');
    const surfaceCount = (surface.match(/^\| `host_[a-z0-9_]+` \|/gm) ?? []).length;
    expect(surfaceCount, '命令面行数解析为 0（生成物形状变了，先修门禁再谈新增）').toBeGreaterThan(
      0,
    );
    expect(surfaceCount, 'V5 §46.3 冻结：新增根 host_* 命令必须显式过账').toBe(85);
  });

  it('轮 16/17：npm 包消费者状态与台账同源（孤儿包不得无声增殖）', () => {
    // 轮 13 把 **crate** 的接线状态钉进了台账；npm 包这一侧却仍然只靠文档叙述。
    // 本轮全包扫描查出六个「只有测试/文档在引用」的包（adapter-svelte、adapter-vue、
    // app-contract-kit、dual-world、framework、shell-matrix）——它们里有的 README
    // 写得像已经接进应用。与其逐条修文案，不如把「谁真的 import 了它」变成可复算
    // 的事实：状态只有三种，新增包不登记就红，登记错了也对不上。
    //
    //   repo-consumed  = 仓内有**非测试**源码 import 它
    //   reference-only = 仓内没有任何非测试源码 import（公开发给接入方，如实登记）
    //   entry-point    = 面向用户的入口（有 bin，或本身是私有测试包）——不适用前者
    //
    // 轮 17 独立复查把判定收紧了三刀，因为**旧口径能绿着说谎**（实测，非推演）：
    // ① `src.includes("from '<name>")` 是**前缀**匹配——`@tauron/ui` 会命中
    //    `@tauron/ui-primitives` 的一句注释，于是「零真 import」的包被判 repo-consumed；
    // ② 只认 `from`——副作用/子路径 import（`examples/minimal-app/src/main.ts`
    //    的 `import '@tauron/ui/wc'`）这条**真消费**看不见，反过来也可能把已接线的
    //    包错钉成 reference-only；
    // ③ **生成器模板串**里的 `from '@tauron/adapter-react'` 被当成消费者——
    //    `@tauron/cli` 自己并不依赖 adapter-react，模板宿主也不会因此装上它。
    // 现在只认「注释剥离后的 import/export/require specifier」∧「消费方自己的
    // package.json 真声明了该依赖」。第三条把 `@tauron/adapter-react` 如实改判为
    // reference-only（与 adapter-vue/svelte 同档）。
    type ConsumerStatus = 'repo-consumed' | 'reference-only' | 'entry-point';
    const CONSUMER_STATUSES: ConsumerStatus[] = ['repo-consumed', 'reference-only', 'entry-point'];
    type PkgMeta = {
      name: string;
      private?: boolean;
      layer: 'framework' | 'app';
      consumerStatus?: ConsumerStatus;
    };
    const ledger = JSON.parse(read('contracts/module-maturity.json')) as {
      packages: Record<string, PkgMeta>;
    };

    const skipDirs = new Set(['node_modules', 'dist', 'target', 'coverage']);
    const collect = (dir: string, out: string[] = []): string[] => {
      for (const entry of readdirSync(resolve(workspaceRoot, dir), { withFileTypes: true })) {
        if (skipDirs.has(entry.name)) continue;
        const rel = `${dir}/${entry.name}`;
        if (entry.isDirectory()) collect(rel, out);
        else if (/\.(ts|tsx|js|mjs)$/.test(entry.name) && !/\.(test|spec)\./.test(entry.name)) {
          out.push(rel);
        }
      }
      return out;
    };
    // 只认「包自己的 src/」与「示例的 src/」里的 import：测试文件里出现包名只证明
    // 它被自检覆盖，不证明有生产消费方；README/文档同理（否则任何自述都算消费者）。
    const sources = [
      ...collect('packages').filter((p) => /^packages\/[^/]+\/src\//.test(p)),
      ...collect('examples').filter((p) => /^examples\/[^/]+\/src\//.test(p)),
    ].map((p) => ({ p, src: read(p) }));
    expect(sources.length, '消费者扫描的源码集合为空（目录形状变了，先修门禁）').toBeGreaterThan(
      50,
    );

    const ownerOf = (sourcePath: string): string => sourcePath.split('/').slice(0, 2).join('/');
    const depsCache = new Map<string, Record<string, string>>();
    const declaredDepsOf = (sourcePath: string): Record<string, string> => {
      const owner = ownerOf(sourcePath);
      if (!depsCache.has(owner)) {
        let pkg: {
          dependencies?: Record<string, string>;
          peerDependencies?: Record<string, string>;
          devDependencies?: Record<string, string>;
        } = {};
        try {
          pkg = JSON.parse(read(`${owner}/package.json`));
        } catch {
          pkg = {};
        }
        depsCache.set(owner, {
          ...(pkg.dependencies ?? {}),
          ...(pkg.peerDependencies ?? {}),
          ...(pkg.devDependencies ?? {}),
        });
      }
      return depsCache.get(owner) as Record<string, string>;
    };
    const specifiersOf = (src: string): string[] => {
      const body = stripComments(src);
      const out: string[] = [];
      for (const m of body.matchAll(/(?:\bfrom|\bimport|\bexport)\s*\(?\s*['"]([^'"]+)['"]/g)) {
        out.push(m[1] as string);
      }
      for (const m of body.matchAll(/\brequire\s*\(\s*['"]([^'"]+)['"]/g)) {
        out.push(m[1] as string);
      }
      return out;
    };

    const consumersOf = (pkgDir: string, name: string): string[] => {
      const hits: string[] = [];
      for (const source of sources) {
        if (source.p.startsWith(`packages/${pkgDir}/`)) continue;
        // specifier 必须**恰好**是包名或其子路径：`@tauron/ui` 不得命中
        // `@tauron/ui-primitives`
        if (
          !specifiersOf(source.src).some((spec) => spec === name || spec.startsWith(`${name}/`))
        ) {
          continue;
        }
        if (!(name in declaredDepsOf(source.p))) continue;
        const owner = ownerOf(source.p);
        if (!hits.includes(owner)) hits.push(owner);
      }
      return hits.sort();
    };

    const counted: Record<ConsumerStatus, number> = {
      'repo-consumed': 0,
      'reference-only': 0,
      'entry-point': 0,
    };
    for (const [dir, meta] of Object.entries(ledger.packages)) {
      const status = meta.consumerStatus;
      if (!status) {
        throw new Error(`${meta.name} 缺 consumerStatus：新增 npm 包必须显式登记消费者状态`);
      }
      // 轮 17：未知状态一律炸掉。旧实现把不认识的字符串丢进 `else`，于是
      // 台账里打错一个词（或将来加第四种状态忘了改判定）就退化成
      // 「只要有 bin 或 private 就通过」——那是最省事的假绿路径。
      if (!(CONSUMER_STATUSES as string[]).includes(status)) {
        throw new Error(
          `${meta.name} 的 consumerStatus="${status}" 不在枚举内（${CONSUMER_STATUSES.join(' | ')}）`,
        );
      }
      counted[status] = (counted[status] ?? 0) + 1;
      const consumers = consumersOf(dir, meta.name);
      if (status === 'repo-consumed') {
        expect(
          consumers.length,
          `${meta.name} 台账记为 repo-consumed，但仓内没有任何非测试源码 import 它（断链或谎报）`,
        ).toBeGreaterThan(0);
      } else if (status === 'reference-only') {
        expect(
          consumers,
          `${meta.name} 台账记为 reference-only，但 ${consumers.join(', ')} 已经在 import 它——请改判并补接线说明`,
        ).toEqual([]);
      } else {
        const pkg = JSON.parse(read(`packages/${dir}/package.json`)) as {
          bin?: Record<string, string>;
          private?: boolean;
        };
        expect(
          Boolean(pkg.private) || Object.keys(pkg.bin ?? {}).length > 0,
          `${meta.name} 记为 entry-point，但它既非私有包也没有 bin 字段`,
        ).toBe(true);
        // 轮 17：`bin` 指向的文件必须真存在——否则「入口包」是靠一个跑不起来的
        // 字段认证出来的，而 publish 脚本之外的任何门禁都不会去看它。
        for (const [command, target] of Object.entries(pkg.bin ?? {})) {
          expect(
            existsSync(resolve(workspaceRoot, `packages/${dir}`, target)),
            `${meta.name} 的 bin「${command}」指向 ${target}，该文件不存在`,
          ).toBe(true);
        }
      }
    }
    // 三种状态都必须有人占位：某个状态突然归零，多半是台账被误删了一列。
    for (const [status, n] of Object.entries(counted)) {
      expect(n, `台账里 ${status} 一档为空（状态表被改坏了）`).toBeGreaterThan(0);
    }
  });

  it('轮 17：registry 现值是两个 CLI 包的单一真源', () => {
    // 轮 16 把「仓库源码版本」与「registry 已发布版本」拆成两个事实：脚手架的 pin
    // 若跟着 CLI 自己的版本走，用户在 registry 抬上去之前 `npm install` 就是 ETARGET
    // ——按 README 走第二步的人拿到的是装不上的工程，这是前后端贯通意义上的真断链。
    //
    // 拆完**必须**有门禁钉住，否则删掉 package.json 里那个字段会静默退回 CLI 版本
    // （`version.ts` 的 `?? pkg.version` 兜底），bug 原地复活而全仓全绿——轮 17 独立
    // 复查实测到的正是这条：没有任何门禁读过它。
    const SEMVER = /^\d+\.\d+\.\d+$/;
    const published = new Map<string, { name: string; version: string; published: string }>();
    for (const dir of ['tauron-cli', 'tauron-app-cli']) {
      const pkg = JSON.parse(read(`packages/${dir}/package.json`)) as {
        name: string;
        version: string;
        tauron?: { publishedNpmVersion?: string };
      };
      const value = pkg.tauron?.publishedNpmVersion;
      if (!value) {
        throw new Error(`${pkg.name} 缺 tauron.publishedNpmVersion：未发布状态没有事实来源`);
      }
      if (!SEMVER.test(value)) {
        throw new Error(`${pkg.name} 的 publishedNpmVersion="${value}" 不是三段版本号`);
      }
      published.set(dir, { name: pkg.name, version: pkg.version, published: value });
    }
    const values = new Set([...published.values()].map((p) => p.published));
    expect(values.size, `两个 CLI 对「registry 现值」各有说法：${[...values].join(' vs ')}`).toBe(
      1,
    );

    // 字段必须真的被代码读——留着 JSON key 而没人读，等于没有事实来源
    for (const [dir, source] of [
      ['tauron-cli', 'packages/tauron-cli/src/version.ts'],
      ['tauron-app-cli', 'packages/tauron-app-cli/src/framework-version.ts'],
    ] as const) {
      // 现值只能从 package.json 取，不能抄进代码。判据先剥注释：这几个文件的
      // **注释**里合法地提到过 1.0.2（记录踩坑现场），把它当成写死会误报。
      const code = stripComments(read(source));
      expect(
        code.includes('publishedNpmVersion'),
        `${dir} 的 package.json 有 publishedNpmVersion，但 ${source} 没读它`,
      ).toBe(true);
      expect(
        new RegExp(`=\\s*['"]${published.get(dir)!.published}['"]`).test(code),
        `${source} 把 registry 现值写死成了字面量（只能从 package.json 取）`,
      ).toBe(false);
    }

    // 文档叙述的 registry 现值必须与字段同源：轮 16 之前 README 与代码各说各话，
    // 接入方按哪一份都会踩坑。
    const [registryValue] = [...values];
    for (const doc of ['README.md', 'docs/api/plugin-development-guide.md']) {
      expect(
        read(doc).includes(registryValue as string),
        `${doc} 里没有 registry 现值 ${registryValue}：文档与 package.json 不同源`,
      ).toBe(true);
    }
  });

  it('R2-b：跨包类型断言的前提被固化（必须先 build 再 typecheck）', () => {
    // 实测发现：`@tauron/app-plugin-sdk` 的类型断言解析的是
    // `@tauron/plugin-context-contract` 的**已构建 dist**，因此在只改契约 src
    // 而不重建依赖时，`pnpm --filter @tauron/app-plugin-sdk typecheck` 会**假绿**
    // （用 `subscribe: () => void` 与 `settings?` 两处突变实测：不重建则 0 错，
    // 先 build 则分别 TS2322 与 TS18048）。
    // 故把正确顺序固化成脚本，而不是靠人记得。
    const pkg = JSON.parse(read('package.json')) as { scripts: Record<string, string> };
    const verify = pkg.scripts.verify ?? '';
    expect(verify, '根 package.json 缺 verify 脚本').not.toBe('');
    // `pnpm -r --no-bail <step>` 与 `pnpm -r <step>` 都算，故先归一化标志位。
    const normalized = verify.replace(/--no-bail\s+/g, '');
    const order = ['build', 'typecheck', 'test'].map((s) => normalized.indexOf(`pnpm -r ${s}`));
    expect(
      order.every((i) => i >= 0),
      `verify 未覆盖三步: ${verify}`,
    ).toBe(true);
    expect(
      order[0]! < order[1]! && order[1]! < order[2]!,
      `verify 顺序必须是 build → typecheck → test（否则类型断言读旧 dist）: ${verify}`,
    ).toBe(true);

    // 两个 SDK 的 typecheck 必须同处一条流水线：只跑一侧会漏掉另一侧的漂移。
    for (const name of ['@tauron/app-plugin-sdk', '@tauron/plugin-sdk']) {
      const p = JSON.parse(
        read(`packages/${name.replace('@tauron/', 'tauron-')}/package.json`),
      ) as {
        scripts: Record<string, string>;
      };
      expect(p.scripts.typecheck, `${name} 缺 typecheck`).toBeTruthy();
    }
  });

  it('授权面：客户端可达命令集不得超过各自档位（插件侧不得触达特权命令）', () => {
    // 为什么需要这条：档位表（`capabilities.ts` / Rust `CommandAuth`）只说"某命令是特权"，
    // 并不阻止有人把特权命令挂到**插件侧**客户端上——插件 webview 于是天然拿到
    // 一个越权入口，而类型面上看不出问题（`host_registry_admin` 曾被误读为在
    // `HostClient` 上，实测它在独立的 `AdminClient`，消费方是主窗
    // `shell-controller.ts`）。这条门禁把"哪个客户端能摸到哪些命令"变成可判定的。
    const capabilitiesTs = read('packages/tauron-host/src/capabilities.ts');
    const tierOf = new Map<string, string>();
    // 按 `command:` 切块再在块内取 `tier:`——若用一个跨条目的正则，前一 match 的
    // lastIndex 会越过下一条的 command 行，静默少解析一条（实测 12 → 11）。
    for (const chunk of capabilitiesTs.split(/\bcommand:\s*'/).slice(1)) {
      const name = /^([a-z0-9_]+)'/.exec(chunk)?.[1];
      const tier = /tier:\s*'([a-z-]+)'/.exec(chunk)?.[1];
      if (name && tier) tierOf.set(name, tier);
    }
    expect(tierOf.size, '能力表解析出的命令数').toBeGreaterThanOrEqual(12);

    const stripComments = (s: string): string =>
      s.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');
    /**
     * 取出某个 `export class` 类体里引用的全部 `host_*` 命令字面量。
     *
     * **刻意不用「调用形状」正则**（`(?:invoke|call)<…>\('host_x'`）：实测它会
     * **静默漏解析**。`AdminClient.registryInstallPreview` 的写法是
     * `this.call<{ permissions: Array<{ … }> }>('host_registry_install_preview', …)`，
     * 泛型实参跨行且**内含 `>`**，`<[^>]*>` 匹配不到 `(`，整条命令被丢掉——反向
     * 覆盖检查因此误报「没有任何客户端入口」。改成类体内 `'host_*'` 字面量抽取，
     * 没有这个盲区，口径也与同文件的 `tsShellCommands()` 一致。
     */
    const commandsOfClass = (src: string, cls: string): string[] => {
      const starts: Array<[string, number]> = [];
      const re = /^export class (\w+)/gm;
      let m: RegExpExecArray | null;
      while ((m = re.exec(src)) !== null) starts.push([m[1]!, m.index]);
      const idx = starts.findIndex(([n]) => n === cls);
      if (idx < 0) throw new Error(`未找到 class ${cls}`);
      const end = idx + 1 < starts.length ? starts[idx + 1]![1] : src.length;
      const body = stripComments(src.slice(starts[idx]![1], end));
      return [...new Set([...body.matchAll(/'([a-z0-9_]*host_[a-z0-9_]+)'/g)].map((x) => x[1]!))];
    };

    const hostTs = read('packages/tauron-host/src/host.ts');
    const shellTs = read('packages/tauron-host/src/shell-client.ts');
    // 前两条是**身份绑定**客户端：`HostClient` 只服务插件自身，`AdminClient` 只服务
    // 主窗管理面。对它们可以整体卡档位（整类命令只允许某一档）。
    const clients: Array<[string, string, string[]]> = [
      // 插件侧：只能 self / scoped-read（越权面就是从这里漏出去的）。
      ['host.ts', 'HostClient', ['self', 'scoped-read']],
      // 注册表管理：唯一的管理客户端，只能是特权档。
      ['host.ts', 'AdminClient', ['privileged']],
    ];

    const reached = new Set<string>();
    for (const [file, cls, allowed] of clients) {
      const src = file === 'host.ts' ? hostTs : shellTs;
      const cmds = commandsOfClass(src, cls);
      expect(cmds.length, `${cls} 一个命令都没解析到——门禁失效`).toBeGreaterThan(0);
      for (const cmd of cmds) {
        reached.add(cmd);
        const tier = tierOf.get(cmd);
        // 这就是实测抓到的缺口：插件侧客户端调了 4 条**完全没有档位**的命令
        // （host_stream_open/write/close、host_events_drain）。没有档位 = 授权层
        // 对插件面这些命令没有定义，且 CLI 的能力白名单（`validateCapabilities`
        // 用 CAPABILITIES 当白名单并生成 capabilities.json）会直接拒绝它们。
        expect(tier, `${cls} 调用了未登记命令 ${cmd}（插件面命令必须有档位）`).toBeTruthy();
        expect(
          allowed.includes(tier!),
          `${cls} 触达了 ${cmd}（档位 ${tier}），但该客户端只允许 ${allowed.join('/')}`,
        ).toBe(true);
      }
    }

    // ── 主窗壳客户端（ShellClient）────────────────────────────────────────
    //
    // 此前这一段是**假绿**：`shellTs` 被读进来却从未使用
    // （`file === 'host.ts' ? hostTs : shellTs` 恒取 `hostTs`，因为 `clients` 里
    // 只有 `host.ts` 两条目），于是"哪个客户端能摸到哪些命令"对**权限最大的**
    // 那个客户端是一句空话——它的注释却正好写着这条门禁是为了防越权入口。
    //
    // 它不能像前两条那样整体卡档位：档位把「谁能**发起**」与「谁的**身份**被操作」
    // 混在一个字段里（`host_call_plugin` 是 self 档，但主窗**可以**作为发起方调用
    // 它）。所以改成**显式允许集**：ShellClient 能触达的插件档命令必须恰好是三条。
    // 主窗的越权方向是"去走插件自身身份面"——主窗没有 `plugin-<id>` 身份，
    // 调 `host_lifecycle_report` / `host_events_*` / `host_stream_*` /
    // `host_plugin_call` 要么恒失败，要么等于伪造一个不存在的插件身份。
    const shellCmds = commandsOfClass(shellTs, 'ShellClient');
    expect(
      shellCmds.length,
      'ShellClient 命令解析为空——授权面门禁对它已失效',
    ).toBeGreaterThanOrEqual(8);
    for (const cmd of shellCmds) reached.add(cmd);

    const SHELL_PLUGIN_FACE_ALLOW = [
      'host_call_plugin', // 主窗作为发起方的跨主体调用
      'host_call_take', // 取回上述调用的结算结果
      'host_contributes_list', // 渲染菜单 / 面板需要读贡献表
      // 轮 7（R1-4 fail-closed）登记：能力协商的**入口**必须是任何主体可读的
      // 自述接口——它不操作任何插件的身份面，只返回宿主自己的命令面。
      // 不放开的后果是主窗无法完成协商，只能退回静态全集，那正是 R1-4 的原始缺陷。
      'host_capabilities', // 主窗 / 插件都经它做能力协商（宿主自述，只读）
    ];
    const shellPluginFace = shellCmds.filter((c) => {
      const t = tierOf.get(c);
      return t === 'self' || t === 'scoped-read';
    });
    expect(
      [...shellPluginFace].sort(),
      'ShellClient 触达了插件档命令：主窗不得操作插件自身身份面（要放开请在此显式登记并写明理由）',
    ).toEqual([...SHELL_PLUGIN_FACE_ALLOW].sort());

    // 反向覆盖：表里每条命令都必须**至少被一个客户端触达**。没有这一条时，
    // 档位表可以养着一条谁都调不到的命令（表里有、代码里没有入口），
    // 而正向检查照样全绿。`reached` 此前只被填充、从未被断言——一并修掉。
    const unreachable = [...tierOf.keys()].filter((c) => !reached.has(c)).sort();
    expect(unreachable, `能力表登记了没有任何客户端入口的命令: ${unreachable.join(', ')}`).toEqual(
      [],
    );

    // 反向：表里的命令必须真在 Rust 宏里注册过（避免表里养着已删除的命令）。
    const rustMacros = new Set(rustHostCommands());
    const ghost = [...tierOf.keys()].filter((c) => !rustMacros.has(c));
    expect(ghost, `档位表登记了 Rust 未注册的命令: ${ghost.join(', ')}`).toEqual([]);

    // 覆盖范围（有意如此）：主窗专属命令由 Tauri ACL 按 label 管辖，不在本表内。
    // 但插件面的其它入口也不能漏——两个 SDK 若直接写出 host_* 字面量（走 Tauri
    // invoke 的那部分），同样必须落在表内，否则就等于绕开了档位。
    for (const pkg of ['tauron-plugin-sdk', 'tauron-app-plugin-sdk']) {
      const dir = resolve(workspaceRoot, `packages/${pkg}/src`);
      const files: string[] = [];
      const walk = (d: string): void => {
        for (const e of readdirSync(d, { withFileTypes: true })) {
          if (e.isDirectory()) walk(join(d, e.name));
          else if (e.name.endsWith('.ts') && !e.name.includes('.test.'))
            files.push(join(d, e.name));
        }
      };
      walk(dir);
      const invoked = new Set<string>();
      for (const f of files) {
        for (const m of readFileSync(f, 'utf8').matchAll(/'(host_[a-z0-9_]+)'/g))
          invoked.add(m[1]!);
      }
      const untiered = [...invoked].filter((c) => !tierOf.has(c) && rustMacros.has(c));
      expect(untiered, `${pkg} 直接调用了无档位的插件面命令: ${untiered.join(', ')}`).toEqual([]);
    }
  });
});

describe('门禁：设置变更镜像 topic 线值（Rust HOST_SETTINGS_CHANGED_TOPIC ↔ SDK）', () => {
  // §33 R2-4 / W6：宿主在设置落盘提交后把变更镜像到消息面，SDK 的
  // `onSettingsChanged` 订阅同一个 topic。两侧各写一份字面量就会漂移——
  // 漂移的后果不是编译失败，而是「钩子安静地永不触发」，正是本仓库最忌讳的断链形态。
  const rustTopic = (): string => {
    const src = read('crates/tauron-adapter/src/lib.rs');
    const value = /pub const HOST_SETTINGS_CHANGED_TOPIC: &str = "([^"]+)"/.exec(src)?.[1] ?? '';
    expect(value, '未解析出 Rust 侧 HOST_SETTINGS_CHANGED_TOPIC（正则失配）').not.toBe('');
    return value;
  };

  const sdkTopic = (): string => {
    const src = read('packages/tauron-app-plugin-sdk/src/createPlugin.ts');
    const value = /export const HOST_SETTINGS_CHANGED_TOPIC = '([^']+)'/.exec(src)?.[1] ?? '';
    expect(value, '未解析出 SDK 侧 HOST_SETTINGS_CHANGED_TOPIC（正则失配）').not.toBe('');
    return value;
  };

  it('两侧 topic 取值一致', () => {
    expect(sdkTopic()).toBe(rustTopic());
  });

  it('SDK 用常量订阅，不重复硬编码字面量', () => {
    const sdk = read('packages/tauron-app-plugin-sdk/src/createPlugin.ts');
    const subscribe =
      /if \(def\.onSettingsChanged\)[\s\S]*?ctx\.events\.subscribe\(([^,]+),/.exec(sdk)?.[1] ?? '';
    expect(subscribe.trim(), 'onSettingsChanged 订阅点未解析（正则失配）').toBe(
      'HOST_SETTINGS_CHANGED_TOPIC',
    );
  });

  it('镜像帧字段（key/value/source/revision）两侧同集合', () => {
    const rust = read('crates/tauron-adapter/src/lib.rs');
    const publish =
      /HOST_SETTINGS_CHANGED_TOPIC,\s*\n\s*serde_json::json!\(\{([\s\S]*?)\n\s*\}\),/.exec(
        rust,
      )?.[1] ?? '';
    const rustFields = [...publish.matchAll(/"([a-z]+)":/g)].map((m) => m[1]!).sort();
    expect(rustFields.length, '未解析出 Rust 镜像帧字段（正则失配）').toBeGreaterThan(0);
    expect(rustFields).toEqual(['key', 'revision', 'source', 'value']);

    const sdk = read('packages/tauron-app-plugin-sdk/src/createPlugin.ts');
    expect(/payload as \{ key\?: unknown; value\?: unknown \}/.test(sdk), 'SDK 未读 key').toBe(
      true,
    );
    // 消息面暴露**线形键**（调用方 set/get 用的形态），不是 Store 的编码点路径。
    expect(/frame\.key/.test(sdk)).toBe(true);
    expect(/isOwnSettingsKey\(def\.id, frame\.key\)/.test(sdk)).toBe(true);
  });

  it('钩子有真实投递路径（不再是只在类型里存在的孤儿 API）', () => {
    const sdk = read('packages/tauron-app-plugin-sdk/src/createPlugin.ts');
    expect(/def\.onSettingsChanged\?\.\(/.test(sdk), 'onSettingsChanged 从未被调用').toBe(false);
    expect(/Promise\.resolve\(hook\(\{ \[frame\.key\]: frame\.value \}, ctx\)\)/.test(sdk)).toBe(
      true,
    );
    const index = read('packages/tauron-app-plugin-sdk/src/index.ts');
    expect(/HOST_SETTINGS_CHANGED_TOPIC/.test(index), 'topic 常量未从公共入口导出').toBe(true);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 轮 36：通知**推送腿**（Rust `emit` → TS 订阅 → 拉取 → 上屏 → 文档）
//
// 这一条的断链形态和轮 31 一模一样：**发出端一路绿灯**。`TauriDispatchSink::send`
// 每次都把通知 `emit` 到 `NOTIFICATION_TOPIC` 并返回 `Ok`，而轮 36 之前全仓没有任何
// 代码监听那个 topic，`ShellClient.notificationsList()` 也只有自己的单测在调——
// 于是"通知已投递"是真的，"用户看得到"是假的，而且不报错。
// 门禁因此逐段钉住：线值不漂移（①）、订阅方吃的是信号不是正文（②③）、
// 生命周期收得住（④）、生产消费者还在（⑤⑥）、文档说的是同一件事（⑦）、
// 没有为这条腿偷偷加命令（⑧，85 条冻结）。
// ──────────────────────────────────────────────────────────────────────────
describe('门禁：轮 36 通知推送腿（信号 → 拉取 → 上屏，逐段可核对）', () => {
  const RUST = 'crates/tauron-adapter/src/tauri.rs';
  const TOPICS = 'packages/tauron-host/src/host-topics.ts';
  const CONTROLLER = 'packages/tauron-host/src/shell-controller.ts';
  const EXAMPLE = 'examples/minimal-app/src/main.ts';

  const rust = read(RUST);
  const controller = read(CONTROLLER);
  const example = read(EXAMPLE);

  /** TS 侧源码清单（排除测试：测试里写线值做断言是允许的）。 */
  function notifSrcFiles(): string[] {
    const files: string[] = [];
    const walk = (dir: string): void => {
      for (const e of readdirSync(dir, { withFileTypes: true })) {
        const p = join(dir, e.name);
        if (e.isDirectory()) walk(p);
        else if (e.name.endsWith('.ts') && !e.name.includes('.test.')) files.push(p);
      }
    };
    for (const base of ['packages', 'examples']) {
      for (const pkg of readdirSync(join(workspaceRoot, base), { withFileTypes: true })) {
        if (!pkg.isDirectory()) continue;
        const src = join(workspaceRoot, base, pkg.name, 'src');
        if (existsSync(src)) walk(src);
      }
    }
    expect(files.length, '通知腿扫描解析到 0 个 TS 源文件（门禁定位失败）').toBeGreaterThan(10);
    return files;
  }

  const rel = (abs: string): string => abs.slice(workspaceRoot.length + 1).replace(/\\/g, '/');

  /** `docs/` 下全部 Markdown（旧事实可能长在任何一个文件里）。 */
  function docFiles(): string[] {
    const out: string[] = [];
    const walk = (dir: string): void => {
      for (const e of readdirSync(dir, { withFileTypes: true })) {
        const p = join(dir, e.name);
        if (e.isDirectory()) walk(p);
        else if (e.name.endsWith('.md')) out.push(p);
      }
    };
    walk(join(workspaceRoot, 'docs'));
    expect(out.length, '未解析出 docs 下的 Markdown（门禁定位失败）').toBeGreaterThan(0);
    return out;
  }

  it('① topic 线值：Rust 是唯一事实源，TS 侧只有 host-topics.ts 写它', () => {
    const rustValue = /pub const NOTIFICATION_TOPIC: &str = "([^"]+)"/.exec(rust)?.[1] ?? '';
    expect(rustValue, '未解析出 Rust 侧 NOTIFICATION_TOPIC（正则失配）').not.toBe('');
    const tsValue = /export const NOTIFICATION_TOPIC = '([^']+)'/.exec(read(TOPICS))?.[1] ?? '';
    expect(tsValue, '未解析出 TS 侧 NOTIFICATION_TOPIC（正则失配）').not.toBe('');
    expect(tsValue, 'TS 镜像与 Rust 线值漂移：监听会静默零命中而发出端仍一路 Ok').toBe(rustValue);

    // 第二份字面量就是下一次漂移的起点，所以这里数的是**出现位置**而不是取值。
    const literals = notifSrcFiles()
      .filter((p) => readFileSync(p, 'utf8').includes('tauron://notification'))
      .map(rel)
      .sort();
    expect(literals, 'tauron://notification 的字面量只许写在 host-topics.ts').toEqual([TOPICS]);

    const index = read('packages/tauron-host/src/index.ts');
    expect(
      /export \{ NOTIFICATION_TOPIC \} from '\.\/host-topics\.js'/.test(index),
      '线值常量未从 @tauron/host 公共入口导出（接入方只能硬编码）',
    ).toBe(true);
  });

  it('② 订阅的是常量、回调只吃信号、正文一律来自拉取', () => {
    expect(
      /import \{ NOTIFICATION_TOPIC \} from '\.\/host-topics\.js'/.test(controller),
      '控制器没有从 host-topics 取线值',
    ).toBe(true);
    expect(
      /\.listen\(\s*NOTIFICATION_TOPIC\s*,\s*\(\)\s*=>/.test(controller),
      '通知腿没有订阅 NOTIFICATION_TOPIC，或回调又开始接收载荷',
    ).toBe(true);
    expect(
      /onNotification\?: \(snapshot: NotificationsListResult\) => void \| Promise<void>/.test(
        controller,
      ),
      'onNotification 的入参不再是拉取快照：正文可能又从事件载荷里取',
    ).toBe(true);
    expect(
      /this\.client\.notificationsList\(this\._notificationLimit\)/.test(controller),
      '收到信号后没有拉 host_notifications_list（或没有按 notificationLimit 拉）',
    ).toBe(true);
    // 反向：这条腿不许从事件载荷读任何内容字段。
    expect(
      /payload[^\n]{0,24}(title|body|message)/.test(controller),
      '通知腿开始从事件载荷渲染正文 = 跨插件泄露的老路（轮 11）',
    ).toBe(false);
    expect(
      /listen\(\s*NOTIFICATION_TOPIC\s*,\s*\(\w/.test(controller),
      'NOTIFICATION_TOPIC 的回调开始接收参数：载荷里没有正文，参数就是误用的入口',
    ).toBe(false);
  });

  it('③ Rust 广播载荷的字段集合封闭（正文不可能混进来）', () => {
    const body =
      /fn notification_payload[\s\S]*?serde_json::json!\(\{([\s\S]*?)\n\s*\}\)/.exec(rust)?.[1] ??
      '';
    const keys = [...body.matchAll(/"([A-Za-z]+)":/g)].map((m) => m[1]!).sort();
    expect(keys.length, '未解析出 notification_payload 字段（正则失配）').toBeGreaterThan(0);
    expect(keys, '通知信号载荷多了字段：广播给所有 webview 的只能是信号与归属').toEqual([
      'id',
      'kind',
      'pluginId',
      'ts',
    ]);
  });

  it('④ 生命周期与突发合并：退订、代际令牌、补拉都在', () => {
    expect(
      /if \(this\._notifGeneration !== generation\) \{\s*unlisten\(\);/.test(controller),
      '订阅回帧晚于 stop() 时没有就地退订（悬挂监听）',
    ).toBe(true);
    expect(/this\._notifGeneration\+\+/.test(controller), 'stop() 没有推进代际令牌').toBe(true);
    expect(/_notifDirty = true/.test(controller), '并发信号没有合并标记').toBe(true);
    expect(
      /if \(this\._notifDirty\) \{/.test(controller),
      '合并标记设了却从不补拉：突发窗口的最后一条通知会丢',
    ).toBe(true);
    for (const exit of ['notification.subscribe', 'notification.pull', 'notification.render']) {
      expect(controller, `失败出口缺少 ${exit}：这一段的错会被并进别处或直接静默`).toContain(exit);
    }
    // 「渲染抛错」与「拉取失败」必须是两个出口（轮 36 被测试逼出来的那条区分）。
    expect(
      /catch \(err\) \{\s*this\._onError\(err, 'notification\.render'\)/.test(controller),
      '接入方回调的同步抛错没有被单独归到 notification.render',
    ).toBe(true);
  });

  it('⑤ 生产消费者：示例接的是控制器那条腿，不是自己监听 topic', () => {
    expect(
      /onNotification: \(snapshot\) => \{/.test(example),
      '示例没有将 onNotification 接上',
    ).toBe(true);
    expect(
      /item\.read \|\| surfacedNotifications\.has\(item\.id\)/.test(example),
      '示例不再去重',
    ).toBe(true);
    expect(/SURFACED_NOTIFICATION_CAP = \d+/.test(example), '去重集合没有上界').toBe(true);
    expect(
      /toast\.push\?\.\(\{ title: item\.title, message: item\.message/.test(example),
      '示例没有把快照正文上屏',
    ).toBe(true);
    expect(
      /tauron:\/\/notification/.test(example),
      '示例自己订阅 topic = 绕过控制器的合并与退订腿',
    ).toBe(false);
  });

  it('⑥ 读端 API 不再是孤儿（只有自己的单测在调）', () => {
    const pullers = notifSrcFiles()
      .filter((p) => /\.notificationsList\(/.test(readFileSync(p, 'utf8')))
      .map(rel)
      .sort();
    expect(pullers, 'notificationsList() 又回到零生产消费者').toEqual([CONTROLLER]);

    const rendered = notifSrcFiles()
      .filter((p) => /(^|[^_A-Za-z])onNotification: \(/.test(readFileSync(p, 'utf8')))
      .map(rel)
      .sort();
    expect(rendered, 'onNotification 没有真实渲染方（轮 36 的断链形态）').toEqual([EXAMPLE]);

    const tests = read('packages/tauron-host/src/shell-controller.test.ts');
    expect(tests).toContain("it('start() 订阅 NOTIFICATION_TOPIC");
    expect(tests).toContain('连发 5 条信号合并');
  });

  it('⑦ 文档与 Rust 注释说的是同一件事（旧事实不许回归）', () => {
    const readme = read('packages/tauron-host/README.md');
    expect(readme).toContain('NOTIFICATION_TOPIC');
    expect(readme).toContain('onNotification');
    expect(readme).toContain('notification.render');

    const v4 = read(
      'docs/Tauron-Universal-Industrial-Application-Substrate-Final-Architecture-V4.md',
    );
    const w11 = /^\| W11 Message Plane push .*$/m.exec(v4)?.[0] ?? '';
    expect(w11, '未解析出 V4 的 W11 行（正则失配）').not.toBe('');
    expect(w11, 'W11 行没有逐条区分有监听方 / 零监听方').toContain('零监听方');
    expect(w11).toContain('tauron://dialog-degraded');
    expect(w11).toContain('tauron://deep-link-registration');

    // Rust 侧的逐 topic 监听方清单与文档同口径（两处各写一份就会分叉）。
    const invStart = rust.indexOf('哪些 `emit` 真的有人在听');
    const invEnd = rust.indexOf('本模块要求 `tauri` 依赖启用');
    expect(invStart, 'Rust 侧的逐 topic 监听方清单丢失').toBeGreaterThan(-1);
    expect(invEnd, 'Rust 侧监听方清单的结束锚点丢失').toBeGreaterThan(invStart);
    const inventory = rust.slice(invStart, invEnd);
    for (const topic of ['NOTIFICATION_TOPIC', 'DIALOG_DEGRADED_TOPIC', 'DEEP_LINK_NATIVE_TOPIC']) {
      expect(inventory, `监听方清单没有覆盖 ${topic}`).toContain(topic);
    }
    expect(inventory).toContain('零监听方');

    // 「DispatchSink 未接线」是旧事实：任何文档行把它和「未接线」写在一起都是回归。
    const stale = docFiles()
      .map(rel)
      .filter((f) =>
        readFileSync(resolve(workspaceRoot, f), 'utf8')
          .split('\n')
          .some((line) => /DispatchSink/.test(line) && /未接线/.test(line)),
      );
    expect(
      stale,
      `这些文档仍宣称 DispatchSink 未接线（与 tauri.rs 的实现矛盾）: ${stale.join(', ')}`,
    ).toEqual([] as string[]);
    expect(
      /notify_sink\.set\(std::sync::Arc::new\(TauriDispatchSink::new\(app\.clone\(\)\)\)\)/.test(
        rust,
      ),
      '真实装配不再注入 TauriDispatchSink（那句「已接线」就成了空话）',
    ).toBe(true);
  });

  it('⑧ 这条腿没有新增命令：host 侧 host_* 字面量全在命令面文档里', () => {
    const surface = read('docs/api/command-surface.md');
    const names = new Set<string>();
    for (const p of notifSrcFiles()) {
      if (!rel(p).startsWith('packages/tauron-host/src/')) continue;
      for (const m of readFileSync(p, 'utf8').matchAll(/'(host_[a-z0-9_]+)'/g)) {
        // 前缀形态（`startsWith('host_stream_')`）不是命令名。
        if (!m[1]!.endsWith('_')) names.add(m[1]!);
      }
    }
    expect(names.size, '未解析出 host 侧命令字面量（门禁定位失败）').toBeGreaterThan(50);
    const missing = [...names].filter((c) => !surface.includes(c)).sort();
    expect(missing, `命令面文档缺这些命令: ${missing.join(', ')}`).toEqual([] as string[]);
    expect(surface, '通知腿的权威出口未登记').toContain('host_notifications_list');
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 轮 37：菜单 / 托盘**点击腿**（Rust 共享路由表 → `AppHandle::emit` → TS `listen` → 文档）
//
// 断链形态比轮 36 更坏：轮 36 是「真发了但没人听」，轮 37 是「文档指着一条根本不通
// 的路」。四处叙述（`lib.rs` 的字段注释、`tauri.rs` 的命令注释、`shell-client.ts`、
// 由它们生成的 `docs/api/command-surface.md`）都写着点击「经 `host_events_*` 总线
// 回传、用 `host_events_drain` 取件」，而实现一直是 `AppHandle::emit`——照文档接线
// 的人会守着一条永远不会有菜单帧的队列，且不报错。同轮还查出两条更隐蔽的：托盘那条
// lane **从未登记**路由（`TauriTraySink` 只把菜单挂上托盘，监听查不到 id），托盘点击
// 被静默丢弃；`ShellClient` 六条菜单/托盘方法零生产消费者、零测试。
// 门禁逐段钉住：线值与 lane 词表不漂移（①）、共享路由表只剩唯一一处查询且两条
// lane 各自整体替换（②）、旧口径与「宿主会补默认 topic」不许回归（③）、回传帧字段
// 两侧同集合（④）、生产消费者还在且只有一处（⑤）、Rust/TS 两侧都有测试（⑥）、
// 文档镜像同一件事（⑦）、这条腿没新增命令（⑧，85 条冻结）。
// ──────────────────────────────────────────────────────────────────────────
describe('门禁：轮 37 菜单/托盘点击腿（路由 → emit → listen，逐段可核对）', () => {
  const ROUTES = 'crates/tauron-adapter/src/menu_routes.rs';
  const RUST = 'crates/tauron-adapter/src/tauri.rs';
  const TOPICS = 'packages/tauron-host/src/host-topics.ts';
  const CLIENT = 'packages/tauron-host/src/shell-client.ts';
  const INDEX = 'packages/tauron-host/src/index.ts';
  const EXAMPLE = 'examples/minimal-app/src/main.ts';
  const SURFACE = 'docs/api/command-surface.md';

  const routes = read(ROUTES);
  const rust = read(RUST);
  const client = read(CLIENT);
  const example = read(EXAMPLE);

  const rel = (abs: string): string => abs.slice(workspaceRoot.length + 1).replace(/\\/g, '/');

  /** 折叠空白：Rust 的 `///` 会把一句话拆到多行，禁止的口径必须跨行也能抓到。 */
  const flat = (s: string): string => s.replace(/\s+/g, '');

  /** 扫一遍目录里符合后缀的文件（排除测试：测试里写线值做断言是允许的）。 */
  function walk(dir: string, out: string[], accept: (name: string) => boolean): void {
    for (const e of readdirSync(dir, { withFileTypes: true })) {
      const p = join(dir, e.name);
      if (e.isDirectory()) walk(p, out, accept);
      else if (accept(e.name)) out.push(p);
    }
  }

  /** `packages/<pkg>/src` 与 `examples/<pkg>/src` 下的 TS 源文件。 */
  function menuSrcFiles(): string[] {
    const files: string[] = [];
    for (const base of ['packages', 'examples']) {
      for (const pkg of readdirSync(join(workspaceRoot, base), { withFileTypes: true })) {
        if (!pkg.isDirectory()) continue;
        const src = join(workspaceRoot, base, pkg.name, 'src');
        if (existsSync(src)) walk(src, files, (n) => n.endsWith('.ts') && !n.includes('.test.'));
      }
    }
    expect(files.length, '点击腿扫描解析到 0 个 TS 源文件（门禁定位失败）').toBeGreaterThan(10);
    return files;
  }

  /** `crates/<pkg>/src` 下的 Rust 源文件（`target/` 里的旧发布副本不在扫描范围）。 */
  function rustSrcFiles(): string[] {
    const files: string[] = [];
    for (const pkg of readdirSync(join(workspaceRoot, 'crates'), { withFileTypes: true })) {
      if (!pkg.isDirectory()) continue;
      const src = join(workspaceRoot, 'crates', pkg.name, 'src');
      if (existsSync(src)) walk(src, files, (n) => n.endsWith('.rs'));
    }
    expect(files.length, '点击腿扫描解析到 0 个 Rust 源文件（门禁定位失败）').toBeGreaterThan(5);
    return files;
  }

  /** `docs/` 下全部 Markdown（旧事实可能长在任何一个文件里）。 */
  function docFiles(): string[] {
    const out: string[] = [];
    walk(join(workspaceRoot, 'docs'), out, (n) => n.endsWith('.md'));
    expect(out.length, '未解析出 docs 下的 Markdown（门禁定位失败）').toBeGreaterThan(0);
    return out;
  }

  /** 全体扫描面：TS 源 + Rust 源 + docs Markdown。 */
  function clickLegScanTargets(): string[] {
    return [...menuSrcFiles(), ...rustSrcFiles(), ...docFiles()];
  }

  it('① topic 线值与 lane 词表：Rust 是唯一事实源，TS 侧只有 host-topics.ts 写字面量', () => {
    const rustValue = /pub const MENU_CLICK_TOPIC: &str = "([^"]+)"/.exec(routes)?.[1] ?? '';
    expect(rustValue, '未解析出 Rust 侧 MENU_CLICK_TOPIC（正则失配）').not.toBe('');
    const tsValue = /export const MENU_CLICK_TOPIC = '([^']+)'/.exec(read(TOPICS))?.[1] ?? '';
    expect(tsValue, '未解析出 TS 侧 MENU_CLICK_TOPIC（正则失配）').not.toBe('');
    expect(tsValue, 'TS 镜像与 Rust 线值漂移：前端监听会静默零命中，而 emit 端一路返回 Ok').toBe(
      rustValue,
    );

    // 第二份字面量就是下一次漂移的起点，所以这里数的是**出现位置**。
    const tsLiterals = menuSrcFiles()
      .filter((p) => readFileSync(p, 'utf8').includes('tauron://menu-click'))
      .map(rel)
      .sort();
    expect(tsLiterals, 'tauron://menu-click 的字面量只许写在 host-topics.ts').toEqual([TOPICS]);
    const rustLiterals = rustSrcFiles()
      .filter((p) => readFileSync(p, 'utf8').includes('tauron://menu-click'))
      .map(rel)
      .sort();
    expect(rustLiterals, 'Rust 侧的字面量只许写在 menu_routes.rs').toEqual([ROUTES]);

    const index = read(INDEX);
    expect(
      /export \{ MENU_CLICK_TOPIC \} from '\.\/host-topics\.js'/.test(index),
      '线值常量未从 @tauron/host 公共入口导出（接入方只能硬编码）',
    ).toBe(true);

    // lane 词表：`MenuLane::as_str()` 的取值就是 `source` 的取值，两侧各写一份必漂移。
    const laneBody =
      /pub fn as_str\(self\) -> &'static str \{\s*match self \{([\s\S]*?)\n\s*\}/.exec(
        routes,
      )?.[1] ?? '';
    const rustSources = [...laneBody.matchAll(/=> "([a-z]+)"/g)].map((m) => m[1]!).sort();
    expect(rustSources.length, '未解析出 MenuLane::as_str 的取值（正则失配）').toBeGreaterThan(1);
    const union = /export type MenuClickSource = ([^;]+);/.exec(client)?.[1] ?? '';
    const tsSources = [...union.matchAll(/'([a-z]+)'/g)].map((m) => m[1]!).sort();
    expect(tsSources, 'TS MenuClickSource 与 Rust MenuLane::as_str() 词表漂移').toEqual(
      rustSources,
    );
  });

  it('② 只剩一张共享路由表：一次点击一个出口，两条 lane 各自整体替换', () => {
    // 监听只注册一次（Tauri 只有一张全局菜单监听表），且查询点唯一。
    expect(
      [...rust.matchAll(/\.on_menu_event\(/g)].length,
      '全局菜单监听注册了不止一次：同一次点击会被 emit 两遍',
    ).toBe(1);
    expect(
      [...rust.matchAll(/menu_routes\(\)\.lookup\(&id\)/g)].length,
      '路由表查询点必须唯一：两处查询就会有两套 lane 语义',
    ).toBe(1);
    expect(/crate::menu_routes\(\)\.lookup\(&id\)/.test(rust), '监听没有查共享路由表').toBe(true);
    expect(/"source": lane\.as_str\(\)/.test(rust), 'source 不再来自命中 lane').toBe(true);

    // 应用菜单 lane：建成才登记（失败不登记），reset 只失效这一条。
    expect(
      /crate::menu_routes\(\)\.replace\(lane, crate::menu_routes_of\(spec\)\)/.test(rust),
      '菜单没有按 lane 整体替换路由（累加会让旧 id 一直可点）',
    ).toBe(true);
    expect(
      /crate::menu_routes\(\)\.replace\(crate::MenuLane::AppMenu, HashMap::new\(\)\)/.test(rust),
      'menu_reset 没有清空应用菜单那条 lane',
    ).toBe(true);

    // 托盘 lane：轮 37 之前**这一整段都不存在**，托盘点击因此被静默丢弃。
    // create 的 needle 必须盯住它自己那一行：`set_menu` 也往同一条 lane 替换，
    // 只写 `replace(\s*crate::MenuLane::TrayMenu,` 会被它顶包（变异验证时抓到过）。
    expect(
      /crate::menu_routes\(\)\.replace\(\s*crate::MenuLane::TrayMenu,\s*spec\.menu\.as_ref\(\)\.map\(crate::menu_routes_of\)\.unwrap_or_default\(\),/.test(
        rust,
      ),
      'tray_create 没有登记托盘 lane（轮 37 的断链：托盘点了没反应；无菜单时登记空表，旧路由一起失效）',
    ).toBe(true);
    expect(
      // 钉形状不钉同一行：rustfmt 会在 `.replace` 前折行（`cargo fmt` 复跑时抓到过，
      // 与轮 37 记的"变异锚点会腐坏"同型——门禁 needle 自己也会被格式化腐坏）。
      /crate::menu_routes\(\)\s*\.replace\(\s*crate::MenuLane::TrayMenu,\s*crate::menu_routes_of\(spec\)\)/.test(
        rust,
      ),
      'tray_set_menu 没有登记托盘 lane',
    ).toBe(true);
    expect(
      /crate::menu_routes\(\)\.replace\(crate::MenuLane::TrayMenu, HashMap::new\(\)\)/.test(rust),
      'tray_remove 没有清空托盘 lane（托盘没了还留着路由）',
    ).toBe(true);
    // 反向：sink 结构体不许再各存一份私有路由表（那正是旧实现漏掉托盘监听的那一类分叉）。
    expect(/self\.routes\.lock\(\)/.test(rust), 'sink 又回到了私有路由表').toBe(false);
    expect(/routes_for_handler/.test(rust), '又出现了私有路由表的读取口').toBe(false);

    // 装配：两个 sink 都必须注入——注册全局监听的只有 TauriMenuSink::new，
    // 漏掉托盘 sink 则托盘不存在，漏掉菜单 sink 则两条 lane 都没有出口。
    for (const needle of [
      'substrate.menu_sink = std::sync::Arc::new(TauriMenuSink::new(app.clone()));',
      'substrate.tray_sink = std::sync::Arc::new(TauriTraySink::new(app.clone()));',
    ]) {
      expect(rust, `装配少了这一句: ${needle}`).toContain(needle);
    }
  });

  it('③ 旧口径不许回归（transport 与「宿主会补默认 topic」都不行）', () => {
    const bans: Array<[string, RegExp]> = [
      ['「菜单点击经事件总线回传」', /事件总线回传/],
      ['「用既有事件总线」', /用既有事件总线/],
      ['「与其余事件帧走同一条通路」', /与其余事件帧走同一条通路/],
      [
        '「MENU_CLICK_TOPIC 是宿主补的缺省 topic」（实现只对显式填了 event 的项发帧）',
        /缺省topic|缺省值.{0,20}MENU_CLICK_TOPIC/,
      ],
    ];
    for (const [label, re] of bans) {
      const offenders = clickLegScanTargets()
        .filter((p) => re.test(flat(readFileSync(p, 'utf8'))))
        .map(rel)
        .sort();
      expect(
        offenders,
        `这些位置又写下了旧事实「${label}」（与 menu_routes.rs / tauri.rs 的实现矛盾）: ${offenders.join(', ')}`,
      ).toEqual([] as string[]);
    }
    // 正向：唯一事实源里必须留着可核对的真口径。
    expect(rust, 'host_menu_set 的命令注释丢了「只有显式填了 event」').toContain('只有显式填了');
    expect(read(SURFACE), '命令面文档丢了「不经 host_events_* 总线」这条区分').toContain(
      '取不到菜单点击',
    );
  });

  it('④ 回传帧字段：Rust emit 与 TS MenuClickFrame 同集合', () => {
    const emitBody =
      /app\.emit\(\s*&topic,\s*serde_json::json!\(\{([\s\S]*?)\n\s*\}\)/.exec(rust)?.[1] ?? '';
    const rustFields = [...emitBody.matchAll(/"([A-Za-z]+)":/g)].map((m) => m[1]!).sort();
    expect(rustFields.length, '未解析出 emit 载荷字段（正则失配）').toBeGreaterThan(0);

    const frameBody = /export interface MenuClickFrame \{([\s\S]*?)\n\}/.exec(client)?.[1] ?? '';
    const tsFields = [...frameBody.matchAll(/^ {2}(\w+):/gm)].map((m) => m[1]!).sort();
    expect(tsFields.length, '未解析出 MenuClickFrame 字段（正则失配）').toBeGreaterThan(0);
    expect(tsFields, 'TS MenuClickFrame 与宿主 emit 载荷字段漂移').toEqual(rustFields);
    expect(rustFields, '回传帧多了正文字段：点击回传只含 id / 来源 / native 标记').toEqual([
      'id',
      'native',
      'source',
    ]);
    expect(/"native": true/.test(rust), 'native 标记不再恒为 true').toBe(true);
  });

  /** 去掉注释，只留代码：文档里的示例调用形状不能算生产消费者。 */
  const stripComments = (s: string): string =>
    s.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^[ \t]*\/\/.*$/gm, '');

  /** 挂过监听的文件（只看代码，注释里的 `backend.listen(…)` 示例不计）。 */
  function clickListeners(): string[] {
    return menuSrcFiles()
      .filter((p) => /\.listen\(\s*MENU_CLICK_TOPIC/.test(stripComments(readFileSync(p, 'utf8'))))
      .map(rel)
      .sort();
  }

  it('⑤ 生产消费者：示例挂了接收方，也真的调了六条命令', () => {
    expect(clickListeners(), '监听方又回到零（轮 37 的断链形态：emit 一路 Ok，没人收）').toEqual([
      EXAMPLE,
    ]);
    expect(
      [...stripComments(example).matchAll(/\.listen\(\s*MENU_CLICK_TOPIC/g)].length,
      '示例的监听不止挂一次：重复订阅会让一次点击渲染两遍',
    ).toBe(1);
    expect(/const frame = payload as MenuClickFrame/.test(example), '示例没按帧类型解码').toBe(
      true,
    );
    // 订阅失败必须有出口：`TauriBackend.listen()` 原样返回 Tauri 的 `listen()` Promise，
    // 事件系统不可用时会被拒绝；裸 `void` 只会留下 unhandled rejection，于是「前端接上了
    // 点击回传」这句又变成核对不了的承诺。钉的是 catch 体内那句上屏文案——把 catch 删掉、
    // 或把它移出 `menu-out`（只丢 console）都会变红。
    expect(
      flat(stripComments(example)).includes(
        "}).catch((err:Error)=>{log('menu-out',`点击回传腿未挂上",
      ),
      '点击腿的订阅失败没有上屏出口（unhandled rejection 等于没人知道腿没挂上）',
    ).toBe(true);
    for (const id of ['menu-about', 'menu-minimize', 'tray-minimize']) {
      expect(example, `示例没接住 ${id} 这次点击`).toContain(`'${id}'`);
    }

    const refs = menuSrcFiles()
      .filter((p) => /MENU_CLICK_TOPIC/.test(readFileSync(p, 'utf8')))
      .map(rel)
      .sort();
    expect(refs, 'MENU_CLICK_TOPIC 的引用面变了（新增引用点要一并进消费者冻结）').toEqual(
      [TOPICS, INDEX, CLIENT, EXAMPLE].sort(),
    );

    for (const method of [
      'menuSet',
      'menuPopup',
      'menuReset',
      'trayCreate',
      'traySetMenu',
      'trayRemove',
    ]) {
      const callers = menuSrcFiles()
        .filter((p) => new RegExp(`\\.${method}\\(`).test(stripComments(readFileSync(p, 'utf8'))))
        .map(rel)
        .sort();
      expect(callers, `ShellClient.${method}() 又没有生产消费者了（只剩单测在调）`).toEqual([
        EXAMPLE,
      ]);
    }
  });

  it('⑥ 两侧都有测试：Rust 路由表与 TS 客户端不是单边绿', () => {
    const testNames = [...routes.matchAll(/#\[test\]\s*\n\s*fn ([^\s(]+)/g)].map((m) => m[1]!);
    expect(
      [...routes.matchAll(/#\[test\]/g)].length,
      'menu_routes.rs 的 #[test] 少于 6 个（路由表行为无人核对）',
    ).toBeGreaterThanOrEqual(6);
    expect(testNames.length, '#[test] 与 fn 名没解析全（正则失配）').toBeGreaterThanOrEqual(6);
    for (const name of [
      'lane_两道互不干扰',
      'replace_旧_id_随之失效',
      'menu_routes_of_只登记带_event_的项',
      'lane_as_str_与_ts_词表一致',
    ]) {
      expect(testNames, `路由表缺这个用例: ${name}`).toContain(name);
    }

    const tests = read('packages/tauron-host/src/shell-client.test.ts');
    for (const name of [
      '六条命令打到对应命令名，spec 原样透传',
      '缺省装配的 Unsupported 原样交给调用方（不造成功）',
      '点击回传经事件 topic 送达，退订后不再收帧',
    ]) {
      expect(tests, `@tauron/host 缺这个用例: ${name}`).toContain(name);
    }
  });

  it('⑦ 文档镜像同一件事（README / roadmap / 接入指南 / 命令面）', () => {
    const readme = read('packages/tauron-host/README.md');
    for (const needle of ['MENU_CLICK_TOPIC', 'MenuClickFrame', 'traySetMenu', '不替你猜']) {
      expect(readme, `README 丢了点击腿的口径: ${needle}`).toContain(needle);
    }
    // 光有类型名不够：README 必须给出**解码形状**，否则接入方抄不出可运行的接收方。
    expect(readme, 'README 的示例没把载荷解码成 MenuClickFrame').toContain(
      'payload as MenuClickFrame',
    );

    const surface = read(SURFACE);
    const row = (cmd: string): string =>
      surface.split('\n').find((l) => l.startsWith(`| \`${cmd}\` `)) ?? '';
    const setRow = row('host_menu_set');
    expect(setRow, '未解析出 host_menu_set 行（正则失配）').not.toBe('');
    for (const needle of ['MenuClickFrame', 'AppHandle::emit', '只有显式填了']) {
      expect(setRow, `host_menu_set 行丢了点击腿口径: ${needle}`).toContain(needle);
    }
    const trayRow = row('host_tray_create');
    expect(trayRow, '未解析出 host_tray_create 行（正则失配）').not.toBe('');
    expect(trayRow, 'host_tray_create 行没说明托盘共用那一个全局监听').toContain('source: "tray"');
    const resetRow = row('host_menu_reset');
    expect(resetRow, '未解析出 host_menu_reset 行（正则失配）').not.toBe('');
    expect(resetRow, 'host_menu_reset 行没说明只失效 AppMenu lane').toContain('MenuLane::AppMenu');

    const roadmap = read('docs/architecture/multi-plugin-substrate-roadmap.md');
    expect(roadmap, 'roadmap 没指向真实实现（MenuRouteTable）').toContain('MenuRouteTable');
    expect(
      /回调链.{0,20}仍是缺口/.test(flat(roadmap)),
      'roadmap 又把已存在的点击回传写成缺口',
    ).toBe(false);

    const adoption = read('docs/integration/incremental-adoption.md');
    expect(adoption, '接入指南没说明点击走 MenuRouteTable 而不是总线').toContain(
      'crate::MenuRouteTable',
    );
  });

  it('⑧ 这条腿没有新增命令：提案名 host_menu_on_select 仍然不存在', () => {
    for (const p of rustSrcFiles()) {
      expect(
        readFileSync(p, 'utf8'),
        `${rel(p)} 出现了 host_menu_on_select：85 条命令面是冻结的，点击腿靠的是路由表`,
      ).not.toMatch(/fn host_menu_on_select|host_menu_on_select\s*\(/);
    }
    expect(read(SURFACE), '命令面文档登记了一条不存在的命令').not.toContain('host_menu_on_select');
    const surface = read(SURFACE);
    for (const cmd of [
      'host_menu_set',
      'host_menu_popup',
      'host_menu_reset',
      'host_tray_create',
      'host_tray_set_menu',
      'host_tray_remove',
    ]) {
      expect(surface, `命令面文档缺 ${cmd}`).toContain(cmd);
    }
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 轮 38：宿主 `emit` 的 topic **词表**两侧同源（把轮 36/37 的逐条规矩推广到全量）
//
// 轮 36 为 `tauron://notification` 立过"线值只许写在一个文件"，轮 37 又为
// `tauron://menu-click` 立了一次——两条都是出事之后**逐条**补的。本轮把整张表摊开查，
// 同一形状还在：深链接投递腿在 TS 侧压根没有常量，监听点写着裸字面量，宿主改一个字
// 就静默零命中而 `emit` 照旧成功；两条诊断帧则连镜像都没有，接入方只能手打字符串。
// 所以规则改成覆盖全量：Rust 侧每一个 `pub const *TOPIC*` 必须归入下面三层之一，
// 成员集合逐字冻结——新增一条 topic 而不进表，①直接红。
// ──────────────────────────────────────────────────────────────────────────
describe('门禁：轮 38 topic 词表两侧同源（Rust 每个 *_TOPIC 都要归层，线值只许一份镜像）', () => {
  const TOPICS = 'packages/tauron-host/src/host-topics.ts';
  const HOST_INDEX = 'packages/tauron-host/src/index.ts';
  const SDK_CREATE = 'packages/tauron-app-plugin-sdk/src/createPlugin.ts';
  const SDK_INDEX = 'packages/tauron-app-plugin-sdk/src/index.ts';
  const DL_CLIENT = 'packages/tauron-host/src/deep-link-client.ts';
  const SHELL_CTRL = 'packages/tauron-host/src/shell-controller.ts';
  const EXAMPLE = 'examples/minimal-app/src/main.ts';
  const TAURI = 'crates/tauron-adapter/src/tauri.rs';

  /** A 层：宿主跨进程 `emit` → webview `listen` 的事件，TS 镜像必须在 host-topics.ts。 */
  const TIER_A = [
    'NOTIFICATION_TOPIC',
    'MENU_CLICK_TOPIC',
    'DEEP_LINK_TOPIC',
    'DIALOG_DEGRADED_TOPIC',
    'DEEP_LINK_NATIVE_TOPIC',
  ];
  /** B 层：`host_events_*` 拉模型总线里的宿主所有 topic，TS 镜像在 app-plugin-sdk。 */
  const TIER_B = ['HOST_SETTINGS_CHANGED_TOPIC'];
  /** C 层：拼 topic 用的前/后缀片段，不是完整 topic（线值单点由 Rust 侧自己保证）。 */
  const TIER_C = ['CALL_TOPIC_PREFIX', 'CALL_TOPIC_SUFFIX', 'EVENT_TOPIC_PREFIX'];

  const relOf = (p: string): string => p.slice(workspaceRoot.length + 1).replace(/\\/g, '/');
  const flat = (s: string): string => s.replace(/\s+/g, '');
  const stripComments = (s: string): string =>
    s.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^[ \t]*\/\/.*$/gm, '');

  function tsSrcFiles(): string[] {
    const out: string[] = [];
    const walk = (dir: string): void => {
      for (const e of readdirSync(dir, { withFileTypes: true })) {
        const p = join(dir, e.name);
        if (e.isDirectory()) walk(p);
        else if (e.name.endsWith('.ts') && !e.name.includes('.test.')) out.push(p);
      }
    };
    for (const base of ['packages', 'examples']) {
      for (const pkg of readdirSync(join(workspaceRoot, base), { withFileTypes: true })) {
        if (!pkg.isDirectory()) continue;
        const src = join(workspaceRoot, base, pkg.name, 'src');
        if (existsSync(src)) walk(src);
      }
    }
    expect(out.length, '轮 38 解析到 0 个 TS 源文件（门禁定位失败）').toBeGreaterThan(10);
    return out;
  }

  function rustSrcFiles(): string[] {
    const out: string[] = [];
    const walk = (dir: string): void => {
      for (const e of readdirSync(dir, { withFileTypes: true })) {
        const p = join(dir, e.name);
        if (e.isDirectory()) walk(p);
        else if (e.name.endsWith('.rs')) out.push(p);
      }
    };
    for (const pkg of readdirSync(join(workspaceRoot, 'crates'), { withFileTypes: true })) {
      if (!pkg.isDirectory()) continue;
      const src = join(workspaceRoot, 'crates', pkg.name, 'src');
      if (existsSync(src)) walk(src);
    }
    expect(out.length, '轮 38 解析到 0 个 Rust 源文件（门禁定位失败）').toBeGreaterThan(5);
    return out;
  }

  /** Rust 侧 `pub const *_TOPIC*` 全集：名字 → 线值 + 声明文件（解析，不硬写）。 */
  function rustTopicConsts(): Map<string, { value: string; file: string }> {
    const m = new Map<string, { value: string; file: string }>();
    for (const p of rustSrcFiles()) {
      const s = readFileSync(p, 'utf8');
      for (const x of s.matchAll(/pub const ([A-Z_]*TOPIC[A-Z_]*)\s*:\s*&\s*str\s*=\s*"([^"]+)"/g))
        m.set(x[1]!, { value: x[2]!, file: relOf(p) });
    }
    return m;
  }

  /** TS 侧某个 `export const` 的值；文件里没有该声明时返回空串。 */
  function tsConstValue(name: string, file: string): string {
    const x = new RegExp(`export const ${name} = '([^']+)'`).exec(read(file));
    return x?.[1] ?? '';
  }

  it('① Rust 的 *_TOPIC 全集必须逐字落进三层表（新增一条不登记即红）', () => {
    const rust = rustTopicConsts();
    expect(
      [...rust.keys()].sort(),
      'Rust 侧 topic 常量集合与三层表不符（新增/删除都要同轮改表并补镜像）',
    ).toEqual([...TIER_A, ...TIER_B, ...TIER_C].sort());
    // C 层只能是片段：真出现完整 topic，说明有人把该进 A/B 层的通道塞进了豁免层。
    for (const name of TIER_C) {
      expect(rust.get(name)!.value, `${name} 看起来是一条完整 topic，不该待在片段层`).not.toMatch(
        /:\/\//,
      );
    }
  });

  it('② A 层五条：线值两侧逐字相等，且全部从 @tauron/host 入口导出', () => {
    const rust = rustTopicConsts();
    const index = flat(read(HOST_INDEX));
    // TS 镜像侧也必须是同一集合：只在一边加常量，等于把词表拆成两份各自演化。
    const tsMirrorNames = [...read(TOPICS).matchAll(/export const ([A-Z_]*TOPIC[A-Z_]*) = '/g)]
      .map((x) => x[1]!)
      .sort();
    expect(
      tsMirrorNames,
      'host-topics.ts 的镜像集合与 A 层表不符（两边各演化一份词表就是下一次漂移）',
    ).toEqual([...TIER_A].sort());
    for (const name of TIER_A) {
      const rv = rust.get(name)?.value ?? '';
      expect(rv, `Rust 侧解析不到 ${name}（正则失配）`).not.toBe('');
      const tv = tsConstValue(name, TOPICS);
      expect(tv, `host-topics.ts 没有镜像 ${name}`).not.toBe('');
      expect(tv, `${name} 的 TS 镜像与 Rust 线值漂移：监听会静默零命中而发出端仍一路 Ok`).toBe(rv);
      expect(
        new RegExp(`export\\{[^}]*\\b${name}\\b[^}]*\\}from'\\./host-topics\\.js'`).test(index),
        `${name} 没有从 @tauron/host 入口导出（接入方只能手打线值）`,
      ).toBe(true);
    }
  });

  it('③ 每条线值在 TS 非测试源里只许出现一次，且就落在它的声明文件', () => {
    const rust = rustTopicConsts();
    const files = tsSrcFiles();
    const all = files.map((p) => readFileSync(p, 'utf8')).join('\n');
    for (const [name, owner] of [
      ...TIER_A.map((n) => [n, TOPICS] as const),
      ...TIER_B.map((n) => [n, SDK_CREATE] as const),
    ]) {
      const wire = rust.get(name)!.value;
      const hits = files
        .filter((p) => readFileSync(p, 'utf8').includes(`'${wire}'`))
        .map(relOf)
        .sort();
      expect(hits, `${wire} 的字面量又有了第二份（下一次改名就是静默断链）`).toEqual([owner]);
      // 光数文件不够：同一个文件里再开一个别名常量，漂移面照样翻倍。
      const occurrences = all.split(`'${wire}'`).length - 1;
      expect(occurrences, `${wire} 在 TS 侧出现了 ${occurrences} 次：只许声明它那一次`).toBe(1);
    }
    expect(
      read(SDK_INDEX),
      'HOST_SETTINGS_CHANGED_TOPIC 不再从 @tauron/app-plugin-sdk 导出',
    ).toContain('HOST_SETTINGS_CHANGED_TOPIC');
  });

  it('④ 投递腿按词表常量监听：客户端里不许再有裸字面量', () => {
    const dl = read(DL_CLIENT);
    expect(dl, '深链接投递腿又回到裸字面量监听（宿主改名即静默零命中）').not.toContain(
      `'${tsConstValue('DEEP_LINK_TOPIC', TOPICS)}'`,
    );
    expect(dl, '投递腿没按 host-topics 的常量监听').toContain('listen(DEEP_LINK_TOPIC,');
    expect(
      /import \{ DEEP_LINK_TOPIC \} from '\.\/host-topics\.js'/.test(dl),
      '投递腿没从 host-topics.js 导入常量',
    ).toBe(true);
  });

  it('⑤ 监听方事实逐条冻结：三条真有接收方，两条诊断帧确实零监听', () => {
    // 零监听方是轮 36 写进文档的**事实**，不是缺陷遮掩：诊断帧的权威结论在命令返回值里。
    // 把它钉成断言，是为了"以后有人补了监听却忘了改口径"时这条会红，而不是口径悄悄过期。
    const expected: Record<string, string[]> = {
      NOTIFICATION_TOPIC: [SHELL_CTRL],
      MENU_CLICK_TOPIC: [EXAMPLE],
      DEEP_LINK_TOPIC: [DL_CLIENT],
      DIALOG_DEGRADED_TOPIC: [],
      DEEP_LINK_NATIVE_TOPIC: [],
    };
    const files = tsSrcFiles();
    for (const [name, want] of Object.entries(expected)) {
      const owners = files
        .filter((p) =>
          new RegExp(`\\.listen\\(\\s*${name}\\b`).test(stripComments(readFileSync(p, 'utf8'))),
        )
        .map(relOf)
        .sort();
      expect(owners, `${name} 的监听方集合变了（文档口径要同轮跟上）`).toEqual(want);
    }
  });

  it('⑥ 两侧口径同步：宿主清单与**每条镜像各自的文档块**都写明诊断帧零监听', () => {
    expect(read(TAURI), '宿主侧的逐 topic 监听方清单丢了"零监听方"这条事实').toContain(
      '仓库内零监听方',
    );
    // 盯各自的文档块：整文件级 needle 会被另一条常量的注释顶包（轮 38 第一次变异就栽在这）。
    // 也不能用 `/** … */ export const X` 的正则——非贪婪会从**上一条**常量的文档块起锚，
    // 把邻居的内容也算进来，照样顶包；这里按声明位置反向找最近的那个块。
    for (const name of ['DIALOG_DEGRADED_TOPIC', 'DEEP_LINK_NATIVE_TOPIC']) {
      const topics = read(TOPICS);
      const decl = topics.indexOf(`export const ${name} = '`);
      expect(decl, `host-topics.ts 里没有 ${name}`).toBeGreaterThan(-1);
      const doc = topics.slice(topics.lastIndexOf('/**', decl), decl);
      expect(doc, `${name} 自己的文档块没写明零监听`).toContain('零监听方');
      expect(doc, `${name} 的文档块没说清权威结论落在哪里`).toContain('返回值');
    }
  });
});

// 轮 39：能力可见性——「主窗特权命令对插件 webview 不可见」此前只在注释里成立。
//
// `isAvailable` 声称插件侧 `available('host_registry_admin')` 为 false，实现却只看
// `backend.capabilities()` 注册集；而 `host_capabilities`（Rust `cmd_host_capabilities`）
// 没有调用方参数、返回**构建级**命令集——插件 webview 也会拿到主窗命令的名字，矩阵
// 于是把 10 条主窗命令报成可见。原有"插件视图"测试又是 TS 表按 consumer 预筛的夹具，
// 断言是同义反复（过滤是测试做的，不是被测代码做的）。本段把两件事一起钉住：
// 实现按 `Backend.principal()` 过滤主体；测试用全量注册集走真实过滤路径。
// ──────────────────────────────────────────────────────────────────────────
describe('门禁：轮 39 能力可见性（主窗特权命令对插件主体不可见）', () => {
  const CAPS = 'packages/tauron-host/src/capabilities.ts';
  const CAPS_TEST = 'packages/tauron-host/src/capabilities.test.ts';
  const SHELL_CLIENT = 'packages/tauron-host/src/shell-client.ts';
  const HOST_README = 'packages/tauron-host/README.md';

  const relOf = (p: string): string => p.slice(workspaceRoot.length + 1).replace(/\\/g, '/');

  /** packages 与 examples 两棵树 src/ 下的非测试 TS 源。 */
  function appSrcFiles(): string[] {
    const out: string[] = [];
    const walk = (dir: string): void => {
      for (const e of readdirSync(dir, { withFileTypes: true })) {
        const p = join(dir, e.name);
        if (e.isDirectory()) walk(p);
        else if (e.name.endsWith('.ts') && !e.name.includes('.test.')) out.push(p);
      }
    };
    for (const base of ['packages', 'examples']) {
      for (const pkg of readdirSync(join(workspaceRoot, base), { withFileTypes: true })) {
        if (!pkg.isDirectory()) continue;
        const src = join(workspaceRoot, base, pkg.name, 'src');
        if (existsSync(src)) walk(src);
      }
    }
    expect(out.length, '轮 39 解析到 0 个 TS 源文件（门禁定位失败）').toBeGreaterThan(10);
    return out;
  }

  it('① isAvailable 双判据：注册 ∩ 主体可见性，且主窗判定是白名单', () => {
    const caps = read(CAPS);
    expect(
      /backend\.capabilities\(\)\.has\(command\)/.test(caps),
      'isAvailable 不再查宿主注册（协商真相），fail-closed 名存实亡',
    ).toBe(true);
    expect(
      /cap\.consumer !== 'main-window' \|\| backend\.principal\(\)\.kind === 'main-window'/.test(
        caps,
      ),
      'isAvailable 缺主体过滤（插件 webview 把主窗特权命令读成可用）；或白名单被改成黑名单（invalid 主体也放行）',
    ).toBe(true);
  });

  it('② 可见性只有一处：矩阵逐条走 isAvailable，非测试源没有第二份主体过滤', () => {
    const caps = read(CAPS);
    expect(
      /out\[cap\.command\] = isAvailable\(backend, cap\.command\)/.test(caps),
      'capabilityMatrix 不再复用 isAvailable（两处可见性判定会各自演化）',
    ).toBe(true);
    let inCaps = 0;
    const offenders: string[] = [];
    for (const p of appSrcFiles()) {
      const n = [...readFileSync(p, 'utf8').matchAll(/\.kind === 'main-window'/g)].length;
      const rel = relOf(p);
      if (rel === CAPS) inCaps = n;
      else if (n > 0) offenders.push(rel);
    }
    expect(inCaps, 'isAvailable 的主体判定不见了').toBe(1);
    expect(offenders, '第二处主体过滤：同一口径两个实现，下一次改动必分叉').toEqual([]);
  });

  it('③ 可见性测试不再循环取材：全量注册夹具 + 插件/畸形双主体 + 主窗对照', () => {
    const t = read(CAPS_TEST);
    // 插件用例的夹具必须是**构建级全量**（与 pluginId 同一个构造）：可见性必须由
    // 被测代码过滤；测试自己按 consumer 预筛 = 断言同义反复。
    expect(
      /capabilities: CAPABILITIES\.map\(\(c\) => c\.command\),\s*pluginId: 'com\.example\.x',/.test(
        t,
      ),
      '插件可见性用例的注册夹具不再是全量（循环取材会让过滤变成测试自己做的）',
    ).toBe(true);
    expect(
      /CAPABILITIES\.filter\(\(c\) => c\.consumer === 'plugin'\)\.map\(\(c\) => c\.command\)/.test(
        t,
      ),
      '旧循环夹具回来了：用 TS 表按 consumer 预筛的集合喂断言，测试与实现同错也绿',
    ).toBe(false);
    expect(t, '缺主窗对照（同一夹具 30 条全可见）：过滤可能把主窗主体一起误伤').toContain(
      '.every(Boolean)',
    );
    expect(t, '缺畸形主体用例：invalid 不得按主窗放行').toContain("kind: 'invalid'");
  });

  it('④ 两侧文档同口径：能力表注释写明构建级真相与过滤层，README 说清两个探测入口', () => {
    const caps = read(CAPS);
    expect(caps, 'isAvailable 注释丢了 host_capabilities 的构建级真相').toContain('构建级');
    expect(caps, 'isAvailable 注释没说清过滤发生在 SDK 层').toContain('可见性快照');
    expect(read(SHELL_CLIENT), 'supports() 注释没和 isAvailable 划清口径').toContain('注册与否');
    const readme = read(HOST_README);
    expect(readme, 'README 没有说明 supports 与 isAvailable 的口径差异').toContain('注册与否');
    expect(readme, 'README 没有给出插件主体下主窗命令报 false 的事实').toContain('一律报');
  });
});

describe('门禁：轮 61 孤儿棘轮的 TS 声明面口径（公开 const/type/interface 进账，取消信封被真用）', () => {
  const SCRIPT = 'scripts/check-orphan-public-api.mjs';
  const LEDGER = 'contracts/orphan-public-api.json';
  const CORE_BACKEND = 'packages/tauron-core/src/tauri-backend.ts';
  const CORE_TEST = 'packages/tauron-core/src/tauri-backend.test.ts';

  it('① B 口径确实枚举三种 TS 公开声明，判据不照抄 A，缺基线必须红', () => {
    const script = read(SCRIPT);
    const ledger = JSON.parse(read(LEDGER)) as {
      discoveryBaseline: number;
      discoveryBaselineDecl?: number;
    };
    // 三种声明各自入枚举（漏一种就等于把那一类对外面继续留在棘轮外）。
    expect(script, 'B 口径没枚举 export const').toContain('export const ([A-Za-z0-9_]+)');
    expect(script, 'B 口径没枚举 export type').toContain('export type ([A-Za-z0-9_]+)');
    expect(script, 'B 口径没枚举 export interface').toContain('export interface ([A-Za-z0-9_]+)');
    // 判据差别：声明文件正文算消费者（候选自身声明行除外），同文件其它声明行算引用位。
    expect(script, 'B 口径的声明面读者判据被搬空').toContain('hasDeclConsumer');
    expect(script, 'B 口径应当有保留声明行的视图，否则签名位类型别名会被误判成孤儿').toContain(
      'declViews',
    );
    // 落差由脚本自己打印，不靠注释口头声称。
    expect(script, '--discover 不再对比两种判据的落差，后来人会照抄 A 判据做出噪声门禁').toContain(
      '同一声明集若按 A 判据',
    );
    // 第二条棘轮独立存在且缺键即红；A 的枚举与上限不许被动过。
    expect(script, 'B 口径上限键名不再被脚本读取').toContain('discoveryBaselineDecl');
    expect(script, '缺 discoveryBaselineDecl 若不红，B 口径等于不设上限').toContain(
      "typeof declBaseline !== 'number'",
    );
    expect(script, 'A 口径的 pub fn 枚举被改动（两条棘轮不许互相借余量）').toContain(
      'pub fn ([a-z0-9_]+)',
    );
    expect(typeof ledger.discoveryBaselineDecl, '台账缺 B 口径上限').toBe('number');
    expect(ledger.discoveryBaseline, 'A 口径上限被顺手改动').toBe(632);
  });

  it('② 行视图的宽松失真已修：import 行不再充当消费者，未闭合判据才开启吞块', () => {
    const script = read(SCRIPT);
    expect(script, '跨行块的开启/终结判据丢了').toContain('blockTerminator');
    // 只有本行真的没闭合才开启块：`;` 与 `}` 两个守卫各管一支。
    expect(script, 'Rust use 语句不再要求本行无 ; ——单行 use 会被当成块起点').toContain(
      '&& !/[;]/.test(line)',
    );
    expect(script, 'TS import/export 不再要求花括号本行闭合').toContain('&& !/[}]/.test(line)');
    expect(script, 'TS 块起点应只由 import/export 的花括号或 * 触发').toContain(
      '(?:import|export)(?:\\s+type)?\\s*[*{]',
    );
  });

  it('③ plugin_cancel 的载荷由已发布类型承担：两处标注、裸字面量不回归、字段集 TS↔Rust 逐字段一致', () => {
    const core = read(CORE_BACKEND);
    expect(core, '取消调用点没再引用 @tauron/types 的取消信封').toContain(
      'type PluginCancelRequest,',
    );
    expect(core, 'CancelPayload 别名不再由已发布的取消信封类型描述').toMatch(
      /type\s+CancelPayload\s*=\s*\{\s*request:\s*PluginCancelRequest\s*\};/,
    );
    expect(core, '显式取消点的载荷丢了取消信封类型标注').toMatch(
      /const\s+request:\s*PluginCancelRequest\s*=\s*\{\s*callId\s*\};/,
    );
    expect(core, '超时取消点的载荷丢了取消信封类型标注').toMatch(
      /const\s+cancel:\s*PluginCancelRequest\s*=\s*\{\s*callId:\s*request\.callId\s*\};/,
    );
    expect(core, '取消调用不再按 payload 变量发出').toMatch(
      /invoke<\s*void\s*>\(\s*TAURON_COMMANDS\.cancel,\s*payload\s*\)/,
    );
    // 裸字面量回归＝类型与线上载荷重新变成零绑定，正是本轮补的空档。
    expect(core, '又出现未经类型标注的取消载荷字面量').not.toMatch(
      /TAURON_COMMANDS\.cancel,\s*\{\s*request:\s*\{/,
    );

    const rust = read(`${SHELL}/envelope.rs`);
    const typesEnvelope = read('packages/types/src/envelope.ts');
    const rustIdx = rust.search(/pub struct PluginCancelRequest\b/);
    expect(rustIdx, 'Rust 侧取消信封 struct 不见了').toBeGreaterThan(-1);
    // deny_unknown_fields 是「改名即整次调用失败」的前提，也是这条逐字段门禁的意义所在。
    expect(rust.slice(Math.max(0, rustIdx - 200), rustIdx), '取消信封不再拒绝未知字段').toContain(
      'deny_unknown_fields',
    );
    const rustBlock = /pub struct PluginCancelRequest\s*\{([\s\S]*?)\n\}/.exec(rust)?.[1] ?? '';
    const tsBlock =
      /export interface PluginCancelRequest\s*\{([\s\S]*?)\n\}/.exec(typesEnvelope)?.[1] ?? '';
    const rustFields = [...rustBlock.matchAll(/^\s+pub (\w+):/gm)]
      .map((x) => x[1]!.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase()))
      .sort();
    const tsFields = [...tsBlock.matchAll(/^\s*(\w+)\??:/gm)].map((x) => x[1]!).sort();
    expect(rustFields.length, 'Rust 字段解析到 0 个：门禁在自我豁免').toBeGreaterThan(0);
    expect(tsFields.length, 'TS 字段解析到 0 个：门禁在自我豁免').toBeGreaterThan(0);
    expect(tsFields, '取消信封字段集与宿主不一致（宿主 deny_unknown_fields）').toEqual(rustFields);
  });

  it('④ 台账与文档同口径：B 口径在册 4 条、取消信封成反例、被证伪的注释不得回归', () => {
    const ledger = JSON.parse(read(LEDGER)) as {
      orphans: { symbol: string; declaredIn: string; docRef: string; disposition: string }[];
      wiredWitnesses: { symbol: string; declaredIn: string; meaning: string }[];
    };
    const symbols = ledger.orphans.map((o) => o.symbol);
    for (const s of [
      'WindowOpRecorder::ops',
      'SIDECAR_ABI_CONTRACT',
      'PERMISSION_GRANULARITY',
      'TERMINAL_STATES',
    ]) {
      expect(symbols, `台账缺 B 口径在册条目 ${s}`).toContain(s);
    }
    for (const s of ['WindowOpRecorder::ops', 'SIDECAR_ABI_CONTRACT']) {
      const entry = ledger.orphans.find((o) => o.symbol === s)!;
      expect(entry.docRef, `${s} 没有可核对的文档落点`).not.toBe('');
    }
    const witness = ledger.wiredWitnesses.find((w) => w.symbol === 'PluginCancelRequest');
    expect(witness, '取消信封没被登记成已接线反例（B 口径就缺非空洞性证明）').toBeTruthy();
    expect(witness!.declaredIn, '取消信封反例的声明文件登记漂移').toBe(
      'packages/types/src/envelope.ts',
    );

    const acl = read('packages/types/src/acl.ts');
    expect(acl, '被证伪的「唯一有真实读者的工具」注释回来了').not.toContain(
      '内层词表里唯一有真实读者的工具',
    );
    expect(acl, '注释没交代 missingPermissions 根本不读那张表').toContain('本函数，不是那张表');
    expect(read('packages/types/src/plugin.ts'), 'TERMINAL_STATES 又装作线上终态口径').toContain(
      '不是线上的终态',
    );
    expect(
      read('packages/tauron-host/src/shell-client.ts'),
      'ABI 契约的「必须」又没了诚实边界',
    ).toContain('轮 61 的诚实边界');
    // 接口文档是插件作者唯一会读的那一份：必填语义与「仓内没有装配点」都得在上面。
    expect(
      read('docs/api/plugin-development-guide.md'),
      '接口文档没说清省略 abi 的真实后果与仓内无装配点',
    ).toContain('省略 `abi` 不是「跳过校验」');
    expect(
      read('docs/api/plugin-development-guide.md'),
      '接口文档没交代这条守卫缺端到端证据',
    ).toContain('没有任何 `runtimeSpawn` 生产');

    const coreTest = read(CORE_TEST);
    expect(coreTest, '缺取消载荷的键名行为测试（只靠类型标注挡不住运行期改名）').toContain(
      '轮 61：两处 plugin_cancel 的载荷键名逐字对上线上信封',
    );
    expect(coreTest, '行为测试不再逐字钉住取消载荷的键名').toMatch(
      /args:\s*\{\s*request:\s*\{\s*callId:\s*'call-1'\s*\}\s*\}/,
    );
    const readme = read('packages/tauron-core/README.md');
    expect(readme, 'README 没说清取消载荷的字段名后果').toContain('deny_unknown_fields');
    expect(readme, 'README 的导出清单缺「别按 README 推断仓库里跑通过」边界段').toContain(
      '已知边界（不要按 README 的导出名推断',
    );
    expect(read('docs/architecture/app-layer-wire.md'), '§6 权限分层没跟上轮 61 的改口').toContain(
      '轮 61 把剩下那半句假话也改口了',
    );
    expect(
      read('docs/architecture/v4-industrial-gap-closure-plan.md'),
      '缺口方案缺轮 61 小节',
    ).toContain('### 轮 61：孤儿棘轮看不见 TS 的声明面');
    expect(read('CHANGELOG.md'), 'CHANGELOG 缺轮 61 的孤儿棘轮条目').toContain(
      '孤儿棘轮第一次看见「TS 声明面」',
    );
  });
});

describe('门禁：轮 62 跨文件消费者视图（别的文件的声明行算引用、同名顶格声明不作证、读数钉双向）', () => {
  const SCRIPT = 'scripts/check-orphan-public-api.mjs';
  const LEDGER = 'contracts/orphan-public-api.json';
  const HOST_CLIENT = 'packages/tauron-host/src/shell-client.ts';
  const ADAPTER = 'crates/tauron-adapter/src/lib.rs';

  /** 取 TS interface 的字段名（跳过注释行）。 */
  function tsFields(text: string, name: string): string[] {
    const block = new RegExp(`export interface ${name}\\s*\\{([\\s\\S]*?)\\n\\}`).exec(text)?.[1];
    if (block === undefined) throw new Error(`找不到 TS 声明：export interface ${name}`);
    return [...block.matchAll(/^\s+([a-z][A-Za-z0-9]*)\s*[?]?:/gm)].map((m) => m[1]!).sort();
  }

  /** 取 Rust 手工 json! 线形的键名（限定在某个函数体内，避免抓到别的 json!）。 */
  function rustJsonKeys(text: string, fnName: string, valuePrefix: string): string[] {
    const start = text.indexOf(`fn ${fnName}`);
    if (start < 0) throw new Error(`找不到 Rust 函数：${fnName}`);
    const region = text.slice(start, start + 2400);
    return [...region.matchAll(new RegExp(`"([a-z][A-Za-z0-9]*)":\\s*${valuePrefix}\\.`, 'g'))]
      .map((m) => m[1]!)
      .sort();
  }

  it('① 两条视图修正都在线上，且各自钉住方向（一松一紧，缺一条就是另一种错）', () => {
    const script = read(SCRIPT);
    expect(script, '跨文件声明行不再算引用位（B 口径的 5 条假孤儿会回来）').toContain(
      'function otherView(',
    );
    expect(script, 'A 口径不再走修正后的消费者视图').toContain('otherView(g, [name])');
    expect(script, '台账 probe 不再走修正后的消费者视图').toContain('otherView(f, names)');
    // 紧的那一条：同名顶格声明的文件整份不作证（PublishResult / WindowState 那类跨包同名类型）。
    expect(script, '同名顶格声明守卫被搬空（跨包同名类型会互相作证）').toContain(
      'declaresSame(rel, n)',
    );
    // 松的那一条只给 TS 用；Rust 一旦跟着放开，跨 crate 同名双胞胎就改成互相作证（实测 9 条）。
    expect(
      script,
      'Rust 侧消费者口径被顺手放开（会让 is_durable/queue_stats 双胞胎互相作证）',
    ).toMatch(/^  if \(rel\.endsWith\('\.rs'\)\) return views\.get\(rel\);$/m);
    // 同名判据只认顶格：带缩进的 impl 方法名不该让整份文件失去作证资格。
    const declBlock = /const DECL_NAME_RES = \{[\s\S]*?\n\};/.exec(script);
    expect(declBlock, '同名判据表被搬走').not.toBeNull();
    expect(declBlock![0], 'Rust 同名判据不再限定顶格声明').toContain('rust: /^(?:pub');
    expect(declBlock![0], 'TS 同名判据不再限定顶格声明').toContain('ts: /^(?:export');
    // 多符号台账条目：整串当名字等于没有守卫（dialog-client.ts 的定义行会替「已接线」作证）。
    expect(script, '多符号台账条目的同名守卫退化成整串匹配').toContain(
      'split(/\\s*\\/\\s*|\\s*::\\s*/)',
    );
  });

  it('② 等值读数钉：缺键即红、涨跌都要显式改账（上限棘轮对「口径被换掉」是瞎的）', () => {
    const script = read(SCRIPT);
    const ledger = JSON.parse(read(LEDGER)) as {
      discoveryBaseline: number;
      discoveryBaselineDecl: number;
      discoveryObserved?: { a?: number; decl?: number };
    };
    expect(script, '等值读数钉被搬空（口径被换掉时门禁没有失败出口）').toContain(
      'discoveryObserved',
    );
    expect(script, '缺 discoveryObserved 若不红，视图判据就永远证不了').toContain(
      "typeof observed[key] !== 'number'",
    );
    expect(script, '读数钉若只盯涨不盯跌，把孤儿接回去也能绿').toContain('observed[key] !== count');
    expect(script, '读数钉漏了 B 口径').toContain("'decl', declCandidates.length");
    expect(ledger.discoveryObserved?.a, '台账缺 A 口径读数').toBe(632);
    expect(ledger.discoveryObserved?.decl, '台账缺 B 口径读数').toBe(39);
    expect(
      ledger.discoveryObserved!.a,
      'A 口径读数越过上限（读数钉与棘轮打架）',
    ).toBeLessThanOrEqual(ledger.discoveryBaseline);
    expect(
      ledger.discoveryObserved!.decl,
      'B 口径读数越过上限（读数钉与棘轮打架）',
    ).toBeLessThanOrEqual(ledger.discoveryBaselineDecl);
  });

  it('③ B 口径改判的 5 条假孤儿：引用行必须还在线上，不许靠删代码凑数', () => {
    expect(
      read('packages/tauron-host/src/memory-transport.ts'),
      'Transport 实现行的引用位没了',
    ).toMatch(/^export class MemoryTransport implements HostTransport \{$/m);
    expect(read('packages/tauron-market/src/sign.ts'), 'KeyPair 的返回类型标注没了').toMatch(
      /^export function generateKeyPair\(\): KeyPair \{$/m,
    );
    expect(
      read('packages/tauron-shell-matrix/src/manager.ts'),
      'ShellManager 的签名引用没了',
    ).toMatch(
      /^export function createShellManager\(options: ShellManagerOptions = \{\}\): ShellManager \{$/m,
    );
  });

  it('④ 被摘掉的假绿：跨包同名声明必须还在（守卫的前提不成立时，这条要一起改）', () => {
    const pairs = [
      ['packages/tauron-ui-primitives/src/title-bar.ts', /^export type WindowState = /m],
      ['packages/tauron-ui/src/plugin-manager.ts', /^function normalizeError\(/m],
      ['packages/tauron-app-cli/src/logger.ts', /^export type LogLevel = /m],
      ['packages/tauron-app-cli/src/pack.ts', /^export interface PublishResult \{$/m],
    ] as const;
    for (const [file, re] of pairs) {
      expect(read(file), `${file} 的顶格同名声明没了——同名守卫的依据需要重新核对`).toMatch(re);
    }
    // 台账里那条多符号 orphans 依然成立（工厂函数在接线面确实没人调）。
    const ledger = JSON.parse(read(LEDGER)) as { orphans: { symbol: string }[] };
    const entry = ledger.orphans.find((o) => o.symbol.startsWith('createAutoUpdateClient'));
    expect(entry, '多符号工厂条目被误删（它至今仍是零消费者的对外宣称）').toBeDefined();
    expect(read('packages/tauron-host/src/dialog-client.ts'), '工厂函数的定义行形态变了').toMatch(
      /^export function createDialogClient\(/m,
    );
  });

  it('⑤ 通知读取端线形：TS NotifyItem / DispatchRecord 与宿主 json! 的键逐字对齐', () => {
    const client = read(HOST_CLIENT);
    const rust = read(ADAPTER);
    expect(tsFields(client, 'NotifyItem'), '通知条目键集与宿主手工 json! 不再逐字段一致').toEqual(
      rustJsonKeys(rust, 'notifications_list_payload', 'e'),
    );
    expect(tsFields(client, 'DispatchRecord'), '分发日志键集与宿主 json! 不再逐字段一致').toEqual(
      rustJsonKeys(rust, 'notifications_list_payload', 'r'),
    );
    // 三份宣称都得有出处：TS 侧写「与 Rust NotifyEntry 逐字段 camelCase 映射」，Rust 侧写「与 TS 契约一致」。
    expect(client, 'TS 侧的映射对象名字漂了（注释钉的是 NotifyEntry）').toContain(
      '与 Rust `NotifyEntry` 逐字段 camelCase 映射',
    );
    expect(rust, 'Rust 侧不再声称与 TS 契约一致').toContain('与 TS 契约一致');
    // 兼容日志那份：不在线上，但两侧都宣称字段名对齐——改名会静默读出 undefined。
    expect(tsFields(client, 'NotificationRecord'), '兼容记录两侧字段不再一致').toEqual(
      (() => {
        const block = /pub struct NotificationRecord \{([\s\S]*?)\n\}/.exec(rust)?.[1] ?? '';
        return [...block.matchAll(/pub ([a-z_]+):/g)]
          .map((m) => m[1]!.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase()))
          .sort();
      })(),
    );
    expect(rust, '兼容记录的 camelCase 线形注解被摘掉（TS 侧的字段名宣称就失去依据）').toContain(
      'pub struct NotificationRecord',
    );
  });

  it('⑥ 本轮记录与文档同步', () => {
    expect(
      read('docs/architecture/v4-industrial-gap-closure-plan.md'),
      '缺口方案缺轮 62 小节',
    ).toContain('### 轮 62：跨文件消费者视图看不见声明行，同名声明却在互相作证');
    expect(read('CHANGELOG.md'), 'CHANGELOG 缺轮 62 条目').toContain('「谁替它作证」第一次被当真');
    const note = JSON.parse(read(LEDGER)) as { note: string[] };
    expect(note.note.join('\n'), '台账注释没记下 Rust 面为什么不动口径').toContain(
      'Rust 面本轮**不动口径**',
    );
  });
});

describe('门禁：轮 63 窗口几何持久化链路（委派出去的那条腿要有装配点，读侧失败不许静默）', () => {
  const CTRL = 'packages/tauron-host/src/shell-controller.ts';
  const WST = 'packages/tauron-host/src/window-state.ts';
  const CTRL_TEST = 'packages/tauron-host/src/shell-controller.test.ts';
  const WST_TEST = 'packages/tauron-host/src/window-state.test.ts';
  const ADAPTER = 'crates/tauron-adapter/src/lib.rs';

  it('① 委派对象必须真的被装配：构造 WindowState，start() 接两条腿，stop() 升代际', () => {
    const ctrl = read(CTRL);
    expect(ctrl, '没有 import 已发布的 WindowState（委派链又回到零装配点）').toMatch(
      /^import \{ WindowState \} from '\.\/window-state\.js';$/m,
    );
    expect(ctrl, '几何腿又变成无条件启动（没有可用存储时每次 resize 刷一次报错）').toContain(
      'this._geometry = hasUsableStorage() ? new WindowState() : null;',
    );
    expect(ctrl, '存储探测被搬空（腿会跑在假存储上）').toContain('typeof storage.setItem');
    expect(ctrl, '恢复腿不再挂 start()（在构造函数里发命令是另一件事）').toContain(
      'void this._restoreGeometry();',
    );
    expect(ctrl, '落盘腿丢了 resize 触发（用户拉窗口不再记账）').toContain(
      "'resize', () => this._saveGeometry()",
    );
    expect(ctrl, '落盘腿丢了 pagehide 触发（退出前最后一帧不落盘）').toContain(
      "'pagehide', () => this._saveGeometry()",
    );
    expect(ctrl, 'stop() 不再升代际（在途恢复会继续动平台）').toContain(
      'this._geometryGeneration++',
    );
    expect(ctrl, '守卫 1 被搬空（无存档也去 apply＝每次启动把窗口摁回默认尺寸）').toContain(
      '!geometry.hasSavedState',
    );
    expect(ctrl, '守卫 2 被搬空（越界存档照落平台，外接屏拔掉后窗口看不见）').toContain(
      'geometry.isValid(state, screenWidth, screenHeight)',
    );
    expect(ctrl, '守卫 3 被搬空（stop 之后剩余回写继续发）').toContain(
      'generation === this._geometryGeneration',
    );
    expect(ctrl, '恢复失败重新静默').toContain("'window.geometry.restore'");
    expect(ctrl, '落盘失败重新静默').toContain("'window.geometry.save'");
  });

  it('② 存档读侧一律留痕，apply 只需要 invoke；两条回写命令都必须在已注册面里', () => {
    const ws = read(WST);
    expect(ws, '读侧又回到整段吞掉的 catch {}').not.toMatch(/catch\s*\{\s*\}/);
    expect(ws, '坏 JSON 不再留痕').toMatch(
      /JSON\.parse\(raw\);\s*\}\s*catch\s*\(err\)\s*\{\s*this\._fail\(err\);/,
    );
    expect(ws, '非对象存档又被当成存档').toContain('存档内容不是对象');
    expect(ws, 'clear() 不复位存在性（下一次恢复拿着假事实动窗口）').toMatch(
      /this\._hasSavedState = false;\s*return ok;/,
    );
    expect(ws, 'save() 成功不登记存在性').toMatch(
      /setItem\(this\._config\.storageKey, JSON\.stringify\(this\._currentState\)\);\s*this\._lastError = null;\s*this\._hasSavedState = true;/,
    );
    expect(ws, '读侧不再把「真读到存档」与「落盘成功」分开登记').toMatch(
      /this\._hasSavedState = true;\s*return \{\s*x:/,
    );
    expect(ws, 'apply 的参数又收回到整份 Backend（守卫只能靠伪造传输层）').toContain(
      "async apply(backend: Pick<Backend, 'invoke'>): Promise<boolean>",
    );
    const list = read('packages/tauron-host/src/tauri-backend.ts');
    for (const cmd of [
      'host_window_set_position',
      'host_window_set_size',
      'host_window_maximize',
    ]) {
      expect(list, `${cmd} 不在已注册命令清单里，恢复腿会当场 command not found`).toContain(
        `'${cmd}'`,
      );
    }
  });

  it('③ 宿主的委派注释必须点到真实装配者，也不许反过来说账本有读取方', () => {
    const rs = read(ADAPTER);
    expect(rs, '注释又只说"在前端"而不交代装配者（下一轮会再次当成已接线）').toContain(
      '这条前端腿由 `ShellController.start()` 装配',
    );
    expect(rs, '「没有任何生产读取方」这句真话被抹掉（账本仍是只写不读）').toContain(
      '**接入状态：今天没有任何生产读取方**',
    );
  });

  it('④ 三条守卫与两条失败出口各有行为用例，键名逐字钉住线上信封', () => {
    const t = read(CTRL_TEST);
    const cases = [
      '没有存档时一条几何命令都不发（默认值长得像结论，但不是事实）',
      '有存档且在屏内：先位置后尺寸，两条命令的键名逐字对上线上信封',
      '越界存档不落平台，但必须留痕且不删存档（坏值不拒启）',
      'stop() 之后剩余回写不再发出（两段式恢复不能只完成前一半）',
      'resize 与 pagehide 各落一次盘（落的是 webview 可见的当前几何）',
      '环境给不出可用存储时整条腿不启动（不恢复，也不把环境问题刷成用户报错）',
      '写盘失败不静默，也不打断用户操作',
    ];
    for (const name of cases) {
      expect(t, `缺行为用例：${name}`).toContain(`it('${name}'`);
    }
    expect(t, '位置命令的键名断言松掉').toContain(
      "{ cmd: 'host_window_set_position', args: { x: 20, y: 30 } }",
    );
    expect(t, '尺寸命令的键名断言松掉').toContain(
      "{ cmd: 'host_window_set_size', args: { width: 900, height: 600 } }",
    );
    const wt = read(WST_TEST);
    for (const name of [
      '坏 JSON 回落默认值，但把原因记下来',
      '存档不是对象（例如一个裸数字）：留痕且不认作存档',
      '只有读到合法存档才算 hasSavedState；save 置真、clear 置假',
      'apply() 只需要 invoke：带守卫的窄后端即可（恢复腿据此在停止后拒发）',
    ]) {
      expect(wt, `缺读侧用例：${name}`).toContain(name);
    }
  });

  it('⑤ 本轮记录、台账与上限同口径（余量真话不许留在纸上）', () => {
    expect(
      read('docs/architecture/v4-industrial-gap-closure-plan.md'),
      '缺口方案缺轮 63 小节',
    ).toContain('### 轮 63：宿主把窗口几何的持久化委派给前端，而前端没有任何装配点');
    expect(read('CHANGELOG.md'), 'CHANGELOG 缺轮 63 条目').toContain(
      '委派出去的那条腿第一次有了装配点',
    );
    const ledger = JSON.parse(read('contracts/orphan-public-api.json')) as {
      discoveryBaseline: number;
      discoveryObserved?: { a?: number };
      note: string[];
    };
    expect(
      ledger.discoveryObserved?.a,
      '台账读数没跟上最新收口（改过孤儿计数就要同步到这里）',
    ).toBe(632);
    expect(ledger.discoveryBaseline, '上限没收口到实测（余量假象回来了）').toBe(632);
    const note = ledger.note.join('\n');
    expect(note, '台账没记下 12 条候选的逐条定性').toContain('12 条 A 候选已逐条定性');
    expect(note, '真断链那一条被糊进其余 11 条里').toContain('①真断链 1 条');
    expect(note, '其余 11 条的「破坏性等批准」口径被删').toContain('②其余 11 条');
  });
});

describe('门禁：轮 64 总线预算的两份镜像必须同源（TS 已发布数字 ↔ Rust 强制点）', () => {
  const BUS = 'crates/tauron-host/src/eventbus.rs';
  const CH = 'packages/tauron-host/src/channels.ts';
  const EV = 'packages/tauron-host/src/events.ts';

  const rustBudget = (src: string, name: string): number => {
    // 注意双反斜杠：模板字面量里的 `\d` 会先被 JS 解析成 `d`，针就永远匹配不上。
    const m = src.match(new RegExp(`pub const ${name}: usize = (\\d+);`));
    if (!m) throw new Error(`Rust 侧读不到 pub const ${name}: usize = N;`);
    return Number(m[1]);
  };
  const tsBudget = (src: string, name: string): number => {
    const m = src.match(new RegExp(`export const ${name} = (\\d+);`));
    if (!m) throw new Error(`TS 侧读不到 export const ${name} = N;`);
    return Number(m[1]);
  };
  const declareBody = (src: string): string => {
    const start = src.indexOf('pub fn declare_topics');
    const end = src.indexOf('pub fn topic_meta');
    if (start < 0 || end < 0 || end <= start) throw new Error('declare_topics 函数体切不出来');
    return src.slice(start, end);
  };
  /** 全文件出现次数（含注释）——B 口径把注释也算读者，自名只能出现一次。 */
  const nameHits = (src: string, name: string): number => src.split(name).length - 1;

  it('① 两份数字两两相等，且 Rust 侧是真强制点而不是第二个声明', () => {
    const bus = read(BUS);
    for (const [name, file] of [
      ['MAX_QUEUE', CH],
      ['OVERFLOW_STREAK_LIMIT', CH],
    ] as const) {
      expect(rustBudget(bus, name), `Rust ${name} 与已发布镜像不再同值`).toBe(
        tsBudget(read(file), name),
      );
    }
    expect(rustBudget(bus, 'MAX_TOPIC_NAME_LENGTH'), 'Rust 名上限与 TOPIC_MAX_LENGTH 漂移').toBe(
      tsBudget(read(EV), 'TOPIC_MAX_LENGTH'),
    );
    // 强制点必须存在于真实判定里（只在 `pub const` 行出现＝又回到纸面约定）。
    expect(bus, '名上限没有真实比较点（只剩声明＝约定又是纸面的）').toMatch(
      /if chars > MAX_TOPIC_NAME_LENGTH \{/,
    );
    // 三处判定各在自己的路径上（字节预算、回压、入队），presence 针会被任意一处满足，
    // 所以这里数次数：削掉任意一处都必须显式改账。
    expect(
      nameHits(bus, 'self.frames.len() >= self.capacity'),
      '队列上限的强制点少了判定处（镜像与真源脱钩：三处判定缺一不可）',
    ).toBeGreaterThanOrEqual(3);
  });

  it('② 三段校验都在写入之前——被拒批次一条声明都不留', () => {
    const body = declareBody(read(BUS));
    const insert = body.indexOf('t.insert(');
    expect(
      insert,
      'declare_topics 里找不到写入点（结构变了，针该跟着改而不是盲改）',
    ).toBeGreaterThan(-1);
    for (const [label, guard] of [
      ['空名', 'd.topic.trim().is_empty()'],
      ['超长名', 'chars > MAX_TOPIC_NAME_LENGTH'],
      ['归属冲突', 't.get(&d.topic)'],
    ] as const) {
      const at = body.indexOf(guard);
      expect(at, `校验「${label}」不在函数体里`).toBeGreaterThan(-1);
      expect(at < insert, `校验「${label}」排在写入之后＝部分声明回来了`).toBe(true);
    }
    expect(body, '归属冲突校验的报错文案缺席（校验还在但不再说明理由）').toContain('不可重复声明');
  });

  it('③ 空名/边界/整批原子三条各有行为用例，边界按「正好等于上限」取值', () => {
    const bus = read(BUS);
    for (const name of [
      'blank_topic_name_is_rejected_at_declare_time',
      'topic_name_boundary_takes_exactly_the_published_budget',
      'a_rejected_batch_leaves_no_partially_declared_topic',
    ]) {
      expect(bus, `缺 Rust 用例：${name}`).toContain(`fn ${name}()`);
    }
    expect(
      bus,
      '边界用例不再按 MAX_TOPIC_NAME_LENGTH 取值（硬编码数字＝用例自己也会漂）',
    ).toContain('"t".repeat(MAX_TOPIC_NAME_LENGTH)');
    expect(bus, '超长用例不再走 +1 边界').toContain('"x".repeat(MAX_TOPIC_NAME_LENGTH + 1)');
    expect(bus, '被拒批次的部分声明不再被断言拦住').toContain(
      'b.topic_meta("com.a.first").is_none()',
    );
  });

  it('④ TS 镜像注释要点到强制者，且不许用自身名字喂孤儿棘轮', () => {
    const ch = read(CH);
    expect(ch, '队列镜像又不说强制点在哪（下一轮会再当成已接线）').toContain(
      '强制点在 Rust `tauron-host::eventbus` 的同名预算常量上',
    );
    // B 口径把注释文本也算读者：注释里一旦出现自己的名字，候选数就被假降一次。
    // 所以「出现次数恰好＝声明行那一次」才是真针，负向正则只是它的一个子集。
    for (const [name, hits] of [
      ['MAX_QUEUE', nameHits(ch, 'MAX_QUEUE')],
      ['OVERFLOW_STREAK_LIMIT', nameHits(ch, 'OVERFLOW_STREAK_LIMIT')],
      ['TOPIC_MAX_LENGTH', nameHits(read(EV), 'TOPIC_MAX_LENGTH')],
    ] as const) {
      expect(hits, `${name} 在声明之外又被提到（注释自名＝假读者，棘轮读数会假降）`).toBe(1);
    }
    expect(read(EV), '名上限约定没写真实强制者').toContain('MAX_TOPIC_NAME_LENGTH');
  });

  it('⑤ 本轮记录与台账同口径（39 条 B 候选的分诊要有名有姓）', () => {
    expect(
      read('docs/architecture/v4-industrial-gap-closure-plan.md'),
      '缺口方案缺轮 64 小节',
    ).toContain('### 轮 64：已发布的总线预算第一次和真源对上');
    expect(read('CHANGELOG.md'), 'CHANGELOG 缺轮 64 条目').toContain(
      '已发布的数字第一次有了强制点',
    );
    const ledger = JSON.parse(read('contracts/orphan-public-api.json')) as {
      note: string[];
      discoveryObserved: { decl: number };
      discoveryBaselineDecl: number;
    };
    const note = ledger.note.join('\n');
    expect(note, '台账没记 B 候选的分诊口径').toContain('39 条 B 候选逐条分诊');
    expect(note, '注释能喂饱棘轮这件事被埋掉').toContain('B 口径把注释也算读者');
    // 单向棘轮只在候选变多时红：把上限抬回 40 也能绿，所以「不留余量」要等值钉。
    expect(
      ledger.discoveryBaselineDecl,
      'B 上限没收口到实测（余量假象回来了，新增公开面就能静默过关）',
    ).toBe(ledger.discoveryObserved.decl);
  });
});

describe('门禁：轮 65 打包端预算必须停在宿主的拒收线上（@tauron/app-cli ↔ tauron-market）', () => {
  // 打包端与安装端读的是同一个包：宿主 `validate_zip_constants` 超线即拒收，
  // 所以打包端的预算**只能等于**它，不能更大——更大只是把失败从打包当场挪到装不上。
  const MARKET = 'crates/tauron-market/src/lib.rs';
  const SIG = 'crates/tauron-market/src/package_signature.rs';
  const PACK = 'packages/tauron-app-cli/src/pack.ts';
  const PLAN = 'docs/architecture/v4-industrial-gap-closure-plan.md';

  /** 读 Rust 侧的整数预算常量；读不到就抛，针失效不许静默变绿。 */
  const rustNum = (src: string, name: string): number => {
    const m = src.match(new RegExp(`pub const ${name}: (?:usize|u64) = (\\d+);`));
    if (!m) throw new Error(`Rust 侧读不到 pub const ${name} = N;`);
    return Number(m[1]);
  };
  /** 读 TS 侧的预算常量，容忍 `N` 与 `N * 1024 * 1024` 两种写法，统一返回字节数。 */
  const tsBytes = (src: string, name: string): number => {
    const m = src.match(new RegExp(`export const ${name} = (\\d+)( \\* 1024 \\* 1024)?;`));
    if (!m) throw new Error(`TS 侧读不到 export const ${name} = N;`);
    return m[2] ? Number(m[1]) * 1024 * 1024 : Number(m[1]);
  };
  const hits = (src: string, needle: string): number => src.split(needle).length - 1;

  it('① 三条预算同值：条目数、解包总大小、单文件大小', () => {
    const market = read(MARKET);
    const pack = read(PACK);
    expect(tsBytes(pack, 'MAX_FILE_COUNT'), '条目数预算与宿主脱钩').toBe(
      rustNum(market, 'MAX_ENTRIES'),
    );
    expect(tsBytes(pack, 'MAX_TOTAL_SIZE'), '解包总预算与宿主脱钩（打包端曾写 512MB）').toBe(
      rustNum(market, 'MAX_UNPACKED_MB') * 1024 * 1024,
    );
    expect(tsBytes(pack, 'MAX_FILE_SIZE'), '单文件预算与宿主脱钩').toBe(
      rustNum(market, 'MAX_SINGLE_FILE_MB') * 1024 * 1024,
    );
    // 自我豁免检查：值针解析不到数字会抛，但解析成功而两侧都是 0 仍然能绿。
    expect(rustNum(market, 'MAX_UNPACKED_MB'), '宿主预算被读成 0：值针在自我豁免').toBeGreaterThan(
      0,
    );
  });

  it('② 边界判据同向：同值不等于同判据（差一正是这轮的另一半）', () => {
    const market = read(MARKET);
    const pack = read(PACK);
    expect(market, '宿主的条目数判据改了，打包端对不上').toContain(
      'if entries.len() > MAX_ENTRIES {',
    );
    expect(market, '宿主的总大小判据改了').toContain(
      'if total_uncompressed > MAX_UNPACKED_MB * 1024 * 1024 {',
    );
    expect(market, '宿主按向下取整到 MiB 后 ≥ 拒单文件，判据方向不许反').toContain(
      'if size_mb >= MAX_SINGLE_FILE_MB {',
    );
    // 总大小有两处判定：打包腿（validateFiles）与读包腿（readPluginArchive 的累计）。
    // presence 针分不清处数，改计数针。
    expect(
      hits(pack, 'if (totalSize > MAX_TOTAL_SIZE) throw new Error('),
      '总大小的两处判定被搬走一处（打包腿与读包腿缺一不可）',
    ).toBe(2);
    expect(
      hits(pack, 'if (file.size >= MAX_FILE_SIZE) {'),
      '单文件判据又退回严格大于（恰好 100 MiB 的包能打出去、装不上）',
    ).toBe(1);
    expect(pack, '被证伪的旧判据还留在别处').not.toContain('if (file.size > MAX_FILE_SIZE)');
  });

  it('③ 签名腿取的是同一份常量，不另抄数字', () => {
    const sig = read(SIG);
    // 「名字还在判定行」不等于「账还从宿主来」：把名字从 import 删掉，Rust 编译会失败，
    // 但门禁只读文本——presence 针与计数针都照样绿（轮 65 变异 S4 连抓两次：一次是
    // presence 针被判定行喂饱，一次是计数针低估了判定行的处数）。所以钉 import 清单本身。
    expect(sig, '签名腿的 import 清单少了某条预算名（判定行还在，账却不再同源）').toMatch(
      /MAX_ENTRIES,\s*MAX_SINGLE_FILE_MB,\s*MAX_UNPACKED_MB,/,
    );
    expect(sig, '签名腿的条目数判据与宿主脱钩').toContain('zip.len() > MAX_ENTRIES');
    expect(sig, '签名腿的总大小判据与宿主脱钩').toContain(
      'if total_unpacked > MAX_UNPACKED_MB * 1024 * 1024 {',
    );
    expect(sig, '签名腿的单文件判据与宿主脱钩').toContain(
      'if file.size() >= MAX_SINGLE_FILE_MB * 1024 * 1024 {',
    );
  });

  it('④ 镜像注释交代真源与被修掉的漂移', () => {
    const pack = read(PACK);
    expect(pack, '打包端预算没交代它的真源').toContain(
      '`MAX_SINGLE_FILE_MB` / `MAX_ENTRIES` / `MAX_UNPACKED_MB`',
    );
    expect(pack, '512MB→200MB 这段被抹平，下一个人会把 200 当成写错').toContain('轮 65 修掉的漂移');
  });

  it('⑤ 本轮记录与对外口径同向（200MB 是竞品分析里写死的宿主承诺）', () => {
    expect(read(PLAN), '缺口方案缺轮 65 小节').toContain(
      '### 轮 65：打包端第一次停在宿主的拒收线上',
    );
    expect(read('CHANGELOG.md'), 'CHANGELOG 缺轮 65 条目').toContain('打包端放行过注定装不上的包');
    expect(
      read('docs/competitive-analysis/competitive-analysis.md'),
      '宿主的对外预算口径漂了',
    ).toContain('解压≤200MB');
  });

  it('⑥ 行为用例在册：两条边界各有真跑过的用例，不只靠值针', () => {
    const t = read('packages/tauron-app-cli/src/pack.test.ts');
    expect(t, '缺行为用例：恰好 100 MiB 的单文件也拒').toContain(
      "it('恰好 100 MiB 的单文件也拒（与宿主的 ≥ 判据对齐，不留差一）'",
    );
    expect(t, '缺行为用例：解包总预算就是宿主的 200 MiB').toContain(
      "it('解包总预算就是宿主的 200 MiB（卡线通过，超 1 字节当场拒）'",
    );
    // 打包端两处预算的调用者都在：值针只证明数字对，不证明这两个函数还在校验。
    expect(t, '打包腿的用例被搬走，值针就成了无读者的空账').toContain("describe('validateFiles'");
    expect(read(PACK), '读包腿不再逐条走同一份校验（100 MiB 的包又能读过去）').toContain(
      'validateFiles([{ path: name, size, hash:',
    );
  });
});

describe('门禁：轮 66 流的三笔额度账必须与宿主同源（@tauron/host ↔ tauron-host::stream）', () => {
  // 额度是接收方驱动的：宿主只保证「超过余额即拒、补给封顶」。这份内核把同样
  // 三条算术又写了一遍，所以每一笔都要能对回真源——轮 65 的教训是同值的数字
  // 不等于同值的判据，也不等于这条判据真的被用例行使过。
  const STREAM_RS = 'crates/tauron-host/src/stream.rs';
  const ADMISSION_RS = 'crates/tauron-host/src/admission.rs';
  const REGISTRY_RS = 'crates/tauron-host/src/registry.rs';
  const ADAPTER = 'crates/tauron-adapter/src/lib.rs';
  const TAURI = 'crates/tauron-adapter/src/tauri.rs';
  const STREAM_TS = 'packages/tauron-host/src/stream.ts';
  const BACKEND_TS = 'packages/tauron-host/src/backend.ts';
  const HOST_TS = 'packages/tauron-host/src/host.ts';
  const TEST_TS = 'packages/tauron-host/src/stream.test.ts';
  const PLAN = 'docs/architecture/v4-industrial-gap-closure-plan.md';

  /** 只认 `N` 与 `N * 1024 [* 1024]` 两种写法；读不懂的算式当场抛，不许静默变绿。 */
  const product = (expr: string, where: string): number => {
    const parts = expr.trim().split(/\s*\*\s*/);
    if (!parts.every((part) => /^\d+$/.test(part)))
      throw new Error(`读不懂的算式 \`${expr}\`（${where}）`);
    return parts.reduce((acc, part) => acc * Number(part), 1);
  };
  const rustNum = (src: string, name: string): number => {
    const expr = src.match(new RegExp(`pub const ${name}: usize = ([^;]+);`))?.[1];
    if (expr === undefined)
      throw new Error(`Rust 侧读不到 pub const ${name}: usize = …;（被改名或换了类型）`);
    return product(expr, name);
  };
  const tsNum = (src: string, name: string): number => {
    const expr = src.match(new RegExp(`(?:export )?const ${name} = ([^;]+);`))?.[1];
    if (expr === undefined)
      throw new Error(`TS 侧读不到 const ${name} = …;（被改名，或降回裸字面量）`);
    return product(expr, name);
  };
  const hits = (src: string, needle: string): number => src.split(needle).length - 1;

  it('① 三笔账同值：初始额度、补额封顶、帧开销（附「读成 0 即自我豁免」自检）', () => {
    const rs = read(STREAM_RS);
    const ts = read(STREAM_TS);
    const backend = read(BACKEND_TS);
    expect(tsNum(ts, 'DEFAULT_STREAM_CREDIT_BYTES'), '初始额度镜像与宿主脱钩').toBe(
      rustNum(rs, 'DEFAULT_STREAM_CREDIT_BYTES'),
    );
    expect(tsNum(ts, 'MAX_STREAM_CREDIT_BYTES'), '补额封顶镜像与宿主脱钩').toBe(
      rustNum(rs, 'MAX_STREAM_CREDIT_BYTES'),
    );
    expect(tsNum(backend, 'STREAM_FRAME_OVERHEAD_BYTES'), '帧开销镜像与宿主脱钩').toBe(
      rustNum(rs, 'STREAM_FRAME_OVERHEAD_BYTES'),
    );
    const readings: Array<[string, number]> = [
      ['宿主初始额度', rustNum(rs, 'DEFAULT_STREAM_CREDIT_BYTES')],
      ['宿主补额封顶', rustNum(rs, 'MAX_STREAM_CREDIT_BYTES')],
      ['宿主帧开销', rustNum(rs, 'STREAM_FRAME_OVERHEAD_BYTES')],
    ];
    for (const [label, value] of readings) {
      expect(value, `${label}被读成 0：等值针在 0 == 0 上自我豁免`).toBeGreaterThan(0);
    }
    // 同族的方向也要钉：初始必须严格低于封顶，否则「补额封顶」无处可试，
    // 而 ② 的 min() 针会变成一句永远成立的话。
    expect(
      rustNum(rs, 'DEFAULT_STREAM_CREDIT_BYTES'),
      '初始额度不低于封顶：窗口没有增长空间，封顶针成为空话',
    ).toBeLessThan(rustNum(rs, 'MAX_STREAM_CREDIT_BYTES'));
  });

  it('② 宿主的构造点与判定式逐字：额度只有那一份原语说了算', () => {
    const rs = read(STREAM_RS);
    const admission = read(ADMISSION_RS);
    expect(rs, '窗口不再由两侧镜像常量构造（初始/封顶换了来源，①的等值针就成了空账）').toContain(
      'CreditWindow::new(DEFAULT_STREAM_CREDIT_BYTES as u64, MAX_STREAM_CREDIT_BYTES as u64)',
    );
    expect(rs, '帧开销没进代价式：空帧免费，只发空帧的流能把额度走得无限远').toMatch(
      /STREAM_FRAME_OVERHEAD_BYTES\s*\.saturating_add\(json_bytes\)\s*\.saturating_add\(/,
    );
    expect(
      admission,
      '拒绝判据被换成 >=：恰好等于余额的帧也会被拒（两侧差一，轮 65 同形）',
    ).toContain('if credits > self.available {');
    expect(admission).not.toMatch(/if credits >=\s*self\.available/);
    expect(admission, '补给不再封顶：窗口可以越过 MAX 无限增长').toContain(
      'self.available = self.available.saturating_add(credits).min(self.max);',
    );
  });

  it('③ 测试替身的判据方向逐字，裸字面量不许回潮', () => {
    const backend = read(BACKEND_TS);
    expect(backend).toContain(
      'const required = STREAM_FRAME_OVERHEAD_BYTES + jsonBytes + rawBytes;',
    );
    expect(backend, '开销降回裸字面量：那是第三份可以各自漂移的账').not.toMatch(
      /const required = \d+ \+/,
    );
    expect(backend, '拒绝判据方向漂走（>= 会把恰好用光的帧也拒掉）').toContain(
      'if (required > state.creditBytes)',
    );
    expect(backend).not.toContain('if (required >= state.creditBytes)');
    expect(backend, '拒了却仍扣额：留下部分效果，与宿主的「拒绝零副作用」相反').toContain(
      'state.creditBytes -= required;',
    );
    expect(backend, '补额不封顶：这份内核的窗口可以越过宿主的硬上限').toContain(
      'state.creditBytes = Math.min(MAX_STREAM_CREDIT_BYTES, state.creditBytes + bytes);',
    );
    expect(backend, '拒绝不再报背压码：用例与真机的失败面从此无关').toContain(
      'E_STREAM_BACKPRESSURE',
    );
    expect(
      hits(backend, 'if (!Number.isSafeInteger(bytes) || bytes < 0)'),
      '补给入参的判定处数变了（这份内核只该有一处）',
    ).toBe(1);
  });

  it('④ 补额这条腿全链可达：命令 → 线格式 → 注册表 → 那份原语', () => {
    const tauri = read(TAURI);
    const adapter = read(ADAPTER);
    const registry = read(REGISTRY_RS);
    const rs = read(STREAM_RS);
    const start = tauri.indexOf('pub fn host_stream_grant(');
    expect(start, '读不到补额命令的定义（下面的切片针会整条失效）').toBeGreaterThanOrEqual(0);
    const body = tauri.slice(start, tauri.indexOf('pub fn host_stream_close', start));
    expect(body, '补额命令不再是 tauri 命令').toContain('#[tauri::command]');
    expect(body, '命令不再取订阅者身份：补额会落到任意主体上（宿主侧是 owner-scoped）').toContain(
      'subscriber_of(window.label())',
    );
    expect(body, '命令体不再进线格式层').toContain('wire_stream_grant(&state, &subscriber, &req)');
    expect(tauri, '命令没注册进 invoke_handler（前端调它只会 command not found）').toContain(
      '$crate::tauri::host_stream_grant,',
    );
    expect(tauri, '线格式层没把字节数交给核心').toContain(
      'crate::cmd_stream_grant(state, subscriber, &req.stream_id, req.bytes)',
    );
    expect(adapter, '核心没问注册表要额度（把返回值换成常数，门禁必须红）').toContain(
      'state.registry.stream_grant(stream_id, subscriber, bytes)',
    );
    expect(registry, '注册表没把补额交给那条流的窗口').toContain(
      'self.streams.lock().grant(stream_id, subscriber, bytes)',
    );
    expect(rs, '原语本身不再收额：§98 的 Consumer 补给无处可去').toContain(
      'handle.credit.grant(bytes as u64)',
    );
    expect(
      hits(read(HOST_TS), "this.call<StreamCredit>('host_stream_grant'"),
      '前端两条补额路径少了一条（开流句柄腿与独立入口腿各一条）',
    ).toBe(2);
  });

  it('⑤ 回执线形态两端字段序同构（宿主驼峰化后 = TS 声明序）', () => {
    const adapter = read(ADAPTER);
    const rustStruct = adapter.slice(
      adapter.indexOf('pub struct StreamCredit {'),
      adapter.indexOf('pub fn cmd_stream_grant('),
    );
    expect(
      [...rustStruct.matchAll(/pub (\w+):/g)].map((m) => m[1]),
      '宿主回执字段序被改：TS 侧读到的就不再是同一份事实',
    ).toEqual(['stream_id', 'credit_bytes']);
    expect(
      adapter,
      '宿主回执不再 camelCase 上线（TS 侧读到 undefined，探针与补额回执从此脱钩）',
    ).toMatch(/#\[serde\(rename_all = "camelCase"\)\]\s*pub struct StreamCredit \{/);
    const ts = read(STREAM_TS);
    const tsStruct = ts.slice(
      ts.indexOf('export interface StreamCredit {'),
      ts.indexOf('export function isTerminalKind'),
    );
    expect(
      [...tsStruct.matchAll(/^\s{2}(\w+)\??:/gm)].map((m) => m[1]),
      'TS 回执字段序与宿主不同构',
    ).toEqual(['streamId', 'creditBytes']);
  });

  it('⑥ 镜像注释、本轮记录与 4 条行为用例同册', () => {
    const stream = read(STREAM_TS);
    const backend = read(BACKEND_TS);
    const t = read(TEST_TS);
    expect(stream, '两个镜像常量没交代真源是谁').toContain('轮 65 在打包端抓到过同一类账');
    expect(backend, '开销常量没交代它是镜像、也没交代为什么不导出').toContain(
      '轮 66 之前这里是一个裸字面量 `32`',
    );
    expect(backend, '内核没交代「拒绝不留部分效果」这条与宿主同形的边界').toContain(
      '拒绝必须发生在扣额与占 `seq` **之前**',
    );
    expect(read(PLAN), '缺口方案缺轮 66 小节').toContain(
      '### 轮 66：额度这条腿第一次被两侧同值钉住',
    );
    const changelog = read('CHANGELOG.md');
    expect(changelog, 'CHANGELOG 缺轮 66 的收口条目').toContain('却从未与宿主对过表');
    expect(changelog, 'CHANGELOG 缺轮 66 的登记条目（补额无生产调用点）').toContain(
      '补额入口在仓内零生产调用点（轮 66 登记，等批准）',
    );
    for (const title of [
      "it('开流即拿到宿主的初始额度：空帧扣固定开销，载荷只按字节数加账'",
      "it('恰好用光额度必须放行，多 1 字节当场拒且零副作用（不发帧、不占 seq）'",
      "it('补额封顶在宿主的硬上限，非法补给当场拒'",
      "it('接收方补额后立刻恢复写帧，扣额仍由同一笔算术裁决'",
    ]) {
      expect(t, `缺行为用例：${title}`).toContain(title);
    }
    // 用例跑得起来的前提：补额命令在本文件的 CAPS 里。此前不在——那条分支从未
    // 被任何用例行使过，值针再多也只会钉住一份没人走过的代码。
    expect(t, '补额命令没进流式用例的 CAPS：那条分支又回到从未被行使').toContain(
      "  'host_stream_grant',\n",
    );
    expect(t, '开销不是从空帧现推的（第三份数字回到了用例里）').toContain(
      'async function deriveFrameOverhead',
    );
  });
});
