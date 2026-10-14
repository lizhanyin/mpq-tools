<script setup>
import { h } from 'vue'
import { NDataTable, NCheckbox } from 'naive-ui'
import {
  S, rows, navigate, previewFile, log,
  toggleSelected, toggleDirSelected,
  MAX_ROWS,
} from '../store'
import { fmtBytes, baseName, collectKeys } from '../utils'

const columns = [
  {
    title: '✔',
    key: 'check',
    width: 36,
    align: 'center',
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
          title: '双击进入文件夹',
        }, `📁 ${row.node.name}（${fmtBytes(row.node.file_count)} 项）`)
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
  return {
    onDblclick: () => {
      if (row.type === 'dir') {
        navigate(row.path)
      } else if (row.entry.name) {
        previewFile(row.entry.name)
      } else {
        log('未命名文件无法预览（缺少名字无法解密/定位）')
      }
    },
  }
}
</script>

<template>
  <div class="table-scroll">
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
  </div>
</template>
