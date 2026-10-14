/* 纯函数工具：路径/树操作与格式化（与后端 tasks.rs 的同名逻辑对应）。 */

export function fmtBytes(n) {
  return (n).toLocaleString('en-US')
}

/** 路径（`\`/`/` 分隔）的最后一段。 */
export function baseName(name) {
  const parts = String(name).split(/[\\/]/)
  return parts[parts.length - 1] || name
}

/** 磁盘文件名的最后一段（用于保存对话框默认名）。 */
export function basenameOf(path) {
  const parts = String(path).split(/[\\/]/)
  return parts[parts.length - 1] || path
}

/** 文件夹 key：路径段小写后用 `\` 连接（大小写不敏感）。 */
export function folderKey(path) {
  return path.map((s) => s.toLowerCase()).join('\\')
}

/** 大小写不敏感地沿 path 下钻树节点（tasks::node_at）。 */
export function nodeAt(root, path) {
  let node = root
  for (const seg of path) {
    const lower = seg.toLowerCase()
    node = node.dirs.find((d) => d.name.toLowerCase() === lower)
    if (!node) return null
  }
  return node
}

/** node 及其子目录下的全部文件条目（tasks::collect_files）。 */
export function collectFiles(node) {
  const out = [...node.files]
  for (const d of node.dirs) out.push(...collectFiles(d))
  return out
}

export function collectKeys(node) {
  return collectFiles(node).map((f) => f.key)
}

/** 全路径包含 needle（已小写）的条目（tasks::search_tree）。 */
export function searchTree(root, needle) {
  const out = []
  ;(function walk(node, prefix) {
    for (const d of node.dirs) {
      walk(d, prefix ? `${prefix}\\${d.name}` : d.name)
    }
    for (const f of node.files) {
      const full = f.name ? (prefix ? `${prefix}\\${f.display}` : f.display)
                          : f.display
      if (full.toLowerCase().includes(needle)) out.push(f)
    }
  })(root, '')
  return out
}

/** node 子树下是否有条目命中 needle（过滤后的树只保留命中分支）。 */
export function subtreeMatches(node, needle) {
  return searchTree(node, needle).length > 0
}

/** 默认修复副本名：<stem>.fixed.<ext>。 */
export function repairedName(path) {
  const base = basenameOf(path)
  const m = base.match(/^(.*?)(\.[^.]*)?$/)
  const ext = m[2] || '.mpq'
  return `${m[1]}.fixed${ext}`
}
