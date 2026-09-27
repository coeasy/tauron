// ──────────────────────────────────────────────────────────────────────────
// 门禁（1.0-W3）：每个 `SHELL_EVENTS` 成员必须「被监听」或「显式登记为未接线」。
//
// 替代了此前「在注释里写一句『属接入方域』就放行」的做法——那种做法下
// `oc-command-select` 长期零消费者而无人察觉（命令面板点了没反应）。
//
// 解析两侧**源码文本**（与 wire-gate 同风格），并在解析到 0 条时失败，
// 防止正则被改坏导致假绿。
// ──────────────────────────────────────────────────────────────────────────

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import { SHELL_EVENTS } from '@tauron/shell-events';
import { UNWIRED_EVENTS, UNWIRED_EVENT_NAMES } from './unwired-events.js';

/** 去掉行注释与块注释（门禁必须剥注释后再匹配，否则注释里的名字会造假）。 */
function stripComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '');
}

function readSource(relative: string): string {
  return readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8');
}

/** `shell-controller.ts` 里经 `_listen(elements, SHELL_EVENTS.<key>, …)` 接的事件。 */
function listenedKeys(): string[] {
  const source = stripComments(readSource('./shell-controller.ts'));
  const keys = new Set<string>();
  for (const m of source.matchAll(/_listen\(\s*[^,]+,\s*SHELL_EVENTS\.(\w+)/g)) {
    keys.add(m[1] as string);
  }
  return [...keys].sort();
}

/** `@tauron/shell-events` 里 `SHELL_EVENTS` 对象的全部键。 */
function declaredKeys(): string[] {
  const source = stripComments(readSource('../../tauron-shell-events/src/index.ts'));
  const block = /export const SHELL_EVENTS = \{([\s\S]*?)\} as const;/.exec(source)?.[1] ?? '';
  return [...block.matchAll(/^\s*(\w+):\s*'oc-[a-z-]+'/gm)].map((m) => m[1] as string).sort();
}

describe('门禁：壳层事件不允许「有派发、无消费者」', () => {
  // 统一按**事件名**（`oc-*` 字面值）比较：`SHELL_EVENTS` 的键是标识符
  // （`trayItem`），值是事件名（`oc-tray-item`），两者混用会得到假结果。
  const all = SHELL_EVENTS as Record<string, string>;
  const declared = declaredKeys()
    .map((k) => all[k] as string)
    .sort();
  const listened = listenedKeys()
    .map((k) => all[k] as string)
    .sort();
  const unwired = [...UNWIRED_EVENT_NAMES].sort();

  it('解析到的已声明事件数 > 0（防解析失败假绿）', () => {
    expect(declared.length).toBeGreaterThan(0);
  });

  it('事件名解析覆盖 SHELL_EVENTS 全部键（防解析漏项假绿）', () => {
    // `declaredKeys()` 用正则 `'oc-[a-z-]+'` 从源码文本里抠事件名。若哪天某个
    // 成员的值带了数字/大写（如 `oc-theme2`），正则会**静默漏掉**它——而下面
    // 「每个已声明事件都被覆盖」是按 `declared` 逐条查的，漏掉的那条就再也不会
    // 被检查，等于给未来新增事件留了一个假绿口子。所以这里钉死：解析条数必须
    // 等于 `SHELL_EVENTS` 的真实键数。
    expect(declaredKeys().length, '事件名解析漏项（正则可解析数 ≠ SHELL_EVENTS 键数）').toBe(
      Object.keys(SHELL_EVENTS).length,
    );
  });

  it('解析到的已监听事件数 > 0（防解析失败假绿）', () => {
    expect(listened.length).toBeGreaterThan(0);
  });

  it('未接线登记表非空且每条都有原因', () => {
    expect(UNWIRED_EVENTS.length).toBeGreaterThan(0);
    for (const entry of UNWIRED_EVENTS) {
      expect(entry.reason.trim().length, `${entry.name} 缺少未接线原因`).toBeGreaterThan(0);
    }
  });

  it('每个已声明事件都被监听或已登记为未接线', () => {
    const covered = new Set([...listened, ...unwired]);
    const orphans = declared.filter((name) => !covered.has(name));
    expect(
      orphans,
      `以下事件既没被 ShellController 监听、也没登记进 UNWIRED_EVENTS：${orphans.join(', ')}`,
    ).toEqual([]);
  });

  it('监听与未接线登记不得重叠（自相矛盾）', () => {
    const both = listened.filter((name) => unwired.includes(name));
    expect(both, `既已监听又登记为未接线：${both.join(', ')}`).toEqual([]);
  });

  it('oc-command-select 已接（本轮的接线目标，防回退）', () => {
    expect(listened).toContain(SHELL_EVENTS.commandSelect);
    expect(SHELL_EVENTS.commandSelect).toBe('oc-command-select');
  });
});
