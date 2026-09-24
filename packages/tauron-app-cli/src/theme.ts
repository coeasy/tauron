// ──────────────────────────────────────────────────────────────────────────
// CLI 主题生成命令（§4.25 `theme generate`）。
//
// 职责：生成主题 JSON 文件、验证主题配置、生成 CSS 变量文本。
//
// 关键约束：
// - 主题 ID 使用反域名格式或内置 ID（light/dark）
// - CSS 变量名必须以 -- 开头
// - 颜色值必须是合法的 CSS 颜色
// ──────────────────────────────────────────────────────────────────────────

import * as fs from 'node:fs';
import * as path from 'node:path';

// ── 类型 ──

/** 主题定义。 */
export interface ThemeDefinition {
  id: string;
  name: string;
  isDark: boolean;
  variables: Record<string, string>;
}

/** 主题生成配置。 */
export interface ThemeGenerateConfig {
  /** 输出文件路径。 */
  outputPath: string;
  /** 是否生成 CSS 文件。 */
  generateCss?: boolean;
  /** 主题定义列表。 */
  themes?: ThemeDefinition[];
}

/** 主题生成结果。 */
export interface ThemeGenerateResult {
  ok: boolean;
  files?: string[];
  error?: string;
}

/** 主题验证结果。 */
export interface ThemeValidationResult {
  ok: boolean;
  errors?: string[];
  warnings?: string[];
}

// ── 内置主题模板 ──

/** 内置亮色主题模板。 */
export function builtinLightTheme(): ThemeDefinition {
  return {
    id: 'light',
    name: '亮色',
    isDark: false,
    variables: {
      '--oc-color-primary': '#2563eb',
      '--oc-color-primary-hover': '#1d4ed8',
      '--oc-color-primary-active': '#1e40af',
      '--oc-color-danger': '#dc2626',
      '--oc-color-success': '#16a34a',
      '--oc-color-warning': '#d97706',
      '--oc-color-text': '#1f2937',
      '--oc-color-text-secondary': '#6b7280',
      '--oc-color-text-muted': '#9ca3af',
      '--oc-color-background': '#ffffff',
      '--oc-color-background-secondary': '#f9fafb',
      '--oc-color-border': '#e5e7eb',
      '--oc-color-border-hover': '#d1d5db',
      '--oc-color-disabled': '#f3f4f6',
      '--oc-color-disabled-text': '#9ca3af',
      '--oc-color-safemode': '#f59e0b',
      '--oc-color-focus-ring': 'rgba(37, 99, 235, 0.5)',
    },
  };
}

/** 内置暗色主题模板。 */
export function builtinDarkTheme(): ThemeDefinition {
  return {
    id: 'dark',
    name: '暗色',
    isDark: true,
    variables: {
      '--oc-color-primary': '#3b82f6',
      '--oc-color-primary-hover': '#60a5fa',
      '--oc-color-primary-active': '#93c5fd',
      '--oc-color-danger': '#f87171',
      '--oc-color-success': '#4ade80',
      '--oc-color-warning': '#fbbf24',
      '--oc-color-text': '#f9fafb',
      '--oc-color-text-secondary': '#d1d5db',
      '--oc-color-text-muted': '#9ca3af',
      '--oc-color-background': '#111827',
      '--oc-color-background-secondary': '#1f2937',
      '--oc-color-border': '#374151',
      '--oc-color-border-hover': '#4b5563',
      '--oc-color-disabled': '#1f2937',
      '--oc-color-disabled-text': '#6b7280',
      '--oc-color-safemode': '#f59e0b',
      '--oc-color-focus-ring': 'rgba(59, 130, 246, 0.5)',
    },
  };
}

// ── 验证 ──

