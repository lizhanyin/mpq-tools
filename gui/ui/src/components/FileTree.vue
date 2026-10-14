<script setup>
import { computed } from 'vue'
import { NTree } from 'naive-ui'
import {
  S, searching, needle, navigate, toggleExpanded,
  SYSTEM_DIR, UNNAMED_DIR,
} from '../store'
import { fmtBytes, folderKey, subtreeMatches } from '../utils'

// 根节点 + 过滤后的文件夹树。节点带原始数据（node/path），供导航与展开使用。
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
  return [{ key: ROOT_KEY, isRoot: true, isLeaf: false, children }]
})

/** 搜索时自动展开所有命中分支；平时由用户控制。 */
const expandedKeys = computed(() => {
  if (searching.value) {
    const keys = []
    ;(function walk(nodes) {
      for (const n of nodes) { keys.push(n.key); walk(n.children) }
    })(treeData.value)
    return keys
  }
  return S.expanded
})

const selectedKeys = computed(() =>
  searching.value ? [] : [S.cwd.length ? folderKey(S.cwd) : ROOT_KEY])

function renderLabel({ option }) {
  if (option.isRoot) return '🏠 归档根目录'
  const icon = option.node.name === SYSTEM_DIR ? '🗂'
    : option.node.name === UNNAMED_DIR ? '❔' : '📁'
  return `${icon} ${option.node.name} (${fmtBytes(option.node.file_count)})`
}

function nodeProps({ option }) {
  return option.isRoot ? { title: '回到归档顶层' } : {}
}

/** 点击文件夹：进入目录；含子目录时顺带切换展开（与 egui 行为一致）。 */
function onUpdateSelected(_keys, option) {
  if (!option || searching.value) return
  if (option.isRoot) {
    navigate([])
    return
  }
  navigate([...option.path])
  if (option.node.dirs.length) toggleExpanded(option.key)
}

function onUpdateExpanded(keys) {
  if (!searching.value) S.expanded = keys
}
</script>

<template>
  <div class="tree-body">
    <n-tree
      block-line
      virtual-scroll
      selectable
      :data="treeData"
      :selected-keys="selectedKeys"
      :expanded-keys="expandedKeys"
      :render-label="renderLabel"
      :node-props="nodeProps"
      @update:selected-keys="onUpdateSelected"
      @update:expanded-keys="onUpdateExpanded"
    />
  </div>
</template>
