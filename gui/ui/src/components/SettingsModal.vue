<script setup>
import { NModal, NButton, NSlider } from 'naive-ui'
import { S, setFontSize, FONT_MIN, FONT_MAX, FONT_DEFAULT } from '../store'

const PRESETS = [
  { text: '小', size: 11 },
  { text: '中', size: 13 },
  { text: '大', size: 16 },
  { text: '特大', size: 19 },
]
</script>

<template>
  <n-modal
    :show="S.settingsOpen"
    preset="card"
    title="设置"
    :style="{ width: '420px', maxWidth: '94vw' }"
    :mask-closable="false"
    :bordered="false"
    @update:show="(v) => { S.settingsOpen = v }"
  >
    <div class="section-label">字体大小</div>

    <n-slider
      :min="FONT_MIN"
      :max="FONT_MAX"
      :step="0.5"
      :value="S.fontSize"
      :format-tooltip="(v) => v + 'px'"
      @update:value="setFontSize"
    />

    <div class="preset-row">
      <span>预设：</span>
      <n-button v-for="p in PRESETS" :key="p.size" size="small"
                :type="Math.abs(S.fontSize - p.size) < 0.01 ? 'primary' : 'default'"
                @click="setFontSize(p.size)">{{ p.text }}</n-button>
      <n-button size="small" @click="setFontSize(FONT_DEFAULT)">恢复默认</n-button>
    </div>

    <div class="settings-preview">
      <div>预览：MPQ 归档浏览器 — 文件列表 ABCabc 0123456789</div>
      <div class="mono">预览（等宽）：war3map.j  SECTOR=4096</div>
    </div>

    <div class="hint-text">字号立即应用于整个界面，并自动保存。</div>

    <div class="modal-actions">
      <n-button size="small" @click="S.settingsOpen = false">关闭</n-button>
    </div>
  </n-modal>
</template>
