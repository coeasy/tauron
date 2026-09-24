/**
 * tauron 配置类型（设计文档 §7.3/§4.6/§12）
 */

/** 壳形态 */
export type ShellSource = 'local' | 'local-server' | 'remote-url' | 'sub-webview';

/** 能力组 */
export type CapabilityGroup =
  | 'base'
  | 'updater'
  | 'store'
  | 'shortcut'
  | 'data'
  | 'network'
  | 'security'
  | 'mobile';

/** 插件配置 */
export interface TauronPluginsConfig {
  /** 本地插件目录 */
  local: string;
  /** 插件仓库 URL */
  registry: string;
  /** 是否自动更新 */
  autoUpdate: boolean;
}

/** 壳配置 */
export interface TauronShellConfig {
  /** 壳形态 */
  type: ShellSource;
  /** 前端构建输出目录 */
  distDir: string;
}

/** 更新配置 */
export interface TauronUpdaterConfig {
  /** 更新端点列表 */
  endpoints: string[];
  /** 更新公钥 */
  pubkey: string;
}

/** 白标配置 */
export interface TauronBrandsConfig {
  /** 品牌名 → 配置文件路径 */
  [brand: string]: string;
}

/** i18n 配置 */
export interface TauronI18nConfig {
  /** 默认语言 */
  defaultLocale: string;
  /** 支持的语言列表 */
  supported: string[];
}

/** 动效预设等级 */
export type TauronMotionPreset = 'none' | 'subtle' | 'standard' | 'expressive';

/** Splash 进度模式 */
export type TauronSplashProgressMode = 'bar' | 'spinner' | 'dots' | 'none';

/** Splash 退出动画 */
export type TauronSplashExitAnimation = 'fade' | 'slide-down' | 'scale-out' | 'none';

/** Splash 配置 */
export interface TauronSplashConfig {
  /** 是否启用 */
  enabled: boolean;
  /** 最短显示时间（ms） */
  minDuration: number;
  /** 品牌 logo（URL 或 data URI） */
  logo?: string;
  /** 标题 */
  title: string;
  /** 副标题 */
  subtitle?: string;
  /** 背景色/渐变 */
  background: string;
  /** 进度条模式 */
  progress: TauronSplashProgressMode;
  /** 退出动画 */
  exitAnimation: TauronSplashExitAnimation;
  /** 自定义 HTML（完全自绘启动画面） */
  customHtml?: string;
  /** 启动钩子（返回 Promise，resolve 后标记 ready） */
  onBoot?: string;
}

/** 退出动画配置 */
export interface TauronExitConfig {
  /** 退出动画类型 */
  animation: 'fade' | 'slide-down' | 'scale-out' | 'none';
  /** 退出动画时长（ms） */
  duration: number;
  /** 退出前保存提示 */
  savingPrompt: string;
  /** 退出提示文案 */
  prompt: string;
  /** 退出前钩子（返回 Promise，resolve 后继续退出） */
  beforeExit?: string;
}

/** 组件级过渡配置 */
export interface TauronTransitionConfig {
  /** Toast 动画 */
  toast: boolean;
  /** Dialog 动画 */
  dialog: boolean;
  /** CommandPalette 动画 */
  commandPalette: boolean;
  /** PluginList 动画 */
  pluginList: boolean;
  /** ThemeSwitch 动画 */
  themeSwitch: boolean;
}

/** Keyframe 定义 */
export interface TauronKeyframeDefinition {
  offset: string;
  properties: Record<string, string>;
}

/** Easing 曲线定义 */
export interface TauronEasingCurve {
  /** 名称 */
  name: string;
  /** CSS cubic-bezier 参数 */
  cubicBezier: [number, number, number, number];
  /** CSS timing function 值 */
  value: string;
}

/** 动效配置 */
export interface TauronMotionConfig {
  /** 预设等级 */
  preset: TauronMotionPreset;
  /** Duration 阶梯（ms） */
  durations: { fast: number; normal: number; slow: number };
  /** Easing 曲线 */
  easings: Record<string, TauronEasingCurve>;
  /** Splash 配置 */
  splash: TauronSplashConfig;
  /** 退出动画配置 */
  exit: TauronExitConfig;
  /** 组件级过渡开关 */
  transitions: TauronTransitionConfig;
  /** 是否尊重 prefers-reduced-motion */
  respectReducedMotion: boolean;
  /** 自定义 keyframes（运行时注册） */
  customKeyframes?: Record<string, TauronKeyframeDefinition[]>;
}

