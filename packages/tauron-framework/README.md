# @tauron/framework

tauron 框架薄封装：**细粒度 signals 响应式内核** + **框架无关 bindings**。

## 定位

本包不绑定任何 UI 框架（React/Vue/Svelte/Lit 均可使用）：

- `signals.ts` — 自实现的细粒度响应式原语（signal/computed/effect/batch），
  提供比虚拟 DOM diff 更精确的更新粒度。
- `bindings.ts` — 与 tauron 宿主交互的框架无关函数式绑定
  （`useInvoke` / `useCapabilities` / `useEvent` / `usePluginId` / `createReactiveStore`），
  在任意框架的组件生命周期中直接调用。

## 安装

```bash
pnpm add @tauron/framework
```

## 快速上手

```typescript
import { createSignal, createComputed, createEffect, batch } from '@tauron/framework';

const count = createSignal(0);
const doubled = createComputed(() => count.get() * 2);

const stop = createEffect(() => {
  console.log('doubled =', doubled.get()); // count 变化时自动重跑
});

count.set(5);          // → 输出 "doubled = 10"
batch(() => {          // 批量更新只触发一次
  count.set(6);
  count.set(7);
});
stop();                // 释放 effect
```

宿主能力绑定：

```typescript
import { useInvoke, useEvent } from '@tauron/framework';

// 调用宿主命令（底层走 tauron 信封协议）
const state = useInvoke<{ theme: string }>('settings_get', { key: 'theme' });
// state: { pending, data, error, refetch }

// 订阅事件总线 topic，返回取消函数
const off = useEvent('plugin:p:theme-changed', (payload) => { /* ... */ });
```

## API 一览

| 分组 | 导出 |
|---|---|
| 信号 | `createSignal` `createComputed` `createEffect` `batch` `enqueueBatch` |
| 消费 | `useSignal` `watchSignal` `Signal` `Computed` `Effect` |
| 宿主绑定 | `useInvoke` `useCapabilities` `useEvent` `usePluginId` `createReactiveStore` |

## 相关包

- `@tauron/core` — 信封调用层 / 后端抽象（bindings 的底层）
- `@tauron/adapter-react` / `@tauron/adapter-vue` / `@tauron/adapter-svelte` — 各框架的独立适配层（与本包平级，不互相依赖）
