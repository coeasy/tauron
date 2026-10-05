import { describe, it, expect } from 'vitest';
import {
  PluginErrorCode,
  RETRYABLE_ERROR_CODES,
  ERROR_CATEGORIES,
  isRetryable,
  errorCategory,
  errorMessage,
  isValidErrorCode,
  isAppLayerErrorCode,
  isCodeLike,
  extractCodeLike,
} from './errors.js';

describe('errors', () => {
  describe('PluginErrorCode', () => {
    it('has exactly 14 error codes', () => {
      const codes = Object.values(PluginErrorCode);
      expect(codes.length).toBe(14);
    });

    it('all codes follow SC-xxxx format', () => {
      for (const code of Object.values(PluginErrorCode)) {
        expect(code).toMatch(/^SC-\d{4}$/);
      }
    });

    it('plugin errors are 0xxx', () => {
      expect(PluginErrorCode.PLUGIN_NOT_FOUND).toBe('SC-0001');
      expect(PluginErrorCode.PLUGIN_DISABLED).toBe('SC-0002');
      expect(PluginErrorCode.PLUGIN_ERRORED).toBe('SC-0003');
    });

    it('permission errors are 1xxx', () => {
      expect(PluginErrorCode.PERMISSION_DENIED).toBe('SC-1001');
      expect(PluginErrorCode.PLUGIN_PERMISSION_DENIED).toBe('SC-1002');
      expect(PluginErrorCode.CAPABILITY_REQUIRED).toBe('SC-1003');
    });

    it('communication errors are 2xxx', () => {
      expect(PluginErrorCode.TIMEOUT).toBe('SC-2001');
      expect(PluginErrorCode.CANCELLED).toBe('SC-2002');
      expect(PluginErrorCode.INVALID_PAYLOAD).toBe('SC-2003');
      expect(PluginErrorCode.CHANNEL_BROKEN).toBe('SC-2004');
    });

    it('implementation errors are 3xxx', () => {
      expect(PluginErrorCode.PLUGIN_PANIC).toBe('SC-3001');
      expect(PluginErrorCode.PLUGIN_OOM).toBe('SC-3002');
      expect(PluginErrorCode.PLUGIN_EXITED).toBe('SC-3003');
    });

    it('system errors are 9xxx', () => {
      expect(PluginErrorCode.INTERNAL).toBe('SC-9001');
    });
  });

  describe('RETRYABLE_ERROR_CODES', () => {
    it('contains TIMEOUT, CHANNEL_BROKEN, INTERNAL', () => {
      expect(RETRYABLE_ERROR_CODES.has(PluginErrorCode.TIMEOUT)).toBe(true);
      expect(RETRYABLE_ERROR_CODES.has(PluginErrorCode.CHANNEL_BROKEN)).toBe(true);
      expect(RETRYABLE_ERROR_CODES.has(PluginErrorCode.INTERNAL)).toBe(true);
    });

    it('does not contain non-retryable errors', () => {
      expect(RETRYABLE_ERROR_CODES.has(PluginErrorCode.PLUGIN_NOT_FOUND)).toBe(false);
      expect(RETRYABLE_ERROR_CODES.has(PluginErrorCode.CANCELLED)).toBe(false);
      expect(RETRYABLE_ERROR_CODES.has(PluginErrorCode.INVALID_PAYLOAD)).toBe(false);
    });
  });

  describe('isRetryable', () => {
    it('returns true for retryable errors', () => {
      expect(isRetryable(PluginErrorCode.TIMEOUT)).toBe(true);
      expect(isRetryable(PluginErrorCode.CHANNEL_BROKEN)).toBe(true);
      expect(isRetryable(PluginErrorCode.INTERNAL)).toBe(true);
    });

    it('returns false for non-retryable errors', () => {
      expect(isRetryable(PluginErrorCode.PLUGIN_NOT_FOUND)).toBe(false);
      expect(isRetryable(PluginErrorCode.CANCELLED)).toBe(false);
    });
  });

  describe('errorCategory', () => {
    it('classifies errors correctly', () => {
      expect(errorCategory(PluginErrorCode.PLUGIN_NOT_FOUND)).toBe('plugin');
      expect(errorCategory(PluginErrorCode.PERMISSION_DENIED)).toBe('permission');
      expect(errorCategory(PluginErrorCode.TIMEOUT)).toBe('communication');
      expect(errorCategory(PluginErrorCode.PLUGIN_PANIC)).toBe('implementation');
      expect(errorCategory(PluginErrorCode.INTERNAL)).toBe('system');
    });
  });

  describe('errorMessage', () => {
    it('returns human-readable messages', () => {
      expect(errorMessage(PluginErrorCode.PLUGIN_NOT_FOUND)).toBe(
        'Plugin not found or not registered',
      );
      expect(errorMessage(PluginErrorCode.TIMEOUT)).toBe('Call timed out');
    });

    it('every error code has a message', () => {
      for (const code of Object.values(PluginErrorCode)) {
        expect(errorMessage(code)).toBeTruthy();
      }
    });
  });

  describe('ERROR_CATEGORIES', () => {
    it('has an entry for every error code', () => {
      for (const code of Object.values(PluginErrorCode)) {
        expect(ERROR_CATEGORIES[code]).toBeDefined();
      }
    });
  });

  describe('isValidErrorCode', () => {
    it('returns true for valid codes', () => {
      expect(isValidErrorCode('SC-0001')).toBe(true);
      expect(isValidErrorCode('SC-9001')).toBe(true);
    });

    it('returns false for invalid codes', () => {
      expect(isValidErrorCode('SC-9999')).toBe(false);
      expect(isValidErrorCode('INVALID')).toBe(false);
      expect(isValidErrorCode('')).toBe(false);
    });
  });

  // 轮 35：两套词表（宿主 `E_*` / 框架层 `SC-####`）穿越 iframe 桥时的"保码"
  // 判据只有这一份，两侧共用；这里把**形态**与**成员资格**的边界钉住——
  // 本层只认形态，合法集仍归各层自己的清单。
  describe('码形态判据（isCodeLike / extractCodeLike）', () => {
    it('识别两套词表的形态', () => {
      expect(isCodeLike('E_LEASE_EXPIRED')).toBe(true);
      expect(isCodeLike('SC-2001')).toBe(true);
      expect(isAppLayerErrorCode('SC-2001')).toBe(true);
    });

    it('不把普通文本或错形态当成码', () => {
      for (const v of ['', 'INTERNAL', 'SC-12', 'SC-12345', 'e_lower', 'SC_2001', '500']) {
        expect(isCodeLike(v), `${v} 不是码形态`).toBe(false);
      }
      // 形态判据故意宽：不在应用层词表内的 SC 码仍是"码形态"，成员资格由
      // isValidErrorCode 负责（两者不可互相替代）。
      expect(isCodeLike('SC-9999')).toBe(true);
      expect(isValidErrorCode('SC-9999')).toBe(false);
    });

    it('从消息文本里抽码，两处都没有返回 null', () => {
      expect(extractCodeLike('调用失败：E_PLUGIN_DISABLED')).toBe('E_PLUGIN_DISABLED');
      expect(extractCodeLike('boom (SC-2004)')).toBe('SC-2004');
      expect(extractCodeLike('彻底没有码')).toBeNull();
      expect(extractCodeLike('E_')).toBeNull();
    });

    it('全枚举值都过形态判据（词表与判据不可漂移）', () => {
      for (const code of Object.values(PluginErrorCode)) {
        expect(isAppLayerErrorCode(code), `${code} 应匹配 SC-#### 形态`).toBe(true);
      }
    });
  });
});
