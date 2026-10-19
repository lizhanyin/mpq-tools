<template>
  <div ref="box" class="fwin-mdx" :class="{ dragging: !!mode }"
       title="左键拖动旋转，滚轮缩放，右键拖动平移；←→ 切换模型，↑↓ 切换动作">
    <canvas ref="cv" class="mdx-canvas"
            @pointerdown="onDown" @pointermove="onMove" @pointerup="onUp"
            @pointercancel="onUp" @wheel.prevent="onWheel"
            @contextmenu.prevent></canvas>
    <!-- 右上角坐标轴指示器（随相机旋转；X 红 / Y 绿 / Z 蓝-朝上） -->
    <canvas ref="axesCv" class="mdx-axes" width="72" height="72"></canvas>
    <!-- 模型使用的贴图（TEXS 引用）：可展开/收缩，标注来源 -->
    <div v-if="state === 'ready' && textures.length" class="mdx-texpanel">
      <button type="button" class="tex-head" @click="toggleTexPanel"
              :title="texPanelOpen ? '收起贴图列表' : '展开贴图列表'">
        <span class="tex-caret">{{ texPanelOpen ? '▾' : '▸' }}</span>
        贴图（{{ textures.length }}）
        <span v-if="missingCount" class="tex-miss">缺失 {{ missingCount }}</span>
      </button>
      <div v-show="texPanelOpen" class="tex-list">
        <div v-for="t in textures" :key="t.path" class="tex-item" :title="t.path">
          <img v-if="t.thumb" class="tex-thumb" :src="t.thumb" alt="" />
          <span v-else class="tex-thumb tex-placeholder" :class="{ loading: t.thumb === undefined }"></span>
          <span class="tex-path">{{ t.path }}</span>
          <span class="tex-src" :class="t.source">{{ srcLabel(t.source) }}</span>
        </div>
      </div>
    </div>
    <!-- 动作序列 + 播放速度：模型就绪后浮在画布左上角 -->
    <div v-if="state === 'ready'" class="mdx-toolbar">
      <n-select v-if="sequences.length" :value="seqId" :options="sequences"
                size="tiny" :consistent-menu-width="false" placeholder="动作"
                title="切换动作 / 动画序列（↑↓）"
                @update:value="onSequence"></n-select>
      <n-select :value="speed" :options="SPEEDS" size="tiny"
                class="mdx-speed" title="播放速度"
                @update:value="(v: number) => speed = v"></n-select>
    </div>
    <div v-if="state === 'loading'" class="mdx-hint">加载模型中…</div>
    <div v-else-if="state === 'error'" class="mdx-hint mdx-error">{{ errMsg }}</div>
  </div>
</template>

