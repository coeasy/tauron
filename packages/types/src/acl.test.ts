import { describe, it, expect } from 'vitest';
import {
  PERMISSION_GRANULARITY,
  hasPermission,
  hasAllPermissions,
  missingPermissions,
  isValidPermission,
  type PluginPermissionGrant,
} from './acl.js';

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
      for (const [perm, desc] of Object.entries(PERMISSION_GRANULARITY)) {
        expect(desc).toBeTruthy();
      }
    });
  });

  describe('hasPermission', () => {
    it('returns true for granted permissions', () => {
      expect(hasPermission(grant, 'store:read')).toBe(true);
      expect(hasPermission(grant, 'http:fetch')).toBe(true);
    });

    it('returns false for ungranted permissions', () => {
      expect(hasPermission(grant, 'clipboard:read')).toBe(false);
      expect(hasPermission(grant, 'shell:execute')).toBe(false);
    });
  });

  describe('hasAllPermissions', () => {
    it('returns true when all required permissions are granted', () => {
      expect(hasAllPermissions(grant, ['store:read', 'fs:read-app-dirs'])).toBe(true);
    });

    it('returns false when any permission is missing', () => {
      expect(hasAllPermissions(grant, ['store:read', 'clipboard:read'])).toBe(false);
    });

    it('returns true for empty required list', () => {
      expect(hasAllPermissions(grant, [])).toBe(true);
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

  describe('isValidPermission', () => {
    it('returns true for known permissions', () => {
      expect(isValidPermission('store:read')).toBe(true);
      expect(isValidPermission('http:fetch')).toBe(true);
    });

    it('returns false for unknown permissions', () => {
      expect(isValidPermission('unknown:perm')).toBe(false);
      expect(isValidPermission('')).toBe(false);
    });
  });
});
