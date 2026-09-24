// UpdaterDialog 状态管理测试

import { describe, expect, it, beforeEach } from 'vitest';
import { UpdaterStore } from './updater-dialog';

describe('UpdaterStore', () => {
  let store: UpdaterStore;

  beforeEach(() => {
    store = new UpdaterStore({ currentVersion: '1.0.0' });
  });

  describe('初始状态', () => {
    it('默认状态为 idle', () => {
      expect(store.state.status).toBe('idle');
      if (store.state.status === 'idle') expect(store.state.currentVersion).toBe('1.0.0');
    });

    it('支持自定义配置', () => {
      const store = new UpdaterStore({ currentVersion: '2.0.0', channel: 'beta' });
      expect(store.config.currentVersion).toBe('2.0.0');
      expect(store.config.channel).toBe('beta');
    });
  });

  describe('checkForUpdate', () => {
    it('正常检查更新', () => {
      const result = store.checkForUpdate();
      expect(result.ok).toBe(true);
      expect(store.state.status).toBe('checking');
    });

    it('正在检查时拒绝', () => {
      store.checkForUpdate();
      const result = store.checkForUpdate();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_ALREADY_CHECKING');
    });

    it('正在下载时拒绝', () => {
      store.checkForUpdate();
      store.setUpdateAvailable({
        version: '2.0.0',
        changelog: ['更新'],
        releaseDate: '2024-01-01',
        downloadUrl: 'https://example.com/update',
        size: 1000,
        signature: 'sig',
      });
      store.startDownload();
      const result = store.checkForUpdate();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_ALREADY_UPDATING');
    });
  });

  describe('setUpdateAvailable', () => {
    it('设置可用更新', () => {
      store.checkForUpdate();
      store.setUpdateAvailable({
        version: '2.0.0',
        changelog: ['新功能'],
        releaseDate: '2024-01-01',
        downloadUrl: 'https://example.com/update',
        size: 1000,
        signature: 'sig',
      });
      expect(store.state.status).toBe('available');
      if (store.state.status === 'available') {
        expect(store.state.updateVersion).toBe('2.0.0');
        expect(store.state.changelog).toContain('新功能');
      }
    });

    it('非检查状态时忽略', () => {
      store.setUpdateAvailable({
        version: '2.0.0',
        changelog: [],
        releaseDate: '',
        downloadUrl: '',
        size: 0,
        signature: '',
      });
      expect(store.state.status).toBe('idle');
    });
  });

  describe('startDownload', () => {
    it('正常开始下载', () => {
      store.checkForUpdate();
      store.setUpdateAvailable({
        version: '2.0.0',
        changelog: [],
        releaseDate: '',
        downloadUrl: '',
        size: 1000,
        signature: 'sig',
      });
      const result = store.startDownload();
      expect(result.ok).toBe(true);
      expect(store.state.status).toBe('downloading');
    });

    it('非 available 状态时拒绝', () => {
      const result = store.startDownload();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_AVAILABLE');
    });
  });

  describe('cancelDownload', () => {
    it('正常取消下载', () => {
      store.checkForUpdate();
      store.setUpdateAvailable({
        version: '2.0.0',
        changelog: [],
        releaseDate: '',
        downloadUrl: '',
        size: 1000,
        signature: 'sig',
      });
      store.startDownload();
      const result = store.cancelDownload();
      expect(result.ok).toBe(true);
      expect(store.state.status).toBe('available');
    });

    it('非下载状态时拒绝', () => {
      const result = store.cancelDownload();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_DOWNLOADING');
    });
  });

  describe('startInstall/completeInstall', () => {
    it('正常安装流程', () => {
      store.checkForUpdate();
      store.setUpdateAvailable({
        version: '2.0.0',
        changelog: [],
        releaseDate: '',
        downloadUrl: '',
        size: 1000,
        signature: 'sig',
      });
      store.startDownload();
      store.completeDownload(1000);
      expect(store.state.status).toBe('downloaded');
      const result = store.startInstall();
      expect(result.ok).toBe(true);
      expect(store.state.status).toBe('installing');
      store.completeInstall();
      expect(store.state.status).toBe('restarting');
    });

    it('未下载时拒绝安装', () => {
      const result = store.startInstall();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_DOWNLOADED');
    });
  });

  describe('cancelInstall', () => {
    it('正常取消安装', () => {
      store.checkForUpdate();
      store.setUpdateAvailable({
        version: '2.0.0',
        changelog: [],
        releaseDate: '',
        downloadUrl: '',
        size: 1000,
        signature: 'sig',
      });
      store.startDownload();
      store.completeDownload(1000);
      store.startInstall();
      const result = store.cancelInstall();
      expect(result.ok).toBe(true);
      expect(store.state.status).toBe('downloaded');
    });
  });

  describe('dismiss', () => {
    it('忽略更新', () => {
      store.checkForUpdate();
      store.setUpdateAvailable({
        version: '2.0.0',
        changelog: [],
        releaseDate: '',
        downloadUrl: '',
        size: 1000,
        signature: 'sig',
      });
      const result = store.dismiss();
      expect(result.ok).toBe(true);
      expect(store.state.status).toBe('idle');
    });

    it('非 available 状态时拒绝', () => {
      const result = store.dismiss();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_AVAILABLE');
    });
  });

  describe('setError/reset', () => {
    it('设置错误', () => {
      store.setError('network error');
      expect(store.state.status).toBe('error');
      if (store.state.status === 'error') expect(store.state.error).toBe('network error');
    });

    it('重置', () => {
      store.setError('error');
      store.reset();
      expect(store.state.status).toBe('idle');
    });
  });
});
