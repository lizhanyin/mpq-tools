/* 全局状态与动作：从原 app.js 迁移，界面组件只读状态并调用这里的动作。 */

import { reactive, computed, watch, shallowRef, triggerRef } from 'vue'
import { Channel } from '@tauri-apps/api/core'
import * as ipc from './ipc'
import { SYSTEM_DIR, UNNAMED_DIR, folderKey } from './utils'
import * as U from './utils'
import { toast } from './toast'
import type {
  ArchiveInfoDto, ConvertDto, DiagnoseDto, DiffResultDto, EntryDto, ExtractDto,
  ImportProgress, ImportStatDto, IssueDto, MoveSpec, NoteDto, OpenResult, PreviewDto, StagedDto,
  SearchDto, TreeNodeDto,
} from './types'

export const ARCHIVE_FILTERS = [
  { name: 'MPQ 归档', extensions: ['mpq', 'w3x', 'w3m', 'w3n', 'scx', 'sc2map', 's2ma', 'rep'] },
  { name: '所有文件', extensions: ['*'] },
]
export { SYSTEM_DIR, UNNAMED_DIR }
// 表格已启用虚拟滚动（渲染成本只与可见行相关），此上限仅作为
// 极端情况（如巨型 listfile）的保险，超过时提示用过滤缩小范围。
export const MAX_ROWS = 50000
export const FONT_MIN = 10, FONT_MAX = 28, FONT_DEFAULT = 13

// ---------------------------------------------------------------------------
// 状态形状（各弹窗 / 浮窗的字段含义见下方各模块注释）

/** 浮动预览窗的全部状态。 */
export interface PreviewState {
  /** 条目选择键（<source>:<index>，未命名条目重载「加载全部」用）。 */
  key: string
  /** 显示名（未命名条目为 <unnamed #N>[.ext]）。 */
  name: string
  /** 归档内真名（编辑保存用；未命名条目为 null）。 */
  fileName: string | null
  text: string
  size: number
  encoding: string
  truncated: boolean
  binary: boolean
  image: PreviewDto['image'] | null   // 图片条目：data URL + 元信息
  model: PreviewDto['model'] | null   // MDX 模型条目：base64 字节
  /** 截断（后半未加载）、二进制回退、图片或模型时只读，避免保存出损坏内容。 */
  editable: boolean
  dirty: boolean        // Monaco 里是否改过
  saving: boolean
  armed: boolean        // 有未保存修改时关闭需二次确认
  loadingFull: boolean  // 「加载全部」请求在途（按钮转圈，防重复点击）
}

/** 「修改归档 → 待导入」的单个条目（stat_imports 结果的落地形态）。 */
export interface ImportItem {
  name: string
  path: string
  size: number
  replaces: boolean
  /** 「移动」排队的条目记录的原归档名；应用后据此不保留被搬空的源目录。 */
  movedFrom?: string | null
}

/** 「修改归档 → 待删除」的单个条目。 */
export interface DeletionItem {
  display: string
  name: string | null   // 未命名条目没有名字，只能按 key 删除
  key: string
}

/** 「导入文件夹」会话信息。 */
export interface FolderImportState {
  paths: string[]       // 本次导入的源路径（菜单选的文件夹，或拖入的混合列表）
  childrenOnly: boolean
  /** 入队前的待导入快照，供切换「只导入子项」时还原。 */
  snapshot: ImportItem[]
  switching?: boolean   // 正在按新模式重新统计（期间禁止再次切换）
}

/** 「修改归档」窗口状态。 */
export interface ModifyState {
  additions: ImportItem[]
  deletions: DeletionItem[]
  reencrypt: boolean
  saveAs: boolean
  busy: boolean
  folderImport: FolderImportState | null
}

/** 「版本转换」窗口状态。 */
export interface ConvertState {
  target: number
  running: boolean
}

/** 「生成补丁」窗口状态：当前归档 = 新文件，与磁盘上选择的旧基线
 *  对比，勾选的差异（新增 + 修改）打包成独立的 patch.mpq。 */
export interface PatchState {
  /** 旧（基线）文件路径；选好后立即对比。 */
  oldPath: string | null
  diff: DiffResultDto | null
  loading: boolean      // 对比进行中
  /** added + changed 默认全选；记录被取消勾选的名字（小写）。 */
  unchecked: Set<string>
  /** 补丁容器版本；null = 跟随当前归档。 */
  version: number | null
  busy: boolean         // 生成进行中
}

/** 「合并归档」窗口状态：把磁盘上选择的来源归档合并进当前归档。
 *  来源独有（added）直接导入；同名不同内容（changed）即「覆盖」项，
 *  默认全选覆盖，可一键全选 / 全不选。 */
export interface MergeState {
  sourcePath: string | null
  diff: DiffResultDto | null
  loading: boolean
  /** added + changed 默认全选；记录被取消勾选的名字（小写）。 */
  unchecked: Set<string>
  staging: boolean      // 正在读取来源内容写临时文件
}

/** 「新建归档」窗口状态。 */
export interface CreateState {
  version: number
  hashSize: number
}

/** 「修复头信息」窗口状态。 */
export interface RepairState {
  issues: IssueDto[]
  applied: boolean
}

/** 「移动到…」窗口状态：keys 为要移动的条目，target 为目标路径段
 *  （[] = 归档根）。 */
export interface MoveState {
  keys: string[]
  target: string[]
}

export type PwMode = 'unlock' | 'set' | 'change' | 'remove'

/** 密码弹窗状态（四种 mode 共用一组输入，按 mode 取用）。 */
export interface PwState {
  mode: PwMode
  password: string      // 用于 unlock / set / remove（当前密码）
  newPassword: string   // 用于 change
  confirm: string       // set / change 的确认输入
  busy: boolean
  errTick: number       // 验证失败计数递增，弹窗据此重新聚焦密码输入框
}

/** 表格 / 网格的行：目录行或文件行（判别联合，见 rows）。 */
export type Row =
  | { type: 'dir'; node: TreeNodeDto; path: string[] }
  | { type: 'file'; entry: EntryDto; showFull: boolean }

