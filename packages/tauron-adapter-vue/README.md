# @tauron/adapter-vue

Vue 适配层 —— useInvoke、useEvent、store。

## 安装

```bash
pnpm add @tauron/adapter-vue
```

## 使用

```vue
<script setup>
import { useInvoke } from '@tauron/adapter-vue';

const { state, invoke } = useInvoke('com.example.plugin');
</script>
```
