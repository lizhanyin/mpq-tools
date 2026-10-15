/* 全局状态与动作：从原 app.js 迁移，界面组件只读状态并调用这里的动作。 */

import { reactive, computed } from 'vue'
import * as ipc from './ipc'
import * as U from './utils'
import { toast } from './toast'

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
  repair: null,          // { issues, applied }
  modify: null,          // { additions, deletions, reencrypt, busy }
  create: null,          // { version, hashSize } — 新建归档弹窗
  convert: null,         // { target, running } — 版本转换弹窗
  pw: null,              // 密码弹窗：{ mode: 'unlock'|'set'|'change'|'remove', … }
  settingsOpen: false,
  logs: [],
  fontSize: FONT_DEFAULT,
  dropping: null,        // 拖拽悬停时的 { count }
})

// ---------------------------------------------------------------------------
// 派生状态

export const needle = computed(() => S.appliedFilter.trim().toLowerCase())
export const searching = computed(() => needle.value !== '')

/** 当前归档是否带 (password) 密码保护（自定义 v4 加密扩展）。 */
export const protectedArchive = computed(() => !!S.info?.protected)
/** 受保护且尚未解锁（读取内容需要先输入密码）。 */
export const lockedArchive = computed(() => !!S.info?.locked)

/** 是否有任意弹窗/浮窗打开（此时拖拽导入被忽略，见 App.vue）。 */
export const anyModalOpen = computed(() =>
  !!(S.modify || S.create || S.convert || S.pw || S.repair || S.preview || S.settingsOpen))

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
    // 受密码保护且尚未解锁：立即弹密码框（可取消，文件名仍可浏览）。
    if (res.info.protected && res.info.locked && !S.pw) {
      log('（归档受密码保护，输入密码后才能读取文件内容）')
      startPwModal('unlock')
    }
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
    toast.error(`打开失败：${res.error}`)
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
    log(String(e)) // 例如：读取失败：…
    toast.error(String(e))
  }
}

export async function reloadArchive() {
  if (!S.openPath) return
  const hadPending = !!S.modify &&
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
    toast.error(String(e))
  }
}

/** 文件菜单「关闭归档」：清空前端状态，并让后端释放归档字节、
 *  主密钥与修复 / 修改 / 转换的中间结果。未保存的「修改归档」
 *  待办一并丢弃。 */
export function closeArchive() {
  if (!S.openPath) return
  const hadPending = !!S.modify &&
    (S.modify.additions.length > 0 || S.modify.deletions.length > 0)
  log(`已关闭 ${S.openPath}`)
  if (hadPending) log('（关闭归档清空了未保存的「修改归档」待办）')
  S.info = null
  S.openPath = null
  S.cwd = []
  S.expanded = []
  S.selected.clear()
  S.filter = ''
  S.appliedFilter = ''
  S.preview = null
  S.modify = null
  S.convert = null
  S.repair = null
  S.pw = null
  ipc.invoke('close_archive').catch(() => {})
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
    toast.error(String(e))
  }
}

// ---------------------------------------------------------------------------
// 预览 / 提取

export async function previewFile(name) {
  try {
    const p = await ipc.invoke('preview_file', { name })
    S.preview = {
      name,                         // 归档内条目名（编辑保存时也用它）
      text: p.text,
      size: p.size,
      encoding: p.encoding || 'UTF-8',
      truncated: !!p.truncated,
      binary: !!p.binary,
      // 截断（后半未加载）或二进制回退时只读，避免保存出损坏内容。
      editable: !p.truncated && !p.binary,
      dirty: false,                 // Monaco 里是否改过
      saving: false,
      armed: false,                 // 有未保存修改时关闭需二次确认
    }
  } catch (e) {
    log(`预览失败 ${name}: ${e}`)
    toast.error(`预览失败 ${name}: ${e}`)
  }
}

export function closePreview() {
  S.preview = null
}

/** 预览窗「保存」：按原编码写临时文件（后端），再把该文件排队到
 * 「修改归档 → 待导入」，由用户在弹窗中确认应用/写盘。 */
