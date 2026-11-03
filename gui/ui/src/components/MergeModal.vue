<template>
  <n-modal
    :show="!!S.merge"
    preset="card"
    title="合并归档"
    :style="{ width: 'max-content', minWidth: '620px', maxWidth: '94vw', borderRadius: '12px' }"
    :mask-closable="false"
    :bordered="false"
    @update:show="(v) => { if (!v && !staging) closeMerge() }"
  >
    <template v-if="S.merge">
      <div class="dialog-body">
        <div class="dialog-meta">
          <span class="meta-label">目标</span>
          <span class="meta-value" :title="S.openPath || ''">{{ S.openPath || '' }}</span>
        </div>
        <div class="dialog-meta">
          <span class="meta-label">来源</span>
          <span v-if="m.sourcePath" class="meta-value" :title="m.sourcePath">{{ m.sourcePath }}</span>
          <n-button size="tiny" quaternary type="primary" :disabled="m.loading || m.staging"
                    @click="pickMergeSource">
            {{ m.sourcePath ? '重新选择…' : '选择…' }}
          </n-button>
        </div>

        <div class="hint-text">
          把来源归档的文件合并进当前归档：来源独有的直接导入；同名且内容
          不同的默认以来源覆盖——可按列表勾选或用组头的「全选 / 清空」一键
          决定。确认后在「修改归档」窗口应用写入。
        </div>

        <!-- 对比进度 -->
        <div v-if="m.loading" class="import-progress">
          <n-progress type="line" :height="6" :show-indicator="false" :percentage="0" processing />
          <span class="import-progress-label">{{ importProgressLabel(S.importProgress) || '对比中…' }}</span>
        </div>

        <template v-if="m.diff">
          <div v-if="m.diff.read_errors.length" class="diff-errors">
            {{ m.diff.read_errors.length }} 个文件内容比较失败，已按「同名不同内容」处理
            <ul>
              <li v-for="e in m.diff.read_errors.slice(0, 5)" :key="e">{{ e }}</li>
              <li v-if="m.diff.read_errors.length > 5">… 其余 {{ m.diff.read_errors.length - 5 }} 项</li>
            </ul>
          </div>
          <DiffList
            :diff="m.diff"
            :unchecked="m.unchecked"
            :labels="{ added: '新增（来源独有）', changed: '覆盖（同名不同内容）', same: '完全相同（跳过）', removed: '仅当前归档有（保持不变）' }"
          />
          <div v-if="m.diff.unnamed_old > 0 || m.diff.unnamed_new > 0" class="hint-text">
            未命名条目不参与合并（当前归档 {{ m.diff.unnamed_old }} 个、来源 {{ m.diff.unnamed_new }} 个）。
          </div>
        </template>
        <div v-else-if="!m.loading" class="hint-text">
          {{ m.sourcePath ? '（对比失败——可重选来源文件）' : '请先选择要合并进来的来源归档。' }}
        </div>
      </div>
    </template>

    <template v-if="S.merge" #action>
      <div class="dialog-actions">
        <div class="grow"></div>
        <n-button size="small" quaternary :disabled="staging" @click="closeMerge">取消</n-button>
        <n-button size="small" type="primary" :loading="staging" :disabled="!m.diff || picked === 0"
                  title="把勾选的来源文件排队为待导入，在「修改归档」窗口应用"
                  @click="confirmMerge">合并到当前归档…</n-button>
      </div>
    </template>
  </n-modal>
</template>

<script setup lang="ts">
  import { computed } from 'vue'
  import { NModal, NButton, NProgress } from 'naive-ui'
  import { S, closeMerge, pickMergeSource, confirmMerge, importProgressLabel } from '../store'
  import DiffList from './DiffList.vue'

  const m = computed(() => S.merge!)

  const staging = computed(() => !!(S.merge && (S.merge.staging || S.merge.loading)))
  const picked = computed(() => {
    const d = S.merge?.diff
    if (!d) return 0
    return d.entries.filter(
      (e) => (e.kind === 'added' || e.kind === 'changed') && !S.merge!.unchecked.has(e.name.toLowerCase()),
    ).length
  })
</script>

<style scoped>
  .diff-errors {
    font-size: 12px; color: var(--warning-color, #c47f17);
    border: 1px solid rgba(196, 127, 23, 0.35); border-radius: 6px;
    padding: 4px 10px; margin-bottom: 4px;
  }
  .diff-errors ul { margin: 2px 0 0; padding-left: 18px; opacity: 0.85; max-height: 72px; overflow-y: auto; }
</style>
