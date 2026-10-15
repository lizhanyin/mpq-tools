//! GUI 保持打开的归档，以及发送给前端的各个 DTO。

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;

use mpq::{Archive, FileEntry, TableSource};
use mpqtool::tasks::{self, TreeNode};
use serde::Serialize;

/// 后端为已载入归档保存的全部信息。前端镜像最近一次
/// [`OpenResult`] 中收到的派生部分（目录树、警告、头信息摘要）。
#[derive(Default)]
pub struct GuiState {
    /// 当前字节来源文件的路径。
    pub path: Option<PathBuf>,
    /// 从磁盘载入的原始字节——修复 / 修改的输入。
    pub raw: Vec<u8>,
    /// 解析后的视图。打开失败时为 `None`，但 `path`/`raw` 仍会保留，
    /// 以便顺势提供修复流程（与 egui 行为一致）。
    pub archive: Option<Archive<Cursor<Vec<u8>>>>,
    /// 支撑目录树的扁平条目列表（用于 key 查找）。
    pub entries: Vec<FileEntry>,
    /// 受密码保护的归档解锁成功后的主密钥（自定义 AEAD 扩展）；
    /// 会回填给修改 / 转换使用。
    pub master_key: Option<[u8; 32]>,
    /// 最近一次应用「修复头信息」后的结果字节。
    pub repair_fixed: Option<Vec<u8>>,
    /// 最近一次应用「修改归档」后的结果字节 + 单行摘要。
    pub modify_applied: Option<(Vec<u8>, String)>,
    /// 最近一次应用「版本转换」后的结果字节 + 单行摘要。
    pub convert_applied: Option<(Vec<u8>, String)>,
}

/// 条目的选择键，跨导航保持稳定，且不受重名显示名的影响：
/// `<source>:<block_index>`。
pub fn key_of(source: TableSource, index: usize) -> String {
    format!("{}:{index}", source_str(source))
}

pub fn source_str(source: TableSource) -> &'static str {
    match source {
        TableSource::Classic => "classic",
        TableSource::Bet => "bet",
    }
}

/// 解析由 [`key_of`] 生成的键。
pub fn parse_key(key: &str) -> Option<(TableSource, usize)> {
    let (src, idx) = key.split_once(':')?;
    let source = match src {
        "classic" => TableSource::Classic,
        "bet" => TableSource::Bet,
        _ => return None,
    };
    Some((source, idx.parse().ok()?))
}

#[derive(Serialize)]
pub struct EntryDto {
    /// 归档内的完整路径（未命名条目为 `None`）。
    pub name: Option<String>,
    /// 列表中显示的名称（未命名时为 `<unnamed #i>`）。
    pub display: String,
    pub size: u32,
    pub csize: u32,
    /// 标志简写串：E=存在 K=已加密 C=已压缩 1=单一单元
    /// #=扇区 CRC P=补丁。
    pub flags: String,
    pub locale: u16,
    pub source: String,
    pub key: String,
    /// `(attributes)` 提供的 `YYYY-MM-DD HH:MM`（UTC）时间（若有）。
    pub mtime: Option<String>,
}

#[derive(Serialize)]
pub struct TreeNodeDto {
    pub name: String,
    pub dirs: Vec<TreeNodeDto>,
    pub files: Vec<EntryDto>,
    /// 此文件夹及其全部子级中的文件数。
    pub file_count: usize,
    pub total_size: u64,
    pub total_csize: u64,
}

#[derive(Serialize)]
pub struct ArchiveInfoDto {
    pub path: String,
    pub version: u32,
    pub sector_size: u32,
    pub hash_table_size: u32,
    pub block_table_size: u32,
    pub het_bet: bool,
    pub listfile_count: usize,
    pub warnings: Vec<String>,
    pub tree: TreeNodeDto,
    /// 条目总数（具名 + 未命名）。
    pub entry_count: usize,
    /// 归档带有 `(password)` 保管库（自定义 AEAD 扩展）。
    pub protected: bool,
    /// 已受保护且尚未解锁（决定是否弹出密码输入框）。
    pub locked: bool,
}

/// open / reload / open-modified / open-repaired 操作的结果。
#[derive(Serialize)]
pub struct OpenResult {
    pub ok: bool,
    /// 可选的附加日志行（如 已载入修改结果（…））。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<ArchiveInfoDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn entry_dto(e: &FileEntry, times: &HashMap<usize, u64>) -> EntryDto {
    EntryDto {
        name: e.name.clone(),
        display: e.display_name(),
        size: e.size,
        csize: e.compressed_size,
        flags: tasks::flag_summary(e.flags),
        locale: e.locale,
        source: source_str(e.source).to_string(),
        key: key_of(e.source, e.block_index),
        mtime: times
            .get(&e.block_index)
            .copied()
            .map(tasks::format_filetime),
    }
}

fn tree_dto(node: &TreeNode, times: &HashMap<usize, u64>) -> TreeNodeDto {
    TreeNodeDto {
        name: node.name.clone(),
        dirs: node.dirs.iter().map(|d| tree_dto(d, times)).collect(),
        files: node.files.iter().map(|f| entry_dto(f, times)).collect(),
        file_count: node.file_count,
        total_size: node.total_size,
        total_csize: node.total_csize,
    }
}

impl GuiState {
    /// 用来自 `path` 的 `bytes` 替换已载入的归档，与 egui 的
    /// `load_bytes` 行为一致：解析失败时仍会记录路径和原始字节，
    /// 以便立即提供修复流程。
    ///
    /// 受密码保护的归档初始处于锁定状态；如果手头有此前解锁得到的
    /// 主密钥，会重新校验一次，因此重载（或载入刚保存的修改结果）
    /// 之后归档仍保持解锁。
    pub fn load(&mut self, path: PathBuf, bytes: Vec<u8>) -> OpenResult {
        match Archive::new(Cursor::new(bytes.clone())) {
            Ok(mut archive) => {
                let protected = archive.is_password_protected();
                if protected {
                    if let Some(master) = self.master_key {
                        // 密钥已失效（归档不同 / 密码已更改）时
                        // 归档只会保持锁定状态。
                        let _ = archive.unlock_with_master_key(&master);
                    }
                } else {
                    self.master_key = None;
                }
                let locked = protected && !archive.is_unlocked();

                let entries = archive.entries();
                let tree = tasks::build_tree(&entries);
                let times = tasks::file_time_map(&archive);
                let h = archive.header();
                let info = ArchiveInfoDto {
                    path: path.display().to_string(),
                    version: archive.version().as_u32(),
                    sector_size: archive.sector_size(),
                    hash_table_size: h.hash_table_size,
                    block_table_size: h.block_table_size,
                    het_bet: archive.uses_het_bet(),
                    listfile_count: archive.file_names().len(),
                    warnings: archive.warnings().to_vec(),
                    tree: tree_dto(&tree, &times),
                    entry_count: entries.len(),
                    protected,
                    locked,
                };
                self.entries = entries;
                self.archive = Some(archive);
                self.path = Some(path);
                self.raw = bytes;
                OpenResult {
                    ok: true,
                    note: None,
                    info: Some(info),
                    error: None,
                }
            }
            Err(e) => {
                self.archive = None;
                self.entries.clear();
                self.master_key = None;
                self.path = Some(path);
                self.raw = bytes;
                OpenResult {
                    ok: false,
                    note: None,
                    info: None,
                    error: Some(e.to_string()),
                }
            }
        }
    }
}
