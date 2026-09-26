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

import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { TAURON_COMMANDS } from '@tauron/core';
import {
  CAPABILITIES,
  HOST_ERROR_CODES,
  LIFECYCLE_EVENTS,
  LIFECYCLE_STATES,
  RETRYABLE_HOST_ERROR_CODES,
} from '@tauron/host';
import { PluginErrorCode, RETRYABLE_ERROR_CODES } from '@tauron/types';

const here = dirname(fileURLToPath(import.meta.url));
// packages/tauron-contract-tests/src → packages/tauron-contract-tests → packages → root
const workspaceRoot = resolve(here, '..', '..', '..');
const read = (rel: string): string => readFileSync(resolve(workspaceRoot, rel), 'utf8');

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

  it('线名逐项同序一致（Rust ErrorCode ↔ TS HOST_ERROR_CODES）', () => {
    const rustCodes = [...variantToCode().values()];
    expect(rustCodes, 'Rust 侧应解析出全部变体').toEqual([...HOST_ERROR_CODES]);
    // 不再硬编码个数（此前写死 18，新增 `E_STREAM_FULL` 就得改这里——个数是从
    // 两侧解析结果里导出的，硬编码只会制造"改一处忘一处"的假失败）。
    // 这里改钉**下限**：确保正则真的抓到了码表，而不是空匹配蒙混过关。
    expect(HOST_ERROR_CODES.length).toBeGreaterThanOrEqual(18);
    expect(rustCodes.length).toBe(HOST_ERROR_CODES.length);
  });

  it('线名必须等于枚举变体名（E_* 大写蛇形，禁止 camelCase 漂移）', () => {
    const variants = [...variantToCode()];
    expect(variants.length).toBeGreaterThan(0);
    for (const [variant, code] of variants) {
      expect(code, `${variant} 的线名必须等于变体名`).toBe(variant);
    }
  });

  it('可重试集合一致（Rust variant → 线名 → TS 集合）', () => {
    const open = hostErrorSrc.indexOf('matches!(', hostErrorSrc.indexOf('pub fn retryable'));
    expect(open, 'retryable() matches! not found').toBeGreaterThan(-1);
    const block = hostErrorSrc.slice(open, hostErrorSrc.indexOf(')', open) + 1);
    const lookup = variantToCode();
    const rustRetryable = [...block.matchAll(/Self::(E_\w+)/g)]
      .map((m) => m[1]!)
      .map((v) => lookup.get(v))
      .filter((c): c is string => Boolean(c));
    expect(rustRetryable.sort()).toEqual([...RETRYABLE_HOST_ERROR_CODES].sort());
    expect(rustRetryable).toHaveLength(3);
  });

  it('HostError 必须以结构化 JSON 穿越 IPC（禁止 {:?} 文本转储）', () => {
    // 派生 Serialize：前端 normalizeError 分支 1 依赖 { code, message, retryable }。
    const derive = hostErrorSrc.match(/#\[derive\(([^)]*)\)\]\s*\n#\[error/)?.[1] ?? '';
    expect(derive, 'HostError 必须 derive(Serialize)').toContain('Serialize');
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
    expect(adapterSrc).toMatch(
      /ProcError::AbiMismatch \{ \.\. \} => ErrorCode::E_ABI_MISMATCH/,
    );
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
    const methods = [...src.matchAll(/async (\w+)\([^)]*\): Promise<[^>]+> \{\s*return this\.call<[^>]*>\('host_\w+'/g)]
      .map((m) => m[1]!);
    // 正则失配保护：ShellClient 至少有这些真实命令包装方法。
    expect(methods.length, '未解析出 ShellClient 命令方法（正则失配）').toBeGreaterThanOrEqual(10);

    const calls = tsShellCommands();
    // 每个命令字面量都应被某个方法用到（孤儿 = 字面量出现但无方法持有，或反之）。
    for (const cmd of ['host_recover_report', 'host_recover_trial_enable', 'host_i18n_set_locale', 'host_i18n_load', 'host_i18n_stats', 'host_i18n_cleanup_plugin']) {
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

    const blob = files.map((f) => read(f)).join('\n');
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
    if (pkg.isDirectory()) walk(join(workspaceRoot, 'packages', pkg.name, 'src'));
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
      const t = token.trim().replace(/#\[[^\]]*\]/g, '').trim();
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
        /\b(CallerSource|TauriCallerSource|WebviewWindow|Webview|AppHandle|Window|State)\b/.test(type);
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

const SCREAMING = (s: string): string =>
  s.replaceAll(/([a-z0-9])([A-Z])/g, '$1_$2').toUpperCase();

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
    expect(hostTs).toMatch(/lifecycleReport\(evt: \{ event: LifecycleEvent/);
    expect(hostTs).not.toMatch(/lifecycleReport\(evt: \{ state:/);
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
  const optional = adapter.slice(adapter.indexOf('pub const PLUGIN_INSTALL_AUTH'), adapter.indexOf('/// 命令状态'));
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
  it('命令集合一致（插件面 17 条 + 主窗特权 6 条）', () => {
    const rust = rustAuthTable();
    const ts = new Map(CAPABILITIES.map((c) => [c.command, c.tier]));
    // 不写死总数（会随命令面增长而漂移）：只钉住两表**逐条相等**与结构比例。
    expect(ts.size, 'TS CAPABILITIES 条目数').toBe(rust.size);
    expect([...ts.keys()].sort(), '命令集合').toEqual([...rust.keys()].sort());
    const pluginFace = CAPABILITIES.filter((c) => c.tier !== 'privileged');
    // 17 = 13（0.4-A1 之前）+ 跨主体调用 3 条 + 0.4 审计补登记 host_contributes_list。
    expect(pluginFace.length, '插件面（self + scoped-read）命令数').toBe(17);
    expect(CAPABILITIES.filter((c) => c.consumer === 'plugin').map((c) => c.command).sort()).toEqual(
      pluginFace.map((c) => c.command).sort(),
    );
    // 审计补登记（轮 11）：这 4 条此前**没有任何档位**，但 `HostClient` 已在调用。
    for (const cmd of ['host_events_drain', 'host_stream_open', 'host_stream_write', 'host_stream_close']) {
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
    // 特权档命令**不得**出现在插件可见能力里。
    expect(CAPABILITIES.filter((c) => c.tier === 'privileged').map((c) => c.command).sort()).toEqual(
      ['host_registry_admin', 'host_registry_install', 'host_registry_install_preview', 'host_resource_stats', 'host_runtime_health', 'host_runtime_spawn'],
    );
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
function rustStruct(
  rel: string,
  structName: string,
): { fields: Set<string>; camelCase: boolean } {
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
    expect([...frame.fields].sort()).toEqual(['payload', 'seq', 'topic']);
    const ts = read('packages/tauron-host/src/events.ts');
    const m = /export interface EventFrame \{([\s\S]*?)\n\}/.exec(ts);
    expect(m, 'TS EventFrame 必须存在').not.toBeNull();
    const tsFields = [...m![1]!.matchAll(/^\s{2}(\w+)[?]*:/gm)].map((x) => x[1]!);
    expect(tsFields.sort()).toEqual(['payload', 'seq', 'topic']);
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
    expect(lib).toMatch(/pub fn cmd_brand_info[\s\S]{0,250}HostResult<UnsupportedBody>/);
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
    expect(read('packages/tauron-host/src/deep-link-client.ts')).toMatch(/Promise<ProviderResult<void>/);
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
    expect(lib).toMatch(/fn cmd_recover_trial_enable[\s\S]{0,4000}reconcile_recovery_phase\(state\)/);
    // trial_enable 走 D28 `TrialEnable`（清标记 + 记独立试验预算 +
    // trialFromSafemode 置位，供插件自报错误时按 D28 回落）。改回
    // SafemodeExit = 注册表不记预算、D28 回落不可达——必须同时让本门变红。
    expect(lib).toMatch(/fn cmd_recover_trial_enable[\s\S]{0,4000}Event::TrialEnable/);
    expect(lib).not.toMatch(/fn cmd_recover_trial_enable[\s\S]{0,4000}Event::SafemodeExit/);
    // 试启结果必须回传前置对账（诊断 + TS RecoveryTrialResult 契约字段）。
    expect(lib).toMatch(/fn cmd_recover_trial_enable[\s\S]{0,6000}result\["phaseReconcile"\]/);
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
      ...[...adapter.matchAll(/\$crate::tauri::(host_registry_install(?:_preview)?),/g)].map((m) => m[1]!),
    ]);

    const missing = cmds.filter((c) => !registered.has(c));
    expect(
      missing,
      `以下命令未注册，前端调用会 command not found：${missing.join(', ')}`,
    ).toEqual([]);
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
  // 断链原型：`host_registry_install*` 挂 `#[cfg(feature = "plugin-install")]` 而
  // `Cargo.toml` 的 `default = []`，默认构建不注册；此前它们被无条件列进
  // `FRAMEWORK_COMMANDS`，于是 `capabilities()` 对它们误报已注册——调用方按能力表
  // 判断"能不能装插件"拿到 `true`，直到 invoke 才 `command not found`。
  // 本门禁把「哪些命令是可选的」这份真相钉在两侧之间：集合必须对得上，
  // 且可选命令**不得**出现在静态全集里（只能由 host_capabilities 运行期开门）。
  it('TS 可选命令集与 Rust feature-gated 命令集一致（默认构建不得误报已注册）', () => {
    const rust = read('crates/tauron-adapter/src/lib.rs');
    const installBlock = /pub const PLUGIN_INSTALL_COMMANDS[\s\S]*?\];/.exec(rust)?.[0] ?? '';
    const rustOptional = [...installBlock.matchAll(/"(host_[a-z0-9_]+)"/g)].map((m) => m[1]!);
    expect(rustOptional.length, '未解析出 Rust 侧可选命令（正则失配）').toBeGreaterThan(0);

    const ts = read('packages/tauron-host/src/tauri-backend.ts');
    const tsOptional = [
      ...(/export const OPTIONAL_FRAMEWORK_COMMANDS = \[([\s\S]*?)\] as const;/.exec(ts)?.[1] ??
        '').matchAll(/'(host_[a-z0-9_]+)'/g),
    ].map((m) => m[1]!);
    expect(tsOptional.length, '未解析出 TS 侧可选命令（正则失配）').toBeGreaterThan(0);
    expect(tsOptional.sort()).toEqual(rustOptional.sort());

    // 静态全集里不得出现可选命令——出现即回退成"能力表说有、invoke 说没有"。
    const staticList =
      /const FRAMEWORK_COMMANDS = \[([\s\S]*?)\] as const;/.exec(ts)?.[1] ?? '';
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

    const union = /phase: ([^;]+);/.exec(read('packages/tauron-host/src/shell-client.ts'))?.[1] ?? '';
    const tsValues = [...union.matchAll(/'([a-z]+)'/g)].map((m) => m[1]!);
    expect(tsValues.length, '未解析出 TS phase 联合类型（正则失配）').toBe(3);

    expect([...tsValues].sort()).toEqual([...rustValues].sort());
  });

  it('loadSource 取值集合两侧一致（Rust LoadSource::as_str ↔ TS LoadSource）', () => {
    // 与 phase 同理：前端按 loadSource 区分「持久化没开 / 首次启动 / 恢复 / 损坏」，
    // 改名会静默失效。
    const body =
      /impl LoadSource \{([\s\S]*?)\n\}/.exec(read('crates/tauron-adapter/src/recovery.rs'))?.[1] ?? '';
    const rustValues = [...body.matchAll(/LoadSource::\w+ => "([a-z]+)"/g)].map((m) => m[1]!);
    expect(rustValues.length, '未解析出 LoadSource::as_str 取值（正则失配）').toBe(4);

    const union =
      /export type LoadSource = ([^;]+);/.exec(read('packages/tauron-host/src/shell-client.ts'))?.[1] ?? '';
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
    expect(src).toMatch(/manage_states\([\s\S]{0,120}command_state_with_dir_and_config\(/);
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

  it('bootstrap 必须上报启动结果（持久化打开后漏报 = 每次重启计一次崩溃）', () => {
    // 断链回归：持久化一旦在宿主入口打开，崩溃检测的干净退出判据就变成
    // 「本轮上报过 success」。前端没有任何一处上报 = 两次重启进安全模式。
    const src = read('packages/tauron-host/src/bootstrap.ts');
    expect(src).toMatch(/recoverReport\('success'\)/);
    // best-effort 必须显式吞错：上报失败不得反过来弄垮启动。
    expect(src).toMatch(/recoverReport\('success'\)\.catch\(/);
  });

  it('参考集成示例必须上报启动结果（它是接入方的唯一样板）', () => {
    // 示例不走 bootstrap()（裸用 ShellClient），漏报不会被上面的门禁抓到——
    // 而抄这个示例的接入方会把「两次重启进安全模式」当成框架行为。
    const src = read('examples/minimal-app/src/main.ts');
    expect(src, '示例必须上报 host_recover_report').toMatch(/recoverReport\('success'\)/);
    expect(src, '示例必须消费阶段决策而非只发后不管').toMatch(/r\.phase/);
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
    expect(code, '不得用 Stdio::null() 丢弃 sidecar 输出（回帧断链）').not.toMatch(/Stdio::null\(\)/);

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
    expect(first, '插件页必须用主推 SDK 的 createPlugin').toMatch(
      /from '@tauron\/app-plugin-sdk'/,
    );
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
    const admin = lib.slice(lib.indexOf('pub fn cmd_registry_admin('), lib.indexOf('pub fn cmd_registry_admin_as('));
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
    expect(src).toMatch(/this\.downloadUpdate\(\)\.catch\(/);
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

  /** 提取某文件里以契约常量名派发的所有事件 key。 */
  function dispatchedKeys(rel: string): string[] {
    return [...read(rel).matchAll(/new CustomEvent\(SHELL_EVENTS\.([a-zA-Z]+)/g)].map((m) => m[1]!);
  }

  it('@tauron/ui-primitives 不得依赖 @tauron/host（底座可取组件而不取宿主）', () => {
    // R3 / C1：此前 `@tauron/ui` 依赖 `@tauron/host`，「哑组件靠 props 喂数」是
    // 假象——只想要标题栏的底座项目会把整个宿主客户端层拖进来。
    const pkg = JSON.parse(read('packages/tauron-ui-primitives/package.json')) as {
      dependencies?: Record<string, string>;
      peerDependencies?: Record<string, string>;
    };
    expect(pkg.dependencies?.['@tauron/host'], 'ui-primitives 不得依赖 @tauron/host').toBeUndefined();
    expect(pkg.peerDependencies?.['@tauron/host'], 'ui-primitives 不得以 peer 依赖 @tauron/host').toBeUndefined();
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

  it('wc-shell 派发的每个契约事件必须在 ShellController 有归属（接线或声明接入方域）', () => {
    // 断链回归：更新对话框的「开始更新/立即重启」与插件管理器的启用开关曾零监听。
    // 本门禁要求每个派发要么被控制器接线，要么在控制器头注的「接入方域」清单写明。
    const contract = shellEventContract();
    expect(contract.size, 'ShellController 契约事件解析失败').toBeGreaterThan(8);

    const dispatched = dispatchedKeys('packages/tauron-ui-primitives/src/wc-shell.ts');
    expect(dispatched.length, 'wc-shell 契约派发提取失败').toBeGreaterThan(4);

    const controller = read('packages/tauron-host/src/shell-controller.ts');
    for (const key of dispatched) {
      const name = contract.get(key);
      expect(name, `契约里没有 ${key} 这个 key`).toBeDefined();
      expect(
        controller.includes(`SHELL_EVENTS.${key}`) || controller.includes(name!),
        `事件 ${name} 在 ShellController 无归属（既没接线也没声明接入方域）`,
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

  it('@tauron/host 不得静态引入 UI 包（保持宿主入口 DOM/lit 无关）', () => {
    // host 是轻量客户端层（`sideEffects: false`，node 测试环境可用）。
    // 静态 import/re-export UI 包会把 lit 与全部 DOM 组件拖进每个消费者。
    const idx = read('packages/tauron-host/src/index.ts');
    expect(idx).not.toMatch(/^export .*from '@tauron\/ui(-primitives)?'/m);
    expect(idx).not.toMatch(/^import .*from '@tauron\/ui(-primitives)?'/m);
    // bootstrap 只允许动态 import（非字面量变量，TS 不静态解析）。
    const boot = read('packages/tauron-host/src/bootstrap.ts');
    expect(boot).toMatch(/await import\(moduleName\)/);
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
    // 22 = 19（0.4-A1 之前）+ host_call_plugin / host_call_result / host_call_take。
    const pluginOnly = full.filter((c) => PLUGIN_DOMAIN.test(c));
    expect(pluginOnly.length, '插件域命令数量异常').toBe(22);
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
    expect(STREAM_CMDS.filter((c) => !full.includes(c)), '流式命令未成组注册').toEqual([]);
    // 流属插件运行时域（句柄表与 pending-call 同锁域），底座集合不得含。
    expect(
      STREAM_CMDS.filter((c) => substrate.includes(c)),
      '底座集合不得含流式命令（它们依赖插件运行时状态）',
    ).toEqual([]);

    // ② 载体必须**真的**登记到调用上：R5 之前 channel 收下即丢，帧进虚空且不报错。
    expect(tauri, 'wire 层必须把 sink 登记到调用上').toMatch(
      /stream_bind\(&call\.call_id, &call\.plugin_id, sink\)/,
    );
    expect(tauri, '登记失败必须撤掉调用条目，不留悬挂').toMatch(
      /call_cancel\(&call\.call_id\)/,
    );
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
      expect(signature[0], `${signature[1]} 不应直接依赖 Tauri 窗口参数`).not.toMatch(/\bWebviewWindow\b/);
    }
    expect(tauri).toMatch(/pub trait CallerSource\s*\{[\s\S]*?fn caller\(&self\) -> HostResult<crate::Caller>/);
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
    const backendCode = backendTs
      .replace(/\/\*[\s\S]*?\*\//g, '')
      .replace(/\/\/[^\n]*/g, '');
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
      expect(src, `${name} 仍在就地构造边界异常`).not.toMatch(/new HostException\(normalizeError\(/);
      expect(src, `${name} 未经 translate_at_boundary`).toMatch(
        /translate_at_boundary\(err, 'plugin-webview→host'\)/,
      );
    }

    // ③ 两套词表**不得**合并（合并会静默改变调用方的分支语义）。
    const hostCodes = errorsTs.slice(
      errorsTs.indexOf('HOST_ERROR_CODES = ['),
      errorsTs.indexOf('] as const;'),
    );
    expect(hostCodes, '宿主码表混入应用层码').not.toMatch(/SC-\d{4}/);
    const appCodes = read('packages/types/src/errors.ts');
    expect(appCodes, '应用层码表混入宿主码').not.toMatch(/'E_[A-Z_]+'/);
    // 形态识别 ≠ 语义接受：非宿主码仍归 E_UNKNOWN，只保留原始码。
    expect(errorsTs).toMatch(/export function isAppLayerErrorCode\(/);
    expect(errorsTs).toMatch(/export function isCodeLike\(/);
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
    for (const crate of ['tauron-brand', 'tauron-market', 'tauron-wasm', 'tauron-theme', 'tauron-distribute', 'tauron-shell']) {
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
    const methodsOf = (src: string) =>
      [...src.matchAll(/^\s{4}pub fn (\w+)/gm)].map((m) => m[1]!);
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
    expect(RT.filter((c) => !full.includes(c)), '运行时命令未成组注册').toEqual([]);
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
    const ts = read('packages/tauron-host/src/shell-client.ts');

    const rustFields = (structName: string): string[] => {
      const m = new RegExp(`pub struct ${structName} \\{([\\s\\S]*?)\\n\\}`).exec(rustHost) ??
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

    const listed = [...errorsTs.matchAll(/UNWIRED_BOUNDARIES[\s\S]*?\];/g)]
      .flatMap((m) => [...m[0]!.matchAll(/'([\w→-]+)'/g)].map((x) => x[1]!));
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
    expect(prose, '未记录 bridge.ts 的 SC-9001 硬换码缺口').toMatch(/bridge\.ts/);
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
    expect(recovery, 'last_context 读取缺容错/默认值').toMatch(/last_context[\s\S]{0,200}unwrap_or/);
    // 线形：boot 返回必须带上它。
    const lib = read('crates/tauron-adapter/src/lib.rs');
    expect(lib, 'host_recover_boot 线形未回传 lastContext').toMatch(/"lastContext"/);
    // 引擎侧容量常量必须存在且被测试钉住（N 是设计决定，不能是魔数）。
    const engine = read('crates/tauron-recovery/src/lib.rs');
    expect(engine, '缺少 context 容量常量').toMatch(/CONTEXT_CAPACITY/);
    expect(engine, 'CONTEXT_CAPACITY 没有测试引用').toMatch(/CONTEXT_CAPACITY[\s\S]{0,400}assert|assert[\s\S]{0,400}CONTEXT_CAPACITY/);
  });

  it('R7：settings 经 SettingsStore（不是裸 HashMap），且迁移有线上入口', () => {
    // 方案 R7 门禁原文：「settings 必须经 Store（非裸 HashMap）」。
    const cargo = read('crates/tauron-adapter/Cargo.toml');
    expect(cargo, 'adapter 未依赖 tauron-settings（孤儿 crate 未激活）').toMatch(/tauron-settings/);
    const lib = read('crates/tauron-adapter/src/lib.rs');
    expect(lib, 'settings 未走 SettingsStore').toMatch(/SettingsStore/);
    expect(lib, 'settings_set 未调用 Store::set').toMatch(/\.set\([\s\S]{0,200}HOST_SETTINGS_NAMESPACE/);

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
    expect(pushAt, '顺序错误：必须先 push 再 send（否则系统通知失败会丢通知）').toBeLessThan(sendAt);

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
      expect(fnBody(lib, core), `${core} 收了主体却不判定`).toMatch(/require_main_window/);
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

    // 第二批的四条主窗专属命令（试启别人的插件、宿主级更新操作）。
    for (const [cmd, core] of [
      ['host_recover_trial_enable', 'cmd_recover_trial_enable_as'],
      ['host_market_check', 'cmd_market_check_as'],
      ['host_market_download', 'cmd_market_download_as'],
      ['host_market_install', 'cmd_market_install_as'],
    ] as Array<[string, string]>) {
      expect(fnBody(tauri, cmd), `${cmd} 未从受信传输上下文取得主体`).toMatch(
        /(?:window\.caller\(\)|Caller::from_label\(window\.label\(\)\))/,
      );
      expect(fnBody(lib, core), `${core} 未做主窗判定`).toMatch(/require_main_window/);
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
    expect(
      scope,
      '命名空间判定不得用裸前缀比较（plugin:p.a 会穿透 plugin:p.ab）',
    ).not.toMatch(/starts_with\((?:&)?(?:ns|namespace|prefix)\b/);
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

  it('无孤儿命令：每条 #[tauri::command] 都必须真的被某个 handler 宏注册', () => {
    // 轮 11 审计抓到的断链类：`cmd_settings_adopt_legacy` / `cmd_settings_migrate`
    // 曾经**有实现、有包装器、有单测，但没进任何宏**——真实宿主里这两条命令根本不存在，
    // 只有 Rust 单测能碰到。这类"定义了却没注册"的断链，编译器不会报、单测也不会报。
    const tauri = read('crates/tauron-adapter/src/tauri.rs');
    const defined = new Set(
      [...tauri.matchAll(/^\s*pub fn (host_\w+)\(/gm)].map((m) => m[1]!),
    );
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
    const PLUGIN_RUNTIME_DOMAIN_SIZE = 20;
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
    for (const f of [
      'crates/tauron-adapter/src/lib.rs',
      'crates/tauron-adapter/src/tauri.rs',
    ]) {
      const src = read(f);
      expect(src, `${f} 出现 todo!/unimplemented! 占位`).not.toMatch(
        /todo!\(|unimplemented!\(/,
      );
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
    expect(order.every((i) => i >= 0), `verify 未覆盖三步: ${verify}`).toBe(true);
    expect(
      order[0]! < order[1]! && order[1]! < order[2]!,
      `verify 顺序必须是 build → typecheck → test（否则类型断言读旧 dist）: ${verify}`,
    ).toBe(true);

    // 两个 SDK 的 typecheck 必须同处一条流水线：只跑一侧会漏掉另一侧的漂移。
    for (const name of ['@tauron/app-plugin-sdk', '@tauron/plugin-sdk']) {
      const p = JSON.parse(read(`packages/${name.replace('@tauron/', 'tauron-')}/package.json`)) as {
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
    const commandsOfClass = (src: string, cls: string): string[] => {
      const starts: Array<[string, number]> = [];
      const re = /^export class (\w+)/gm;
      let m: RegExpExecArray | null;
      while ((m = re.exec(src)) !== null) starts.push([m[1]!, m.index]);
      const idx = starts.findIndex(([n]) => n === cls);
      if (idx < 0) throw new Error(`未找到 class ${cls}`);
      const end = idx + 1 < starts.length ? starts[idx + 1]![1] : src.length;
      const body = stripComments(src.slice(starts[idx]![1], end));
      return [
        ...new Set([...body.matchAll(/(?:invoke|call)(?:<[^>]*>)?\(\s*'([a-z_]+)'/g)].map((x) => x[1]!)),
      ];
    };

    const hostTs = read('packages/tauron-host/src/host.ts');
    const shellTs = read('packages/tauron-host/src/shell-client.ts');
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
          else if (e.name.endsWith('.ts') && !e.name.includes('.test.')) files.push(join(d, e.name));
        }
      };
      walk(dir);
      const invoked = new Set<string>();
      for (const f of files) {
        for (const m of readFileSync(f, 'utf8').matchAll(/'(host_[a-z0-9_]+)'/g)) invoked.add(m[1]!);
      }
      const untiered = [...invoked].filter((c) => !tierOf.has(c) && rustMacros.has(c));
      expect(
        untiered,
        `${pkg} 直接调用了无档位的插件面命令: ${untiered.join(', ')}`,
      ).toEqual([]);
    }
  });
});
