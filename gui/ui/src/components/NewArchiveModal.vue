<template>
  <n-modal
    :show="!!S.create"
    preset="card"
    title="新建归档"
    :style="{ width: '520px', maxWidth: '94vw', borderRadius: '12px' }"
    :mask-closable="false"
    :bordered="false"
    @update:show="(v) => { if (!v) closeCreate() }"
  >
    <template v-if="S.create">
      <div class="dialog-body">
        <div class="field-label">文件版本</div>
        <n-radio-group v-model:value="S.create.version" name="create-version">
          <div class="version-list">
            <div v-for="v in VERSIONS" :key="v.value" class="version-row">
              <n-radio :value="v.value">{{ v.label }}</n-radio>
              <span class="version-hint">{{ v.hint }}</span>
            </div>
          </div>
        </n-radio-group>

        <div class="field-label">哈希表大小（文件名槽数）</div>
        <n-select v-model:value="S.create.hashSize" :options="HASH_SIZES"
                  size="small" :style="{ width: '160px' }" />

        <div class="hint-text">
          将创建一个空的归档文件，之后用「文件 → 导入文件到归档…」添加内容。
          槽位不足时导入会自动扩容；v4 会额外写入 MD5 校验数据。
        </div>
      </div>
    </template>

    <template v-if="S.create" #action>
      <div class="dialog-actions">
        <n-button size="small" quaternary @click="closeCreate">取消</n-button>
        <div class="grow"></div>
        <n-button size="small" type="primary" @click="createArchive">选择位置并创建…</n-button>
      </div>
    </template>

    <template v-if="S.create && S.create.note" #footer>
      <div class="dialog-note" :class="{ error: noteIsError }">{{ S.create.note }}</div>
    </template>
  </n-modal>
</template>

<script setup>
  import { computed } from 'vue'
  import { NModal, NButton, NRadioGroup, NRadio, NSelect } from 'naive-ui'
  import { S, createArchive, closeCreate } from '../store'

  const VERSIONS = [
    { value: 1, label: 'v1', hint: '经典格式，兼容性最好' },
    { value: 2, label: 'v2', hint: 'v1 + hi-block 表（64 位文件偏移）' },
    { value: 3, label: 'v3', hint: 'v2 + 64 位归档大小与 HET/BET 表' },
    { value: 4, label: 'v4', hint: 'v3 + 头部与各表的 MD5 校验' },
  ]

  const HASH_SIZES = [64, 256, 1024, 4096].map((n) => ({
    label: `${n.toLocaleString('en-US')} 槽`,
    value: n,
  }))

  const noteIsError = computed(() => (S.create?.note || '').includes('失败'))
</script>
