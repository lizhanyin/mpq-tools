/* JASS / vJASS 语言定义。
 *
 * 语法关键字与 github.com/naichabaobao/jass(VS Code JASS 插件)的
 * static/jass.tmLanguage.json 对齐,涵盖原生 JASS 与 vJASS 扩展;
 * API 补全数据来自其 common.j / blizzard.j(见 jass-api.js,生成文件)。 */
import {
  JASS_NATIVES, JASS_FUNCTIONS, JASS_CONSTANTS, JASS_TYPES,
} from './jass-api.js'

/* 关键字(声明/语句/修饰符,含 vJASS)—— 对应 tmLanguage 的
 * keyword、keyword-control、storage-modifier、vjass-keyword-inline。 */
const KEYWORDS = [
  // 声明与语句
  'function', 'native', 'endfunction', 'type', 'globals', 'endglobals',
  'set', 'call', 'takes', 'returns', 'extends',
  'if', 'else', 'elseif', 'endif', 'then', 'loop', 'endloop', 'exitwhen', 'return',
  // 修饰符
  'constant', 'local', 'array', 'debug',
  'private', 'public', 'static', 'readonly', 'optional', 'stub', 'delegate',
  // vJASS
  'library', 'library_once', 'endlibrary', 'scope', 'endscope',
  'interface', 'endinterface', 'struct', 'endstruct', 'module', 'endmodule',
  'method', 'endmethod', 'initializer', 'finalizer', 'onInit', 'onDestroy',
  'hook', 'operator', 'needs', 'uses', 'requires', 'implement',
  'this', 'super', 'thistype', 'defaults', 'allocate', 'deallocate',
  'create', 'destroy', 'execute', 'evaluate', 'name', 'exists', 'size',
  'getType', 'typeid', 'key',
]

/* 常量(true/false/null/nothing)与词法运算符(and/or/not)。 */
const CONSTANTS = ['true', 'false', 'null', 'nothing']
const WORD_OPERATORS = ['and', 'or', 'not']

/* 基础类型 + common.j 中的 handle 类型树。 */
const BASE_TYPES = ['boolean', 'code', 'handle', 'integer', 'real', 'string']
const TYPES = [...BASE_TYPES, ...JASS_TYPES.map(([name]) => name)]

const conf = {
  comments: { lineComment: '//', blockComment: ['/*', '*/'] },
  brackets: [['(', ')'], ['[', ']']],
  autoClosingPairs: [
    { open: '(', close: ')' },
    { open: '"', close: '"', notIn: ['string'] },
  ],
  surroundingPairs: [['(', ')'], ['[', ']'], ['"', '"'], ['\'', '\'']],
  indentationRules: {
    increaseIndentPattern:
      /^\s*(?:globals|function|loop|struct|library|scope|method|module|interface)\b.*$|^\s*(?:if|elseif)\b.*\bthen\b.*$|^\s*else\b.*$/,
    decreaseIndentPattern:
      /^\s*(?:endglobals|endfunction|endloop|endif|else|elseif|endstruct|endlibrary|endscope|endmethod|endmodule|endinterface)\b/,
  },
}

const monarch = {
  defaultToken: '',
  tokenPostfix: '.jass',
  keywords: KEYWORDS,
  constants: CONSTANTS,
  wordOperators: WORD_OPERATORS,
  types: TYPES,
  tokenizer: {
    root: [
      // //! 预处理指令(vJASS)须先于普通行注释
      [/\/\/!.*/, 'annotation'],
      [/\/\/.*$/, 'comment'],
      [/\/\*/, 'comment', '@blockComment'],
      // rawcode 四字符码,如 'Hpal'
      [/'[a-zA-Z0-9]{4}'/, 'string'],
      [/"/, 'string', '@string'],
      // textmacro 参数 $NAME$ 须先于 $hex 数字
      [/\$[a-zA-Z][a-zA-Z0-9_]*\$/, 'variable'],
      [/\$[0-9a-fA-F]+/, 'number.hex'],
      [/0[xX][0-9a-fA-F]+/, 'number.hex'],
      [/\d+\.?\d*|\.\d+/, 'number'],
      // 函数/类型名声明高亮:consumes 下一处的标识符
      [/\b(?:function|native|method)\b/, { token: 'keyword', next: '@funcDecl' }],
      [/\btype\b/, { token: 'keyword', next: '@typeDecl' }],
      [/[a-zA-Z_]\w*/, {
        cases: {
          '@wordOperators': 'operator',
          '@constants': 'keyword',
          '@keywords': 'keyword',
          '@types': 'type',
          '@default': 'identifier',
        },
      }],
      [/[()[\]]/, '@brackets'],
      [/[=<>!+\-*/]/, 'operator'],
    ],
    blockComment: [
      [/[^/*]+/, 'comment'],
      [/\*\//, 'comment', '@pop'],
      [/[/*]/, 'comment'],
    ],
    string: [
      // WC3 字符串内颜色码与换行符
      [/\|[cC][0-9a-fA-F]{8}/, 'string.escape'],
      [/\|[nr]/, 'string.escape'],
      [/\\./, 'string.escape'],
      [/"/, 'string', '@pop'],
      [/[^\\"]+/, 'string'],
    ],
    funcDecl: [
      [/[ \t]+/, ''],
      [/[a-zA-Z_]\w*/, { token: 'entity.name.function', next: '@pop' }],
      [/[\s\S]/, { token: '', next: '@pop' }],
    ],
    typeDecl: [
      [/[ \t]+/, ''],
      [/[a-zA-Z_]\w*/, { token: 'entity.name.type', next: '@pop' }],
      [/[\s\S]/, { token: '', next: '@pop' }],
    ],
  },
}

/* 补全建议只构建一次;range 每次调用时刷新到当前光标。 */
let cachedSuggestions = null

function buildSuggestions(monaco) {
  if (cachedSuggestions) return cachedSuggestions
  const K = monaco.languages.CompletionItemKind
  const items = KEYWORDS.map((w) => ({ label: w, kind: K.Keyword, insertText: w }))
  for (const [name, parent] of JASS_TYPES) {
    items.push({ label: name, kind: K.Struct, insertText: name, detail: `type ${name} extends ${parent}` })
  }
  for (const [name, params, ret] of JASS_NATIVES) {
    items.push({ label: name, kind: K.Function, insertText: name, detail: `native · takes ${params} returns ${ret}` })
  }
  for (const [name, params, ret] of JASS_FUNCTIONS) {
    items.push({ label: name, kind: K.Function, insertText: name, detail: `blizzard.j · takes ${params} returns ${ret}` })
  }
  for (const [name, type] of JASS_CONSTANTS) {
    items.push({ label: name, kind: K.Constant, insertText: name, detail: type })
  }
  cachedSuggestions = items
  return items
}

export function registerJass(monaco) {
  monaco.languages.register({ id: 'jass', extensions: ['.j', '.ai'] })
  monaco.languages.setLanguageConfiguration('jass', conf)
  monaco.languages.setMonarchTokensProvider('jass', monarch)
  monaco.languages.registerCompletionItemProvider('jass', {
    provideCompletionItems(model, position) {
      const word = model.getWordUntilPosition(position)
      const range = {
        startLineNumber: position.lineNumber, endLineNumber: position.lineNumber,
        startColumn: word.startColumn, endColumn: word.endColumn,
      }
      const suggestions = buildSuggestions(monaco).map((it) => ({ ...it, range }))
      return { suggestions }
    },
  })
}
