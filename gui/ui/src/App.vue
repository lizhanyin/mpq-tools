<script setup>
import { computed, onMounted, onUnmounted } from 'vue'
import { NConfigProvider } from 'naive-ui'
import MenuBar from './components/MenuBar.vue'
import StatusBar from './components/StatusBar.vue'
import Welcome from './components/Welcome.vue'
import BrowserPane from './components/BrowserPane.vue'
import LogPanel from './components/LogPanel.vue'
import PreviewWindow from './components/PreviewWindow.vue'
import RepairModal from './components/RepairModal.vue'
import ModifyModal from './components/ModifyModal.vue'
import NewArchiveModal from './components/NewArchiveModal.vue'
import NewFolderModal from './components/NewFolderModal.vue'
import ConvertModal from './components/ConvertModal.vue'
import PasswordModal from './components/PasswordModal.vue'
import SettingsModal from './components/SettingsModal.vue'
import { S, reloadArchive, handleDrop, anyModalOpen, selectAll, log } from './store'
import { toast } from './toast'
import * as ipc from './ipc'

const ACCENT = '#2f6fe0'
const FONT_SANS = 'system-ui, "Segoe UI", "Microsoft YaHei", "PingFang SC", sans-serif'

// 根字号与 Naive 组件字号同步缩放。
const themeOverrides = computed(() => {
  const v = S.fontSize
  return {
    common: {
      fontFamily: FONT_SANS,
      fontSize: `${v - 1}px`,
      fontSizeMini: `${v - 3}px`,
      fontSizeSmall: `${v - 1}px`,
      fontSizeMedium: `${v}px`,
      fontSizeLarge: `${v + 2}px`,
      borderRadius: '4px',
      primaryColor: ACCENT,
      primaryColorHover: '#255cb8',
      primaryColorPressed: '#1e4da0',
      primaryColorSuppl: ACCENT,
    },
  }
})

let offDrop = null
function onKeyDown(ev) {
  if (ev.key === 'F5') {
    ev.preventDefault()
    reloadArchive()
    return
  }
  // Ctrl+A：文件列表全选（当前视图范围）。文本输入/编辑场景保留原生全选。
  if ((ev.ctrlKey || ev.metaKey) && !ev.altKey && !ev.shiftKey && ev.key.toLowerCase() === 'a') {
    const t = ev.target
    if (t?.closest?.('input, textarea, [contenteditable="true"], .monaco-editor, .log-body')) return
    if (!S.info || anyModalOpen.value) return
    ev.preventDefault()
    selectAll()
  }
}

onMounted(async () => {
  try {
    offDrop = await ipc.onDragDropEvent((event) => {
      const p = event.payload
      // 有弹窗/浮窗打开时忽略拖拽：不显示「松开导入」覆盖层，
      // 松手时提示原因（拖拽事件在 webview 层触发，不受弹窗遮罩限制）。
      if (anyModalOpen.value) {
        if (p.type === 'drop' && (p.paths || []).length > 0) {
          log('（有窗口打开，已忽略拖入的文件）')
          toast.info('有窗口打开时不能拖拽导入，请先关闭窗口')
        }
        return
      }
      if (p.type === 'enter') {
        // 只有 enter/drop 事件携带 paths；over 仅有 position，
        // 不能用它重算数量（否则一移动鼠标就变成 0 个文件）。
        S.dropping = { count: (p.paths || []).length }
      } else if (p.type === 'over') {
        S.dropping ??= { count: 0 }
      } else if (p.type === 'drop') {
        S.dropping = null
        handleDrop(p.paths || [])
      } else if (p.type === 'leave' || p.type === 'exit') {
        S.dropping = null
      }
    })
  } catch (e) {
    // 拖拽只是便捷功能，绝不致命
    console.warn('onDragDropEvent unavailable:', e)
  }
  window.addEventListener('keydown', onKeyDown)
})

onUnmounted(() => {
  if (offDrop) offDrop()
  window.removeEventListener('keydown', onKeyDown)
})
</script>

<template>
  <n-config-provider :theme-overrides="themeOverrides">
    <div class="app-root">
      <MenuBar />
      <StatusBar />
      <div class="main">
        <Welcome v-if="!S.info" />
        <BrowserPane v-else />
      </div>
      <LogPanel />

      <PreviewWindow v-if="S.preview" />
      <RepairModal />
      <ModifyModal />
      <NewArchiveModal />
      <NewFolderModal />
      <ConvertModal />
      <PasswordModal />
      <SettingsModal />

      <div v-if="S.dropping" class="drop-overlay">
        <div class="banner">{{
          S.info ? `📥 松开鼠标以导入 ${S.dropping.count} 项文件/文件夹到当前归档`
                 : '📂 松开鼠标以打开归档'
        }}</div>
      </div>
    </div>
  </n-config-provider>
</template>
