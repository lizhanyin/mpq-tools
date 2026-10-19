<template>
  <n-modal
    :show="!!S.move"
    preset="card"
    title="移动到…"
    :style="{ width: '460px', maxWidth: '94vw', borderRadius: '12px' }"
    :mask-closable="false"
    :bordered="false"
    :auto-focus="false"
    @update:show="(v: boolean) => { if (!v) closeMoveModal() }"
  >
    <template v-if="S.move">
      <div class="dialog-body">
        <div class="hint-text">
          把 {{ fmtBytes(S.move.keys.length) }} 个文件移动到所选文件夹。
          移动以「新名字待导入 + 旧条目待删除」排队，在「修改归档」窗口应用后写入归档。
        </div>
        <div class="move-target">目标：{{ S.move.target.length ? S.move.target.join('\\') : '（归档根目录）' }}</div>
        <div class="move-tree">
          <n-tree
            block-line
            selectable
            :data="treeData"
            :selected-keys="selectedKeys"
            :expanded-keys="expanded"
            :render-label="renderLabel"
            @update:selected-keys="onSelect"
            @update:expanded-keys="(keys: Array<string | number>) => { expanded = keys as string[] }"
          />
        </div>
      </div>
    </template>

    <template v-if="S.move" #action>
      <div class="dialog-actions">
        <div class="grow"></div>
        <n-button size="small" quaternary @click="closeMoveModal">取消</n-button>
        <n-button size="small" type="primary" @click="confirmMove">移动</n-button>
      </div>
    </template>
  </n-modal>
</template>

<script setup lang="ts">
  import { ref, computed, watch } from 'vue'
  import { NModal, NButton, NTree } from 'naive-ui'
  import type { TreeOption } from 'naive-ui'
  import { S, moveKeysTo, closeMoveModal, SYSTEM_DIR, UNNAMED_DIR } from '../store'
  import type { MoveState } from '../store'
  import { fmtBytes, folderKey } from '../utils'
  import type { TreeNodeDto } from '../types'

  const ROOT_KEY = '__root__'

  // 归档内的文件夹树（系统文件 / 未命名两个伪目录不能作为移动目标）。
  // type 而非 interface：naive 的 TreeOption 带字符串索引签名（同 FileTree）。
  type DirRow = {
    key: string
    path: string[]
    isLeaf: boolean
    children: DirRow[]
  }
  type RootRow = {
    key: string
    isRoot: true
    isLeaf: boolean
    children: DirRow[]
  }

  function buildNode(d: TreeNodeDto, path: string[]): DirRow {
    return {
      key: folderKey(path),
      path,
      isLeaf: false,
      children: d.dirs.map((c) => buildNode(c, [...path, c.name])),
    }
  }

  const treeData = computed(() => {
    if (!S.info) return [] as (DirRow | RootRow)[]
    const children = S.info.tree.dirs
      .filter((d) => d.name !== SYSTEM_DIR && d.name !== UNNAMED_DIR)
      .map((d) => buildNode(d, [d.name]))
    const root: RootRow = { key: ROOT_KEY, isRoot: true, isLeaf: false, children }
    return [root]
  })

  const selectedKeys = computed(() =>
    S.move ? [S.move.target.length ? folderKey(S.move.target) : ROOT_KEY] : [])

  const expanded = ref<Array<string | number>>([ROOT_KEY])

  // 弹窗打开时展开默认目标的祖先链，让它直接可见。
  watch(() => S.move, (m: MoveState | null) => {
    if (!m) return
    const keys: string[] = [ROOT_KEY]
    let k = ''
    for (const seg of m.target) {
      k = k ? `${k}\\${seg.toLowerCase()}` : seg.toLowerCase()
      keys.push(k)
    }
    expanded.value = keys
  }, { immediate: true })

  function renderLabel({ option }: { option: TreeOption }) {
    const o = option as DirRow | RootRow
    if ('isRoot' in o) return '🏠 归档根目录'
    return `📁 ${o.path[o.path.length - 1]}`
  }

  /** 选择目标（不允许取消选择：点已选节点等于维持原目标）。 */
  function onSelect(keys: Array<string | number>, opts: Array<TreeOption | null>) {
    if (!S.move || keys.length === 0) return
    const opt = opts[0] as DirRow | RootRow | null
    if (!opt) return
    S.move.target = 'isRoot' in opt ? [] : [...opt.path]
  }

  async function confirmMove() {
    const m = S.move
    if (!m) return
    closeMoveModal()
    await moveKeysTo(m.keys, m.target)
  }
</script>
