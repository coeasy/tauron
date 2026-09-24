import { describe, expect, it } from 'vitest';
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as os from 'node:os';
import {
  builtinLightTheme,
  builtinDarkTheme,
  validateTheme,
  validateThemes,
  generateThemeJson,
  generateThemeCss,
  themeGenerate,
} from './theme.js';

describe('theme', () => {
  let tmpDir: string;

  it('builtinLightTheme 返回完整主题', () => {
    const theme = builtinLightTheme();
    expect(theme.id).toBe('light');
    expect(theme.name).toBe('亮色');
    expect(theme.isDark).toBe(false);
    expect(Object.keys(theme.variables).length).toBeGreaterThan(10);
    expect(theme.variables['--oc-color-primary']).toBe('#2563eb');
  });

  it('builtinDarkTheme 返回完整主题', () => {
    const theme = builtinDarkTheme();
    expect(theme.id).toBe('dark');
    expect(theme.name).toBe('暗色');
    expect(theme.isDark).toBe(true);
    expect(Object.keys(theme.variables).length).toBeGreaterThan(10);
    expect(theme.variables['--oc-color-background']).toBe('#111827');
  });

  it('validateTheme 合法主题', () => {
    const result = validateTheme(builtinLightTheme());
    expect(result.ok).toBe(true);
  });

  it('validateTheme 空 ID', () => {
    const result = validateTheme({ ...builtinLightTheme(), id: '' });
    expect(result.ok).toBe(false);
    expect(result.errors).toContain('主题 ID 不能为空');
  });

  it('validateTheme 空名称', () => {
    const result = validateTheme({ ...builtinLightTheme(), name: '' });
    expect(result.ok).toBe(false);
    expect(result.errors).toContain('主题名称不能为空');
  });

  it('validateTheme 非法变量名', () => {
    const result = validateTheme({
      ...builtinLightTheme(),
      variables: { 'color-primary': '#000' },
    });
    expect(result.ok).toBe(false);
    expect(result.errors?.some((e) => e.includes('必须以 -- 开头'))).toBe(true);
  });

  it('validateThemes 重复 ID', () => {
    const themes = [builtinLightTheme(), builtinLightTheme()];
    const result = validateThemes(themes);
    expect(result.ok).toBe(false);
    expect(result.errors?.some((e) => e.includes('重复'))).toBe(true);
  });

  it('generateThemeJson 生成合法 JSON', () => {
    const json = generateThemeJson([builtinLightTheme(), builtinDarkTheme()]);
    const parsed = JSON.parse(json);
    expect(parsed.schemaVersion).toBe(1);
    expect(parsed.activeId).toBe('light');
    expect(parsed.themes.length).toBe(2);
    expect(parsed.themes[0].id).toBe('light');
  });

  it('generateThemeCss 生成合法 CSS', () => {
    const css = generateThemeCss([builtinLightTheme(), builtinDarkTheme()]);
    expect(css).toContain(':root {');
    expect(css).toContain(':root[data-theme="dark"] {');
    expect(css).toContain('--oc-color-primary: #2563eb;');
    expect(css).toContain('--oc-color-background: #111827;');
  });

  it('themeGenerate 写入文件', () => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'oc-theme-'));
    const outputPath = path.join(tmpDir, 'themes.json');
    const result = themeGenerate({ outputPath });
    expect(result.ok).toBe(true);
    expect(result.files?.length).toBe(1);
    expect(fs.existsSync(outputPath)).toBe(true);
    const content = JSON.parse(fs.readFileSync(outputPath, 'utf-8'));
    expect(content.themes.length).toBe(2);
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it('themeGenerate 生成 CSS', () => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'oc-theme-'));
    const outputPath = path.join(tmpDir, 'themes.json');
    const result = themeGenerate({ outputPath, generateCss: true });
    expect(result.ok).toBe(true);
    expect(result.files?.length).toBe(2);
    const cssPath = outputPath.replace(/\.json$/, '.css');
    expect(fs.existsSync(cssPath)).toBe(true);
    const cssContent = fs.readFileSync(cssPath, 'utf-8');
    expect(cssContent).toContain(':root {');
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it('themeGenerate 验证失败', () => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'oc-theme-'));
    const outputPath = path.join(tmpDir, 'themes.json');
    const result = themeGenerate({
      outputPath,
      themes: [{ id: '', name: 'Test', isDark: false, variables: {} }],
    });
    expect(result.ok).toBe(false);
    expect(result.error).toContain('主题 ID 不能为空');
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });
});
