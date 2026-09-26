import { defineConfig } from 'vite';
import { resolve } from 'node:path';

// 多入口：index.html（宿主主窗）+ plugin.html（沙箱 iframe 插件页，legacy）+
// plugin-window.html（插件面板窗口页，0.4-A2 主推 SDK）。
// 沙箱 iframe 是 opaque origin，产物资源必须用相对路径（base: './'）。
export default defineConfig({
  base: './',
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    // tauri dev 依赖固定端口
    watch: {
      ignored: ['**/src-tauri/**'],
    },
  },
  build: {
    target: 'es2022',
    rollupOptions: {
      input: {
        main: resolve(__dirname, 'index.html'),
        plugin: resolve(__dirname, 'plugin.html'),
        pluginWindow: resolve(__dirname, 'plugin-window.html'),
      },
    },
  },
});
