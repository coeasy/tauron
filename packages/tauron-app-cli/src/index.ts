// @tauron/app-cli — 开发计划 §4.17 CLI 工具。
//
// 导出：
// - 脚手架核心逻辑（create-tauron）
// - 插件脚手架逻辑（plugin new/dev/pack/sign/publish）
// - 品牌构建逻辑（brand build）
// - 插件打包/签名/发布逻辑（plugin pack/sign/publish）

export {
  scaffold,
  validateConfig,
  validateName,
  validateFramework,
  validateTargets,
  validateShell,
  validateCapabilities,
  toSlug,
  generatePackageJson,
  generateTauriConfig,
  generateTsconfig,
  generateAppEntry,
  generateCapabilitiesJson,
  generateGitignore,
  generateFiles,
  InvalidTargetError,
  ConfigValidationError,
  SUPPORTED_FRAMEWORKS,
  SUPPORTED_TARGETS,
  SUPPORTED_SHELLS,
  UNSUPPORTED_TARGETS,
  type ScaffoldConfig,
  type ScaffoldConfigInput,
  type ScaffoldResult,
  type Framework,
  type Target,
  type Shell,
} from './scaffold.js';

export {
  pluginScaffold,
  validatePluginConfig,
  validatePluginId,
  validatePluginName,
  validateVersion,
  validatePluginType,
  validatePermissions,
  generatePluginManifest,
  generatePluginPackageJson,
  generatePluginEntry,
  generatePluginTsconfig,
  generatePluginGitignore,
  generatePluginFiles,
  validateDevWatchConfig,
  isPathWithinDir,
  isIgnored,
  SUPPORTED_PLUGIN_TYPES,
  type PluginConfig,
  type PluginConfigInput,
  type PluginScaffoldResult,
  type PluginPackConfig,
  type PluginDevWatchConfig,
  type PluginType,
} from './plugin.js';

export {
  brandBuild,
  validateBrandBuildConfig,
  validateBrandName,
  validateIdentifier,
  validateProtocolName,
  validateAutostartName,
  validateDataDirName,
  validateShortcut,
  validateIcons,
  validatePlatforms,
  validateBuildType,
  checkUniqueness,
  generateBrandTauriConfig,
  generateBrandManifest,
  generateCiMatrix,
  generateIconReport,
  generateBrandFiles,
  UniquenessConflictError,
  SUPPORTED_PLATFORMS,
  SUPPORTED_BUILD_TYPES,
  type BrandBuildConfig,
  type BrandBuildResult,
  type BuildPlatform,
  type BuildType,
  type IconFiles,
} from './brand.js';

export {
  pluginPack,
  pluginSign,
  pluginPublish,
  validatePackConfig,
  validateSignConfig,
  validatePublishConfig,
  validateSignAlgorithm,
  isSignAlgorithm,
  validateKid,
  validateFiles,
  validateFrameworkRange,
  frameworkMatchesRange,
  verifySignature,
  verifySignatureCrypto,
  signPayload,
  toPkcs8PrivateKeyHex,
  toSpkiPublicKeyHex,
  generatePackManifest,
  collectPluginFiles,
  SUPPORTED_SIGN_ALGORITHMS,
  MAX_FILE_SIZE,
  MAX_FILE_COUNT,
  type SignAlgorithm,
  type PluginFileInfo,
  type PluginScanResult,
  type PluginSignature,
  type PackConfig,
  type ResolvedPackConfig,
  type SignConfig,
  type ResolvedSignConfig,
  type PublishConfig,
  type ResolvedPublishConfig,
  type PackResult,
  type SignResult,
  type PublishResult,
} from './pack.js';

export {
  generateClientConfig,
  validateClientConfigFile,
  validateClientConfigFilePath,
  type ClientConfigOptions,
  type ClientConfigResult,
} from './client-config.js';

export {
  ensureDir,
  writeFile,
  writeFiles,
  readJsonFile,
  writeJsonFile,
  pathExists,
  listFiles,
  copyFile,
  writeScaffoldResult,
  type WriteResult,
  type BatchWriteResult,
} from './fs-operations.js';

export {
  generateThemeJson,
  generateThemeCss,
  themeGenerate,
  validateTheme,
  validateThemes,
  builtinLightTheme,
  builtinDarkTheme,
  type ThemeDefinition,
  type ThemeGenerateConfig,
  type ThemeGenerateResult,
  type ThemeValidationResult,
} from './theme.js';

export {
  initProject,
  type InitConfig,
  type InitResult,
} from './init.js';

export {
  doctor,
  type DoctorCheck,
  type DoctorResult,
} from './doctor.js';

export { main as cliMain } from './cli.js';

export {
  PluginDevWatcher,
  createDevWatcher,
  type WatcherConfig,
  type ReloadEvent,
  type ReloadEventType,
} from './dev-server.js';

export {
  Logger,
  createLogger,
  isLevelEnabled,
  type LogLevel,
  type LogEntry,
  type LoggerConfig,
} from './logger.js';

export {
  CrashReporter,
  createCrashReporter,
  type CrashRecord,
  type CrashSummary,
  type CrashReportConfig,
} from './crash-report.js';

export {
  createTemplate,
  copyTemplate,
  listTemplates,
  getTemplateInfo,
  validateTemplateName,
  listTemplateFiles,
  generatePluginScaffold,
  SUPPORTED_TEMPLATE_TYPES,
  type TemplateType,
  type TemplateOptions,
  type TemplateResult,
  type TemplateFileInfo,
  type TemplateInfo,
} from './template.js';