interface AppState {
  /** 最近一次成功打开的 ArchiveInfoDto（未打开时为 null）。树是懒加载
   *  的：只有已进入 / 展开的目录有内容，其余为计数齐全的占位符。 */
  info: ArchiveInfoDto | null
  /** 当前视图背后的文件路径；解析失败时也保留（修复流程作用在这些字节上）。 */
  openPath: string | null
  cwd: string[]         // 路径段；[] = 归档根
  expanded: string[]    // 展开的文件夹 key（小写 `\` 连接）
  filter: string
  appliedFilter: string // 120ms 防抖后的过滤词
  selected: Set<string> // 条目 key "classic:12" / "bet:3"
  /** 后端搜索的结果（searching 时填充；懒加载后前端树不完整，过滤
   *  改在后端完整树上做）。 */
  searchResults: EntryDto[]
  searchTruncated: boolean  // 命中超过 MAX_ROWS 被截断
  searchLoading: boolean    // 搜索请求在途
  dirLoading: boolean       // 当前目录懒加载在途（面包屑提示「正在载入」）
  viewMode: 'table' | 'grid' // 文件浏览视图（localStorage 持久化）
  preview: PreviewState | null // 浮动预览窗
  repair: RepairState | null
  modify: ModifyState | null
  newFolder: { name: string } | null
  move: MoveState | null       // 「移动到」弹窗
  create: CreateState | null   // 新建归档弹窗
  convert: ConvertState | null // 版本转换弹窗
  patch: PatchState | null     // 生成补丁弹窗
  merge: MergeState | null     // 合并归档弹窗
  pw: PwState | null           // 密码弹窗
  settingsOpen: boolean
  aboutOpen: boolean           // 「关于」弹窗
  logs: string[]
  fontSize: number
  gridThumbs: boolean   // 网格视图显示图片缩略图（localStorage 持久化，默认关闭）
  /** 老版本 Warcraft III 客户端目录（localStorage 持久化；模型预览
   *  的贴图在归档里找不到时回退到其中的 War3*.mpq 读取）。 */
  war3Dir: string
  dropping: { count: number } | null // 拖拽悬停
  /** 导入进度（stat_imports 扫描 / apply_modify 读取-编码-校验 /
   *  保存）；null = 当前没有导入在途。弹窗开着时 ModifyModal 内显示
   *  详情，否则 App.vue 顶部显示细进度条。 */
  importProgress: ImportProgress | null
  /** 键盘焦点区：'list' 文件列表（表格/网格）| 'preview' 预览浮窗。
   *  方向键归谁：列表有焦点 → 网格导航、预览跟随选择；
   *  预览有焦点 → 预览切图、选择反向跟随预览。 */
  focusZone: 'list' | 'preview'
  /** 预览窗请求网格把光标/滚动定位到的行索引（-1 = 无请求）。 */
  gridCursor: number
}

