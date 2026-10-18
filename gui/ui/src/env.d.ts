/// <reference types="vite/client" />

/* TS 文件导入 .vue 单文件组件时的垫片（组件内部暂为 JS，随迁移逐个移除）。 */
declare module '*.vue' {
  import type { DefineComponent } from 'vue'
  const component: DefineComponent<{}, {}, any>
  export default component
}

/* PreviewWindow 暴露的 Monaco 调试 / 自动化测试句柄。 */
interface Window {
  __previewEditor?: unknown
}
