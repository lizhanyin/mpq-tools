<template>
  <n-modal
    :show="!!S.modify"
    preset="card"
    title="修改归档"
    :style="{ width: 'max-content', minWidth: showImports && showDeletes ? '640px' : '480px', maxWidth: '94vw', borderRadius: '12px' }"
    :mask-closable="false"
    :bordered="false"
    :auto-focus="false"
    @after-enter="focusConfirm"
    @update:show="(v) => { if (!v && !busy) closeModify() }"
  >
    <template v-if="S.modify">
      <div class="dialog-body">
        <div class="dialog-meta">
          <span class="meta-label">来源</span>
          <span class="meta-value">{{ S.openPath || '（未保存）' }}</span>
        </div>

        <!-- 「导入到」只在有待导入（添加模式）时显示 -->
        <div v-if="adds > 0" class="dialog-meta">
          <span class="meta-label">导入到</span>
          <span class="meta-value" :title="importTargetDir || '（归档根目录）'">
            {{ importTargetDir || '（归档根目录）' }}
          </span>
        </div>

        <!-- 单列时自动占满整行 -->
        <div class="modify-columns">
          <div v-if="showImports" class="modify-col">
            <div class="modify-col-head">
              <span class="section-title">待导入</span>
              <span class="section-count">{{ fmtBytes(S.modify.additions.length) }} 项</span>
            </div>
            <div ref="addsBody" class="modify-col-body">
              <div v-if="S.modify.additions.length === 0" class="col-empty">
                {{ scanning ? '正在统计待导入内容…' : '（无）— 用「操作 → 导入文件 / 导入文件夹」或拖拽添加' }}
              </div>
              <div v-else class="pending-list">
                <div v-for="(a, i) in shownAdds" :key="a.name + i" class="pending-row">
                  <n-tag size="small" round :bordered="false"
                         :type="a.movedFrom ? 'info' : a.replaces ? 'warning' : 'success'">
                    {{ a.movedFrom ? '移动' : a.replaces ? '替换' : '新增' }}
                  </n-tag>
                  <span class="name" :title="a.movedFrom ? `${a.movedFrom} → ${a.name}` : a.name">{{ baseNameOf(a.name) }}</span>
                  <span class="meta">{{ fmtBytes(a.size) }} 字节</span>
                  <n-button size="tiny" quaternary type="error" class="remove"
                            :disabled="busy" title="移除该项"
                            @click="S.modify.additions.splice(i, 1)">✕</n-button>
                </div>
                <!-- 大批量分批渲染：先出 100 条，点「加载更多」再追加 -->
                <div v-if="hiddenAdds > 0" class="load-more">
                  <span>已显示 {{ shownAdds.length }} / 共 {{ S.modify.additions.length }} 项</span>
                  <n-button size="tiny" quaternary type="primary" :disabled="busy"
                            @click="visibleAdds += IMPORT_PREVIEW_STEP">
                    加载更多（还有 {{ hiddenAdds }} 项）
                  </n-button>
                </div>
              </div>
            </div>
          </div>

          <div v-if="showDeletes" class="modify-col">
            <div class="modify-col-head">
              <span class="section-title">待删除</span>
              <span class="section-count">{{ fmtBytes(S.modify.deletions.length) }} 项</span>
            </div>
            <div ref="delsBody" class="modify-col-body">
              <div v-if="S.modify.deletions.length === 0" class="col-empty">
                （无）— 勾选文件后点「删除选中」
              </div>
              <div v-else class="pending-list">
                <div v-for="(d, i) in shownDels" :key="d.key + i" class="pending-row">
                  <n-tag size="small" round :bordered="false" type="error">删除</n-tag>
                  <span class="name">{{ d.display }}</span>
                  <n-button size="tiny" quaternary type="error" class="remove"
                            :disabled="busy" title="移除该项"
                            @click="S.modify.deletions.splice(i, 1)">✕</n-button>
                </div>
                <div v-if="hiddenDels > 0" class="load-more">
                  <span>已显示 {{ shownDels.length }} / 共 {{ S.modify.deletions.length }} 项</span>
                  <n-button size="tiny" quaternary type="primary" :disabled="busy"
                            @click="visibleDels += IMPORT_PREVIEW_STEP">
                    加载更多（还有 {{ hiddenDels }} 项）
                  </n-button>
                </div>
              </div>
            </div>
          </div>
        </div>

        <n-checkbox v-model:checked="S.modify.reencrypt" size="small" :disabled="busy">
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

        <n-checkbox v-model:checked="S.modify.saveAs" size="small" :disabled="busy">
          另存为
          <n-tooltip trigger="hover" placement="top" :style="{ maxWidth: '320px' }">
            <template #trigger>
              <span class="info-mark">ⓘ</span>
            </template>
            勾选：确认时选择新的保存位置，原文件保持不变；<br />
            不勾：修改直接写回当前打开的文件
          </n-tooltip>
        </n-checkbox>

        <!-- 文件夹导入才显示；勾选后待导入名称与「导入到」地址同步变化 -->
        <n-checkbox v-if="adds > 0 && S.modify.folderImport" size="small" :disabled="busy"
                    :checked="S.modify.folderImport.childrenOnly"
                    @update:checked="setImportChildrenOnly">
          只导入子项
        </n-checkbox>

        <!-- 导入进度：读取（含选择文件夹后的统计扫描）→ 准备 → 导入（压缩）
             → 校验 → 保存。统计阶段弹窗未 busy，但也必须有加载反馈。 -->
        <div v-if="busy || S.importProgress" class="import-progress">
          <n-progress type="line" :height="6" :show-indicator="false"
                      :percentage="pct ?? 0" :processing="pct === undefined" />
          <span class="import-progress-label">
            {{ importProgressLabel(S.importProgress) || '处理中…' }}
          </span>
        </div>
      </div>
    </template>

    <template v-if="S.modify" #action>
      <div class="dialog-actions">
        <div class="grow"></div>
        <n-button size="small" quaternary :disabled="busy" @click="closeModify">取消</n-button>
        <n-button ref="confirmBtn" size="small" type="primary" :disabled="empty" :loading="busy"
                  title="应用修改并自动重新载入；勾选「另存为」后改为选择保存位置"
                  @click="confirmModify">确认</n-button>
      </div>
    </template>
  </n-modal>
