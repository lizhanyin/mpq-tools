<script setup>
import { computed } from 'vue'
import { S } from '../store'
import { fmtBytes } from '../utils'

const parts = computed(() => {
  if (!S.info) return null
  const i = S.info
  const out = [
    { text: `MPQ v${i.version}`, strong: true },
    { text: `扇区 ${fmtBytes(i.sector_size)} B` },
    { text: `hash ${fmtBytes(i.hash_table_size)} / block ${fmtBytes(i.block_table_size)}` },
  ]
  if (i.het_bet) out.push({ text: '+ HET/BET', strong: true })
  out.push(
    { text: `listfile ${fmtBytes(i.listfile_count)} 项` },
    { text: `警告 ${fmtBytes(i.warnings.length)}` },
  )
  return out
})
</script>

<template>
  <div class="statusbar">
    <template v-if="parts">
      <template v-for="(p, idx) in parts" :key="idx">
        <span v-if="idx > 0" class="sep">|</span>
        <span :class="p.strong ? 'strong' : ''">{{ p.text }}</span>
      </template>
    </template>
    <span v-else>打开一个 .mpq / .w3x / .sc2map 文件开始</span>
  </div>
</template>
