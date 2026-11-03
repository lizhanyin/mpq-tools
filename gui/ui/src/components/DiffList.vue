<template>
  <div class="diff-list">
    <!-- 可勾选组（added / changed）与只读组（same / removed）。
         大批量时每组先渲染 100 条，「加载更多」追加（与「修改归档」一致）。 -->
    <section v-for="g in groups" :key="g.kind" class="diff-group">
      <header class="diff-group-head">
        <n-tag size="small" round :bordered="false" :type="g.tagType">{{ g.title }}</n-tag>
        <span class="diff-group-count">{{ g.items.length }} 项</span>
        <template v-if="g.checkable">
          <div class="grow"></div>
          <n-button size="tiny" quaternary type="primary" :disabled="allUnchecked(g)"
                    @click="setGroup(g, true)">全选</n-button>
          <n-button size="tiny" quaternary :disabled="allChecked(g)"
                    @click="setGroup(g, false)">清空</n-button>
        </template>
        <template v-else>
          <div class="grow"></div>
          <n-button size="tiny" quaternary @click="g.open = !g.open">
            {{ g.open ? '收起' : `展开（${g.items.length}）` }}
          </n-button>
        </template>
      </header>

      <div v-if="g.checkable || g.open" class="diff-group-body">
        <div v-if="g.items.length === 0" class="diff-empty">（无）</div>
        <div v-for="e in shown(g)" :key="e.name" class="diff-row" :class="{ pickable: g.checkable }">
          <n-checkbox v-if="g.checkable" size="small"
                      :checked="!unchecked.has(keyOf(e))"
                      @update:checked="(v) => toggle(e, v)" />
          <span class="name" :title="e.name">{{ e.name }}</span>
          <span class="sizes">{{ sizesOf(e) }}</span>
        </div>
        <div v-if="hidden(g) > 0" class="load-more">
          <span>已显示 {{ shown(g).length }} / 共 {{ g.items.length }} 项</span>
          <n-button size="tiny" quaternary type="primary" @click="g.visible += STEP">
            加载更多（还有 {{ hidden(g) }} 项）
          </n-button>
        </div>
      </div>
    </section>
  </div>
</template>

<script setup lang="ts">
  import { ref, watch } from 'vue'
  import { NButton, NCheckbox, NTag } from 'naive-ui'
  import type { DiffEntryDto, DiffResultDto } from '../types'
  import { fmtBytes } from '../utils'

  /** 分组渲染一份 diff 结果。可勾选组（默认勾选）通过 `unchecked`
   *  集合与调用方共享勾选状态——传入 store 中的 reactive Set，
   *  本组件直接 add / delete。diff 变化时分组重建（展开 / 分批
   *  渲染状态随之重置）。 */
  const props = defineProps<{
    diff: DiffResultDto
    unchecked: Set<string>
    /** 四个分组的标题。 */
    labels: { added: string; changed: string; same: string; removed: string }
  }>()

  const STEP = 100

  interface Group {
    kind: DiffEntryDto['kind']
    title: string
    tagType: 'success' | 'warning' | 'default' | 'info'
    items: DiffEntryDto[]
    checkable: boolean
    open: boolean
    visible: number
  }

  const groups = ref<Group[]>([])
  watch(
    () => props.diff,
    (diff) => {
      const by: Record<DiffEntryDto['kind'], DiffEntryDto[]> = { added: [], changed: [], same: [], removed: [] }
      for (const e of diff.entries) by[e.kind].push(e)
      groups.value = [
        { kind: 'added', title: props.labels.added, tagType: 'success', items: by.added, checkable: true, open: true, visible: STEP },
        { kind: 'changed', title: props.labels.changed, tagType: 'warning', items: by.changed, checkable: true, open: true, visible: STEP },
        { kind: 'same', title: props.labels.same, tagType: 'default', items: by.same, checkable: false, open: false, visible: STEP },
        { kind: 'removed', title: props.labels.removed, tagType: 'info', items: by.removed, checkable: false, open: false, visible: STEP },
      ]
    },
    { immediate: true },
  )

  const keyOf = (e: DiffEntryDto) => e.name.toLowerCase()
  const shown = (g: Group) => g.items.slice(0, g.visible)
  const hidden = (g: Group) => g.items.length - Math.min(g.visible, g.items.length)

  const allChecked = (g: Group) => g.items.every((e) => !props.unchecked.has(keyOf(e)))
  const allUnchecked = (g: Group) => g.items.every((e) => props.unchecked.has(keyOf(e)))

  function toggle(e: DiffEntryDto, on: boolean) {
    if (on) props.unchecked.delete(keyOf(e))
    else props.unchecked.add(keyOf(e))
  }

  /** 一键设置整组的勾选状态。 */
  function setGroup(g: Group, on: boolean) {
    for (const e of g.items) toggle(e, on)
  }

  /** 大小列：added 只有新大小，changed 展示旧 → 新，removed 只有旧大小。 */
  function sizesOf(e: DiffEntryDto): string {
    const fmt = (n: number | undefined) => (n === undefined ? '？' : fmtBytes(n))
    if (e.kind === 'added') return fmt(e.new_size)
    if (e.kind === 'removed') return fmt(e.old_size)
    if (e.kind === 'changed' && e.old_size !== e.new_size) return `${fmt(e.old_size)} → ${fmt(e.new_size)}`
    return fmt(e.new_size)
  }
</script>

<style scoped>
  .diff-list { display: flex; flex-direction: column; gap: 8px; }
  .diff-group { border: 1px solid var(--border-color, #e0e0e6); border-radius: 6px; overflow: hidden; }
  .diff-group-head {
    display: flex; align-items: center; gap: 8px; padding: 4px 10px;
    background: rgba(128, 128, 128, 0.06);
  }
  .diff-group-count { opacity: 0.75; font-size: 12px; }
  .grow { flex: 1; }
  .diff-group-body { max-height: 220px; overflow-y: auto; padding: 2px 10px; }
  .diff-row {
    display: flex; align-items: center; gap: 8px; padding: 1px 0;
    font-size: 12px; min-height: 22px;
  }
  .diff-row .name {
    flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    direction: rtl; text-align: left;   /* 长路径优先显示文件名尾部 */
  }
  .diff-row .sizes { opacity: 0.7; white-space: nowrap; }
  .diff-empty { padding: 4px 0; opacity: 0.6; font-size: 12px; }
  .load-more {
    display: flex; align-items: center; gap: 8px; padding: 4px 0;
    font-size: 12px; opacity: 0.8;
  }
</style>
