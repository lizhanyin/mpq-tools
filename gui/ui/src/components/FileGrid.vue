<template>
  <div ref="wrapEl" class="grid-scroll" @click="onBlankClick">
    <n-virtual-list v-if="cols > 0" ref="vlRef" :items="gridRows" :item-size="ROW_H">
      <template #default="{ item }">
        <div class="grid-row" :style="{ height: ROW_H + 'px' }">
          <div v-for="r in item.items" :key="rowKey(r)" class="grid-tile"
               :class="tileClass(r)" :title="tileTitle(r)"
               @click="onTileClick(r, $event)"
               @dblclick="onTileOpen(r)"
               @contextmenu="onTileMenu(r, $event)">
            <div class="tile-icon" :class="{ warn: r.type === 'file' && !r.entry.name }">
              <img v-if="thumbUrl(r)" class="tile-thumb" :src="thumbUrl(r)" alt="">
              <template v-else>{{ tileIcon(r) }}</template>
            </div>
            <div class="tile-name">{{ tileName(r) }}</div>
          </div>
        </div>
      </template>
    </n-virtual-list>
    <div v-if="rows.truncated" class="table-note">
      （结果过多，仅显示前 {{ fmtBytes(MAX_ROWS) }} 项，请用过滤缩小范围）
    </div>
    <n-dropdown trigger="manual" placement="bottom-start"
                :show="ctx.show" :x="ctx.x" :y="ctx.y"
                :options="ctxOptions"
                @select="onCtxSelect" @clickoutside="ctx.show = false" />
  </div>
</template>

