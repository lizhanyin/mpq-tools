<template>
  <div ref="wrapEl" class="grid-scroll"
       @pointerdown="onPointerDown"
       @click.capture="onClickCapture"
       @click="onBlankClick">
    <n-virtual-list v-if="cols > 0" ref="vlRef" :items="gridRows" :item-size="ROW_H">
      <template #default="{ item }">
        <div class="grid-row" :style="{ height: ROW_H + 'px' }">
          <div v-for="(r, n) in item.items" :key="rowKey(r)" class="grid-tile"
               :class="tileClass(r)" :title="tileTitle(r)"
               :data-idx="item.start + n"
               @click="onTileClick(r, $event)"
               @dblclick="onTileOpen(r)"
               @contextmenu="onTileMenu(r, $event)">
            <div class="tile-icon" :class="{ warn: r.type === 'file' && !r.entry.name }">
              <img v-if="thumbUrl(r)" class="tile-thumb" :src="thumbUrl(r) || undefined" alt="">
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

    <!-- 框选矩形与拖拽提示浮层：绝对定位不占布局，也不响应指针。 -->
    <div v-if="band.active" class="band-rect" :style="bandStyle"></div>
    <div v-if="drag.active" class="drag-ghost" :style="{ left: drag.x + 'px', top: drag.y + 'px' }">
      移动 {{ fmtBytes(S.selected.size) }} 项{{ drag.over ? ` → ${drag.over.node.name}` : '' }}
    </div>

    <n-dropdown trigger="manual" placement="bottom-start"
                :show="ctx.show" :x="ctx.x" :y="ctx.y"
                :options="ctxOptions"
                @select="onCtxSelect" @clickoutside="ctx.show = false" />
  </div>
</template>

