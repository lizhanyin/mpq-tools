<template>
  <div class="table-scroll">
    <n-data-table
      size="small"
      striped
      flex-height
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
                :show="ctxShow" :x="ctxX" :y="ctxY"
                :options="ctxOptions"
                @select="onCtxSelect" @clickoutside="ctxShow = false" />
  </div>
</template>

<script setup>
  import { h, ref, computed, nextTick } from 'vue'
  import { NDataTable, NCheckbox, NDropdown } from 'naive-ui'
  import {
    S, rows, navigate, previewFile, log,
    toggleSelected, toggleDirSelected,
    scopeSelection, toggleSelectAll,
    extractKeys, markDeletions,
    MAX_ROWS,
  } from '../store'
  import { fmtBytes, baseName, collectKeys } from '../utils'
  import { toast } from '../toast'

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
      onContextmenu: (ev) => onContextMenu(row, ev),
    }
  }

  // -- 右键菜单 -------------------------------------------------------------------

  const ctxShow = ref(false)
  const ctxX = ref(0)
  const ctxY = ref(0)
  const ctxRow = ref(null)

  function onContextMenu(row, ev) {
    ev.preventDefault()
    ctxRow.value = row
    ctxShow.value = false
    nextTick(() => {
      ctxX.value = ev.clientX
      ctxY.value = ev.clientY
      ctxShow.value = true
    })
  }

  function dirAllSelected(node) {
    const keys = collectKeys(node)
    return keys.length > 0 && keys.every((k) => S.selected.has(k))
  }

  const ctxOptions = computed(() => {
    const row = ctxRow.value
    if (!row) return []
    if (row.type === 'dir') {
      return [
        { label: '打开', key: 'enter' },
        {
          label: dirAllSelected(row.node) ? '取消选中' : '选中',
          key: 'dir-select',
        },
        { label: '提取…', key: 'dir-extract' },
        { type: 'divider', key: 'd1' },
        { label: '复制文件夹名', key: 'copy-name' },
        { type: 'divider', key: 'd2' },
        {
          label: '删除',
          key: 'dir-delete',
          disabled: row.node.file_count === 0,
        },
      ]
    }
    const e = row.entry
    return [
      { label: '预览', key: 'preview', disabled: !e.name },
      { label: '提取…', key: 'extract' },
      { label: S.selected.has(e.key) ? '取消选中' : '选中', key: 'toggle' },
      { label: '复制文件名', key: 'copy-name' },
      { type: 'divider', key: 'd1' },
      { label: '删除', key: 'delete' },
    ]
  })

  function onCtxSelect(key) {
    ctxShow.value = false
    const row = ctxRow.value
    if (!row) return
    if (row.type === 'dir') {
      if (key === 'enter') navigate(row.path)
      else if (key === 'dir-select') toggleDirSelected(row.node, !dirAllSelected(row.node))
      else if (key === 'dir-extract') extractKeys(collectKeys(row.node))
      else if (key === 'copy-name') copyText(row.node.name)
      else if (key === 'dir-delete') markDeletions(collectKeys(row.node))
      return
    }
    const e = row.entry
    if (key === 'preview') previewFile(e.name)
    else if (key === 'extract') extractKeys([e.key])
    else if (key === 'toggle') toggleSelected(e.key, !S.selected.has(e.key))
    else if (key === 'copy-name') copyText(e.name || e.display)
    else if (key === 'delete') markDeletions([e.key])
  }

  /** 复制到剪贴板：优先异步 Clipboard API，失败时退回 execCommand（WebView 兼容）。 */
  function copyText(text) {
    const fallback = () => {
      const ta = document.createElement('textarea')
      ta.value = text
      ta.style.cssText = 'position:fixed;opacity:0'
      document.body.appendChild(ta)
      ta.select()
      const ok = document.execCommand('copy')
      ta.remove()
      if (ok) toast.success(`已复制：${text}`)
      else toast.error('复制失败')
    }
    if (navigator.clipboard?.writeText) {
      navigator.clipboard.writeText(text)
        .then(() => toast.success(`已复制：${text}`), fallback)
    } else {
      fallback()
    }
  }
</script>
