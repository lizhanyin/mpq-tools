/* 网格视图的图片缩略图：按需请求后端 thumbnail 命令（解码 + 缩放到
 * 192px 的 PNG data URL），前端做缓存与并发限制；未加载完或失败时
 * 组件回退显示图标。设置「界面设置 → 图片缩略图」默认关闭。 */

import { reactive, watch } from 'vue'
import { S } from './store'
import * as ipc from './ipc'

const cache = reactive(new Map()) // key → data URL（'' = 请求中或失败）
const MAX_CACHE = 1000  // 缓存条数上限（超出淘汰最早请求的）
const MAX_ACTIVE = 3    // 最多同时解码的条目数

const queue = []
const queued = new Set()
let active = 0

/** 网格单元的缩略图 URL；不可用 / 未就绪返回 null（渲染图标）。 */
export function thumbFor(entry) {
  if (!S.gridThumbs || !entry?.name) return null
  if (!cache.has(entry.key)) enqueue(entry)
  return cache.get(entry.key) || null
}

function enqueue(entry) {
  cache.set(entry.key, '') // 占位，避免重复入队
  if (queued.has(entry.key)) return
  queued.add(entry.key)
  queue.push(entry)
  pump()
}

function pump() {
  while (active < MAX_ACTIVE && queue.length > 0) {
    const e = queue.shift()
    active++
    ipc.invoke('thumbnail', { name: e.name })
      .then((url) => {
        if (url) cache.set(e.key, url)
      })
      .catch(() => {}) // 失败保持占位 ''：本会话内不再重试，回退图标
      .finally(() => {
        active--
        queued.delete(e.key)
        pump()
      })
  }
  // Map 保持插入序：超出上限时淘汰最早请求的（再次可见会重新请求）。
  while (cache.size > MAX_CACHE) {
    cache.delete(cache.keys().next().value)
  }
}

// 打开 / 重载 / 关闭归档后旧缩略图失效，清空缓存与队列。
watch(() => S.info, () => {
  cache.clear()
  queue.length = 0
  queued.clear()
})
