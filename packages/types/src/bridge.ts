/**
 * tauron postMessage 代理协议（设计文档 §2.4）
 *
 * JS 插件沙箱通信协议：
 * - Host 侧（PluginBridge）→ iframe
 * - iframe（插件）→ Host 侧
 *
 * 握手流程：
 * 1. Host 创建 iframe 时注入随机 token
 * 2. 插件加载后发送 { action: 'ready', token } 验证身份
 * 3. Host 回复 { action: 'init', permissions: [...] } 授予能力
 * 4. 正式通信开始，所有消息携带 token
 */

/**
 * 消息类型标识。
 *
 * 取值沿用框架的事件命名空间风格（`plugin:<id>:<event>`），
 * 而不是 SCREAMING_SNAKE —— 它是**线上判别值**，不是常量名。
 */
export const BRIDGE_MESSAGE_TYPE = 'tauron:bridge' as const;

/** Host → 插件消息 */
export interface BridgeToPluginMessage {
  type: typeof BRIDGE_MESSAGE_TYPE;
  direction: 'host-to-plugin';
  token: string;
  action: 'init' | 'invoke-result' | 'event' | 'cancel';
  payload: unknown;
}

/** 插件 → Host 消息 */
export interface PluginToBridgeMessage {
  type: typeof BRIDGE_MESSAGE_TYPE;
  direction: 'plugin-to-host';
  action: 'invoke' | 'emit' | 'ready' | 'error';
  callId?: string;
  payload: unknown;
}

/** 联合类型 */
export type BridgeMessage = BridgeToPluginMessage | PluginToBridgeMessage;

/**
 * 构建 init 消息（Host → 插件）
 */
export function buildInitMessage(token: string, permissions: string[]): BridgeToPluginMessage {
  return {
    type: BRIDGE_MESSAGE_TYPE,
    direction: 'host-to-plugin',
    token,
    action: 'init',
    payload: { permissions },
  };
}

/**
 * 构建 ready 消息（插件 → Host）
 */
export function buildReadyMessage(token: string): PluginToBridgeMessage {
  return {
    type: BRIDGE_MESSAGE_TYPE,
    direction: 'plugin-to-host',
    action: 'ready',
    payload: { token },
  };
}

/**
 * 构建 invoke 消息（插件 → Host）
 */
export function buildInvokeMessage(
  callId: string,
  method: string,
  args: unknown,
): PluginToBridgeMessage {
  return {
    type: BRIDGE_MESSAGE_TYPE,
    direction: 'plugin-to-host',
    action: 'invoke',
    callId,
    payload: { method, args },
  };
}

/**
 * 构建 invoke-result 消息（Host → 插件）
 */
export function buildInvokeResultMessage(
  token: string,
  callId: string,
  result: unknown,
): BridgeToPluginMessage {
  return {
    type: BRIDGE_MESSAGE_TYPE,
    direction: 'host-to-plugin',
    token,
    action: 'invoke-result',
    payload: { callId, result },
  };
}

/**
 * 构建 event 消息（插件 → Host）
 */
export function buildEmitMessage(
  eventName: string,
  payload: unknown,
): PluginToBridgeMessage {
  return {
    type: BRIDGE_MESSAGE_TYPE,
    direction: 'plugin-to-host',
    action: 'emit',
    payload: { eventName, payload },
  };
}

/**
 * 构建 event 消息（Host → 插件）
 *
 * 与 {@link buildEmitMessage}（插件 → Host）方向相反，构成完整的事件回路。
 */
export function buildEventMessage(
  token: string,
  eventName: string,
  payload: unknown,
): BridgeToPluginMessage {
  return {
    type: BRIDGE_MESSAGE_TYPE,
    direction: 'host-to-plugin',
    token,
    action: 'event',
    payload: { eventName, payload },
  };
}

/**
 * 构建 cancel 消息（Host → 插件）
 */
export function buildCancelMessage(token: string, callId: string): BridgeToPluginMessage {
  return {
    type: BRIDGE_MESSAGE_TYPE,
    direction: 'host-to-plugin',
    token,
    action: 'cancel',
    payload: { callId },
  };
}

/**
 * 验证消息是否来自正确的方向
 */
export function isHostToPlugin(msg: BridgeMessage): msg is BridgeToPluginMessage {
  return msg.type === BRIDGE_MESSAGE_TYPE && msg.direction === 'host-to-plugin';
}

export function isPluginToHost(msg: BridgeMessage): msg is PluginToBridgeMessage {
  return msg.type === BRIDGE_MESSAGE_TYPE && msg.direction === 'plugin-to-host';
}
