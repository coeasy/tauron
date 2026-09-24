// ──────────────────────────────────────────────────────────────────────────
// 设计令牌（开发计划 §4.10）。
//
// 关键约束：shadow root 内禁止 Tailwind 类，只用 CSS 自定义属性 + ::part。
// 本模块定义设计令牌为 CSS 自定义属性值，供 WC 使用。
// ──────────────────────────────────────────────────────────────────────────

/** 设计令牌定义。 */
export const designTokens = {
  // ── 颜色 ──────────────────────────────────────────────────────────────
  colorPrimary: '#2563eb',
  colorPrimaryHover: '#1d4ed8',
  colorPrimaryActive: '#1e40af',
  colorDanger: '#dc2626',
  colorDangerHover: '#b91c1c',
  colorSuccess: '#16a34a',
  colorSuccessHover: '#15803d',
  colorWarning: '#d97706',
  colorWarningHover: '#b45309',
  colorText: '#1f2937',
  colorTextSecondary: '#6b7280',
  colorTextMuted: '#9ca3af',
  colorBackground: '#ffffff',
  colorBackgroundSecondary: '#f9fafb',
  colorBorder: '#e5e7eb',
  colorBorderHover: '#d1d5db',
  colorDisabled: '#f3f4f6',
  colorDisabledText: '#9ca3af',
  colorSafemode: '#f59e0b',

  // ── 间距 ──────────────────────────────────────────────────────────────
  spacingXs: '4px',
  spacingSm: '8px',
  spacingMd: '12px',
  spacingLg: '16px',
  spacingXl: '24px',
  spacing2xl: '32px',

  // ── 字体 ──────────────────────────────────────────────────────────────
  fontSizeXs: '11px',
  fontSizeSm: '12px',
  fontSizeMd: '14px',
  fontSizeLg: '16px',
  fontSizeXl: '18px',
  fontSize2xl: '24px',
  fontWeightNormal: '400',
  fontWeightMedium: '500',
  fontWeightBold: '600',

  // ── 圆角 ──────────────────────────────────────────────────────────────
  borderRadiusSm: '4px',
  borderRadiusMd: '6px',
  borderRadiusLg: '8px',
  borderRadiusXl: '12px',
  borderRadiusFull: '9999px',

  // ── 阴影 ──────────────────────────────────────────────────────────────
  boxShadowSm: '0 1px 2px rgba(0,0,0,0.05)',
  boxShadowMd: '0 4px 6px rgba(0,0,0,0.07)',
  boxShadowLg: '0 10px 15px rgba(0,0,0,0.1)',

  // ── 动画 ──────────────────────────────────────────────────────────────
  transitionDuration: 'var(--oc-motion-duration-fast, 150ms)',
  transitionEasing: 'var(--oc-motion-easing-standard, ease-in-out)',
  // 扩展动效 duration（引用 motion.ts CSS 变量）
  motionDurationFast: 'var(--oc-motion-duration-fast, 150ms)',
  motionDurationNormal: 'var(--oc-motion-duration-normal, 300ms)',
  motionDurationSlow: 'var(--oc-motion-duration-slow, 500ms)',
  motionEasingStandard: 'var(--oc-motion-easing-standard, ease-in-out)',
  motionEasingEmphasized: 'var(--oc-motion-easing-emphasized, ease-in)',
  motionEasingDecelerated: 'var(--oc-motion-easing-decelerated, ease-out)',
  motionEasingAccelerated: 'var(--oc-motion-easing-accelerated, ease-in)',

  // ── 焦点 ──────────────────────────────────────────────────────────────
  focusRingWidth: '2px',
  focusRingOffset: '2px',
  focusRingColor: 'rgba(37, 99, 235, 0.5)',

  // ── 安全模式 ──────────────────────────────────────────────────────────
  safemodeBannerHeight: '48px',
  safemodeBadgePadding: '4px 8px',
} as const;

/** 设计令牌类型。 */
export type DesignTokens = typeof designTokens;

/**
 * 将设计令牌序列化为 CSS 自定义属性。
 *
 * 返回一个对象，键为 CSS 变量名，值为令牌值。
 * 可直接注入到 WC 的 shadow root 中。
 */
export function toCssVariables(tokens: DesignTokens = designTokens): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(tokens)) {
    const cssName = toCssVarName(key);
    out[cssName] = value;
  }
  return out;
}

/**
 * 将 camelCase 键名转换为 CSS 变量名（kebab-case）。
 */
function toCssVarName(key: string): string {
  return `--oc-${key.replace(/([a-z])([A-Z])/g, '$1-$2').toLowerCase()}`;
}

/**
 * 生成设计令牌的 CSS 文本。
 *
 * 可直接嵌入 <style> 标签或 WC 的 shadow root。
 */
export function toCssText(tokens: DesignTokens = designTokens): string {
  const vars = toCssVariables(tokens);
  const lines = Object.entries(vars).map(([name, value]) => `  ${name}: ${value};`);
  return `:root {\n${lines.join('\n')}\n}`;
}

/**
 * 获取 safemode 相关的样式令牌。
 */
export const safemodeTokens = {
  /** 安全模式横幅背景色。 */
  bannerBg: designTokens.colorSafemode,
  /** 安全模式横幅文字色。 */
  bannerText: '#ffffff',
  /** 安全模式徽章背景色。 */
  badgeBg: 'rgba(255, 255, 255, 0.2)',
  /** 安全模式徽章文字色。 */
  badgeText: '#ffffff',
  /** 安全模式徽章边框色。 */
  badgeBorder: 'rgba(255, 255, 255, 0.4)',
  /** 安全模式操作按钮背景色。 */
  actionBtnBg: '#ffffff',
  /** 安全模式操作按钮文字色。 */
  actionBtnText: designTokens.colorSafemode,
} as const;
