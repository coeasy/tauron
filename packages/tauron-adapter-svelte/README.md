# @tauron/adapter-svelte

Svelte 适配层 —— createInvokeStore、createEventStore。

## 安装

```bash
pnpm add @tauron/adapter-svelte
```

## 使用

```svelte
<script>
import { createInvokeStore } from '@tauron/adapter-svelte';
const { subscribe, invoke } = createInvokeStore('com.example.plugin');
</script>
```