export async function savePreviewEdit(text) {
  const p = S.preview
  if (!p || !p.editable || p.saving) return
  p.saving = true
  try {
    const st = await ipc.invoke('stage_edited_file', {
      name: p.name, text, encoding: p.encoding,
    })
    S.modify ??= defaultModify()
    const dupIdx = S.modify.additions.findIndex((a) => a.name.toLowerCase() === p.name.toLowerCase())
    const item = {
      name: p.name,
      path: st.path,
      size: st.size,
      replaces: st.replaces || dupIdx >= 0,
    }
    if (dupIdx >= 0) S.modify.additions[dupIdx] = item
    else S.modify.additions.push(item)
    p.dirty = false
    p.armed = false
    log(`已保存编辑 ${p.name}（${U.fmtBytes(st.size)} 字节，${item.replaces ? '替换已有文件' : '新增'}），在「修改归档」窗口中应用`)
    toast.success(`已保存编辑：${p.name}（在「修改归档」中确认写入）`)
  } catch (e) {
    log(String(e))
    toast.error(String(e))
  }
  p.saving = false
}

async function pickFolder(title) {
  return ipc.dialogOpen({ title, directory: true })
}

export async function extractSelected() {
  await extractKeys([...S.selected])
}

export async function extractAll() {
  const dir = await pickFolder('选择输出目录')
  if (!dir) return
  await runExtract([], dir)
}

/** 提取指定的一组条目（右键菜单「提取该文件」等入口）。 */
export async function extractKeys(keys) {
  if (keys.length === 0) { toast.info('未选择任何文件。'); return }
  const dir = await pickFolder('选择输出目录')
  if (!dir) return
  await runExtract(keys, dir)
}

async function runExtract(keys, dir) {
  try {
    const r = await ipc.invoke('extract', { keys, dir })
    for (const m of r.messages) log(m)
    log(`提取完成：${U.fmtBytes(r.ok)} 成功，${U.fmtBytes(r.failed)} 失败 → ${dir}`)
    if (r.failed > 0) toast.error(`提取完成：${U.fmtBytes(r.ok)} 成功，${U.fmtBytes(r.failed)} 失败`)
    else toast.success(`提取完成：${U.fmtBytes(r.ok)} 个文件 → ${dir}`)
  } catch (e) {
    log(String(e))
    toast.error(String(e))
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
  return { additions: [], deletions: [], reencrypt: true, saveAs: false, busy: false }
}

export async function importFilesDialog() {
  const paths = await ipc.dialogOpen({ title: '选择要导入的文件', multiple: true })
  if (!paths) return
  await queueImports(Array.isArray(paths) ? paths : [paths])
}

export async function queueImports(paths) {
  if (!paths.length) return
  S.modify ??= defaultModify()

  let stats
  try {
    stats = await ipc.invoke('stat_imports', { paths })
  } catch (e) {
    log(String(e))
    toast.error(String(e))
    return
  }

  paths.forEach((path, i) => {
    const st = stats[i]
    if (!st) return
    if (st.error) {
      log(`读取失败 ${path}: ${st.error}`)
      toast.error(`读取失败 ${path}: ${st.error}`)
      return
    }
    // 同名后导入的取代先排队的；替换已有条目或已排队项。
    const dupIdx = S.modify.additions.findIndex((a) => a.name.toLowerCase() === st.name.toLowerCase())
    const replaces = st.replaces || dupIdx >= 0
    log(`待导入 ${st.name}（${U.fmtBytes(st.size)} 字节，${replaces ? '替换已有文件' : '新增'}）`)
    const item = { name: st.name, path, size: st.size, replaces }
    if (dupIdx >= 0) S.modify.additions[dupIdx] = item
    else S.modify.additions.push(item)
  })
}

export function deleteSelected() {
  if (S.selected.size === 0) { toast.info('未选择任何文件。'); return }
  markDeletions([...S.selected])
}

/** 标记一组条目待删除（右键菜单「删除该文件」等入口）。 */
export function markDeletions(keys) {
  if (keys.length === 0) { toast.info('未选择任何文件。'); return }
  S.modify ??= defaultModify()

  // 与 egui 一致：按 (source, block_index) 排序 —— classic 在前、下标升序。
  const sorted = [...keys].sort((a, b) => {
    const [sa, ia] = a.split(':'), [sb, ib] = b.split(':')
    return sa === sb ? (+ia) - (+ib) : sa.localeCompare(sb)
  })

  for (const key of sorted) {
    if (S.modify.deletions.some((d) => d.key === key)) continue
    const entry = entryByKey.value.get(key)
    if (!entry) continue
    S.modify.deletions.push({
      display: entry.name || entry.display,
      name: entry.name || null,
      key,
    })
  }
}

/** 「修改归档」确认：勾选「另存为」时选保存位置，否则直接写回原文件；
 * 应用修改 → 写盘 → 自动重新载入。 */
export async function confirmModify() {
  const m = S.modify
  if (!m || m.busy) return
  let out = m.saveAs ? null : S.openPath
  if (!out) {
    // 另存为（或没有磁盘来源可写回）时选择保存位置。
    out = await ipc.dialogSave({ title: '保存修改后的归档', defaultPath: U.basenameOf(S.openPath || '') })
    if (!out) return
  }
  m.busy = true
  try {
    const r = await ipc.invoke('apply_modify', {
      additions: m.additions.map((a) => ({ name: a.name, path: a.path })),
      deletions: m.deletions.map((d) => ({ name: d.name, key: d.name ? null : d.key })),
      reencrypt: m.reencrypt,
    })
    const res = await ipc.invoke('save_modified', { path: out })
    const verb = out === S.openPath ? '修改已直接写入' : '修改已保存到'
    log(`${verb} ${out}（${r.note}）`)
    toast.success(`${verb} ${out}（${r.note}）`)
    S.modify = null
    ipc.invoke('discard_modify').catch(() => {})
    applyOpenResult(res)
  } catch (e) {
    log(String(e)) // 修改失败：… / 保存失败：…
    toast.error(String(e))
    m.busy = false
  }
}

/** 「修改归档」取消：丢弃待办与已应用结果，不写盘。 */
export function closeModify() {
  S.modify = null
  ipc.invoke('discard_modify').catch(() => {})
}

// ---------------------------------------------------------------------------
// 新建归档（「新建归档」窗口）

function defaultCreate() {
  return { version: 1, hashSize: 1024 }
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
      hashTableSize: c.hashSize, // Tauri 命令参数为 camelCase → Rust hash_table_size
    })
    S.create = null
    applyOpenResult(res)
    toast.success(`已创建 ${out}`)
  } catch (e) {
    toast.error(String(e)) // 创建失败：… / 保存失败：…
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
  if (!S.info) { toast.info('请先打开一个可解析的归档。'); return }
  S.convert = { target: defaultConvertTarget(S.info.version), running: false }
}

