/**
 * tauron plugin — 插件脚手架生成器
 *
 * 纯生成器：只拼字符串，**不碰文件系统**（落盘归 `scaffold-writer.ts`）。
 *
 * ## 生成两个清单，各司其职（别再合并）
 *
 * - `package.json` —— **npm 打包**用（name / version / main / devDependencies）；
 * - `tauron.plugin.json` —— **tauron 宿主清单**（id / type / entry / framework /
 *   permissions），`plugin dev|test|pack|sign|publish` 读的就是它。
 *
 * 历史缺陷：本文件只生成 `package.json`，而那五条生命周期命令全都找
 * `tauron.plugin.json`，并在找不到时提示「Run "tauron plugin new" to create a
 * plugin first」——**形成一个死循环指令**：照它说的做了，下一个命令还是找不到文件。
 *
 * ## 清单字段必须能被宿主接受
 *
 * `tauron.plugin.json` 的形状对齐 Rust `tauron_host::manifest::PluginManifest`
 * （`deny_unknown_fields`）：`id` 必须反域名、`framework` 必填（缺失即不兼容）、
 * `permissions` 只能取自 `schema/permissions.index.json`（表外即安装失败）。
 * 早期脚手架默认写的 `store:read` / `http:fetch` 是**框架 ACL** 词表，不在 index 里，
 * 照抄会被 `validate()` 拒掉。
 */

import type { PluginConfig, PackageManifest, CliOptions, PluginType } from './types.js';

/**
 * tauron 宿主清单（`tauron.plugin.json`）的线形。
 *
 * 对齐 Rust `tauron_host::manifest::PluginManifest` 的**必填子集**；
 * `abi` 仅 A/D 类需要。
 */
export interface TauronPluginManifest {
  id: string;
  name: string;
  version: string;
  type: PluginType;
  entry: {
    js?: string;
    sidecar?: string;
    wasm?: string;
    ui?: string;
  };
  /** semver range；**必填**，缺失即视为与框架不兼容。 */
  framework: string;
  /** 只能取自 `schema/permissions.index.json`。 */
  permissions: string[];
  abi?: { rust?: string; wasm?: string };
}

/**
 * 默认权限：**取自 `schema/permissions.index.json`**（不是框架 ACL 的 `store:read`）。
 * 表外权限会被宿主 `PluginManifest::validate` 拒绝。
 */
export const DEFAULT_MANIFEST_PERMISSIONS = ['store:allow-get', 'http:allow-fetch'] as const;

/** 当前脚手架对齐的框架版本区间。 */
export const FRAMEWORK_RANGE = '>=2.0 <3.0';

/** 生成合法的反域名 plugin id。 */
export function derivePluginId(name: string): string {
  // 保留 `.` 作为段分隔符（早期实现把点也替换掉了，`com.acme.x` 会退化成单段）
  const cleaned = name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9.-]+/g, '-')
    .replace(/\.{2,}/g, '.')
    .replace(/-{2,}/g, '-')
    .replace(/^[.-]+/, '')
    .replace(/[.-]+$/, '');

  const parts = cleaned.split('.').filter(Boolean);
  if (parts.length >= 2 && parts.every((p) => /^[a-z0-9-]+$/.test(p))) {
    return cleaned;
  }

  const slug = cleaned.replace(/\./g, '-') || 'plugin';
  return `com.example.${slug}`;
}

/**
 * 创建新插件。
 */
export function pluginNew(config: PluginConfig, options: CliOptions): PluginCreateResult {
  const files: Record<string, string> = {};
  const permissions = [...(config.permissions ?? DEFAULT_MANIFEST_PERMISSIONS)];
  const manifest = generatePluginManifest(config, permissions);

  // npm 打包描述
  files['package.json'] = JSON.stringify(generatePackageJson(config), null, 2);

  // tauron 宿主清单 —— 生命周期命令读的就是它
  files['tauron.plugin.json'] = JSON.stringify(manifest, null, 2) + '\n';

  // 入口文件随类型而定
  if (config.type === 'js') {
    files['src/index.js'] = generateJsPlugin(config);
  } else if (config.type === 'wasm') {
    files['src/index.js'] = generateJsPlugin(config);
    files['Cargo.toml'] = generateWasmCargo(config);
    files['src/lib.rs'] = generateWasmRust(config);
  } else if (config.type === 'process') {
    files['src/index.js'] = generateJsPlugin(config);
    files['main.js'] = generateProcessPlugin(config);
  }

  files['README.md'] = generatePluginReadme(config, manifest);

  return {
    files,
    manifest,
    packageJson: generatePackageJson(config),
    dirName: config.name,
  };
}

