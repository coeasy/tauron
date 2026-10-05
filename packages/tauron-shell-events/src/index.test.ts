import { describe, expect, it } from 'vitest';

import {
  SHELL_EVENTS,
  SHELL_EVENT_NAMES,
  UPDATER_PRIMARY_ACTION,
  UPDATER_STATUSES,
} from './index.js';

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
    expect([...SHELL_EVENT_NAMES].sort()).toEqual([...new Set(Object.values(SHELL_EVENTS))].sort());
  });

  it('关键事件存在（回归锁：改名即失败）', () => {
    // 这些名字被 ShellController 与组件模板共同依赖，改名必须双方同步。
    expect(SHELL_EVENTS.minimize).toBe('oc-minimize');
    expect(SHELL_EVENTS.maximize).toBe('oc-maximize');
    expect(SHELL_EVENTS.close).toBe('oc-close');
    expect(SHELL_EVENTS.updaterCheck).toBe('oc-updater-check');
    expect(SHELL_EVENTS.updaterDismiss).toBe('oc-updater-dismiss');
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

describe('更新流程状态词表（轮 32：写侧与读侧同源）', () => {
  it('词表就是这 8 个取值，且没有历史平行词表的残留', () => {
    expect([...UPDATER_STATUSES].sort()).toEqual(
      [
        'idle',
        'checking',
        'available',
        'downloading',
        'downloaded',
        'installing',
        'ready',
        'error',
      ].sort(),
    );
    // "done" / "updating" 是 `<oc-updater-dialog>` 旧渲染分支用的名字：全仓没有
    // 任何生产者产出过它们（客户端的已装好终态叫 "ready"），留着只会再养出一套。
    const statuses: readonly string[] = UPDATER_STATUSES;
    expect(statuses).not.toContain('done');
    expect(statuses).not.toContain('updating');
  });

  it('每个状态都有唯一主按钮动作，且动作都是已登记的契约事件', () => {
    expect(Object.keys(UPDATER_PRIMARY_ACTION).sort()).toEqual([...UPDATER_STATUSES].sort());
    for (const [status, action] of Object.entries(UPDATER_PRIMARY_ACTION)) {
      expect(SHELL_EVENT_NAMES, `${status} 映射到未登记的事件 ${action}`).toContain(action);
    }
  });

  it('「立即重启」只由 ready 触发（这条腿在轮 32 之前点不出来）', () => {
    expect(UPDATER_PRIMARY_ACTION.ready).toBe(SHELL_EVENTS.restart);
    // 其余任何状态都不得点亮重启按钮：downloaded / installing 还没装完，
    // error 只表示"这一轮不行"，允许重查而不是重启。
    for (const status of UPDATER_STATUSES.filter((s) => s !== 'ready')) {
      expect(UPDATER_PRIMARY_ACTION[status], `${status} 不该触发重启`).not.toBe(
        SHELL_EVENTS.restart,
      );
    }
    expect(UPDATER_PRIMARY_ACTION.idle).toBe(SHELL_EVENTS.updaterCheck);
    expect(UPDATER_PRIMARY_ACTION.error).toBe(SHELL_EVENTS.updaterCheck);
    expect(UPDATER_PRIMARY_ACTION.available).toBe(SHELL_EVENTS.updateStart);
  });
});
