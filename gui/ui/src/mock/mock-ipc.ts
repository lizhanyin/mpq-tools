/* 浏览器开发 mock（?mock）：伪造 Tauri invoke / 对话框 / 窗口 API，
 * 让 UI 可以脱离 Tauri 在纯浏览器中开发。仅 dev 模式被动态加载。
 * 数据形状与 src/types.ts 的 DTO 契约一致：None 字段省略（与
 * skip_serializing_if 的序列化行为相同），不是 null。 */

'use strict'

import type {
  ArchiveInfoDto, ConvertDto, DiagnoseDto, EntryDto, OpenResult,
  PreviewDto, TreeNodeDto,
} from '../types'

// MDX 预览样例：mdx-m3-viewer 官方示例资源（模型 + 其引用的 BLP 贴图）。
// 二进制资产用 new URL 引用（vite 对任意扩展都返回服务 URL；`?url` 导入
// 会让 import-analysis 把 .mdx 当 JS 解析）。
const mdxAssetUrl = new URL('./assets/SmileyGW_004.mdx.bin', import.meta.url).href
const blpAssetUrl = new URL('./assets/SmileyGW.blp', import.meta.url).href

async function fetchBase64(url: string): Promise<string> {
  const buf = new Uint8Array(await (await fetch(url)).arrayBuffer())
  let bin = ''
  for (let i = 0; i < buf.length; i += 0x8000) {
    bin += String.fromCharCode(...buf.subarray(i, i + 0x8000))
  }
  return btoa(bin)
}

const b64Cache = new Map<string, string>()
async function cachedBase64(key: string, url: string): Promise<string> {
  const hit = b64Cache.get(key)
  if (hit) return hit
  const b64 = await fetchBase64(url)
  b64Cache.set(key, b64)
  return b64
}
const mdxBase64 = () => cachedBase64('mdx', mdxAssetUrl)
const blpBase64 = () => cachedBase64('blp', blpAssetUrl)

function entry(
  name: string | null, key: string, size: number, csize: number,
  flags: string, locale: number, source: string, mtime: string | null,
): EntryDto {
  const display = name || `<unnamed #${key.split(':')[1]}>`
  return {
    name,
    display,
    size, csize, flags, locale, source, key, mtime,
  }
}

function dir(name: string, files: EntryDto[] = [], dirs: TreeNodeDto[] = []): TreeNodeDto {
  return {
    name,
    files,
    dirs,
    file_count: files.length + dirs.reduce((n, d) => n + d.file_count, 0),
    total_size: files.reduce((n, f) => n + f.size, 0) + dirs.reduce((n, d) => n + d.total_size, 0),
    total_csize: files.reduce((n, f) => n + f.csize, 0) + dirs.reduce((n, d) => n + d.total_csize, 0),
  }
}

const MOCK_TREE: TreeNodeDto = dir('', [
  entry('war3map.j', 'classic:0', 48213, 9120, 'EKC', 0, 'classic', '2023-01-01 08:00'),
  entry('war3map.lua', 'classic:13', 12040, 9100, 'EKC', 0, 'classic', '2023-01-01 08:00'),
  entry('war3map.w3e', 'classic:1', 12045, 8990, 'EKC#', 0, 'classic', '2023-01-01 08:00'),
], [
  dir('(系统文件)', [
    entry('(listfile)', 'classic:8', 210, 190, 'EC', 0, 'classic', null),
    entry('(attributes)', 'classic:9', 96, 96, 'E1', 0, 'classic', null),
  ]),
  dir('(未命名)', [
    entry(null, 'classic:10', 4096, 4100, 'K', 0, 'classic', null),
  ]),
  dir('Units', [], [
    dir('Human', [
      entry('Units\\Human\\Footman.def', 'classic:2', 640, 512, 'EC', 0, 'classic', '2022-11-30 19:20'),
      entry('Units\\Human\\Knight.def', 'classic:3', 710, 560, 'EC', 0, 'classic', '2022-11-30 19:21'),
    ]),
    dir('Orc', [
      entry('Units\\Orc\\Grunt.def', 'classic:4', 655, 520, 'EC', 0, 'classic', null),
    ]),
  ]),
  dir('war3mapImported', [
    entry('war3mapImported\\toast.blp', 'classic:5', 33792, 21000, 'EKC', 0, 'classic', '2024-06-15 12:30'),
    entry('war3mapImported\\readme.txt', 'classic:6', 800, 400, 'E', 0, 'classic', '2024-06-15 12:31'),
    entry('war3mapImported\\logo.png', 'classic:11', 40960, 39800, 'EK', 0, 'classic', '2024-06-15 12:32'),
    entry('war3mapImported\\floor.tga', 'classic:12', 69904, 21000, 'EKC', 0, 'classic', '2024-06-15 12:33'),
    // MDX 预览样例（mdx-m3-viewer 官方示例资源），贴图在 Textures\ 下。
    entry('war3mapImported\\SmileyGW_004.mdx', 'classic:14', 3964, 3500, 'EKC', 0, 'classic', '2024-06-15 12:34'),
    entry('war3mapImported\SmileyGW_005.mdx', 'classic:16', 3964, 3500, 'EKC', 0, 'classic', '2024-06-15 12:35'),
  ]),
  dir('Textures', [
    entry('Textures\\SmileyGW.blp', 'classic:15', 73014, 71000, 'EKC', 0, 'classic', '2024-06-15 12:34'),
  ]),
])

