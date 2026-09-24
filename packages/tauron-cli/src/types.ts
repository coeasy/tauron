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
  template: 'vanilla' | 'react' | 'vue' | 'svelte';
  pluginTypes?: PluginType[];
}

export interface PackageManifest {
  name: string;
  version: string;
  description: string;
  type: PluginType;
  main: string;
  permissions: string[];
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
