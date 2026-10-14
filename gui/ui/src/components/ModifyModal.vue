<script setup>
import { computed } from 'vue'
import { NModal, NButton, NTag, NCheckbox, NTooltip } from 'naive-ui'
import { S, applyModify, saveModified, openModified, closeModify } from '../store'
import { fmtBytes } from '../utils'

const frozen = computed(() => !!S.modify?.applied)
const empty = computed(() => S.modify.additions.length === 0 && S.modify.deletions.length === 0)
const noteIsError = computed(() => (S.modify?.note || '').includes('失败'))
</script>

<template>
  <n-modal
    :show="!!S.modify"
    preset="card"
    title="修改归档"
    :style="{ width: '640px', maxWidth: '94vw', borderRadius: '12px' }"
    :mask-closable="false"
    :bordered="false"
    @update:show="(v) => { if (!v) closeModify() }"
  >
    <template v-if="S.modify">
      <div class="dialog-body">
        <div class="dialog-meta">
          <span class="meta-label">来源</span>
          <span class="meta-value">{{ S.openPath || '（未保存）' }}</span>
        </div>

        <!-- 左：待导入 / 右：待删除，各自独立滚动 -->
        <div class="modify-columns">
          <div class="modify-col">
            <div class="modify-col-head">
              <span class="section-title">待导入</span>
              <span class="section-count">{{ fmtBytes(S.modify.additions.length) }} 项</span>
            </div>
            <div class="modify-col-body">
              <div v-if="S.modify.additions.length === 0" class="col-empty">
                （无）— 用「文件 → 导入文件到归档…」添加
              </div>
              <div v-else class="pending-list">
                <div v-for="(a, i) in S.modify.additions" :key="a.name + i" class="pending-row">
                  <n-tag size="small" round :bordered="false" :type="a.replaces ? 'warning' : 'success'">
                    {{ a.replaces ? '替换' : '新增' }}
                  </n-tag>
                  <span class="name">{{ a.name }}</span>
                  <span class="meta">{{ fmtBytes(a.size) }} 字节</span>
                  <n-button v-if="!frozen" size="tiny" quaternary type="error" class="remove"
                            title="移除该项" @click="S.modify.additions.splice(i, 1)">✕</n-button>
                </div>
              </div>
            </div>
          </div>

          <div class="modify-col">
            <div class="modify-col-head">
              <span class="section-title">待删除</span>
              <span class="section-count">{{ fmtBytes(S.modify.deletions.length) }} 项</span>
            </div>
            <div class="modify-col-body">
              <div v-if="S.modify.deletions.length === 0" class="col-empty">
                （无）— 勾选文件后点「删除选中」
              </div>
              <div v-else class="pending-list">
                <div v-for="(d, i) in S.modify.deletions" :key="d.key + i" class="pending-row">
                  <n-tag size="small" round :bordered="false" type="error">删除</n-tag>
                  <span class="name">{{ d.display }}</span>
                  <n-button v-if="!frozen" size="tiny" quaternary type="error" class="remove"
                            title="移除该项" @click="S.modify.deletions.splice(i, 1)">✕</n-button>
                </div>
              </div>
            </div>
          </div>
        </div>

        <div class="hint-text">应用时保留文件将原样拷贝；(listfile) 重建，(attributes)/(signature) 自动移除。</div>

        <n-checkbox v-if="!frozen" v-model:checked="S.modify.reencrypt" size="small">
          key-v2 文件按新偏移重新加密
          <n-tooltip trigger="hover" placement="top" :style="{ maxWidth: '320px' }">
            <template #trigger>
              <span class="info-mark">ⓘ</span>
            </template>
            key-v2 的密钥包含块偏移，修改后必须重算。<br />
            勾选：用新偏移的密钥重新加密（保持加密状态）；<br />
            取消：存为未加密（内容不变，任何工具都能读）
          </n-tooltip>
        </n-checkbox>
      </div>
    </template>

    <template v-if="S.modify" #action>
      <div class="dialog-actions">
        <n-button size="small" quaternary @click="closeModify">关闭</n-button>
        <div class="grow"></div>
        <n-button size="small" :disabled="!frozen"
                  @click="openModified">在界面中打开结果</n-button>
        <n-button size="small" :disabled="!frozen"
                  title="选择保存位置（默认覆盖原文件）后自动重新载入"
                  @click="saveModified">保存到文件…</n-button>
        <n-button size="small" type="primary" :disabled="frozen || empty"
                  @click="applyModify">应用修改</n-button>
      </div>
    </template>

    <template v-if="S.modify && S.modify.note" #footer>
      <div class="dialog-note" :class="{ error: noteIsError }">{{ S.modify.note }}</div>
    </template>
  </n-modal>
</template>
