<template>
  <n-modal
    :show="!!S.modify"
    preset="card"
    title="修改归档"
    :style="{ width: showImports && showDeletes ? '640px' : '480px', maxWidth: '94vw', borderRadius: '12px' }"
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

        <!-- 单列时自动占满整行 -->
        <div class="modify-columns">
          <div v-if="showImports" class="modify-col">
            <div class="modify-col-head">
              <span class="section-title">待导入</span>
              <span class="section-count">{{ fmtBytes(S.modify.additions.length) }} 项</span>
            </div>
            <div class="modify-col-body">
              <div v-if="S.modify.additions.length === 0" class="col-empty">
                （无）— 用「操作 → 导入文件 / 导入文件夹」或拖拽添加
              </div>
              <div v-else class="pending-list">
                <div v-for="(a, i) in S.modify.additions" :key="a.name + i" class="pending-row">
                  <n-tag size="small" round :bordered="false" :type="a.replaces ? 'warning' : 'success'">
                    {{ a.replaces ? '替换' : '新增' }}
                  </n-tag>
                  <span class="name">{{ a.name }}</span>
                  <span class="meta">{{ fmtBytes(a.size) }} 字节</span>
                  <n-button size="tiny" quaternary type="error" class="remove"
                            :disabled="busy" title="移除该项"
                            @click="S.modify.additions.splice(i, 1)">✕</n-button>
                </div>
              </div>
            </div>
          </div>

          <div v-if="showDeletes" class="modify-col">
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
                  <n-button size="tiny" quaternary type="error" class="remove"
                            :disabled="busy" title="移除该项"
                            @click="S.modify.deletions.splice(i, 1)">✕</n-button>
                </div>
              </div>
            </div>
          </div>
        </div>

        <div class="hint-text">确认时保留文件将原样拷贝；(listfile) 重建，(attributes)/(signature) 自动移除。</div>

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
      </div>
    </template>

    <template v-if="S.modify" #action>
      <div class="dialog-actions">
        <n-button v-if="S.modify.importBack" size="small" quaternary :disabled="busy"
                  title="撤掉本次文件夹导入的待办，返回「导入文件夹」确认窗口"
                  @click="stepBackImportFolder">上一步</n-button>
        <div class="grow"></div>
        <n-button size="small" quaternary :disabled="busy" @click="closeModify">取消</n-button>
        <n-button ref="confirmBtn" size="small" type="primary" :disabled="empty" :loading="busy"
                  title="应用修改并自动重新载入；勾选「另存为」后改为选择保存位置"
                  @click="confirmModify">确认</n-button>
      </div>
    </template>
  </n-modal>
</template>

<script setup>
  import { computed, ref } from 'vue'
  import { NModal, NButton, NTag, NCheckbox, NTooltip } from 'naive-ui'
  import { S, confirmModify, closeModify, stepBackImportFolder } from '../store'
  import { fmtBytes } from '../utils'

  const adds = computed(() => S.modify?.additions.length ?? 0)
  const dels = computed(() => S.modify?.deletions.length ?? 0)
  const empty = computed(() => adds.value === 0 && dels.value === 0)
  const busy = computed(() => !!S.modify?.busy)
  // 常态（先排队一类再打开弹窗）只显示有待办的一列；
  // 两类都有（弹窗开着时拖入文件）才同时显示两列。
  const showImports = computed(() => adds.value > 0 || dels.value === 0)
  const showDeletes = computed(() => dels.value > 0)

  // 弹窗过渡结束后聚焦「确认」，回车可直接应用修改。naive-ui 按钮
  // 实例没有 focus 方法，取底层 <button> 元素调用原生 focus。
  const confirmBtn = ref(null)
  function focusConfirm() {
    const el = confirmBtn.value?.$el
    if (!el) return
    el.focus()
    // 前一个弹窗（如导入确认窗）关闭归还焦点可能发生在此之后，补一次。
    setTimeout(() => el.focus(), 150)
  }
</script>