const MOCK_INFO: ArchiveInfoDto = {
  path: 'D:\\maps\\demo.w3x',
  version: 1,
  sector_size: 4096,
  hash_table_size: 1024,
  block_table_size: 17,
  het_bet: false,
  listfile_count: 14,
  warnings: ['archive_size 0x2200 与实际 0x22ac 不符，已按实际值处理'],
  tree: MOCK_TREE,
  entry_count: 17,
  protected: true,
  locked: true,
}

const MOCK_TEXT = `function config takes nothing returns nothing\n    call SetMapName("TRIGSTR_1")\nendfunction\n\n// 中文内容预览测试\n// war3map.j`

/* Lua 预览样例:刻意覆盖多版本语法(长字符串、goto 标签、<const>、
 * 位运算、整除、十六进制、长注释),用于验证 5.0+ 并集词法。 */
const MOCK_LUA = `local limit <const> = 0x7F
local name = [==[war3map lua 长字符串]==]
::retry::
if (a & 0xFF) // 2 ~= 0 then
    goto retry
end
print(string.format("%s %d", name, math.max(1, 2)))
--[[ 多行
注释 ]]`

/* 图片预览 mock：canvas 画一张渐变卡片当 data URL；tga/blp 模拟
 * 「已转换为 PNG」的解码路径。 */
const MOCK_IMAGE_MIME: Record<string, string> = {
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif',
  bmp: 'image/bmp', webp: 'image/webp', avif: 'image/avif',
  ico: 'image/x-icon', svg: 'image/svg+xml',
}

function mockImageUrl(ext: string): string {
  const c = document.createElement('canvas')
  c.width = 220
  c.height = 140
  const g = c.getContext('2d')!
  const grad = g.createLinearGradient(0, 0, 220, 140)
  grad.addColorStop(0, '#4b6cb7')
  grad.addColorStop(1, '#182848')
  g.fillStyle = grad
  g.fillRect(0, 0, 220, 140)
  g.strokeStyle = 'rgba(255,255,255,.3)'
  for (let i = 1; i < 6; i++) {
    g.beginPath(); g.moveTo(i * 36, 0); g.lineTo(i * 36, 140); g.stroke()
  }
  g.fillStyle = '#fff'
  g.font = 'bold 22px sans-serif'
  g.fillText(`.${ext}`, 14, 34)
  g.font = '14px sans-serif'
  g.fillText('mock 图片预览', 14, 60)
  return c.toDataURL('image/png')
}

function mockPreview(name: string): PreviewDto {
  const ext = (String(name).toLowerCase().match(/\.([a-z0-9]+)$/) || [])[1] || ''
  if (ext === 'tga' || ext === 'blp') {
    return {
      text: '', size: 33792 + name.length, truncated: false,
      encoding: 'UTF-8', binary: false,
      image: { url: mockImageUrl(ext), mime: 'image/png', converted: true },
    }
  }
  if (MOCK_IMAGE_MIME[ext]) {
    return {
      text: '', size: 40960 + name.length, truncated: false,
      encoding: 'UTF-8', binary: false,
      image: { url: mockImageUrl(ext), mime: MOCK_IMAGE_MIME[ext], converted: false },
    }
  }
  if (ext === 'lua') {
    return {
      text: `--- ${name} ---\n` + MOCK_LUA,
      size: name.length + MOCK_LUA.length + 120,
      truncated: false,
      encoding: 'UTF-8',
      binary: false,
    }
  }
  return {
    text: `--- ${name} ---\n` + MOCK_TEXT,
    size: name.length + MOCK_TEXT.length + 120,
    truncated: false,
    encoding: 'UTF-8',
    binary: false,
  }
}

