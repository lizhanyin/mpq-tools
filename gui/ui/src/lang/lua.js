/* Lua 语言定义(覆盖内置版本)。
 *
 * 注意 Lua 存在多个常用版本(5.0 ~ 5.4,魔兽地图脚本为 5.1):词法
 * 取各版本语法的并集 —— goto/label(5.2+)、整除 //、位运算 & | ~ << >>
 * (5.3+)、局部变量属性 <const>/<close>(5.4)、长字符串 [=[...]=] 与
 * 长注释 --[[ ]]、十六进制浮点(5.2+)等全部可正确着色。
 * 标准库补全带版本标注,老版本专有 API(loadstring、unpack 等)也会给出。 */

const KEYWORDS = [
  'and', 'break', 'do', 'else', 'elseif', 'end', 'for', 'function',
  'goto', 'if', 'in', 'local', 'not', 'or', 'repeat', 'return',
  'then', 'until', 'while',
]
const CONSTANTS = ['nil', 'true', 'false']

const conf = {
  comments: { lineComment: '--', blockComment: ['--[[', ']]'] },
  brackets: [['{', '}'], ['[', ']'], ['(', ')']],
  autoClosingPairs: [
    { open: '{', close: '}' },
    { open: '[', close: ']' },
    { open: '(', close: ')' },
    { open: '"', close: '"', notIn: ['string'] },
    { open: '\'', close: '\'', notIn: ['string'] },
  ],
  surroundingPairs: [
    { open: '{', close: '}' },
    { open: '[', close: ']' },
    { open: '(', close: ')' },
    { open: '"', close: '"' },
    { open: '\'', close: '\'' },
  ],
  indentationRules: {
    increaseIndentPattern:
      /\b(?:then|do|repeat)\s*(?:--.*)?$|^\s*(?:else|elseif)\b.*$|^\s*(?:local\s+)?function\b|\bfunction\s*\([^)]*\)\s*(?:--.*)?$|[{(]\s*(?:--.*)?$/,
    decreaseIndentPattern: /^\s*(?:end|until|else|elseif|\)|\})/,
  },
}

