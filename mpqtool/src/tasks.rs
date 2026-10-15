//! CLI 与 GUI 共用的辅助函数。

use std::io::Read;
use std::path::{Path, PathBuf};

use mpq::{Archive, FileEntry};

/// 批量提取的结果。
pub struct ExtractResult {
    pub ok: usize,
    pub failed: usize,
    pub messages: Vec<String>,
}

/// 将给定条目提取到 `out_dir`，保持与归档内一致的路径结构。
pub fn extract_entries<R: Read + std::io::Seek>(
    archive: &Archive<R>,
    entries: &[FileEntry],
    out_dir: &Path,
) -> ExtractResult {
    let mut result = ExtractResult {
        ok: 0,
        failed: 0,
        messages: Vec::new(),
    };
    if entries.is_empty() {
        result
            .messages
            .push("没有可提取的条目（未知名文件需 --all-entries / “包含未命名”）".to_string());
        return result;
    }
    if std::fs::create_dir_all(out_dir).is_err() {
        result
            .messages
            .push(format!("无法创建输出目录 {}", out_dir.display()));
        result.failed = entries.len();
        return result;
    }

    for entry in entries {
        let display = entry.display_name();
        match archive.read_entry(entry) {
            Ok(data) => {
                let out_path = safe_join(out_dir, &display);
                if let Some(parent) = out_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                match std::fs::write(&out_path, &data) {
                    Ok(()) => {
                        result.ok += 1;
                        result
                            .messages
                            .push(format!("{} ({})", out_path.display(), data.len()));
                    }
                    Err(e) => {
                        result.failed += 1;
                        result
                            .messages
                            .push(format!("写入失败 {}: {e}", out_path.display()));
                    }
                }
            }
            Err(e) => {
                result.failed += 1;
                result.messages.push(format!("{display}: {e}"));
            }
        }
    }
    result
}

/// 将归档内的名称拼接到输出目录上，拒绝路径逃逸。
pub fn safe_join(base: &Path, name: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    let cleaned = name.replace('/', "\\");
    for part in cleaned.split('\\') {
        if part.is_empty() || part == "." || part == ".." {
            continue; // 绝不逃出输出目录
        }
        path.push(part);
    }
    path
}

/// 列表视图用的短标志串：E=存在 K=已加密 A=密码 AEAD
/// C=已压缩 1=单一单元 #=sector CRC P=补丁。
pub fn flag_summary(flags: u32) -> String {
    let mut s = String::new();
    if flags & mpq::FILE_EXISTS != 0 {
        s.push('E');
    }
    if flags & mpq::FILE_PASSWORD_AEAD != 0 {
        s.push('A');
    } else if flags & mpq::FILE_ENCRYPTED != 0 {
        s.push('K');
    }
    if flags & (mpq::FILE_COMPRESS | mpq::FILE_IMPLODE) != 0 {
        s.push('C');
    }
    if flags & mpq::FILE_SINGLE_UNIT != 0 {
        s.push('1');
    }
    if flags & mpq::FILE_SECTOR_CRC != 0 {
        s.push('#');
    }
    if flags & mpq::FILE_PATCH != 0 {
        s.push('P');
    }
    s
}

/// 将整个文件载入内存。
pub fn read_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut buf)?;
    Ok(buf)
}

/// 修复后归档的默认输出名称：`<stem>.fixed.<ext>`。
pub fn repaired_path(path: &Path) -> PathBuf {
    let mut p = path.to_path_buf();
    let ext = p
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_else(|| "mpq".to_string());
    p.set_file_name(format!(
        "{}.fixed.{}",
        p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        ext
    ));
    p
}

// -- 虚拟目录树 ------------------------------------------------

/// 收集未命名条目的虚拟文件夹（它们没有 listfile 名称）。
pub const UNNAMED_DIR: &str = "(未命名)";
/// 收集 MPQ 内部元数据文件的虚拟文件夹。
pub const SYSTEM_DIR: &str = "(系统文件)";

/// MPQ 没有真正的目录——文件名就是以 `\` 为分隔符的扁平路径。
/// 这棵树合成出文件管理器会呈现的文件夹结构，文件夹按不区分
/// 大小写的方式合并（MPQ 的名称哈希同样不区分大小写）。
#[derive(Default)]
pub struct TreeNode {
    /// 此文件夹段的显示名（根为 ""）。
    pub name: String,
    /// 子文件夹，按名称不区分大小写排序。
    pub dirs: Vec<TreeNode>,
    /// 直接位于此文件夹内的文件，按名称不区分大小写排序。
    pub files: Vec<FileEntry>,
    /// 此文件夹及其下所有层级的文件数量。
    pub file_count: usize,
    /// `file_count` 个文件的未压缩大小之和。
    pub total_size: u64,
    /// `file_count` 个文件的压缩后大小之和。
    pub total_csize: u64,
}

/// 将归档内的名称拆分为路径段，接受 `\` 和 `/` 两种分隔符，
/// 并丢弃空段 / `.` / `..`（与 [`safe_join`] 相同的归一化规则）。
pub fn split_path(name: &str) -> Vec<String> {
    name.replace('/', "\\")
        .split('\\')
        .filter(|p| !p.is_empty() && *p != "." && *p != "..")
        .map(|p| p.to_string())
        .collect()
}

