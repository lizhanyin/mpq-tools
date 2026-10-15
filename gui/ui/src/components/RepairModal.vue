<template>
  <n-modal
    :show="!!S.repair"
    preset="card"
    title="修复头信息"
    :style="{ width: '600px', maxWidth: '94vw', borderRadius: '12px' }"
    :mask-closable="false"
    :bordered="false"
    @update:show="(v) => { if (!v) closeRepair() }"
  >
    <template v-if="S.repair">
      <div class="dialog-body">
        <div class="dialog-meta">
          <span class="meta-label">来源</span>
          <span class="meta-value">{{ S.openPath || '' }}</span>
        </div>

        <div v-if="S.repair.issues.length" class="dialog-subtitle">
          共 {{ S.repair.issues.length }} 个问题 · {{ fixable }} 个可自动修复 · {{ fatal }} 个无法修复
        </div>

        <n-empty v-if="S.repair.issues.length === 0" description="未发现头信息问题。" />
        <div v-else class="issue-panel">
          <div v-for="(issue, i) in S.repair.issues" :key="i" class="issue">
            <n-tag size="small" round :bordered="false" :type="issue.action ? 'warning' : 'error'">
              {{ issue.action ? '可修复' : '无法修复' }}
            </n-tag>
            <span class="chip">{{ issue.field }}</span>
            <span class="desc">{{ issue.action ? `${issue.problem} → ${issue.action}` : issue.problem }}</span>
          </div>
        </div>
      </div>
    </template>

    <template v-if="S.repair" #action>
      <div class="dialog-actions">
        <n-button size="small" quaternary @click="closeRepair">关闭</n-button>
        <div class="grow"></div>
        <n-button size="small" :disabled="!S.repair.applied"
                  @click="saveRepaired">保存修复副本…</n-button>
        <n-button size="small" :disabled="!S.repair.applied"
                  @click="openRepaired">在界面中打开修复结果</n-button>
        <n-button size="small" type="primary"
                  :disabled="S.repair.applied || S.repair.issues.length === 0"
                  @click="applyRepair">应用修复</n-button>
      </div>
    </template>
  </n-modal>
</template>

<script setup>
  import { computed } from 'vue'
  import { NModal, NButton, NTag, NEmpty } from 'naive-ui'
  import { S, applyRepair, saveRepaired, openRepaired, closeRepair } from '../store'

  const fixable = computed(() =>
    S.repair ? S.repair.issues.filter((i) => i.action).length : 0)
  const fatal = computed(() =>
    S.repair ? S.repair.issues.length - fixable.value : 0)
</script>
