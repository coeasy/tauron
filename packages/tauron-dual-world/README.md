# @tauron/dual-world

双世界骨架 —— 隔离沙箱的**接口与策略层** + Host JS bridge。

> ⚠️ **诚实边界（轮 11 审计，请先读这段）**
>
> - 标题此前写的是「QuickJS-WASM 沙箱」，但**本包不含任何 JS 运行时**（依赖闭包里
>   没有 QuickJS-WASM，硬约束禁止新增依赖）。
> - `sandbox.execute(code)` 现在是 **fail closed**：返回
>   `{ ok: false, error: { code: 'SANDBOX_UNAVAILABLE', … } }`，**不执行任何代码**。
>   修正前它 `setTimeout` 随机延迟后返回 `{ ok: true, result: { executed: true } }`——
>   一行都没跑却声称执行成功（调用方会据此认为插件代码已通过），现在不会了。
> - 仍然**真实**的部分：`hostFunctions` 白名单与调用面、`memoryLimit <= 0` 的拒绝、
>   状态容器（`getState`/`setState`）、`call()` 对 host bridge 的真实转发、
>   `destroy()` 清理，以及 bridge 侧的 host 函数白名单校验。
> - 接入 QuickJS-WASM 后，请把 `execute()` 改成真实执行，并把
>   `sandbox.test.ts` 里那条 fail-closed 断言替换为真实执行断言。

## 安装

```bash
pnpm add @tauron/dual-world
```

## 使用

```typescript
import { createSandbox, createBridge } from '@tauron/dual-world';

const sandbox = createSandbox();
const bridge = createBridge({
  allowedHostFunctions: ['invoke', 'emit'],
});

// execute() 当前会如实返回失败（未接入运行时）：
const result = await sandbox.execute('console.log("hello")');
// → { ok: false, error: { code: 'SANDBOX_UNAVAILABLE', … } }
```
