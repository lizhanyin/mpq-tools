//! 暴露给前端的 IPC 命令。路径（归档、提取目录、导入文件、保存目标）
//! 均来自 Tauri 文件对话框；所有文件 I/O 都在 Rust 这一侧完成。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use mpq::{Deletion, ModifyOptions};
use mpqtool::tasks;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::{parse_key, GuiState, OpenResult};

type St<'a> = State<'a, Mutex<GuiState>>;

/// 锁定全局状态；若锁已中毒（之前的某条命令发生过 panic）也视为可恢复——
/// 其中的数据仍然足够一致。
fn lock<'a>(state: &'a St<'a>) -> MutexGuard<'a, GuiState> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

/// 把前端传来的显示版本号（1–4）映射为 FormatVersion。
fn format_version(n: u32) -> Result<mpq::FormatVersion, String> {
    n.checked_sub(1)
        .and_then(|raw| mpq::FormatVersion::from_raw(raw as u16).ok())
        .ok_or_else(|| format!("未知版本 v{n}"))
}

// -- 打开 / 重载 --------------------------------------------------------------

#[tauri::command]
pub fn open_archive(path: String, state: St) -> Result<OpenResult, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("读取失败：{e}"))?;
    let mut s = lock(&state);
    Ok(s.load(PathBuf::from(&path), bytes))
}

/// 从磁盘重新读取当前归档（🔄 / F5）。
#[tauri::command]
pub fn reload_archive(state: St) -> Result<OpenResult, String> {
    let mut s = lock(&state);
    let path = s.path.clone().ok_or("请先打开一个归档。")?;
    let bytes = std::fs::read(&path).map_err(|e| format!("刷新失败：{e}（当前仍显示已载入的内容）"))?;
    Ok(s.load(path, bytes))
}

// -- 密码保护（自定义 AEAD 扩展）---------------------------------

