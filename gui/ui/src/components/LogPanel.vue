<template>
  <div class="log-panel">
    <div class="log-header">
      <strong>日志</strong>
      <n-button size="tiny" @click="clearLog">清空</n-button>
    </div>
    <div ref="el" class="log-body">{{ text() }}</div>
  </div>
</template>

<script setup lang="ts">
  import { ref, watch, nextTick } from 'vue'
  import { NButton } from 'naive-ui'
  import { S, clearLog } from '../store'

  const el = ref<HTMLElement | null>(null)

  watch(() => S.logs.length, async () => {
    await nextTick()
    if (el.value) el.value.scrollTop = el.value.scrollHeight
  })

  const text = () => S.logs.join('\n') + (S.logs.length ? '\n' : '')
</script>
