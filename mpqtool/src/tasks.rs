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
