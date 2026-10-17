<template>
  <div ref="wrapEl" class="grid-scroll" @click="onBlankClick">
    <n-virtual-list v-if="cols > 0" :items="gridRows" :item-size="ROW_H">
      <template #default="{ item }">
        <div class="grid-row" :style="{ height: ROW_H + 'px' }">
          <div v-for="r in item.items" :key="rowKey(r)" class="grid-tile"
               :class="tileClass(r)" :title="tileTitle(r)"
               @click="activateRow(r, $event)"
               @dblclick="openRow(r)"
               @contextmenu="openContextMenu(r, $event)">
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
  import { ref, computed, onMounted, onBeforeUnmount } from 'vue'
  import { NVirtualList, NDropdown } from 'naive-ui'
  import { rows, selectNone, MAX_ROWS } from '../store'
  import { fmtBytes, baseName, isImageName } from '../utils'
  import { thumbFor } from '../thumbs'
  import {
    ctx, ctxOptions, onCtxSelect,
    activateRow, openRow, openContextMenu, rowSelectedState,
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
  })
  onBeforeUnmount(() => ro?.disconnect())

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
</script>
