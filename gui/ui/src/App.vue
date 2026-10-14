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
import SettingsModal from './components/SettingsModal.vue'
import { S, reloadArchive, handleDrop } from './store'
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
  }
}

onMounted(async () => {
  try {
    offDrop = await ipc.onDragDropEvent((event) => {
      const p = event.payload
      if (p.type === 'enter' || p.type === 'over') {
        S.dropping = { count: (p.paths || []).length }
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
      <SettingsModal />

      <div v-if="S.dropping" class="drop-overlay">
        <div class="banner">{{
          S.info ? `📥 松开鼠标以导入 ${S.dropping.count} 个文件到当前归档`
                 : '📂 松开鼠标以打开归档'
        }}</div>
      </div>
    </div>
  </n-config-provider>
</template>