/// 在全部条目之上构建虚拟树。未命名条目归入 [`UNNAMED_DIR`]
/// 文件夹，MPQ 内部文件归入 [`SYSTEM_DIR`] 文件夹。
pub fn build_tree(entries: &[FileEntry]) -> TreeNode {
    let mut root = TreeNode::default();

    for entry in entries {
        // 文件所在的*文件夹*的路径段：常规名称的最后一段
        // 是文件名本身，而不是文件夹。
        let dir_segments: Vec<String> = match &entry.name {
            Some(name) if mpq::SPECIAL_FILES.contains(&name.as_str()) => {
                vec![SYSTEM_DIR.to_string()]
            }
            Some(name) => {
                let mut segs = split_path(name);
                segs.pop();
                segs
            }
            None => vec![UNNAMED_DIR.to_string()],
        };
        insert(&mut root, &dir_segments, entry.clone());
    }

    finalize(&mut root);
    root
}

fn insert(node: &mut TreeNode, segments: &[String], entry: FileEntry) {
    let Some((first, rest)) = segments.split_first() else {
        node.files.push(entry);
        return;
    };
    let lower = first.to_lowercase();
    if let Some(child) = node
        .dirs
        .iter_mut()
        .find(|d| d.name.to_lowercase() == lower)
    {
        insert(child, rest, entry);
    } else {
        let mut child = TreeNode {
            name: first.clone(),
            ..TreeNode::default()
        };
        insert(&mut child, rest, entry);
        node.dirs.push(child);
    }
}

/// 对子节点排序并自底向上聚合大小。
fn finalize(node: &mut TreeNode) {
    for dir in &mut node.dirs {
        finalize(dir);
    }
    node.dirs
        .sort_by_cached_key(|d| d.name.to_lowercase());
    node.files
        .sort_by_cached_key(|f| f.display_name().to_lowercase());

    node.file_count = node.files.len();
    node.total_size = node.files.iter().map(|f| f.size as u64).sum();
    node.total_csize = node.files.iter().map(|f| f.compressed_size as u64).sum();
    for dir in &node.dirs {
        node.file_count += dir.file_count;
        node.total_size += dir.total_size;
        node.total_csize += dir.total_csize;
    }
}

/// 在树中解析文件夹路径（不区分大小写）。
pub fn node_at<'a>(root: &'a TreeNode, path: &[String]) -> Option<&'a TreeNode> {
    let mut node = root;
    for seg in path {
        let lower = seg.to_lowercase();
        node = node.dirs.iter().find(|d| d.name.to_lowercase() == lower)?;
    }
    Some(node)
}

/// `node` 及其子文件夹内的所有文件条目。
pub fn collect_files(node: &TreeNode) -> Vec<FileEntry> {
    let mut out = node.files.clone();
    for dir in &node.dirs {
        out.extend(collect_files(dir));
    }
    out
}

/// `node` 及其子文件夹内所有文件的选择键——与 [`collect_files`]
/// 类似，但不克隆（路径很占空间的）条目。
pub fn collect_keys(node: &TreeNode) -> Vec<(mpq::TableSource, usize)> {
    let mut out: Vec<_> = node
        .files
        .iter()
        .map(|f| (f.source, f.block_index))
        .collect();
    for dir in &node.dirs {
        out.extend(collect_keys(dir));
    }
    out
}

/// 归档内完整路径包含 `needle` 的条目（不区分大小写）。
pub fn search_tree(root: &TreeNode, needle: &str) -> Vec<FileEntry> {
    fn walk(node: &TreeNode, prefix: &str, needle: &str, out: &mut Vec<FileEntry>) {
        for dir in &node.dirs {
            let dir_prefix = if prefix.is_empty() {
                dir.name.clone()
            } else {
                format!("{prefix}\\{}", dir.name)
            };
            walk(dir, &dir_prefix, needle, out);
        }
        for file in &node.files {
            let full = match &file.name {
                Some(_) if prefix.is_empty() => file.display_name(),
                Some(_) => format!("{prefix}\\{}", file.display_name()),
                None => file.display_name(),
            };
            if full.to_lowercase().contains(needle) {
                out.push(file.clone());
            }
        }
    }
    let mut out = Vec::new();
    walk(root, "", needle.to_lowercase().as_str(), &mut out);
    out
}

/// `node` 之下是否有任何文件匹配搜索词；用于驱动过滤后的树视图。
pub fn subtree_matches(node: &TreeNode, needle: &str) -> bool {
    fn walk(node: &TreeNode, prefix: &str, needle: &str) -> bool {
        node.files.iter().any(|f| {
            let full = if prefix.is_empty() {
                f.display_name()
            } else {
                format!("{prefix}\\{}", f.display_name())
            };
            full.to_lowercase().contains(needle)
        }) || node.dirs.iter().any(|dir| {
            let dir_prefix = if prefix.is_empty() {
                dir.name.clone()
            } else {
                format!("{prefix}\\{}", dir.name)
            };
            walk(dir, &dir_prefix, needle)
        })
    }
    walk(node, "", needle.to_lowercase().as_str())
}

