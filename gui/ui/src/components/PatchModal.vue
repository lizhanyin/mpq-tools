<template>
  <n-modal
    :show="!!S.patch"
    preset="card"
    title="生成补丁 (patch.mpq)"
    :style="{ width: 'max-content', minWidth: '620px', maxWidth: '94vw', borderRadius: '12px' }"
    :mask-closable="false"
    :bordered="false"
    @update:show="(v) => { if (!v && !busy) closePatch() }"
  >
    <template v-if="S.patch">
      <div class="dialog-body">
        <div class="dialog-meta">
          <span class="meta-label">新文件</span>
          <span class="meta-value" :title="S.openPath || ''">{{ S.openPath || '' }}</span>
        </div>
        <div class="dialog-meta">
          <span class="meta-label">旧文件（基线）</span>
          <span v-if="p.oldPath" class="meta-value" :title="p.oldPath">{{ p.oldPath }}</span>
          <n-button size="tiny" quaternary type="primary" :disabled="p.loading || p.busy"
                    @click="pickPatchBaseline">
            {{ p.oldPath ? '重新选择…' : '选择…' }}
          </n-button>
        </div>

        <div class="hint-text">
          对比当前归档（新）与基线（旧），勾选的「新增 / 修改」文件按新版本
          打包成独立的补丁归档；游戏 / 工具按补丁优先加载即可覆盖旧文件。
        </div>

        <!-- 对比进度（读取两侧内容并逐文件比较） -->
        <div v-if="p.loading || (p.busy && S.importProgress)" class="import-progress">
          <n-progress type="line" :height="6" :show-indicator="false"
                      :percentage="pct ?? 0" :processing="pct === undefined" />
          <span class="import-progress-label">
            {{ importProgressLabel(S.importProgress) || '处理中…' }}
          </span>
        </div>

        <template v-if="p.diff">
          <div v-if="p.diff.read_errors.length" class="diff-errors">
            {{ p.diff.read_errors.length }} 个文件内容比较失败，已按「修改」处理
           （展开查看详情）
            <ul>
              <li v-for="e in p.diff.read_errors.slice(0, 5)" :key="e">{{ e }}</li>
              <li v-if="p.diff.read_errors.length > 5">… 其余 {{ p.diff.read_errors.length - 5 }} 项</li>
            </ul>
          </div>
          <DiffList
            :diff="p.diff"
            :unchecked="p.unchecked"
            :labels="{ added: '新增（进补丁）', changed: '修改（进补丁）', same: '相同（跳过）', removed: '仅旧文件有' }"
          />
          <div v-if="p.diff.unnamed_old > 0 || p.diff.unnamed_new > 0" class="hint-text">
            未命名条目不参与对比（旧 {{ p.diff.unnamed_old }} 个、新 {{ p.diff.unnamed_new }} 个）。
          </div>

          <div class="field-label">补丁版本</div>
          <n-radio-group :value="p.version ?? 'auto'" name="patch-version" :disabled="p.busy"
                         @update:value="(v) => { p.version = v === 'auto' ? null : v }">
            <n-radio value="auto">跟随当前（v{{ S.info?.version ?? '?' }}）</n-radio>
            <n-radio :value="1">v1</n-radio>
            <n-radio :value="2">v2</n-radio>
            <n-radio :value="3">v3</n-radio>
            <n-radio :value="4">v4</n-radio>
          </n-radio-group>
        </template>
        <div v-else-if="!p.loading" class="hint-text">
          {{ p.oldPath ? '（对比失败——可重选基线文件）' : '请先选择要对比的旧文件。' }}
        </div>
      </div>
    </template>

    <template v-if="S.patch" #action>
      <div class="dialog-actions">
        <div class="grow"></div>
        <n-button size="small" quaternary :disabled="busy" @click="closePatch">取消</n-button>
        <n-button size="small" type="primary" :loading="busy" :disabled="!p.diff || picked === 0"
                  title="选择保存位置并生成补丁归档"
                  @click="confirmBuildPatch">生成 patch.mpq…</n-button>
      </div>
    </template>
  </n-modal>
</template>

<script setup lang="ts">
  import { computed } from 'vue'
  import { NModal, NButton, NProgress, NRadioGroup, NRadio } from 'naive-ui'
  import { S, closePatch, pickPatchBaseline, confirmBuildPatch, importProgressLabel, importProgressPercent } from '../store'
  import DiffList from './DiffList.vue'

  const p = computed(() => S.patch!)

  const busy = computed(() => !!(S.patch && (S.patch.busy || S.patch.loading)))
  const pct = computed(() => importProgressPercent(S.importProgress))
  const picked = computed(() => {
    const d = S.patch?.diff
    if (!d) return 0
    return d.entries.filter(
      (e) => (e.kind === 'added' || e.kind === 'changed') && !S.patch!.unchecked.has(e.name.toLowerCase()),
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