/** CSS 颜色正则（简化版：hex、rgb、rgba、hsl、hsla、命名颜色）。 */
const COLOR_RE = /^(#[0-9a-fA-F]{3,8}|rgba?\(\s*\d+%?\s*,\s*\d+%?\s*,\s*\d+%?\s*(,\s*[\d.]+\s*)?\)|hsla?\(\s*[\d.]+deg?\s*,\s*\d+%?\s*,\s*\d+%?\s*(,\s*[\d.]+\s*)?\)|[a-zA-Z]+)$/;

/**
 * 验证单个主题定义。
 */
export function validateTheme(theme: ThemeDefinition): ThemeValidationResult {
  const errors: string[] = [];
  const warnings: string[] = [];

  // ID 验证
  if (!theme.id || theme.id.trim() === '') {
    errors.push('主题 ID 不能为空');
  } else if (theme.id.length > 200) {
    errors.push(`主题 ID 长度 ${theme.id.length} 超过 200`);
  }

  // 名称验证
  if (!theme.name || theme.name.trim() === '') {
    errors.push('主题名称不能为空');
  } else if (theme.name.length > 128) {
    errors.push(`主题名称长度 ${theme.name.length} 超过 128`);
  }

  // 变量验证
  for (const [key, value] of Object.entries(theme.variables)) {
    if (!key.startsWith('--')) {
      errors.push(`CSS 变量名 "${key}" 必须以 -- 开头`);
    }
    if (!value || value.trim() === '') {
      errors.push(`CSS 变量 "${key}" 的值不能为空`);
    }
    // 颜色值警告（非颜色值不报错，因为可能是间距、字体等）
    if (value.startsWith('#') && !COLOR_RE.test(value)) {
      warnings.push(`CSS 变量 "${key}" 的值 "${value}" 可能不是有效的颜色`);
    }
  }

  return {
    ok: errors.length === 0,
    ...(errors.length > 0 ? { errors } : {}),
    ...(warnings.length > 0 ? { warnings } : {}),
  };
}

/**
 * 验证主题列表。
 */
export function validateThemes(themes: ThemeDefinition[]): ThemeValidationResult {
  const allErrors: string[] = [];
  const allWarnings: string[] = [];
  const seenIds = new Set<string>();

  for (const theme of themes) {
    const result = validateTheme(theme);
    if (result.errors) allErrors.push(...result.errors);
    if (result.warnings) allWarnings.push(...result.warnings);

    if (theme.id) {
      if (seenIds.has(theme.id)) {
        allErrors.push(`主题 ID "${theme.id}" 重复`);
      }
      seenIds.add(theme.id);
    }
  }

  return {
    ok: allErrors.length === 0,
    ...(allErrors.length > 0 ? { errors: allErrors } : {}),
    ...(allWarnings.length > 0 ? { warnings: allWarnings } : {}),
  };
}

// ── 生成 ──

/**
 * 生成主题 JSON 文件内容。
 */
export function generateThemeJson(themes: ThemeDefinition[]): string {
  return JSON.stringify(
    {
      schemaVersion: 1,
      activeId: themes[0]?.id ?? 'light',
      themes: themes.map((t) => ({
        id: t.id,
        name: t.name,
        isDark: t.isDark,
        variables: t.variables,
      })),
    },
    null,
    2,
  ) + '\n';
}

/**
 * 生成主题 CSS 文件内容。
 */
export function generateThemeCss(themes: ThemeDefinition[]): string {
  const parts = themes.map((theme) => {
    const selector = theme.id === 'light'
      ? ':root'
      : `:root[data-theme="${theme.id}"]`;
    const vars = Object.entries(theme.variables)
      .map(([k, v]) => `  ${k}: ${v};`)
      .join('\n');
    return `${selector} {\n${vars}\n}`;
  });
  return parts.join('\n') + '\n';
}

/**
 * 执行主题生成。
 */
export function themeGenerate(config: ThemeGenerateConfig): ThemeGenerateResult {
  try {
    const themes = config.themes ?? [builtinLightTheme(), builtinDarkTheme()];

    // 验证
    const validation = validateThemes(themes);
    if (!validation.ok) {
      return {
        ok: false,
        ...(validation.errors !== undefined ? { error: validation.errors.join('; ') } : {}),
      };
    }

    const files: string[] = [];
    const outputPath = path.resolve(config.outputPath);

    // 写入 JSON
    const json = generateThemeJson(themes);
    fs.writeFileSync(outputPath, json, 'utf-8');
    files.push(outputPath);

    // 写入 CSS（可选）
    if (config.generateCss) {
      const cssPath = outputPath.replace(/\.json$/, '.css');
      const css = generateThemeCss(themes);
      fs.writeFileSync(cssPath, css, 'utf-8');
      files.push(cssPath);
    }

    return { ok: true, files };
  } catch (err) {
    return {
      ok: false,
      error: err instanceof Error ? err.message : String(err),
    };
  }
}
