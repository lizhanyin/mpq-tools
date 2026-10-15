/* 重新生成 src/lang/jass-api.js(Monaco JASS 补全数据)。
 *
 * 数据来源:https://github.com/naichabaobao/jass 的 static/common.j 与
 * static/blizzard.j(下载后传入路径):
 *
 *   curl -LO https://raw.githubusercontent.com/naichabaobao/jass/master/static/common.j
 *   curl -LO https://raw.githubusercontent.com/naichabaobao/jass/master/static/blizzard.j
 *   node scripts/gen-jass-api.mjs common.j blizzard.j
 */
import { readFileSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const [commonPath, blizzardPath] = process.argv.slice(2)
if (!commonPath || !blizzardPath) {
  console.error('用法: node scripts/gen-jass-api.mjs <common.j> <blizzard.j>')
  process.exit(1)
}
const read = (f) => readFileSync(f, 'utf8')
const stripComment = (s) => s.replace(/\/\/.*$/, '')

/* 解析 native/function 声明(允许 takes/returns 参数跨行)。*/
function collectDecls(src, kind) {
  const out = []
  let buf = null
  for (const raw of src.split(/\r?\n/)) {
    const line = stripComment(raw)
    if (buf) {
      buf.text += ' ' + line.trim()
    } else if (new RegExp(`^\\s*(?:constant\\s+)?${kind}\\s+\\w+\\s+takes\\b`).test(line)) {
      buf = { text: line.trim() }
    } else {
      continue
    }
    if (/\breturns\b/.test(buf.text)) {
      const m = buf.text.match(new RegExp(`^(?:constant\\s+)?${kind}\\s+(\\w+)\\s+takes\\s+(.*?)\\s+returns\\s+([A-Za-z_]\\w*)`))
      if (m) out.push([m[1], m[2].replace(/\s+/g, ' '), m[3]])
      buf = null
    }
  }
  return out
}

/* 解析 globals 块里的常量/全局变量。*/
function collectGlobals(src, { constantsOnly, bjOnly }) {
  const out = []
  let inGlobals = false
  for (const raw of src.split(/\r?\n/)) {
    const line = stripComment(raw)
    if (/^\s*globals\b/.test(line)) { inGlobals = true; continue }
    if (/^\s*endglobals\b/.test(line)) { inGlobals = false; continue }
    if (!inGlobals) continue
    const m = line.match(/^\s*(constant\s+)?([A-Za-z_]\w*)\s+(array\s+)?([A-Za-z_]\w*)\s*(?:=|$)/)
    if (!m) continue
    const [, isConst, type, isArray, name] = m
    if (constantsOnly && !isConst) continue
    if (bjOnly && !name.startsWith('bj_')) continue
    out.push([name, (isConst ? 'constant ' : '') + (isArray ? 'array ' : '') + type])
  }
  return out
}

const commonSrc = read(commonPath)
const blizzardSrc = read(blizzardPath)
const types = [...commonSrc.matchAll(/^type\s+(\w+)\s+extends\s+(\w+)/gm)].map((m) => [m[1], m[2]])
const natives = collectDecls(commonSrc, 'native')
const functions = collectDecls(blizzardSrc, 'function')
const constants = [
  ...collectGlobals(commonSrc, { constantsOnly: true, bjOnly: false }),
  ...collectGlobals(blizzardSrc, { constantsOnly: false, bjOnly: true }),
]

for (const [src, kind, got] of [[commonSrc, 'native', natives], [blizzardSrc, 'function', functions]]) {
  const want = (src.match(new RegExp(`^\\s*(?:constant\\s+)?${kind}\\s+\\w+\\s+takes\\b`, 'gm')) || []).length
  if (want !== got.length) console.warn(`警告: ${kind} 声明 ${want} 处,成功解析 ${got.length} 处`)
}

const fmt = (arr) => arr.map((x) => '  ' + JSON.stringify(x) + ',').join('\n')
const header = `/* JASS API 补全数据 —— 生成文件,勿手改。
 *
 * 来源: https://github.com/naichabaobao/jass (static/common.j + static/blizzard.j,
 * 即魔兽 War3 1.36 标准库,含中文注释与 @since 版本标注)。
 * 生成: ${new Date().toISOString().slice(0, 10)} — native ${natives.length} /
 * function ${functions.length} / constant ${constants.length} / type ${types.length}。
 * 结构: JASS_NATIVES/JASS_FUNCTIONS 为 [名称, 参数, 返回值]×N 平铺;
 *       JASS_CONSTANTS 为 [名称, 类型]×N;JASS_TYPES 为 [类型, 父类型]×N。
 * 重新生成: 见 scripts/gen-jass-api.mjs。 */

`

const outPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'src', 'lang', 'jass-api.js')
writeFileSync(outPath, header +
  `export const JASS_NATIVES = [\n${fmt(natives)}\n]\n\n` +
  `export const JASS_FUNCTIONS = [\n${fmt(functions)}\n]\n\n` +
  `export const JASS_CONSTANTS = [\n${fmt(constants)}\n]\n\n` +
  `export const JASS_TYPES = [\n${fmt(types)}\n]\n`)
console.log(`已生成 ${outPath} — native ${natives.length} / function ${functions.length} / constant ${constants.length} / type ${types.length}`)
