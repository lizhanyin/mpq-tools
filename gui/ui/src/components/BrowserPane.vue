<template>
  <div ref="browserEl" class="browser">
    <div class="tree-panel" :style="{ width: treeWidth + 'px' }">
      <div class="tree-header">文件夹（{{ fmtBytes(rootFileCount) }} 项）</div>
      <FileTree />
    </div>
    <div class="tree-splitter" :class="{ dragging }" @pointerdown="startSplit"></div>

    <!-- 按下列表区任意位置 = 键盘焦点归列表（方向键导航网格，预览跟随选择） -->
    <div class="files-panel" @pointerdown.capture="S.focusZone = 'list'">
      <div class="toolbar">
        <n-button size="small"
                  title="选择文件加入「修改归档」的待导入列表（导入到当前文件夹）"
                  @click="importFilesDialog">导入文件</n-button>
        <n-button size="small"
                  title="递归导入所选文件夹的全部内容，归档内以该文件夹为根"
                  @click="importFolderDialog">导入文件夹</n-button>
        <n-button size="small"
                  title="在当前文件夹下新建（导入文件后保存才会写入归档）"
                  @click="startNewFolder">新建文件夹</n-button>
        <n-button size="small" :disabled="selCount.size === 0" @click="extractSelected">提取选中</n-button>
        <n-button size="small" :disabled="selCount.size === 0"
                  title="标记选中条目待删除，随后在「修改归档」窗口应用"
                  @click="deleteSelected">删除选中</n-button>
        <span class="vsep"></span>
        <div class="filter-box">
          <span class="filter-label">过滤：</span>
          <n-input :value="S.filter" size="small" clearable spellcheck="false"
                   placeholder="按全路径搜索"
                   @update:value="setFilter" />
        </div>
        <span class="sel-count">已选 {{ fmtBytes(selCount.size) }} 项</span>
        <span class="vsep"></span>
        <n-button-group size="small">
          <n-button :type="S.viewMode === 'table' ? 'primary' : 'default'"
                    title="表格视图" @click="setViewMode('table')">☰ 列表</n-button>
          <n-button :type="S.viewMode === 'grid' ? 'primary' : 'default'"
                    title="网格视图（大图标，虚拟滚动）" @click="setViewMode('grid')">▦ 网格</n-button>
        </n-button-group>
      </div>

      <div class="breadcrumb">
        <n-button size="tiny" quaternary title="重新从磁盘载入当前归档（F5）；外部修改或保存后可用来同步视图"
                  @click="reloadArchive">🔄</n-button>
        <span class="sep">|</span>

        <template v-if="searching">
          <span class="searching">搜索 “{{ S.appliedFilter.trim() }}”：{{ fmtBytes(matchCount) }} 个匹配</span>
        </template>
        <template v-else>
          <n-button size="tiny" quaternary :disabled="S.cwd.length === 0"
                    @click="navigate(S.cwd.slice(0, -1))">⬆ 上一级</n-button>
          <n-button size="tiny" quaternary @click="navigate([])">🏠</n-button>
          <template v-for="(seg, i) in S.cwd" :key="i">
            <span class="sep">›</span>
            <n-button size="tiny" quaternary class="crumb-seg"
                      @click="navigate(S.cwd.slice(0, i + 1))">{{ seg }}</n-button>
          </template>
          <span v-if="cwdNode" class="weak">
            （{{ fmtBytes(cwdNode.dirs.length) }} 个子文件夹，{{ fmtBytes(cwdNode.files.length) }} 个文件）
          </span>
        </template>
      </div>

      <FileTable v-if="S.viewMode !== 'grid'" />
      <FileGrid v-else />
    </div>
  </div>
</template>

<script setup lang="ts">
  import { ref, computed } from 'vue'
  import { NButton, NButtonGroup, NInput } from 'naive-ui'
  import FileTree from './FileTree.vue'
  import FileTable from './FileTable.vue'
  import FileGrid from './FileGrid.vue'
  import {
    S, searching, searchResults, setViewMode,
    setFilter, reloadArchive, navigate,
    extractSelected, deleteSelected, importFilesDialog, importFolderDialog, startNewFolder,
  } from '../store'
  import { fmtBytes, nodeAt } from '../utils'

  // -- 目录树宽度（可拖动分隔条） ------------------------------------------------

  const browserEl = ref<HTMLElement | null>(null)
  const treeWidth = ref(230)
  const dragging = ref(false)

  function startSplit(ev: PointerEvent) {
    dragging.value = true
    const move = (e: PointerEvent) => {
      const rect = browserEl.value!.getBoundingClientRect()
      const w = Math.min(Math.max(e.clientX - rect.left, 150), Math.max(200, rect.width * 0.6))
      treeWidth.value = w
    }
    const up = () => {
      dragging.value = false
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }

  // -- 工具栏 / 面包屑 ------------------------------------------------------------

  const selCount = S.selected // 模板中直接读 S.selected.size

  // BrowserPane 只在 S.info 存在时挂载（App.vue 的 v-else），树根必有值。
  const rootFileCount = computed(() => S.info?.tree.file_count ?? 0)
  const cwdNode = computed(() => (S.info && !searching.value) ? nodeAt(S.info.tree, S.cwd) : null)
  const matchCount = computed(() => searching.value ? searchResults.value.length : 0)
</script>
