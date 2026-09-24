import { describe, expect, it, beforeEach, afterEach } from 'vitest';
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as os from 'node:os';

import {
  ensureDir,
  writeFile,
  writeFiles,
  readJsonFile,
  writeJsonFile,
  pathExists,
  listFiles,
  copyFile,
} from './fs-operations.js';

describe('fs-operations', () => {
  let tmpDir: string;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'oc-test-'));
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  describe('ensureDir', () => {
    it('创建不存在的目录', () => {
      const dir = path.join(tmpDir, 'sub', 'dir');
      const result = ensureDir(dir);
      expect(result.ok).toBe(true);
      expect(fs.existsSync(dir)).toBe(true);
    });

    it('已存在的目录幂等', () => {
      const result = ensureDir(tmpDir);
      expect(result.ok).toBe(true);
    });
  });

  describe('writeFile', () => {
    it('写入文件并自动创建父目录', () => {
      const filePath = path.join(tmpDir, 'sub', 'file.txt');
      const result = writeFile(filePath, 'hello');
      expect(result.ok).toBe(true);
      expect(fs.readFileSync(filePath, 'utf-8')).toBe('hello');
    });

    it('覆盖已存在的文件', () => {
      const filePath = path.join(tmpDir, 'file.txt');
      writeFile(filePath, 'old');
      writeFile(filePath, 'new');
      expect(fs.readFileSync(filePath, 'utf-8')).toBe('new');
    });
  });

  describe('writeFiles', () => {
    it('批量写入多个文件', () => {
      const files = new Map([
        ['index.js', 'console.log("hi")'],
        ['README.md', '# Test'],
        ['src/main.ts', 'export {}'],
      ]);
      const result = writeFiles(files, tmpDir);
      expect(result.ok).toBe(true);
      expect(result.written.length).toBe(3);
      expect(result.failed.length).toBe(0);
      expect(fs.readFileSync(path.join(tmpDir, 'index.js'), 'utf-8')).toBe('console.log("hi")');
      expect(fs.readFileSync(path.join(tmpDir, 'src', 'main.ts'), 'utf-8')).toBe('export {}');
    });

    it('空 Map 返回 ok', () => {
      const result = writeFiles(new Map(), tmpDir);
      expect(result.ok).toBe(true);
      expect(result.written.length).toBe(0);
    });
  });

  describe('readJsonFile', () => {
    it('读取合法 JSON', () => {
      const filePath = path.join(tmpDir, 'config.json');
      fs.writeFileSync(filePath, JSON.stringify({ name: 'test', version: 1 }));
      const result = readJsonFile(filePath);
      expect(result.ok).toBe(true);
      expect(result.data).toEqual({ name: 'test', version: 1 });
    });

    it('不存在的文件返回错误', () => {
      const result = readJsonFile(path.join(tmpDir, 'nope.json'));
      expect(result.ok).toBe(false);
      expect(result.error).toContain('读取 JSON 失败');
    });

    it('非法 JSON 返回错误', () => {
      const filePath = path.join(tmpDir, 'bad.json');
      fs.writeFileSync(filePath, '{ invalid }');
      const result = readJsonFile(filePath);
      expect(result.ok).toBe(false);
      expect(result.error).toContain('读取 JSON 失败');
    });
  });

  describe('writeJsonFile', () => {
    it('写入格式化 JSON', () => {
      const filePath = path.join(tmpDir, 'out.json');
      const result = writeJsonFile(filePath, { name: 'test', items: [1, 2] });
      expect(result.ok).toBe(true);
      const content = fs.readFileSync(filePath, 'utf-8');
      expect(content).toContain('"name": "test"');
      expect(content).toContain('"items": [');
      expect(JSON.parse(content)).toEqual({ name: 'test', items: [1, 2] });
    });
  });

  describe('pathExists', () => {
    it('存在的路径返回 true', () => {
      writeFile(path.join(tmpDir, 'exists.txt'), 'x');
      expect(pathExists(path.join(tmpDir, 'exists.txt'))).toBe(true);
    });

    it('不存在的路径返回 false', () => {
      expect(pathExists(path.join(tmpDir, 'nope.txt'))).toBe(false);
    });
  });

  describe('listFiles', () => {
    it('列出目录下的文件', () => {
      writeFile(path.join(tmpDir, 'a.txt'), 'a');
      writeFile(path.join(tmpDir, 'b.txt'), 'b');
      const result = listFiles(tmpDir);
      expect(result.ok).toBe(true);
      expect(result.files).toContain('a.txt');
      expect(result.files).toContain('b.txt');
    });

    it('不存在的目录返回错误', () => {
      const result = listFiles(path.join(tmpDir, 'nope'));
      expect(result.ok).toBe(false);
    });
  });

  describe('copyFile', () => {
    it('复制文件到新位置', () => {
      const src = path.join(tmpDir, 'src.txt');
      const dest = path.join(tmpDir, 'sub', 'dest.txt');
      writeFile(src, 'content');
      const result = copyFile(src, dest);
      expect(result.ok).toBe(true);
      expect(fs.readFileSync(dest, 'utf-8')).toBe('content');
    });

    it('源文件不存在返回错误', () => {
      const result = copyFile(path.join(tmpDir, 'nope.txt'), path.join(tmpDir, 'dest.txt'));
      expect(result.ok).toBe(false);
      expect(result.error).toContain('源文件不存在');
    });
  });

  describe('端到端：脚手架写入', () => {
    it('scaffold 结果写入磁盘', () => {
      // 模拟 scaffold 生成的文件
      const files = new Map<string, string>([
        ['package.json', '{"name":"test"}'],
        ['src/index.ts', 'console.log("hi")'],
        ['README.md', '# Test Project'],
      ]);
      const outputDir = path.join(tmpDir, 'my-plugin');
      const result = writeFiles(files, outputDir);
      expect(result.ok).toBe(true);
      expect(pathExists(path.join(outputDir, 'package.json'))).toBe(true);
      expect(pathExists(path.join(outputDir, 'src', 'index.ts'))).toBe(true);
      expect(pathExists(path.join(outputDir, 'README.md'))).toBe(true);
    });

    it('client config 写入磁盘', () => {
      const configPath = path.join(tmpDir, 'client-config.json');
      const result = writeJsonFile(configPath, {
        registry: {
          plugin_filter: {
            allow: ['com.example.a'],
            types: ['js'],
          },
        },
        log_level: 'info',
      });
      expect(result.ok).toBe(true);
      const read = readJsonFile(configPath);
      expect(read.ok).toBe(true);
      expect(read.data).toMatchObject({ log_level: 'info' });
    });
  });
});
