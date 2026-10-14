<script setup>
import { NModal, NButton, NTag } from 'naive-ui'
import { S, applyRepair, saveRepaired, openRepaired, closeRepair } from '../store'
</script>

<template>
  <n-modal
    :show="!!S.repair"
    preset="card"
    title="修复头信息"
    :style="{ width: '560px', maxWidth: '94vw' }"
    :mask-closable="false"
    :bordered="false"
    @update:show="(v) => { if (!v) closeRepair() }"
  >
    <template v-if="S.repair">
      <div class="modal-src">来源：{{ S.openPath || '' }}</div>
      <hr class="modal-sep" />

      <div v-if="S.repair.issues.length === 0">未发现头信息问题。</div>
      <div v-else class="issue-scroll">
        <div v-for="(issue, i) in S.repair.issues" :key="i" class="issue">
          <n-tag size="small" :type="issue.action ? 'warning' : 'error'" :bordered="false">
            {{ issue.action ? '可修复' : '无法修复' }}
          </n-tag>
          <span class="field">{{ issue.field }}</span>
          <span class="desc">{{ issue.action ? `${issue.problem} → ${issue.action}` : issue.problem }}</span>
        </div>
      </div>
      <hr class="modal-sep" />

      <div class="modal-actions">
        <n-button size="small" :disabled="S.repair.applied || S.repair.issues.length === 0"
                  @click="applyRepair">应用修复</n-button>
        <n-button size="small" :disabled="!S.repair.applied"
                  @click="saveRepaired">保存修复副本…</n-button>
        <n-button size="small" :disabled="!S.repair.applied"
                  @click="openRepaired">在界面中打开修复结果</n-button>
        <n-button size="small" @click="closeRepair">关闭</n-button>
      </div>
      <div v-if="S.repair.note" class="modal-note">{{ S.repair.note }}</div>
    </template>
  </n-modal>
</template>
