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
    /// 老客户端 MPQ 链（贴图 / 模型回退查找用），惰性打开。
    pub war3_archives: Vec<Archive<std::fs::File>>,
    /// war3_archives 对应的客户端目录（设置变化时重新加载）。
    pub war3_dir_loaded: Option<String>,
}

/// 老客户端目录中的 MPQ，按游戏资源加载顺序（补丁优先）排列。
const WAR3_MPQ_ORDER: [&str; 5] = [
    "War3Patch.mpq",
    "War3x.mpq",
    "War3xlocal.mpq",
    "War3local.mpq",
    "War3.mpq",
];

impl GuiState {
    /// 确保来自 `dir` 的老客户端 MPQ 链已打开（目录变化时重载）。
    /// 返回链中是否有可用归档。
    pub fn ensure_war3_chain(&mut self, dir: &str) -> bool {
        if self.war3_dir_loaded.as_deref() == Some(dir) {
            return !self.war3_archives.is_empty();
        }
        self.war3_dir_loaded = Some(dir.to_string());
        self.war3_archives.clear();
        for name in WAR3_MPQ_ORDER {
            let path = std::path::Path::new(dir).join(name);
            if let Ok(archive) = Archive::open(&path) {
                self.war3_archives.push(archive);
            }
        }
        !self.war3_archives.is_empty()
    }

    /// 在老客户端 MPQ 链中查找并读取文件（按补丁优先序）。
    /// `dir` 为空或链不可用时返回 `None`。
    pub fn read_war3_bytes(&mut self, name: &str, dir: &str) -> Option<Vec<u8>> {
        if dir.is_empty() || !self.ensure_war3_chain(dir) {
            return None;
        }
        for archive in &mut self.war3_archives {
            if archive.has_file(name) {
                if let Ok(data) = archive.read_file(name) {
                    return Some(data);
                }
            }
        }
        None
    }
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

                // 存在未命名条目时，自动应用外部 listfile 清单
                //（归档旁 / 程序旁的 Listfiles 目录，MPQMaster 布局）。
                let mut extra_warnings: Vec<String> = Vec::new();
                if archive.has_unnamed_entries() {
                    let lists = tasks::discover_listfiles(&path);
                    if !lists.is_empty() {
                        let (recovered, errors) =
                            tasks::apply_listfiles(&mut archive, &lists);
                        if recovered > 0 {
                            extra_warnings
                                .push(format!("外部清单恢复 {recovered} 个文件名"));
                        }
                        for e in errors {
                            extra_warnings.push(format!("清单不可读：{e}"));
                        }
                    }
                }

                let entries = archive.entries();
                let tree = tasks::build_tree(&entries);
                let times = tasks::file_time_map(&archive);
                let h = archive.header();
                let mut warnings = archive.warnings().to_vec();
                warnings.extend(extra_warnings);
                let info = ArchiveInfoDto {
                    path: path.display().to_string(),
                    version: archive.version().as_u32(),
                    sector_size: archive.sector_size(),
                    hash_table_size: h.hash_table_size,
                    block_table_size: h.block_table_size,
                    het_bet: archive.uses_het_bet(),
                    listfile_count: archive.file_names().len(),
                    warnings,
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

#[cfg(test)]
mod war3_chain_tests {
    use super::*;

    /// 生成一个含给定文件的最小 MPQ。
    fn mini_mpq(files: &[(&str, &[u8])]) -> Vec<u8> {
        let base = mpq::create(&mpq::CreateOptions {
            version: mpq::FormatVersion::V1,
            hash_table_size: 16,
            sector_shift: 3,
        })
        .expect("create");
        let additions: Vec<(String, Vec<u8>)> = files
            .iter()
            .map(|(n, d)| (n.to_string(), d.to_vec()))
            .collect();
        mpq::modify(&base, &[], &additions).expect("modify").bytes
    }

    fn setup_client_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mpqtool-war3-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 补丁归档：同名文件覆盖基础归档（优先级更高）。
        std::fs::write(
            dir.join("War3Patch.mpq"),
            mini_mpq(&[("ReplaceableTextures\\teamcolor.blp", b"patch-version")]),
        )
        .unwrap();
        std::fs::write(
            dir.join("War3.mpq"),
            mini_mpq(&[
                ("ReplaceableTextures\\teamcolor.blp", b"base-version"),
                ("Units\\Human\\Footman.blp", b"footman"),
            ]),
        )
        .unwrap();
        dir
    }

    #[test]
    fn finds_files_with_patch_priority() {
        let dir = setup_client_dir("prio");
        let mut s = GuiState::default();
        assert!(s.ensure_war3_chain(dir.to_str().unwrap()));
        assert_eq!(s.war3_archives.len(), 2);

        // 补丁优先：同名文件取 War3Patch 的版本。
        assert_eq!(
            s.read_war3_bytes("ReplaceableTextures\\teamcolor.blp", dir.to_str().unwrap()),
            Some(b"patch-version".to_vec())
        );
        // 仅存在于基础归档的文件正常读取；hash 大小写不敏感。
        assert_eq!(
            s.read_war3_bytes("units\\human\\FOOTMAN.blp", dir.to_str().unwrap()),
            Some(b"footman".to_vec())
        );
        // 两边都没有 → None。
        assert_eq!(s.read_war3_bytes("nope.blp", dir.to_str().unwrap()), None);
    }

    #[test]
    fn reloads_when_dir_changes() {
        let dir = setup_client_dir("reload");
        let empty = std::env::temp_dir().join(format!("mpqtool-war3-empty-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        let mut s = GuiState::default();

        assert!(s.ensure_war3_chain(dir.to_str().unwrap()));
        // 换成空目录：链重载为空。
        assert!(!s.ensure_war3_chain(empty.to_str().unwrap()));
        assert!(s.war3_archives.is_empty());
        assert_eq!(
            s.read_war3_bytes("Units\\Human\\Footman.blp", empty.to_str().unwrap()),
            None
        );

        // 换回有效目录再次可用；空目录参数直接短路。
        assert!(s.ensure_war3_chain(dir.to_str().unwrap()));
        assert_eq!(
            s.read_war3_bytes("Units\\Human\\Footman.blp", dir.to_str().unwrap()),
            Some(b"footman".to_vec())
        );
        assert_eq!(s.read_war3_bytes("Units\\Human\\Footman.blp", ""), None);
    }
}
