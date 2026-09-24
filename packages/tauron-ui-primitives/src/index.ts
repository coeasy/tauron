// ──────────────────────────────────────────────────────────────────────────
// @tauron/ui-primitives — 零宿主依赖的 UI 原语入口。
//
// 本包**不得**依赖 `@tauron/host`（R3 / C1）：哑组件只接受属性喂数与派发
// `oc-*` 事件，事件名取自 `@tauron/shell-events` 契约。任意桌面宿主
// （包括不装插件运行时的底座项目）都可以只取本包，不拖入宿主命令面。
//
// 导出：
// - designTokens / toCssVariables / toCssText / safemodeTokens（设计令牌）
// - TitleBarStore / UpdaterStore / ToastStore / CommandPaletteStore /
//   ShortcutRecorderStore（纯状态层，可离线测试）
// - Web Components：oc-toast / oc-title-bar / oc-tray-menu / oc-updater-dialog /
//   oc-command-palette / oc-shortcut-recorder / oc-plugin-manager /
//   oc-theme-picker / oc-splash / oc-skeleton
// - 动效基础设施（motion / wc-motion / animation-hook）
// - 退出动画与组件动画（自 `@tauron/host` 迁入：它们是 DOM 工具，不是宿主能力）
//
// 注意：需要 Backend 的 `PluginManagerStore` 在 `@tauron/ui`（便利包），
// 因为它要调宿主命令；哑组件 `oc-plugin-manager` 留在本包。
// ──────────────────────────────────────────────────────────────────────────

export {
  designTokens,
  safemodeTokens,
  toCssVariables,
  toCssText,
} from './tokens.js';
export type { DesignTokens } from './tokens.js';

export {
  TitleBarStore,
} from './title-bar.js';
export type {
  TitleBarConfig,
  TitleBarSnapshot,
  TitleBarState,
  WindowActionResult,
  WindowState,
} from './title-bar.js';

export {
  UpdaterStore,
} from './updater-dialog.js';
export type {
  UpdaterActionResult,
  UpdaterConfig,
  UpdaterSnapshot,
  UpdaterState,
  UpdateInfo,
} from './updater-dialog.js';

export {
  ToastStore,
} from './toast.js';
export type {
  ToastAction,
  ToastActionResult,
  ToastConfig,
  ToastItem,
  ToastLevel,
  ToastSnapshot,
} from './toast.js';

export {
  CommandPaletteStore,
} from './command-palette.js';
export type {
  CommandItem,
  CommandPaletteActionResult,
  CommandPaletteConfig,
  CommandPaletteSnapshot,
  CommandPaletteState,
} from './command-palette.js';

export {
  ShortcutRecorderStore,
} from './shortcut-recorder.js';
export type {
  KeyInfo,
  ModifierKey,
  ShortcutFormat,
  ShortcutRecorderActionResult,
  ShortcutRecorderConfig,
  ShortcutRecorderSnapshot,
  ShortcutRecorderState,
} from './shortcut-recorder.js';

// Web Components（`import '@tauron/ui-primitives/wc'` 注册 oc-toast）
export { OcToast } from './wc.js';

// WC 壳组件（TitleBar、TrayMenu、UpdaterDialog、CommandPalette、ShortcutRecorder、PluginManager）
export {
  OcTitleBar,
  OcTrayMenu,
  OcUpdaterDialog,
  OcCommandPalette,
  OcShortcutRecorder,
  OcPluginManager,
} from './wc-shell.js';
export type {
  TrayMenuItem,
  CommandItem as PaletteCommandItem,
  PluginInfo,
  // detail 类型的事实源在 @tauron/shell-events，这里转出以保持既有导入路径可用。
  TrayItemEventDetail,
  CommandSelectEventDetail,
  ShortcutChangeEventDetail,
  PluginToggleEventDetail,
} from './wc-shell.js';

// 主题选择器（`<oc-theme-picker>` + ThemePickerStore）
export { OcThemePicker, ThemePickerStore } from './theme-picker.js';
export type {
  ThemePreview,
  ThemeChangeEventDetail,
  ThemePickerSnapshot,
} from './theme-picker.js';

// 动效基础设施（§5.4 动效与启动退出体验）
export {
  DEFAULT_DURATIONS,
  DEFAULT_EASINGS,
  DEFAULT_KEYFRAMES,
  EASING_STANDARD,
  EASING_EMPHASIZED,
  EASING_DECELERATED,
  EASING_ACCELERATED,
  defineMotionTheme,
  getActiveMotionTheme,
  getDuration,
  getEasing,
  animation,
  motionToCssVariables,
  motionToCssText,
  durationsForPreset,
  prefersReducedMotion,
  onReducedMotionChange,
  shouldAnimate,
  resetMotionTheme,
} from './motion.js';
export type {
  MotionPreset,
  MotionThemeConfig,
  MotionConfig,
  KeyframeDefinition,
  EasingCurve,
  DurationScale,
} from './motion.js';

// Splash 启动画面（<oc-splash> + SplashStore）
export {
  OcSplash,
  SplashStore,
  DEFAULT_SPLASH_CONFIG,
} from './wc-motion.js';
export type {
  SplashConfig,
  SplashPhase,
  SplashProgressMode,
  SplashExitAnimation,
  SplashSnapshot,
} from './wc-motion.js';

// 骨架屏（<oc-skeleton>）
export { OcSkeleton } from './skeleton.js';
export type { SkeletonShape } from './skeleton.js';

// 动画 hook
export { AnimationHook, useAnimation } from './animation-hook.js';
export type { AnimationState } from './animation-hook.js';

// 退出动画（自 @tauron/host 迁入：纯 DOM 时序工具）
export { ExitAnimation, createExitAnimation } from './exit-animation.js';
export type { ExitAnimationConfig, ExitAnimationType, ExitPhase } from './exit-animation.js';

// 组件动画（自 @tauron/host 迁入：纯 DOM 时序工具）
export { animateEnter, animateLeave, staggerIn, staggerOut, animateListUpdate } from './component-animation.js';
export type { AnimationType, AnimationConfig, StaggerConfig } from './component-animation.js';
