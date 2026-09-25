/**
 * @tauron/cli 共享类型
 */

export type PluginType = 'js' | 'process' | 'wasm';

export interface PluginConfig {
  name: string;
  type: PluginType;
  description?: string;
  permissions?: string[];
  entry?: string;
}

export interface AppConfig {
  name: string;
  description?: string;
  /**
   * 应用模板。
   *
   * **已实现**：`vanilla`（默认）、`react`——见 `scaffold.ts` 的 `IMPLEMENTED_TEMPLATES`。
   * `vue` / `svelte` 在此已声明但**尚无骨架实现**；CLI 对它们**如实失败**，
   * 不会静默退回 `vanilla`（那会让用户拿到与所填模板不符、且编译时才暴露的工程）。
   */
  template: 'vanilla' | 'react' | 'vue' | 'svelte';
  pluginTypes?: PluginType[];
}

/**
 * npm `package.json` 描述（插件脚手架的**打包**描述，不是宿主清单）。
 *
 * 宿主清单是 `tauron.plugin.json`（见 `plugin.ts` 的 `TauronPluginManifest`）。
 * 两者**故意不重叠**：权限等宿主语义只写在宿主清单里，避免两处漂移。
 */
export interface PackageManifest {
  name: string;
  version: string;
  description: string;
  /**
   * Node 模块系统。生成物入口是 ESM（`import`），故恒为 `"module"`。
   *
   * ⚠️ 这里**不能**放插件形态（`js`/`process`/`wasm`）——那会让 Node 把 `.js`
   * 按 CommonJS 解析，入口的 `import` 语句直接加载失败。
   */
  type: 'module';
  main: string;
  peerDependencies?: Record<string, string>;
  devDependencies?: Record<string, string>;
}

export interface CliOptions {
  verbose: boolean;
  dryRun: boolean;
  force: boolean;
  /**
   * 落盘根目录。缺省 `process.cwd()`。
   *
   * 单独开这个口子是为了让单测能把脚手架写进临时目录——否则
   * `runCli(['create', 'my-app'])` 会在**仓库里**建出 `my-app/`。
   */
  cwd?: string;
}
