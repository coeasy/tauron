/**
 * @tauron/shell-matrix — tauron Shell 矩阵
 *
 * 设计文档引用：
 * - §7.2 Shell 矩阵
 * - §7.2.1 local shell
 * - §7.2.2 local-server shell
 * - §7.2.3 remote-url shell
 * - §7.2.4 sub-webview shell
 */

// ---- Types ----
export type {
  ShellForm,
  ShellConfig,
  LocalShellConfig,
  LocalServerShellConfig,
  RemoteUrlShellConfig,
  SubWebviewShellConfig,
  ShellInstance,
  ShellManager,
} from './types.js';

// ---- Manager ----
export { createShellManager } from './manager.js';
