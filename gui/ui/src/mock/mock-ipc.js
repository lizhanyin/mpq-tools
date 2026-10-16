/* 浏览器开发 mock（?mock）：伪造 Tauri invoke / 对话框 / 窗口 API，
 * 让 UI 可以脱离 Tauri 在纯浏览器中开发。仅 dev 模式被动态加载。 */

'use strict'

function entry(name, key, size, csize, flags, locale, source, mtime) {
  const display = name || `<unnamed #${key.split(':')[1]}>`
  return {
    name: name || null,
    display,
    size, csize, flags, locale, source, key, mtime,
  }
}

function dir(name, files, dirs) {
  return {
    name,
    files: files || [],
    dirs: dirs || [],
    file_count: (files || []).length + (dirs || []).reduce((n, d) => n + d.file_count, 0),
    total_size: (files || []).reduce((n, f) => n + f.size, 0) + (dirs || []).reduce((n, d) => n + d.total_size, 0),
    total_csize: (files || []).reduce((n, f) => n + f.csize, 0) + (dirs || []).reduce((n, d) => n + d.total_csize, 0),
  }
}

const MOCK_TREE = dir('', [
  entry('war3map.j', 'classic:0', 48213, 9120, 'EKC', 0, 'classic', '2023-01-01 08:00'),
  entry('war3map.lua', 'classic:13', 12040, 9100, 'EKC', 0, 'classic', '2023-01-01 08:00'),
  entry('war3map.w3e', 'classic:1', 12045, 8990, 'EKC#', 0, 'classic', '2023-01-01 08:00'),
], [
  dir('(系统文件)', [
    entry('(listfile)', 'classic:8', 210, 190, 'EC', 0, 'classic', null),
    entry('(attributes)', 'classic:9', 96, 96, 'E1', 0, 'classic', null),
  ]),
  dir('(未命名)', [
    entry(null, 'classic:10', 4096, 4100, 'K', 0, 'classic', null),
  ]),
  dir('Units', [], [
    dir('Human', [
      entry('Units\\Human\\Footman.def', 'classic:2', 640, 512, 'EC', 0, 'classic', '2022-11-30 19:20'),
      entry('Units\\Human\\Knight.def', 'classic:3', 710, 560, 'EC', 0, 'classic', '2022-11-30 19:21'),
    ]),
    dir('Orc', [
      entry('Units\\Orc\\Grunt.def', 'classic:4', 655, 520, 'EC', 0, 'classic', null),
    ]),
  ]),
  dir('war3mapImported', [
    entry('war3mapImported\\toast.blp', 'classic:5', 33792, 21000, 'EKC', 0, 'classic', '2024-06-15 12:30'),
    entry('war3mapImported\\readme.txt', 'classic:6', 800, 400, 'E', 0, 'classic', '2024-06-15 12:31'),
    entry('war3mapImported\\logo.png', 'classic:11', 40960, 39800, 'EK', 0, 'classic', '2024-06-15 12:32'),
    entry('war3mapImported\\floor.tga', 'classic:12', 69904, 21000, 'EKC', 0, 'classic', '2024-06-15 12:33'),
  ]),
])

const MOCK_INFO = {
  path: 'D:\\maps\\demo.w3x',
  version: 1,
  sector_size: 4096,
  hash_table_size: 1024,
  block_table_size: 14,
  het_bet: false,
  listfile_count: 11,
  warnings: ['archive_size 0x2200 与实际 0x22ac 不符，已按实际值处理'],
  tree: MOCK_TREE,
  entry_count: 14,
  protected: false,
  locked: false,
}

const MOCK_TEXT = `function config takes nothing returns nothing\n    call SetMapName("TRIGSTR_1")\nendfunction\n\n// 中文内容预览测试\n// war3map.j`

/* Lua 预览样例:刻意覆盖多版本语法(长字符串、goto 标签、<const>、
 * 位运算、整除、十六进制、长注释),用于验证 5.0+ 并集词法。 */
const MOCK_LUA = `local limit <const> = 0x7F
local name = [==[war3map lua 长字符串]==]
::retry::
if (a & 0xFF) // 2 ~= 0 then
    goto retry
end
print(string.format("%s %d", name, math.max(1, 2)))
--[[ 多行
注释 ]]`

