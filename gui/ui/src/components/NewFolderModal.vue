<template>
  <n-modal
    :show="!!S.newFolder"
    preset="card"
    title="新建文件夹"
    :style="{ width: 'max-content', minWidth: '440px', maxWidth: '94vw', borderRadius: '12px' }"
    :mask-closable="false"
    :bordered="false"
    @update:show="(v) => { if (!v) closeNewFolder() }"
  >
    <template v-if="S.newFolder">
      <div class="dialog-body">
        <div class="field-label">文件夹名</div>
        <n-input
          v-model:value="S.newFolder.name"
          placeholder="输入名称；用 \ 分隔可一次建多级，如 Units\Human"
          spellcheck="false"
          autofocus
          @keyup.enter="createFolder()"
        />
        <div class="hint-text">
          在 {{ S.cwd.length ? `当前文件夹（${S.cwd.join('\\')}）` : '归档根目录' }} 下创建。
          MPQ 无法保存空文件夹——把文件导入其中并应用修改后，文件夹才会随文件写入归档。
        </div>
      </div>
    </template>

    <template v-if="S.newFolder" #action>
      <div class="dialog-actions">
        <div class="grow"></div>
        <n-button size="small" quaternary @click="closeNewFolder">取消</n-button>
        <n-button size="small" type="primary" @click="createFolder()">创建</n-button>
      </div>
    </template>
  </n-modal>
</template>

<script setup lang="ts">
  import { NModal, NButton, NInput } from 'naive-ui'
  import { S, createFolder, closeNewFolder } from '../store'
</script>
