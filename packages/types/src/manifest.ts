/**
 * tauron 插件 manifest 类型（设计文档 §4.1）
 */

import type { PluginType } from './plugin.js';

/** 作者信息 */
export interface ManifestAuthor {
  name: string;
  email?: string;
}

/** 入口配置 */
export interface ManifestEntry {
  /** B 类：UI 页面 */
  html?: string;
  /** B 类：预加载脚本 */
  preload?: string;
  /** D 类：WASM 模块 */
  module?: string;
  /** C 类：sidecar 二进制（按平台三元组替换 {{target}}） */
  binary?: string;
}

/** 命令贡献 */
export interface CommandContribute {
  id: string;
  title: string;
  icon?: string;
}

/** 菜单贡献 */
export interface MenuContribute {
  id: string;
  command: string;
  position?: string;
}

/** 面板贡献 */
export interface PanelContribute {
  id: string;
  title: string;
  icon?: string;
}

/** 设置 Tab 贡献 */
export interface SettingsTabContribute {
  id: string;
  title: string;
}

/** 快捷键贡献 */
export interface ShortcutContribute {
  accelerator: string;
  command: string;
  platforms: string[];
}

/** UI 扩展点贡献 */
export interface ManifestContributes {
  commands?: CommandContribute[];
  menus?: MenuContribute[];
  panels?: PanelContribute[];
  settingsTabs?: SettingsTabContribute[];
  shortcuts?: ShortcutContribute[];
}

/** 设置 Schema 控件 */
export interface SettingsSchemaField {
  key: string;
  type: 'select' | 'switch' | 'slider' | 'textarea' | 'keybind' | 'textbox' | 'password' | 'color';
  label: string;
  default?: unknown;
  // select
  options?: string[];
  // slider
  min?: number;
  max?: number;
  // 通用
  description?: string;
}

/** 签名信息 */
export interface ManifestSignature {
  algorithm: 'ed25519';
  value: string;
  publicKey: string;
}

/** 发布方信息 */
export interface ManifestPublisher {
  id: string;
  level: 'verified' | 'community' | 'local';
}

/** 插件 manifest 完整定义 */
export interface PluginManifest {
  // ---- 基础元数据 ----
  id: string;
  name: string;
  version: string;
  description?: string;
  author: ManifestAuthor;
  license: string;
  repository?: string;
  icon?: string;
  screenshots?: string[];

  // ---- 框架兼容 ----
  apiVersion: string;
  minFrameworkVersion: string;
  platforms: string[];
  type: PluginType;

  // ---- 入口 ----
  entry: ManifestEntry;

  // ---- 能力申请 ----
  permissions: string[];
  hostFunctions?: string[];

  // ---- UI 扩展点 ----
  contributes?: ManifestContributes;

  // ---- 设置 Schema ----
  settingsSchema?: SettingsSchemaField[];

  // ---- 签名 ----
  signature?: ManifestSignature;

  // ---- 发布方 ----
  publisher?: ManifestPublisher;
}
