//! IPC commands exposed to the frontend. Paths (archive, extraction dir,
//! import files, save targets) come from the Tauri file dialogs; all file
//! I/O happens here in Rust.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use mpq::{Deletion, ModifyOptions};
use mpqtool::tasks;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::{parse_key, GuiState, OpenResult};

type St<'a> = State<'a, Mutex<GuiState>>;

/// Lock the state, treating a poisoned lock (a panic in an earlier
/// command) as recoverable — the data inside is still consistent enough.
fn lock<'a>(state: &'a St<'a>) -> MutexGuard<'a, GuiState> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

/// Map a display version number (1–4) from the frontend to FormatVersion.
fn format_version(n: u32) -> Result<mpq::FormatVersion, String> {
    n.checked_sub(1)
        .and_then(|raw| mpq::FormatVersion::from_raw(raw as u16).ok())
        .ok_or_else(|| format!("未知版本 v{n}"))
}

// -- open / reload --------------------------------------------------------------

#[tauri::command]
pub fn open_archive(path: String, state: St) -> Result<OpenResult, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("读取失败：{e}"))?;
    let mut s = lock(&state);
    Ok(s.load(PathBuf::from(&path), bytes))
}

/// Re-read the current archive from disk (🔄 / F5).
#[tauri::command]
pub fn reload_archive(state: St) -> Result<OpenResult, String> {
    let mut s = lock(&state);
    let path = s.path.clone().ok_or("请先打开一个归档。")?;
    let bytes = std::fs::read(&path).map_err(|e| format!("刷新失败：{e}（当前仍显示已载入的内容）"))?;
    Ok(s.load(path, bytes))
}

// -- preview / extract ------------------------------------------------------------

#[derive(Serialize)]
pub struct PreviewDto {
    pub text: String,
    pub size: usize,
    pub truncated: bool,
}

/// Read one named file for the preview window (first 256 KiB, lossy UTF-8).
#[tauri::command]
pub fn preview_file(name: String, state: St) -> Result<PreviewDto, String> {
    const LIMIT: usize = 256 * 1024;
    let s = lock(&state);
    let Some(archive) = &s.archive else {
        return Err("未打开归档".to_string());
    };
    let data = archive.read_file(&name).map_err(|e| e.to_string())?;
    let truncated = data.len() > LIMIT;
    let slice = &data[..data.len().min(LIMIT)];
    Ok(PreviewDto {
        text: String::from_utf8_lossy(slice).to_string(),
        size: data.len(),
        truncated,
    })
}

#[derive(Serialize)]
pub struct ExtractDto {
    pub ok: usize,
    pub failed: usize,
    pub messages: Vec<String>,
}

impl From<tasks::ExtractResult> for ExtractDto {
    fn from(r: tasks::ExtractResult) -> Self {
        ExtractDto {
            ok: r.ok,
            failed: r.failed,
            messages: r.messages,
        }
    }
}

/// Extract entries by selection key into `dir`; an empty `keys` list means
/// "every named file" (提取全部).
#[tauri::command]
pub fn extract(keys: Vec<String>, dir: String, state: St) -> Result<ExtractDto, String> {
    let s = lock(&state);
    let Some(archive) = &s.archive else {
        return Err("未打开归档".to_string());
    };
    let picked: Vec<mpq::FileEntry> = if keys.is_empty() {
        s.entries.iter().filter(|e| e.name.is_some()).cloned().collect()
    } else {
        let wanted: std::collections::HashSet<(mpq::TableSource, usize)> =
            keys.iter().filter_map(|k| parse_key(k)).collect();
        s.entries
            .iter()
            .filter(|e| wanted.contains(&(e.source, e.block_index)))
            .cloned()
            .collect()
    };
    Ok(tasks::extract_entries(archive, &picked, Path::new(&dir)).into())
}

// -- repair ---------------------------------------------------------------------