const MOCKS: Record<string, (args: any) => unknown> = {
  open_archive: () => {
    pendingDeletedNames = []
    pendingDeletedKeys = []
    appliedDeletedNames = []
    appliedDeletedKeys = []
    return okOpen()
  },
  reload_archive: () => okOpen(),
  close_archive: () => null,
  unlock_archive: ({ password }: any) => {
    if (password === 'wrong') throw new Error('密码错误。')
    return { note: '已解锁：可以读取、提取和修改文件内容。' }
  },
  write_password_note: ({ archivePath, password }: any) => ({
    note: password
      ? `密码已记录到 ${archivePath.replace(/\.[^./]+$/, '')}.txt`
      : '未发现密码记录 …',
  }),
  apply_password_op: ({ op }: any) => ({
    note: op === 'set'
      ? '已启用密码保护（11 个文件已按扇区密封，容器转为 v4）。'
      : op === 'change' ? '已更换密码。' : '已解除密码保护（11 个文件已解密）。',
  }),
  preview_file: async ({ name }: any) => {
    // MDX：真实样本字节（assets 经 ?url 由 vite 提供），验证 3D 预览。
    if (String(name).toLowerCase().endsWith('.mdx')) {
      return {
        text: '', size: 3964, truncated: false,
        encoding: 'UTF-8', binary: false,
        model: { data: await mdxBase64() },
      } satisfies PreviewDto
    }
    return mockPreview(name)
  },
  // MDX 贴图按名取字节：Smiley 模型的贴图返回真实 BLP，其余 null（回退无贴图）。
  read_file_bytes: async ({ name }: any) => {
    const n = String(name).toLowerCase().replace(/\//g, '\\')
    if (n === 'textures\\smileygw.blp') return await blpBase64()
    return null
  },
  // 缩略图 mock：图片扩展名返回 canvas 生成的 data URL，其余 null。
  thumbnail: ({ name }: any) => {
    const ext = (String(name).toLowerCase().match(/\.([a-z0-9]+)$/) || [])[1] || ''
    return (MOCK_IMAGE_MIME[ext] || ext === 'tga' || ext === 'blp') ? mockImageUrl(ext) : null
  },
  stage_edited_file: ({ name, text }: any) => ({
    name,
    path: `C:\\Users\\dev\\AppData\\Local\\Temp\\mpqtool-edit\\${name.replace(/[\\/]/g, '_')}`,
    size: text.length,
    replaces: name === 'war3map.j',
  }),
  extract: ({ keys, dir }: any) => ({
    ok: keys.length || 11,
    failed: 0,
    messages: [`${dir}\\war3map.j (48213)`, `${dir}\\war3map.w3e (12045)`],
  }),
  diagnose_archive: (): DiagnoseDto => ({
    issues: [
      { field: 'hash_table_pos', problem: '哈希表位置 0x7f000000 越界', action: '扫描定位到 0x4c' },
      { field: 'archive_size', problem: '归档大小 0x1000 与实际 0xac 不符', action: '改写为 0xac' },
      { field: 'block_table', problem: '表内容校验失败' },
    ],
  }),
  apply_repair: () => ({ note: '已应用 2 项修复；1 项无法自动修复。' }),
  save_repaired: ({ path }: any) => ({ note: `已保存 ${path}` }),
  open_repaired: () => okOpen('D:\\maps\\demo.fixed.w3x'),
  discard_repair: () => null,
  stat_imports: ({ paths, prefix, childrenOnly }: any) => {
    const pre = prefix ? `${prefix}\\` : ''
    const out = []
    for (const p of paths) {
      // 文件夹导入：展开为递归文件列表（默认以文件夹自身为根，
      // childrenOnly 时只导入子项；键名与真实 Tauri 调用一致用 camelCase）。
      if (/war3mapImported$/i.test(p)) {
        const root = childrenOnly ? pre : `${pre}war3mapImported\\`
        out.push(
          { name: `${root}icon.blp`, path: `${p}\\icon.blp`, size: 8192, replaces: false },
          { name: `${root}ui\\panel.tga`, path: `${p}\\ui\\panel.tga`, size: 4096, replaces: false },
        )
        continue
      }
      const name = p.split(/[\\/]/).pop()
      out.push({
        name: pre + name,
        path: p,
        size: 12345,
        replaces: (pre + name).toLowerCase() === 'war3map.j',
      })
    }
    return out
  },
  apply_modify: ({ deletions }: any) => {
    pendingDeletedNames = (deletions || []).map((d: any) => d.name).filter(Boolean)
    pendingDeletedKeys = (deletions || []).filter((d: any) => !d.name && d.key).map((d: any) => d.key)
    const del = pendingDeletedNames.length + pendingDeletedKeys.length
    return { note: del ? `删除 ${del}` : '新增 1；替换 1' }
  },
  save_modified: ({ path }: any) => {
    appliedDeletedNames = [...appliedDeletedNames, ...pendingDeletedNames]
    appliedDeletedKeys = [...appliedDeletedKeys, ...pendingDeletedKeys]
    pendingDeletedNames = []
    pendingDeletedKeys = []
    return okOpen(path)
  },
  open_modified: () => {
    const r = okOpen('D:\\maps\\demo.w3x')
    r.note = '已载入修改结果（新增 1；替换 1；删除 1。）'
    return r
  },
  discard_modify: () => {
    pendingDeletedNames = []
    pendingDeletedKeys = []
    return null
  },
  create_archive: ({ path, version, hashTableSize }: any): OpenResult => {
    const info: ArchiveInfoDto = {
      ...MOCK_INFO,
      path,
      version,
      hash_table_size: hashTableSize ?? 1024,
      block_table_size: 1,
      het_bet: false,
      listfile_count: 0,
      warnings: ['no (listfile); file names unknown'],
      tree: dir('', []),
      entry_count: 0,
    }
    return { ok: true, note: '（新建空归档，用「操作 → 导入文件 / 导入文件夹」添加内容）', info }
  },
  apply_convert: ({ target }: any): ConvertDto => ({
    note: `已从 v${MOCK_INFO.version} 转换为 v${target}（11 个条目）；自动移除 (attributes)。`,
    applied: target !== MOCK_INFO.version,
  }),
  save_converted: ({ path }: any) => okOpen(path),
  open_converted: () => {
    const r = okOpen('D:\\maps\\demo.w3x')
    r.note = '已载入转换结果（已从 v1 转换为 v3（11 个条目）。）'
    return r
  },
  discard_convert: () => null,
}

function okOpen(path?: string): OpenResult {
  const tree = appliedDeletedNames.length || appliedDeletedKeys.length
    ? filterTree(MOCK_TREE) : MOCK_TREE
  const info: ArchiveInfoDto = { ...MOCK_INFO, tree, path: path || MOCK_INFO.path }
  if (tree !== MOCK_TREE) {
    let named = 0
    ;(function walk(n: TreeNodeDto) {
      for (const d of n.dirs) walk(d)
      named += n.files.filter((f) => f.name).length
    })(tree)
    info.listfile_count = named
    info.entry_count = (function count(n: TreeNodeDto): number {
      return n.files.length + n.dirs.reduce((s, d) => s + count(d), 0)
    })(tree)
  }
  return { ok: true, info }
}

/* 删除流程模拟：apply_modify 记录待删除名单（会话内替换），取消即
 * 丢弃；save_modified 把名单并入已应用集合并返回过滤后的树（模拟
 * 后端行为：文件消失、空目录不保留）。 */
let pendingDeletedNames: string[] = []
let pendingDeletedKeys: string[] = []
let appliedDeletedNames: string[] = []
let appliedDeletedKeys: string[] = []

function filterTree(node: TreeNodeDto): TreeNodeDto {
  const files = node.files.filter(
    (f) => !appliedDeletedKeys.includes(f.key)
      && !(f.name && appliedDeletedNames.some((n) => n.toLowerCase() === f.name!.toLowerCase())),
  )
  const dirs = node.dirs.map(filterTree).filter((d) => d.file_count > 0)
  return {
    ...node,
    files,
    dirs,
    file_count: files.length + dirs.reduce((n, d) => n + d.file_count, 0),
    total_size: files.reduce((n, f) => n + f.size, 0) + dirs.reduce((n, d) => n + d.total_size, 0),
    total_csize: files.reduce((n, f) => n + f.csize, 0) + dirs.reduce((n, d) => n + d.total_csize, 0),
  }
}

export function invoke(cmd: string, args?: Record<string, unknown>): Promise<unknown> {
  return new Promise((resolve, reject) => {
    setTimeout(() => {
      try {
        const fn = MOCKS[cmd]
        if (!fn) reject(new Error(`mock: unknown command ${cmd}`))
        else resolve(fn(args || {}))
      } catch (e) {
        reject(e) // mock 里 throw 的语义与 Tauri 的 Err → reject 一致
      }
    }, 60)
  })
}

interface MockDialogOpts {
  title?: string
  directory?: boolean
  multiple?: boolean
  defaultPath?: string
}

export function dialogOpen(opts?: MockDialogOpts): Promise<string | string[]> {
  if (opts && opts.directory) {
    // 「导入文件夹」返回带内容的目录，提取等其它用途保持原样。
    return Promise.resolve(opts.title && opts.title.includes('导入') ? 'D:\\import\\war3mapImported' : 'D:\\out')
  }
  if (opts && opts.multiple) return Promise.resolve(['D:\\import\\war3map.j', 'D:\\import\\newfile.txt'])
  return Promise.resolve('D:\\maps\\demo.w3x')
}

export function dialogSave(opts?: MockDialogOpts): Promise<string> {
  const p = (opts && opts.defaultPath) || 'out.w3x'
  // 绝对路径原样返回（建议路径现在带目录）；纯文件名落到 mock 目录。
  return Promise.resolve(/^[A-Za-z]:[\\/]/.test(p) ? p : `D:\\maps\\${p}`)
}

export function closeWindow(): void {
  window.close()
}
