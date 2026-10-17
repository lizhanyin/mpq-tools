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

<script setup>
  import { computed } from 'vue'
  import { NTree } from 'naive-ui'
  import {
    S, searching, needle, navigate, toggleExpanded,
    SYSTEM_DIR, UNNAMED_DIR,
  } from '../store'
  import { fmtBytes, folderKey, subtreeMatches, basenameOf } from '../utils'

  // 根节点 + 过滤后的文件夹树。节点带原始数据（node/path），供导航与展开使用。
  // 根节点始终展开（disableExpand + expandedKeys 强制包含），顶层文件夹常显。
  const ROOT_KEY = '__root__'

  function buildNode(d, path) {
    if (needle.value && !subtreeMatches(d, needle.value)) return null
    return {
      key: folderKey(path),
      node: d,
      path,
      isLeaf: false,
      children: d.dirs.map((c) => buildNode(c, [...path, c.name])).filter(Boolean),
    }
  }

  const treeData = computed(() => {
    if (!S.info) return []
    const children = S.info.tree.dirs
      .map((d) => buildNode(d, [d.name]))
      .filter(Boolean)
    return [{ key: ROOT_KEY, isRoot: true, isLeaf: false, disableExpand: true, children }]
  })

  /** 搜索时自动展开所有命中分支；平时由用户控制（根节点强制展开）。 */
  const expandedKeys = computed(() => {
    if (searching.value) {
      const keys = [ROOT_KEY]
      ;(function walk(nodes) {
        for (const n of nodes) { keys.push(n.key); walk(n.children) }
      })(treeData.value)
      return keys
    }
    return S.expanded.includes(ROOT_KEY) ? S.expanded : [ROOT_KEY, ...S.expanded]
  })

  const selectedKeys = computed(() =>
    searching.value ? [] : [S.cwd.length ? folderKey(S.cwd) : ROOT_KEY])

  function renderLabel({ option }) {
    if (option.isRoot) {
      const name = S.openPath ? basenameOf(S.openPath) : '归档根目录'
      return `🏠 ${name}`
    }
    const icon = option.node.name === SYSTEM_DIR ? '🗂'
      : option.node.name === UNNAMED_DIR ? '❔' : '📁'
    return `${icon} ${option.node.name} (${fmtBytes(option.node.file_count)})`
  }

  function nodeProps({ option }) {
    const props = {
      // 单击：进入目录；双击：展开/收缩；点展开箭头只切展开，不导航。
      onClick: (e) => {
        if (e.target.closest('.n-tree-node-switcher')) return
        selectNode(option)
      },
      onDblclick: (e) => {
        if (e.target.closest('.n-tree-node-switcher')) return
        toggleDir(option)
      },
    }
    // 根节点标签是文件名，悬停显示完整路径。
    if (option.isRoot) props.title = S.openPath || '回到归档顶层'
    return props
  }

  /** 点击文件夹：进入目录（不切展开，展开/收缩交给双击）。 */
  function selectNode(option) {
    navigate(option.isRoot ? [] : [...option.path])
  }

  /** 双击文件夹：切换展开/收缩（搜索态下展开被强制，不切换）。 */
  function toggleDir(option) {
    if (!option.isRoot && option.node.dirs.length && !searching.value) toggleExpanded(option.key)
  }

  function onUpdateExpanded(keys) {
    if (!searching.value) S.expanded = keys.filter((k) => k !== ROOT_KEY)
  }
</script>
