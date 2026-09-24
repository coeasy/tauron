// ──────────────────────────────────────────────────────────────────────────
// 插件打包/签名/发布单元测试。
//
// 测试 plugin pack/sign/publish 的核心逻辑。
// ──────────────────────────────────────────────────────────────────────────

import { describe, expect, it } from 'vitest';
import { generateKeyPairSync } from 'node:crypto';
import {
  pluginPack,
  pluginSign,
  pluginPublish,
  validatePackConfig,
  validateSignConfig,
  validatePublishConfig,
  validateSignAlgorithm,
  validateKid,
  validateFiles,
  validateFrameworkRange,
  frameworkMatchesRange,
  verifySignature,
  verifySignatureCrypto,
  generatePackManifest,
  SUPPORTED_SIGN_ALGORITHMS,
  MAX_FILE_SIZE,
  MAX_FILE_COUNT,
  type PluginConfig,
  type PluginFileInfo,
  type PluginSignature,
} from '../src/pack.js';

/** 生成测试用真实 Ed25519 密钥对（PEM 文本）。 */
function makeTestKeys(): { privateKey: string; publicKey: string } {
  const { privateKey, publicKey } = generateKeyPairSync('ed25519', {
    privateKeyEncoding: { type: 'pkcs8', format: 'pem' },
    publicKeyEncoding: { type: 'spki', format: 'pem' },
  });
  return { privateKey: privateKey.toString(), publicKey: publicKey.toString() };
}

// ──────────────────────────────────────────────────────────────────────────
// 测试辅助
// ──────────────────────────────────────────────────────────────────────────

function makePluginConfig(): PluginConfig {
  return {
    id: 'com.example.plugin',
    name: 'My Plugin',
    description: 'A test plugin',
    pluginType: 'js',
    version: '1.0.0',
    permissions: [],
    enabled: true,
  };
}

function makeFiles(count: number = 3): PluginFileInfo[] {
  return Array.from({ length: count }, (_, i) => ({
    path: `src/file${i}.js`,
    size: 1024 * (i + 1),
    hash: 'a'.repeat(64), // 模拟 SHA-256 哈希
  }));
}

// ──────────────────────────────────────────────────────────────────────────
// 验证函数
// ──────────────────────────────────────────────────────────────────────────

describe('validateSignAlgorithm', () => {
  it('接受所有支持的算法', () => {
    for (const algo of SUPPORTED_SIGN_ALGORITHMS) {
      expect(validateSignAlgorithm(algo)).toBe(algo);
    }
  });

  it('拒绝不支持的算法', () => {
    expect(() => validateSignAlgorithm('dsa')).toThrow();
    expect(() => validateSignAlgorithm('ecdsa')).toThrow();
  });
});

describe('validateKid', () => {
  it('接受合法 kid', () => {
    expect(validateKid('kid-123')).toBe('kid-123');
    expect(validateKid('key.id.123')).toBe('key.id.123');
    expect(validateKid('key_id_123')).toBe('key_id_123');
  });

  it('拒绝空 kid', () => {
    expect(() => validateKid('')).toThrow();
    expect(() => validateKid('   ')).toThrow();
  });

  it('拒绝过长 kid', () => {
    expect(() => validateKid('a'.repeat(101))).toThrow();
  });

  it('拒绝包含非法字符', () => {
    expect(() => validateKid('kid/123')).toThrow();
    expect(() => validateKid('kid:123')).toThrow();
  });
});