</template>

<script setup lang="ts">
  import { computed, ref, watch } from 'vue'
  import { NModal, NButton, NTag, NCheckbox, NTooltip, NProgress } from 'naive-ui'
  import {
    S, confirmModify, closeModify, setImportChildrenOnly, importTargetDir,
    importProgressLabel, importProgressPercent,
  } from '../store'
  import { fmtBytes, basenameOf } from '../utils'

  const adds = computed(() => S.modify?.additions.length ?? 0)
  const dels = computed(() => S.modify?.deletions.length ?? 0)
  const empty = computed(() => adds.value === 0 && dels.value === 0)
  const busy = computed(() => !!S.modify?.busy)
  // 正在统计待导入内容（选择文件夹 / 拖入 / 切换「只导入子项」的扫描阶段）。
  const scanning = computed(() => !!S.importProgress && !busy.value)
  // 常态（先排队一类再打开弹窗）只显示有待办的一列；
  // 两类都有（弹窗开着时拖入文件）才同时显示两列。
  const showImports = computed(() => adds.value > 0 || dels.value === 0)
  const showDeletes = computed(() => dels.value > 0)

  // 待导入列表只显示文件名（添加模式）；悬停可见完整归档内路径。
  const baseNameOf = basenameOf

  // -- 大批量分批渲染 -------------------------------------------------------
  // 列表先渲染 100 条，点「加载更多」每次追加 100 条；上万条目时避免
  // 一次性建出全部 DOM。移除单条用 splice 原位修改（引用不变、索引
  // 一致）；「只导入子项」重新统计会整体替换数组引用，此时重置回
  // 首批并回到列表顶部。
  const IMPORT_PREVIEW_STEP = 100
  const visibleAdds = ref(IMPORT_PREVIEW_STEP)
  const visibleDels = ref(IMPORT_PREVIEW_STEP)
  const shownAdds = computed(() => S.modify?.additions.slice(0, visibleAdds.value) ?? [])
  const shownDels = computed(() => S.modify?.deletions.slice(0, visibleDels.value) ?? [])
  const hiddenAdds = computed(() => adds.value - shownAdds.value.length)
  const hiddenDels = computed(() => dels.value - shownDels.value.length)

  const addsBody = ref<HTMLElement | null>(null)
  const delsBody = ref<HTMLElement | null>(null)
  watch(() => S.modify?.additions, (arr, old) => {
    if (arr === old) return
    visibleAdds.value = IMPORT_PREVIEW_STEP
    addsBody.value?.scrollTo({ top: 0 })
  })
  watch(() => S.modify?.deletions, (arr, old) => {
    if (arr === old) return
    visibleDels.value = IMPORT_PREVIEW_STEP
    delsBody.value?.scrollTo({ top: 0 })
  })

  // 确认后的导入进度（store.importProgress 的展示辅助）。
  const pct = computed(() => importProgressPercent(S.importProgress))

  // 弹窗过渡结束后聚焦「确认」，回车可直接应用修改。naive-ui 按钮
  // 实例没有 focus 方法，取底层 <button> 元素调用原生 focus。
  const confirmBtn = ref<InstanceType<typeof NButton> | null>(null)
  function focusConfirm() {
    const el = confirmBtn.value?.$el as HTMLButtonElement | undefined
    if (!el) return
    el.focus()
    // 前一个弹窗（如导入确认窗）关闭归还焦点可能发生在此之后，补一次。
    setTimeout(() => el.focus(), 150)
  }
</script>