/** 「开始转换」：先弹系统保存框选输出位置（默认文件名带目标版本号，
 *  位于当前归档所在目录），再转换、写盘并重新载入；结果与异常以 toast 提示。 */
export async function runConvert() {
  const c = S.convert
  if (!c || c.running) return
  const out = await ipc.dialogSave({
    title: '保存转换后的归档',
    defaultPath: suggestedPath(markedName(`v${c.target}`)),
  })
  if (!out) return
  c.running = true
  try {
    const r = await ipc.invoke('apply_convert', { target: c.target })
    if (!r.applied) {
      toast.info(r.note)
      return
    }
    log(r.note)
    const res = await ipc.invoke('save_converted', { path: out })
    log(`转换结果已保存到 ${out}`)
    S.convert = null
    ipc.invoke('discard_convert').catch(() => {})
    applyOpenResult(res)
    toast.success(r.note)
  } catch (e) {
    toast.error(`转换失败：${e}`)
  } finally {
    c.running = false
  }
}

export function closeConvert() {
  S.convert = null
  ipc.invoke('discard_convert').catch(() => {})
}

// ---------------------------------------------------------------------------
// 密码保护（自定义 v4 加密扩展）

function startPwModal(mode) {
  S.pw = {
    mode,                 // 取值：'unlock' | 'set' | 'change' | 'remove'
    password: '',         // 用于 unlock / set / remove（当前密码）
    newPassword: '',      // 用于 change
    confirm: '',          // set / change 的确认输入
    busy: false,
  }
  if (mode === 'unlock') {
    toast.info('文件名无需密码即可浏览；读取、提取和修改文件内容需要密码。')
  }
}

/** 菜单入口：重新弹出解锁框（受保护但尚未解锁时可用）。 */
export function startUnlock() {
  if (!lockedArchive.value) { toast.info('该归档未处于锁定状态。'); return }
  startPwModal('unlock')
}

/** 菜单入口：设置 / 修改 / 解除密码保护。 */
export function startPwOp(mode) {
  if (!S.info) { toast.info('请先打开一个归档。'); return }
  if (mode === 'set' && protectedArchive.value) {
    toast.info('归档已启用密码保护，请使用「修改密码」。'); return
  }
  if (mode !== 'set' && !protectedArchive.value) {
    toast.info('归档未启用密码保护。'); return
  }
  startPwModal(mode)
}

/** 解锁确认：验证密码，成功后即可读取/提取/修改；异常以 toast 提示。 */
export async function submitUnlock() {
  const p = S.pw
  if (!p || p.busy) return
  if (!p.password) { toast.error('请输入密码。'); return }
  p.busy = true
  try {
    const r = await ipc.invoke('unlock_archive', { password: p.password })
    S.pw = null
    if (S.info) S.info.locked = false
    log(r.note)
    toast.success('已解锁')
  } catch (e) {
    toast.error(String(e))
    p.busy = false
  }
}

