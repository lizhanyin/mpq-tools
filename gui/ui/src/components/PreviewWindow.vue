<script setup>
import { ref } from 'vue'
import { NButton } from 'naive-ui'
import { S, closePreview } from '../store'

// 非模态浮动窗口；标题栏拖动。初始位置与原版一致。
const x = ref(90)
const y = ref(70)
const titleEl = ref(null)

function startDrag(ev) {
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
</script>

<template>
  <div class="fwindow" :style="{ left: x + 'px', top: y + 'px' }">
    <div ref="titleEl" class="fwin-title" @pointerdown="startDrag">
      <span>预览</span>
      <span class="spacer"></span>
      <n-button size="tiny" quaternary title="关闭" @click="closePreview">✕</n-button>
    </div>
    <div class="fwin-body">
      <strong>{{ S.preview.name }}</strong>
      <pre>{{ S.preview.text }}</pre>
    </div>
  </div>
</template>
