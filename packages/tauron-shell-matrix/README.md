# @tauron/shell-matrix

Shell 矩阵 —— 4 种 Shell 形态 (local/local-server/remote-url/sub-webview)。

> ⚠️ **诚实边界（轮 11 审计，请先读这段）**
>
> - 本包是**形态矩阵 + 生命周期骨架**：`start()` **不装载任何资源**——不读文件、
>   不起本地服务器、不建 webview，只 `setTimeout` 模拟延迟后把 `status` 置为 `ready`。
> - 因此 **`status: 'ready'` 不等于"可用"**：请先看实例上的 `simulated`（当前恒为
>   `true`）再判断 shell 是否真的就绪。
> - 本包目前**没有任何消费者**（包与 crate 层面都无依赖方）。
> - 真正的装载在宿主/适配层（`crates/tauron-adapter` 的窗口与 `contributes` 面）。

## 安装

```bash
pnpm add @tauron/shell-matrix
```

## 使用

```typescript
import { createShellManager } from '@tauron/shell-matrix';

const shell = createShellManager();
const instance = shell.create({
  pluginId: 'com.example.plugin',
  form: 'local-server',
  host: 'localhost',
  port: 8080,
});
await instance.start();
```
