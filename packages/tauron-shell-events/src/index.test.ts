import { describe, expect, it } from 'vitest';

import { SHELL_EVENTS, SHELL_EVENT_NAMES } from './index.js';

describe('@tauron/shell-events 契约自检', () => {
  it('所有事件名以 oc- 前缀且全小写连字符', () => {
    for (const name of SHELL_EVENT_NAMES) {
      expect(name, `事件名 ${name} 前缀错误`).toMatch(/^oc-[a-z-]+$/);
    }
  });

  it('事件名唯一（防止两个语义共用一个名字）', () => {
    expect(new Set(SHELL_EVENT_NAMES).size).toBe(SHELL_EVENT_NAMES.length);
  });

  it('SHELL_EVENT_NAMES 与 SHELL_EVENTS 值域一致', () => {
    expect([...SHELL_EVENT_NAMES].sort()).toEqual(
      [...new Set(Object.values(SHELL_EVENTS))].sort(),
    );
  });

  it('关键事件存在（回归锁：改名即失败）', () => {
    // 这些名字被 ShellController 与组件模板共同依赖，改名必须双方同步。
    expect(SHELL_EVENTS.minimize).toBe('oc-minimize');
    expect(SHELL_EVENTS.maximize).toBe('oc-maximize');
    expect(SHELL_EVENTS.close).toBe('oc-close');
    expect(SHELL_EVENTS.updaterCheck).toBe('oc-updater-check');
    expect(SHELL_EVENTS.updateStart).toBe('oc-update-start');
    expect(SHELL_EVENTS.restart).toBe('oc-restart');
    expect(SHELL_EVENTS.pluginToggle).toBe('oc-plugin-toggle');
    expect(SHELL_EVENTS.pluginUninstall).toBe('oc-plugin-uninstall');
    expect(SHELL_EVENTS.trayItem).toBe('oc-tray-item');
    expect(SHELL_EVENTS.commandSelect).toBe('oc-command-select');
    expect(SHELL_EVENTS.shortcutChange).toBe('oc-shortcut-change');
    expect(SHELL_EVENTS.themeChange).toBe('oc-theme-change');
    expect(SHELL_EVENTS.toastAction).toBe('oc-toast-action');
  });
});
