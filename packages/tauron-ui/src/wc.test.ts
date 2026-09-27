// @vitest-environment happy-dom

import { describe, expect, it } from 'vitest';

// 示例 app 用的就是这条路径（`examples/minimal-app/src/main.ts`）。
import './wc.js';

/**
 * 门禁（1.0-W2，防 P0-1 回归）。
 *
 * `@tauron/ui/wc` 是示例 app 唯一 import 的 UI 入口。它此前只转出
 * `@tauron/ui-primitives/wc`（只注册 `oc-toast`），导致示例里
 * `<oc-plugin-manager>` 永不 upgrade——插件管理 UI 运行时完全惰性。
 *
 * 这里断言该入口确实注册了壳组件（示例依赖的正是这些标签）。
 */
describe('@tauron/ui/wc 必须注册壳组件（示例 app 的唯一 UI 入口）', () => {
  const REQUIRED = [
    'oc-toast',
    'oc-title-bar',
    'oc-tray-menu',
    'oc-updater-dialog',
    'oc-command-palette',
    'oc-shortcut-recorder',
    'oc-plugin-manager',
  ];

  it.each(REQUIRED)('<%s> 已注册', (tag) => {
    expect(customElements.get(tag), `@tauron/ui/wc 未注册 <${tag}>`).toBeDefined();
  });
});
