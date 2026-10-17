/* 表格/网格视图共用的行交互与右键菜单：单击选中、Ctrl+单击多选、
 * Shift+单击范围选、双击打开；右键未选中的行时先把选择切换到该行
 * （资源管理器语义），右键已选中的行保持多选，此时菜单中的批量项
 * （提取/复制/删除 N 个文件）作用于整个选择集。 */

import { reactive, computed, nextTick, watch } from 'vue'
import {
  S, rows, navigate, previewFile, log, entryByKey,
  toggleSelected, toggleDirSelected, setSelectedKeys,
  extractKeys, markDeletions,
} from '../store'
import { collectKeys } from '../utils'
import { toast } from '../toast'

// -- 行的选中语义 ---------------------------------------------------------------

/** 行对应的条目 key：目录行 = 其下全部文件（递归，与勾选列一致），文件行 = 自身。 */
function rowKeys(row) {
  return row.type === 'dir' ? collectKeys(row.node) : [row.entry.key]
}

/** 行的选中态：'full' 全选 / 'partial' 部分选中（仅目录行会出现）/ 'none'。 */
export function rowSelectedState(row) {
  const keys = rowKeys(row)
  if (keys.length === 0) return 'none'
  let sel = 0
  for (const k of keys) if (S.selected.has(k)) sel++
  return sel === keys.length ? 'full' : (sel > 0 ? 'partial' : 'none')
}

// -- 单击 / Ctrl 多选 / Shift 范围 -------------------------------------------------

// Shift 范围选的锚点（rows.list 中的索引）；行列表变化（导航/过滤）后作废。
let anchorIdx = -1
// 双击/回车进入文件夹时置位：列表更新后不沿用原选择（进入前点选目录=递归
// 全选其文件），改为只选中第一项，锚点随之落到第一项。
let enterSelectFirst = false
watch(() => rows.value.list, () => {
  if (enterSelectFirst) {
    enterSelectFirst = false
    const first = rows.value.list[0]
    setSelectedKeys(first ? rowKeys(first) : [])
    anchorIdx = first ? 0 : -1
    return
  }
  anchorIdx = -1
})

/** 把 list[a..b]（含）的全部条目 key 设为选择集。 */
function selectRange(a, b) {
  const list = rows.value.list
  const keys = []
  for (let i = a; i <= b; i++) {
    for (const k of rowKeys(list[i])) keys.push(k) // 不用 spread：大目录可能超参数上限
  }
  setSelectedKeys(keys)
}

export function activateRow(row, ev) {
  const list = rows.value.list
  const idx = list.indexOf(row)
  if (ev.shiftKey && anchorIdx >= 0 && anchorIdx < list.length && idx >= 0) {
    const [a, b] = anchorIdx <= idx ? [anchorIdx, idx] : [idx, anchorIdx]
    selectRange(a, b)
    return
  }
  if (ev.ctrlKey || ev.metaKey) {
    const on = rowSelectedState(row) !== 'full'
    if (row.type === 'dir') toggleDirSelected(row.node, on)
    else toggleSelected(row.entry.key, on)
  } else {
    setSelectedKeys(rowKeys(row))
  }
  anchorIdx = idx
}

/** 方向键导航（网格视图）：把选择移动到 rows.list[idx]。
 * extend = Shift+方向键：从锚点扩选（与 Shift+单击一致）；
 * 普通方向键单选该行并把锚点移过去。返回目标行（越界返回 null）。 */
export function moveCursorTo(idx, extend) {
  const list = rows.value.list
  const row = list[idx]
  if (!row) return null
  if (extend && anchorIdx >= 0 && anchorIdx < list.length) {
    const [a, b] = anchorIdx <= idx ? [anchorIdx, idx] : [idx, anchorIdx]
    selectRange(a, b)
  } else {
    setSelectedKeys(rowKeys(row))
    anchorIdx = idx
  }
  return row
}