<script setup lang="ts">
  /** MDX（Warcraft III 模型）预览：mdx-m3-viewer（WebGL2）渲染，按需
   *  懒加载。贴图经 pathSolver 从归档内按名取字节（大小写不敏感），找
   *  不到时回退无贴图渲染。轨道相机参考官方示例（简化：无触屏）。
   *  viewer 内部 API 无公开类型出口，这里以 any 触达。 */
  import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
  import { NSelect } from 'naive-ui'
  import { S, rows, previewFile } from '../store'
  import type { Row } from '../store'
  import type { EntryDto } from '../types'
  import { invoke } from '../ipc'

  const props = defineProps<{
    /** 模型文件的 base64 字节。 */
    data: string
    /** 归档内条目名。 */
    name?: string
  }>()

  const box = ref<HTMLDivElement | null>(null)
  const cv = ref<HTMLCanvasElement | null>(null)
  const axesCv = ref<HTMLCanvasElement | null>(null)
  const state = ref<'loading' | 'ready' | 'error'>('loading')
  const errMsg = ref('')
  const mode = ref<'' | 'rotate' | 'pan'>('')

  // 动作 / 动画序列：模型加载完成后填充；MDL 定长字符串常带尾部空格。
  const sequences = ref<{ label: string; value: number }[]>([])
  const seqId = ref<number | null>(null)
  function onSequence(v: number) {
    seqId.value = v
    instance?.setSequence(v)
  }

  // 播放速度：rAF 的 dt 按倍率缩放后交给 viewer，动画 / 粒子整体变速。
  const SPEEDS = [0.25, 0.5, 1, 2, 4].map((v) => ({
    value: v, label: `${v}×`,
  }))
  const speed = ref(1)

  // eslint 无 @typescript-eslint 配置，viewer 内部 API 以 any 触达。
  let viewer: any = null
  let scene: any = null
  let instance: any = null
  let model: any = null
  let modelSeq = 0 // 快速切换时只认最后一次加载
  let raf = 0
  let ro: ResizeObserver | null = null

  // -- 轨道相机状态（Z 轴朝上，与 WC3 一致）--------------------------------------
  const FOV = Math.PI / 4
  const cam = {
    target: [0, 0, 0] as [number, number, number],
    distance: 400,
    h: Math.PI / 2, // 水平角
    v: Math.PI / 4, // 俯仰角
  }

  function applyCamera() {
    if (!scene) return
    const sv = Math.sin(cam.v), cv2 = Math.cos(cam.v)
    const sh = Math.sin(cam.h), ch = Math.cos(cam.h)
    const pos: [number, number, number] = [
      cam.target[0] + cam.distance * sv * sh,
      cam.target[1] - cam.distance * sv * ch,
      cam.target[2] + cam.distance * cv2,
    ]
    scene.camera.moveToAndFace(pos, cam.target, [0, 0, 1])
    drawAxes()
  }

  function resize() {
    const el = cv.value
    if (!el || !scene) return
    const w = Math.max(el.clientWidth, 1)
    const h = Math.max(el.clientHeight, 1)
    el.width = w
    el.height = h
    scene.viewport[2] = w
    scene.viewport[3] = h
    scene.camera.perspective(FOV, w / h, 1, 200000)
    applyCamera()
  }

  /** 右上角坐标轴指示器：把世界轴投影到相机屏幕面画三条线。 */
  function drawAxes() {
    const c = axesCv.value
    if (!c) return
    const g = c.getContext('2d')
    if (!g) return
    const sv = Math.sin(cam.v), chv = Math.cos(cam.h)
    const sh = Math.sin(cam.h), ch = Math.cos(cam.h)
    // 相机基：right = (cos h, sin h, 0)；forward = 视线方向；up = right×forward
    const f: [number, number, number] = [-sv * sh, sv * chv, -Math.cos(cam.v)]
    const r: [number, number, number] = [ch, sh, 0]
    const cross = (a: number[], b: number[]): [number, number, number] => [
      a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0],
    ]
    const dot = (a: number[], b: number[]) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    const u = cross(r, f)
    const cx = c.width / 2, cy = c.height / 2, len = c.width / 2 - 10
    g.clearRect(0, 0, c.width, c.height)
    g.font = '9px Consolas, monospace'
    g.textAlign = 'center'
    g.textBaseline = 'middle'
    const lines: [number[], string, string][] = [
      [[1, 0, 0], 'X', '#ff7a76'],
      [[0, 1, 0], 'Y', '#7bd88f'],
      [[0, 0, 1], 'Z', '#6ba1ff'],
    ]
    for (const [axis, label, color] of lines) {
      const x = dot(axis, r), y = -dot(axis, u)
      const ex = cx + x * len, ey = cy + y * len
      g.strokeStyle = color
      g.lineWidth = 1.5
      g.beginPath(); g.moveTo(cx, cy); g.lineTo(ex, ey); g.stroke()
      g.fillStyle = color
      g.fillText(label, cx + x * (len + 6), cy + y * (len + 6))
    }
  }

  // -- 指针交互：左键旋转 / 右键平移 / 滚轮缩放 ----------------------------------

  let drag: { mode: 'rotate' | 'pan'; x: number; y: number; id: number } | null = null
  function onDown(ev: PointerEvent) {
    drag = { mode: ev.button === 2 ? 'pan' : 'rotate', x: ev.clientX, y: ev.clientY, id: ev.pointerId }
    mode.value = drag.mode
    try { (ev.currentTarget as HTMLElement).setPointerCapture(ev.pointerId) } catch { /* 合成事件 */ }
  }
  function onMove(ev: PointerEvent) {
    if (!drag || ev.pointerId !== drag.id) return
    const dx = ev.clientX - drag.x
    const dy = ev.clientY - drag.y
    drag.x = ev.clientX
    drag.y = ev.clientY
    if (drag.mode === 'rotate') {
      const spd = Math.PI / 180 // 每像素 1°
      cam.h -= dx * spd
      cam.v = Math.min(Math.max(0.05, cam.v - dy * spd), Math.PI - 0.05)
    } else {
      // 视线在 XY 平面上的平移基向量（随水平角旋转）
      const el = cv.value
      const s = (cam.distance / Math.max(el?.clientHeight || 1, 1)) * 1.2
      const ch = Math.cos(cam.h), sh = Math.sin(cam.h)
      cam.target[0] -= (dx * ch - dy * sh) * s
      cam.target[1] -= (dx * sh + dy * ch) * s
    }
    applyCamera()
  }
  function onUp(ev: PointerEvent) {
    if (drag && ev.pointerId !== drag.id) return
    drag = null
    mode.value = ''
  }
  function onWheel(ev: WheelEvent) {
    cam.distance = Math.max(1, cam.distance * (1 + (ev.deltaY > 0 ? 1 : -1) * 0.12))
    applyCamera()
  }

  // -- 方向键：←→ 切换模型文件，↑↓ 切换动作序列 ----------------------------------

  /** 当前视图里的 MDX 条目（与图片 ←→ 切换同一列表口径；
   *  未命名条目按识别出的 .mdx 后缀一并纳入）。 */
  type MdxRow = { type: 'file'; entry: EntryDto; showFull: boolean }
  function mdxEntries(): EntryDto[] {
    return rows.value.list
      .filter((r): r is MdxRow => r.type === 'file' && /\.mdx$/i.test(r.entry.display))
      .map((r) => r.entry)
  }

  function onKey(ev: KeyboardEvent) {
    if (!S.preview?.model) return
    if (S.focusZone === 'list') return // 焦点在列表：方向键归表格/网格
    if (S.modify || S.create || S.convert || S.pw || S.repair || S.newFolder || S.settingsOpen) return
    const t = ev.target as HTMLElement | null
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return
    if (ev.key === 'ArrowLeft' || ev.key === 'ArrowRight') {
      const list = mdxEntries()
      const i = list.findIndex((e) => e.display === S.preview?.name)
      if (i < 0 || list.length < 2) return
      const next = ev.key === 'ArrowRight' ? Math.min(i + 1, list.length - 1) : Math.max(i - 1, 0)
      if (next !== i) { ev.preventDefault(); previewFile(list[next]) }
    } else if (ev.key === 'ArrowUp' || ev.key === 'ArrowDown') {
      const n = sequences.value.length
      if (!n || seqId.value == null) return
      ev.preventDefault()
      const next = ev.key === 'ArrowDown'
        ? (seqId.value + 1) % n
        : (seqId.value - 1 + n) % n
      onSequence(next)
    }
  }

  // -- 贴图按需加载 ----------------------------------------------------------------

  const texCache = new Map<string, ArrayBuffer>() // 路径（小写）→ 字节（会话级复用）
  /** 解析贴图：统一走后端 —— 先查当前归档，未命中回退老客户端
   *  MPQ 链（S.war3Dir，hash 大小写不敏感，原始拼写直接可用）。 */
  async function resolveTexture(path: string): Promise<string | ArrayBuffer> {
    const key = path.toLowerCase().replace(/\//g, '\\')
    const hit = texCache.get(key)
    if (hit) return hit
    try {
      const b64 = await invoke<string | null>('read_file_bytes', {
        name: path, war3dir: S.war3Dir || undefined,
      })
      if (!b64) return path // 归档与客户端都没有 → 回退无贴图渲染
      const buf = base64ToBytes(b64)
      if (texCache.size > 40) texCache.clear() // 简单限量，防超大会话膨胀
      texCache.set(key, buf)
      return buf
    } catch {
      return path
    }
  }

  // -- 模型使用的贴图（TEXS 引用；展开/收缩面板） -------------------------------

  interface TexItem { path: string; source: string; thumb?: string | null }
  const textures = ref<TexItem[]>([])
  const texPanelOpen = ref(false)
  const missingCount = computed(() => textures.value.filter((t) => t.source === 'missing').length)

  function srcLabel(s: string): string {
    return s === 'map' ? '归档' : s === 'war3' ? '客户端' : '缺失'
  }

  function toggleTexPanel() {
    texPanelOpen.value = !texPanelOpen.value
    if (texPanelOpen.value) loadThumbs()
  }

  /** 模型就绪后向后端要 TEXS 贴图清单（含来源判定）。 */
  async function loadTextures() {
    textures.value = []
    const key = S.preview?.key
    if (!key) return
    try {
      const list = await invoke<{ path: string; source: string }[]>('model_textures', {
        key, war3dir: S.war3Dir || undefined,
      })
      textures.value = list.map((t) => ({ ...t, thumb: undefined }))
    } catch {
      textures.value = []
    }
  }

  /** 展开时逐个取缩略图（后端解码 BLP/TGA，地图内优先、客户端回退）。 */
  async function loadThumbs() {
    for (const t of textures.value) {
      if (t.thumb !== undefined) continue
      try {
        t.thumb = await invoke<string | null>('texture_thumbnail', {
          path: t.path, war3dir: S.war3Dir || undefined,
        })
      } catch {
        t.thumb = null
      }
    }
  }

  function base64ToBytes(b64: string): ArrayBuffer {
    const bin = atob(b64)
    const u8 = new Uint8Array(bin.length)
    for (let i = 0; i < bin.length; i++) u8[i] = bin.charCodeAt(i)
    return u8.buffer
  }

  // -- 模型加载 --------------------------------------------------------------------

  async function loadModel(dataB64: string) {
    const seq = ++modelSeq
    state.value = 'loading'
    errMsg.value = ''
    sequences.value = []
    seqId.value = null
    textures.value = [] // 旧模型的贴图清单失效
    try {
      if (!viewer) await ensureViewer()
      if (seq !== modelSeq) return
      const buffer = base64ToBytes(dataB64)
      // 模型本体直接以 ArrayBuffer 加载；pathSolver 靠身份识别放行本体，
      // 其余（贴图路径）从归档解析。
      const solver = (s: unknown) => (s === buffer ? s : resolveTexture(String(s)))
      const loaded = await viewer.load(buffer, solver)
      if (seq !== modelSeq) {
        if (loaded) viewer.unload(loaded)
        return
      }
      if (!loaded) throw new Error('模型解析失败（不是有效的 MDX 文件）')
      // 卸掉旧实例与旧模型（切换预览目标时）
      if (instance) { instance.detach(); instance = null }
      if (model && model !== loaded) viewer.unload(model)
      model = loaded
      instance = model.addInstance()
      instance.setScene(scene)
      sequences.value = (model.sequences ?? []).map((sq: any, i: number) => ({
        value: i,
        label: String(sq?.name ?? '').replace(/\s+$/, '') || `动作 ${i + 1}`,
      }))
      seqId.value = sequences.value.length ? 0 : null
      if (sequences.value.length) {
        instance.setSequence(0)
        instance.setSequenceLoopMode(2) // 无限循环
      }
      frameModel(model)
      state.value = 'ready'
      if (seq === modelSeq) loadTextures() // 就绪后拉取 TEXS 贴图清单
    } catch (e) {
      if (seq !== modelSeq) return
      state.value = 'error'
      errMsg.value = `模型加载失败：${e}`
    }
  }

  /** 按包围球取景：距离按竖直/水平 FOV 中较小者计算（窄窗口不裁边），
   *  留 15% 边距；目标点移到包围盒中心。 */
  function frameModel(m: any) {
    const ext = m.extent
    let radius = 0
    if (ext) {
      const [mn, mx] = [ext.min, ext.max]
      if (mn && mx) {
        cam.target = [(mn[0] + mx[0]) / 2, (mn[1] + mx[1]) / 2, (mn[2] + mx[2]) / 2]
        radius = Math.max(
          ext.boundsRadius || 0,
          Math.hypot(mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]) / 2,
        )
      }
    }
    if (radius > 0) {
      const el = cv.value
      const aspect = Math.max((el?.clientWidth || 1) / (el?.clientHeight || 1), 0.1)
      const fovH = 2 * Math.atan(Math.tan(FOV / 2) * aspect)
      const fit = Math.min(FOV, fovH)
      cam.distance = Math.min(60000, Math.max(60, (radius / Math.sin(fit / 2)) * 1.15))
    }
    cam.v = Math.PI / 4
    cam.h = Math.PI / 2
    applyCamera()
  }

  async function ensureViewer() {
    // CJS 包深路径导入：顶层入口会连带 utils→jass2→fengari（Lua VM，
    // 仅地图事件脚本需要，还带 Node 内建依赖）；只取 viewer 与贴图/模型
    // handler，包体更小也无需 process/os 兜底。
    const [mod, hMdx, hBlp, hDds, hTga] = await Promise.all([
      import('mdx-m3-viewer/dist/cjs/viewer/viewer.js'),
      import('mdx-m3-viewer/dist/cjs/viewer/handlers/mdx/handler.js'),
      import('mdx-m3-viewer/dist/cjs/viewer/handlers/blp/handler.js'),
      import('mdx-m3-viewer/dist/cjs/viewer/handlers/dds/handler.js'),
      import('mdx-m3-viewer/dist/cjs/viewer/handlers/tga/handler.js'),
    ] as any[])
    const Viewer = mod.default?.default ?? mod.default
    const pick = (m: any) => m.default?.default ?? m.default
    // alpha 上下文 + 清屏改透明：透出容器的深色渐变背景。
    viewer = new Viewer(cv.value, { alpha: true, antialias: true })
    viewer.startFrame = function (this: any) {
      const gl = this.gl
      gl.depthMask(true)
      gl.clearColor(0, 0, 0, 0)
      gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT)
    }
    scene = viewer.addScene()
    scene.alpha = true // 场景不清屏（默认会以不透明背景色盖掉透明画布）
    viewer.addHandler(pick(hMdx), (s: unknown) => (typeof s === 'string' ? resolveTexture(s) : s))
    viewer.addHandler(pick(hBlp))
    viewer.addHandler(pick(hDds))
    viewer.addHandler(pick(hTga))
    ro = new ResizeObserver(resize)
    if (box.value) ro.observe(box.value)
    resize()
    // rAF 的 dt 乘上速度倍率再交给 viewer：动画、粒子、事件对象整体变速。
    let lastT = performance.now()
    const step = (now: number) => {
      raf = requestAnimationFrame(step)
      const dt = Math.min(now - lastT, 100) * speed.value
      lastT = now
      viewer.updateAndRender(dt)
    }
    step(performance.now())
  }

  onMounted(() => {
    loadModel(props.data)
    window.addEventListener('keydown', onKey)
  })
  watch(() => props.data, (d) => { if (d) loadModel(d) })
  onUnmounted(() => {
    modelSeq++
    cancelAnimationFrame(raf)
    ro?.disconnect()
    window.removeEventListener('keydown', onKey)
    instance?.detach()
    if (model && viewer) viewer.unload(model)
    viewer?.clear()
    viewer = scene = model = instance = null
  })
