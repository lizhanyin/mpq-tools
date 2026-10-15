/* mpqtool-gui frontend — Vue 3 + Naive UI，通过 @tauri-apps/api 调用 Rust 后端。
 * 开发：`?mock` 查询参数启用浏览器 mock（见 src/mock/），无需 Tauri 环境。 */

import { createApp } from 'vue'
import App from './App.vue'
import { installIpc } from './ipc'
import { initStore } from './store'
import './style.css'

/* 屏蔽 webview 默认右键菜单。只在 document 层 preventDefault、不拦截
 * 传播，Monaco 等组件内部的右键菜单仍照常工作。 */
document.addEventListener('contextmenu', (e) => e.preventDefault())

async function boot() {
  await installIpc()
  initStore()
  createApp(App).mount('#app')
}

boot()