/* 图片预览 mock：canvas 画一张渐变卡片当 data URL；tga/blp 模拟
 * 「已转换为 PNG」的解码路径。 */
const MOCK_IMAGE_MIME = {
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif',
  bmp: 'image/bmp', webp: 'image/webp', avif: 'image/avif',
  ico: 'image/x-icon', svg: 'image/svg+xml',
}

function mockImageUrl(ext) {
  const c = document.createElement('canvas')
  c.width = 220
  c.height = 140
  const g = c.getContext('2d')
  const grad = g.createLinearGradient(0, 0, 220, 140)
  grad.addColorStop(0, '#4b6cb7')
  grad.addColorStop(1, '#182848')
  g.fillStyle = grad
  g.fillRect(0, 0, 220, 140)
  g.strokeStyle = 'rgba(255,255,255,.3)'
  for (let i = 1; i < 6; i++) {
    g.beginPath(); g.moveTo(i * 36, 0); g.lineTo(i * 36, 140); g.stroke()
  }
  g.fillStyle = '#fff'
  g.font = 'bold 22px sans-serif'
  g.fillText(`.${ext}`, 14, 34)
  g.font = '14px sans-serif'
  g.fillText('mock 图片预览', 14, 60)
  return c.toDataURL('image/png')
}

function mockPreview(name) {
  const ext = (String(name).toLowerCase().match(/\.([a-z0-9]+)$/) || [])[1] || ''
  if (ext === 'tga' || ext === 'blp') {
    return {
      text: '', size: 33792 + name.length, truncated: false,
      encoding: 'UTF-8', binary: false,
      image: { url: mockImageUrl(ext), mime: 'image/png', converted: true },
    }
  }
  if (MOCK_IMAGE_MIME[ext]) {
    return {
      text: '', size: 40960 + name.length, truncated: false,
      encoding: 'UTF-8', binary: false,
      image: { url: mockImageUrl(ext), mime: MOCK_IMAGE_MIME[ext], converted: false },
    }
  }
  if (ext === 'lua') {
    return {
      text: `--- ${name} ---\n` + MOCK_LUA,
      size: name.length + MOCK_LUA.length + 120,
      truncated: false,
      encoding: 'UTF-8',
      binary: false,
    }
  }
  return {
    text: `--- ${name} ---\n` + MOCK_TEXT,
    size: name.length + MOCK_TEXT.length + 120,
    truncated: false,
    encoding: 'UTF-8',
    binary: false,
  }
}

