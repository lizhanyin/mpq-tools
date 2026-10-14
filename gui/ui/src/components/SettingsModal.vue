<script setup>
import { computed, ref, watch } from 'vue'
import { NModal, NButton, NSelect, NSlider } from 'naive-ui'
import { S, setFontSize, FONT_MIN, FONT_MAX, FONT_DEFAULT } from '../store'

const CUSTOM = 'custom'

const PRESETS = [
  { text: '小', size: 11 },
  { text: '中（默认）', size: 13 },
  { text: '大', size: 16 },
  { text: '特大', size: 19 },
  { text: '超大', size: 24 },
]

const options = [
  ...PRESETS.map((p) => ({ label: `${p.text} — ${p.size} px`, value: p.size })),
  { label: '自定义', value: CUSTOM },
]

/** 用户显式选择「自定义」后保持该状态，直到从下拉切回某个预设。 */
const customMode = ref(false)
const matchedPreset = computed(() => PRESETS.find((p) => Math.abs(S.fontSize - p.size) < 0.01))
const selectValue = computed(() => (customMode.value || !matchedPreset.value) ? CUSTOM : matchedPreset.value.size)

function onSelect(v) {
  if (v === CUSTOM) {
    customMode.value = true
  } else {
    customMode.value = false
    setFontSize(v)
  }
}

// 每次打开弹窗时按当前字号重置：命中预设则显示预设，否则显示「自定义」。
watch(() => S.settingsOpen, (open) => {
  if (open) customMode.value = !matchedPreset.value
})
</script>

<template>
  <n-modal
    :show="S.settingsOpen"
    preset="card"
    title="界面设置"
    :style="{ width: '560px', maxWidth: '94vw', minHeight: '420px', borderRadius: '12px' }"
    :mask-closable="false"
    :bordered="false"
    @update:show="(v) => { S.settingsOpen = v }"
  >
    <div class="dialog-body settings-body">
      <div class="section-head">
        <span class="section-title">字体大小</span>
        <span class="section-count">当前 {{ S.fontSize }} px</span>
      </div>

      <n-select
        size="large"
        :value="selectValue"
        :options="options"
        @update:value="onSelect"
      />

      <template v-if="selectValue === CUSTOM">
        <div class="section-head">
          <span class="section-title">自定义字号</span>
          <span class="section-count">{{ FONT_MIN }} – {{ FONT_MAX }} px，步进 0.5</span>
        </div>

        <n-slider
          :min="FONT_MIN"
          :max="FONT_MAX"
          :step="0.5"
          :value="S.fontSize"
          :format-tooltip="(v) => v + 'px'"
          @update:value="setFontSize"
        />
      </template>

      <div class="settings-preview">
        <div>预览：MPQ 归档浏览器 — 文件列表 ABCabc 0123456789</div>
        <div class="mono">预览（等宽）：war3map.j  SECTOR=4096</div>
      </div>

      <div class="hint-text">字号立即应用于整个界面，并自动保存。</div>
    </div>

    <template #action>
      <div class="dialog-actions">
        <n-button v-if="selectValue === CUSTOM" size="small" quaternary @click="setFontSize(FONT_DEFAULT)">
          恢复默认
        </n-button>
        <div class="grow"></div>
        <n-button size="small" @click="S.settingsOpen = false">关闭</n-button>
      </div>
    </template>
  </n-modal>
</template>
