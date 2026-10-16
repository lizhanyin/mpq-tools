<template>
  <n-modal
    :show="!!S.importFolder"
    preset="card"
    title="导入文件夹"
    :style="{ width: '440px', maxWidth: '94vw', borderRadius: '12px' }"
    :mask-closable="false"
    :bordered="false"
    @update:show="(v) => { if (!v) closeImportFolder() }"
  >
    <template v-if="S.importFolder">
      <div class="dialog-body">
        <div class="field-label">所选文件夹</div>
        <div class="mono" style="word-break: break-all">{{ S.importFolder.path }}</div>
        <n-checkbox v-model:checked="S.importFolder.childrenOnly">
          只导入子项（不以文件夹名为根）
        </n-checkbox>
        <div class="hint-text">
          <template v-if="S.importFolder.childrenOnly">
            文件夹的全部内容直接导入{{ S.cwd.length ? `当前文件夹（${S.cwd.join('\\')}）` : '归档根目录' }}，
            如 {{ base }} 内的 a.png 会成为 a.png、ui\b.png 会成为 ui\b.png。
          </template>
          <template v-else>
            以文件夹为根导入，如 {{ base }} 内的 a.png 会成为 {{ base }}\a.png。
          </template>
        </div>
      </div>
    </template>

    <template v-if="S.importFolder" #action>
      <div class="dialog-actions">
        <div class="grow"></div>
        <n-button size="small" quaternary @click="closeImportFolder">取消</n-button>
        <n-button size="small" type="primary" @click="confirmImportFolder()">导入</n-button>
      </div>
    </template>
  </n-modal>
</template>

<script setup>
  import { computed } from 'vue'
  import { NModal, NButton, NCheckbox } from 'naive-ui'
  import { S, confirmImportFolder, closeImportFolder } from '../store'

  const base = computed(() =>
    (S.importFolder?.path || '').split(/[\\/]/).filter(Boolean).pop() || '(文件夹)')
</script>