/** 安装包配置 */
export interface TauronInstallerConfig {
  /** 安装包类型 */
  type: 'nsis' | 'dmg' | 'appimage' | 'msi' | 'deb' | 'rpm';
  /** 安装后是否立即启动 */
  launchAfterInstall: boolean;
  /** 安装完成后显示消息 */
  installMessage: string;
  /** 卸载完成后显示消息 */
  uninstallMessage: string;
}

/** 安全配置 */
export interface TauronSecurityConfig {
  /** Content Security Policy */
  csp: string | null;
  /** 是否允许开发工具（仅开发环境） */
  devTools: boolean;
  /** 是否启用 CSP 强制模式 */
  cspEnforce: boolean;
}

/** 网络配置 */
export interface TauronNetworkConfig {
  /** 代理服务器 URL */
  proxy?: string;
  /** 代理绕过列表 */
  noProxy?: string[];
  /** 请求超时（ms） */
  timeout: number;
  /** 最大重试次数 */
  maxRetries: number;
}

/** 日志配置 */
export interface TauronLoggingConfig {
  /** 日志级别 */
  level: 'error' | 'warn' | 'info' | 'debug' | 'trace';
  /** 是否输出到控制台 */
  console: boolean;
  /** 是否写入文件 */
  file: boolean;
  /** 日志文件路径 */
  filePath?: string;
  /** 最大日志文件大小（MB） */
  maxFileSize: number;
}

/** 无障碍配置 */
export interface TauronAccessibilityConfig {
  /** 高对比度模式 */
  highContrast: boolean;
  /** 字体缩放比例 */
  fontScaling: number;
  /** 是否启用屏幕阅读器支持 */
  screenReader: boolean;
  /** 减少动画效果 */
  reducedMotion: boolean;
}

/** 窗口配置 */
export interface TauronWindowConfig {
  /** 是否持久化窗口位置/大小 */
  persistState: boolean;
  /** 默认宽度 */
  defaultWidth: number;
  /** 默认高度 */
  defaultHeight: number;
  /** 默认 X 位置 */
  defaultX: number;
  /** 默认 Y 位置 */
  defaultY: number;
  /** 最小宽度 */
  minWidth: number;
  /** 最小高度 */
  minHeight: number;
  /** 标题栏样式 */
  titleBarStyle: 'visible' | 'hidden' | 'hiddenInset';
}

/** 全局快捷键配置 */
export interface TauronShortcutConfig {
  /** 是否启用全局快捷键 */
  enabled: boolean;
  /** 快捷键列表 */
  shortcuts: Array<{
    /** 快捷键名称 */
    name: string;
    /** 按键组合（如 "CmdOrCtrl+Shift+N"） */
    keys: string;
    /** 是否全局生效 */
    global: boolean;
  }>;
}

/** 系统托盘配置 */
export interface TauronTrayConfig {
  /** 是否启用系统托盘 */
  enabled: boolean;
  /** 托盘图标路径 */
  icon: string;
  /** 托盘提示文字 */
  tooltip: string;
  /** 菜单项 */
  menu: Array<{
    /** 菜单项 ID */
    id: string;
    /** 菜单项标签 */
    label: string;
    /** 是否启用 */
    enabled: boolean;
    /** 是否显示分隔线 */
    separator: boolean;
  }>;
}

/** 深链接配置 */
export interface TauronDeepLinksConfig {
  /** 是否启用深链接 */
  enabled: boolean;
  /** 协议名（如 "tauron"） */
  protocol: string;
  /** 深链接处理器列表 */
  handlers: Array<{
    /** 匹配路径 */
    path: string;
    /** 处理器标识 */
    handler: string;
  }>;
}

