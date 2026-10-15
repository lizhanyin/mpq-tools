/* 全局状态与动作：从原 app.js 迁移，界面组件只读状态并调用这里的动作。 */

import { reactive, computed } from 'vue'
import * as ipc from './ipc'
import * as U from './utils'

export const ARCHIVE_FILTERS = [
  { name: 'MPQ 归档', extensions: ['mpq', 'w3x', 'w3m', 'w3n', 'scx', 'sc2map', 's2ma', 'rep'] },
  { name: '所有文件', extensions: ['*'] },
]
export const SYSTEM_DIR = '(系统文件)'
export const UNNAMED_DIR = '(未命名)'
export const MAX_ROWS = 5000
export const FONT_MIN = 10, FONT_MAX = 28, FONT_DEFAULT = 13

export const S = reactive({
  /** 最近一次成功打开的 ArchiveInfoDto（未打开时为 null）。 */
  info: null,
  /** 当前视图背后的文件路径；解析失败时也保留（修复流程作用在这些字节上）。 */
  openPath: null,
  cwd: [],               // 路径段；[] = 归档根
  expanded: [],          // 展开的文件夹 key（小写 `\` 连接）
  filter: '',
  appliedFilter: '',     // 120ms 防抖后的过滤词
  selected: new Set(),   // 条目 key "classic:12" / "bet:3"
  preview: null,         // { name, text } — 浮动预览窗
  repair: null,          // { issues, applied, note }
  modify: null,          // { additions, deletions, reencrypt, applied, note }
  create: null,          // { version, hashSize, note } — 新建归档弹窗
  convert: null,         // { target, applied, note } — 版本转换弹窗
  settingsOpen: false,
  logs: [],
  fontSize: FONT_DEFAULT,
  dropping: null,        // 拖拽悬停时的 { count }
})

// ---------------------------------------------------------------------------
// 派生状态

export const needle = computed(() => S.appliedFilter.trim().toLowerCase())
export const searching = computed(() => needle.value !== '')
export const modifyFrozen = computed(() => !!S.modify && !!S.modify.applied)

/** key → entry 的扁平映射（按需构建，info 变化时自动重建）。 */
export const entryByKey = computed(() => {
  const map = new Map()
  if (S.info) {
    ;(function walk(node) {
      for (const d of node.dirs) walk(d)
      for (const f of node.files) map.set(f.key, f)
    })(S.info.tree)
  }
  return map
})

export const searchResults = computed(() =>
  (searching.value && S.info) ? U.searchTree(S.info.tree, needle.value) : [])

/** 表格当前行：{type:'dir', node, path} | {type:'file', entry, showFull}。 */
export const rows = computed(() => {
  const out = []
  if (!S.info) return { list: out, truncated: false }
  if (searching.value) {
    for (const e of searchResults.value) out.push({ type: 'file', entry: e, showFull: true })
  } else {
    const node = U.nodeAt(S.info.tree, S.cwd)
    if (node) {
      for (const d of node.dirs) out.push({ type: 'dir', node: d, path: [...S.cwd, d.name] })
      for (const f of node.files) out.push({ type: 'file', entry: f, showFull: false })
    }
  }
  return { list: out.slice(0, MAX_ROWS), truncated: out.length > MAX_ROWS }
})

// ---------------------------------------------------------------------------
// 过滤（防抖）与字号

let filterTimer = null
export function setFilter(v) {
  S.filter = v
  clearTimeout(filterTimer)
  filterTimer = setTimeout(() => { S.appliedFilter = v }, 120)
}

function loadFontSize() {
  const v = parseFloat(localStorage.getItem('font_size'))
  return Number.isFinite(v) ? Math.min(FONT_MAX, Math.max(FONT_MIN, v)) : FONT_DEFAULT
}

export function setFontSize(v) {
  const clamped = Math.min(FONT_MAX, Math.max(FONT_MIN, v))
  S.fontSize = clamped
  document.documentElement.style.fontSize = `${clamped}px`
  localStorage.setItem('font_size', String(clamped))
}

export function initStore() {
  S.fontSize = loadFontSize()
  document.documentElement.style.fontSize = `${S.fontSize}px`
}

// ---------------------------------------------------------------------------
// 日志

export function log(msg) {
  S.logs.push(msg)
}
export function clearLog() {
  S.logs.length = 0
}

// ---------------------------------------------------------------------------
// 打开 / 重载 / 拖放

