<template>
  <div class="fwindow fwindow-editor" :class="{ maximized }"
       :style="{ left: x + 'px', top: y + 'px', width: w + 'px', height: h + 'px' }">
    <div ref="titleEl" class="fwin-title" @pointerdown="startDrag">
      <!-- 图片预览：文件名与大小等信息直接放标题栏，不再占内部一行 -->
      <template v-if="p?.image">
        <strong class="mono title-name" :title="p?.name">{{ p?.name }}</strong>
        <span class="weak title-note" :title="headerNote">{{ headerNote }}</span>
      </template>
      <span v-else>预览</span>
      <span v-if="p?.dirty" class="dirty-dot" title="有未保存的修改">●</span>
      <span class="spacer"></span>
      <n-button v-if="p?.image" size="tiny" quaternary title="逆时针旋转 90°" @click="rotate(-1)">↺</n-button>
      <n-button v-if="p?.image" size="tiny" quaternary title="顺时针旋转 90°" @click="rotate(1)">↻</n-button>
      <n-button v-if="!p?.image" size="tiny" type="primary" :disabled="!p?.editable"
                :loading="p?.saving"
                title="把编辑内容排队到「修改归档」，随后在弹窗中确认写入（Ctrl+S）"
                @click="save">保存</n-button>
      <n-button size="tiny" quaternary :title="maximized ? '还原' : '最大化'"
                @click="toggleMaximize">⛶</n-button>
      <n-button size="tiny" quaternary title="关闭" @click="tryClose">✕</n-button>
    </div>
    <div v-if="!p?.image" class="fwin-head">
      <strong class="mono">{{ p?.name }}</strong>
      <span class="weak">{{ headerNote }}</span>
    </div>
    <div class="fwin-editor">
      <div v-if="loading && !p?.image" class="editor-loading">正在加载编辑器…</div>
      <div v-show="!p?.image" ref="editorHost" class="editor-host"></div>
      <!-- 图片：滚轮缩放（跟随光标）、按住拖动平移、点击切换 100%/完整显示 -->
      <div v-if="p?.image" ref="imgBox" class="fwin-img" :class="{ panning }"
           title="滚轮缩放，拖动平移；点击在 100% 与完整显示间切换"
           @wheel.prevent="onWheel" @pointerdown="startPan" @click="onImgClick">
        <div v-if="!imgFailed" class="img-frame" :style="frameStyle">
          <img :src="p.image.url" alt="" draggable="false" :style="imgStyle"
               @load="onImgLoad" @error="imgFailed = true">
        </div>
        <div v-else class="img-fallback">图片解码失败：文件可能不是有效的图片</div>
      </div>
      <!-- 左右切换：视图中还有其它图片时显示（含搜索结果视图） -->
      <template v-if="p?.image && imgCount > 1">
        <button class="img-nav prev" :disabled="imgIndex <= 0"
                title="上一张（←）" @click="switchImage(-1)">‹</button>
        <button class="img-nav next" :disabled="imgIndex < 0 || imgIndex >= imgCount - 1"
                title="下一张（→）" @click="switchImage(1)">›</button>
      </template>
    </div>
    <div v-if="!maximized" class="fwin-resize" @pointerdown="startResize"></div>
  </div>
</template>

