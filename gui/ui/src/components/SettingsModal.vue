<template>
  <n-modal
    :show="S.settingsOpen"
    preset="card"
    title="设置"
    :style="{ width: '560px', maxWidth: '94vw', borderRadius: '12px' }"
    :content-style="{ maxHeight: 'calc(90vh - 110px)', overflowY: 'auto' }"
    :mask-closable="false"
    :bordered="false"
    @update:show="(v) => { S.settingsOpen = v }"
  >
    <div class="dialog-body settings-body">
      <div class="setting-card">
        <div class="setting-row">
          <div class="setting-info">
            <div class="setting-name">字体大小</div>
            <div class="setting-desc">立即应用于整个界面并自动保存；拖动滑块微调，点击下方标签直达预设值。</div>
          </div>
          <span class="setting-value">{{ S.fontSize }} px</span>
        </div>

        <!-- 滑块常驻；预设以刻度标签形式按数值比例定位，点击直达 -->
        <div class="setting-slider">
          <div class="slider-wrap">
            <n-slider :min="FONT_MIN" :max="FONT_MAX" :step="0.5"
                      :value="S.fontSize"
                      :format-tooltip="(v) => v + 'px'"
                      @update:value="setFontSize" />
            <div class="slider-marks">
              <button v-for="p in PRESETS" :key="p.size" type="button" class="slider-mark"
                      :class="{ active: matchedPreset?.size === p.size }"
                      :style="markStyle(p.size)"
                      :title="p.text + ' — ' + p.size + ' px'"
                      @click="setFontSize(p.size)">{{ p.text }}</button>
            </div>
          </div>
          <n-button size="tiny" quaternary :disabled="S.fontSize === FONT_DEFAULT"
                    title="恢复默认字号"
                    @click="setFontSize(FONT_DEFAULT)">恢复默认</n-button>
        </div>

        <div class="settings-preview">
          <div>预览：MPQ 归档浏览器 — 文件列表 ABCabc 0123456789</div>
          <div class="mono">预览（等宽）：war3map.j  SECTOR=4096</div>
        </div>
      </div>

      <div class="setting-card">
        <div class="setting-row">
          <div class="setting-info">
            <div class="setting-name">图片缩略图</div>
            <div class="setting-desc">
              网格视图中为图片文件（含 TGA / BLP）显示缩略图。需要逐个解码归档内容，
              图片很多时首次滚动会稍慢；开关立即生效并自动保存。
            </div>
          </div>
          <n-switch size="large" :value="S.gridThumbs" @update:value="setGridThumbs" />
        </div>
      </div>
    </div>

    <template #action>
      <div class="dialog-actions">
        <div class="grow"></div>
        <n-button size="small" @click="S.settingsOpen = false">关闭</n-button>
      </div>
    </template>
  </n-modal>
</template>

<script setup>
  import { computed } from 'vue'
  import { NModal, NButton, NSlider, NSwitch } from 'naive-ui'
  import { S, setFontSize, setGridThumbs, FONT_MIN, FONT_MAX, FONT_DEFAULT } from '../store'

  const PRESETS = [
    { text: '小', size: 11 },
    { text: '中', size: 13 },
    { text: '大', size: 16 },
    { text: '特大', size: 19 },
    { text: '超大', size: 24 },
  ]

  /** 当前字号命中的预设；滑块微调到预设之间时无高亮（即为自定义值）。 */
  const matchedPreset = computed(() => PRESETS.find((p) => Math.abs(S.fontSize - p.size) < 0.01))

  /** 刻度标签与把手对齐：naive-ui 把把手中心映射为 P×(轨宽−18px)+9px，而非简单百分比。 */
  const HANDLE_W = 18
  const markStyle = (size) => ({
    left: `calc((100% - ${HANDLE_W}px) * ${(size - FONT_MIN) / (FONT_MAX - FONT_MIN)} + ${HANDLE_W / 2}px)`,
  })
</script>
