<script setup>
import { computed } from 'vue'
import { NModal, NButton, NTag, NCheckbox } from 'naive-ui'
import { S, applyModify, saveModified, openModified, closeModify } from '../store'
import { fmtBytes } from '../utils'

const frozen = computed(() => !!S.modify?.applied)
const empty = computed(() => S.modify.additions.length === 0 && S.modify.deletions.length === 0)
</script>

<template>
  <n-modal
    :show="!!S.modify"
    preset="card"
    title="修改归档"
    :style="{ width: '560px', maxWidth: '94vw' }"
    :mask-closable="false"
    :bordered="false"
    @update:show="(v) => { if (!v) closeModify() }"
  >
    <template v-if="S.modify">
      <div class="modal-src">来源：{{ S.openPath || '（未保存）' }}</div>
      <hr class="modal-sep" />

      <div class="modify-scroll">
        <div class="section-label">待导入（{{ fmtBytes(S.modify.additions.length) }} 项）</div>
        <div v-if="S.modify.additions.length === 0" class="hint-text">
          （无）— 用「文件 → 导入文件到归档…」添加
        </div>
        <div v-else class="pending-list">
          <div v-for="(a, i) in S.modify.additions" :key="a.name + i" class="pending-row">
            <n-tag size="small" :bordered="false" :type="a.replaces ? 'warning' : 'default'">
              {{ a.replaces ? '替换' : '新增' }}
            </n-tag>
            <span class="name">{{ a.name }}</span>
            <span class="meta">{{ fmtBytes(a.size) }} 字节</span>
            <n-button v-if="!frozen" size="tiny" quaternary class="remove"
                      @click="S.modify.additions.splice(i, 1)">✕</n-button>
          </div>
        </div>

        <div class="section-label" style="margin-top: 8px">
          待删除（{{ fmtBytes(S.modify.deletions.length) }} 项）
        </div>
        <div v-if="S.modify.deletions.length === 0" class="hint-text">
          （无）— 勾选文件后点「删除选中」
        </div>
        <div v-else class="pending-list">
          <div v-for="(d, i) in S.modify.deletions" :key="d.key + i" class="pending-row">
            <n-tag size="small" :bordered="false">删除</n-tag>
            <span class="name">{{ d.display }}</span>
            <n-button v-if="!frozen" size="tiny" quaternary class="remove"
                      @click="S.modify.deletions.splice(i, 1)">✕</n-button>
          </div>
        </div>

        <div class="hint-text" style="margin-top: 4px">
          应用时保留文件将原样拷贝；(listfile) 重建，(attributes)/(signature) 自动移除。
        </div>
      </div>
      <hr class="modal-sep" />

      <n-checkbox v-if="!frozen" v-model:checked="S.modify.reencrypt"
        title="key-v2 的密钥包含块偏移，修改后必须重算。勾选：用新偏移的密钥重新加密（保持加密状态）；&#10;取消：存为未加密（内容不变，任何工具都能读）">
        key-v2 文件按新偏移重新加密
      </n-checkbox>

      <div class="modal-actions">
        <n-button size="small" type="primary" :disabled="frozen || empty"
                  @click="applyModify">应用修改</n-button>
        <n-button size="small" :disabled="!frozen" title="选择保存位置（默认覆盖原文件）后自动重新载入"
                  @click="saveModified">保存到文件…</n-button>
        <n-button size="small" :disabled="!frozen" title="不写盘，直接在界面中浏览修改结果"
                  @click="openModified">在界面中打开结果</n-button>
        <n-button size="small" @click="closeModify">关闭</n-button>
      </div>
      <div v-if="S.modify.note" class="modal-note">{{ S.modify.note }}</div>
    </template>
  </n-modal>
</template>
