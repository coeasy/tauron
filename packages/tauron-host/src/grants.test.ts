import type { GrantSet, ScopeChange } from './grants.js';
import {
  GRANT_SET_SCHEMA_VERSION,
  IDENTITY_LABEL_PREFIX,
  capabilityOfGrantSet,
  requiresReapproval,
  scopeGrew,
} from './grants.js';

import { describe, expect, it } from 'vitest';

const gs = (patch: Partial<GrantSet> = {}): GrantSet => ({
  schemaVersion: GRANT_SET_SCHEMA_VERSION,
  revision: 1,
  pluginId: 'com.example.formatter',
  pluginVersion: '1.0.0',
  framework: '>=2.0 <3.0',
  grants: [
    { permission: 'http:allow-fetch', risk: 'elevated', description: 'x', scoped: true },
  ],
  scopes: { 'http:allow-fetch': ['https://api.example.com/**'] },
  approvedAtUnix: 1_700_000_000,
  approver: 'user',
  ...patch,
});

describe('scopeGrew', () => {
  it('从无到有算变宽', () => {
    expect(scopeGrew(undefined, ['a'])).toBe(true);
  });

  it('两个都没有不算变宽', () => {
    expect(scopeGrew(undefined, undefined)).toBe(false);
  });

  it('集合变大算变宽', () => {
    expect(scopeGrew(['a'], ['a', 'b'])).toBe(true);
  });

  it('严格子集不算变宽', () => {
    expect(scopeGrew(['a', 'b'], ['a'])).toBe(false);
  });

  it('同长度但含新条目算变宽', () => {
    expect(scopeGrew(['a'], ['b'])).toBe(true);
  });

  it('仅顺序不同不算变宽', () => {
    expect(scopeGrew(['a', 'b'], ['b', 'a'])).toBe(false);
  });

  it('单字符串形式等价于单元素数组', () => {
    expect(scopeGrew('a', 'a')).toBe(false);
    expect(scopeGrew('a', ['a', 'b'])).toBe(true);
    expect(scopeGrew(['a', 'b'], 'a')).toBe(false);
  });

  it('新 scope 非数组/非字符串时不算变宽', () => {
    expect(scopeGrew(['a'], null)).toBe(false);
    expect(scopeGrew(['a'], { nope: 1 })).toBe(false);
  });
});

describe('requiresReapproval', () => {
  const base = {
    added: [] as string[],
    removed: [] as string[],
    unchanged: [] as string[],
    scopeChanges: [] as ScopeChange[],
    frameworkChanged: false,
    pluginVersionChanged: false,
    requiresReapproval: false,
  };
  const sc = (grew: boolean): ScopeChange => ({ permission: 'http:allow-fetch', grew });

  it('新增权限必须重审', () => {
    expect(requiresReapproval({ ...base, added: ['store:allow-set'] })).toBe(true);
  });

  it('scope 变宽必须重审', () => {
    expect(requiresReapproval({ ...base, scopeChanges: [sc(true)] })).toBe(true);
  });

  it('仅收缩不重审（ADR-05 撤销语义如实）', () => {
    expect(
      requiresReapproval({ ...base, removed: ['shell:allow-open'], scopeChanges: [sc(false)] }),
    ).toBe(false);
  });

  it('仅框架/版本漂移不重审（计划 §4.5 只列权限与 scope）', () => {
    expect(
      requiresReapproval({ ...base, frameworkChanged: true, pluginVersionChanged: true }),
    ).toBe(false);
  });

  it('完全无变更不重审', () => {
    expect(requiresReapproval(base)).toBe(false);
  });
});

describe('capabilityOfGrantSet', () => {
  it('标识与 webview 都用插件身份 label 前缀', () => {
    const cap = capabilityOfGrantSet(gs());
    expect(cap.identifier).toBe(`${IDENTITY_LABEL_PREFIX}com.example.formatter`);
    expect(cap.webviews).toEqual([`${IDENTITY_LABEL_PREFIX}com.example.formatter`]);
    expect(cap.permissions).toEqual(['http:allow-fetch']);
  });

  it('权限顺序与授予集一致', () => {
    const cap = capabilityOfGrantSet(
      gs({
        grants: [
          { permission: 'a:b', risk: 'low', description: '', scoped: false },
          { permission: 'c:d', risk: 'high', description: '', scoped: true },
        ],
      }),
    );
    expect(cap.permissions).toEqual(['a:b', 'c:d']);
  });

  it('空授予集产出空权限列表', () => {
    expect(capabilityOfGrantSet(gs({ grants: [] })).permissions).toEqual([]);
  });
});

describe('线名常量', () => {
  it('授予集 schema 版本是当前支持的版本', () => {
    expect(GRANT_SET_SCHEMA_VERSION).toBe(1);
  });

  it('身份 label 前缀固定', () => {
    expect(IDENTITY_LABEL_PREFIX).toBe('plugin-');
  });
});