/// 针对 `(password)` 保管库校验密码，并把主密钥保存在状态中，
/// 供后续读取 / 修改使用。
#[tauri::command]
pub fn unlock_archive(password: String, state: St) -> Result<NoteDto, String> {
    let mut s = lock(&state);
    let archive = s.archive.as_mut().ok_or("未打开归档。")?;
    if !archive.is_password_protected() {
        return Err("该归档未启用密码保护。".to_string());
    }
    archive.unlock(&password).map_err(|e| match e {
        mpq::Error::PasswordMismatch => "密码错误。".to_string(),
        other => other.to_string(),
    })?;
    s.master_key = archive.master_key();
    Ok(NoteDto {
        note: "已解锁：可以读取、提取和修改加密文件。".to_string(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordOpSpec {
    /// 取值："set" | "change" | "remove"
    pub op: String,
    pub password: String,
    pub new_password: Option<String>,
}

/// 对已载入的原始字节应用密码操作；结果（与 apply_modify 一样）
/// 保留在状态中，直到保存 / 打开 / 丢弃。
#[tauri::command]
pub fn apply_password_op(spec: PasswordOpSpec, state: St) -> Result<NoteDto, String> {
    let mut s = lock(&state);
    if s.raw.is_empty() {
        return Err("请先打开一个归档。".to_string());
    }
    let note = match spec.op.as_str() {
        "set" => {
            if spec.password.is_empty() {
                return Err("密码不能为空。".to_string());
            }
            let outcome = mpq::protect(&s.raw, &spec.password, &mpq::Argon2Params::default())
                .map_err(|e| format!("设置密码失败：{e}"))?;
            // 保留新生成的主密钥，这样保存后重新载入仍保持解锁状态。
            s.master_key = outcome.master_key;
            let mut note = format!(
                "已启用密码保护（{} 个文件已按扇区密封，容器转为 v4）",
                outcome.report.encrypted
            );
            if !outcome.report.dropped.is_empty() {
                note.push_str(&format!(
                    "；自动移除 {}",
                    outcome.report.dropped.join("、")
                ));
            }
            note.push_str("。注意：第三方 MPQ 工具将无法打开该文件。");
            s.modify_applied = Some((outcome.bytes, note.clone()));
            note
        }
        "change" => {
            let new = spec
                .new_password
                .as_deref()
                .ok_or("缺少新密码。")?
                .to_string();
            if new.is_empty() {
                return Err("新密码不能为空。".to_string());
            }
            let bytes =
                mpq::change_password(&s.raw, &spec.password, &new, &mpq::Argon2Params::default())
                    .map_err(|e| format!("修改密码失败：{e}"))?;
            // 主密钥未变；s.master_key 依然有效。
            let note = "已更换密码（仅重新包装主密钥，数据未重写）。".to_string();
            s.modify_applied = Some((bytes, note.clone()));
            note
        }
        "remove" => {
            let outcome = mpq::unprotect(&s.raw, &spec.password)
                .map_err(|e| format!("解除密码保护失败：{e}"))?;
            s.master_key = None;
            let note = format!(
                "已解除密码保护（{} 个文件已解密）。",
                outcome.report.decrypted
            );
            s.modify_applied = Some((outcome.bytes, note.clone()));
            note
        }
        other => return Err(format!("未知的密码操作 {other}")),
    };
    Ok(NoteDto { note })
}

// -- 预览 / 提取 ------------------------------------------------------------

#[derive(Serialize)]
pub struct PreviewDto {
    pub text: String,
    pub size: usize,
    pub truncated: bool,
}

/// 为预览窗口读取一个具名文件（前 256 KiB，按有损 UTF-8 解码）。
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

/// 按选择键把条目提取到 `dir`；`keys` 列表为空表示
/// “全部具名文件”（提取全部）。
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

// -- 修复 ---------------------------------------------------------------------

#[derive(Serialize)]
pub struct IssueDto {
    pub field: String,
    pub problem: String,
    /// 该问题将如何修复；无法修复时为 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

#[derive(Serialize)]
pub struct DiagnoseDto {
    pub issues: Vec<IssueDto>,
}

/// 对已载入的原始字节做头信息损坏诊断（即使归档打开失败也能执行——
/// 这正是最主要的使用场景）。
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

/// 对已载入的原始字节应用修复；结果保留在状态中，
/// 直到保存 / 打开 / 丢弃。
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

/// 把修复后的字节载入界面，而不写入磁盘。
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

/// 丢弃待处理的修复结果（窗口已关闭）。
#[tauri::command]
pub fn discard_repair(state: St) {
    lock(&state).repair_fixed = None;
}

// -- 导入 / 删除 -------------------------------------------------------------

#[derive(Serialize)]
pub struct ImportStatDto {
    /// 归档内名称（源文件的基名）。
    pub name: String,
    pub size: u64,
    /// 是否存在同名的归档条目。
    pub replaces: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 检查待导入的文件：名称、大小，以及是否会替换已有条目。
/// 文件内容在应用修改时才读取。
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
    /// 已知名称时按名称删除（会删除所有 locale 变体）。
    #[serde(default)]
    pub name: Option<String>,
    /// 选择键 `"<source>:<block_index>"` —— 定位未命名条目的唯一方式。
    #[serde(default)]
    pub key: Option<String>,
}

/// 把待处理的导入 / 删除列表应用到已载入的原始字节；结果保留在
/// 状态中，直到保存 / 打开 / 丢弃。
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
        master_key: s.master_key,
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

/// 把已应用的修改写入磁盘，并从写出的文件重新载入归档
/// （与 egui 流程一致）。
#[tauri::command]
pub fn save_modified(path: String, state: St) -> Result<OpenResult, String> {
    let mut s = lock(&state);
    let (bytes, _) = s.modify_applied.clone().ok_or("请先应用修改。")?;
    std::fs::write(&path, &bytes).map_err(|e| format!("保存失败：{e}"))?;
    Ok(s.load(PathBuf::from(&path), bytes))
}

/// 把修改后的字节载入界面，而不写入磁盘。
#[tauri::command]
pub fn open_modified(state: St) -> Result<OpenResult, String> {
    let mut s = lock(&state);
    let (bytes, summary) = s.modify_applied.clone().ok_or("请先应用修改。")?;
    let path = s.path.clone().unwrap_or_else(|| PathBuf::from("modified.mpq"));
    let mut result = s.load(path, bytes);
    result.note = Some(format!("已载入修改结果（{summary}）"));
    Ok(result)
}

/// 丢弃待处理的修改结果（窗口已关闭）。
#[tauri::command]
pub fn discard_modify(state: St) {
    lock(&state).modify_applied = None;
}

/// 修改报告的单行摘要（与 egui 界面文字相同）。
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
    if report.encrypted > 0 {
        parts.push(format!("加密 {}", report.encrypted));
    }
    if report.decrypted > 0 {
        parts.push(format!("解密 {}", report.decrypted));
    }
    if parts.is_empty() {
        "没有变化。".to_string()
    } else {
        parts.join("；")
    }
}

// -- 新建 / 转换 ------------------------------------------------------------

/// 在 `path` 处新建一个空归档，使用所选的头版本（v1–v4）与哈希表
/// 大小，然后载入界面。
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

/// 版本转换的应用结果：当没有内容可保存（请求的就是当前版本）时
/// `applied` 为 false。
#[derive(Serialize)]
pub struct ConvertDto {
    pub note: String,
    pub applied: bool,
}

/// 把已载入归档的容器转换为另一种头版本；结果保留在状态中，
/// 直到保存 / 打开 / 丢弃。无法转换的情况会以携带原因的错误返回。
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

    // convert() 本质上是只带目标版本的 modify_with；直接调用它
    // 便可以传入受保护归档的主密钥。
    let options = ModifyOptions {
        target_version: Some(target),
        master_key: s.master_key,
        ..Default::default()
    };
    let outcome =
        mpq::modify_with(&s.raw, &[], &[], &options).map_err(|e| format!("转换失败：{e}"))?;
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

/// 把已应用的转换写入磁盘，并从写出的文件重新载入归档。
#[tauri::command]
pub fn save_converted(path: String, state: St) -> Result<OpenResult, String> {
    let mut s = lock(&state);
    let (bytes, _) = s.convert_applied.clone().ok_or("请先应用转换。")?;
    std::fs::write(&path, &bytes).map_err(|e| format!("保存失败：{e}"))?;
    Ok(s.load(PathBuf::from(&path), bytes))
}

/// 把转换后的字节载入界面，而不写入磁盘。
#[tauri::command]
pub fn open_converted(state: St) -> Result<OpenResult, String> {
    let mut s = lock(&state);
    let (bytes, note) = s.convert_applied.clone().ok_or("请先应用转换。")?;
    let path = s.path.clone().unwrap_or_else(|| PathBuf::from("converted.mpq"));
    let mut result = s.load(path, bytes);
    result.note = Some(format!("已载入转换结果（{note}）"));
    Ok(result)
}

/// 丢弃待处理的转换结果（窗口已关闭）。
#[tauri::command]
pub fn discard_convert(state: St) {
    lock(&state).convert_applied = None;
}
