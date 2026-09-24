import { describe, expect, it } from 'vitest';

import {
  HOST_ERROR_CODES,
  RETRYABLE_HOST_ERROR_CODES,
  HostException,
  isAppLayerErrorCode,
  isHostErrorCode,
  isRetryable,
  normalizeError,
  translate_at_boundary,
} from './errors.js';

describe('HOST_ERROR_CODES', () => {
  it('有 18 个线上错误码', () => {
    expect(HOST_ERROR_CODES).toHaveLength(18);
  });

  it('订阅表满码在表内且不可自动重试（与其余「表满」类一致）', () => {
    expect(HOST_ERROR_CODES).toContain('E_SUBSCRIPTION_FULL');
    expect(isRetryable('E_SUBSCRIPTION_FULL')).toBe(false);
  });

  it('无重复', () => {
    expect(new Set(HOST_ERROR_CODES).size).toBe(HOST_ERROR_CODES.length);
  });

  it('全部以 E_ 开头（跨 IPC 线名约定）', () => {
    for (const code of HOST_ERROR_CODES) {
      expect(code.startsWith('E_'), `非法错误码线名：${code}`).toBe(true);
      expect(code === code.toUpperCase(), `必须全大写：${code}`).toBe(true);
    }
  });

  it('可重试集合恰好 3 个，且都在线名全集内', () => {
    expect(RETRYABLE_HOST_ERROR_CODES).toHaveLength(3);
    for (const c of RETRYABLE_HOST_ERROR_CODES) {
      expect((HOST_ERROR_CODES as readonly string[])).toContain(c);
    }
  });

  it('isRetryable 对未知线名返回 false（不静默当可重试）', () => {
    expect(isRetryable('E_CALL_TIMEOUT')).toBe(true);
    expect(isRetryable('E_HOST_PANIC')).toBe(true);
    expect(isRetryable('E_AUTH_DENIED')).toBe(false);
    expect(isRetryable('totally-unknown')).toBe(false);
  });

  it('isHostErrorCode 区分已知与未知', () => {
    expect(isHostErrorCode('E_ABI_MISMATCH')).toBe(true);
    expect(isHostErrorCode('E_NAUGHTY')).toBe(false);
  });
});

describe('normalizeError', () => {
  it('标准宿主错误对象 → 保留 code 与 message，算出 retryable', () => {
    const shape = normalizeError({ code: 'E_CALL_TIMEOUT', message: 'call expired' });
    expect(shape.code).toBe('E_CALL_TIMEOUT');
    expect(shape.message).toBe('call expired');
    expect(shape.retryable).toBe(true);
  });

  it('确定性失败标 retryable=false', () => {
    expect(normalizeError({ code: 'E_AUTH_DENIED', message: 'denied' }).retryable).toBe(false);
    expect(normalizeError({ code: 'E_INSTALL_FAILED', message: 'bad sig' }).retryable).toBe(false);
  });

  it('E_PLUGIN_FILTERED 可重试（用户修改配置后重试）', () => {
    expect(normalizeError({ code: 'E_PLUGIN_FILTERED', message: 'filtered' }).retryable).toBe(true);
  });

  it('非标准 code 归入 E_UNKNOWN，不按可重试处理', () => {
    const shape = normalizeError({ code: 'E_NOT_A_REAL_CODE', message: 'x' });
    expect(shape.code).toBe('E_UNKNOWN');
    expect(shape.retryable).toBe(false);
  });

  it('从 Error 消息里抽取线上错误码', () => {
    const shape = normalizeError(new Error('invoke failed: E_PLUGIN_EXISTS: 已存在'));
    expect(shape.code).toBe('E_PLUGIN_EXISTS');
    expect(shape.retryable).toBe(false);
  });

  it('Tauri 层错误对象里含错误码时也能抽取', () => {
    const shape = normalizeError({ message: 'command host_registry_admin failed: E_AUTH_DENIED' });
    expect(shape.code).toBe('E_AUTH_DENIED');
    expect(shape.retryable).toBe(false);
  });

  it('内层消息是 to_tauri_err 的 JSON 字符串 → 还原结构化形状', () => {
    const json = JSON.stringify({
      code: 'E_CALL_TIMEOUT',
      message: 'call expired',
      retryable: true,
    });
    const shape = normalizeError({ message: json });
    expect(shape.code).toBe('E_CALL_TIMEOUT');
    expect(shape.message, '必须取 JSON 内的 message，不是整串转储').toBe('call expired');
    expect(shape.retryable, '必须取宿主给出的 retryable').toBe(true);
  });

  it('Error.message 是 JSON 字符串 → 同样还原', () => {
    const json = JSON.stringify({
      code: 'E_AUTH_DENIED',
      message: 'denied',
      retryable: false,
    });
    const shape = normalizeError(new Error(json));
    expect(shape.code).toBe('E_AUTH_DENIED');
    expect(shape.message).toBe('denied');
  });

  it('JSON 形状不符时不猜，回退正则抽取', () => {
    const msg = '{"code":"SC-0001","message":"not a host error"}';
    const shape = normalizeError({ message: msg });
    expect(shape.code).toBe('E_UNKNOWN');
  });

  it('命令未注册 → E_UNKNOWN 且不可重试（确定性失败）', () => {
    const shape = normalizeError({ message: 'command not found: host_missing' });
    expect(shape.code).toBe('E_UNKNOWN');
    expect(shape.retryable).toBe(false);
  });

  it('纯字符串错误降级为 E_UNKNOWN', () => {
    const shape = normalizeError('boom');
    expect(shape.code).toBe('E_UNKNOWN');
    expect(shape.message).toBe('boom');
    expect(shape.retryable).toBe(false);
  });

  it('对象里没有 code 且 message 无错误码 → E_UNKNOWN（不猜测）', () => {
    const shape = normalizeError({ error: { code: 'E_ABI_MISMATCH' } });
    expect(shape.code).toBe('E_UNKNOWN');
  });

  it('永远返回完整形状（不会抛异常）', () => {
    for (const input of [undefined, null, 0, NaN, {}, [], Symbol('s')]) {
      const shape = normalizeError(input);
      expect(typeof shape.code).toBe('string');
      expect(typeof shape.message).toBe('string');
      expect(typeof shape.retryable).toBe('boolean');
    }
  });
});

