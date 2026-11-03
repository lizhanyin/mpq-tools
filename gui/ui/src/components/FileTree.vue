<template>
  <div class="tree-body">
    <n-tree
      block-line
      selectable
      :data="treeData"
      :selected-keys="selectedKeys"
      :expanded-keys="expandedKeys"
      :render-label="renderLabel"
      :node-props="nodeProps"
      @update:expanded-keys="onUpdateExpanded"
    />
  </div>
</template>

<script setup lang="ts">
  import { computed } from 'vue'
  import { NTree } from 'naive-ui'
  import type { TreeOption } from 'naive-ui'
  import {
    S, searching, navigate, toggleExpanded, ensureDirLoaded,
    SYSTEM_DIR, UNNAMED_DIR,
  } from '../store'
  import { fmtBytes, folderKey, basenameOf, splitPath, isSpecialName } from '../utils'
  import type { TreeNodeDto } from '../types'

  // 根节点 + 文件夹树。节点带原始数据（node/path），供导航与展开使用。
  // 根节点始终展开（disableExpand + expandedKeys 强制包含），顶层文件夹常显。
  const ROOT_KEY = '__root__'

  /* type 而非 interface：naive 的 TreeOption 带字符串索引签名，
   * 只有对象字面量形状能获得隐式索引签名匹配。 */
  type TreeRow = {
    key: string
    node: TreeNodeDto
    path: string[]
    isLeaf: boolean
    children: TreeRow[]
  }
  type RootRow = {
    key: string
    isRoot: true
    isLeaf: boolean
    disableExpand: boolean
    children: TreeRow[]
  }

  function buildNode(d: TreeNodeDto, path: string[]): TreeRow {
    return {
      key: folderKey(path),
      node: d,
      path,
      isLeaf: false,
      children: d.dirs.map((c) => buildNode(c, [...path, c.name])),
    }
  }

  /** 搜索态：从后端搜索结果的完整路径构建只含命中分支的骨架树
   * （懒加载后前端树不完整，无法在本地过滤）。节点计数 = 该分支下
   * 的命中数；点击仍按真实路径导航（navigate 会按需加载真目录）。 */
  function buildSearchSkeleton(): TreeRow[] {
    type Skel = { name: string; node: TreeNodeDto; children: Map<string, Skel> }
    const root: Skel = { name: '', node: emptySkeletonNode(''), children: new Map() }
    const at = (parent: Skel, seg: string): Skel => {
      const lower = seg.toLowerCase()
      let child = parent.children.get(lower)
      if (!child) {
        child = { name: seg, node: emptySkeletonNode(seg), children: new Map() }
        parent.children.set(lower, child)
      }
      return child
    }
    for (const f of S.searchResults) {
      const segs = !f.name ? [UNNAMED_DIR]
        : isSpecialName(f.name) ? [SYSTEM_DIR]
        : splitPath(f.name).slice(0, -1)
      let cur = root
      for (const seg of segs) cur = at(cur, seg)
      cur.node.file_count++
    }
    const toRows = (skel: Skel, path: string[]): TreeRow[] => {
      const children = [...skel.children.values()]
        .sort((a, b) => (a.name.toLowerCase() < b.name.toLowerCase() ? -1 : 1))
      return children.map((c) => {
        const childPath = [...path, c.name]
        // 分支计数自底向上聚合（含本层命中 + 子分支）。
        const rows = toRows(c, childPath)
        c.node.file_count += rows.reduce((n, r) => n + r.node.file_count, 0)
        return { key: folderKey(childPath), node: c.node, path: childPath, isLeaf: false, children: rows }
      })
    }
    return toRows(root, [])
  }

  function emptySkeletonNode(name: string): TreeNodeDto {
    return { name, dirs: [], files: [], file_count: 0, total_size: 0, total_csize: 0, loaded: true }
  }

  const treeData = computed(() => {
    if (!S.info) return [] as (TreeRow | RootRow)[]
    let children: TreeRow[]
    if (searching.value) {
      children = buildSearchSkeleton()
    } else {
      children = S.info.tree.dirs.map((d) => buildNode(d, [d.name]))
    }
    const root: RootRow = { key: ROOT_KEY, isRoot: true, isLeaf: false, disableExpand: true, children }
    return [root]
  })

  /** 搜索时自动展开所有命中分支；平时由用户控制（根节点强制展开）。 */
  const expandedKeys = computed(() => {
    if (searching.value) {
      const keys: Array<string | number> = [ROOT_KEY]
      ;(function walk(nodes: (TreeRow | RootRow)[]) {
        for (const n of nodes) { keys.push(n.key); walk(n.children) }
      })(treeData.value)
      return keys
    }
    return S.expanded.includes(ROOT_KEY) ? S.expanded : [ROOT_KEY, ...S.expanded]
  })

  const selectedKeys = computed(() =>
    searching.value ? [] : [S.cwd.length ? folderKey(S.cwd) : ROOT_KEY])

  function renderLabel({ option }: { option: TreeOption }) {
    const o = option as TreeRow | RootRow
    if ('isRoot' in o) {
      const name = S.openPath ? basenameOf(S.openPath) : '归档根目录'
      return `🏠 ${name}`
    }
    const icon = o.node.name === SYSTEM_DIR ? '🗂'
      : o.node.name === UNNAMED_DIR ? '❔' : '📁'
    return `${icon} ${o.node.name} (${fmtBytes(o.node.file_count)})`
  }

  function nodeProps({ option }: { option: TreeOption }) {
    const o = option as TreeRow | RootRow
    const props: {
      onClick: (e: MouseEvent) => void
      onDblclick: (e: MouseEvent) => void
      title?: string
    } = {
      // 单击：进入目录；双击：展开/收缩；点展开箭头只切展开，不导航。
      onClick: (e: MouseEvent) => {
        if ((e.target as HTMLElement | null)?.closest?.('.n-tree-node-switcher')) return
        selectNode(o)
      },
      onDblclick: (e: MouseEvent) => {
        if ((e.target as HTMLElement | null)?.closest?.('.n-tree-node-switcher')) return
        toggleDir(o)
      },
    }
    // 根节点标签是文件名，悬停显示完整路径。
    if ('isRoot' in o) props.title = S.openPath || '回到归档顶层'
    return props
  }

  /** 点击文件夹：进入目录（不切展开，展开/收缩交给双击）。 */
  function selectNode(option: TreeRow | RootRow) {
    navigate('isRoot' in option ? [] : [...option.path])
  }

  /** 双击文件夹：切换展开/收缩（搜索态下展开被强制，不切换）。
   * 未加载的占位符也可切换——展开本身会触发按需加载。 */
  function toggleDir(option: TreeRow | RootRow) {
    if (!('isRoot' in option) && !searching.value
      && (!option.node.loaded || option.node.dirs.length > 0)) {
      toggleExpanded(option.key)
    }
  }

  function onUpdateExpanded(keys: Array<string | number>) {
    if (searching.value) return
    const prev = new Set(S.expanded)
    S.expanded = keys.filter((k) => k !== ROOT_KEY) as string[]
    // 新展开的分支按需加载一层（key 即小写路径段，nodeAt 大小写不敏感）。
    for (const k of S.expanded) {
      if (!prev.has(k)) void ensureDirLoaded(k.split('\\'))
    }
  }
</script>