export const S = reactive<AppState>({
  info: null,
  openPath: null,
  cwd: [],
  expanded: [],
  filter: '',
  appliedFilter: '',
  selected: new Set(),
  searchResults: [],
  searchTruncated: false,
  searchLoading: false,
  dirLoading: false,
  viewMode: 'table',
  preview: null,
  repair: null,
  modify: null,
  newFolder: null,
  move: null,
  create: null,
  convert: null,
  patch: null,
  merge: null,
  pw: null,
  settingsOpen: false,
  aboutOpen: false,
  logs: [],
  fontSize: FONT_DEFAULT,
  gridThumbs: false,
  war3Dir: '',
  dropping: null,
  importProgress: null,
  focusZone: 'list',
  gridCursor: -1,
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
  !!(S.modify || S.create || S.convert || S.patch || S.merge || S.pw || S.repair || S.preview || S.newFolder || S.move || S.settingsOpen || S.aboutOpen))

/** key → entry 的累积缓存：打开时的根目录、每次 list_dir / 搜索 /
 * 目录递归条目都汇入（代替原先对整棵树的一次性遍历——懒加载后前端
 * 不再持有全树）。选择 / 删除 / 移动的展示名从这里取；能被选中的
 * 条目必然来自某个已加载视图或搜索结果，因此总能命中。 */
export const entryByKey = shallowRef(new Map<string, EntryDto>())
function ingestEntries(entries: Iterable<EntryDto>) {
  const m = entryByKey.value
  for (const e of entries) m.set(e.key, e)
  triggerRef(entryByKey)
}

export const searchResults = computed(() => S.searchResults)

/** 表格当前行。目录未加载完成时先给空列表（navigate 已触发
 * ensureDirLoaded，合并进 reactive 树后本 computed 自动重算）。 */
export const rows = computed<{ list: Row[]; truncated: boolean }>(() => {
  const out: Row[] = []
  if (!S.info) return { list: out, truncated: false }
  if (searching.value) {
    for (const e of S.searchResults) out.push({ type: 'file', entry: e, showFull: true })
  } else {
    const node = U.nodeAt(S.info.tree, S.cwd)
    if (node?.loaded) {
      for (const d of node.dirs) out.push({ type: 'dir', node: d, path: [...S.cwd, d.name] })
      for (const f of node.files) out.push({ type: 'file', entry: f, showFull: false })
    }
  }
  return {
    list: out.slice(0, MAX_ROWS),
    truncated: out.length > MAX_ROWS || (searching.value && S.searchTruncated),
  }
})

/** 已选条目按目录前缀（folderKey 形态，根为 ''）的计数——懒加载树下
 * 没有子树 key 清单，目录行 / 表头复选框也能同步推导选中态。 */
export const selectedByPrefix = computed(() => {
  const map = new Map<string, number>()
  for (const k of S.selected) {
    const e = entryByKey.value.get(k)
    if (!e) continue
    map.set('', (map.get('') ?? 0) + 1)
    const prefix = U.dirPrefixOf(e)
    if (!prefix) continue
    let acc = ''
    for (const seg of prefix.split('\\')) {
      acc = acc ? `${acc}\\${seg}` : seg
      map.set(acc, (map.get(acc) ?? 0) + 1)
    }
  }
  return map
})

// ---------------------------------------------------------------------------
// 目录懒加载（大归档初始只发根目录一层，进入 / 展开时按需取）

/** 懒加载代数：每次打开 / 重载 / 关闭归档递增，用于丢弃过期的在途响应。 */
let lazyGen = 0
/** list_dir 在途请求（folderKey → Promise），并发展开去重。 */
const listDirInflight = new Map<string, Promise<TreeNodeDto | null>>()
/** dir_entries 请求（folderKey → Promise）：在途去重 + 结果缓存
 *  （目录行勾选 / 框选 / 右键批量操作会反复取同一目录）。 */
const dirEntriesCache = new Map<string, Promise<EntryDto[]>>()
/** 已解析的递归条目（同步窥视用，见 peekDirEntries）。 */
const dirEntriesResolved = new Map<string, EntryDto[]>()

function resetLazyState() {
  lazyGen++
  listDirInflight.clear()
  dirEntriesCache.clear()
  dirEntriesResolved.clear()
  entryByKey.value = new Map()
  S.searchResults = []
  S.searchTruncated = false
  S.searchLoading = false
  S.dirLoading = false
}

/** 确保路径上的目录（含祖先链）已加载。返回是否全部就绪；失败只在
 * 日志里记一行（表格保持空白，用户可导航别处）。 */
export async function ensureDirLoaded(path: string[]): Promise<boolean> {
  if (!S.info) return false
  const myGen = lazyGen
  for (let i = 1; i <= path.length; i++) {
    const segs = path.slice(0, i)
    const node = U.nodeAt(S.info.tree, segs)
    if (!node) return false
    if (node.loaded) continue
    const key = folderKey(segs)
    let p = listDirInflight.get(key)
    if (!p) {
      p = ipc.invoke<TreeNodeDto>('list_dir', { path: segs })
        .catch((e: unknown) => { log(`载入文件夹失败 ${key}：${e}`); return null })
      listDirInflight.set(key, p)
      void p.finally(() => listDirInflight.delete(key))
    }
    const dto = await p
    if (myGen !== lazyGen || !dto || !S.info) return false
    const parent = i === 1 ? S.info.tree : U.nodeAt(S.info.tree, segs.slice(0, -1))!
    const lower = segs[i - 1].toLowerCase()
    const idx = parent.dirs.findIndex((d) => d.name.toLowerCase() === lower)
    if (idx >= 0) parent.dirs.splice(idx, 1, dto)
    ingestEntries(dto.files)
  }
  return true
}

/** 目录（含未加载子目录）的递归条目：后端取，会话内缓存 + 去重。
 * 前端虚拟空目录（file_count 0，如「新建文件夹」）直接返回空。 */
export function getDirEntries(path: string[]): Promise<EntryDto[]> {
  const node = S.info ? U.nodeAt(S.info.tree, path) : null
  if (node && node.file_count === 0) return Promise.resolve([])
  const key = folderKey(path)
  let p = dirEntriesCache.get(key)
  if (!p) {
    const myGen = lazyGen
    p = ipc.invoke<EntryDto[]>('dir_entries', { path })
      .catch((e: unknown): EntryDto[] => { log(`读取文件夹内容失败 ${key}：${e}`); return [] })
      .then((entries) => {
        if (myGen === lazyGen) {
          dirEntriesResolved.set(key, entries)
          ingestEntries(entries)
        }
        return entries
      })
    dirEntriesCache.set(key, p)
  }
  return p
}

/** getDirEntries 的同步窥视：仅在已解析时命中（网格框选连续重算用，
 * 未命中的目录由调用方异步补取，完成后重算）。 */
export function peekDirEntries(path: string[]): EntryDto[] | null {
  const node = S.info ? U.nodeAt(S.info.tree, path) : null
  if (node && node.file_count === 0) return []
  return dirEntriesResolved.get(folderKey(path)) ?? null
}

// ---------------------------------------------------------------------------
// 搜索（后端完整树上进行；懒加载后前端只有已载入的目录）

let searchSeq = 0 // 并发搜索只认最后一次（与预览的 previewSeq 同理）

async function runSearch(n: string) {
  const seq = ++searchSeq
  const myGen = lazyGen
  S.searchLoading = true
  try {
    const r = await ipc.invoke<SearchDto>('search_archive', { needle: n })
    if (seq !== searchSeq || myGen !== lazyGen || !S.info) return
    S.searchResults = r.entries
    S.searchTruncated = r.truncated
    ingestEntries(r.entries)
  } catch (e) {
    if (seq !== searchSeq || myGen !== lazyGen) return
    S.searchResults = []
    S.searchTruncated = false
    log(`搜索失败：${e}`)
  } finally {
    if (seq === searchSeq) S.searchLoading = false
  }
}

watch(needle, (n) => {
  if (!S.info) return
  if (n === '') {
    S.searchResults = []
    S.searchTruncated = false
    return
  }
  void runSearch(n)
})

// ---------------------------------------------------------------------------
// 过滤（防抖）与字号

let filterTimer: ReturnType<typeof setTimeout> | undefined
export function setFilter(v: string) {
  S.filter = v
  clearTimeout(filterTimer)
  filterTimer = setTimeout(() => { S.appliedFilter = v }, 120)
}

function loadFontSize(): number {
  const v = parseFloat(localStorage.getItem('font_size') ?? '')
  return Number.isFinite(v) ? Math.min(FONT_MAX, Math.max(FONT_MIN, v)) : FONT_DEFAULT
}

export function setFontSize(v: number) {
  const clamped = Math.min(FONT_MAX, Math.max(FONT_MIN, v))
  S.fontSize = clamped
  document.documentElement.style.fontSize = `${clamped}px`
  localStorage.setItem('font_size', String(clamped))
}

export function initStore() {
  S.fontSize = loadFontSize()
  document.documentElement.style.fontSize = `${S.fontSize}px`
  S.viewMode = localStorage.getItem('view_mode') === 'grid' ? 'grid' : 'table'
  S.gridThumbs = localStorage.getItem('grid_thumbs') === '1'
  S.war3Dir = localStorage.getItem('war3_dir') ?? ''
}

export function setViewMode(v: string) {
  S.viewMode = v === 'grid' ? 'grid' : 'table'
  localStorage.setItem('view_mode', S.viewMode)
}

export function setGridThumbs(v: boolean) {
  S.gridThumbs = !!v
  localStorage.setItem('grid_thumbs', v ? '1' : '0')
}

/** 设置老客户端目录：立即持久化。预览中的模型需重新打开才会应用
 *  新的贴图回退（已在浏览器里的贴图有会话缓存）。 */
export function setWar3Dir(v: string) {
  S.war3Dir = v
  localStorage.setItem('war3_dir', v)
}

// ---------------------------------------------------------------------------
// 日志

export function log(msg: string) {
  S.logs.push(msg)
}
export function clearLog() {
  S.logs.length = 0
}

// ---------------------------------------------------------------------------
// 打开 / 重载 / 拖放

function applyOpenResult(res: OpenResult, pathIfLoaded?: string, { keepNav = false }: { keepNav?: boolean } = {}) {
  if (res.note) log(res.note)
  // 懒加载状态全部作废：代数 +1 让在途响应过期，缓存与条目表清空。
  resetLazyState()
  if (res.ok) {
    S.info = res.info
    S.openPath = res.info.path
    if (!keepNav) {
      S.cwd = []
      S.expanded = []
      S.selected.clear()
    }
    ingestEntries(res.info.tree.files) // 根目录一层
    log(`已打开 ${res.info.path}（${U.fmtBytes(res.info.entry_count)} 个条目）`)
    for (const w of res.info.warnings) log(`warning: ${w}`)
    // 受密码保护且尚未解锁：立即弹密码框（可取消，文件名仍可浏览）。
    if (res.info.protected && res.info.locked && !S.pw) {
      log('（归档受密码保护，输入密码后才能读取文件内容）')
      startPwModal('unlock')
    }
    // 重载保留导航：新树只有根一层，把原当前目录与已展开分支补齐。
    if (keepNav) {
      void ensureDirLoaded(S.cwd)
      for (const k of S.expanded) void ensureDirLoaded(k.split('\\'))
    }
    // 过滤词仍有效时在新归档上重新搜索（原先由 computed 在全树上
    // 直接重算；懒加载后搜索结果来自后端，需要主动重取）。
    if (needle.value) void runSearch(needle.value)
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
  if (!path || Array.isArray(path)) return
  await openArchivePath(path)
}

export async function openArchivePath(path: string) {
  try {
    const res = await ipc.invoke<OpenResult>('open_archive', { path })
    applyOpenResult(res, path)
  } catch (e) {
    log(String(e)) // 例如：读取失败：…
    toast.error(String(e))
  }
}

export async function reloadArchive() {
  if (!S.openPath) return
  const openPath = S.openPath
  const hadPending = !!S.modify &&
    (S.modify.additions.length > 0 || S.modify.deletions.length > 0)
  const cwd = S.cwd, expanded = S.expanded, filter = S.filter
  try {
    const res = await ipc.invoke<OpenResult>('reload_archive')
    applyOpenResult(res, openPath, { keepNav: true })
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
  resetLazyState()
  S.info = null
  S.openPath = null
  S.cwd = []
  S.expanded = []
  S.selected.clear()
  S.filter = ''
  S.appliedFilter = ''
  S.preview = null
  S.modify = null
  S.newFolder = null
  S.move = null
  S.convert = null
  S.repair = null
  S.pw = null
  ipc.invoke('close_archive').catch(() => {})
}

export async function handleDrop(paths: string[]) {
  if (S.info) {
    log(`（通过拖拽导入 ${U.fmtBytes(paths.length)} 项文件/文件夹）`)
    const snapshot = (S.modify?.additions ?? []).map((a) => ({ ...a }))
    const added = await queueImports(paths)
    const m = S.modify
    // 拖入内容含文件夹时按「导入文件夹」处理（可用「只导入子项」）。
    // 判定：有条目的磁盘路径位于 `该拖入路径\` 之下即为文件夹内容；
    // 空文件夹没有任何条目，无从切换导入方式，不识别也不影响结果。
    if (m && !m.folderImport && added?.length) {
      const hasFolder = paths.some((p) => {
        const norm = String(p).replace(/[\\/]+$/, '').toLowerCase()
        return added.some((a) => a.path.toLowerCase().startsWith(norm + '\\'))
      })
      if (hasFolder) m.folderImport = { paths, childrenOnly: false, snapshot }
    }
    return
  }
  try {
    const res = await ipc.invoke<OpenResult>('open_archive', { path: paths[0] })
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

let previewSeq = 0 // 并发预览时只认最后一次，防止慢的旧响应覆盖新预览

/** 预览请求的条目信息：key 恒有（未命名条目按 key 寻址），
 *  name / display 用于标题展示与联动比较。 */
export interface PreviewTarget {
  key: string
  name: string | null
  display: string
}

export async function previewFile(target: PreviewTarget, opts: { full?: boolean } = {}) {
  const seq = ++previewSeq
  try {
    const p = await ipc.invoke<PreviewDto>('preview_file', {
      key: target.key, name: target.name ?? undefined, full: !!opts.full,
    })
    if (seq !== previewSeq) return // 期间又预览了其它条目，丢弃过期响应
    S.preview = {
      key: target.key,
      name: target.display,
      fileName: target.name,
      text: p.text,
      size: p.size,
      encoding: p.encoding || 'UTF-8',
      truncated: !!p.truncated,
      binary: !!p.binary,
      image: p.image || null,
      model: p.model || null,
      editable: !!target.name && !p.truncated && !p.binary && !p.image && !p.model,
      dirty: false,                 // Monaco 里是否改过
      saving: false,
      armed: false,                 // 有未保存修改时关闭需二次确认
      loadingFull: false,           // 完整加载完成（或未触发）后的新对象
    }
  } catch (e) {
    if (seq !== previewSeq) return
    log(`预览失败 ${name}: ${e}`)
    toast.error(`预览失败 ${name}: ${e}`)
  }
}

export function closePreview() {
  S.preview = null
}

/** 预览窗开着时，列表里单选了图片条目 → 预览跟随切换（列表→预览方向；
 *  预览→列表方向见 PreviewWindow 的 syncSelection，名字相同即停，不成环）。 */
const selectedImageName = computed((): string | null => {
  if (S.selected.size !== 1) return null
  const e = entryByKey.value.get([...S.selected][0])
  return e && U.isImageName(e.display) ? e.display : null
})
watch(selectedImageName, (display) => {
  if (!display || !S.preview || S.preview.name === display) return
  const e = entryByKey.value.get([...S.selected][0])
  if (e) previewFile(e)
})

/** 预览窗「保存」：按原编码写临时文件（后端），再把该文件排队到
 *  「修改归档 → 待导入」，由用户在弹窗中确认应用/写盘。 */
export async function savePreviewEdit(text: string) {
  const p = S.preview
  if (!p || !p.editable || !p.fileName || p.saving) return
  p.saving = true
  try {
    const name = p.fileName
    const st = await ipc.invoke<StagedDto>('stage_edited_file', {
      name, text, encoding: p.encoding,
    })
    S.modify ??= defaultModify()
    const dupIdx = S.modify.additions.findIndex((a) => a.name.toLowerCase() === name.toLowerCase())
    const item: ImportItem = {
      name,
      path: st.path,
      size: st.size,
      replaces: st.replaces || dupIdx >= 0,
    }
    if (dupIdx >= 0) S.modify.additions[dupIdx] = item
    else S.modify.additions.push(item)
    p.dirty = false
    p.armed = false
    log(`已保存编辑 ${name}（${U.fmtBytes(st.size)} 字节，${item.replaces ? '替换已有文件' : '新增'}），在「修改归档」窗口中应用`)
    toast.success(`已保存编辑：${name}（在「修改归档」中确认写入）`)
  } catch (e) {
    log(String(e))
    toast.error(String(e))
  }
  p.saving = false
}

async function pickFolder(title: string): Promise<string | null> {
  const dir = await ipc.dialogOpen({ title, directory: true })
  return Array.isArray(dir) ? null : dir
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
export async function extractKeys(keys: string[]) {
  if (keys.length === 0) { toast.info('未选择任何文件。'); return }
  const dir = await pickFolder('选择输出目录')
  if (!dir) return
  await runExtract(keys, dir)
}

async function runExtract(keys: string[], dir: string) {
  try {
    const r = await ipc.invoke<ExtractDto>('extract', { keys, dir })
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

export function toggleSelected(key: string, on: boolean) {
  if (on) S.selected.add(key)
  else S.selected.delete(key)
}

/** 勾选目录行：递归选中 / 取消其下全部文件。子树条目经后端取
 * （懒加载树下前端未必加载过该目录的内容）。 */
export function toggleDirSelected(path: string[], on: boolean) {
  void getDirEntries(path).then((entries) => {
    for (const e of entries) {
      if (on) S.selected.add(e.key)
      else S.selected.delete(e.key)
    }
  })
}

/** 用一组 key 整体替换当前选择（行单击单选 / Shift 范围选共用）。 */
export function setSelectedKeys(keys: string[]) {
  S.selected.clear()
  for (const k of keys) S.selected.add(k)
}

/** 全选当前视图：搜索态 = 全部命中（前端已有）；目录态 = 当前文件
 * 夹递归（经后端取子树条目）。 */
export function selectAll() {
  if (searching.value) {
    for (const f of S.searchResults) S.selected.add(f.key)
    return
  }
  void getDirEntries(S.cwd).then((entries) => {
    for (const e of entries) S.selected.add(e.key)
  })
}

export function selectNone() {
  S.selected.clear()
}

/** 表头全选复选框的状态。搜索态对结果 key 精确计数；目录态用
 * selectedByPrefix（cwd 子树内已选数）对比占位符自带的 file_count。 */
export const scopeSelection = computed(() => {
  if (searching.value) {
    if (S.searchResults.length === 0) return { checked: false, indeterminate: false, disabled: true }
    let sel = 0
    for (const f of S.searchResults) if (S.selected.has(f.key)) sel++
    return {
      checked: sel === S.searchResults.length,
      indeterminate: sel > 0 && sel < S.searchResults.length,
      disabled: false,
    }
  }
  if (!S.info) return { checked: false, indeterminate: false, disabled: true }
  const node = U.nodeAt(S.info.tree, S.cwd)
  if (!node) return { checked: false, indeterminate: false, disabled: true }
  const total = node.file_count
  const sel = selectedByPrefix.value.get(folderKey(S.cwd)) ?? 0
  return {
    checked: total > 0 && sel === total,
    indeterminate: sel > 0 && sel < total,
    disabled: total === 0,
  }
})

export function toggleSelectAll(on: boolean) {
  if (on) selectAll()
  else selectNone()
}

// ---------------------------------------------------------------------------
// 导入 / 删除（「修改归档」窗口）

function defaultModify(): ModifyState {
  return {
    additions: [], deletions: [], reencrypt: true, saveAs: false, busy: false,
    folderImport: null,
  }
}

export async function importFilesDialog() {
  const paths = await ipc.dialogOpen({ title: '选择要导入的文件', multiple: true })
  if (!paths) return
  await queueImports(Array.isArray(paths) ? paths : [paths])
}

/** 「导入文件夹」：选完目录直接入队（以文件夹为根）。「只导入子项」
 *  在「修改归档」里切换，不再经过确认弹窗。 */
export async function importFolderDialog() {
  const dir = await ipc.dialogOpen({ title: '选择要导入的文件夹', directory: true })
  if (!dir || Array.isArray(dir)) return
  await queueImportsAsFolder([dir])
}

/** 按文件夹导入会话入队：记录快照与来源，让「修改归档」里的
 *  「只导入子项」与「导入到」可用（拖拽导入同样走这里）。 */
async function queueImportsAsFolder(paths: string[]) {
  const snapshot = (S.modify?.additions ?? []).map((a) => ({ ...a }))
  const added = await queueImports(paths)
  if (S.modify && added?.length) {
    S.modify.folderImport = { paths, childrenOnly: false, snapshot }
  }
}

/** 「修改归档」里切换「只导入子项」。先按新模式完成统计，成功后才
 *  一次性替换：勾选状态、导入到地址、待导入列表同帧切换，不经过
 *  清空→重填的中间态（否则弹窗内容会闪动）。统计失败则保持原样。 */
export async function setImportChildrenOnly(on: boolean) {
  const m = S.modify
  const fi = m?.folderImport
  if (!m || !fi || m.busy || fi.switching || !!on === fi.childrenOnly) return
  fi.switching = true
  let stats: ImportStatDto[]
  try {
    stats = await fetchStats(fi.paths, { childrenOnly: !!on })
  } catch (e) {
    log(String(e))
    toast.error(String(e))
    return
  } finally {
    fi.switching = false
  }
  fi.childrenOnly = !!on
  m.additions = fi.snapshot.map((a) => ({ ...a }))
  mergeStats(stats, m)
}

/** 「修改归档」的「导入到」：本次待导入条目落地的归档内目录。
 *  单个文件夹以文件夹为根时落在 `前缀\文件夹名` 下；勾选「只导入
 *  子项」、多个文件夹/混合拖入或普通文件导入落在前缀本身。 */
export const importTargetDir = computed(() => {
  const m = S.modify
  if (!m) return ''
  const prefix = importPrefix() || ''
  const fi = m.folderImport
  if (fi && !fi.childrenOnly && fi.paths.length === 1) {
    const base = U.basenameOf(fi.paths[0])
    return prefix ? `${prefix}\\${base}` : base
  }
  return prefix
})

/** 导入到当前浏览的目录：浏览到子目录时待导入名称带该前缀。
 *  （系统文件）/(未命名) 是视图伪目录，不作为前缀。 */
function importPrefix(): string | null {
  if (!S.cwd.length || S.cwd[0] === SYSTEM_DIR || S.cwd[0] === UNNAMED_DIR) return null
  return S.cwd.join('\\')
}

/** 单条待导入日志的上限；文件夹导入超过此数只记一条汇总。 */
const IMPORT_LOG_MAX = 50

/** 建 Tauri Channel 并把进度事件落到全局状态。浏览器 mock 模式下
 *  Channel 构造依赖 __TAURI_INTERNALS__（不存在），改用普通对象；
 *  mock-ipc 直接调 onmessage 推事件，与真实路径共用同一形状。 */
function progressChannel(): Channel<ImportProgress> {
  if (ipc.isMock) {
    const ch = { onmessage: (_p: ImportProgress) => {} } as unknown as Channel<ImportProgress>
    ch.onmessage = (p) => { S.importProgress = p }
    return ch
  }
  const ch = new Channel<ImportProgress>()
  ch.onmessage = (p) => { S.importProgress = p }
  return ch
}

/** 进度文案：读取中 / 准备中 / 对比中 / 导入中 / 校验中 / 保存中 + 计数或百分比。 */
export function importProgressLabel(p: ImportProgress | null): string {
  if (!p) return ''
  if (p.phase === 'saving') return '保存中…'
  if (p.phase === 'preparing') return '准备中…'
  const name = p.phase === 'reading' ? '读取中' : p.phase === 'encoding' ? '导入中'
    : p.phase === 'diffing' ? '对比中' : '校验中'
  const count = p.total > 0 ? `${p.done}/${p.total}` : `已发现 ${p.done} 项`
  const pct = p.total > 0 ? `（${importProgressPercent(p)}%）` : ''
  return `${name} ${count}${pct}`
}

/** 进度百分比（0–100 取整）；total 未知时返回 undefined（显示不确定进度）。 */
export function importProgressPercent(p: ImportProgress | null): number | undefined {
  if (!p || p.total <= 0) return undefined
  return Math.min(100, Math.floor((p.done / p.total) * 100))
}

/** 调 stat_imports 检查待导入内容（不发任何 UI 变更）。扫描期间经
 *  Channel 上报「读取中」进度（拖拽 / 菜单导入大量文件时的反馈）。 */
async function fetchStats(paths: string[], opts: { childrenOnly?: boolean } = {}): Promise<ImportStatDto[]> {
  S.importProgress = { phase: 'reading', done: 0, total: 0 }
  try {
    return await ipc.invoke<ImportStatDto[]>('stat_imports', {
      paths,
      prefix: importPrefix(),
      // Tauri 命令参数为 camelCase → Rust children_only
      ...(opts.childrenOnly ? { childrenOnly: true } : {}),
      onProgress: progressChannel(),
    })
  } finally {
    S.importProgress = null
  }
}

/** 把 stat 结果合并进待导入列表（同名后者取代前者）并记日志。 */
function mergeStats(stats: ImportStatDto[], m: ModifyState): ImportItem[] {
  // 同名（大小写不敏感）后导入的取代先排队的。
  const idxByName = new Map(m.additions.map((a, i) => [a.name.toLowerCase(), i] as const))
  const added: ImportItem[] = []
  for (const st of stats) {
    if (st.error) {
      log(`读取失败 ${st.path}: ${st.error}`)
      toast.error(`读取失败 ${st.name}: ${st.error}`)
      continue
    }
    const lower = st.name.toLowerCase()
    const dupIdx = idxByName.get(lower) ?? -1
    const item: ImportItem = { name: st.name, path: st.path, size: st.size, replaces: st.replaces || dupIdx >= 0 }
    if (dupIdx >= 0) m.additions[dupIdx] = item
    else m.additions.push(item)
    idxByName.set(lower, dupIdx >= 0 ? dupIdx : m.additions.length - 1)
    added.push(item)
  }

  if (added.length === 0) {
    toast.info('没有可导入的文件（所选内容为空或全部读取失败）。')
    return []
  }
  added.slice(0, IMPORT_LOG_MAX).forEach((it) =>
    log(`待导入 ${it.name}（${U.fmtBytes(it.size)} 字节，${it.replaces ? '替换已有文件' : '新增'}）`))
  if (added.length > IMPORT_LOG_MAX) {
    log(`（其余 ${added.length - IMPORT_LOG_MAX} 项已加入待导入队列）`)
  }
  return added
}

/** 统计并入队待导入；返回本次入队的条目（供拖拽判定文件夹用）。 */
export async function queueImports(paths: string[], opts: { childrenOnly?: boolean } = {}): Promise<ImportItem[]> {
  if (!paths.length) return []
  S.modify ??= defaultModify()

  let stats: ImportStatDto[]
  try {
    stats = await fetchStats(paths, opts)
  } catch (e) {
    log(String(e))
    toast.error(String(e))
    return []
  }
  return mergeStats(stats, S.modify)
}

// ---------------------------------------------------------------------------
// 新建文件夹（虚拟目录）

/** 弹出「新建文件夹」；浏览到子目录时在其中创建。MPQ 无法保存空
 *  文件夹——先在视图中建出目录，把文件导入其中并应用修改后，
 *  该目录才随文件真正写入归档。 */
export function startNewFolder() {
  if (!S.info) { toast.info('请先打开一个归档。'); return }
  S.newFolder = { name: '' }
}

export function closeNewFolder() {
  S.newFolder = null
}

/** 拆分并校验输入的文件夹名（可用 `\` `/` 一次建多级）；非法时提示
 *  并返回 null。 */
function splitFolderName(input: string): string[] | null {
  const segs = input.split(/[\\/]+/).map((s) => s.trim()).filter(Boolean)
  if (!segs.length) { toast.error('请输入文件夹名。'); return null }
  for (const s of segs) {
    if (s === '.' || s === '..') { toast.error(`文件夹名不能是「${s}」。`); return null }
    if (/[:*?"<>|]/.test(s)) { toast.error(`文件夹名不能包含 : * ? " < > |（${s}）`); return null }
    if (/[. ]$/.test(s)) { toast.error(`文件夹名不能以点或空格结尾（${s}）`); return null }
  }
  return segs
}

/** 「新建文件夹」确认：沿当前目录逐级补齐缺失的目录节点（大小写
 *  不敏感复用已有）。不进入新目录——留在当前位置，仅展开树的祖先
 *  分支让新文件夹立即可见。 */
export function createFolder() {
  const f = S.newFolder
  if (!f || !S.info) return
  const segs = splitFolderName(f.name)
  if (!segs) return
  const base = (S.cwd[0] === SYSTEM_DIR || S.cwd[0] === UNNAMED_DIR) ? [] : S.cwd
  let node = U.nodeAt(S.info.tree, base)
  if (!node) { S.newFolder = null; return }

  let created = 0
  const full = [...base]
  for (const seg of segs) {
    const lower = seg.toLowerCase()
    let next: TreeNodeDto | undefined = node.dirs.find((d) => d.name.toLowerCase() === lower)
    if (!next) {
      // 虚拟空目录：loaded=true（内容已知为空，无需也不应再向
      // 后端请求——归档里并不存在这个文件夹）。
      next = { name: seg, dirs: [], files: [], file_count: 0, total_size: 0, total_csize: 0, loaded: true }
      node.dirs.push(next)
      // 与后端 build_tree 一致按小写名排序，保持目录顺序稳定。
      node.dirs.sort((a, b) =>
        a.name.toLowerCase() < b.name.toLowerCase() ? -1 : a.name.toLowerCase() > b.name.toLowerCase() ? 1 : 0)
      created++
    }
    full.push(seg)
    node = next
  }
  S.newFolder = null
  setFilter('') // 退出搜索视图，让新文件夹可见
  // 展开整条祖先链，让新文件夹在目录树中可见（不导航进入）。
  let key = ''
  for (const seg of full) {
    key = key ? `${key}\\${seg.toLowerCase()}` : seg.toLowerCase()
    if (!S.expanded.includes(key)) S.expanded.push(key)
  }
  if (!created) { toast.info('文件夹已存在。'); return }
  log(`已新建文件夹 ${full.join('\\')}——空文件夹不会写入归档，导入文件到其中并应用修改后生效`)
  toast.success(`已新建文件夹：${full.join('\\')}（导入文件后保存才会写入归档）`)
}

// ---------------------------------------------------------------------------
// 移动（把条目挪到别的文件夹：以新名字排队待导入 + 标记旧条目待删除）

/** 弹出「移动到…」窗口：keys 为要移动的条目（通常来自当前选择），
 *  目标默认为当前文件夹（特殊目录退化为归档根）。 */
export function startMoveModal(keys: string[]) {
  if (!S.info) { toast.info('请先打开一个归档。'); return }
  if (keys.length === 0) { toast.info('未选择任何文件。'); return }
  const base = (S.cwd[0] === SYSTEM_DIR || S.cwd[0] === UNNAMED_DIR) ? [] : S.cwd
  S.move = { keys: [...keys], target: [...base] }
}

export function closeMoveModal() {
  S.move = null
}

/** 把一组条目移动到 targetDir（路径段数组，[] = 归档根）：内容经
 *  stage_move_files 读入临时文件后按新名字排队为「待导入」，旧条目
 *  按选择键标记待删除——都在「修改归档」窗口应用后生效。
 *  未命名条目（没有名字无法重命名）与已在目标文件夹中的条目跳过。 */
export async function moveKeysTo(keys: string[], targetDir: string[]) {
  if (!S.info) return
  if (targetDir[0] === SYSTEM_DIR || targetDir[0] === UNNAMED_DIR) {
    toast.error('不能移动到该特殊文件夹。')
    return
  }
  const targetName = targetDir.join('\\')
  const targetLower = targetName.toLowerCase()

  const moves: MoveSpec[] = []
  let unnamed = 0, already = 0
  for (const k of keys) {
    const e = entryByKey.value.get(k)
    if (!e) continue
    if (!e.name) { unnamed++; continue } // 没有名字无法以新路径写回
    if (U.dirNameOf(e.name).toLowerCase() === targetLower) { already++; continue }
    // 不同来源的同名文件移到同一处：只保留最后一个（与待导入去重一致）。
    const name = targetName ? `${targetName}\\${U.baseName(e.name)}` : U.baseName(e.name)
    const dup = moves.findIndex((m) => m.name.toLowerCase() === name.toLowerCase())
    if (dup >= 0) moves[dup] = { key: k, name }
    else moves.push({ key: k, name })
  }
  if (unnamed) log(`（${U.fmtBytes(unnamed)} 个未命名条目没有名字，无法移动）`)
  if (moves.length === 0) {
    toast.info(already > 0 ? '所选条目已在目标文件夹中。' : '没有可移动的条目。')
    return
  }

  let staged: StagedDto[]
  try {
    staged = await ipc.invoke<StagedDto[]>('stage_move_files', { moves })
  } catch (e) {
    log(String(e))
    toast.error(String(e))
    return
  }

  S.modify ??= defaultModify()
  const m = S.modify
  // 待导入：同名（大小写不敏感）后者取代前者，与 mergeStats 一致；
  // movedFrom 记录旧名字，应用后据此不保留被搬空的源目录。
  const idxByName = new Map(m.additions.map((a, i) => [a.name.toLowerCase(), i] as const))
  for (let i = 0; i < staged.length; i++) {
    const st = staged[i]
    const from = entryByKey.value.get(moves[i].key)?.name || null
    const lower = st.name.toLowerCase()
    const dupIdx = idxByName.get(lower) ?? -1
    const item: ImportItem = { name: st.name, path: st.path, size: st.size, replaces: st.replaces || dupIdx >= 0, movedFrom: from }
    if (dupIdx >= 0) m.additions[dupIdx] = item
    else m.additions.push(item)
    idxByName.set(lower, dupIdx >= 0 ? dupIdx : m.additions.length - 1)
  }
  // 旧条目标记待删除（name 留空 → 按选择键精确删除）。
  const sorted = [...moves].sort((a, b) => {
    const [sa, ia] = a.key.split(':'), [sb, ib] = b.key.split(':')
    return sa === sb ? (+ia) - (+ib) : sa.localeCompare(sb)
  })
  for (const mv of sorted) {
    if (m.deletions.some((d) => d.key === mv.key)) continue
    const entry = entryByKey.value.get(mv.key)
    if (!entry) continue
    m.deletions.push({ display: entry.name || entry.display, name: null, key: mv.key })
  }

  const where = targetName || '归档根目录'
  log(`已标记移动 ${U.fmtBytes(staged.length)} 个文件到 ${where}——在「修改归档」窗口应用后生效`)
  toast.success(`已标记移动 ${U.fmtBytes(staged.length)} 个文件到 ${where}（在「修改归档」中确认写入）`)
}

// ---------------------------------------------------------------------------
// 生成补丁 / 合并归档（当前归档 + 磁盘上选择的另一个归档）

/** 弹出「生成补丁」：当前归档作为新文件，另选旧基线对比。 */
export function startPatch() {
  if (!S.info) { toast.info('请先打开一个归档。'); return }
  S.patch = { oldPath: null, diff: null, loading: false, unchecked: new Set(), version: null, busy: false }
}

export function closePatch() {
  S.patch = null
}

/** 弹出「合并归档」：另选来源归档合并进当前归档。 */
export function startMerge() {
  if (!S.info) { toast.info('请先打开一个归档。'); return }
  S.merge = { sourcePath: null, diff: null, loading: false, unchecked: new Set(), staging: false }
}

export function closeMerge() {
  S.merge = null
}

/** 执行对比（两个弹窗共用）：调 diff_with_disk，结果落到弹窗状态。
 *  期间经 Channel 上报「对比中」进度（弹窗内的进度条显示）。 */
async function runDiff(target: PatchState | MergeState, path: string, baseline: boolean) {
  target.diff = null
  target.unchecked = new Set()
  target.loading = true
  S.importProgress = { phase: 'diffing', done: 0, total: 0 }
  try {
    target.diff = await ipc.invoke<DiffResultDto>('diff_with_disk', {
      path, baseline, onProgress: progressChannel(),
    })
  } catch (e) {
    log(String(e))
    toast.error(String(e))
  } finally {
    target.loading = false
    S.importProgress = null
  }
}

/** 选择旧（基线）文件并立即对比。 */
export async function pickPatchBaseline() {
  const p = S.patch
  if (!p || p.loading) return
  const path = await ipc.dialogOpen({ title: '选择旧文件（基线归档）', filters: ARCHIVE_FILTERS })
  if (!path || Array.isArray(path)) return
  p.oldPath = path
  await runDiff(p, path, true)
}

/** 选择合并来源归档并立即对比。 */
export async function pickMergeSource() {
  const m = S.merge
  if (!m || m.loading) return
  const path = await ipc.dialogOpen({ title: '选择要合并进来的来源归档', filters: ARCHIVE_FILTERS })
  if (!path || Array.isArray(path)) return
  m.sourcePath = path
  await runDiff(m, path, false)
}

/** 差异列表中默认勾选（added + changed）且未被取消的名字。 */
function pickedDiffNames(d: DiffResultDto, unchecked: Set<string>): string[] {
  return d.entries
    .filter((e) => (e.kind === 'added' || e.kind === 'changed') && !unchecked.has(e.name.toLowerCase()))
    .map((e) => e.name)
}

/** 「生成补丁」确认：选择保存位置 → 后端提取选中文件并打包写出。
 *  读取 → 编码 → 自检的进度显示在弹窗内。 */
export async function confirmBuildPatch() {
  const p = S.patch
  if (!p || !p.diff || p.busy) return
  const names = pickedDiffNames(p.diff, p.unchecked)
  if (!names.length) { toast.info('请至少勾选一个差异文件。'); return }
  const out = await ipc.dialogSave({
    title: '保存补丁归档',
    defaultPath: 'patch.mpq',
    filters: ARCHIVE_FILTERS,
  })
  if (!out) return
  const version = p.version ?? S.info?.version ?? 1
  p.busy = true
  S.importProgress = { phase: 'reading', done: 0, total: names.length }
  try {
    const r = await ipc.invoke<NoteDto>('build_patch', {
      names, outPath: out, version, onProgress: progressChannel(),
    })
    log(r.note)
    toast.success(r.note)
    S.patch = null
  } catch (e) {
    log(String(e))
    toast.error(String(e))
  } finally {
    S.importProgress = null
    p.busy = false
  }
}

/** 「合并归档」确认：来源内容经 stage_merge_files 读入临时文件后按
 *  原名排队为「待导入」——同名条目在「修改归档」应用时自动替换
 *  （即覆盖），与「移动」一致的两阶段流程。 */
export async function confirmMerge() {
  const m = S.merge
  if (!m || !m.diff || m.staging) return
  if (!m.sourcePath) { toast.info('请先选择来源归档。'); return }
  const names = pickedDiffNames(m.diff, m.unchecked)
  if (!names.length) { toast.info('请至少勾选一个要合并的文件。'); return }
  m.staging = true
  let staged: StagedDto[]
  try {
    staged = await ipc.invoke<StagedDto[]>('stage_merge_files', { source: m.sourcePath, names })
  } catch (e) {
    log(String(e))
    toast.error(String(e))
    m.staging = false
    return
  }

  S.modify ??= defaultModify()
  const mod = S.modify
  // 待导入：同名（大小写不敏感）后者取代前者，与 queueImports 一致。
  const idxByName = new Map(mod.additions.map((a, i) => [a.name.toLowerCase(), i] as const))
  for (const st of staged) {
    const lower = st.name.toLowerCase()
    const dupIdx = idxByName.get(lower) ?? -1
    const item: ImportItem = { name: st.name, path: st.path, size: st.size, replaces: st.replaces || dupIdx >= 0 }
    if (dupIdx >= 0) mod.additions[dupIdx] = item
    else mod.additions.push(item)
    idxByName.set(lower, dupIdx >= 0 ? dupIdx : mod.additions.length - 1)
  }
  const nReplace = staged.filter((s) => s.replaces).length
  S.merge = null
  const summary = `已排队合并 ${staged.length} 个文件（新增 ${staged.length - nReplace}、覆盖 ${nReplace}）`
  log(`${summary}——在「修改归档」窗口应用后生效`)
  toast.success(`${summary}，请在「修改归档」窗口确认写入`)
}

export function deleteSelected() {
  if (S.selected.size === 0) { toast.info('未选择任何文件。'); return }
  markDeletions([...S.selected])
}

/** 标记一组条目待删除（右键菜单「删除该文件」等入口）。 */
export function markDeletions(keys: string[]) {
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

/** 应用删除后：内容被删空的目录在重载的树里已消失（MPQ 存不了空
 *  目录），把它们重新挂回上级之下作为虚拟目录——与「新建文件夹」
 *  一致：不写入归档，重新载入后消失，导入文件并应用后成为真实目录。
 *  names 为本次删除的条目名（未命名条目没有名字，跳过）。 */
function keepEmptiedDirsVirtual(names: string[]) {
  if (!S.info) return
  let kept = 0
  for (const name of names) {
    const segs = String(name).split('\\')
    segs.pop()                    // 去掉文件名，剩下目录链
    if (!segs.length) continue    // 根目录下的文件没有目录链
    let node = S.info.tree
    for (const seg of segs) {
      const lower = seg.toLowerCase()
      let next = node.dirs.find((d) => d.name.toLowerCase() === lower)
      if (!next) {
        // 与「新建文件夹」一致的虚拟空目录（loaded=true，见 createFolder）。
        next = { name: seg, dirs: [], files: [], file_count: 0, total_size: 0, total_csize: 0, loaded: true }
        node.dirs.push(next)
        // 与后端 build_tree 一致按小写名排序，保持目录顺序稳定。
        node.dirs.sort((a, b) =>
          a.name.toLowerCase() < b.name.toLowerCase() ? -1 : a.name.toLowerCase() > b.name.toLowerCase() ? 1 : 0)
        kept++
      }
      node = next
    }
  }
  if (kept) {
    log(`（内容被删空的 ${U.fmtBytes(kept)} 个目录已保留为空文件夹——不会写入归档，重新载入归档后消失）`)
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
  // 初始态先给出读取进度（等 Channel 事件刷新）；保存 / 载入阶段无
  // 后端事件，前端置 'saving' 表示写盘 + 重新载入。
  S.importProgress = { phase: 'reading', done: 0, total: m.additions.length }
  try {
    const r = await ipc.invoke<NoteDto>('apply_modify', {
      additions: m.additions.map((a) => ({ name: a.name, path: a.path })),
      deletions: m.deletions.map((d) => ({ name: d.name, key: d.name ? null : d.key })),
      reencrypt: m.reencrypt,
      onProgress: progressChannel(),
    })
    S.importProgress = { phase: 'saving', done: 0, total: 0 }
    const res = await ipc.invoke<OpenResult>('save_modified', { path: out })
    const verb = out === S.openPath ? '修改已直接写入' : '修改已保存到'
    log(`${verb} ${out}（${r.note}）`)
    toast.success(`${verb} ${out}（${r.note}）`)
    const deletedNames = m.deletions.map((d) => d.name).filter((n): n is string => !!n)
    S.modify = null
    ipc.invoke('discard_modify').catch(() => {})
    applyOpenResult(res)
    keepEmptiedDirsVirtual(deletedNames)
  } catch (e) {
    log(String(e)) // 修改失败：… / 保存失败：…
    toast.error(String(e))
    m.busy = false
  } finally {
    S.importProgress = null
  }
}

/** 「修改归档」取消：丢弃待办与已应用结果，不写盘。 */
export function closeModify() {
  S.modify = null
  ipc.invoke('discard_modify').catch(() => {})
}

// ---------------------------------------------------------------------------
// 新建归档（「新建归档」窗口）

function defaultCreate(): CreateState {
  return { version: 1, hashSize: 1024 }
}

export function startCreate() {
  S.create = defaultCreate()
}

export async function createArchive() {
  const c = S.create
  if (!c) return
  const out = await ipc.dialogSave({
    title: '新建 MPQ 归档',
    defaultPath: '新建归档.mpq',
    filters: ARCHIVE_FILTERS,
  })
  if (!out) return
  try {
    const res = await ipc.invoke<OpenResult>('create_archive', {
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

function defaultConvertTarget(from: number): number {
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
    const r = await ipc.invoke<ConvertDto>('apply_convert', { target: c.target })
    if (!r.applied) {
      toast.info(r.note)
      return
    }
    log(r.note)
    const res = await ipc.invoke<OpenResult>('save_converted', { path: out })
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

function startPwModal(mode: PwMode) {
  S.pw = {
    mode,
    password: '',         // 用于 unlock / set / remove（当前密码）
    newPassword: '',      // 用于 change
    confirm: '',          // set / change 的确认输入
    busy: false,
    errTick: 0,
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
export function startPwOp(mode: PwMode) {
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
    const r = await ipc.invoke<NoteDto>('unlock_archive', { password: p.password })
    S.pw = null
    if (S.info) S.info.locked = false
    log(r.note)
    toast.success('已解锁')
  } catch (e) {
    toast.error(String(e))
    p.busy = false
    p.errTick++
  }
}

/** 在当前归档所在目录生成建议输出路径（name 为完整文件名）；
 *  没有磁盘来源时退化为纯文件名，由保存对话框采用默认目录。 */
function suggestedPath(name: string): string {
  const full = S.openPath || ''
  const i = Math.max(full.lastIndexOf('\\'), full.lastIndexOf('/'))
  return i >= 0 ? full.slice(0, i + 1) + name : name
}

/** 在原文件名的扩展名前插入标记：demo.w3x → demo.<marker>.w3x。 */
function markedName(marker: string): string {
  const m = U.basenameOf(S.openPath || '').match(/^(.*?)(\.[^.]*)?$/)!
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
    const r = await ipc.invoke<NoteDto>('apply_password_op', args)
    log(r.note)
    const res = await ipc.invoke<OpenResult>('save_modified', { path: out })
    S.pw = null
    ipc.invoke('discard_modify').catch(() => {})
    // 在生成的归档旁维护「同名 .txt」密码记录：set/change 写入，
    // remove 删除。失败只记日志，不影响已保存的归档。
    const password = p.mode === 'remove' ? null
      : p.mode === 'change' ? p.newPassword : p.password
    try {
      const r2 = await ipc.invoke<NoteDto | null>('write_password_note', { archivePath: out, password })
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
    p.errTick++
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
    const r = await ipc.invoke<DiagnoseDto>('diagnose_archive')
    S.repair = { issues: r.issues, applied: false }
  } catch (e) {
    log(`诊断失败：${e}`)
    toast.error(`诊断失败：${e}`)
  }
}

export async function applyRepair() {
  try {
    const r = await ipc.invoke<NoteDto>('apply_repair')
    if (S.repair) S.repair.applied = true
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
    const r = await ipc.invoke<NoteDto>('save_repaired', { path: out })
    log(r.note)
    toast.success(r.note)
  } catch (e) {
    toast.error(String(e)) // 保存失败：…
  }
}

export async function openRepaired() {
  try {
    const res = await ipc.invoke<OpenResult>('open_repaired')
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

export function toggleExpanded(key: string) {
  const i = S.expanded.indexOf(key)
  if (i >= 0) S.expanded.splice(i, 1)
  else S.expanded.push(key)
}

/** 进入目录：立即切 cwd（面包屑先变），未加载的目录异步取一层，
 * 合并进树后 rows 自动重算。 */
export function navigate(path: string[]) {
  S.cwd = path
  void refreshCwd()
}

async function refreshCwd() {
  if (!S.info || searching.value) { S.dirLoading = false; return }
  const node = U.nodeAt(S.info.tree, S.cwd)
  if (node?.loaded) { S.dirLoading = false; return }
  S.dirLoading = true
  await ensureDirLoaded(S.cwd)
  S.dirLoading = false
}