const MOCKS = {
  open_archive: () => okOpen(),
  reload_archive: () => okOpen(),
  close_archive: () => null,
  unlock_archive: ({ password }) => {
    if (password === 'wrong') throw new Error('密码错误。')
    return { note: '已解锁：可以读取、提取和修改文件内容。' }
  },
  write_password_note: ({ archivePath, password }) => ({
    note: password
      ? `密码已记录到 ${archivePath.replace(/\.[^.\/]+$/, '')}.txt`
      : '未发现密码记录 …',
  }),
  apply_password_op: ({ op }) => ({
    note: op === 'set'
      ? '已启用密码保护（11 个文件已按扇区密封，容器转为 v4）。'
      : op === 'change' ? '已更换密码。' : '已解除密码保护（11 个文件已解密）。',
  }),
  preview_file: ({ name }) => mockPreview(name),
  stage_edited_file: ({ name, text }) => ({
    name,
    path: `C:\\Users\\dev\\AppData\\Local\\Temp\\mpqtool-edit\\${name.replace(/[\\/]/g, '_')}`,
    size: text.length,
    replaces: name === 'war3map.j',
  }),
  extract: ({ keys, dir }) => ({
    ok: keys.length || 11,
    failed: 0,
    messages: [`${dir}\\war3map.j (48213)`, `${dir}\\war3map.w3e (12045)`],
  }),
  diagnose_archive: () => ({
    issues: [
      { field: 'hash_table_pos', problem: '哈希表位置 0x7f000000 越界', action: '扫描定位到 0x4c' },
      { field: 'archive_size', problem: '归档大小 0x1000 与实际 0xac 不符', action: '改写为 0xac' },
      { field: 'block_table', problem: '表内容校验失败', action: null },
    ],
  }),
  apply_repair: () => ({ note: '已应用 2 项修复；1 项无法自动修复。' }),
  save_repaired: ({ path }) => ({ note: `已保存 ${path}` }),
  open_repaired: () => okOpen('D:\\maps\\demo.fixed.w3x'),
  discard_repair: () => null,
  stat_imports: ({ paths, prefix, childrenOnly }) => {
    const pre = prefix ? `${prefix}\\` : ''
    const out = []
    for (const p of paths) {
      // 文件夹导入：展开为递归文件列表（默认以文件夹自身为根，
      // childrenOnly 时只导入子项；键名与真实 Tauri 调用一致用 camelCase）。
      if (/war3mapImported$/i.test(p)) {
        const root = childrenOnly ? pre : `${pre}war3mapImported\\`
        out.push(
          { name: `${root}icon.blp`, path: `${p}\\icon.blp`, size: 8192, replaces: false, error: null },
          { name: `${root}ui\\panel.tga`, path: `${p}\\ui\\panel.tga`, size: 4096, replaces: false, error: null },
        )
        continue
      }
      const name = p.split(/[\\/]/).pop()
      out.push({
        name: pre + name,
        path: p,
        size: 12345,
        replaces: (pre + name).toLowerCase() === 'war3map.j',
        error: null,
      })
    }
    return out
  },
  apply_modify: () => ({ note: '新增 1；替换 1；删除 1；自动移除 (attributes)；重新编码 1（key-v2 已按新偏移重新加密）。' }),
  save_modified: ({ path }) => okOpen(path),
  open_modified: () => {
    const r = okOpen('D:\\maps\\demo.w3x')
    r.note = '已载入修改结果（新增 1；替换 1；删除 1。）'
    return r
  },
  discard_modify: () => null,
  create_archive: ({ path, version, hashTableSize }) => {
    const info = {
      ...MOCK_INFO,
      path,
      version,
      hash_table_size: hashTableSize ?? 1024,
      block_table_size: 1,
      het_bet: false,
      listfile_count: 0,
      warnings: ['no (listfile); file names unknown'],
      tree: dir('', []),
      entry_count: 0,
    }
    return { ok: true, note: '（新建空归档，用「操作 → 导入文件 / 导入文件夹」添加内容）', info, error: null }
  },
  apply_convert: ({ target }) => ({
    note: `已从 v${MOCK_INFO.version} 转换为 v${target}（11 个条目）；自动移除 (attributes)。`,
    applied: target !== MOCK_INFO.version,
  }),
  save_converted: ({ path }) => okOpen(path),
  open_converted: () => {
    const r = okOpen('D:\\maps\\demo.w3x')
    r.note = '已载入转换结果（已从 v1 转换为 v3（11 个条目）。）'
    return r
  },
  discard_convert: () => null,
}

function okOpen(path) {
  const info = { ...MOCK_INFO, tree: MOCK_TREE, path: path || MOCK_INFO.path }
  return { ok: true, note: null, info, error: null }
}

export function invoke(cmd, args) {
  return new Promise((resolve, reject) => {
    setTimeout(() => {
      try {
        const fn = MOCKS[cmd]
        if (!fn) reject(new Error(`mock: unknown command ${cmd}`))
        else resolve(fn(args || {}))
      } catch (e) {
        reject(e) // mock 里 throw 的语义与 Tauri 的 Err → reject 一致
      }
    }, 60)
  })
}

export function dialogOpen(opts) {
  if (opts && opts.directory) {
    // 「导入文件夹」返回带内容的目录，提取等其它用途保持原样。
    return Promise.resolve(opts.title && opts.title.includes('导入') ? 'D:\\import\\war3mapImported' : 'D:\\out')
  }
  if (opts && opts.multiple) return Promise.resolve(['D:\\import\\war3map.j', 'D:\\import\\newfile.txt'])
  return Promise.resolve('D:\\maps\\demo.w3x')
}

export function dialogSave(opts) {
  const p = (opts && opts.defaultPath) || 'out.w3x'
  // 绝对路径原样返回（建议路径现在带目录）；纯文件名落到 mock 目录。
  return Promise.resolve(/^[A-Za-z]:[\\/]/.test(p) ? p : `D:\\maps\\${p}`)
}

export function closeWindow() {
  window.close()
}