// -- 来自 (attributes) 的修改时间 -----------------------------------

/// 携带含 FILETIME 数据的 `(attributes)` 表的归档所对应的
/// `block_index -> file_time` 映射。索引与 `entries()` 遍历的表
/// 一一对应（经典块表，或没有经典块表时的 BET 记录）。
pub fn file_time_map<R: Read + std::io::Seek>(
    archive: &mpq::Archive<R>,
) -> std::collections::HashMap<usize, u64> {
    let mut map = std::collections::HashMap::new();
    if let Some(attrs) = archive.attributes() {
        for (i, &t) in attrs.file_times.iter().enumerate() {
            if t != 0 {
                map.insert(i, t);
            }
        }
    }
    map
}

/// 将 Windows FILETIME（自 1601-01-01 UTC 起的 100 纳秒刻度数）
/// 格式化为 UTC 时区的 `YYYY-MM-DD HH:MM`。
pub fn format_filetime(ft: u64) -> String {
    // 1601-01-01 与 1970-01-01 之间的秒数。
    const EPOCH_DIFF_100NS: u128 = 116_444_736_000_000_000;
    if ft < EPOCH_DIFF_100NS as u64 {
        return String::new();
    }
    let secs = ((ft as u128 - EPOCH_DIFF_100NS) / 10_000_000) as i64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        tod / 3600,
        (tod % 3600) / 60
    )
}

/// Howard Hinnant 的 `civil_from_days`：自 1970-01-01 起的天数 -> (y, m, d)。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mpq::TableSource;

    fn entry(name: Option<&str>, size: u32, csize: u32) -> FileEntry {
        FileEntry {
            name: name.map(|s| s.to_string()),
            locale: 0,
            block_index: 0,
            offset: 0,
            compressed_size: csize,
            size,
            flags: mpq::FILE_EXISTS,
            source: TableSource::Classic,
        }
    }

    #[test]
    fn tree_builds_folders_and_merges_case() {
        let entries = vec![
            entry(Some("data\\Global\\a.txt"), 10, 5),
            entry(Some("data\\global\\b.txt"), 20, 8),
            entry(Some("top.txt"), 1, 1),
            entry(Some("(listfile)"), 4, 4),
            entry(None, 7, 7),
        ];
        let tree = build_tree(&entries);

        assert_eq!(tree.dirs.len(), 3); // data, (系统文件), (未命名)
        assert_eq!(tree.file_count, 5);
        assert_eq!(tree.total_size, 42);
        assert_eq!(tree.total_csize, 25);

        // `data` 把它的大小写变体合并成了一个文件夹。
        let data = tree.dirs.iter().find(|d| d.name == "data").unwrap();
        assert_eq!(data.dirs.len(), 1);
        assert_eq!(data.dirs[0].files.len(), 2);
        assert_eq!(data.files.len(), 0);

        // 特殊文件进入了系统文件夹。
        let sys = tree
            .dirs
            .iter()
            .find(|d| d.name == SYSTEM_DIR)
            .unwrap();
        assert_eq!(sys.files.len(), 1);
        assert_eq!(sys.files[0].name.as_deref(), Some("(listfile)"));

        assert_eq!(tree.files.len(), 1); // top.txt 位于根
    }

    #[test]
    fn split_path_normalizes_separators_and_dots() {
        assert_eq!(split_path("a/b\\c"), vec!["a", "b", "c"]);
        assert_eq!(split_path("..\\a\\./b"), vec!["a", "b"]);
        assert!(split_path("..").is_empty());
    }

    #[test]
    fn node_at_resolves_case_insensitively() {
        let entries = vec![entry(Some("Data\\Global\\x.txt"), 1, 1)];
        let tree = build_tree(&entries);
        assert!(node_at(&tree, &["data".into(), "GLOBAL".into()]).is_some());
        assert!(node_at(&tree, &["nope".into()]).is_none());
    }

    #[test]
    fn search_matches_full_paths() {
        let entries = vec![
            entry(Some("data\\global\\excel\\items.txt"), 1, 1),
            entry(Some("war3map.j"), 1, 1),
        ];
        let tree = build_tree(&entries);
        let hits = search_tree(&tree, "EXCEL");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name.as_deref(), Some("data\\global\\excel\\items.txt"));
        assert!(subtree_matches(&tree, "war3map"));
        assert!(!subtree_matches(&tree, "zzz"));
    }

    #[test]
    fn filetime_formatting() {
        assert_eq!(format_filetime(0), "");
        // Unix 纪元：1970-01-01 00:00 UTC。
        assert_eq!(format_filetime(116_444_736_000_000_000), "1970-01-01 00:00");
        // 2023-01-01 00:00 UTC.
        assert_eq!(format_filetime(133_170_048_000_000_000), "2023-01-01 00:00");
        // 2000-02-29 12:34 UTC（闰日）。
        assert_eq!(format_filetime(125_963_012_400_000_000), "2000-02-29 12:34");
    }
}
