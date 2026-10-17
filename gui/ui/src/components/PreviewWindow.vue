<template>
  <div class="fwindow fwindow-editor" :class="{ maximized }"
       :style="{ left: x + 'px', top: y + 'px', width: w + 'px', height: h + 'px' }"
       @pointerdown.capture="S.focusZone = 'preview'">
    <div ref="titleEl" class="fwin-title" @pointerdown="startDrag">
      <!-- 图片预览：标题栏只留文件名；详细信息在窗口下方的悬浮条（👁 开关） -->
      <template v-if="p?.image">
        <strong class="mono title-name" :title="p?.name">{{ p?.name }}</strong>
      </template>
      <span v-else>预览</span>
      <span v-if="p?.dirty" class="dirty-dot" title="有未保存的修改">●</span>
      <span class="spacer"></span>
      <n-button v-if="p?.image" size="tiny" quaternary :class="{ off: !infoBar }"
                :title="infoBar ? '隐藏图片信息浮条' : '显示图片信息浮条'"
                @click="toggleInfoBar">👁</n-button>
      <n-button v-if="p?.image" size="tiny" quaternary title="逆时针旋转 90°" @click="rotate(-1)">↺</n-button>
      <n-button v-if="p?.image" size="tiny" quaternary title="顺时针旋转 90°" @click="rotate(1)">↻</n-button>
      <n-button v-if="!p?.image" size="tiny" type="primary" :disabled="!p?.editable"
                :loading="p?.saving"
                title="把编辑内容排队到「修改归档」，随后在弹窗中确认写入（Ctrl+S）"
                @click="save">保存</n-button>
      <n-button size="tiny" quaternary :title="maximized ? '还原' : '最大化'"
                @click="toggleMaximize">⛶</n-button>
      <n-button size="tiny" quaternary title="缩为最小尺寸并贴到右下角（再点还原）"
                @click="toggleMiniCorner">◲</n-button>
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
          <img :src="shownUrl" alt="" draggable="false" :style="imgStyle"
               @load="onImgLoad" @error="imgFailed = true">
        </div>
        <div v-else class="img-fallback">图片解码失败：文件可能不是有效的图片</div>
      </div>
      <!-- 左右切换：视图中还有其它图片时显示（含搜索结果视图）。
           点按翻一张；按住约 0.4s 后连翻，锁定 60 张/秒。 -->
      <template v-if="p?.image && imgCount > 1">
        <button class="img-nav prev" :disabled="imgIndex <= 0"
                title="上一张（←）· 按住连翻"
                @pointerdown.prevent="pressNav(-1, $event)"
                @pointerup="stopFlip" @pointercancel="stopFlip" @lostpointercapture="stopFlip"
                @click="ev => ev.detail === 0 && switchImage(-1)">‹</button>
        <button class="img-nav next" :disabled="imgIndex < 0 || imgIndex >= imgCount - 1"
                title="下一张（→）· 按住连翻"
                @pointerdown.prevent="pressNav(1, $event)"
                @pointerup="stopFlip" @pointercancel="stopFlip" @lostpointercapture="stopFlip"
                @click="ev => ev.detail === 0 && switchImage(1)">›</button>
      </template>
      <!-- 图片信息浮条：悬浮在编辑区底部内侧，不挡图片交互（仅缩放锁可点） -->
      <div v-if="p?.image && infoBar" class="fwin-info">
        <span v-if="imgCount > 1 && imgIndex >= 0">第 {{ imgIndex + 1 }}/{{ imgCount }} 张</span>
        <span v-if="dims">{{ dims }} px</span>
        <span>{{ fmtSize(p.size) }}</span>
        <span>{{ Math.round(zoom * 100) }}%</span>
        <button class="lock-btn" :class="{ on: zoomLocked }"
                :title="zoomLocked ? '缩放已锁定，点击解锁（切换图片回到完整显示）'
                                   : '锁定缩放：切换图片时保持当前比例'"
                @click="toggleZoomLock">{{ zoomLocked ? '🔒' : '🔓' }}</button>
        <span v-if="p.image.converted">已转换为 PNG</span>
      </div>
    </div>
    <div v-if="!maximized" class="fwin-resize" @pointerdown="startResize"></div>
  </div>