/** tauron.config.ts 完整配置 */
export interface TauronConfig {
  /** 能力集 */
  capabilities: CapabilityGroup[];
  /** 插件配置 */
  plugins: TauronPluginsConfig;
  /** 壳配置 */
  shell: TauronShellConfig;
  /** 更新配置 */
  updater: TauronUpdaterConfig;
  /** 白标配置 */
  brands: TauronBrandsConfig;
  /** i18n 配置 */
  i18n: TauronI18nConfig;
  /** 动效配置 */
  motion: TauronMotionConfig;
  /** 安装包配置 */
  installer?: TauronInstallerConfig;
  /** 安全配置 */
  security?: TauronSecurityConfig;
  /** 网络配置 */
  network?: TauronNetworkConfig;
  /** 日志配置 */
  logging?: TauronLoggingConfig;
  /** 无障碍配置 */
  accessibility?: TauronAccessibilityConfig;
  /** 窗口配置 */
  window?: TauronWindowConfig;
  /** 全局快捷键配置 */
  shortcuts?: TauronShortcutConfig;
  /** 系统托盘配置 */
  tray?: TauronTrayConfig;
  /** 深链接配置 */
  deepLinks?: TauronDeepLinksConfig;
}

/**
 * 验证配置对象
 */
export function validateConfig(config: unknown): string[] {
  const errors: string[] = [];
  const cfg = config as TauronConfig;

  if (!cfg || typeof cfg !== 'object') {
    errors.push('Config must be an object');
    return errors;
  }

  // capabilities
  if (!Array.isArray(cfg.capabilities)) {
    errors.push('capabilities must be an array');
  }

  // plugins
  if (!cfg.plugins || typeof cfg.plugins !== 'object') {
    errors.push('plugins must be an object');
  } else {
    if (!cfg.plugins.local) errors.push('plugins.local is required');
    if (!cfg.plugins.registry) errors.push('plugins.registry is required');
    if (typeof cfg.plugins.autoUpdate !== 'boolean') errors.push('plugins.autoUpdate must be boolean');
  }

  // shell
  if (!cfg.shell || typeof cfg.shell !== 'object') {
    errors.push('shell must be an object');
  } else {
    const validShells: ShellSource[] = ['local', 'local-server', 'remote-url', 'sub-webview'];
    if (!validShells.includes(cfg.shell.type)) {
      errors.push(`shell.type must be one of: ${validShells.join(', ')}`);
    }
    if (!cfg.shell.distDir) errors.push('shell.distDir is required');
  }

  // updater
  if (!cfg.updater || typeof cfg.updater !== 'object') {
    errors.push('updater must be an object');
  } else {
    if (!Array.isArray(cfg.updater.endpoints) || cfg.updater.endpoints.length === 0) {
      errors.push('updater.endpoints must be a non-empty array');
    }
    if (!cfg.updater.pubkey) errors.push('updater.pubkey is required');
  }

  // i18n
  if (!cfg.i18n || typeof cfg.i18n !== 'object') {
    errors.push('i18n must be an object');
  } else {
    if (!cfg.i18n.defaultLocale) errors.push('i18n.defaultLocale is required');
    if (!Array.isArray(cfg.i18n.supported) || cfg.i18n.supported.length === 0) {
      errors.push('i18n.supported must be a non-empty array');
    }
  }

  // motion
  if (cfg.motion !== undefined) {
    if (typeof cfg.motion !== 'object') {
      errors.push('motion must be an object');
    } else {
      const validPresets = ['none', 'subtle', 'standard', 'expressive'];
      if (!validPresets.includes(cfg.motion.preset)) {
        errors.push(`motion.preset must be one of: ${validPresets.join(', ')}`);
      }
      if (cfg.motion.durations) {
        for (const k of ['fast', 'normal', 'slow'] as const) {
          if (typeof cfg.motion.durations[k] !== 'number' || cfg.motion.durations[k] < 0) {
            errors.push(`motion.durations.${k} must be a non-negative number`);
          }
        }
      }
      if (cfg.motion.splash && typeof cfg.motion.splash !== 'object') {
        errors.push('motion.splash must be an object');
      }
      if (cfg.motion.exit && typeof cfg.motion.exit !== 'object') {
        errors.push('motion.exit must be an object');
      }
    }
  }

  return errors;
}