/** 在当前归档所在目录生成建议输出路径（name 为完整文件名）；
 *  没有磁盘来源时退化为纯文件名，由保存对话框采用默认目录。 */
function suggestedPath(name) {
  const full = S.openPath || ''
  const i = Math.max(full.lastIndexOf('\\'), full.lastIndexOf('/'))
  return i >= 0 ? full.slice(0, i + 1) + name : name
}

/** 在原文件名的扩展名前插入标记：demo.w3x → demo.<marker>.w3x。 */
function markedName(marker) {
  const m = U.basenameOf(S.openPath || '').match(/^(.*?)(\.[^.]*)?$/)
  return `${m[1]}.${marker}${m[2] || '.mpq'}`
}

/** 密码操作确认（set / change / remove）：先选保存位置，再应用到已
 *  载入字节、写盘并重新载入；验证与异常以 toast 提示。
 *  设置密码默认输出「原名.protected.扩展名」，修改 / 解除默认沿用
 *  原文件名（对话框中均可改）。 */
export async function confirmPwOp() {
  const p = S.pw
  if (!p || p.busy) return
  if (p.mode === 'set') {
    if (!p.password) { toast.error('请输入密码。'); return }
    if (p.password !== p.confirm) { toast.error('两次输入的密码不一致。'); return }
  } else if (p.mode === 'change') {
    if (!p.password) { toast.error('请输入当前密码。'); return }
    if (!p.newPassword) { toast.error('请输入新密码。'); return }
    if (p.newPassword !== p.confirm) { toast.error('两次输入的新密码不一致。'); return }
  } else if (!p.password) {
    toast.error('请输入当前密码。'); return
  }
  const suggested = suggestedPath(
    p.mode === 'set' ? markedName('protected') : U.basenameOf(S.openPath || ''))
  const out = await ipc.dialogSave({ title: '保存密码操作后的归档', defaultPath: suggested })
  if (!out) return
  p.busy = true
  try {
    const args = p.mode === 'change'
      ? { op: 'change', password: p.password, newPassword: p.newPassword }
      : { op: p.mode, password: p.password }
    const r = await ipc.invoke('apply_password_op', args)
    log(r.note)
    const res = await ipc.invoke('save_modified', { path: out })
    S.pw = null
    ipc.invoke('discard_modify').catch(() => {})
    // 在生成的归档旁维护「同名 .txt」密码记录：set/change 写入，
    // remove 删除。失败只记日志，不影响已保存的归档。
    const password = p.mode === 'remove' ? null
      : p.mode === 'change' ? p.newPassword : p.password
    try {
      const r2 = await ipc.invoke('write_password_note', { archivePath: out, password })
      if (r2?.note) log(r2.note)
    } catch (e) {
      log(`（密码记录文件更新失败：${e}）`)
    }
    applyOpenResult(res)
    log(`密码操作结果已保存到 ${out}`)
    toast.success(r.note)
  } catch (e) {
    toast.error(String(e))
    p.busy = false
  }
}

/** 关闭密码弹窗（未保存的已应用结果一并丢弃）。 */
export function closePw() {
  const wasUnlock = S.pw?.mode === 'unlock'
  S.pw = null
  ipc.invoke('discard_modify').catch(() => {})
  if (wasUnlock) toast.info('仍可浏览文件列表；读取、提取和修改内容时会提示需要密码。')
}

// ---------------------------------------------------------------------------
// 修复（「修复头信息」窗口）

export async function startRepair() {
  if (!S.openPath) { toast.info('请先打开一个归档。'); return }
  try {
    const r = await ipc.invoke('diagnose_archive')
    S.repair = { issues: r.issues, applied: false }
  } catch (e) {
    log(`诊断失败：${e}`)
    toast.error(`诊断失败：${e}`)
  }
}

export async function applyRepair() {
  try {
    const r = await ipc.invoke('apply_repair')
    S.repair.applied = true
    log(r.note)
    toast.success(r.note)
  } catch (e) {
    toast.error(`修复失败：${e}`)
  }
}

export async function saveRepaired() {
  const suggested = U.repairedName(S.openPath || 'archive.mpq')
  const out = await ipc.dialogSave({ title: '保存修复后的归档', defaultPath: suggested })
  if (!out) return
  try {
    const r = await ipc.invoke('save_repaired', { path: out })
    log(r.note)
    toast.success(r.note)
  } catch (e) {
    toast.error(String(e)) // 保存失败：…
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
    toast.error(String(e))
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