export interface PluginCreateResult {
  files: Record<string, string>;
  /** 写入 `tauron.plugin.json` 的宿主清单。 */
  manifest: TauronPluginManifest;
  /** 写入 `package.json` 的 npm 描述。 */
  packageJson: PackageManifest;
  dirName: string;
}

/**
 * 生成 tauron 宿主清单。
 */
export function generatePluginManifest(
  config: PluginConfig,
  permissions: string[] = [...DEFAULT_MANIFEST_PERMISSIONS],
): TauronPluginManifest {
  const manifest: TauronPluginManifest = {
    id: derivePluginId(config.name),
    name: config.name,
    version: '0.1.0',
    type: config.type,
    entry: entryForType(config),
    framework: FRAMEWORK_RANGE,
    permissions,
  };

  // D 类（wasm）必须声明 `abi.wasm`，否则 validate() 拒绝。
  // 值是**接口 hash**，只有在真正构建出 wasm 模块后才能确定，故先留占位。
  if (config.type === 'wasm') {
    manifest.abi = { wasm: 'pending-build' };
  }

  return manifest;
}

/** 按类型给出入口声明。 */
function entryForType(config: PluginConfig): TauronPluginManifest['entry'] {
  switch (config.type) {
    case 'js':
      return { js: 'src/index.js' };
    case 'process':
      return { sidecar: 'main.js' };
    case 'wasm':
      return { wasm: `${config.name}.wasm` };
  }
}

/**
 * 生成 npm `package.json`。
 *
 * - `type` 恒为 `"module"`（入口是 ESM；早期写的是插件形态 `js`/`process`/`wasm`，
 *   那会让 Node 按 CommonJS 解析 `.js`，入口的 `import` 直接加载失败）；
 * - **不**声明 `permissions`——那是宿主清单（`tauron.plugin.json`）的事，
 *   两处都写必然漂移。
 */
export function generatePackageJson(config: PluginConfig): PackageManifest {
  return {
    name: config.name,
    version: '0.1.0',
    description: config.description ?? `tauron plugin: ${config.name}`,
    type: 'module',
    main: config.type === 'process' ? 'main.js' : 'src/index.js',
    devDependencies: {
      '@tauron/plugin-sdk': '^0.1.0',
    },
  };
}

/**
 * 生成 JS 插件入口。
 *
 * 方法表用**固定样例**（`ping`），不再从权限名派生——权限是「能做什么」的声明，
 * 不是「有哪些方法」的清单，早期把 `store:read` 变成方法名 `store_read` 是错的映射。
 */
function generateJsPlugin(config: PluginConfig): string {
  return `// ${config.name} — tauron 插件（B 类：JS，运行在身份 Webview 内）
//
// 与宿主的握手/调用协议由 @tauron/plugin-sdk 的 registerPlugin 建立：
// 宿主经 URL hash 注入握手 token，调用走 \`__invoke:\` / \`__result:\` 消息。
import { registerPlugin } from '@tauron/plugin-sdk';

registerPlugin({
  name: '${config.name}',
  version: '0.1.0',
  methods: {
    ping({ args, ctx }) {
      return { pong: true, pluginId: ctx.pluginId, echo: args };
    },
  },
  events: {},
  onEnable(ctx) {
    console.log('[${config.name}] enabled as', ctx.pluginId);
  },
  onDisable(ctx) {
    console.log('[${config.name}] disabled', ctx.pluginId);
  },
});
`;
}

/**
 * 生成 WASM Cargo.toml。
 */
function generateWasmCargo(config: PluginConfig): string {
  return `[package]
name = "${config.name.replace(/-/g, '_')}"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

# 诚实边界：tauron 的 **wasm 插件侧 SDK 尚未提供**。
# \`tauron-wasm\` 是宿主侧的引擎/校验库（不是 proc-macro，也没有插件模板宏），
# 且整个 workspace 的 crate 都未发布到 crates.io。
# 接入后在这里加依赖，并把 src/lib.rs 的入口换成 SDK 提供的形态。
[dependencies]
`;
}

/**
 * 生成 WASM Rust 入口。
 *
 * **不生成任何框架宏调用**：早期这里写的是 `use tauron_wasm::plugin; #[plugin] fn …`，
 * 而 `tauron-wasm` 既不是 proc-macro crate、也没有 `plugin` 属性宏——那段代码编译不过。
 * 现在只放一个最小 cdylib 骨架，导出符号由接入 wasm 运行时后补齐。
 */