</template>

<script setup>
  import { ref, computed, onMounted, onUnmounted, watch, nextTick } from 'vue'
  import { NButton } from 'naive-ui'
  import { S, rows, previewFile, closePreview, savePreviewEdit, setSelectedKeys } from '../store'
  import { loadMonaco, languageFor } from '../monaco'
  import { fmtBytes, fmtSize, isImageName } from '../utils'
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
  const shownUrl = ref('') // 当前显示的图片 URL；先屏外解码再替换，切换时不闪白
  let imgSeq = 0           // 快速连续切换时只认最后一次的结果
  const imgBox = ref(null)   // 图片滚动容器：平移 = 拖动它的滚动
  const panning = ref(false)
  let suppressClick = false  // 拖动平移后不再当作点击缩放

  // 图片信息浮条（窗口下方）：👁 开关；缩放锁：切换图片时保持比例。
  // 都随 localStorage 持久化，默认浮条开、锁关。
  const infoBar = ref(localStorage.getItem('img_info_bar') !== '0')
  const zoomLocked = ref(localStorage.getItem('img_zoom_lock') === '1')
  function toggleInfoBar() {
    infoBar.value = !infoBar.value
    localStorage.setItem('img_info_bar', infoBar.value ? '1' : '0')
  }
  function toggleZoomLock() {
    zoomLocked.value = !zoomLocked.value
    localStorage.setItem('img_zoom_lock', zoomLocked.value ? '1' : '0')
  }

  /** 文本预览的头部注释（图片信息在编辑区底部内侧的悬浮条）。 */
  const headerNote = computed(() => {
    const bits = []
    if (!p.value || p.value.image) return ''
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

  // -- 按住连翻（‹ › 按钮 / ←→ 方向键）--------------------------------------------
  // 短按只翻一张；按住约 0.4s 后开始连翻，锁定最多 60 张/秒（rAF 驱动）。
  // 每张都等上一张完成「提取 → 解码 → 上屏」才请求下一张：管线快时贴满
  // 60fps，管线慢时降到管线速度，不跳张、不闪白。

  const FLIP_DELAY_MS = 400    // 按住多久后进入连翻
  const FLIP_MIN_MS = 1000 / 60 // 连翻最快间隔：锁 60 帧
  let flipDir = 0              // 连翻方向；0 = 空闲
  let flipRaf = 0              // 连翻 rAF 句柄
  let flipHoldTimer = 0        // 起翻延时句柄
  let flipInFlight = false     // previewFile 请求在途
  let lastFlipReq = 0          // 上次发起请求的 rAF 时间戳（限速用）
  let imgReady = true          // 当前图已解码上屏（或加载失败）

  /** 尝试向 flipDir 翻一张；返回是否发起了请求。到达边界自动停。 */
  function flipStep() {
    const i = imgIndex.value + flipDir
    if (i < 0 || i >= imgEntries.value.length) { stopFlip(); return false }
    if (flipInFlight || !imgReady) return false
    const name = imgEntries.value[i].entry.name
    flipInFlight = true
    imgReady = false
    previewFile(name)
      .then(() => {
        // 预览没变成目标（提取失败等）：恢复节奏门并停翻，避免原地刷请求
        if (S.preview?.name !== name) { imgReady = true; stopFlip() }
      })
      .finally(() => { flipInFlight = false })
    return true
  }

  function flipTick(now) {
    if (!flipDir) return
    if (now - lastFlipReq >= FLIP_MIN_MS - 1 && flipStep()) lastFlipReq = now
    flipRaf = requestAnimationFrame(flipTick)
  }

  /** 按下立刻翻一张；一直按住则延时后进入 60fps 连翻。 */
  function startFlip(dir) {
    if (flipDir === dir) return
    stopFlip()
    flipDir = dir
    flipStep()
    flipHoldTimer = setTimeout(() => {
      lastFlipReq = 0
      flipRaf = requestAnimationFrame(flipTick)
    }, FLIP_DELAY_MS)
  }

  function stopFlip() {
    flipDir = 0
    clearTimeout(flipHoldTimer)
    cancelAnimationFrame(flipRaf)
  }

  /** 指针按到 ‹/› 上：捕获指针，按住期间即使移出按钮也能收到抬起。 */
  function pressNav(dir, ev) {
    try { ev.currentTarget.setPointerCapture(ev.pointerId) } catch { /* 合成事件没有真实指针 */ }
    startFlip(dir)
  }

  /** 图片预览时 ←/→ 切换，按住连翻；正在输入文本（过滤框等）时不抢按键。
   *  注意不能用 anyModalOpen——它包含预览窗自身，会让本处理器
   *  在预览打开时永远短路。 */
  function onKey(ev) {
    if (!S.preview?.image) return
    if (S.focusZone === 'list') return // 焦点在列表：方向键归网格，预览经选择联动跟随
    if (S.modify || S.create || S.convert || S.pw || S.repair || S.newFolder || S.settingsOpen) return
    const t = ev.target
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return
    if (ev.key === 'ArrowLeft' || ev.key === 'ArrowRight') {
      ev.preventDefault()
      // 系统按键重复不直接翻页：连翻节奏统一由按住的 rAF 循环控制
      if (!ev.repeat) startFlip(ev.key === 'ArrowLeft' ? -1 : 1)
    }
  }

  /** 松开方向键即停翻（按错方向重按也走这里的停→起）。 */
  function onKeyUp(ev) {
    if (ev.key === 'ArrowLeft' || ev.key === 'ArrowRight') stopFlip()
  }

  /** 屏外解码新图，成功后才一次性替换 src 与视图状态：旧图保持可见
   *  直到新位图可绘制，避免中间一帧露出白底。imgReady 供按住连翻掌握
   *  解码节奏。 */
  async function showImage(url) {
    const seq = ++imgSeq
    if (!url) { shownUrl.value = ''; imgReady = true; return }
    imgReady = false
    const im = new Image()
    im.src = url
    let ok = false
    try { await im.decode(); ok = im.naturalWidth > 0 } catch { ok = false }
    if (seq !== imgSeq) return // 解码期间又切了目标，丢弃本次结果
    if (!ok) {
      shownUrl.value = ''
      natW.value = 0
      natH.value = 0
      dims.value = ''
      imgFailed.value = true
      imgReady = true
      return
    }
    natW.value = im.naturalWidth
    natH.value = im.naturalHeight
    dims.value = `${natW.value}×${natH.value}`
    rot.value = 0
    imgFailed.value = false
    if (zoomLocked.value) {
      // 缩放锁定：保持比例，切换后把滚动居中到新图
      nextTick(() => {
        const el = imgBox.value
        if (!el) return
        el.scrollLeft = (el.scrollWidth - el.clientWidth) / 2
        el.scrollTop = (el.scrollHeight - el.clientHeight) / 2
      })
    } else {
      zoom.value = fitZoom()
    }
    shownUrl.value = url // 位图已解码，与上面的状态同帧上屏
    imgReady = true
  }

  function onImgLoad(ev) {
    imgFailed.value = false
    natW.value = ev.target.naturalWidth
    natH.value = ev.target.naturalHeight
    dims.value = `${natW.value}×${natH.value}`
    if (!zoomLocked.value) zoom.value = fitZoom() // 打开 / 切换图片时先完整显示（contain）
  }

  /** 预览→列表透传：预览切图后，把列表选择与网格光标/滚动定位到该行。
   *  与 store 的「选中→预览」联动名字相同即停，不会成环。 */
  function syncSelection() {
    const name = S.preview?.name
    if (!S.preview?.image || !name) return
    const idx = rows.value.list.findIndex((r) => r.type === 'file' && r.entry.name === name)
    if (idx < 0) return
    const key = rows.value.list[idx].entry.key
    if (S.selected.size !== 1 || !S.selected.has(key)) setSelectedKeys([key])
    S.gridCursor = idx
  }

  // -- 缩放 / 旋转 / 平移 ---------------------------------------------------------

  /** 缩放后考虑旋转的显示宽高。 */
  const rotW = (z) => Math.round((rot.value % 180 ? natH.value : natW.value) * z)
  const rotH = (z) => Math.round((rot.value % 180 ? natW.value : natH.value) * z)

  /** 完整显示（contain）所需缩放；小图放大封顶 200%，避免打开时过度放大。 */
  function fitZoom() {
    const el = imgBox.value
    if (!el || !natW.value) return 1
    return Math.min(2, el.clientWidth / rotW(1), el.clientHeight / rotH(1))
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

  /** 旋转 ±90°，保持缩放；原本是完整显示则按新方向重新 contain（锁定缩放时不重排）。 */
  function rotate(dir) {
    const wasFit = Math.abs(zoom.value - fitZoom()) < 1e-3
    rot.value = (rot.value + dir * 90 + 360) % 360
    if (!zoomLocked.value && wasFit) zoom.value = fitZoom()
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
    if (maximized.value) {
      w.value = window.innerWidth
      h.value = window.innerHeight
    } else if (miniMode.value) {
      // 迷你小窗贴住右下角
      x.value = Math.max(0, window.innerWidth - w.value)
      y.value = Math.max(0, window.innerHeight - h.value)
    }
  }

  /** 缩为最小高宽并贴到视口右下角；再点一次还原到进入前的位置尺寸。 */
  const miniMode = ref(false)
  let preMiniRect = null
  function toggleMiniCorner() {
    if (miniMode.value) {
      if (preMiniRect) {
        x.value = preMiniRect.x
        y.value = preMiniRect.y
        w.value = preMiniRect.w
        h.value = preMiniRect.h
      }
      miniMode.value = false
      return
    }
    // 从最大化进入时先回到最大化前的位置尺寸
    if (maximized.value) {
      if (savedRect) {
        x.value = savedRect.x
        y.value = savedRect.y
        w.value = savedRect.w
        h.value = savedRect.h
      }
      maximized.value = false
    }
    preMiniRect = { x: x.value, y: y.value, w: w.value, h: h.value }
    w.value = 320 // 与 startResize 下限、CSS min-width/min-height 一致
    h.value = 250
    x.value = Math.max(0, window.innerWidth - 320)
    y.value = Math.max(0, window.innerHeight - 250)
    miniMode.value = true
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
    window.addEventListener('keyup', onKeyUp)
    window.addEventListener('blur', stopFlip) // 失焦收不到 keyup，防止连翻失控
    window.addEventListener('resize', onViewportResize)
  })
  // 切换预览目标时重置视图状态；syncModel 负责文本装载。图片的视图
  // 状态不在此时清零——由 showImage 解码完成后统一重置，期间旧图保持
  // 显示，避免旧图消失、新图未解码之间闪白。
  watch(() => S.preview?.name, () => {
    if (!S.preview?.image) {
      zoom.value = 1
      rot.value = 0
      natW.value = 0
      natH.value = 0
      dims.value = ''
      imgFailed.value = false
    }
    syncModel()
  })
  // 打开 / 切换图片：先屏外解码，再一次性上屏。也盯住 name——两张
  // 内容相同的图片 data URL 相同，只看 url 不会触发重置。
  // 同时把列表选择/网格光标同步到该行（预览有焦点时的「事件透传」）。
  watch(() => [S.preview?.name, S.preview?.image?.url], ([, url]) => {
    showImage(url)
    syncSelection()
  }, { immediate: true })
  // 界面字号变化时同步编辑器。
  watch(() => S.fontSize, (v) => { editor?.updateOptions({ fontSize: v }) })
  onUnmounted(() => {
    stopFlip()
    editor?.dispose()
    editor = null
    window.__previewEditor = null
    window.removeEventListener('keydown', onKey)
    window.removeEventListener('keyup', onKeyUp)
    window.removeEventListener('blur', stopFlip)
    window.removeEventListener('resize', onViewportResize)
  })
</script>