/** 双击行：进入目录 / 预览文件。进入目录后只选中第一项（见上方 watcher）。 */
export function openRow(row) {
  if (row.type === 'dir') {
    enterSelectFirst = true
    navigate(row.path)
  } else if (row.entry.name) {
    previewFile(row.entry.name)
  } else {
    log('未命名文件无法预览（缺少名字无法解密/定位）')
  }
}

// -- 右键菜单（表格/网格共用一份状态，两个视图各自挂一个 dropdown 绑定它） ----------

export const ctx = reactive({ show: false, x: 0, y: 0, row: null })

/** 选择集是否恰好等于该行（= 单选了这一行，菜单用单项形态）。 */
function selectionIsExactly(row) {
  const keys = rowKeys(row)
  if (keys.length !== S.selected.size) return false
  for (const k of keys) if (!S.selected.has(k)) return false
  return true
}

export function openContextMenu(row, ev) {
  ev.preventDefault()
  if (rowSelectedState(row) !== 'full') {
    setSelectedKeys(rowKeys(row))
    anchorIdx = rows.value.list.indexOf(row)
  }
  ctx.row = row
  ctx.show = false
  nextTick(() => {
    ctx.x = ev.clientX
    ctx.y = ev.clientY
    ctx.show = true
  })
}

function dirAllSelected(node) {
  const keys = collectKeys(node)
  return keys.length > 0 && keys.every((k) => S.selected.has(k))
}

export const ctxOptions = computed(() => {
  const row = ctx.row
  if (!row) return []
  // 批量形态：右键的行在选中集里，且选中集不止这一行。
  if (rowSelectedState(row) === 'full' && !selectionIsExactly(row)) {
    const n = S.selected.size
    return [
      { label: `提取选中项（${n} 个文件）…`, key: 'batch-extract' },
      { label: `复制名称（${n} 项）`, key: 'batch-copy' },
      { type: 'divider', key: 'd1' },
      { label: `删除选中项（${n} 个文件）`, key: 'batch-delete' },
    ]
  }
  if (row.type === 'dir') {
    return [
      { label: '打开', key: 'enter' },
      { label: dirAllSelected(row.node) ? '取消选中' : '选中', key: 'dir-select' },
      { label: '提取…', key: 'dir-extract' },
      { type: 'divider', key: 'd1' },
      { label: '复制文件夹名', key: 'copy-name' },
      { type: 'divider', key: 'd2' },
      { label: '删除', key: 'dir-delete', disabled: row.node.file_count === 0 },
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

export function onCtxSelect(key) {
  ctx.show = false
  const row = ctx.row
  if (!row) return
  if (key === 'batch-extract') { extractKeys([...S.selected]); return }
  if (key === 'batch-copy') { copyText(selectedNames().join('\n'), `${S.selected.size} 个名称`); return }
  if (key === 'batch-delete') { markDeletions([...S.selected]); return }
  if (row.type === 'dir') {
    if (key === 'enter') openRow(row)
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

/** 批量复制名称：选中条目名（未命名条目用 display）逐行拼接。 */
function selectedNames() {
  const out = []
  for (const k of S.selected) {
    const e = entryByKey.value.get(k)
    if (e) out.push(e.name || e.display)
  }
  return out
}

/** 复制到剪贴板：优先异步 Clipboard API，失败时退回 execCommand（WebView 兼容）。
 * label 为提示用的简短描述（批量复制时避免把全部内容打进 toast）。 */
export function copyText(text, label = text) {
  const fallback = () => {
    const ta = document.createElement('textarea')
    ta.value = text
    ta.style.cssText = 'position:fixed;opacity:0'
    document.body.appendChild(ta)
    ta.select()
    const ok = document.execCommand('copy')
    ta.remove()
    if (ok) toast.success(`已复制：${label}`)
    else toast.error('复制失败')
  }
  if (navigator.clipboard?.writeText) {
    navigator.clipboard.writeText(text)
      .then(() => toast.success(`已复制：${label}`), fallback)
  } else {
    fallback()
  }
}