function applyOpenResult(res, pathIfLoaded, { keepNav = false } = {}) {
  if (res.note) log(res.note)
  if (res.ok) {
    S.info = res.info
    S.openPath = res.info.path
    if (!keepNav) {
      S.cwd = []
      S.expanded = []
      S.selected.clear()
    }
    log(`已打开 ${res.info.path}（${U.fmtBytes(res.info.entry_count)} 个条目）`)
    for (const w of res.info.warnings) log(`warning: ${w}`)
  } else {
    // 后端仍记录了原始字节（只是解析失败），修复流程依然可用。
    if (pathIfLoaded !== undefined) S.openPath = pathIfLoaded
    S.info = null
    if (!keepNav) {
      S.cwd = []
      S.expanded = []
      S.selected.clear()
    }
    log(`打开失败：${res.error}`)
    log('可以尝试「工具 → 修复头信息」。')
  }
}

export async function openArchiveDialog() {
  const path = await ipc.dialogOpen({ title: '打开 MPQ 归档', filters: ARCHIVE_FILTERS })
  if (!path) return
  await openArchivePath(path)
}

export async function openArchivePath(path) {
  try {
    const res = await ipc.invoke('open_archive', { path })
    applyOpenResult(res, path)
  } catch (e) {
    log(String(e)) // e.g. 读取失败：…
  }
}

export async function reloadArchive() {
  if (!S.openPath) return
  const hadPending = !!S.modify && !S.modify.applied &&
    (S.modify.additions.length > 0 || S.modify.deletions.length > 0)
  const cwd = S.cwd, expanded = S.expanded, filter = S.filter
  try {
    const res = await ipc.invoke('reload_archive')
    applyOpenResult(res, S.openPath, { keepNav: true })
    S.cwd = cwd
    S.expanded = expanded
    S.filter = filter
    S.selected.clear()
    S.preview = null
    if (hadPending) S.modify = null
    if (res.ok) {
      log('（已从磁盘刷新）')
      if (hadPending) log('（刷新已清空未保存的「修改归档」待办）')
    }
  } catch (e) {
    log(String(e)) // 刷新失败：…（当前仍显示已载入的内容）
  }
}

export async function handleDrop(paths) {
  if (S.info) {
    log(`（通过拖拽导入 ${U.fmtBytes(paths.length)} 个文件）`)
    await queueImports(paths)
    return
  }
  try {
    const res = await ipc.invoke('open_archive', { path: paths[0] })
    applyOpenResult(res, paths[0])
    if (res.ok) {
      log('（通过拖拽打开）')
      if (paths.length > 1) {
        log(`（忽略了另外 ${U.fmtBytes(paths.length - 1)} 个拖入的文件，一次只能打开一个归档）`)
      }
    }
  } catch (e) {
    log(String(e))
  }
}

// ---------------------------------------------------------------------------
// 预览 / 提取

export async function previewFile(name) {
  try {
    const p = await ipc.invoke('preview_file', { name })
    const note = p.truncated
      ? `（仅显示前 256 字节，共 ${U.fmtBytes(p.size)} 字节）`
      : `（${U.fmtBytes(p.size)} 字节）`
    S.preview = { name: `${name} ${note}`, text: p.text }
  } catch (e) {
    log(`预览失败 ${name}: ${e}`)
  }
}

export function closePreview() {
  S.preview = null
}

async function pickFolder(title) {
  return ipc.dialogOpen({ title, directory: true })
}

export async function extractSelected() {
  if (S.selected.size === 0) { log('未选择任何文件。'); return }
  const dir = await pickFolder('选择输出目录')
  if (!dir) return
  await runExtract([...S.selected], dir)
}

export async function extractAll() {
  const dir = await pickFolder('选择输出目录')
  if (!dir) return
  await runExtract([], dir)
}

async function runExtract(keys, dir) {
  try {
    const r = await ipc.invoke('extract', { keys, dir })
    for (const m of r.messages) log(m)
    log(`提取完成：${U.fmtBytes(r.ok)} 成功，${U.fmtBytes(r.failed)} 失败 → ${dir}`)
  } catch (e) {
    log(String(e))
  }
}

// ---------------------------------------------------------------------------
// 勾选

export function toggleSelected(key, on) {
  if (on) S.selected.add(key)
  else S.selected.delete(key)
}

export function toggleDirSelected(node, on) {
  for (const k of U.collectKeys(node)) {
    if (on) S.selected.add(k)
    else S.selected.delete(k)
  }
}

export function selectAll() {
  for (const k of scopeKeys()) S.selected.add(k)
}

export function selectNone() {
  S.selected.clear()
}