function generateWasmRust(config: PluginConfig): string {
  return `// ${config.name} — tauron WASM 插件（D 类）骨架
//
// 诚实边界：本仓库目前**没有 wasm 插件侧 SDK**，所以这里不引用任何框架 crate，
// 只保证这个 crate 本身能编译成 cdylib。与宿主的入口符号/ABI 约定请在接入
// wasm 运行时后补齐（届时同步更新 tauron.plugin.json 的 \`abi.wasm\` 接口 hash）。

/// 插件初始化入口占位。
///
/// 返回 \`0\` 表示初始化成功。真实 ABI 约定接入后替换本函数。
#[no_mangle]
pub extern "C" fn tauron_plugin_init() -> i32 {
    0
}

#[cfg(test)]
mod tests {
    #[test]
    fn init_returns_success() {
        assert_eq!(super::tauron_plugin_init(), 0);
    }
}
`;
}

/**
 * 生成进程插件入口（C 类：sidecar）。
 */
function generateProcessPlugin(config: PluginConfig): string {
  return `// ${config.name} — tauron 进程插件（C 类：sidecar）
//
// 宿主以子进程方式拉起本文件，经 stdin/stdout 或 socket 交换 JSON-RPC 帧。
// 帧格式：每行一个 JSON 对象 { id, method, params } → { id, result | error }。
const { createServer } = require('node:net');

function handleMethod(method, params) {
  switch (method) {
    case 'ping':
      return { pong: true, echo: params };
    default:
      throw new Error(\`Unknown method: \${method}\`);
  }
}

function handleRequest(request) {
  const { id, method, params } = request;
  try {
    return { id, result: handleMethod(method, params) };
  } catch (error) {
    return { id, error: error.message };
  }
}

const server = createServer((socket) => {
  socket.on('data', (data) => {
    for (const line of data.toString().split('\\n')) {
      if (!line.trim()) continue;
      socket.write(JSON.stringify(handleRequest(JSON.parse(line))) + '\\n');
    }
  });
});

server.listen(0, () => {
  // 端口经 stdout 报给宿主（宿主按此端口建立连接）
  console.log(JSON.stringify({ ready: true, port: server.address().port }));
});
`;
}

/**
 * 生成插件 README。
 */
function generatePluginReadme(config: PluginConfig, manifest: TauronPluginManifest): string {
  return `# ${config.name}

${config.description ?? `tauron plugin: ${config.name}`}

## 清单

- plugin id：\`${manifest.id}\`
- 类型：\`${manifest.type}\`
- 入口：\`${Object.values(manifest.entry).join(', ')}\`
- framework：\`${manifest.framework}\`
- permissions：${manifest.permissions.map((p) => `\`${p}\``).join('、') || '（无）'}

清单文件是 \`tauron.plugin.json\`，形状对齐 Rust \`tauron_host::manifest::PluginManifest\`
（未知字段会被拒绝、\`framework\` 缺失即视为不兼容、\`permissions\` 只能取自
\`schema/permissions.index.json\`）。

> \`id\` 由插件名推导（\`${manifest.id}\`）。发布前请改成你自己的反域名。

## 目录

| 文件 | 用途 |
|---|---|
| \`tauron.plugin.json\` | 宿主清单（\`plugin dev/test/pack/sign/publish\` 读它） |
| \`package.json\` | npm 打包描述 |
| \`src/index.js\` | 插件入口 |
| \`README.md\` | 本文件 |
${
  config.type === 'wasm'
    ? `
## WASM 说明

**诚实边界**：本仓库尚未提供 wasm 插件侧 SDK，\`src/lib.rs\` 只是能编译的 cdylib
骨架，没有框架宏。构建出 \`.wasm\` 后需回填 \`tauron.plugin.json\` 的 \`abi.wasm\`
接口 hash，否则宿主加载期校验会拒绝。
`
    : ''
}${
  config.type === 'process'
    ? `
## 进程插件说明

入口是 \`main.js\`（sidecar）。宿主按 \`entry.sidecar\` 拉起并连接其自报端口。
`
    : ''
}
## 本地依赖

\`@tauron/*\` 包**尚未发布到 npm**，本地开发请把 \`devDependencies\` 改成
\`workspace:*\` 或 \`file:\` 指向本仓库对应目录。
`;
}
