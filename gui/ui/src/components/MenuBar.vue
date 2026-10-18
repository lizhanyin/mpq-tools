<template>
  <div class="menubar">
    <n-dropdown trigger="click" placement="bottom-start"
                v-model:show="fileShow" :options="fileOptions" @select="onFileSelect">
      <n-button quaternary size="tiny">文件</n-button>
    </n-dropdown>
    <n-dropdown trigger="click" placement="bottom-start"
                v-model:show="opsShow" :options="opsOptions" @select="onOpsSelect">
      <n-button quaternary size="tiny">操作</n-button>
    </n-dropdown>
    <n-dropdown trigger="click" placement="bottom-start"
                v-model:show="toolsShow" :options="toolsOptions" @select="onToolsSelect">
      <n-button quaternary size="tiny">工具</n-button>
    </n-dropdown>
    <n-dropdown trigger="click" placement="bottom-start"
                v-model:show="settingsShow" :options="settingsOptions" @select="onSettingsSelect">
      <n-button quaternary size="tiny">设置</n-button>
    </n-dropdown>
    <div class="menu-path">{{ S.openPath || '未打开归档' }}</div>
  </div>
</template>

<script setup lang="ts">
  import { computed, ref } from 'vue'
  import { NDropdown, NButton } from 'naive-ui'
  import type { DropdownOption } from 'naive-ui'
  import {
    S, openArchiveDialog, extractAll, importFilesDialog, importFolderDialog, startNewFolder, reloadArchive, closeArchive, startRepair, startCreate, startConvert, startUnlock, startPwOp, protectedArchive, lockedArchive,
  } from '../store'
  import * as ipc from '../ipc'

  const hasArchive = computed(() => !!S.info)
  const hasPath = computed(() => !!S.openPath)

  // 显式受控 show：select 后立即收起，避免气泡在模态框打开时残留。
  const fileShow = ref(false)
  const opsShow = ref(false)
  const toolsShow = ref(false)
  const settingsShow = ref(false)

  const fileOptions = computed((): DropdownOption[] => [
    { label: '新建归档…', key: 'create' },
    { label: '打开归档…', key: 'open' },
    { label: '重新载入归档\u2003F5', key: 'reload', disabled: !hasPath.value },
    { label: '关闭归档', key: 'close', disabled: !hasPath.value },
    { type: 'divider', key: 'd1' },
    { label: '退出', key: 'quit' },
  ])

  const opsOptions = computed((): DropdownOption[] => [
    { label: '提取全部…', key: 'extract-all', disabled: !hasArchive.value },
    { label: '导入文件到归档…', key: 'import', disabled: !hasArchive.value },
    { label: '导入文件夹到归档…', key: 'import-folder', disabled: !hasArchive.value },
    { label: '新建文件夹…', key: 'new-folder', disabled: !hasArchive.value },
  ])

  const toolsOptions = computed((): DropdownOption[] => [
    { label: '修复头信息…', key: 'repair', disabled: !hasPath.value },
    { label: '版本转换…', key: 'convert', disabled: !hasArchive.value },
    { type: 'divider', key: 'd2' },
    { label: '设置密码…', key: 'pw-set', disabled: !hasArchive.value || protectedArchive.value },
    { label: '修改密码…', key: 'pw-change', disabled: !protectedArchive.value },
    { label: '解除密码保护…', key: 'pw-remove', disabled: !protectedArchive.value },
    {
      label: lockedArchive.value ? '输入密码解锁…' : '密码已解锁',
      key: 'pw-unlock',
      disabled: !lockedArchive.value,
    },
  ])

  const settingsOptions: DropdownOption[] = [
    { label: '界面设置…', key: 'settings' },
  ]

  function onFileSelect(key: string | number) {
    fileShow.value = false
    if (key === 'create') startCreate()
    else if (key === 'open') openArchiveDialog()
    else if (key === 'reload') reloadArchive()
    else if (key === 'close') closeArchive()
    else if (key === 'quit') ipc.closeWindow()
  }

  function onOpsSelect(key: string | number) {
    opsShow.value = false
    if (key === 'extract-all') extractAll()
    else if (key === 'import') importFilesDialog()
    else if (key === 'import-folder') importFolderDialog()
    else if (key === 'new-folder') startNewFolder()
  }

  function onToolsSelect(key: string | number) {
    toolsShow.value = false
    if (key === 'repair') startRepair()
    else if (key === 'convert') startConvert()
    else if (key === 'pw-set') startPwOp('set')
    else if (key === 'pw-change') startPwOp('change')
    else if (key === 'pw-remove') startPwOp('remove')
    else if (key === 'pw-unlock') startUnlock()
  }

  function onSettingsSelect(key: string | number) {
    settingsShow.value = false
    if (key === 'settings') S.settingsOpen = true
  }
</script>