/** 当前视图的选择范围（与旧「全选」一致）：搜索态=全部命中；目录态=当前文件夹递归。 */
export function scopeKeys() {
  if (searching.value) return searchResults.value.map((f) => f.key)
  if (!S.info) return []
  const node = U.nodeAt(S.info.tree, S.cwd)
  return node ? U.collectKeys(node) : []
}

/** 表头全选复选框的状态。 */
export const scopeSelection = computed(() => {
  const keys = scopeKeys()
  if (keys.length === 0) return { checked: false, indeterminate: false, disabled: true }
  const sel = keys.reduce((n, k) => n + (S.selected.has(k) ? 1 : 0), 0)
  return {
    checked: sel === keys.length,
    indeterminate: sel > 0 && sel < keys.length,
    disabled: false,
  }
})

export function toggleSelectAll(on) {
  if (on) selectAll()
  else selectNone()
}

// ---------------------------------------------------------------------------
// 导入 / 删除（「修改归档」窗口）

function defaultModify() {
  return { additions: [], deletions: [], reencrypt: true, applied: null, note: '' }
}

export async function importFilesDialog() {
  const paths = await ipc.dialogOpen({ title: '选择要导入的文件', multiple: true })
  if (!paths) return
  await queueImports(Array.isArray(paths) ? paths : [paths])
}

export async function queueImports(paths) {
  if (!paths.length) return
  if (modifyFrozen.value) {
    log('已有应用过的修改待保存或关闭，请先处理「修改归档」窗口。')
    return
  }
  S.modify ??= defaultModify()

  let stats
  try {
    stats = await ipc.invoke('stat_imports', { paths })
  } catch (e) {
    log(String(e))
    return
  }

  let queued = 0
  paths.forEach((path, i) => {
    const st = stats[i]
    if (!st) return
    if (st.error) {
      log(`读取失败 ${path}: ${st.error}`)
      return
    }
    // 同名后导入的取代先排队的；替换已有条目或已排队项。
    const dupIdx = S.modify.additions.findIndex((a) => a.name.toLowerCase() === st.name.toLowerCase())
    const replaces = st.replaces || dupIdx >= 0
    log(`待导入 ${st.name}（${U.fmtBytes(st.size)} 字节，${replaces ? '替换已有文件' : '新增'}）`)
    const item = { name: st.name, path, size: st.size, replaces }
    if (dupIdx >= 0) S.modify.additions[dupIdx] = item
    else S.modify.additions.push(item)
    queued++
  })
  if (queued > 0) {
    log(`已加入导入队列（${U.fmtBytes(queued)} 项），在「修改归档」窗口中应用。`)
  }
}

export function deleteSelected() {
  if (S.selected.size === 0) { log('未选择任何文件。'); return }
  if (modifyFrozen.value) return
  S.modify ??= defaultModify()

  // 与 egui 一致：按 (source, block_index) 排序 —— classic 在前、下标升序。
  const sorted = [...S.selected].sort((a, b) => {
    const [sa, ia] = a.split(':'), [sb, ib] = b.split(':')
    return sa === sb ? (+ia) - (+ib) : sa.localeCompare(sb)
  })
  let queued = 0
  for (const key of sorted) {
    if (S.modify.deletions.some((d) => d.key === key)) continue
    const entry = entryByKey.value.get(key)
    if (!entry) continue
    S.modify.deletions.push({
      display: entry.name || entry.display,
      name: entry.name || null,
      key,
    })
    queued++
  }
  if (queued > 0) {
    log(`已标记 ${U.fmtBytes(queued)} 项待删除，在「修改归档」窗口中应用。`)
  }
}

export async function applyModify() {
  const m = S.modify
  try {
    const r = await ipc.invoke('apply_modify', {
      additions: m.additions.map((a) => ({ name: a.name, path: a.path })),
      deletions: m.deletions.map((d) => ({ name: d.name, key: d.name ? null : d.key })),
      reencrypt: m.reencrypt,
    })
    m.applied = r.note
    m.note = r.note
  } catch (e) {
    m.note = `修改失败：${e}`
  }
}

export async function saveModified() {
  const suggested = U.basenameOf(S.openPath || '')
  const out = await ipc.dialogSave({ title: '保存修改后的归档', defaultPath: suggested })
  if (!out) return
  try {
    const res = await ipc.invoke('save_modified', { path: out })
    log(`修改已保存到 ${out}`)
    S.modify = null
    ipc.invoke('discard_modify').catch(() => {})
    applyOpenResult(res)
  } catch (e) {
    S.modify.note = String(e) // 保存失败：…
  }
}