<script setup lang="ts">
  import { ref, reactive, computed, watch, onMounted, onBeforeUnmount } from 'vue'
  import { NVirtualList, NDropdown } from 'naive-ui'
  import type { VirtualListInst } from 'naive-ui'
  import {
    rows, selectNone, setSelectedKeys, moveKeysTo, anyModalOpen, MAX_ROWS, S,
    SYSTEM_DIR, UNNAMED_DIR,
  } from '../store'
  import type { Row } from '../store'
  import { fmtBytes, baseName, isImageName, folderKey } from '../utils'
  import { thumbFor } from '../thumbs'
  import {
    ctx, ctxOptions, onCtxSelect,
    activateRow, openRow, openContextMenu, moveCursorTo, rowSelectedState, rowKeys,
  } from './entryActions'

  // 网格单元几何尺寸（px，固定）：列数随容器宽度自适应，行高固定，
  // 这样 n-virtual-list 的 itemSize 恒定，无需逐项测量。
  const TILE_W = 120, TILE_H = 118
  const GAP_X = 8, GAP_Y = 8
  const ROW_H = TILE_H + GAP_Y
  const PAD = 6 // 网格左右留白

  // -- 列数自适应（挂载时先同步测一次，之后交给 ResizeObserver） --------------------

  const wrapEl = ref<HTMLElement | null>(null)
  const cols = ref(0)
  let ro: ResizeObserver | null = null

  function measure(w: number) {
    cols.value = Math.max(1, Math.floor((w - 2 * PAD + GAP_X) / (TILE_W + GAP_X)))
  }

  onMounted(() => {
    measure(wrapEl.value!.clientWidth) // 不依赖 RO 的初始回调（个别 WebView 不触发）
    ro = new ResizeObserver((entries) => {
      const e = entries[entries.length - 1]!
      const cbs = e.contentBoxSize
      const w = Array.isArray(cbs) ? cbs[0]?.inlineSize : undefined
      measure(w ?? e.contentRect.width)
    })
    ro.observe(wrapEl.value!)
    window.addEventListener('keydown', onKey)
  })
  onBeforeUnmount(() => {
    ro?.disconnect()
    window.removeEventListener('keydown', onKey)
    detachWindow()
    endInteraction()
  })

  /** 把行列表按列数切成「网格行」，每个虚拟项渲染一行（start = 该行在
   * 扁平行列表中的起始下标，供 data-idx 反查行对象）。 */
  const gridRows = computed(() => {
    const c = Math.max(1, cols.value)
    const out: { key: string; start: number; items: Row[] }[] = []
    const list = rows.value.list
    for (let i = 0; i < list.length; i += c) {
      out.push({ key: `r${i}`, start: i, items: list.slice(i, i + c) })
    }
    return out
  })

  // -- 单元格内容 -----------------------------------------------------------------

  function rowKey(r: Row) {
    return r.type === 'dir' ? `d:${r.path.join('\\')}` : r.entry.key
  }

  function tileClass(r: Row) {
    const st = rowSelectedState(r)
    return {
      sel: st === 'full',
      'sel-partial': st === 'partial',
      'drop-target': drag.active && drag.over === r,
    }
  }

  function tileName(r: Row) {
    if (r.type === 'dir') return r.node.name
    const e = r.entry
    return e.name ? (r.showFull ? e.name : baseName(e.name)) : e.display
  }

  function tileTitle(r: Row) {
    if (r.type === 'dir') {
      return `${r.path.join('\\')}（${fmtBytes(r.node.file_count)} 个文件）`
    }
    const e = r.entry
    if (!e.name) return `${e.display}（未命名文件无法预览）`
    return `${e.name}（${fmtBytes(e.size)} 字节）`
  }

  function tileIcon(r: Row) {
    if (r.type === 'dir') return '📁'
    if (!r.entry.name) return '⚠'
    return isImageName(r.entry.name) ? '🖼️' : '📄'
  }

  /** 图片文件的缩略图 URL（设置开启且已加载时）；否则 null 显示图标。 */
  function thumbUrl(r: Row) {
    if (r.type !== 'file' || !r.entry.name || !isImageName(r.entry.name)) return null
    return thumbFor(r.entry)
  }

  /** 点击单元格以外的空白处清空选择。 */
  function onBlankClick(ev: MouseEvent) {
    if ((ev.target as HTMLElement | null)?.closest?.('.grid-tile')) return
    selectNone()
  }

  // -- 方向键导航（↑↓←→ 移动选择，Shift 扩选，Enter 打开） --------------------------

  const vlRef = ref<VirtualListInst | null>(null)
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
  function onTileOpen(r: Row) {
    dirEnter = r.type === 'dir'
    openRow(r)
  }

  /** 单击 / 右键单元格时把光标同步到该格，方向键从它继续。 */
  function onTileClick(r: Row, ev: MouseEvent) {
    cursor.value = rows.value.list.indexOf(r)
    activateRow(r, ev)
  }

  function onTileMenu(r: Row, ev: MouseEvent) {
    cursor.value = rows.value.list.indexOf(r)
    openContextMenu(r, ev)
  }

  function onKey(ev: KeyboardEvent) {
    if (ctx.show) return
    // 预览窗开着不算「有弹窗」：焦点区在列表时方向键归网格，
    // 预览通过选择联动跟随；焦点区在预览时仍让位给预览切图。
    if (anyModalOpen.value && !(S.preview && S.focusZone === 'list')) return
    const t = ev.target as HTMLElement | null
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

  // -- 指针交互：空白处拖动 = 框选；条目上拖动 = 移动到文件夹 -----------------------
  // 不用 HTML5 拖放：Tauri 的窗口级 drag-drop 会抢占 webview 内的 DnD 事件，
  // 指针事件一套逻辑同时覆盖框选与拖拽，还能做边缘自动滚动。

  const band = reactive({ active: false, x0: 0, y0: 0, x1: 0, y1: 0, add: false, base: [] as string[] })
  // over 只会是目录行（dropAllowed 已过滤），模板据此安全访问 node.name。
  type DirRow = Extract<Row, { type: 'dir' }>
  const drag = reactive<{ active: boolean; row: Row | null; base: string[]; x: number; y: number; over: DirRow | null }>({
    active: false, row: null, base: [], x: 0, y: 0, over: null,
  })
  // 按下但尚未超过拖拽阈值的候选；超过阈值才真正开始框选/拖拽。
  let press: { row: Row | null; x: number; y: number; add: boolean; started: boolean } | null = null
  // 框选/拖拽结束后紧随的一次 click 要吞掉，否则单击选中语义会覆盖多选结果。
  let suppressClick = false

  const DRAG_THRESHOLD = 5        // 位移超过该值（px）才算拖拽，否则仍是点击
  const AUTOSCROLL_EDGE = 28      // 距滚动区上下边缘该范围内开始自动滚动
  const AUTOSCROLL_SPEED = 14     // 每次指针移动滚动的像素

  /** DOM 元素（事件目标 / elementFromPoint）对应的行对象。 */
  function rowAt(el: Element | null): Row | null {
    const tile = el?.closest('.grid-tile')
    if (!tile) return null
    return rows.value.list[Number((tile as HTMLElement).dataset?.idx)] ?? null
  }

  /** n-virtual-list 的内部滚动元素（XScrollbar 渲染的 .v-vl）。 */
  function scrollEl(): HTMLElement | null {
    return wrapEl.value?.querySelector('.n-scrollbar-container, .v-vl') ?? null
  }

  function onPointerDown(ev: PointerEvent) {
    if (ev.button !== 0 || press) return
    // 拖动滚动条是滚动，不是框选。
    if ((ev.target as HTMLElement | null)?.closest?.('.n-scrollbar-rail')) return
    press = {
      row: rowAt(ev.target as Element | null),
      x: ev.clientX,
      y: ev.clientY,
      add: ev.ctrlKey || ev.metaKey,
      started: false,
    }
    window.addEventListener('pointermove', onPointerMove)
    window.addEventListener('pointerup', onPointerUp)
    window.addEventListener('pointercancel', onPointerCancel)
    window.addEventListener('keydown', onPointerKey)
  }

  function detachWindow() {
    window.removeEventListener('pointermove', onPointerMove)
    window.removeEventListener('pointerup', onPointerUp)
    window.removeEventListener('pointercancel', onPointerCancel)
    window.removeEventListener('keydown', onPointerKey)
  }

  function onPointerMove(ev: PointerEvent) {
    if (!press) return
    const dx = ev.clientX - press.x, dy = ev.clientY - press.y
    if (!press.started) {
      if (dx * dx + dy * dy < DRAG_THRESHOLD * DRAG_THRESHOLD) return
      press.started = true
      suppressClick = true
      if (press.row) startDrag(press.row)
      else startBand()
    }
    if (band.active) updateBand(ev.clientX, ev.clientY)
    else if (drag.active) updateDrag(ev.clientX, ev.clientY)
  }

  function onPointerUp(ev: PointerEvent) {
    if (ev.button !== 0 || !press) return
    detachWindow()
    if (press.started) {
      // 拖拽结束：悬停在合法目录上才移动（框选无需收尾，选择已实时应用）。
      if (drag.active && drag.over) moveKeysTo([...S.selected], drag.over.path)
      endInteraction()
    }
    press = null
    if (suppressClick) setTimeout(() => { suppressClick = false }, 0)
  }

  /** 指针被系统取消（如触控被抢占）：直接结束交互，不执行移动。 */
  function onPointerCancel() {
    detachWindow()
    endInteraction()
    press = null
    if (suppressClick) setTimeout(() => { suppressClick = false }, 0)
  }

  function onPointerKey(ev: KeyboardEvent) {
    if (ev.key !== 'Escape' || !press?.started) return
    detachWindow()
    if (band.active) setSelectedKeys(band.base)
    else if (drag.active) setSelectedKeys(drag.base)
    endInteraction()
    press = null
    suppressClick = true
    setTimeout(() => { suppressClick = false }, 0)
  }

  function startBand() {
    if (!press) return
    band.active = true
    band.x0 = band.x1 = press.x
    band.y0 = band.y1 = press.y
    band.add = press.add
    band.base = press.add ? [...S.selected] : [] // Ctrl 框选 = 在原选择上追加
    scrollEl()?.addEventListener('scroll', onBandScroll)
  }

  function updateBand(x: number, y: number) {
    band.x1 = x
    band.y1 = y
    applyBandSelection()
    autoScroll(x, y)
  }

  /** 自动滚动后可见行变化，框选范围需要重算（矩形是视口坐标）。 */
  function onBandScroll() {
    if (band.active) applyBandSelection()
  }

  /** 选中与矩形相交的全部（已渲染的）单元格；目录格 = 其下全部文件。 */
  function applyBandSelection() {
    const wrap = wrapEl.value
    if (!wrap) return
    const l = Math.min(band.x0, band.x1), r = Math.max(band.x0, band.x1)
    const t = Math.min(band.y0, band.y1), b = Math.max(band.y0, band.y1)
    const keys: string[] = []
    for (const el of wrap.querySelectorAll('.grid-tile')) {
      const rc = el.getBoundingClientRect()
      if (rc.right <= l || rc.left >= r || rc.bottom <= t || rc.top >= b) continue
      const row = rows.value.list[Number((el as HTMLElement).dataset?.idx)]
      if (row) for (const k of rowKeys(row)) keys.push(k)
    }
    setSelectedKeys(band.add ? [...band.base, ...keys] : keys)
  }

  const bandStyle = computed(() => {
    const wrap = wrapEl.value?.getBoundingClientRect()
    if (!wrap) return {}
    return {
      left: `${Math.min(band.x0, band.x1) - wrap.left}px`,
      top: `${Math.min(band.y0, band.y1) - wrap.top}px`,
      width: `${Math.abs(band.x1 - band.x0)}px`,
      height: `${Math.abs(band.y1 - band.y0)}px`,
    }
  })

  function startDrag(row: Row) {
    if (!press) return
    // 拖动未选中的行时把选择切到该行（资源管理器语义）；已选中则拖整个选择集。
    drag.base = [...S.selected]
    if (rowSelectedState(row) !== 'full') setSelectedKeys(rowKeys(row))
    drag.active = true
    drag.row = row
    drag.x = press.x
    drag.y = press.y
    drag.over = null
    document.body.style.cursor = 'move'
  }

  function updateDrag(x: number, y: number) {
    drag.x = x
    drag.y = y
    const row = rowAt(document.elementFromPoint(x, y))
    drag.over = dropAllowed(row) ? row : null
    autoScroll(x, y)
  }

  /** 放下目标合法性：目录行（伪目录除外），且不是拖动起始目录自身或其子目录。 */
  function dropAllowed(row: Row | null): row is Extract<Row, { type: 'dir' }> {
    if (!row || row.type !== 'dir') return false
    if (row.path[0] === SYSTEM_DIR || row.path[0] === UNNAMED_DIR) return false
    if (drag.row?.type === 'dir') {
      const a = folderKey(drag.row.path), b = folderKey(row.path)
      if (a === b || b.startsWith(a + '\\')) return false
    }
    return true
  }

  function endInteraction() {
    if (band.active) {
      scrollEl()?.removeEventListener('scroll', onBandScroll)
      band.active = false
    }
    if (drag.active) {
      drag.active = false
      drag.row = null
      drag.over = null
      document.body.style.cursor = ''
    }
  }

  /** 指针接近滚动区上下边缘时自动滚动（框选与拖拽共用）。 */
  function autoScroll(x: number, y: number) {
    const el = scrollEl()
    if (!el) return
    const rc = el.getBoundingClientRect()
    if (y < rc.top + AUTOSCROLL_EDGE) el.scrollTop -= AUTOSCROLL_SPEED
    else if (y > rc.bottom - AUTOSCROLL_EDGE) el.scrollTop += AUTOSCROLL_SPEED
  }

  /** 框选/拖拽后吞掉紧随的 click（含冒泡到单元格的单击选中）。 */
  function onClickCapture(ev: MouseEvent) {
    if (suppressClick) {
      ev.stopPropagation()
      ev.preventDefault()
    }
  }
</script>
