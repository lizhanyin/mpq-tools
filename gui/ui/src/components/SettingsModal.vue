<template>
  <n-modal
    :show="S.settingsOpen"
    preset="card"
    title="设置"
    :style="{ width: '560px', maxWidth: '94vw', borderRadius: '12px' }"
    :content-style="{ maxHeight: 'calc(90vh - 110px)', overflowY: 'auto' }"
    :mask-closable="false"
    :bordered="false"
    @update:show="onShowUpdate"
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
                      :format-tooltip="(v: number) => v + 'px'"
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

      <div class="setting-card">
        <div class="setting-row">
          <div class="setting-info">
            <div class="setting-name">老版本 Warcraft III 目录</div>
            <div class="setting-desc">
              模型预览的贴图在当前归档里找不到时，回退到该目录下的
              War3Patch / War3x / War3xlocal / War3local / War3.mpq
              （按游戏加载顺序）读取原版资源。设置后重新打开模型生效。
            </div>
          </div>
        </div>
        <div class="setting-dir">
          <input class="dir-input" type="text" spellcheck="false"
                 placeholder="例如 E:\Game\Warcraft III"
                 :value="S.war3Dir" @change="onDirInput" />
          <n-button size="small" @click="pickWar3Dir">浏览…</n-button>
          <n-button size="small" quaternary :disabled="!S.war3Dir"
                    title="清除目录（不再回退到老客户端）"
                    @click="clearWar3Dir">清除</n-button>
        </div>
        <div v-if="dirState" class="dir-state" :class="{ ok: dirState.ok }">{{ dirState.text }}</div>
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

<script setup lang="ts">
  import { computed, ref } from 'vue'
  import { NModal, NButton, NSlider, NSwitch } from 'naive-ui'
  import { S, setFontSize, setGridThumbs, setWar3Dir, FONT_MIN, FONT_MAX, FONT_DEFAULT } from '../store'
  import * as ipc from '../ipc'

  function onShowUpdate(v: boolean) {
    S.settingsOpen = v
  }

  // -- 老客户端目录 ----------------------------------------------------------

  const dirState = ref<{ ok: boolean; text: string } | null>(null)

  /** 目录里是否有 War3*.mpq（提示设置是否有效，不阻断保存）。 */
  async function validateDir(dir: string) {
    if (!dir) { dirState.value = null; return }
    const has = await invoke_ls_war3(dir)
    dirState.value = has
      ? { ok: true, text: `已找到 ${has} 个 MPQ（设置有效）` }
      : { ok: false, text: '该目录下没有 War3*.mpq —— 请确认指向老版本客户端根目录' }
  }

  /** 通过后端列目录（Tauri 环境）；mock 下简单给个确定值。 */
  async function invoke_ls_war3(dir: string): Promise<number> {
    try {
      return await ipc.invoke<number>('war3_mpq_count', { dir })
    } catch {
      return 0
    }
  }

  function onDirInput(ev: Event) {
    const v = (ev.target as HTMLInputElement).value.trim()
    setWar3Dir(v)
    validateDir(v)
  }

  async function pickWar3Dir() {
    const picked = await ipc.dialogOpen({
      title: '选择老版本 Warcraft III 目录（包含 War3.mpq）',
      directory: true,
    })
    const dir = Array.isArray(picked) ? null : picked
    if (!dir) return
    setWar3Dir(dir)
    validateDir(dir)
  }

  function clearWar3Dir() {
    setWar3Dir('')
    dirState.value = null
  }

  // 首次打开设置时对已有目录做一次校验。
  if (S.war3Dir) validateDir(S.war3Dir)

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
  const markStyle = (size: number) => ({
    left: `calc((100% - ${HANDLE_W}px) * ${(size - FONT_MIN) / (FONT_MAX - FONT_MIN)} + ${HANDLE_W / 2}px)`,
  })
</script>