#[derive(Serialize)]
pub struct IssueDto {
    pub field: String,
    pub problem: String,
    /// How the issue will be fixed; `None` when it cannot be fixed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

#[derive(Serialize)]
pub struct DiagnoseDto {
    pub issues: Vec<IssueDto>,
}

/// Diagnose header damage on the loaded raw bytes (even when the archive
/// failed to open — that is the main use case).
#[tauri::command]
pub fn diagnose_archive(state: St) -> Result<DiagnoseDto, String> {
    let s = lock(&state);
    if s.raw.is_empty() {
        return Err("请先打开一个归档。".to_string());
    }
    let report = mpq::diagnose(&s.raw).map_err(|e| format!("诊断失败：{e}"))?;
    Ok(DiagnoseDto {
        issues: report
            .issues
            .iter()
            .map(|i| IssueDto {
                field: i.field.clone(),
                problem: i.problem.clone(),
                action: i.action.clone(),
            })
            .collect(),
    })
}

#[derive(Serialize)]
pub struct NoteDto {
    pub note: String,
}

/// Apply the repair to the loaded raw bytes; the result is kept in state
/// until saved / opened / discarded.
#[tauri::command]
pub fn apply_repair(state: St) -> Result<NoteDto, String> {
    let mut s = lock(&state);
    if s.raw.is_empty() {
        return Err("请先打开一个归档。".to_string());
    }
    let mut bytes = s.raw.clone();
    let report = mpq::repair(&mut bytes, true).map_err(|e| format!("修复失败：{e}"))?;
    let unfixed = report.unfixed().len();
    let note = if report.changed {
        format!("已应用 {} 项修复；{unfixed} 项无法自动修复。", report.issues.len() - unfixed)
    } else {
        "没有需要修改的字段。".to_string()
    };
    s.repair_fixed = Some(bytes);
    Ok(NoteDto { note })
}

#[tauri::command]
pub fn save_repaired(path: String, state: St) -> Result<NoteDto, String> {
    let s = lock(&state);
    let fixed = s.repair_fixed.as_ref().ok_or("请先应用修复。")?;
    std::fs::write(&path, fixed).map_err(|e| format!("保存失败：{e}"))?;
    Ok(NoteDto {
        note: format!("已保存 {path}"),
    })
}

/// Load the repaired bytes into the UI without writing them to disk.
#[tauri::command]
pub fn open_repaired(state: St) -> Result<OpenResult, String> {
    let mut s = lock(&state);
    let fixed = s.repair_fixed.clone().ok_or("请先应用修复。")?;
    let path = s
        .path
        .as_ref()
        .map(|p| tasks::repaired_path(p))
        .unwrap_or_else(|| PathBuf::from("repaired.mpq"));
    Ok(s.load(path, fixed))
}

/// Drop the pending repair result (window closed).
#[tauri::command]
pub fn discard_repair(state: St) {
    lock(&state).repair_fixed = None;
}

// -- import / delete -------------------------------------------------------------

#[derive(Serialize)]
pub struct ImportStatDto {
    /// In-archive name (base name of the source file).
    pub name: String,
    pub size: u64,
    /// Whether an existing archive entry has the same name.
    pub replaces: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Inspect files chosen for import: name, size and whether they would
/// replace an existing entry. File contents are read at 应用修改 time.
#[tauri::command]
pub fn stat_imports(paths: Vec<String>, state: St) -> Vec<ImportStatDto> {
    let s = lock(&state);
    paths
        .iter()
        .map(|p| {
            let path = Path::new(p);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let (size, error) = match std::fs::metadata(path) {
                Ok(m) => (m.len(), None),
                Err(e) => (0, Some(e.to_string())),
            };
            let replaces = s
                .archive
                .as_ref()
                .is_some_and(|a| a.find_file(&name).is_some());
            ImportStatDto {
                name,
                size,
                replaces,
                error,
            }
        })
        .collect()
}

#[derive(Deserialize)]
pub struct AddSpec {
    pub name: String,
    pub path: String,
}

#[derive(Deserialize)]
pub struct DelSpec {
    /// Delete-by-name when known (deletes every locale variant).
    #[serde(default)]
    pub name: Option<String>,
    /// Selection key `"<source>:<block_index>"` — the only way to address
    /// unnamed entries.
    #[serde(default)]
    pub key: Option<String>,
}

/// Apply the pending import / delete list to the loaded raw bytes; the
/// result is kept in state until saved / opened / discarded.
#[tauri::command]
pub fn apply_modify(
    additions: Vec<AddSpec>,
    deletions: Vec<DelSpec>,
    reencrypt: bool,
    state: St,
) -> Result<NoteDto, String> {
    let mut s = lock(&state);
    if s.raw.is_empty() {
        return Err("请先打开一个归档。".to_string());
    }

    let mut adds: Vec<(String, Vec<u8>)> = Vec::new();
    for a in &additions {
        let data = std::fs::read(&a.path).map_err(|e| format!("读取失败 {}: {e}", a.path))?;
        adds.push((a.name.clone(), data));
    }
    let dels: Vec<Deletion> = deletions
        .iter()
        .map(|d| match (&d.name, &d.key) {
            (Some(name), _) => Ok(Deletion::ByName(name.clone())),
            (None, Some(key)) => {
                let (source, index) =
                    parse_key(key).ok_or_else(|| format!("无效的表键 {key}"))?;
                Ok(Deletion::ByKey(source, index))
            }
            (None, None) => Err("删除项缺少名字或表键".to_string()),
        })
        .collect::<Result<_, String>>()?;

    let options = ModifyOptions {
        reencrypt_key_v2: reencrypt,
        ..Default::default()
    };
    let outcome =
        mpq::modify_with(&s.raw, &dels, &adds, &options).map_err(|e| format!("修改失败：{e}"))?;
    let mut summary = summarize_modify(&outcome.report);
    if !outcome.report.re_encoded.is_empty() {
        summary.push_str(if reencrypt {
            "（key-v2 已按新偏移重新加密）"
        } else {
            "（key-v2 已存为未加密）"
        });
    }
    s.modify_applied = Some((outcome.bytes, summary.clone()));
    Ok(NoteDto { note: summary })
}

/// Write the applied modification and reload the archive from the written
/// file (like the egui flow).
#[tauri::command]
pub fn save_modified(path: String, state: St) -> Result<OpenResult, String> {
    let mut s = lock(&state);
    let (bytes, _) = s.modify_applied.clone().ok_or("请先应用修改。")?;
    std::fs::write(&path, &bytes).map_err(|e| format!("保存失败：{e}"))?;
    Ok(s.load(PathBuf::from(&path), bytes))
}

/// Load the modified bytes into the UI without writing them to disk.
#[tauri::command]
pub fn open_modified(state: St) -> Result<OpenResult, String> {
    let mut s = lock(&state);
    let (bytes, summary) = s.modify_applied.clone().ok_or("请先应用修改。")?;
    let path = s.path.clone().unwrap_or_else(|| PathBuf::from("modified.mpq"));
    let mut result = s.load(path, bytes);
    result.note = Some(format!("已载入修改结果（{summary}）"));
    Ok(result)
}

/// Drop the pending modification result (window closed).
#[tauri::command]
pub fn discard_modify(state: St) {
    lock(&state).modify_applied = None;
}

/// One-line summary of a modification report (same text as the egui UI).
fn summarize_modify(report: &mpq::ModifyReport) -> String {
    let mut parts = Vec::new();
    if !report.added.is_empty() {
        parts.push(format!("新增 {}", report.added.len()));
    }
    if !report.replaced.is_empty() {
        parts.push(format!("替换 {}", report.replaced.len()));
    }
    if !report.deleted.is_empty() {
        parts.push(format!("删除 {}", report.deleted.len()));
    }
    if !report.dropped.is_empty() {
        parts.push(format!(
            "自动移除 {}",
            report
                .dropped
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("、")
        ));
    }
    if !report.re_encoded.is_empty() {
        parts.push(format!("重新编码 {}", report.re_encoded.len()));
    }
    if parts.is_empty() {
        "没有变化。".to_string()
    } else {
        parts.join("；") + "。"
    }
}

// -- create / convert ------------------------------------------------------------

/// Create a new, empty archive at `path` with the chosen header version
/// (v1–v4) and hash table size, then load it into the UI.
#[tauri::command]
pub fn create_archive(
    path: String,
    version: u32,
    hash_table_size: u32,
    state: St,
) -> Result<OpenResult, String> {
    let version = format_version(version)?;
    let options = mpq::CreateOptions {
        version,
        hash_table_size,
        ..Default::default()
    };
    let bytes = mpq::create(&options).map_err(|e| format!("创建失败：{e}"))?;
    std::fs::write(&path, &bytes).map_err(|e| format!("保存失败：{e}"))?;
    let mut s = lock(&state);
    let mut result = s.load(PathBuf::from(&path), bytes);
    if result.ok {
        result.note = Some("（新建空归档，用「文件 → 导入文件到归档…」添加内容）".to_string());
    }
    Ok(result)
}

/// Result of a 版本转换 apply: `applied` is false when there is nothing
/// to save (same-version request).
#[derive(Serialize)]
pub struct ConvertDto {
    pub note: String,
    pub applied: bool,
}

/// Convert the loaded archive's container to another header version; the
/// result is kept in state until saved / opened / discarded. Cases that
/// cannot be converted come back as errors carrying the reason.
#[tauri::command]
pub fn apply_convert(target: u32, state: St) -> Result<ConvertDto, String> {
    let mut s = lock(&state);
    if s.raw.is_empty() {
        return Err("请先打开一个归档。".to_string());
    }
    let target = format_version(target)?;

    let source = mpq::Archive::new(std::io::Cursor::new(&s.raw))
        .map_err(|e| format!("归档解析失败：{e}（可先尝试「工具 → 修复头信息」）"))?;
    let from = source.version();
    if from == target {
        return Ok(ConvertDto {
            note: "当前已是该版本，无需转换。".to_string(),
            applied: false,
        });
    }
    let het_bet = source.uses_het_bet();
    let entry_count = source.entries().len();
    drop(source);

    let outcome = mpq::convert(&s.raw, target).map_err(|e| format!("转换失败：{e}"))?;
    let mut parts = vec![format!(
        "已从 v{} 转换为 v{}（{} 个条目）",
        from.as_u32(),
        target.as_u32(),
        entry_count
    )];
    if het_bet {
        parts.push("HET/BET 表已改为经典表（文件内容不变）".to_string());
    }
    if !outcome.report.dropped.is_empty() {
        parts.push(format!(
            "自动移除 {}",
            outcome
                .report
                .dropped
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("、")
        ));
    }
    if !outcome.report.re_encoded.is_empty() {
        parts.push(format!(
            "{} 个 key-v2 文件按新偏移重新加密",
            outcome.report.re_encoded.len()
        ));
    }
    let note = parts.join("；") + "。";
    s.convert_applied = Some((outcome.bytes, note.clone()));
    Ok(ConvertDto {
        note,
        applied: true,
    })
}

/// Write the applied conversion and reload the archive from the written
/// file.
#[tauri::command]
pub fn save_converted(path: String, state: St) -> Result<OpenResult, String> {
    let mut s = lock(&state);
    let (bytes, _) = s.convert_applied.clone().ok_or("请先应用转换。")?;
    std::fs::write(&path, &bytes).map_err(|e| format!("保存失败：{e}"))?;
    Ok(s.load(PathBuf::from(&path), bytes))
}

/// Load the converted bytes into the UI without writing them to disk.
#[tauri::command]
pub fn open_converted(state: St) -> Result<OpenResult, String> {
    let mut s = lock(&state);
    let (bytes, note) = s.convert_applied.clone().ok_or("请先应用转换。")?;
    let path = s.path.clone().unwrap_or_else(|| PathBuf::from("converted.mpq"));
    let mut result = s.load(path, bytes);
    result.note = Some(format!("已载入转换结果（{note}）"));
    Ok(result)
}

/// Drop the pending conversion result (window closed).
#[tauri::command]
pub fn discard_convert(state: St) {
    lock(&state).convert_applied = None;
}
