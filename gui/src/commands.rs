//! 暴露给前端的 IPC 命令。路径（归档、提取目录、导入文件、保存目标）
//! 均来自 Tauri 文件对话框；所有文件 I/O 都在 Rust 这一侧完成。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use mpq::{Deletion, ModifyOptions};
use mpqtool::tasks;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::{GuiState, OpenResult, parse_key};

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
    let bytes =
        std::fs::read(&path).map_err(|e| format!("刷新失败：{e}（当前仍显示已载入的内容）"))?;
    Ok(s.load(path, bytes))
}

/// 关闭当前归档（文件菜单）：释放后端保存的字节、主密钥与
/// 修复 / 修改 / 转换的中间结果。
#[tauri::command]
pub fn close_archive(state: St) {
    *lock(&state) = GuiState::default();
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

/// 对已载入的原始字节应用密码操作（`op` 取值 "set" | "change" |
/// "remove"）；结果（与 apply_modify 一样）保留在状态中，直到保存 /
/// 打开 / 丢弃。
#[tauri::command]
pub fn apply_password_op(
    op: String,
    password: String,
    new_password: Option<String>,
    state: St,
) -> Result<NoteDto, String> {
    let mut s = lock(&state);
    if s.raw.is_empty() {
        return Err("请先打开一个归档。".to_string());
    }
    let note = match op.as_str() {
        "set" => {
            if password.is_empty() {
                return Err("密码不能为空。".to_string());
            }
            let outcome = mpq::protect(&s.raw, &password, &mpq::Argon2Params::default())
                .map_err(|e| format!("设置密码失败：{e}"))?;
            // 保留新生成的主密钥，这样保存后重新载入仍保持解锁状态。
            s.master_key = outcome.master_key;
            let mut note = format!(
                "已启用密码保护（{} 个文件已按扇区密封，容器转为 v4）",
                outcome.report.encrypted
            );
            if !outcome.report.dropped.is_empty() {
                note.push_str(&format!("；自动移除 {}", outcome.report.dropped.join("、")));
            }
            note.push_str("。注意：第三方 MPQ 工具将无法打开该文件。");
            s.modify_applied = Some((outcome.bytes, note.clone()));
            note
        }
        "change" => {
            let new = new_password.as_deref().ok_or("缺少新密码。")?.to_string();
            if new.is_empty() {
                return Err("新密码不能为空。".to_string());
            }
            let bytes =
                mpq::change_password(&s.raw, &password, &new, &mpq::Argon2Params::default())
                    .map_err(|e| format!("修改密码失败：{e}"))?;
            // 主密钥未变；s.master_key 依然有效。
            let note = "已更换密码（仅重新包装主密钥，数据未重写）。".to_string();
            s.modify_applied = Some((bytes, note.clone()));
            note
        }
        "remove" => {
            let outcome =
                mpq::unprotect(&s.raw, &password).map_err(|e| format!("解除密码保护失败：{e}"))?;
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

/// 密码记录文本文档中用于自我标识的标记行（删除前据此确认文件
/// 确实是本工具生成的记录，而不是碰巧同名的无关文件）。
const PASSWORD_NOTE_MARK: &str = "的打开密码";

/// 在归档同目录维护一份密码记录文本文档（`<主名>.txt`）：
/// `password` 为 `Some` 时写入（设置 / 修改密码保存后调用），
/// `None` 时删除（解除密码保护后调用，且只删除带标记的记录文件）。
/// 密码以明文保存，仅作备忘。
#[tauri::command]
pub fn write_password_note(
    archive_path: String,
    password: Option<String>,
) -> Result<NoteDto, String> {
    let path = Path::new(&archive_path);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or_else(|| format!("无法从路径取得文件名 {archive_path}"))?;
    let mut note_path = path.to_path_buf();
    note_path.set_file_name(format!("{stem}.txt"));

    match password {
        Some(pw) => {
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| archive_path.clone());
            // UTF-8 BOM + CRLF：Windows 记事本双击打开不乱码。
            let text = format!(
                "\u{FEFF}「{file_name}」{PASSWORD_NOTE_MARK}：\r\n{pw}\r\n\r\n\
                 （本文件以明文记录密码，请妥善保管或及时删除）\r\n"
            );
            std::fs::write(&note_path, text).map_err(|e| format!("写入密码记录失败：{e}"))?;
            Ok(NoteDto {
                note: format!("密码已记录到 {}", note_path.display()),
            })
        }
        None => {
            if !note_path.exists() {
                return Ok(NoteDto {
                    note: format!("未发现密码记录 {}", note_path.display()),
                });
            }
            let existing = std::fs::read_to_string(&note_path)
                .map_err(|e| format!("读取密码记录失败：{e}"))?;
            if !existing.contains(PASSWORD_NOTE_MARK) {
                return Ok(NoteDto {
                    note: format!(
                        "未删除 {}：同名文件不是本工具生成的密码记录",
                        note_path.display()
                    ),
                });
            }
            std::fs::remove_file(&note_path).map_err(|e| format!("删除密码记录失败：{e}"))?;
            Ok(NoteDto {
                note: format!("已删除密码记录 {}", note_path.display()),
            })
        }
    }
}

// -- 预览 / 提取 ------------------------------------------------------------

#[derive(Serialize)]
pub struct PreviewDto {
    pub text: String,
    pub size: usize,
    pub truncated: bool,
    /// 实际使用的文本编码（"UTF-8" / "GBK" / "UTF-16LE" / "UTF-16BE"）。
    pub encoding: String,
    /// 内容按有损 UTF-8 回退（多半是二进制）——前端据此禁止编辑。
    pub binary: bool,
    /// 图片条目：data URL（此时 text 为空，前端改用图片视图）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<crate::imagepreview::ImageDto>,
}

struct Decoded {
    text: String,
    encoding: &'static str,
    /// 走了有损 UTF-8 回退（无法可靠还原原字节）。
    lossy: bool,
}

/// 预览文本解码：BOM → 严格 UTF-8 → GBK（中文 MPQ 文本最常见的本地编码）。
/// GBK 解码结果含汉字（或完全无解码错误）就按 GBK 采用；既无汉字又有
/// 非法序列的多半是二进制数据，退回有损 UTF-8 以保留 ASCII 部分。
fn decode_preview(slice: &[u8]) -> Decoded {
    if slice.starts_with(&[0xFF, 0xFE]) {
        let (s, _, _) = encoding_rs::UTF_16LE.decode(slice);
        return Decoded {
            text: s.into_owned(),
            encoding: "UTF-16LE",
            lossy: false,
        };
    }
    if slice.starts_with(&[0xFE, 0xFF]) {
        let (s, _, _) = encoding_rs::UTF_16BE.decode(slice);
        return Decoded {
            text: s.into_owned(),
            encoding: "UTF-16BE",
            lossy: false,
        };
    }
    if let Ok(s) = std::str::from_utf8(slice) {
        return Decoded {
            text: s.to_string(),
            encoding: "UTF-8",
            lossy: false,
        };
    }
    let (s, _, had_errors) = encoding_rs::GBK.decode(slice);
    let s = s.into_owned();
    let has_cjk = s.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c));
    if has_cjk || !had_errors {
        Decoded {
            text: s,
            encoding: "GBK",
            lossy: false,
        }
    } else {
        Decoded {
            text: String::from_utf8_lossy(slice).into_owned(),
            encoding: "UTF-8",
            lossy: true,
        }
    }
}

/// 把编辑后的文本按预览时识别的编码写回字节（UTF-16 保留 BOM），
/// 保证归档里的其它字节不被改动。无法映射的字符按 WHATWG 标准以
/// 数字字符引用（如 `&#128512;`）写入。
///
/// 注意 UTF-16 必须手动按 code unit 序列化：encoding_rs 的
/// `UTF_16xx.encode()` 遵循 WHATWG 规范，实际输出 UTF-8 字节。
fn encode_text(text: &str, encoding: &str) -> Result<Vec<u8>, String> {
    match encoding {
        "UTF-8" | "" => Ok(text.as_bytes().to_vec()),
        "GBK" => Ok(encoding_rs::GBK.encode(text).0.into_owned()),
        "UTF-16LE" => {
            let mut out = vec![0xFF, 0xFE];
            for u in text.encode_utf16() {
                out.extend_from_slice(&u.to_le_bytes());
            }
            Ok(out)
        }
        "UTF-16BE" => {
            let mut out = vec![0xFE, 0xFF];
            for u in text.encode_utf16() {
                out.extend_from_slice(&u.to_be_bytes());
            }
            Ok(out)
        }
        other => Err(format!("未知编码 {other}")),
    }
}

/// 为预览窗口读取一个具名文件（前 256 KiB）。
#[tauri::command]
pub fn preview_file(name: String, state: St) -> Result<PreviewDto, String> {
    const LIMIT: usize = 256 * 1024;
    let s = lock(&state);
    let Some(archive) = &s.archive else {
        return Err("未打开归档".to_string());
    };
    let data = archive.read_file(&name).map_err(|e| e.to_string())?;
    // 图片条目按扩展名走图片预览（无 256 KiB 截断，也不可编辑）。
    if let Some(image) = crate::imagepreview::build(&name, &data)? {
        return Ok(PreviewDto {
            text: String::new(),
            size: data.len(),
            truncated: false,
            encoding: "UTF-8".to_string(),
            binary: false,
            image: Some(image),
        });
    }
    let truncated = data.len() > LIMIT;
    let slice = &data[..data.len().min(LIMIT)];
    let d = decode_preview(slice);
    Ok(PreviewDto {
        text: d.text,
        size: data.len(),
        truncated,
        encoding: d.encoding.to_string(),
        binary: d.lossy,
        image: None,
    })
}

#[derive(Serialize)]
pub struct StagedDto {
    /// 归档内的条目名（含子目录路径，原样传入的名字）。
    pub name: String,
    /// 临时文件路径；「应用修改」时从这里读取字节。
    pub path: String,
    pub size: usize,
    /// 是否存在同名的归档条目（保存将替换它）。
    pub replaces: bool,
}

/// 把预览窗中编辑后的文本按原编码写入临时文件，返回其路径，
/// 供前端作为「修改归档 → 待导入」条目排队（保存回到归档的流程）。
#[tauri::command]
pub fn stage_edited_file(
    name: String,
    text: String,
    encoding: String,
    state: St,
) -> Result<StagedDto, String> {
    let bytes = encode_text(&text, &encoding)?;
    let dir = std::env::temp_dir().join("mpqtool-edit");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建临时目录失败：{e}"))?;
    // 临时文件名：全路径名净化（\ / 等替换为 _），同名条目的多次编辑后者覆盖前者。
    let sanitized: String = name
        .chars()
        .map(|c| {
            matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_')
                .then_some(c)
                .unwrap_or('_')
        })
        .collect();
    let file = dir.join(sanitized);
    std::fs::write(&file, &bytes).map_err(|e| format!("写入临时文件失败：{e}"))?;
    let s = lock(&state);
    let replaces = s
        .archive
        .as_ref()
        .is_some_and(|a| a.find_file(&name).is_some());
    Ok(StagedDto {
        name,
        path: file.to_string_lossy().to_string(),
        size: bytes.len(),
        replaces,
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
        s.entries
            .iter()
            .filter(|e| e.name.is_some())
            .cloned()
            .collect()
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
        format!(
            "已应用 {} 项修复；{unfixed} 项无法自动修复。",
            report.issues.len() - unfixed
        )
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

#[cfg(test)]
mod tests {
    use super::{decode_preview, encode_text};

    #[test]
    fn gbk_text_decodes_as_gbk() {
        // “中文测试” 的 GBK 编码
        let d = decode_preview(&[0xD6, 0xD0, 0xCE, 0xC4, 0xB2, 0xE2, 0xCA, 0xD4]);
        assert_eq!(
            (d.text.as_str(), d.encoding, d.lossy),
            ("中文测试", "GBK", false)
        );
    }

    #[test]
    fn utf8_stays_utf8() {
        let d = decode_preview("中文 ASCII".as_bytes());
        assert_eq!(
            (d.text.as_str(), d.encoding, d.lossy),
            ("中文 ASCII", "UTF-8", false)
        );
    }

    #[test]
    fn utf16_bom_sniffed() {
        let mut le = vec![0xFF, 0xFE];
        for u in "中文".encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
        }
        let d = decode_preview(&le);
        assert_eq!(
            (d.text.as_str(), d.encoding, d.lossy),
            ("中文", "UTF-16LE", false)
        );
    }

    #[test]
    fn binary_falls_back_to_lossy_utf8() {
        // 0xFF 在 GBK 中非法且无汉字：应触发有损 UTF-8 回退（禁止编辑）。
        let d = decode_preview(&[0xFFu8; 64]);
        assert_eq!((d.encoding, d.lossy), ("UTF-8", true));
    }

    #[test]
    fn gbk_truncated_at_boundary_still_gbk() {
        // “中文测试” 截断在最后一个字符中间：仅一个替换符，仍按 GBK 展示。
        let d = decode_preview(&[0xD6, 0xD0, 0xCE, 0xC4, 0xB2, 0xE2, 0xCA]);
        assert_eq!(d.encoding, "GBK");
        assert!(d.text.starts_with("中文测"));
        assert!(d.text.ends_with('\u{FFFD}'));
    }

    /// 解码 → 编辑 → 按原编码回写，字节应与原文件一致（无丢失字符时）。
    fn roundtrip(bytes: &[u8]) {
        let d = decode_preview(bytes);
        let back = encode_text(&d.text, d.encoding).unwrap();
        assert_eq!(back, bytes);
    }

    #[test]
    fn gbk_roundtrip() {
        roundtrip(&[0xD6, 0xD0, 0xCE, 0xC4, 0xB2, 0xE2, 0xCA, 0xD4]);
    }

    #[test]
    fn utf16_roundtrip() {
        let mut le = vec![0xFF, 0xFE];
        for u in "中文 edited!".encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
        }
        roundtrip(&le);
    }

    #[test]
    fn utf8_roundtrip() {
        roundtrip("中文 mixed\ntext\r\n".as_bytes());
    }

    #[test]
    fn encode_rejects_unknown_encoding() {
        assert!(encode_text("x", "latin1").is_err());
    }

    /// 文件夹导入：递归收集的归档名以文件夹自身为根、用 `\` 连接，
    /// 且包含隐藏文件（用户选了整个文件夹，不做筛选）。
    #[test]
    fn folder_import_walk_collects_relative_paths() {
        let base = std::env::temp_dir().join("mpqtool-gui-test-walk");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("img/ui")).unwrap();
        std::fs::write(base.join("img/a.png"), b"a").unwrap();
        std::fs::write(base.join("img/ui/b.png"), b"bb").unwrap();
        std::fs::write(base.join("img/.hidden"), b"h").unwrap();

        let s = crate::state::GuiState::default();
        let mut out = Vec::new();
        super::walk_import_dir(&base.join("img"), "img", &s, &mut out);

        let mut names: Vec<&str> = out.iter().map(|o| o.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["img\\.hidden", "img\\a.png", "img\\ui\\b.png"]);
        assert!(out.iter().all(|o| o.error.is_none() && !o.replaces));

        // 「只导入子项」在归档根目录：前缀为空，不能出现前导 `\`。
        let mut out = Vec::new();
        super::walk_import_dir(&base.join("img"), "", &s, &mut out);
        let mut names: Vec<&str> = out.iter().map(|o| o.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec![".hidden", "a.png", "ui\\b.png"]);

        // 不同子目录下的同名文件：名字必须互相独立，不能串。
        std::fs::create_dir_all(base.join("dup/dirA")).unwrap();
        std::fs::create_dir_all(base.join("dup/dirB")).unwrap();
        std::fs::write(base.join("dup/file.txt"), b"root").unwrap();
        std::fs::write(base.join("dup/dirA/file.txt"), b"A").unwrap();
        std::fs::write(base.join("dup/dirB/file.txt"), b"B").unwrap();
        let mut out = Vec::new();
        super::walk_import_dir(&base.join("dup"), "", &s, &mut out);
        let mut names: Vec<&str> = out.iter().map(|o| o.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["dirA\\file.txt", "dirB\\file.txt", "file.txt"]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn join_arc_omits_leading_separator() {
        assert_eq!(super::join_arc("", "a.png"), "a.png");
        assert_eq!(super::join_arc("pre", "a.png"), "pre\\a.png");
        assert_eq!(super::join_arc("pre\\sub", "a.png"), "pre\\sub\\a.png");
    }
}

/// 丢弃待处理的修复结果（窗口已关闭）。
#[tauri::command]
pub fn discard_repair(state: St) {
    lock(&state).repair_fixed = None;
}

// -- 导入 / 删除 -------------------------------------------------------------

#[derive(Serialize)]
pub struct ImportStatDto {
    /// 归档内名称（普通文件为基名；文件夹导入为
    /// `文件夹名\相对路径` 或「只导入子项」时的 `相对路径`，
    /// 可能还带前端传入的前缀）。
    pub name: String,
    /// 源文件在磁盘上的完整路径；应用修改时从这里读取字节。
    pub path: String,
    pub size: u64,
    /// 是否存在同名的归档条目。
    pub replaces: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 检查待导入的文件 / 文件夹：名称、大小，以及是否会替换已有条目。
/// 文件内容在应用修改时才读取。路径是文件夹时递归展开其中的全部
/// 文件，归档名以文件夹自身为根（如选 `D:\img` 会得到
/// `img\a.png`、`img\ui\b.png`）；`children_only` 为真时只导入其
/// 子项（得到 `a.png`、`ui\b.png`）。`prefix` 非空时（前端浏览到
/// 子目录时导入）加在每项名称之前。符号链接目录不跟随，避免循环。
#[tauri::command]
pub fn stat_imports(
    paths: Vec<String>,
    prefix: Option<String>,
    children_only: Option<bool>,
    state: St,
) -> Vec<ImportStatDto> {
    let s = lock(&state);
    let mut out = Vec::new();
    let pre = prefix.as_deref().unwrap_or("");
    for p in &paths {
        let path = Path::new(p);
        if path.is_dir() {
            let root = if children_only.unwrap_or(false) {
                pre.to_string()
            } else {
                let base = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                join_arc(pre, &base)
            };
            walk_import_dir(path, &root, &s, &mut out);
        } else {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            out.push(stat_one(path, join_arc(pre, &name), &s));
        }
    }
    out
}

/// `\` 连接归档内路径；`a` 为空时直接返回 `b`（避免前导 `\`）。
fn join_arc(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else {
        format!("{a}\\{b}")
    }
}

/// 单个文件（或读取失败的路径）的导入信息。
fn stat_one(path: &Path, name: String, s: &GuiState) -> ImportStatDto {
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
        path: path.to_string_lossy().to_string(),
        size,
        replaces,
        error,
    }
}

/// 递归收集 `dir` 下的文件，归档名前缀为 `arc_prefix`（已含文件夹
/// 自身名与可能的导入前缀）。读取失败只影响对应条目，不中断整体。
fn walk_import_dir(dir: &Path, arc_prefix: &str, s: &GuiState, out: &mut Vec<ImportStatDto>) {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            out.push(ImportStatDto {
                name: arc_prefix.to_string(),
                path: dir.to_string_lossy().to_string(),
                size: 0,
                replaces: false,
                error: Some(format!("读取文件夹失败：{e}")),
            });
            return;
        }
    };
    // 按名称排序，保证待导入列表的顺序稳定。
    let mut children: Vec<std::fs::DirEntry> = read.filter_map(|e| e.ok()).collect();
    children.sort_by_key(|e| e.file_name());
    for child in children {
        // DirEntry::file_type 不穿透符号链接：链接目录直接跳过。
        let Ok(ft) = child.file_type() else { continue };
        let path = child.path();
        if ft.is_dir() {
            let sub = join_arc(arc_prefix, &child.file_name().to_string_lossy());
            walk_import_dir(&path, &sub, s, out);
        } else if ft.is_file() {
            let name = join_arc(arc_prefix, &child.file_name().to_string_lossy());
            out.push(stat_one(&path, name, s));
        }
    }
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
                let (source, index) = parse_key(key).ok_or_else(|| format!("无效的表键 {key}"))?;
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
    let path = s
        .path
        .clone()
        .unwrap_or_else(|| PathBuf::from("modified.mpq"));
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
        result.note = Some("（新建空归档，用「操作 → 导入文件 / 导入文件夹」添加内容）".to_string());
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
    let path = s
        .path
        .clone()
        .unwrap_or_else(|| PathBuf::from("converted.mpq"));
    let mut result = s.load(path, bytes);
    result.note = Some(format!("已载入转换结果（{note}）"));
    Ok(result)
}

/// 丢弃待处理的转换结果（窗口已关闭）。
#[tauri::command]
pub fn discard_convert(state: St) {
    lock(&state).convert_applied = None;
}
