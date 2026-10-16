<template>
  <div class="fwindow fwindow-editor"
       :style="{ left: x + 'px', top: y + 'px', width: w + 'px', height: h + 'px' }">
    <div ref="titleEl" class="fwin-title" @pointerdown="startDrag">
      <span>预览</span>
      <span v-if="p?.dirty" class="dirty-dot" title="有未保存的修改">●</span>
      <span class="spacer"></span>
      <n-button v-if="!p?.image" size="tiny" type="primary" :disabled="!p?.editable"
                :loading="p?.saving"
                title="把编辑内容排队到「修改归档」，随后在弹窗中确认写入（Ctrl+S）"
                @click="save">保存</n-button>
      <n-button size="tiny" quaternary title="关闭" @click="tryClose">✕</n-button>
    </div>
    <div class="fwin-head">
      <strong class="mono">{{ p?.name }}</strong>
      <span class="weak">{{ headerNote }}</span>
    </div>
    <div class="fwin-editor">
      <div v-if="loading && !p?.image" class="editor-loading">正在加载编辑器…</div>
      <div v-show="!p?.image" ref="editorHost" class="editor-host"></div>
      <div v-if="p?.image" class="fwin-img" :class="{ actual: !fit }"
           :title="fit ? '点击按原始像素大小查看' : '点击恢复适应窗口'"
           @click="fit = !fit">
        <img v-if="!imgFailed" :src="p.image.url" alt="" draggable="false"
             @load="onImgLoad" @error="imgFailed = true">
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
    <div class="fwin-resize" @pointerdown="startResize"></div>
  </div>
</template>

<script setup>
  import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
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
  const titleEl = ref(null)
  const editorHost = ref(null)
  const loading = ref(true)
  let editor = null
  let monaco = null
  let applying = false // 程序化 setValue 不计入 dirty

  const p = computed(() => S.preview)
  const fit = ref(true)      // 图片：适应窗口 / 原始像素大小
  const dims = ref('')       // 图片加载后的自然尺寸（如 512×512）
  const imgFailed = ref(false)

  const headerNote = computed(() => {
    const bits = []
    if (!p.value) return ''
    if (p.value.image) {
      if (imgCount.value > 1 && imgIndex.value >= 0) {
        bits.push(`第 ${imgIndex.value + 1}/${imgCount.value} 张`)
      }
      if (dims.value) bits.push(dims.value)
      bits.push(fmtBytes(p.value.size))
      if (p.value.image.converted) bits.push('已转换为 PNG')
      return bits.join('，')
    }
    if (p.value.encoding !== 'UTF-8') bits.push(`按 ${p.value.encoding} 解码`)
    if (p.value.truncated) bits.push(`仅加载前 256 KiB / 共 ${fmtBytes(p.value.size)} 字节，只读`)
    else if (p.value.binary) bits.push('二进制内容，只读')
    else bits.push(`${fmtBytes(p.value.size)} 字节`)
    return bits.join('，')
  })

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
    dims.value = `${ev.target.naturalWidth}×${ev.target.naturalHeight}`
  }

  // -- 拖动 / 调整大小 ---------------------------------------------------------

  function startDrag(ev) {
    // 点击按钮等交互元素时不启动拖动：setPointerCapture 会吞掉
    // 后续指针事件，导致按钮收不到 click。
    if (ev.button !== 0 || ev.target.closest('button')) return
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
    if (ev.button !== 0) return
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
  })
  // 切换预览目标时重置图片视图状态；syncModel 负责文本装载。
  watch(() => S.preview?.name, () => {
    fit.value = true
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
  })
</script>
