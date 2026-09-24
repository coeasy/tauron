import { describe, it, expect, beforeEach } from 'vitest';
import {
  checkPluginPermission,
  grantPermissions,
  revokePermissions,
  getMissingPermissions,
  type PermissionGrants,
} from './acl.js';
import { PluginErrorCode } from '@tauron/types';

describe('ACL', () => {
  let grants: PermissionGrants;

  beforeEach(() => {
    grants = new Map();
  });

  describe('grantPermissions', () => {
    it('grants permissions to a new plugin', () => {
      grantPermissions(grants, 'com.example.test', ['store:read', 'http:fetch'], 'install');

      const grant = grants.get('com.example.test');
      expect(grant).toBeDefined();
      expect(grant?.permissions).toEqual(['store:read', 'http:fetch']);
      expect(grant?.grantedBy).toBe('install');
    });

    it('merges permissions for existing plugin', () => {
      grantPermissions(grants, 'com.example.test', ['store:read'], 'install');
      grantPermissions(grants, 'com.example.test', ['http:fetch', 'store:read'], 'upgrade');

      const grant = grants.get('com.example.test');
      expect(grant?.permissions).toContain('store:read');
      expect(grant?.permissions).toContain('http:fetch');
      expect(grant?.permissions.length).toBe(2); // No duplicates
      expect(grant?.grantedBy).toBe('upgrade');
    });
  });

  describe('revokePermissions', () => {
    it('removes all permissions for a plugin', () => {
      grantPermissions(grants, 'com.example.test', ['store:read'], 'install');
      revokePermissions(grants, 'com.example.test');

      expect(grants.has('com.example.test')).toBe(false);
    });
  });

  describe('checkPluginPermission', () => {
    it('returns null when plugin has all required permissions', () => {
      grantPermissions(grants, 'com.example.test', ['store:read', 'http:fetch'], 'install');

      const error = checkPluginPermission('com.example.test', 'format', ['store:read'], grants);
      expect(error).toBeNull();
    });

    it('returns PLUGIN_NOT_FOUND when plugin is not registered', () => {
      const error = checkPluginPermission('com.unknown.test', 'format', [], grants);
      expect(error).toBe(PluginErrorCode.PLUGIN_NOT_FOUND);
    });

    it('returns PLUGIN_PERMISSION_DENIED when permission is missing', () => {
      grantPermissions(grants, 'com.example.test', ['store:read'], 'install');

      const error = checkPluginPermission('com.example.test', 'format', ['http:fetch'], grants);
      expect(error).toBe(PluginErrorCode.PLUGIN_PERMISSION_DENIED);
    });

    it('allows call with empty required permissions', () => {
      grantPermissions(grants, 'com.example.test', ['store:read'], 'install');

      const error = checkPluginPermission('com.example.test', 'format', [], grants);
      expect(error).toBeNull();
    });
  });

  describe('getMissingPermissions', () => {
    it('returns empty array when all permissions are granted', () => {
      grantPermissions(grants, 'com.example.test', ['store:read', 'http:fetch'], 'install');

      const missing = getMissingPermissions(grants, 'com.example.test', ['store:read']);
      expect(missing).toEqual([]);
    });

    it('returns all permissions when plugin is not registered', () => {
      const missing = getMissingPermissions(grants, 'com.unknown.test', ['store:read', 'http:fetch']);
      expect(missing).toEqual(['store:read', 'http:fetch']);
    });

    it('returns only missing permissions', () => {
      grantPermissions(grants, 'com.example.test', ['store:read'], 'install');

      const missing = getMissingPermissions(grants, 'com.example.test', ['store:read', 'http:fetch', 'clipboard:read']);
      expect(missing).toEqual(['http:fetch', 'clipboard:read']);
    });
  });
});
