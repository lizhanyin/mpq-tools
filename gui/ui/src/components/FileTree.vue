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
    S, searching, needle, navigate, toggleExpanded,
    SYSTEM_DIR, UNNAMED_DIR,
  } from '../store'
  import { fmtBytes, folderKey, subtreeMatches, basenameOf } from '../utils'
  import type { TreeNodeDto } from '../types'

  // 根节点 + 过滤后的文件夹树。节点带原始数据（node/path），供导航与展开使用。
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

  function buildNode(d: TreeNodeDto, path: string[]): TreeRow | null {
    if (needle.value && !subtreeMatches(d, needle.value)) return null
    return {
      key: folderKey(path),
      node: d,
      path,
      isLeaf: false,
      children: d.dirs.map((c) => buildNode(c, [...path, c.name])).filter(Boolean) as TreeRow[],
    }
  }

  const treeData = computed(() => {
    if (!S.info) return [] as (TreeRow | RootRow)[]
    const children = S.info.tree.dirs
      .map((d) => buildNode(d, [d.name]))
      .filter(Boolean) as TreeRow[]
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

  /** 双击文件夹：切换展开/收缩（搜索态下展开被强制，不切换）。 */
  function toggleDir(option: TreeRow | RootRow) {
    if (!('isRoot' in option) && option.node.dirs.length && !searching.value) toggleExpanded(option.key)
  }

  function onUpdateExpanded(keys: Array<string | number>) {
    if (!searching.value) S.expanded = keys.filter((k) => k !== ROOT_KEY) as string[]
  }
</script>
