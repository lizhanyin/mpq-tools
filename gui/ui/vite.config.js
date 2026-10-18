import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// Tauri 嵌入 WebView2（Chromium evergreen），target 无需兼容旧浏览器。
export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  // mdx-m3-viewer（CJS）require 了 Node 内建 events；dev 预打包不会自动
  // shim，钉到 events 浏览器 polyfill（构建时也保持同一实现）。
  resolve: {
    alias: {
      events: fileURLToPath(new URL('./node_modules/events/events.js', import.meta.url)),
    },
  },
  // mock 的 MDX 样例资产按二进制原样伺服（默认会当 JS 解析）。
  assetsInclude: ['**/*.mdx.bin', '**/*.blp'],
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    outDir: 'dist',
    target: 'es2022',
    chunkSizeWarningLimit: 1600,
  },
})
