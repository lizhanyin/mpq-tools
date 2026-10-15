<template>
  <div class="fwindow fwindow-editor"
       :style="{ left: x + 'px', top: y + 'px', width: w + 'px', height: h + 'px' }">
    <div ref="titleEl" class="fwin-title" @pointerdown="startDrag">
      <span>预览</span>
      <span v-if="p?.dirty" class="dirty-dot" title="有未保存的修改">●</span>
      <span class="spacer"></span>
      <n-button size="tiny" type="primary" :disabled="!p?.editable"
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
      <div v-if="loading" class="editor-loading">正在加载编辑器…</div>
      <div ref="editorHost" class="editor-host"></div>
    </div>
    <div class="fwin-resize" @pointerdown="startResize"></div>
  </div>
</template>

<script setup>
  import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
  import { NButton } from 'naive-ui'
  import { S, closePreview, savePreviewEdit } from '../store'
  import { loadMonaco, languageFor } from '../monaco'
  import { fmtBytes } from '../utils'
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

  const headerNote = computed(() => {
    const bits = []
    if (!p.value) return ''
    if (p.value.encoding !== 'UTF-8') bits.push(`按 ${p.value.encoding} 解码`)
    if (p.value.truncated) bits.push(`仅加载前 256 KiB / 共 ${fmtBytes(p.value.size)} 字节，只读`)
    else if (p.value.binary) bits.push('二进制内容，只读')
    else bits.push(`${fmtBytes(p.value.size)} 字节`)
    return bits.join('，')
  })

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

  onMounted(syncModel)
  watch(() => S.preview?.name, syncModel)
  // 界面字号变化时同步编辑器。
  watch(() => S.fontSize, (v) => { editor?.updateOptions({ fontSize: v }) })
  onUnmounted(() => {
    editor?.dispose()
    editor = null
    window.__previewEditor = null
  })
</script>
