/* 与 Rust 后端 (gui/src/state.rs、commands.rs、imagepreview.rs) 的 DTO 一一对应。
 *
 * 序列化契约（无 serde rename_all，见 Rust 侧定义）：
 *   - 返回的 DTO 字段保持 Rust 原名 —— snake_case；
 *   - 命令参数键由 Tauri v2 自动映射为 camelCase（Rust 侧 snake_case）；
 *   - `#[serde(skip_serializing_if = "Option::is_none")]` 的字段在 JSON 中
 *     缺失，这里用 `?` 标记。
 * 修改后端字段时请同步更新本文件。 */

// -- 归档信息（state.rs） ------------------------------------------------------

/** 归档内的一条文件条目。 */
export interface EntryDto {
  /** 归档内的完整路径（未命名条目为 null）。 */
  name: string | null
  /** 列表中显示的名称（未命名时为 `<unnamed #i>`）。 */
  display: string
  size: number
  csize: number
  /** 标志简写串：E=存在 K=已加密 C=已压缩 1=单一单元 #=扇区 CRC P=补丁。 */
  flags: string
  locale: number
  source: string
  /** `<source>:<block_index>` 选择键，未命名条目的唯一标识。 */
  key: string
  /** `(attributes)` 提供的 `YYYY-MM-DD HH:MM`（UTC）时间（若有）。 */
  mtime: string | null
}

/** 归档目录树的节点（根节点 name 为空串）。 */
export interface TreeNodeDto {
  name: string
  dirs: TreeNodeDto[]
  files: EntryDto[]
  /** 此文件夹及其全部子级中的文件数。 */
  file_count: number
  total_size: number
  total_csize: number
}

/** 最近一次成功打开的归档概要。 */
export interface ArchiveInfoDto {
  path: string
  version: number
  sector_size: number
  hash_table_size: number
  block_table_size: number
  het_bet: boolean
  listfile_count: number
  warnings: string[]
  tree: TreeNodeDto
  /** 条目总数（具名 + 未命名）。 */
  entry_count: number
  /** 归档带有 `(password)` 保管库（自定义 AEAD 扩展）。 */
  protected: boolean
  /** 已受保护且尚未解锁（决定是否弹出密码输入框）。 */
  locked: boolean
}

/** open / reload / open-modified / open-repaired 操作的结果。
 *  Rust 侧所有构造点都经过 GuiState::load（state.rs）：ok=true 必带
 *  info，ok=false 必带 error——这里按判别联合如实建模。 */
export type OpenResult =
  | { ok: true; note?: string; info: ArchiveInfoDto }
  | { ok: false; note?: string; error: string }

// -- 预览 / 提取（commands.rs、imagepreview.rs） -------------------------------

/** 图片条目的载荷：直接作为 `<img src>`。 */
export interface ImageDto {
  /** 完整 data URL（`data:<mime>;base64,…`）。 */
  url: string
  mime: string
  /** 由后端解码转 PNG 显示（TGA / BLP）；false = 原样透传。 */
  converted: boolean
}

/** MDX（Warcraft III 模型）预览载荷：base64 的原始文件字节。 */
export interface ModelDto {
  data: string
}

/** preview_file 的返回。文本条目用 text；图片条目 text 为空、改用 image；
 * MDX 模型条目用 model（前端 WebGL 渲染，不可编辑）。 */
export interface PreviewDto {
  text: string
  size: number
  truncated: boolean
  /** 实际使用的文本编码（"UTF-8" / "GBK" / "UTF-16LE" / "UTF-16BE"）。 */
  encoding: string
  /** 内容按有损 UTF-8 回退（多半是二进制）——前端据此禁止编辑。 */
  binary: boolean
  image?: ImageDto
  model?: ModelDto
}

/** stage_edited_file 的返回：编辑后文本落盘的临时文件。 */
export interface StagedDto {
  /** 归档内的条目名（含子目录路径，原样传入的名字）。 */
  name: string
  /** 临时文件路径；「应用修改」时从这里读取字节。 */
  path: string
  size: number
  /** 是否存在同名的归档条目（保存将替换它）。 */
  replaces: boolean
}

/** extract 的返回。 */
export interface ExtractDto {
  ok: number
  failed: number
  messages: string[]
}

// -- 修复 -----------------------------------------------------------------------

/** 头信息诊断发现的单个问题。 */
export interface IssueDto {
  field: string
  problem: string
  /** 该问题将如何修复；无法修复时缺失。 */
  action?: string
}

/** diagnose_archive 的返回。 */
export interface DiagnoseDto {
  issues: IssueDto[]
}

// -- 修改 / 导入 / 转换 ----------------------------------------------------------

/** apply_repair / apply_modify 等操作的结果说明。 */
export interface NoteDto {
  note: string
}

/** stat_imports 返回的单个待导入项。 */
export interface ImportStatDto {
  /** 归档内名称（普通文件为基名；文件夹导入为 `文件夹名\相对路径` 或
   *  「只导入子项」时的 `相对路径`，可能还带前端传入的前缀）。 */
  name: string
  /** 源文件在磁盘上的完整路径；应用修改时从这里读取字节。 */
  path: string
  size: number
  /** 是否存在同名的归档条目。 */
  replaces: boolean
  error?: string
}

/** apply_modify 的待导入项（参数）。 */
export interface AddSpec {
  name: string
  path: string
}

/** apply_modify 的待删除项（参数）。 */
export interface DelSpec {
  /** 已知名称时按名称删除（会删除所有 locale 变体）。 */
  name?: string | null
  /** 选择键 `"<source>:<block_index>"` —— 定位未命名条目的唯一方式。 */
  key?: string | null
}

/** apply_convert 的返回：当没有内容可保存（请求的就是当前版本）时
 *  applied 为 false。 */
export interface ConvertDto {
  note: string
  applied: boolean
}