describe('validateFiles', () => {
  it('接受合法文件列表', () => {
    const files = makeFiles(3);
    expect(validateFiles(files)).toEqual(files);
  });

  it('拒绝空文件列表', () => {
    expect(() => validateFiles([])).toThrow();
  });

  it('拒绝超过文件数上限', () => {
    const files = Array.from({ length: MAX_FILE_COUNT + 1 }, (_, i) => ({
      path: `file${i}.js`,
      size: 1024,
      hash: 'a'.repeat(64),
    }));
    expect(() => validateFiles(files)).toThrow();
  });

  it('拒绝文件路径为空', () => {
    expect(() => validateFiles([{ path: '', size: 1024, hash: 'a'.repeat(64) }])).toThrow();
  });

  it('拒绝包含 .. 的路径', () => {
    expect(() => validateFiles([{ path: '../file.js', size: 1024, hash: 'a'.repeat(64) }])).toThrow();
  });

  it('拒绝绝对路径', () => {
    expect(() => validateFiles([{ path: '/usr/local/file.js', size: 1024, hash: 'a'.repeat(64) }])).toThrow();
  });

  it('拒绝负数大小', () => {
    expect(() => validateFiles([{ path: 'file.js', size: -1, hash: 'a'.repeat(64) }])).toThrow();
  });

  it('拒绝超过文件大小上限', () => {
    expect(() => validateFiles([{ path: 'file.js', size: MAX_FILE_SIZE + 1, hash: 'a'.repeat(64) }])).toThrow();
  });

  it('拒绝格式错误的哈希', () => {
    expect(() => validateFiles([{ path: 'file.js', size: 1024, hash: 'short' }])).toThrow();
    expect(() => validateFiles([{ path: 'file.js', size: 1024, hash: 'Z'.repeat(64) }])).toThrow();
  });
});

describe('validateFrameworkRange', () => {
  it('接受合法范围', () => {
    expect(validateFrameworkRange('>=0.1.0')).toBe('>=0.1.0');
    expect(validateFrameworkRange('<2.0.0')).toBe('<2.0.0');
    expect(validateFrameworkRange('^1.0.0')).toBe('^1.0.0');
    expect(validateFrameworkRange('~1.2.3')).toBe('~1.2.3');
  });

  it('拒绝空范围', () => {
    expect(() => validateFrameworkRange('')).toThrow();
  });

  it('拒绝格式错误的范围', () => {
    expect(() => validateFrameworkRange('>=0.1')).toThrow();
    expect(() => validateFrameworkRange('latest')).toThrow();
  });
});