export async function openModified() {
  try {
    const res = await ipc.invoke('open_modified')
    S.modify = null
    ipc.invoke('discard_modify').catch(() => {})
    applyOpenResult(res)
  } catch (e) {
    log(String(e))
  }
}

export function closeModify() {
  S.modify = null
  ipc.invoke('discard_modify').catch(() => {})
}

// ---------------------------------------------------------------------------
// 新建归档（「新建归档」窗口）

function defaultCreate() {
  return { version: 1, hashSize: 1024, note: '' }
}

export function startCreate() {
  S.create = defaultCreate()
}

export async function createArchive() {
  const c = S.create
  const out = await ipc.dialogSave({
    title: '新建 MPQ 归档',
    defaultPath: '新建归档.mpq',
    filters: ARCHIVE_FILTERS,
  })
  if (!out) return
  try {
    const res = await ipc.invoke('create_archive', {
      path: out,
      version: c.version,
      hash_table_size: c.hashSize,
    })
    S.create = null
    applyOpenResult(res)
  } catch (e) {
    c.note = String(e) // 创建失败：… / 保存失败：…
  }
}

export function closeCreate() {
  S.create = null
}

// ---------------------------------------------------------------------------
// 版本转换（「版本转换」窗口）

function defaultConvertTarget(from) {
  return from === 4 ? 3 : 4
}

export function startConvert() {
  if (!S.info) { log('请先打开一个可解析的归档。'); return }
  S.convert = { target: defaultConvertTarget(S.info.version), applied: null, note: '' }
}

export async function applyConvert() {
  const c = S.convert
  try {
    const r = await ipc.invoke('apply_convert', { target: c.target })
    c.note = r.note
    c.applied = r.applied ? r.note : null
  } catch (e) {
    c.note = `转换失败：${e}`
  }
}

export async function saveConverted() {
  const base = basenameOf(S.openPath || '')
  const m = base.match(/^(.*?)(\.[^.]*)?$/)
  const suggested = `${m[1]}.v${S.convert.target}${m[2] || '.mpq'}`
  const out = await ipc.dialogSave({ title: '保存转换后的归档', defaultPath: suggested })
  if (!out) return
  try {
    const res = await ipc.invoke('save_converted', { path: out })
    log(`转换结果已保存到 ${out}`)
    S.convert = null
    ipc.invoke('discard_convert').catch(() => {})
    applyOpenResult(res)
  } catch (e) {
    S.convert.note = String(e) // 保存失败：…
  }
}

export async function openConverted() {
  try {
    const res = await ipc.invoke('open_converted')
    S.convert = null
    ipc.invoke('discard_convert').catch(() => {})
    applyOpenResult(res)
  } catch (e) {
    log(String(e))
  }
}

export function closeConvert() {
  S.convert = null
  ipc.invoke('discard_convert').catch(() => {})
}

// ---------------------------------------------------------------------------
// 修复（「修复头信息」窗口）

export async function startRepair() {
  if (!S.openPath) { log('请先打开一个归档。'); return }
  try {
    const r = await ipc.invoke('diagnose_archive')
    S.repair = {
      issues: r.issues,
      applied: false,
      note: r.issues.length === 0 ? '未发现头信息问题。' : '',
    }
  } catch (e) {
    log(`诊断失败：${e}`)
  }
}

export async function applyRepair() {
  try {
    const r = await ipc.invoke('apply_repair')
    S.repair.applied = true
    S.repair.note = r.note
  } catch (e) {
    S.repair.note = `修复失败：${e}`
  }
}

export async function saveRepaired() {
  const suggested = U.repairedName(S.openPath || 'archive.mpq')
  const out = await ipc.dialogSave({ title: '保存修复后的归档', defaultPath: suggested })
  if (!out) return
  try {
    const r = await ipc.invoke('save_repaired', { path: out })
    S.repair.note = r.note
    log(`修复副本已保存到 ${out}`)
  } catch (e) {
    S.repair.note = String(e) // 保存失败：…
  }
}

export async function openRepaired() {
  try {
    const res = await ipc.invoke('open_repaired')
    S.repair = null
    ipc.invoke('discard_repair').catch(() => {})
    applyOpenResult(res)
  } catch (e) {
    log(String(e))
  }
}

export function closeRepair() {
  S.repair = null
  ipc.invoke('discard_repair').catch(() => {})
}

// ---------------------------------------------------------------------------
// 树导航

export function toggleExpanded(key) {
  const i = S.expanded.indexOf(key)
  if (i >= 0) S.expanded.splice(i, 1)
  else S.expanded.push(key)
}

export function navigate(path) {
  S.cwd = path
}
