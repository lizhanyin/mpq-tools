<template>
  <div class="table-scroll" @click="onBlankClick">
    <n-data-table
      ref="tableRef"
      size="small"
      striped
      flex-height
      virtual-scroll
      :bordered="false"
      :single-line="false"
      :columns="columns"
      :data="rows.list"
      :row-key="rowKey"
      :row-props="rowProps"
    />
    <div v-if="rows.truncated" class="table-note">
      （结果过多，仅显示前 {{ fmtBytes(MAX_ROWS) }} 项，请用过滤缩小范围）
    </div>
    <n-dropdown trigger="manual" placement="bottom-start"
                :show="ctx.show" :x="ctx.x" :y="ctx.y"
                :options="ctxOptions"
                @select="onCtxSelect" @clickoutside="ctx.show = false" />
  </div>
</template>

<script setup>
  import { h, ref, watch, onMounted, onBeforeUnmount } from 'vue'
  import { NDataTable, NCheckbox, NDropdown } from 'naive-ui'
  import {
    S, rows, toggleSelected, toggleDirSelected, selectNone,
    scopeSelection, toggleSelectAll, anyModalOpen,
    MAX_ROWS,
  } from '../store'
  import { fmtBytes, baseName, collectKeys } from '../utils'
  import {
    ctx, ctxOptions, onCtxSelect,
    activateRow, openRow, openContextMenu, moveCursorTo, rowSelectedState,
  } from './entryActions'

  const columns = [
    {
      key: 'check',
      width: 36,
      align: 'center',
      // 表头：当前视图的全选 / 取消全选（带半选态）。
      title: () => h(NCheckbox, {
        size: 'small',
        checked: scopeSelection.value.checked,
        indeterminate: scopeSelection.value.indeterminate,
        disabled: scopeSelection.value.disabled,
        title: '全选 / 取消全选（当前视图）',
        'onUpdate:checked': (v) => toggleSelectAll(v),
      }),
      render(row) {
        if (row.type === 'dir') {
          const keys = collectKeys(row.node)
          const sel = keys.reduce((n, k) => n + (S.selected.has(k) ? 1 : 0), 0)
          return h(NCheckbox, {
            size: 'small',
            checked: keys.length > 0 && sel === keys.length,
            indeterminate: sel > 0 && sel < keys.length,
            title: '选中此文件夹下的全部文件',
            'onUpdate:checked': (v) => toggleDirSelected(row.node, v),
          })
        }
        return h(NCheckbox, {
          size: 'small',
          checked: S.selected.has(row.entry.key),
          'onUpdate:checked': (v) => toggleSelected(row.entry.key, v),
        })
      },
    },
    {
      title: '名称',
      key: 'name',
      minWidth: 160,
      ellipsis: { tooltip: true },
      render(row) {
        if (row.type === 'dir') {
          return h('span', {
            class: 'cdir',
            title: `双击进入文件夹（共 ${fmtBytes(row.node.file_count)} 个文件）`,
          }, `📁 ${row.node.name}`)
        }
        const e = row.entry
        const named = !!e.name
        const label = row.showFull ? (named ? e.name : e.display)
                                  : (named ? baseName(e.name) : e.display)
        if (named) return h('span', { class: 'cname' }, label)
        return h('span', {
          class: 'cname unnamed',
          title: '未命名文件无法预览（缺少名字无法解密/定位）',
        }, `⚠ ${label}`)
      },
    },
    {
      title: '大小',
      key: 'size',
      align: 'right',
      width: 90,
      render(row) {
        return fmtBytes(row.type === 'dir' ? row.node.total_size : row.entry.size)
      },
    },
    {
      title: '压缩后',
      key: 'csize',
      align: 'right',
      width: 90,
      render(row) {
        return fmtBytes(row.type === 'dir' ? row.node.total_csize : row.entry.csize)
      },
    },
    {
      title: '属性',
      key: 'flags',
      width: 64,
      className: 'cname',
      render(row) {
        return row.type === 'file' ? row.entry.flags : ''
      },
    },
    {
      title: '修改时间(UTC)',
      key: 'mtime',
      width: 140,
      render(row) {
        return (row.type === 'file' && row.entry.mtime) ? row.entry.mtime : '—'
      },
    },
    {
      title: '地区',
      key: 'locale',
      align: 'right',
      width: 52,
      render(row) {
        return row.type === 'file' ? row.entry.locale : ''
      },
    },
    {
      title: '来源',
      key: 'source',
      width: 52,
      className: 'cname',
      render(row) {
        return row.type === 'file' ? (row.entry.source === 'classic' ? 'blk' : 'bet') : ''
      },
    },
  ]

  function rowKey(row) {
    return row.type === 'dir' ? `d:${row.path.join('\\')}` : row.entry.key
  }

  function rowProps(row) {
    const st = rowSelectedState(row)
    return {
      class: st === 'full' ? 'row-sel' : st === 'partial' ? 'row-sel-partial' : undefined,
      onClick: (ev) => {
        if (ev.target?.closest?.('.n-checkbox')) return // 勾选列自己处理，不当作行点击
        cursor.value = rows.value.list.indexOf(row)
        activateRow(row, ev)
      },
      onDblclick: () => onRowOpen(row),
      onContextmenu: (ev) => onRowMenu(row, ev),
    }
  }

  /** 点击行/表头以外的空白处清空选择。 */
  function onBlankClick(ev) {
    if (ev.target?.closest?.('.n-data-table-tr')) return
    selectNone()
  }

  // -- 方向键导航（↑↓ 移动选择，Shift 扩选，Enter 打开） ----------------------------

  const tableRef = ref(null)
  const cursor = ref(-1) // 光标在 rows.list 中的索引；-1 = 尚未定位

  // 行列表变化（导航/过滤）后光标作废，方向键重新从头开始；
  // 双击/回车进入文件夹除外：光标落到第一项，方向键从它继续。
  let dirEnter = false
  watch(() => rows.value.list, () => {
    cursor.value = dirEnter ? 0 : -1
    dirEnter = false
  })

  // 预览窗切图时把光标与滚动定位到该行（与网格视图共用 S.gridCursor）。
  watch(() => S.gridCursor, (idx) => {
    if (idx < 0 || idx >= rows.value.list.length) return
    cursor.value = idx
    tableRef.value?.scrollTo({ index: idx })
  })

  /** 双击 / 回车打开：进入文件夹时光标落到第一项。 */
  function onRowOpen(row) {
    dirEnter = row.type === 'dir'
    openRow(row)
  }

  /** 右键行时把光标同步到该行，方向键从它继续。 */
  function onRowMenu(row, ev) {
    cursor.value = rows.value.list.indexOf(row)
    openContextMenu(row, ev)
  }

  function onKey(ev) {
    if (ctx.show) return
    // 预览窗开着不算「有弹窗」：焦点区在列表时方向键归表格，
    // 预览通过选择联动跟随；焦点区在预览时仍让位给预览切图。
    if (anyModalOpen.value && !(S.preview && S.focusZone === 'list')) return
    const t = ev.target
    if (t?.closest?.('input, textarea, [contenteditable="true"]')) return
    const list = rows.value.list
    if (list.length === 0) return
    if (ev.key === 'Enter') {
      if (cursor.value >= 0) { ev.preventDefault(); onRowOpen(list[cursor.value]) }
      return
    }
    const cur = cursor.value
    let next = -1
    if (ev.key === 'ArrowDown') next = cur < 0 ? 0 : (cur + 1 < list.length ? cur + 1 : -1)
    else if (ev.key === 'ArrowUp') next = cur < 0 ? list.length - 1 : (cur - 1 >= 0 ? cur - 1 : -1)
    else return
    if (next < 0) return // 已在列表边缘（首行的↑/末行的↓），保留原生行为
    ev.preventDefault()
    cursor.value = next
    moveCursorTo(next, ev.shiftKey)
    // scrollTo({index}) 仅当目标行未完全可见时才滚动（vueuc 内部已判断）。
    tableRef.value?.scrollTo({ index: next })
  }

  onMounted(() => window.addEventListener('keydown', onKey))
  onBeforeUnmount(() => window.removeEventListener('keydown', onKey))
</script>
