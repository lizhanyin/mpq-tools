/* Tauri API 的统一入口：真实环境走 @tauri-apps 包，浏览器开发模式（?mock）
 * 走 src/mock/mock-ipc.js，两者导出相同形状的函数。
 *
 * 命令的参数 / 返回类型契约见 types.ts：参数键 camelCase，返回 DTO
 * 字段 snake_case。调用方用 `invoke<T>(cmd, args)` 标注返回类型。 */

import { invoke as tauriInvoke } from '@tauri-apps/api/core'
import type { EventCallback, UnlistenFn } from '@tauri-apps/api/event'
import type { DragDropEvent } from '@tauri-apps/api/webview'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { open as tauriOpen, save as tauriSave } from '@tauri-apps/plugin-dialog'
import type { OpenDialogOptions, SaveDialogOptions } from '@tauri-apps/plugin-dialog'

/** 浏览器 mock 模式：仅开发构建 + URL 带 ?mock。 */
export const isMock: boolean = import.meta.env.DEV && /[?&]mock\b/.test(location.search)

type MockIpc = typeof import('./mock/mock-ipc')

let mock: MockIpc | null = null

/** 在 createApp 之前调用（mock 模块按需异步加载）。 */
export async function installIpc(): Promise<void> {
  if (isMock) mock = await import('./mock/mock-ipc')
}

/**
 * 调用 Rust 命令。Tauri v2 约定：参数键用 camelCase（Rust 侧 snake_case，
 * 如前端 `hashTableSize` → Rust `hash_table_size`）；返回的 DTO 字段保持
 * Rust 原名（snake_case），类型定义见 types.ts。
 */
export function invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return (mock ? mock.invoke(cmd, args) : tauriInvoke(cmd, args)) as Promise<T>
}

export function dialogOpen(opts?: OpenDialogOptions): Promise<string | string[] | null> {
  return (mock ? mock.dialogOpen(opts) : tauriOpen(opts)) as Promise<string | string[] | null>
}

export function dialogSave(opts?: SaveDialogOptions): Promise<string | null> {
  return (mock ? mock.dialogSave(opts) : tauriSave(opts)) as Promise<string | null>
}

/** 拖拽事件；返回取消函数。mock / 不支持的环境中静默跳过。 */
export async function onDragDropEvent(
  handler: EventCallback<DragDropEvent>,
): Promise<UnlistenFn> {
  if (mock) return () => {}
  const webview = getCurrentWebview()
  return webview.onDragDropEvent(handler)
}

export function closeWindow(): void {
  if (mock) {
    mock.closeWindow()
    return
  }
  getCurrentWindow().close()
}
