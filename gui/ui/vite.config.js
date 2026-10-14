import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// Tauri 嵌入 WebView2（Chromium evergreen），target 无需兼容旧浏览器。
export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
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