describe('frameworkMatchesRange', () => {
  it('>= 匹配', () => {
    expect(frameworkMatchesRange('1.0.0', '>=0.1.0')).toBe(true);
    expect(frameworkMatchesRange('0.1.0', '>=0.1.0')).toBe(true);
    expect(frameworkMatchesRange('0.0.9', '>=0.1.0')).toBe(false);
  });

  it('< 匹配', () => {
    expect(frameworkMatchesRange('1.0.0', '<2.0.0')).toBe(true);
    expect(frameworkMatchesRange('2.0.0', '<2.0.0')).toBe(false);
    expect(frameworkMatchesRange('1.9.9', '<2.0.0')).toBe(true);
  });

  it('= 匹配', () => {
    expect(frameworkMatchesRange('1.0.0', '=1.0.0')).toBe(true);
    expect(frameworkMatchesRange('1.0.1', '=1.0.0')).toBe(false);
  });

  it('> 匹配', () => {
    expect(frameworkMatchesRange('1.0.1', '>1.0.0')).toBe(true);
    expect(frameworkMatchesRange('1.0.0', '>1.0.0')).toBe(false);
  });

  it('<= 匹配', () => {
    expect(frameworkMatchesRange('1.0.0', '<=1.0.0')).toBe(true);
    expect(frameworkMatchesRange('1.0.1', '<=1.0.0')).toBe(false);
  });

  it('组合范围匹配', () => {
    expect(frameworkMatchesRange('1.5.0', '>=1.0.0,<2.0.0')).toBe(true);
    expect(frameworkMatchesRange('0.9.0', '>=1.0.0,<2.0.0')).toBe(false);
    expect(frameworkMatchesRange('2.1.0', '>=1.0.0,<2.0.0')).toBe(false);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// validatePackConfig
// ──────────────────────────────────────────────────────────────────────────

describe('validatePackConfig', () => {
  it('完整配置', () => {
    const config = validatePackConfig({
      dir: '/project/plugin',
      outputPath: '/project/plugin/dist/plugin.zip',
      includeSource: true,
      compress: true,
      files: makeFiles(),
      pluginConfig: makePluginConfig(),
    });
    expect(config.dir).toBe('/project/plugin');
    expect(config.outputPath).toBe('/project/plugin/dist/plugin.zip');
    expect(config.includeSource).toBe(true);
    expect(config.compress).toBe(true);
    expect(config.files).toHaveLength(3);
    expect(config.pluginConfig.id).toBe('com.example.plugin');
  });

  it('默认值', () => {
    const config = validatePackConfig({
      dir: '/project/plugin',
      files: makeFiles(),
      pluginConfig: makePluginConfig(),
    });
    expect(config.outputPath).toBe('/project/plugin/dist/com.example.plugin.zip');
    expect(config.includeSource).toBe(false);
    expect(config.compress).toBe(true);
  });

  it('拒绝空目录', () => {
    expect(() => validatePackConfig({
      dir: '',
      files: makeFiles(),
      pluginConfig: makePluginConfig(),
    })).toThrow();
  });

  it('拒绝空文件列表', () => {
    expect(() => validatePackConfig({
      dir: '/project/plugin',
      files: [],
      pluginConfig: makePluginConfig(),
    })).toThrow();
  });

  it('拒绝空插件配置', () => {
    expect(() => validatePackConfig({
      dir: '/project/plugin',
      files: makeFiles(),
    })).toThrow();
  });
});

// ──────────────────────────────────────────────────────────────────────────
// validateSignConfig
// ──────────────────────────────────────────────────────────────────────────

describe('validateSignConfig', () => {
  it('完整配置', () => {
    const config = validateSignConfig({
      dir: '/project/plugin',
      algorithm: 'ed25519',
      kid: 'kid-123',
      privateKey: 'base64-encoded-key',
      includeSource: true,
      frameworkRange: '>=0.1.0',
    });
    expect(config.dir).toBe('/project/plugin');
    expect(config.algorithm).toBe('ed25519');
    expect(config.kid).toBe('kid-123');
    expect(config.privateKey).toBe('base64-encoded-key');
    expect(config.includeSource).toBe(true);
    expect(config.frameworkRange).toBe('>=0.1.0');
  });

  it('默认值', () => {
    const config = validateSignConfig({
      dir: '/project/plugin',
      algorithm: 'ed25519',
      kid: 'kid-123',
      privateKey: 'base64-encoded-key',
    });
    expect(config.includeSource).toBe(false);
    expect(config.frameworkRange).toBe('>=0.1.0');
  });

  it('拒绝空目录', () => {
    expect(() => validateSignConfig({
      dir: '',
      algorithm: 'ed25519',
      kid: 'kid-123',
      privateKey: 'key',
    })).toThrow();
  });

  it('拒绝空算法', () => {
    expect(() => validateSignConfig({
      dir: '/project/plugin',
      algorithm: '',
      kid: 'kid-123',
      privateKey: 'key',
    })).toThrow();
  });

  it('拒绝空 kid', () => {
    expect(() => validateSignConfig({
      dir: '/project/plugin',
      algorithm: 'ed25519',
      kid: '',
      privateKey: 'key',
    })).toThrow();
  });

  it('拒绝空私钥', () => {
    expect(() => validateSignConfig({
      dir: '/project/plugin',
      algorithm: 'ed25519',
      kid: 'kid-123',
      privateKey: '',
    })).toThrow();
  });
});

// ──────────────────────────────────────────────────────────────────────────
// validatePublishConfig
// ──────────────────────────────────────────────────────────────────────────

describe('validatePublishConfig', () => {
  it('完整配置', () => {
    const config = validatePublishConfig({
      dir: '/project/plugin',
      targetUrl: 'https://registry.example.com',
      token: 'token-123',
      overwrite: true,
      includeSignature: true,
    });
    expect(config.dir).toBe('/project/plugin');
    expect(config.targetUrl).toBe('https://registry.example.com');
    expect(config.token).toBe('token-123');
    expect(config.overwrite).toBe(true);
    expect(config.includeSignature).toBe(true);
  });

  it('默认值', () => {
    const config = validatePublishConfig({
      dir: '/project/plugin',
      targetUrl: 'https://registry.example.com',
      token: 'token-123',
    });
    expect(config.overwrite).toBe(false);
    expect(config.includeSignature).toBe(true);
  });

  it('拒绝空目录', () => {
    expect(() => validatePublishConfig({
      dir: '',
      targetUrl: 'https://registry.example.com',
      token: 'token-123',
    })).toThrow();
  });

  it('拒绝空 URL', () => {
    expect(() => validatePublishConfig({
      dir: '/project/plugin',
      targetUrl: '',
      token: 'token-123',
    })).toThrow();
  });

  it('拒绝非 HTTPS URL', () => {
    expect(() => validatePublishConfig({
      dir: '/project/plugin',
      targetUrl: 'http://registry.example.com',
      token: 'token-123',
    })).toThrow();
  });

  it('拒绝空 token', () => {
    expect(() => validatePublishConfig({
      dir: '/project/plugin',
      targetUrl: 'https://registry.example.com',
      token: '',
    })).toThrow();
  });
});

// ──────────────────────────────────────────────────────────────────────────
// pluginPack
// ──────────────────────────────────────────────────────────────────────────

describe('pluginPack', () => {
  it('成功打包', () => {
    const result = pluginPack({
      dir: '/project/plugin',
      includeSource: true,
      compress: true,
      files: makeFiles(5),
      pluginConfig: makePluginConfig(),
    });
    expect(result.ok).toBe(true);
    expect(result.outputPath).toBe('/project/plugin/dist/com.example.plugin.zip');
    expect(result.files).toHaveLength(5);
    expect(result.totalSize).toBeGreaterThan(0);
  });

  it('失败时返回错误', () => {
    const result = pluginPack({
      dir: '',
      includeSource: true,
      compress: true,
      files: makeFiles(),
      pluginConfig: makePluginConfig(),
    });
    expect(result.ok).toBe(false);
    expect(result.error).toBeDefined();
  });
});

// ──────────────────────────────────────────────────────────────────────────
// pluginSign
// ──────────────────────────────────────────────────────────────────────────

describe('pluginSign', () => {
  it('成功签名', async () => {
    const keys = makeTestKeys();
    const result = await pluginSign({
      dir: '/project/plugin',
      algorithm: 'ed25519',
      kid: 'kid-123',
      privateKey: keys.privateKey,
      includeSource: true,
      frameworkRange: '>=0.1.0',
    }, makeFiles(3));
    expect(result.ok).toBe(true);
    const signature = result.signature;
    expect(signature).toBeDefined();
    if (signature === undefined) {
      throw new Error('签名失败：结果缺少 signature');
    }
    expect(signature.algorithm).toBe('ed25519');
    expect(signature.kid).toBe('kid-123');
    expect(signature.issuedAt).toBeDefined();
    expect(signature.signature).toBeDefined();
    expect(signature.files).toHaveLength(3);
  });

  it('签名与声明算法一致：公钥可独立验证（回路）', async () => {
    const keys = makeTestKeys();
    const files = makeFiles(3);
    const result = await pluginSign({
      dir: '/project/plugin',
      algorithm: 'ed25519',
      kid: 'kid-123',
      privateKey: keys.privateKey,
    }, files);
    expect(result.ok).toBe(true);
    const signature = result.signature!;
    expect(await verifySignatureCrypto(signature, keys.publicKey)).toBe(true);
  });

  it('篡改文件列表后密码学验证失败', async () => {
    const keys = makeTestKeys();
    const files = makeFiles(3);
    const result = await pluginSign({
      dir: '/project/plugin',
      algorithm: 'ed25519',
      kid: 'kid-123',
      privateKey: keys.privateKey,
    }, files);
    const signature = result.signature!;
    // 注意：makeFiles 的 hash 全部相同，必须改变 hash 内容本身才是真篡改
    const tampered: PluginSignature = {
      ...signature,
      files: signature.files.map((f, i) =>
        i === 0 ? { ...f, hash: 'b'.repeat(64) } : f,
      ),
    };
    expect(await verifySignatureCrypto(tampered, keys.publicKey)).toBe(false);
    // 原签名对原始载荷仍然有效（防误伤）
    expect(await verifySignatureCrypto(signature, keys.publicKey)).toBe(true);
  });

  it('无效私钥格式被拒绝', async () => {
    const result = await pluginSign({
      dir: '/project/plugin',
      algorithm: 'ed25519',
      kid: 'kid-123',
      privateKey: 'not-a-real-key',
    }, makeFiles(2));
    expect(result.ok).toBe(false);
    expect(result.error).toContain('私钥格式无效');
  });

  it('rsa-* 显式拒绝：@tauron/market 只实现 ed25519（不静默降级）', async () => {
    const keys = makeTestKeys();
    const result = await pluginSign({
      dir: '/project/plugin',
      algorithm: 'rsa-2048',
      kid: 'kid-123',
      privateKey: keys.privateKey,
    }, makeFiles(2));
    expect(result.ok).toBe(false);
    expect(result.error).toContain('@tauron/market 仅实现 ed25519');
  });

  it('失败时返回错误', async () => {
    const keys = makeTestKeys();
    const result = await pluginSign({
      dir: '',
      algorithm: 'ed25519',
      kid: 'kid-123',
      privateKey: keys.privateKey,
      includeSource: true,
      frameworkRange: '>=0.1.0',
    }, makeFiles(3));
    expect(result.ok).toBe(false);
    expect(result.error).toBeDefined();
  });
});

// ──────────────────────────────────────────────────────────────────────────
// verifySignature
// ──────────────────────────────────────────────────────────────────────────

describe('verifySignature', () => {
  const validSignature: PluginSignature = {
    algorithm: 'ed25519',
    kid: 'kid-123',
    issuedAt: '2024-01-01T00:00:00.000Z',
    signature: 'a'.repeat(128), // Ed25519 签名 hex（结构验证不校验真伪）
    files: makeFiles(3),
  };

  it('验证有效签名', () => {
    const result = verifySignature(validSignature);
    expect(result.ok).toBe(true);
  });

  it('拒绝空算法', () => {
    const result = verifySignature({ ...validSignature, algorithm: '' as unknown as 'ed25519' });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('签名算法');
  });

  it('拒绝不支持的算法', () => {
    const result = verifySignature({ ...validSignature, algorithm: 'dsa' as unknown as 'ed25519' });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('不支持的签名算法');
  });

  it('拒绝空 kid', () => {
    const result = verifySignature({ ...validSignature, kid: '' });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('密钥 ID');
  });

  it('拒绝空签发时间', () => {
    const result = verifySignature({ ...validSignature, issuedAt: '' });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('签发时间');
  });

  it('拒绝空签名内容', () => {
    const result = verifySignature({ ...validSignature, signature: '' });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('签名内容');
  });

  it('拒绝空文件列表', () => {
    const result = verifySignature({ ...validSignature, files: [] });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('文件列表');
  });

  it('拒绝格式错误的哈希', () => {
    const files = [{ path: 'file.js', size: 1024, hash: 'invalid' }];
    const result = verifySignature({ ...validSignature, files });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('哈希格式');
  });
});

// ──────────────────────────────────────────────────────────────────────────
// pluginPublish
// ──────────────────────────────────────────────────────────────────────────

describe('pluginPublish', () => {
  it('成功发布', () => {
    const result = pluginPublish({
      dir: '/project/plugin',
      targetUrl: 'https://registry.example.com',
      token: 'token-123',
      overwrite: true,
      includeSignature: true,
    });
    expect(result.ok).toBe(true);
    expect(result.url).toBe('https://registry.example.com/plugin');
  });

  it('失败时返回错误', () => {
    const result = pluginPublish({
      dir: '',
      targetUrl: 'https://registry.example.com',
      token: 'token-123',
      overwrite: true,
      includeSignature: true,
    });
    expect(result.ok).toBe(false);
    expect(result.error).toBeDefined();
  });

  it('拒绝非 HTTPS URL', () => {
    const result = pluginPublish({
      dir: '/project/plugin',
      targetUrl: 'http://registry.example.com',
      token: 'token-123',
      overwrite: true,
      includeSignature: true,
    });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('HTTPS');
  });
});

// ──────────────────────────────────────────────────────────────────────────
// generatePackManifest
// ──────────────────────────────────────────────────────────────────────────

describe('generatePackManifest', () => {
  it('生成有效的打包清单', () => {
    const config = {
      dir: '/project/plugin',
      outputPath: '/project/plugin/dist/plugin.zip',
      includeSource: true,
      compress: true,
      files: makeFiles(3),
      pluginConfig: makePluginConfig(),
    };
    const manifest = JSON.parse(generatePackManifest(config));
    expect(manifest.version).toBe(1);
    expect(manifest.pluginId).toBe('com.example.plugin');
    expect(manifest.pluginName).toBe('My Plugin');
    expect(manifest.pluginVersion).toBe('1.0.0');
    expect(manifest.pluginType).toBe('js');
    expect(manifest.files).toHaveLength(3);
    expect(manifest.totalSize).toBeGreaterThan(0);
    expect(manifest.includeSource).toBe(true);
    expect(manifest.compress).toBe(true);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 常量
// ──────────────────────────────────────────────────────────────────────────

describe('常量', () => {
  it('SUPPORTED_SIGN_ALGORITHMS 包含 3 种算法', () => {
    expect(SUPPORTED_SIGN_ALGORITHMS).toHaveLength(3);
    expect(SUPPORTED_SIGN_ALGORITHMS).toContain('ed25519');
    expect(SUPPORTED_SIGN_ALGORITHMS).toContain('rsa-2048');
    expect(SUPPORTED_SIGN_ALGORITHMS).toContain('rsa-4096');
  });

  it('MAX_FILE_SIZE 为 100MB', () => {
    expect(MAX_FILE_SIZE).toBe(100 * 1024 * 1024);
  });

  it('MAX_FILE_COUNT 为 2000', () => {
    expect(MAX_FILE_COUNT).toBe(2000);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 端到端测试
// ──────────────────────────────────────────────────────────────────────────

describe('端到端：plugin pack/sign/publish', () => {
  it('完整流程：打包 → 签名 → 发布', async () => {
    const files = makeFiles(5);
    const pluginConfig = makePluginConfig();

    // 1. 打包
    const packResult = pluginPack({
      dir: '/project/plugin',
      includeSource: true,
      compress: true,
      files,
      pluginConfig,
    });
    expect(packResult.ok).toBe(true);
    const packFiles = packResult.files ?? [];
    expect(packFiles).toHaveLength(5);
    expect(packResult.totalSize).toBeGreaterThan(0);

    // 2. 签名
    const keys = makeTestKeys();
    const signResult = await pluginSign({
      dir: '/project/plugin',
      algorithm: 'ed25519',
      kid: 'kid-123',
      privateKey: keys.privateKey,
      includeSource: true,
      frameworkRange: '>=0.1.0',
    }, packFiles);
    expect(signResult.ok).toBe(true);
    const signature = signResult.signature;
    expect(signature).toBeDefined();
    if (signature === undefined) {
      throw new Error('签名失败：结果缺少 signature');
    }
    expect(signature.algorithm).toBe('ed25519');
    expect(signature.files).toHaveLength(5);

    // 3. 验证签名（结构 + 密码学回路）
    const verifyResult = verifySignature(signature);
    expect(verifyResult.ok).toBe(true);
    expect(await verifySignatureCrypto(signature, keys.publicKey)).toBe(true);

    // 4. 发布
    const publishResult = pluginPublish({
      dir: '/project/plugin',
      targetUrl: 'https://registry.example.com',
      token: 'token-123',
      overwrite: false,
      includeSignature: true,
    });
    expect(publishResult.ok).toBe(true);
    expect(publishResult.url).toBe('https://registry.example.com/plugin');
  });

  it('framework 版本范围验证', () => {
    // 验证插件的 framework range 是否匹配
    expect(frameworkMatchesRange('1.0.0', '>=0.1.0,<2.0.0')).toBe(true);
    expect(frameworkMatchesRange('2.0.0', '>=0.1.0,<2.0.0')).toBe(false);
    expect(frameworkMatchesRange('0.5.0', '>=0.1.0,<2.0.0')).toBe(true);

    // 验证格式
    expect(() => validateFrameworkRange('>=0.1.0,<2.0.0')).not.toThrow();
    expect(() => validateFrameworkRange('invalid')).toThrow();
  });
});