</script>

<style scoped>
  .fwin-mdx {
    position: absolute;
    inset: 0;
    overflow: hidden;
    /* 透明 WebGL 画布透出的纯黑背景（贴图 / 模型观察对比度最好） */
    background: #000;
    cursor: grab;
  }
  .fwin-mdx.dragging { cursor: grabbing; }
  .mdx-canvas { display: block; width: 100%; height: 100%; }
  .mdx-axes {
    position: absolute;
    top: 6px;
    right: 8px;
    pointer-events: none;
  }
  .mdx-toolbar {
    position: absolute;
    top: 8px;
    left: 8px;
    display: flex;
    gap: 6px;
    width: 176px;
    z-index: 2;
  }
  .mdx-toolbar .n-select { flex: 1; min-width: 0; }
  .mdx-toolbar .mdx-speed { flex: 0 0 64px; }
  .mdx-hint {
    position: absolute;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    padding: 10px 22px;
    border-radius: 8px;
    background: rgba(0, 0, 0, 0.55);
    color: #dfe7ef;
    font-size: 13px;
    pointer-events: none;
  }
  .mdx-error { color: #ffb4a8; }

  /* 模型贴图面板：左下角叠加，可展开/收缩 */
  .mdx-texpanel {
    position: absolute;
    left: 8px;
    bottom: 8px;
    z-index: 2;
    max-width: min(420px, calc(100% - 20px));
    border-radius: 8px;
    background: rgba(0, 0, 0, 0.65);
    backdrop-filter: blur(4px);
    color: #cfd8e3;
    font-size: 12px;
  }
  .tex-head {
    display: flex;
    align-items: center;
    gap: 5px;
    width: 100%;
    padding: 5px 10px;
    border: none;
    background: none;
    color: inherit;
    font: inherit;
    cursor: pointer;
    white-space: nowrap;
  }
  .tex-head:hover { color: #fff; }
  .tex-caret { width: 12px; text-align: center; }
  .tex-miss { margin-left: auto; color: #ffb4a8; font-size: 11px; }
  .tex-list {
    max-height: 180px;
    overflow-y: auto;
    padding: 2px 0 6px;
  }
  .tex-item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 3px 10px;
  }
  .tex-item:hover { background: rgba(255, 255, 255, 0.06); }
  .tex-thumb {
    flex: none;
    width: 32px;
    height: 32px;
    border-radius: 4px;
    object-fit: contain;
    background: #1a1a1a;
  }
  .tex-placeholder.loading { background: #262d36; }
  .tex-path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--mono, monospace);
  }
  .tex-src {
    flex: none;
    padding: 1px 6px;
    border-radius: 8px;
    font-size: 10px;
    line-height: 16px;
    background: #2a3a4a;
  }
  .tex-src.war3 { background: #31404f; color: #9fd0ff; }
  .tex-src.missing { background: #4a2a2a; color: #ffb4a8; }

</style>
