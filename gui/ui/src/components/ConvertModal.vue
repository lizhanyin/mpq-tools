<template>
  <n-modal
    :show="!!S.convert"
    preset="card"
    title="版本转换"
    :style="{ width: '560px', maxWidth: '94vw', borderRadius: '12px' }"
    :mask-closable="false"
    :bordered="false"
    @update:show="(v) => { if (!v) closeConvert() }"
  >
    <template v-if="S.convert">
      <div class="dialog-body">
        <div class="dialog-meta">
          <span class="meta-label">来源</span>
          <span class="meta-value">{{ S.openPath || '' }}</span>
        </div>
        <div class="dialog-meta">
          <span class="meta-label">当前版本</span>
          <n-tag size="small" round :bordered="false">v{{ from }}</n-tag>
          <span class="meta-label" style="margin-left: 12px">条目</span>
          <span>{{ S.info ? fmtBytes(S.info.entry_count) : 0 }}</span>
          <n-tag v-if="S.info?.het_bet" size="small" round :bordered="false" type="info">
            HET/BET
          </n-tag>
        </div>

        <div class="field-label">目标版本</div>
        <n-radio-group v-model:value="S.convert.target" name="convert-target" :disabled="running">
          <div class="version-list">
            <div v-for="v in VERSIONS" :key="v.value" class="version-row">
              <n-radio :value="v.value" :disabled="v.value === from">
                {{ v.value === from ? `${v.label}（当前）` : v.label }}
              </n-radio>
            </div>
          </div>
        </n-radio-group>

        <ul v-if="hints.length" class="convert-hints">
          <li v-for="(h, i) in hints" :key="i">{{ h }}</li>
        </ul>
        <div class="hint-text">
          所有文件按原始存储字节拷贝，内容不变。个别无法转换的归档（如缺少
          (listfile) 的纯 HET/BET 归档）会在应用时说明原因。
        </div>
      </div>
    </template>

    <template v-if="S.convert" #action>
      <div class="dialog-actions">
        <div class="grow"></div>
        <n-button size="small" type="primary" :loading="running" @click="runConvert">
          开始转换
        </n-button>
      </div>
    </template>

    <template v-if="S.convert && S.convert.note" #footer>
      <div class="dialog-note" :class="{ error: noteIsError }">{{ S.convert.note }}</div>
    </template>
  </n-modal>
</template>

<script setup>
  import { computed } from 'vue'
  import { NModal, NButton, NRadioGroup, NRadio, NTag } from 'naive-ui'
  import { S, runConvert, closeConvert } from '../store'
  import { fmtBytes } from '../utils'

  const VERSIONS = [
    { value: 1, label: 'v1' },
    { value: 2, label: 'v2' },
    { value: 3, label: 'v3' },
    { value: 4, label: 'v4' },
  ]

  const running = computed(() => !!S.convert?.running)
  const noteIsError = computed(() => (S.convert?.note || '').includes('失败'))
  const from = computed(() => S.info?.version ?? 0)

  /** 转换后果提示；无法转换的情况由后端在「应用转换」时给出理由。 */
  const hints = computed(() => {
    if (!S.convert) return []
    const out = []
    if (S.convert.target !== from.value && S.info?.het_bet) {
      out.push('HET/BET 表将改为经典表（文件内容不变，名字取自 (listfile)）')
    }
    out.push('(attributes)/(signature) 将自动移除（其校验数据重写后失效）')
    if (S.convert.target === 4) out.push('v4：将写入头部与各表的 MD5 校验')
    return out
  })
</script>