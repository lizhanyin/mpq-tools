/* Monaco Editor 懒加载与语言配置。
 *
 * Monaco 主包约 3–4 MB，首次打开预览时才动态加载（Vite 自动代码分割）。
 * 只挂核心编辑 worker（换行/查找等基础能力）；不需要 ts/json/css/html
 * 的 IntelliSense worker，省体积。
 * JASS/Lua 语言模块（src/lang/）也在此时动态引入，其数据
 * （common.j/blizzard.j 提取的 API 列表约 340KB）随懒加载 chunk 载入。 */

let monacoPromise = null

export function loadMonaco() {
  if (!monacoPromise) {
    monacoPromise = (async () => {
      // worker 必须在首个编辑器创建前注册。monaco-editor 的 exports 映射
      // 以 ./esm/vs/ 为根，故子路径写作 editor/editor.worker。
      const { default: EditorWorker } = await import(
        'monaco-editor/editor/editor.worker?worker'
      )
      self.MonacoEnvironment = { getWorker: () => new EditorWorker() }
      const monaco = await import('monaco-editor')
      const [{ registerJass }, { registerLua }] = await Promise.all([
        import('./lang/jass.js'),
        import('./lang/lua.js'),
      ])
      registerJass(monaco)
      registerLua(monaco)
      return monaco
    })()
  }
  return monacoPromise
}

/* 归档文件扩展名 → Monaco 语言 id（未列出的按纯文本）。 */
const LANG_BY_EXT = {
  j: 'jass', ai: 'jass',
  txt: 'plaintext', log: 'plaintext', wts: 'plaintext',
  ini: 'ini', cfg: 'ini',
  json: 'json',
  xml: 'xml',
  html: 'html', htm: 'html',
  css: 'css',
  js: 'javascript', mjs: 'javascript',
  ts: 'typescript',
  lua: 'lua',
  md: 'markdown',
  yaml: 'yaml', yml: 'yaml',
  sql: 'sql',
  c: 'cpp', h: 'cpp', cpp: 'cpp', hpp: 'cpp',
}

export function languageFor(name) {
  const m = String(name).toLowerCase().match(/\.([a-z0-9]+)$/)
  return (m && LANG_BY_EXT[m[1]]) || 'plaintext'
}