const monarch = {
  defaultToken: '',
  tokenPostfix: '.lua',
  keywords: [...KEYWORDS, ...CONSTANTS],
  brackets: [
    { token: 'delimiter.bracket', open: '{', close: '}' },
    { token: 'delimiter.array', open: '[', close: ']' },
    { token: 'delimiter.parenthesis', open: '(', close: ')' },
  ],
  operators: [
    '+', '-', '*', '/', '//', '%', '^', '#', '&', '~', '|', '<<', '>>',
    '==', '~=', '<=', '>=', '<', '>', '=', ';', ':', ',', '.', '..', '...',
  ],
  symbols: /[=><!~?:&|+\-*/%#]+/,
  // 转义并集:基础 \n\t、\ddd(≤5.1)、\xXX 与 \z(5.2+)、\u{XXX}(5.3+)
  escapes: /\\(?:[abfnrtv\\"']|x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f]+\}|\d{1,3}|z\s*)/,
  tokenizer: {
    root: [
      [/^#!.*$/, 'comment'],
      // 标签 ::name::(5.2+)须先于 ':' 运算符
      [/::[a-zA-Z_]\w*::/, 'entity.name.tag'],
      // 局部变量属性 <const>/<close>(5.4)须先于 '<' 运算符
      [/<(?:const|close)>/, 'annotation'],
      [/[a-zA-Z_]\w*/, { cases: { '@keywords': 'keyword.$0', '@default': 'identifier' } }],
      { include: '@whitespace' },
      // 长字符串 [=[...]=] 须先于 '[' 括号
      [/\[(=*)\[/, 'string', '@longstring.$1'],
      [/(,)(\s*)([a-zA-Z_]\w*)(\s*)(:)(?!:)/, ['delimiter', '', 'key', '', 'delimiter']],
      [/({)(\s*)([a-zA-Z_]\w*)(\s*)(:)(?!:)/, ['@brackets', '', 'key', '', 'delimiter']],
      [/[{}()[\]]/, '@brackets'],
      [/@symbols/, { cases: { '@operators': 'delimiter', '@default': '' } }],
      // 十六进制(含 5.2+ 十六进制浮点/ p 指数)先于十进制
      [/0[xX][0-9a-fA-F]+(?:\.[0-9a-fA-F]*)?(?:[pP][-+]?\d+)?/, 'number.hex'],
      [/(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?/, 'number'],
      [/[;,.]/, 'delimiter'],
      // 未闭合字符串恢复
      [/"([^"\\]|\\.)*$/, 'string.invalid'],
      [/'([^'\\]|\\.)*$/, 'string.invalid'],
      [/"/, 'string', '@string."'],
      [/'/, 'string', "@string.'"],
    ],
    whitespace: [
      [/[ \t\r\n]+/, ''],
      [/--\[([=]*)\[/, 'comment', '@comment.$1'],
      [/--.*$/, 'comment'],
    ],
    comment: [
      [/[^\]]+/, 'comment'],
      [/\]([=]*)\]/, {
        cases: {
          '$1==$S2': { token: 'comment', next: '@pop' },
          '@default': 'comment',
        },
      }],
      [/./, 'comment'],
    ],
    longstring: [
      [/[^\]]+/, 'string'],
      [/\]([=]*)\]/, {
        cases: {
          '$1==$S2': { token: 'string', next: '@pop' },
          '@default': 'string',
        },
      }],
      [/./, 'string'],
    ],
    string: [
      [/@escapes/, 'string.escape'],
      [/\\./, 'string.escape.invalid'],
      [/["']/, {
        cases: {
          '$#==$S2': { token: 'string', next: '@pop' },
          '@default': 'string',
        },
      }],
      [/[^\\"]+/, 'string'],
    ],
  },
}

/* 标准库补全数据,格式 [名称, 类型(f/v), 版本标注?]。
 * 版本取并集策略:通用的不标,5.1+/5.2+/5.3+/5.4+ 标新增,
 * 已移除的标区间(如 loadstring 仅 5.0–5.1)。 */
const STDLIB = {
  '': [
    ['_G', 'v'], ['_VERSION', 'v'],
    ['assert', 'f'], ['collectgarbage', 'f'], ['dofile', 'f'], ['error', 'f'],
    ['getfenv', 'f', 'Lua 5.0–5.1(5.2 移除)'], ['getmetatable', 'f'],
    ['ipairs', 'f'], ['load', 'f'], ['loadfile', 'f'],
    ['loadstring', 'f', 'Lua 5.0–5.1(5.2 并入 load)'],
    ['module', 'f', '仅 Lua 5.1'],
    ['next', 'f'], ['pairs', 'f'], ['pcall', 'f'], ['print', 'f'],
    ['rawequal', 'f'], ['rawget', 'f'], ['rawset', 'f'],
    ['rawlen', 'f', 'Lua 5.2+'],
    ['require', 'f'], ['select', 'f', 'Lua 5.1+'],
    ['setfenv', 'f', 'Lua 5.0–5.1(5.2 移除)'], ['setmetatable', 'f'],
    ['tonumber', 'f'], ['tostring', 'f'], ['type', 'f'],
    ['unpack', 'f', 'Lua 5.0–5.1(5.2 起为 table.unpack)'],
    ['xpcall', 'f'], ['gcinfo', 'f', '仅 Lua 5.0'],
  ],
  string: [
    ['byte', 'f'], ['char', 'f'], ['find', 'f'], ['format', 'f'], ['gsub', 'f'],
    ['len', 'f'], ['lower', 'f'], ['rep', 'f'], ['sub', 'f'], ['upper', 'f'],
    ['gfind', 'f', 'Lua 5.0(gmatch 前身)'],
    ['gmatch', 'f'], ['match', 'f', 'Lua 5.1+'],
    ['pack', 'f', 'Lua 5.3+'], ['packsize', 'f', 'Lua 5.3+'],
    ['unpack', 'f', 'Lua 5.3+'],
  ],
  table: [
    ['concat', 'f'], ['insert', 'f'], ['remove', 'f'], ['sort', 'f'],
    ['foreach', 'f', 'Lua 5.0–5.1(5.2 移除)'],
    ['foreachi', 'f', 'Lua 5.0–5.1(5.2 移除)'],
    ['getn', 'f', 'Lua 5.0–5.1(5.2 移除,# 替代)'],
    ['setn', 'f', 'Lua 5.0–5.1(5.2 移除)'],
    ['maxn', 'f', '仅 Lua 5.1'],
    ['unpack', 'f', 'Lua 5.2+(原全局 unpack)'],
    ['move', 'f', 'Lua 5.3+'],
  ],
  math: [
    ['pi', 'v'], ['huge', 'v', 'Lua 5.1+'],
    ['abs', 'f'], ['acos', 'f'], ['asin', 'f'], ['atan', 'f'], ['ceil', 'f'],
    ['cos', 'f'], ['deg', 'f'], ['exp', 'f'], ['floor', 'f'], ['fmod', 'f'],
    ['log', 'f'], ['max', 'f'], ['min', 'f'], ['rad', 'f'], ['random', 'f'],
    ['randomseed', 'f'], ['sin', 'f'], ['sqrt', 'f'], ['tan', 'f'],
    ['atan2', 'f', 'Lua 5.0–5.2(5.3 起用 atan 双参)'],
    ['pow', 'f', 'Lua 5.0–5.2(5.3 起用 ^)'],
    ['frexp', 'f', 'Lua 5.0–5.2'], ['ldexp', 'f', 'Lua 5.0–5.2'],
    ['log10', 'f', 'Lua 5.0–5.2(5.3 起用 log(x,10))'],
    ['mod', 'f', '仅 Lua 5.0(fmod 前身)'],
    ['type', 'f', 'Lua 5.3+'], ['tointeger', 'f', 'Lua 5.3+'],
    ['maxinteger', 'v', 'Lua 5.3+'], ['mininteger', 'v', 'Lua 5.3+'],
  ],
  os: [
    ['clock', 'f'], ['date', 'f'], ['difftime', 'f'], ['time', 'f'],
    ['execute', 'f'], ['exit', 'f'], ['getenv', 'f'], ['setlocale', 'f'],
    ['remove', 'f'], ['rename', 'f'], ['tmpname', 'f'],
  ],
  io: [
    ['close', 'f'], ['flush', 'f'], ['input', 'f'], ['lines', 'f'],
    ['open', 'f'], ['output', 'f'], ['popen', 'f'], ['read', 'f'],
    ['seek', 'f'], ['tmpfile', 'f'], ['type', 'f'], ['write', 'f'],
  ],
  coroutine: [
    ['create', 'f'], ['resume', 'f'], ['status', 'f'], ['wrap', 'f'], ['yield', 'f'],
    ['isyieldable', 'f', 'Lua 5.3+'], ['close', 'f', 'Lua 5.4+'],
  ],
  debug: [
    ['debug', 'f'], ['gethook', 'f'], ['getinfo', 'f'], ['getlocal', 'f'],
    ['getmetatable', 'f'], ['getupvalue', 'f'], ['sethook', 'f'],
    ['setlocal', 'f'], ['setmetatable', 'f'], ['setupvalue', 'f'], ['traceback', 'f'],
    ['getregistry', 'f', 'Lua 5.1+'],
    ['getuservalue', 'f', 'Lua 5.2+'], ['setuservalue', 'f', 'Lua 5.2+'],
    ['upvalueid', 'f', 'Lua 5.2+'], ['upvaluejoin', 'f', 'Lua 5.2+'],
  ],
  // 整个 utf8 库自 Lua 5.3 起提供
  utf8: [
    ['char', 'f', 'Lua 5.3+ 整库'], ['charpattern', 'v', 'Lua 5.3+ 整库'],
    ['codepoint', 'f', 'Lua 5.3+ 整库'], ['codes', 'f', 'Lua 5.3+ 整库'],
    ['len', 'f', 'Lua 5.3+ 整库'], ['offset', 'f', 'Lua 5.3+ 整库'],
  ],
}

function libSuggestions(libName, K) {
  return STDLIB[libName].map(([name, kind, ver]) => ({
    label: name,
    kind: kind === 'f' ? K.Function : K.Variable,
    insertText: name,
    detail: libName === '' ? (ver || '') : `${libName}.${name}${ver ? ' · ' + ver : ''}`,
  }))
}

export function registerLua(monaco) {
  // 覆盖 Monaco 内置 Lua:语法并集 + 注释切换/缩进规则 + 版本标注补全
  monaco.languages.setLanguageConfiguration('lua', conf)
  monaco.languages.setMonarchTokensProvider('lua', monarch)
  const K = monaco.languages.CompletionItemKind
  const globalItems = [
    ...libSuggestions('', K),
    ...KEYWORDS.map((w) => ({ label: w, kind: K.Keyword, insertText: w })),
  ]
  monaco.languages.registerCompletionItemProvider('lua', {
    // 输入 string. / math. 等成员时按库补全
    triggerCharacters: ['.'],
    provideCompletionItems(model, position) {
      const before = model.getValueInRange({
        startLineNumber: position.lineNumber, startColumn: 1,
        endLineNumber: position.lineNumber, endColumn: position.column,
      })
      const m = before.match(/([a-zA-Z_]\w*)\.$/)
      if (!m) {
        const word = model.getWordUntilPosition(position)
        const range = {
          startLineNumber: position.lineNumber, endLineNumber: position.lineNumber,
          startColumn: word.startColumn, endColumn: word.endColumn,
        }
        return { suggestions: globalItems.map((it) => ({ ...it, range })) }
      }
      const lib = STDLIB[m[1]]
      if (!lib) return { suggestions: [] }
      const range = {
        startLineNumber: position.lineNumber, endLineNumber: position.lineNumber,
        startColumn: position.column, endColumn: position.column,
      }
      return { suggestions: libSuggestions(m[1], K).map((it) => ({ ...it, range })) }
    },
  })
}
