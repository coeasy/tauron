// @vitest-environment happy-dom
// dialog-client.ts 测试（P2-13：对话框客户端）

import { describe, it, expect, beforeEach } from 'vitest';
import { DialogClient, createDialogClient, isUnsupportedBody } from './dialog-client.js';
import { MockBackend } from './backend.js';

describe('DialogClient', () => {
  let backend: MockBackend;
  let client: DialogClient;

  beforeEach(() => {
    backend = new MockBackend({
      capabilities: [
        'host_dialog_open',
        'host_dialog_save',
        'host_dialog_message',
        'host_dialog_confirm',
        'host_clipboard_read',
        'host_clipboard_write',
      ],
      cases: [
        { cmd: 'host_dialog_open', result: '/path/to/file.png' },
        { cmd: 'host_dialog_save', result: '/path/to/save.txt' },
        { cmd: 'host_dialog_message', result: undefined },
        { cmd: 'host_dialog_confirm', result: true },
        { cmd: 'host_clipboard_read', result: { supported: false, reason: 'system clipboard missing', fallback: 'in-process-buffer', value: 'clipboard text' } },
        { cmd: 'host_clipboard_write', result: { supported: false, reason: 'system clipboard missing', fallback: 'in-process-buffer' } },
      ],
    });
    client = new DialogClient({ backend });
  });

  describe('基本功能', () => {
    it('创建实例', () => {
      expect(client).toBeDefined();
    });

    it('createDialogClient 工厂函数', () => {
      const c = createDialogClient({ backend });
      expect(c).toBeInstanceOf(DialogClient);
    });
  });

  describe('openFile()', () => {
    it('缺少原生 provider 时暴露 UnsupportedBody，不伪装成取消', async () => {
      const backend2 = new MockBackend({
        capabilities: ['host_dialog_open'],
        cases: [{ cmd: 'host_dialog_open', result: { supported: false, reason: 'dialog provider missing', fallback: null } }],
      });
      const result = await new DialogClient({ backend: backend2 }).openFile();
      expect(isUnsupportedBody(result)).toBe(true);
      if (isUnsupportedBody(result)) expect(result.reason).toBe('dialog provider missing');
    });

    it('打开文件对话框', async () => {
      const path = await client.openFile();
      expect(path).toBe('/path/to/file.png');
      expect(backend.invocations.some(i => i.cmd === 'host_dialog_open')).toBe(true);
    });

    it('传递 options', async () => {
      await client.openFile({
        filters: [{ name: 'Images', extensions: ['png', 'jpg'] }],
        multiple: true,
      });
      const inv = backend.invocations.find(i => i.cmd === 'host_dialog_open');
      expect(inv?.args?.multiple).toBe(true);
    });
  });

  describe('openDirectory()', () => {
    it('打开目录对话框', async () => {
      const path = await client.openDirectory();
      expect(path).toBe('/path/to/file.png');
      const inv = backend.invocations.find(i => i.cmd === 'host_dialog_open');
      expect(inv?.args?.directory).toBe(true);
    });
  });

  describe('saveFile()', () => {
    it('保存文件对话框', async () => {
      const path = await client.saveFile();
      expect(path).toBe('/path/to/save.txt');
      expect(backend.invocations.some(i => i.cmd === 'host_dialog_save')).toBe(true);
    });

    it('传递 options', async () => {
      await client.saveFile({
        defaultName: 'test.txt',
        filters: [{ name: 'Text', extensions: ['txt'] }],
      });
      const inv = backend.invocations.find(i => i.cmd === 'host_dialog_save');
      expect(inv?.args?.defaultName).toBe('test.txt');
    });
  });

  describe('message()', () => {
    it('消息对话框', async () => {
      await client.message({ title: 'Info', message: 'Hello!' });
      const inv = backend.invocations.find(i => i.cmd === 'host_dialog_message');
      expect(inv?.args?.title).toBe('Info');
      expect(inv?.args?.message).toBe('Hello!');
    });
  });

  describe('error()', () => {
    it('错误对话框', async () => {
      await client.error({ title: 'Error', message: 'Failed!' });
      const inv = backend.invocations.find(i => i.cmd === 'host_dialog_message');
      expect(inv?.args?.kind).toBe('error');
    });
  });

  describe('warn()', () => {
    it('警告对话框', async () => {
      await client.warn({ title: 'Warning', message: 'Careful!' });
      const inv = backend.invocations.find(i => i.cmd === 'host_dialog_message');
      expect(inv?.args?.kind).toBe('warning');
    });
  });

  describe('confirm()', () => {
    it('确认对话框（确认）', async () => {
      const result = await client.confirm({ title: 'Confirm', message: 'Sure?' });
      expect(result).toBe(true);
      expect(backend.invocations.some(i => i.cmd === 'host_dialog_confirm')).toBe(true);
    });

    it('传递自定义标签', async () => {
      await client.confirm({
        title: 'Confirm',
        message: 'Sure?',
        confirmLabel: 'Yes',
        cancelLabel: 'No',
      });
      const inv = backend.invocations.find(i => i.cmd === 'host_dialog_confirm');
      expect(inv?.args?.confirmLabel).toBe('Yes');
      expect(inv?.args?.cancelLabel).toBe('No');
    });
  });

  describe('clipboardRead()', () => {
    it('读取剪贴板', async () => {
      const text = await client.clipboardRead();
      expect(text).toBe('clipboard text');
      expect(backend.invocations.some(i => i.cmd === 'host_clipboard_read')).toBe(true);
    });

    it('详细读取结果明确标记进程内回退', async () => {
      const result = await client.clipboardReadDetailed();
      expect(result.supported).toBe(false);
      expect(result.fallback).toBe('in-process-buffer');
      expect(result.value).toBe('clipboard text');
    });
  });

  describe('clipboardWrite()', () => {
    it('写入剪贴板', async () => {
      const result = await client.clipboardWrite('new text');
      expect(result.supported).toBe(false);
      expect(result.fallback).toBe('in-process-buffer');
      const inv = backend.invocations.find(i => i.cmd === 'host_clipboard_write');
      expect(inv?.args?.text).toBe('new text');
    });
  });

  describe('destroy()', () => {
    it('清理资源', () => {
      expect(() => client.destroy()).not.toThrow();
    });
  });
});
