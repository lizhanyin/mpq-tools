/* mpqtool-gui frontend — Vue 3 + Naive UI，通过 @tauri-apps/api 调用 Rust 后端。
 * 开发：`?mock` 查询参数启用浏览器 mock（见 src/mock/），无需 Tauri 环境。 */

import { createApp } from 'vue'
import App from './App.vue'
import { installIpc } from './ipc'
import { initStore } from './store'
import './style.css'

async function boot() {
  await installIpc()
  initStore()
  createApp(App).mount('#app')
}

boot()