describe('HostException', () => {
  it('是可抛可捕的 Error，且保留线名形状', () => {
    let caught: HostException | undefined;
    try {
      throw new HostException({
        code: 'E_STATE_INVALID_TRANSITION',
        rawCode: 'E_STATE_INVALID_TRANSITION',
        message: '非法迁移',
        retryable: false,
      });
    } catch (e) {
      caught = e as HostException;
    }
    expect(caught).toBeInstanceOf(Error);
    expect(caught).toBeInstanceOf(HostException);
    expect(caught?.code).toBe('E_STATE_INVALID_TRANSITION');
    expect(caught?.rawCode).toBe('E_STATE_INVALID_TRANSITION');
    expect(caught?.retryable).toBe(false);
    expect(caught?.name).toBe('HostException');
    expect(caught?.toShape()).toEqual({
      code: 'E_STATE_INVALID_TRANSITION',
      rawCode: 'E_STATE_INVALID_TRANSITION',
      message: '非法迁移',
      retryable: false,
    });
  });
});

describe('未识别错误码的原串保留（版本偏斜下的遥测定位）', () => {
  it('新宿主发来表外码：收窄为 E_UNKNOWN，但原串保留在 rawCode', () => {
    const shape = normalizeError({ code: 'E_QUOTA_EXCEEDED', message: '配额用尽', retryable: true });
    expect(shape.code).toBe('E_UNKNOWN');
    expect(shape.rawCode).toBe('E_QUOTA_EXCEEDED');
    // 宿主显式给了 retryable 就尊重宿主（它比本端的旧码表更懂新码）。
    expect(shape.retryable).toBe(true);
    // 宿主没给 → 回退本端码表；表外码一律按不可重试处理（保守）。
    expect(normalizeError({ code: 'E_QUOTA_EXCEEDED', message: 'x' }).retryable).toBe(false);
  });

  it('已知码：rawCode 与 code 一致', () => {
    const shape = normalizeError({ code: 'E_CALL_TIMEOUT', message: 't', retryable: true });
    expect(shape.code).toBe('E_CALL_TIMEOUT');
    expect(shape.rawCode).toBe('E_CALL_TIMEOUT');
  });

  it('消息里抽出未识别码时保留原串（正则兜底路径）', () => {
    const shape = normalizeError(new Error('host failed: E_WEIRD_TOKEN'));
    expect(shape.code).toBe('E_UNKNOWN');
    expect(shape.rawCode).toBe('E_WEIRD_TOKEN');
  });

  it('消息里根本没有码时为 null（区别于"收到但不认识"）', () => {
    const shape = normalizeError(new Error('boom'));
    expect(shape.code).toBe('E_UNKNOWN');
    expect(shape.rawCode).toBeNull();
    expect(normalizeError('plain failure').rawCode).toBeNull();
  });

  it('JSON 字符串形态同样保留原串', () => {
    const shape = normalizeError('{"code":"E_FUTURE_CODE","message":"m","retryable":false}');
    expect(shape.code).toBe('E_UNKNOWN');
    expect(shape.rawCode).toBe('E_FUTURE_CODE');
    expect(shape.message).toBe('m');
  });

  it('rawCode 参与 toShape 往返', () => {
    const shape = normalizeError({ code: 'E_FUTURE_CODE', message: 'm' });
    expect(new HostException(shape).toShape()).toEqual(shape);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// R2-c：错误词表边界显式化（B3）
// ──────────────────────────────────────────────────────────────────────────
describe('translate_at_boundary（R2-c）', () => {
  it('webview→host 边界：宿主码原样穿过，标记未翻译', () => {
    const r = translate_at_boundary({ code: 'E_AUTH_DENIED', message: 'no perm' }, 'plugin-webview→host');
    expect(r.error).toBeInstanceOf(HostException);
    expect(r.error.code).toBe('E_AUTH_DENIED');
    expect(r.boundary).toBe('plugin-webview→host');
    expect(r.translated).toBe(false);
    expect(r.foreignVocabulary).toBe(false);
    expect(r.rawCode).toBe('E_AUTH_DENIED');
  });

  it('webview→host 边界：应用层码被收窄，但原始码与"发生过翻译"都留痕', () => {
    const r = translate_at_boundary({ code: 'SC-1001', message: 'denied' }, 'plugin-webview→host');
    // 边界之后只有宿主词表可用 → 判定依据是 E_UNKNOWN…
    expect(r.error.code).toBe('E_UNKNOWN');
    // …但绝不能丢原始码，也不能假装"宿主真的返回了 E_UNKNOWN"。
    expect(r.rawCode).toBe('SC-1001');
    expect(r.translated).toBe(true);
    expect(r.foreignVocabulary).toBe(true);
  });

  it('plugin-internal 边界：应用层码保留，不构成"被翻译"', () => {
    const r = translate_at_boundary(new Error('SC-2001: 超时'), 'plugin-internal');
    expect(r.rawCode).toBe('SC-2001');
    expect(r.foreignVocabulary).toBe(true);
    // 插件内部错误不是"穿越边界被改写"，标记必须为 false，否则日志会误报翻译。
    expect(r.translated).toBe(false);
  });

  it('host→plugin-webview 边界：宿主码不被二次翻译', () => {
    const r = translate_at_boundary({ code: 'E_CALL_TIMEOUT', message: 't' }, 'host→plugin-webview');
    expect(r.error.code).toBe('E_CALL_TIMEOUT');
    expect(r.error.retryable).toBe(true);
    expect(r.translated).toBe(false);
  });

  it('两套词表形态互不混淆', () => {
    expect(isAppLayerErrorCode('SC-1001')).toBe(true);
    expect(isAppLayerErrorCode('SC-99999')).toBe(false);
    expect(isAppLayerErrorCode('E_AUTH_DENIED')).toBe(false);
    expect(isHostErrorCode('SC-1001')).toBe(false);
  });

  it('未知形态（无码）也产出可分支的宿主错误', () => {
    const r = translate_at_boundary('boom', 'plugin-webview→host');
    expect(r.error.code).toBe('E_UNKNOWN');
    expect(r.rawCode).toBeNull();
    expect(r.translated).toBe(false); // 没有码 → 谈不上"改写"
    expect(r.error.message).toBe('boom');
  });

  it('翻译结果自带边界元信息，调用方可观测地知道"翻译发生过"', () => {
    const r = translate_at_boundary({ code: 'E_FUTURE_CODE', message: 'm' }, 'plugin-webview→host');
    expect(r).toMatchObject({ boundary: 'plugin-webview→host', translated: true, rawCode: 'E_FUTURE_CODE' });
  });
});
