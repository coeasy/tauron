# @tauron/adapter-react

React 适配层 —— TauronProvider、useInvoke、useEvent、store。

## 安装

```bash
pnpm add @tauron/adapter-react
```

## 使用

```tsx
import { TauronProvider, useInvoke } from '@tauron/adapter-react';

function App() {
  return <TauronProvider backend={backend}><Main /></TauronProvider>;
}

function Main() {
  const { state, invoke } = useInvoke('com.example.plugin');
  return <button onClick={() => invoke('format', {})}>Format</button>;
}
```