<script setup>
  import { ref, computed, onMounted, onUnmounted, watch, nextTick } from 'vue'
  import { NButton } from 'naive-ui'
  import { S, rows, previewFile, closePreview, savePreviewEdit } from '../store'
  import { loadMonaco, languageFor } from '../monaco'
  import { fmtBytes, isImageName } from '../utils'
  import { toast } from '../toast'

  // 非模态浮动窗口；标题栏拖动、右下角调整大小。Monaco 编辑器填充窗体。
  const x = ref(90)
  const y = ref(70)
  const w = ref(720)
  const h = ref(520)
  const maximized = ref(false)
  let savedRect = null // 最大化前的位置尺寸，还原时用
  const titleEl = ref(null)
  const editorHost = ref(null)
  const loading = ref(true)
  let editor = null
  let monaco = null
  let applying = false // 程序化 setValue 不计入 dirty

  const p = computed(() => S.preview)
  const zoom = ref(1)        // 图片缩放：1 = 原始像素；完整显示时为 contain 比例
  const rot = ref(0)         // 旋转角（0/90/180/270）
  const dims = ref('')       // 图片加载后的自然尺寸（如 512×512）
  const natW = ref(0)        // 自然宽高（数值形式），供缩放 / 旋转计算
  const natH = ref(0)
  const imgFailed = ref(false)
  const imgBox = ref(null)   // 图片滚动容器：平移 = 拖动它的滚动
  const panning = ref(false)
  let suppressClick = false  // 拖动平移后不再当作点击缩放

  const headerNote = computed(() => {
    const bits = []
    if (!p.value) return ''
    if (p.value.image) {
      if (imgCount.value > 1 && imgIndex.value >= 0) {
        bits.push(`第 ${imgIndex.value + 1}/${imgCount.value} 张`)
      }
      if (dims.value) bits.push(dims.value)
      bits.push(fmtBytes(p.value.size))
      bits.push(`${Math.round(zoom.value * 100)}%`)
      if (p.value.image.converted) bits.push('已转换为 PNG')
      return bits.join('，')
    }
    if (p.value.encoding !== 'UTF-8') bits.push(`按 ${p.value.encoding} 解码`)
    if (p.value.truncated) bits.push(`仅加载前 256 KiB / 共 ${fmtBytes(p.value.size)} 字节，只读`)
    else if (p.value.binary) bits.push('二进制内容，只读')
    else bits.push(`${fmtBytes(p.value.size)} 字节`)
    return bits.join('，')
  })

  /** 图片显示尺寸 = 自然尺寸 × 缩放；旋转 90/270 时外层占位宽高互换。 */
  const dispW = computed(() => Math.round(natW.value * zoom.value))
  const dispH = computed(() => Math.round(natH.value * zoom.value))
  const frameStyle = computed(() => ({
    width: (rot.value % 180 ? dispH.value : dispW.value) + 'px',
    height: (rot.value % 180 ? dispW.value : dispH.value) + 'px',
  }))
  const imgStyle = computed(() => ({
    width: dispW.value + 'px',
    height: dispH.value + 'px',
    // 先平移居中，再绕图片中心旋转，视觉上恰好填满外层 frame
    transform: `translate(-50%, -50%) rotate(${rot.value}deg)`,
  }))

  // -- 图片左右切换 ---------------------------------------------------------------

  /** 当前视图（含搜索视图）里的图片条目，按列表顺序切换。 */
  const imgEntries = computed(() =>
    rows.value.list.filter((r) => r.type === 'file' && r.entry.name && isImageName(r.entry.name)))
  const imgIndex = computed(() =>
    imgEntries.value.findIndex((r) => r.entry.name === p.value?.name))
  const imgCount = computed(() => imgEntries.value.length)

  function switchImage(dir) {
    const i = imgIndex.value + dir
    if (i < 0 || i >= imgEntries.value.length) return
    previewFile(imgEntries.value[i].entry.name)
  }

  /** 图片预览时 ←/→ 切换；正在输入文本（过滤框等）时不抢按键。
   *  注意不能用 anyModalOpen——它包含预览窗自身，会让本处理器
   *  在预览打开时永远短路。 */
  function onKey(ev) {
    if (!S.preview?.image) return
    if (S.modify || S.create || S.convert || S.pw || S.repair || S.newFolder || S.settingsOpen) return
    const t = ev.target
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return
    if (ev.key === 'ArrowLeft') { switchImage(-1); ev.preventDefault() }
    else if (ev.key === 'ArrowRight') { switchImage(1); ev.preventDefault() }
  }

  function onImgLoad(ev) {
    imgFailed.value = false
    natW.value = ev.target.naturalWidth
    natH.value = ev.target.naturalHeight
    dims.value = `${natW.value}×${natH.value}`
    zoom.value = fitZoom() // 打开 / 切换图片时先完整显示（contain）
  }

  // -- 缩放 / 旋转 / 平移 ---------------------------------------------------------

  /** 缩放后考虑旋转的显示宽高。 */
  const rotW = (z) => Math.round((rot.value % 180 ? natH.value : natW.value) * z)
  const rotH = (z) => Math.round((rot.value % 180 ? natW.value : natH.value) * z)

  /** 完整显示（contain）所需缩放，小图会放大到贴满一边。 */
  function fitZoom() {
    const el = imgBox.value
    if (!el || !natW.value) return 1
    return Math.min(el.clientWidth / rotW(1), el.clientHeight / rotH(1))
  }

  /** 以屏幕坐标 (cx,cy) 为锚点缩放：缩放后光标下的内容点保持不动。
   *  下限为「完整显示」（不再缩小成孤岛），上限 1000%。 */
  function zoomBy(factor, cx, cy) {
    const el = imgBox.value
    if (!el || !natW.value) return
    const oz = zoom.value
    const nz = Math.min(10, Math.max(Math.min(fitZoom(), 1), oz * factor))
    if (Math.abs(nz - oz) < 1e-9) return
    const r = el.getBoundingClientRect()
    // 光标处内容点在 frame 中的比例坐标（0..1）；frame 居中留白在
    // 溢出时为 0，此时 frame 左缘相对视口即 -scrollLeft
    const fx = Math.max(0, (r.width - rotW(oz)) / 2) - el.scrollLeft
    const fy = Math.max(0, (r.height - rotH(oz)) / 2) - el.scrollTop
    const ux = (cx - r.left - fx) / rotW(oz)
    const uy = (cy - r.top - fy) / rotH(oz)
    zoom.value = nz
    nextTick(() => {
      el.scrollLeft = Math.max(0, (r.width - rotW(nz)) / 2) - (cx - r.left - ux * rotW(nz))
      el.scrollTop = Math.max(0, (r.height - rotH(nz)) / 2) - (cy - r.top - uy * rotH(nz))
    })
  }

  function onWheel(ev) {
    if (!S.preview?.image) return
    zoomBy(ev.deltaY < 0 ? 1.1 : 1 / 1.1, ev.clientX, ev.clientY)
  }

  /** 点击（没拖动过）在 100% 与完整显示之间切换。 */
  function onImgClick(ev) {
    if (suppressClick) { suppressClick = false; return }
    if (!imgBox.value || !natW.value) return
    const target = Math.abs(zoom.value - 1) < 1e-3 ? fitZoom() : 1
    zoomBy(target / zoom.value, ev.clientX, ev.clientY)
  }

  /** 旋转 ±90°，保持缩放；原本是完整显示则按新方向重新 contain。 */
  function rotate(dir) {
    const wasFit = Math.abs(zoom.value - fitZoom()) < 1e-3
    rot.value = (rot.value + dir * 90 + 360) % 360
    if (wasFit) zoom.value = fitZoom()
    nextTick(() => {
      const el = imgBox.value
      if (!el) return
      el.scrollLeft = (el.scrollWidth - el.clientWidth) / 2
      el.scrollTop = (el.scrollHeight - el.clientHeight) / 2
    })
  }

  /** 按住拖动平移：内容溢出窗口时四处查看（实现为拖动滚动位置）。 */
  function startPan(ev) {
    if (ev.button !== 0) return
    const el = imgBox.value
    const sx = ev.clientX, sy = ev.clientY
    const sl = el.scrollLeft, st = el.scrollTop
    let moved = false
    suppressClick = false
    el.setPointerCapture(ev.pointerId)
    const move = (e) => {
      const dx = e.clientX - sx, dy = e.clientY - sy
      if (!moved && Math.hypot(dx, dy) > 3) { moved = true; panning.value = true }
      if (moved) { el.scrollLeft = sl - dx; el.scrollTop = st - dy }
    }
    const up = () => {
      el.removeEventListener('pointermove', move)
      el.removeEventListener('pointerup', up)
      el.removeEventListener('pointercancel', up)
      panning.value = false
      if (moved) suppressClick = true
    }
    el.addEventListener('pointermove', move)
    el.addEventListener('pointerup', up)
    el.addEventListener('pointercancel', up)
  }

  // -- 拖动 / 调整大小 / 最大化 ---------------------------------------------------

  /** 最大化铺满视口 / 还原到之前的位置尺寸。 */
  function toggleMaximize() {
    if (maximized.value) {
      if (savedRect) { x.value = savedRect.x; y.value = savedRect.y; w.value = savedRect.w; h.value = savedRect.h }
      maximized.value = false
    } else {
      savedRect = { x: x.value, y: y.value, w: w.value, h: h.value }
      x.value = 0
      y.value = 0
      w.value = window.innerWidth
      h.value = window.innerHeight
      maximized.value = true
    }
  }

  /** 最大化期间浏览器窗口尺寸变化时保持铺满。 */
  function onViewportResize() {
    if (!maximized.value) return
    w.value = window.innerWidth
    h.value = window.innerHeight
  }

  function startDrag(ev) {
    // 点击按钮等交互元素时不启动拖动：setPointerCapture 会吞掉
    // 后续指针事件，导致按钮收不到 click。最大化时也不拖动。
    if (ev.button !== 0 || maximized.value || ev.target.closest('button')) return
    const sx = ev.clientX, sy = ev.clientY
    const ox = x.value, oy = y.value
    const title = titleEl.value
    title.setPointerCapture(ev.pointerId)
    const move = (e) => {
      x.value = Math.max(0, ox + e.clientX - sx)
      y.value = Math.max(0, oy + e.clientY - sy)
    }
    const up = () => {
      title.removeEventListener('pointermove', move)
      title.removeEventListener('pointerup', up)
    }
    title.addEventListener('pointermove', move)
    title.addEventListener('pointerup', up)
  }

  function startResize(ev) {
    if (ev.button !== 0 || maximized.value) return
    ev.preventDefault()
    ev.stopPropagation()
    const sx = ev.clientX, sy = ev.clientY
    const ow = w.value, oh = h.value
    const move = (e) => {
      w.value = Math.max(320, ow + e.clientX - sx)
      h.value = Math.max(200, oh + e.clientY - sy)
    }
    const up = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }

  // -- Monaco -------------------------------------------------------------------

  let loadedName = ''

  async function ensureEditor() {
    if (editor) return
    monaco = await loadMonaco()
    editor = monaco.editor.create(editorHost.value, {
      value: '',
      language: 'plaintext',
      theme: 'vs',
      readOnly: true,
      fontSize: S.fontSize,
      fontFamily: 'Consolas, "Cascadia Mono", monospace',
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      automaticLayout: true, // 窗口拖大拖小时自动重排
      renderWhitespace: 'selection',
      scrollbar: { verticalScrollbarSize: 10, horizontalScrollbarSize: 10 },
    })
    editor.onDidChangeModelContent(() => {
      if (!applying && S.preview) S.preview.dirty = true
    })
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, save)
    // 控制台调试 / 自动化测试用的句柄。
    window.__previewEditor = editor
  }

  /** 把 S.preview 装进编辑器；窗口保持打开时双击了其它文件也会触发。 */
  async function syncModel() {
    const cur = S.preview
    if (!cur) return
    if (cur.image) { loading.value = false; return } // 图片不加载 Monaco
    await ensureEditor()
    if (!editor || loadedName === cur.name) return
    loadedName = cur.name
    applying = true
    editor.setValue(cur.text)
    monaco.editor.setModelLanguage(editor.getModel(), languageFor(cur.name))
    editor.updateOptions({ readOnly: !cur.editable })
    cur.dirty = false
    applying = false
    loading.value = false
  }

  async function save() {
    const cur = S.preview
    if (!cur || !cur.editable || cur.saving || !editor) return
    await savePreviewEdit(editor.getValue())
  }

  /** 有未保存修改时需要第二次点击才关闭（放弃修改）。 */
  function tryClose() {
    const cur = S.preview
    if (cur?.dirty && !cur.armed) {
      cur.armed = true
      toast.info('有未保存的修改：再点一次关闭将放弃修改（或先点「保存」）')
      setTimeout(() => { if (S.preview) S.preview.armed = false }, 3000)
      return
    }
    closePreview()
  }

  onMounted(() => {
    syncModel()
    window.addEventListener('keydown', onKey)
    window.addEventListener('resize', onViewportResize)
  })
  // 切换预览目标时重置图片视图状态；syncModel 负责文本装载。
  watch(() => S.preview?.name, () => {
    zoom.value = 1
    rot.value = 0
    natW.value = 0
    natH.value = 0
    dims.value = ''
    imgFailed.value = false
    syncModel()
  })
  // 界面字号变化时同步编辑器。
  watch(() => S.fontSize, (v) => { editor?.updateOptions({ fontSize: v }) })
  onUnmounted(() => {
    editor?.dispose()
    editor = null
    window.__previewEditor = null
    window.removeEventListener('keydown', onKey)
    window.removeEventListener('resize', onViewportResize)
  })
</script>
