/* 纯函数工具：路径/树操作与格式化（与后端 tasks.rs 的同名逻辑对应）。 */

import type { EntryDto, TreeNodeDto } from './types'

/** 收集未命名条目的虚拟文件夹名（与后端 tasks.rs 一致）。 */
export const UNNAMED_DIR = '(未命名)'
/** 收集 MPQ 内部元数据文件的虚拟文件夹名（与后端 tasks.rs 一致）。 */
export const SYSTEM_DIR = '(系统文件)'

/** 与后端 mpq::SPECIAL_FILES 一致的内部文件名（大小写不敏感全名匹配）：
 * 归入 (系统文件) 虚拟文件夹（build_tree 的规则）。 */
const SPECIAL_NAMES = new Set([
  '(listfile)', '(attributes)', '(signature)', '(userdata)', '(password)',
])

export function isSpecialName(name: string): boolean {
  return SPECIAL_NAMES.has(name.toLowerCase())
}

/** 与后端 split_path 一致：`\`/`/` 分隔，丢弃空段与 `.`/`..`。 */
export function splitPath(name: string): string[] {
  return name.replace(/\//g, '\\').split('\\').filter((s) => s && s !== '.' && s !== '..')
}

/** 条目所属目录的小写 `\` 前缀（根为 ''）：未命名 → (未命名)、内部
 * 文件 → (系统文件)，其余按名字的目录部分——与 build_tree 的分桶
 * 一致，供 selectedByPrefix 按目录前缀计数。 */
export function dirPrefixOf(e: EntryDto): string {
  if (!e.name) return UNNAMED_DIR.toLowerCase()
  if (isSpecialName(e.name)) return SYSTEM_DIR.toLowerCase()
  return dirNameOf(e.name).toLowerCase()
}

export function fmtBytes(n: number): string {
  return (n).toLocaleString('en-US')
}

/** 字节数带单位：33 KB / 1.5 MB；信息浮条等展示用。 */
export function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let v = n
  for (const u of units) {
    v /= 1024
    if (v < 1024 || u === 'TB') return `${v >= 100 ? Math.round(v) : v.toFixed(1)} ${u}`
  }
  return `${v.toFixed(1)} TB` // 不可达：循环至 TB 必返回
}

/** 路径（`\`/`/` 分隔）的最后一段。 */
export function baseName(name: string): string {
  const parts = String(name).split(/[\\/]/)
  return parts[parts.length - 1] || name
}

/** 磁盘文件名的最后一段（用于保存对话框默认名）。 */
export function basenameOf(path: string): string {
  const parts = String(path).split(/[\\/]/)
  return parts[parts.length - 1] || path
}

/** 路径（`\`/`/` 分隔）去掉最后一段后的目录部分（`\` 连接；根为空串）。 */
export function dirNameOf(name: string): string {
  const parts = String(name).split(/[\\/]/)
  parts.pop()
  return parts.join('\\')
}

/** 与后端 imagepreview 一致的图片扩展名（原生直读 + TGA/BLP 解码）。 */
const IMAGE_EXTS = new Set([
  'png', 'jpg', 'jpeg', 'jfif', 'gif', 'bmp', 'dib',
  'webp', 'avif', 'ico', 'cur', 'svg', 'tga', 'blp',
])

/** 归档内条目是否是可预览的图片（用于图片预览的左右切换列表）。 */
export function isImageName(name: string | null): boolean {
  const s = String(name)
  const i = s.lastIndexOf('.')
  return i >= 0 && IMAGE_EXTS.has(s.slice(i + 1).toLowerCase())
}

/** 文件夹 key：路径段小写后用 `\` 连接（大小写不敏感）。 */
export function folderKey(path: string[]): string {
  return path.map((s) => s.toLowerCase()).join('\\')
}

/** 大小写不敏感地沿 path 下钻树节点（tasks::node_at）。 */
export function nodeAt(root: TreeNodeDto, path: string[]): TreeNodeDto | null {
  let node: TreeNodeDto | undefined = root
  for (const seg of path) {
    const lower = seg.toLowerCase()
    node = node.dirs.find((d) => d.name.toLowerCase() === lower)
    if (!node) return null
  }
  return node ?? null
}

/** 默认修复副本名：<stem>.fixed.<ext>。 */
export function repairedName(path: string): string {
  const base = basenameOf(path)
  const m = base.match(/^(.*?)(\.[^.]*)?$/)!
  const ext = m[2] || '.mpq'
  return `${m[1]}.fixed${ext}`
}
