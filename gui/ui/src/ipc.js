/* Tauri API 的统一入口：真实环境走 @tauri-apps 包，浏览器开发模式（?mock）
 * 走 src/mock/mock-ipc.js，两者导出相同形状的函数。 */

import { invoke as tauriInvoke } from '@tauri-apps/api/core'
import { open as tauriOpen, save as tauriSave } from '@tauri-apps/plugin-dialog'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { getCurrentWindow } from '@tauri-apps/api/window'

/** 浏览器 mock 模式：仅开发构建 + URL 带 ?mock。 */
export const isMock = import.meta.env.DEV && /[?&]mock\b/.test(location.search)

let mock = null

/** 在 createApp 之前调用（mock 模块按需异步加载）。 */
export async function installIpc() {
  if (isMock) mock = await import('./mock/mock-ipc')
}

export function invoke(cmd, args) {
  return mock ? mock.invoke(cmd, args) : tauriInvoke(cmd, args)
}

export function dialogOpen(opts) {
  return mock ? mock.dialogOpen(opts) : tauriOpen(opts)
}

export function dialogSave(opts) {
  return mock ? mock.dialogSave(opts) : tauriSave(opts)
}

/** 拖拽事件；返回取消函数。mock / 不支持的环境中静默跳过。 */
export async function onDragDropEvent(handler) {
  if (mock) return () => {}
  const webview = getCurrentWebview()
  return webview.onDragDropEvent(handler)
}

export function closeWindow() {
  if (mock) return mock.closeWindow()
  getCurrentWindow().close()
}
