<script setup>
import { NButton } from 'naive-ui'
import { openArchiveDialog } from '../store'
</script>

<template>
  <div class="welcome">
    <h1>MPQ 归档浏览器</h1>
    <n-button type="primary" @click="openArchiveDialog">打开归档…</n-button>
    <p class="weak">或直接把 .mpq / .w3x / .sc2map 文件拖进窗口</p>
  </div>
</template>