<script setup>
  import { ref, computed, watch, onMounted, onBeforeUnmount } from 'vue'
  import { NVirtualList, NDropdown } from 'naive-ui'
  import { rows, selectNone, anyModalOpen, MAX_ROWS, S } from '../store'
  import { fmtBytes, baseName, isImageName } from '../utils'
  import { thumbFor } from '../thumbs'
  import {
    ctx, ctxOptions, onCtxSelect,
    activateRow, openRow, openContextMenu, moveCursorTo, rowSelectedState,
  } from './entryActions'

  // 网格单元几何尺寸（px，固定）：列数随容器宽度自适应，行高固定，
  // 这样 n-virtual-list 的 itemSize 恒定，无需逐项测量。
  const TILE_W = 120, TILE_H = 118
  const GAP_X = 8, GAP_Y = 8
  const ROW_H = TILE_H + GAP_Y
  const PAD = 6 // 网格左右留白

  // -- 列数自适应（挂载时先同步测一次，之后交给 ResizeObserver） --------------------

  const wrapEl = ref(null)
  const cols = ref(0)
  let ro = null

  function measure(w) {
    cols.value = Math.max(1, Math.floor((w - 2 * PAD + GAP_X) / (TILE_W + GAP_X)))
  }

  onMounted(() => {
    measure(wrapEl.value.clientWidth) // 不依赖 RO 的初始回调（个别 WebView 不触发）
    ro = new ResizeObserver((entries) => {
      const e = entries[entries.length - 1]
      measure(e.contentBoxSize?.[0]?.inlineSize ?? e.contentRect.width)
    })
    ro.observe(wrapEl.value)
    window.addEventListener('keydown', onKey)
  })
  onBeforeUnmount(() => {
    ro?.disconnect()
    window.removeEventListener('keydown', onKey)
  })

  /** 把行列表按列数切成「网格行」，每个虚拟项渲染一行。 */
  const gridRows = computed(() => {
    const c = Math.max(1, cols.value)
    const out = []
    const list = rows.value.list
    for (let i = 0; i < list.length; i += c) {
      out.push({ key: `r${i}`, items: list.slice(i, i + c) })
    }
    return out
  })

  // -- 单元格内容 -----------------------------------------------------------------

  function rowKey(r) {
    return r.type === 'dir' ? `d:${r.path.join('\\')}` : r.entry.key
  }

  function tileClass(r) {
    const st = rowSelectedState(r)
    return st === 'full' ? 'sel' : st === 'partial' ? 'sel-partial' : ''
  }

  function tileName(r) {
    if (r.type === 'dir') return r.node.name
    const e = r.entry
    return e.name ? (r.showFull ? e.name : baseName(e.name)) : e.display
  }

  function tileTitle(r) {
    if (r.type === 'dir') {
      return `${r.path.join('\\')}（${fmtBytes(r.node.file_count)} 个文件）`
    }
    const e = r.entry
    if (!e.name) return `${e.display}（未命名文件无法预览）`
    return `${e.name}（${fmtBytes(e.size)} 字节）`
  }

  function tileIcon(r) {
    if (r.type === 'dir') return '📁'
    if (!r.entry.name) return '⚠'
    return isImageName(r.entry.name) ? '🖼️' : '📄'
  }

  /** 图片文件的缩略图 URL（设置开启且已加载时）；否则 null 显示图标。 */
  function thumbUrl(r) {
    if (r.type !== 'file' || !r.entry.name || !isImageName(r.entry.name)) return null
    return thumbFor(r.entry)
  }

  /** 点击单元格以外的空白处清空选择。 */
  function onBlankClick(ev) {
    if (ev.target?.closest?.('.grid-tile')) return
    selectNone()
  }

  // -- 方向键导航（↑↓←→ 移动选择，Shift 扩选，Enter 打开） --------------------------

  const vlRef = ref(null)
  const cursor = ref(-1) // 光标在 rows.list 中的索引；-1 = 尚未定位

  // 行列表变化（导航/过滤）后光标作废，方向键重新从头开始；
  // 双击/回车进入文件夹除外：光标落到第一项，方向键从它继续。
  let dirEnter = false
  watch(() => rows.value.list, () => {
    cursor.value = dirEnter ? 0 : -1
    dirEnter = false
  })

  // 预览窗切图时（预览有焦点的方向键 / 连翻）把光标与滚动定位到该行。
  watch(() => S.gridCursor, (idx) => {
    if (idx < 0 || idx >= rows.value.list.length) return
    cursor.value = idx
    vlRef.value?.scrollTo({ index: Math.floor(idx / Math.max(1, cols.value)) })
  })

  /** 双击 / 回车打开：进入文件夹时光标落到第一项。 */
  function onTileOpen(r) {
    dirEnter = r.type === 'dir'
    openRow(r)
  }

  /** 单击 / 右键单元格时把光标同步到该格，方向键从它继续。 */
  function onTileClick(r, ev) {
    cursor.value = rows.value.list.indexOf(r)
    activateRow(r, ev)
  }

  function onTileMenu(r, ev) {
    cursor.value = rows.value.list.indexOf(r)
    openContextMenu(r, ev)
  }

  function onKey(ev) {
    if (ctx.show) return
    // 预览窗开着不算「有弹窗」：焦点区在列表时方向键归网格，
    // 预览通过选择联动跟随；焦点区在预览时仍让位给预览切图。
    if (anyModalOpen.value && !(S.preview && S.focusZone === 'list')) return
    const t = ev.target
    if (t?.closest?.('input, textarea, [contenteditable="true"]')) return
    const list = rows.value.list
    if (list.length === 0) return
    if (ev.key === 'Enter') {
      if (cursor.value >= 0) { ev.preventDefault(); onTileOpen(list[cursor.value]) }
      return
    }
    // 图标流式布局：←→ 在整列表上 ±1，↑↓ 跨一整行（cols 列）。
    const c = Math.max(1, cols.value)
    const cur = cursor.value
    let next = -1
    if (ev.key === 'ArrowRight') next = cur < 0 ? 0 : Math.min(list.length - 1, cur + 1)
    else if (ev.key === 'ArrowLeft') next = cur < 0 ? list.length - 1 : Math.max(0, cur - 1)
    else if (ev.key === 'ArrowDown') next = cur < 0 ? 0 : (cur + c < list.length ? cur + c : -1)
    else if (ev.key === 'ArrowUp') next = cur < 0 ? list.length - 1 : (cur - c >= 0 ? cur - c : -1)
    else return
    if (next < 0) return // 已在网格边缘（首行的↑/末行的↓），保留原生行为
    ev.preventDefault()
    cursor.value = next
    moveCursorTo(next, ev.shiftKey)
    // scrollTo({index}) 仅当目标行未完全可见时才滚动（vueuc 内部已判断）。
    vlRef.value?.scrollTo({ index: Math.floor(next / c) })
  }
</script>
