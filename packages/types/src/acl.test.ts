import { describe, it, expect } from 'vitest';
import { PERMISSION_GRANULARITY, missingPermissions, type PluginPermissionGrant } from './acl.js';

describe('ACL', () => {
  const grant: PluginPermissionGrant = {
    pluginId: 'com.example.test',
    permissions: ['store:read', 'store:write', 'fs:read-app-dirs', 'http:fetch'],
    grantedAt: '2026-01-01T00:00:00Z',
    grantedBy: 'install',
  };

  describe('PERMISSION_GRANULARITY', () => {
    it('has at least 20 permissions defined', () => {
      expect(Object.keys(PERMISSION_GRANULARITY).length).toBeGreaterThanOrEqual(20);
    });

    it('every permission has a description', () => {
      for (const desc of Object.values(PERMISSION_GRANULARITY)) {
        expect(desc).toBeTruthy();
      }
    });
  });

  describe('missingPermissions', () => {
    it('returns empty array when all permissions are granted', () => {
      expect(missingPermissions(grant, ['store:read', 'store:write'])).toEqual([]);
    });

    it('returns missing permissions', () => {
      expect(missingPermissions(grant, ['store:read', 'clipboard:read', 'shell:execute'])).toEqual([
        'clipboard:read',
        'shell:execute',
      ]);
    });
  });

  // 轮 58：内层词表只留**有读者**的导出。曾经与它并列的 `hasPermission` /
  // `hasAllPermissions` / `isValidPermission` 在接线面零读者，而 `isValidPermission`
  // 是唯一读 `PERMISSION_GRANULARITY` 的函数——删掉它之后，「这张表被谁执行」必须
  // 由 acl.ts 的注释如实回答（答案：仓库内没有执行者），而不是由一个无人调用的
  // 校验函数假装回答。
  describe('导出面（轮 58）', () => {
    it('@tauron/types 不再导出零读者的内层工具', async () => {
      const mod = (await import('./index.js')) as Record<string, unknown>;
      expect(typeof mod.missingPermissions).toBe('function');
      expect(mod.PERMISSION_GRANULARITY).toBeTruthy();
      for (const gone of ['hasPermission', 'hasAllPermissions', 'isValidPermission']) {
        expect(mod[gone], `@tauron/types 又导出了一份没人用的 ${gone}`).toBeUndefined();
      }
    });
  });
});
