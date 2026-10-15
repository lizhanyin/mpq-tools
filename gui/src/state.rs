//! The archive held open by the GUI plus the DTOs sent to the frontend.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;

use mpq::{Archive, FileEntry, TableSource};
use mpqtool::tasks::{self, TreeNode};
use serde::Serialize;

/// Everything the backend keeps about the loaded archive. The frontend
/// mirrors the derived parts (tree, warnings, header summary) it received
/// in the last [`OpenResult`].
pub struct GuiState {
    /// Path of the file the current bytes came from.
    pub path: Option<PathBuf>,
    /// Raw bytes as loaded from disk — the input for repair / modify.
    pub raw: Vec<u8>,
    /// Parsed view. `None` after a failed open, but `path`/`raw` stay set
    /// so the repair flow can offer itself (mirrors the egui behaviour).
    pub archive: Option<Archive<Cursor<Vec<u8>>>>,
    /// Flat entry list backing the tree (used for key lookups).
    pub entries: Vec<FileEntry>,
    /// Result bytes of the last applied 修复头信息 run.
    pub repair_fixed: Option<Vec<u8>>,
    /// Result bytes + one-line summary of the last applied 修改归档 run.
    pub modify_applied: Option<(Vec<u8>, String)>,
    /// Result bytes + one-line summary of the last applied 版本转换 run.
    pub convert_applied: Option<(Vec<u8>, String)>,
}

impl Default for GuiState {
    fn default() -> Self {
        Self {
            path: None,
            raw: Vec::new(),
            archive: None,
            entries: Vec::new(),
            repair_fixed: None,
            modify_applied: None,
            convert_applied: None,
        }
    }
}

/// Selection key of an entry, stable across navigation and immune to
/// duplicate display names: `<source>:<block_index>`.
pub fn key_of(source: TableSource, index: usize) -> String {
    format!("{}:{index}", source_str(source))
}

pub fn source_str(source: TableSource) -> &'static str {
    match source {
        TableSource::Classic => "classic",
        TableSource::Bet => "bet",
    }
}

/// Parse a key produced by [`key_of`].
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
    /// Full in-archive path (`None` for unnamed entries).
    pub name: Option<String>,
    /// Name shown in listings (`<unnamed #i>` when unnamed).
    pub display: String,
    pub size: u32,
    pub csize: u32,
    /// Short flag string: E=exists K=encrypted C=compressed 1=single unit
    /// #=sector CRC P=patch.
    pub flags: String,
    pub locale: u16,
    pub source: String,
    pub key: String,
    /// `YYYY-MM-DD HH:MM` (UTC) from `(attributes)`, if present.
    pub mtime: Option<String>,
}

#[derive(Serialize)]
pub struct TreeNodeDto {
    pub name: String,
    pub dirs: Vec<TreeNodeDto>,
    pub files: Vec<EntryDto>,
    /// Number of files in this folder and everything below it.
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
    /// Total entries (named + unnamed).
    pub entry_count: usize,
}

/// Result of open / reload / open-modified / open-repaired.
#[derive(Serialize)]
pub struct OpenResult {
    pub ok: bool,
    /// Optional extra log line (e.g. 已载入修改结果（…）).
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
    /// Replace the loaded archive with `bytes` from `path`, mirroring the
    /// egui `load_bytes` behaviour: a parse failure still records path and
    /// raw bytes so the repair flow can be offered right away.
    pub fn load(&mut self, path: PathBuf, bytes: Vec<u8>) -> OpenResult {
        match Archive::new(Cursor::new(bytes.clone())) {
            Ok(archive) => {
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
