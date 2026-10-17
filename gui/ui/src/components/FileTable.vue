<template>
  <div class="table-scroll" @click="onBlankClick">
    <n-data-table
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
  import { h } from 'vue'
  import { NDataTable, NCheckbox, NDropdown } from 'naive-ui'
  import {
    S, rows, toggleSelected, toggleDirSelected, selectNone,
    scopeSelection, toggleSelectAll,
    MAX_ROWS,
  } from '../store'
  import { fmtBytes, baseName, collectKeys } from '../utils'
  import {
    ctx, ctxOptions, onCtxSelect,
    activateRow, openRow, openContextMenu, rowSelectedState,
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
        activateRow(row, ev)
      },
      onDblclick: () => openRow(row),
      onContextmenu: (ev) => openContextMenu(row, ev),
    }
  }

  /** 点击行/表头以外的空白处清空选择。 */
  function onBlankClick(ev) {
    if (ev.target?.closest?.('.n-data-table-tr')) return
    selectNone()
  }
</script>
