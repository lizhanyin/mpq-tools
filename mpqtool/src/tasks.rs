//! Shared helpers used by both the CLI and the GUI.

use std::io::Read;
use std::path::{Path, PathBuf};

use mpq::{Archive, FileEntry};

/// Result of a batch extraction.
pub struct ExtractResult {
    pub ok: usize,
    pub failed: usize,
    pub messages: Vec<String>,
}

/// Extract the given entries into `out_dir`, mirroring in-archive paths.
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

/// Join an in-archive name onto the output dir, refusing path escapes.
pub fn safe_join(base: &Path, name: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    let cleaned = name.replace('/', "\\");
    for part in cleaned.split('\\') {
        if part.is_empty() || part == "." || part == ".." {
            continue; // never escape the output directory
        }
        path.push(part);
    }
    path
}

/// Short flag string for list views: E=exists K=encrypted C=compressed
/// 1=single unit #=sector CRC P=patch.
pub fn flag_summary(flags: u32) -> String {
    let mut s = String::new();
    if flags & mpq::FILE_EXISTS != 0 {
        s.push('E');
    }
    if flags & mpq::FILE_ENCRYPTED != 0 {
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

/// Load a whole file into memory.
pub fn read_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Default output name for a repaired archive: `<stem>.fixed.<ext>`.
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

// -- virtual directory tree ------------------------------------------------

/// Virtual folder that collects unnamed entries (no listfile names for them).
pub const UNNAMED_DIR: &str = "(未命名)";
/// Virtual folder that collects MPQ-internal metadata files.
pub const SYSTEM_DIR: &str = "(系统文件)";

/// MPQ has no real directories — file names are flat paths with `\`
/// separators. This tree synthesizes the folder structure a file manager
/// would show, merging folders case-insensitively (MPQ name hashing is
/// case-insensitive too).
#[derive(Default)]
pub struct TreeNode {
    /// Display name of this folder segment ("" for the root).
    pub name: String,
    /// Sub-folders, sorted case-insensitively by name.
    pub dirs: Vec<TreeNode>,
    /// Files directly inside this folder, sorted case-insensitively by name.
    pub files: Vec<FileEntry>,
    /// Number of files in this folder and everything below it.
    pub file_count: usize,
    /// Sum of uncompressed sizes over `file_count` files.
    pub total_size: u64,
    /// Sum of compressed sizes over `file_count` files.
    pub total_csize: u64,
}

/// Split an in-archive name into path segments, accepting both `\` and `/`
/// separators and dropping empty / `.` / `..` parts (same normalization as
/// [`safe_join`]).
pub fn split_path(name: &str) -> Vec<String> {
    name.replace('/', "\\")
        .split('\\')
        .filter(|p| !p.is_empty() && *p != "." && *p != "..")
        .map(|p| p.to_string())
        .collect()
}

/// Build the virtual tree over all entries. Unnamed entries land in a
/// [`UNNAMED_DIR`] folder, MPQ-internal files in a [`SYSTEM_DIR`] folder.
pub fn build_tree(entries: &[FileEntry]) -> TreeNode {
    let mut root = TreeNode::default();

    for entry in entries {
        // Path segments of the *folder* containing the file: the last
        // segment of a regular name is the file name itself, not a folder.
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

/// Sort children and aggregate sizes bottom-up.
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

/// Resolve a folder path inside the tree (case-insensitively).
pub fn node_at<'a>(root: &'a TreeNode, path: &[String]) -> Option<&'a TreeNode> {
    let mut node = root;
    for seg in path {
        let lower = seg.to_lowercase();
        node = node.dirs.iter().find(|d| d.name.to_lowercase() == lower)?;
    }
    Some(node)
}

/// Every file entry inside `node` and its sub-folders.
pub fn collect_files(node: &TreeNode) -> Vec<FileEntry> {
    let mut out = node.files.clone();
    for dir in &node.dirs {
        out.extend(collect_files(dir));
    }
    out
}

/// Selection keys of every file inside `node` and its sub-folders — like
/// [`collect_files`] but without cloning the (path-heavy) entries.
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

/// Entries whose full in-archive path contains `needle` (case-insensitive).
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

/// Whether any file below `node` matches the search needle; drives the
/// filtered tree view.
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

// -- modification times from (attributes) -----------------------------------

/// `block_index -> file_time` for archives carrying a `(attributes)` table
/// with FILETIME data. Indices line up with the table `entries()` walked
/// (classic block table, or BET records when there is no classic one).
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

/// Format a Windows FILETIME (100 ns ticks since 1601-01-01, UTC) as
/// `YYYY-MM-DD HH:MM` in UTC.
pub fn format_filetime(ft: u64) -> String {
    // Seconds between 1601-01-01 and 1970-01-01.
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

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 -> (y, m, d).
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

        // `data` merged its case variants into one folder.
        let data = tree.dirs.iter().find(|d| d.name == "data").unwrap();
        assert_eq!(data.dirs.len(), 1);
        assert_eq!(data.dirs[0].files.len(), 2);
        assert_eq!(data.files.len(), 0);

        // The special file went to the system folder.
        let sys = tree
            .dirs
            .iter()
            .find(|d| d.name == SYSTEM_DIR)
            .unwrap();
        assert_eq!(sys.files.len(), 1);
        assert_eq!(sys.files[0].name.as_deref(), Some("(listfile)"));

        assert_eq!(tree.files.len(), 1); // top.txt at root
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
        // Unix epoch: 1970-01-01 00:00 UTC.
        assert_eq!(format_filetime(116_444_736_000_000_000), "1970-01-01 00:00");
        // 2023-01-01 00:00 UTC.
        assert_eq!(format_filetime(133_170_048_000_000_000), "2023-01-01 00:00");
        // 2000-02-29 12:34 UTC (leap day).
        assert_eq!(format_filetime(125_963_012_400_000_000), "2000-02-29 12:34");
    }
}
