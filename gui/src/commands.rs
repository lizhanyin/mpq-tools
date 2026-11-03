//! 暴露给前端的 IPC 命令。路径（归档、提取目录、导入文件、保存目标）
//! 均来自 Tauri 文件对话框；所有文件 I/O 都在 Rust 这一侧完成。
//!
//! 重 I/O 命令（整归档读取 / 解析 / 写盘、Argon2 校验、目录遍历、
//! 图片解码等）标注 `(async)`：Tauri 的同步命令在**主线程**执行，
//! 秒级的工作会冻结窗口事件循环（Windows 判定“无响应”）；
//! `(async)` 把同步函数放到线程池执行，签名与前端调用方式不变。
//! 所有会取全局状态锁的命令（包括 `list_dir` 等原本轻量的）一律
//! `(async)`：长操作（修改 / 转换 / 密码）可能持锁数分钟，同步命令
//! 在主线程等锁会连带冻结窗口，并堵住同样经主线程派发的 Channel
//! 进度事件。`write_password_note` 不碰状态锁且只写小文本，保持同步。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};

use mpq::{Deletion, ModifyOptions, ModifyPhase};
use mpqtool::tasks;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
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

#[tauri::command(async)]
pub fn open_archive(path: String, state: St) -> Result<OpenResult, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("读取失败：{e}"))?;
    let mut s = lock(&state);
    Ok(s.load(PathBuf::from(&path), bytes))
}

/// 从磁盘重新读取当前归档（🔄 / F5）。
#[tauri::command(async)]
pub fn reload_archive(state: St) -> Result<OpenResult, String> {
    let mut s = lock(&state);
    let path = s.path.clone().ok_or("请先打开一个归档。")?;
    let bytes =
        std::fs::read(&path).map_err(|e| format!("刷新失败：{e}（当前仍显示已载入的内容）"))?;
    Ok(s.load(path, bytes))
}

/// 关闭当前归档（文件菜单）：释放后端保存的字节、主密钥与
/// 修复 / 修改 / 转换的中间结果。
#[tauri::command(async)]
pub fn close_archive(state: St) {
    *lock(&state) = GuiState::default();
}

// -- 目录懒加载（大归档初始只发根目录一层） ----------------------------

/// 搜索结果的上限：与前端 MAX_ROWS 一致——更宽泛的词只会在表格里
/// 显示同样的上限，没必要经 IPC 多传。
const SEARCH_LIMIT: usize = 50_000;

/// 进入 / 展开文件夹时取该目录的一层视图（本层文件 + 子文件夹占位符）。
/// `path` 为路径段数组（[] = 归档根）。
#[tauri::command(async)]
pub fn list_dir(path: Vec<String>, state: St) -> Result<crate::state::TreeNodeDto, String> {
    let s = lock(&state);
    s.list_dir_dto(&path)
        .ok_or_else(|| format!("文件夹不存在：{}", path.join("\\")))
}

#[derive(Serialize)]
pub struct SearchDto {
    pub entries: Vec<crate::state::EntryDto>,
    /// 命中数超过上限被截断。
    pub truncated: bool,
}

/// 在整个归档（含前端尚未加载的目录）中按全路径搜索。
#[tauri::command(async)]
pub fn search_archive(needle: String, state: St) -> Result<SearchDto, String> {
    let s = lock(&state);
    let (entries, truncated) = s.search_entries(&needle, SEARCH_LIMIT);
    Ok(SearchDto { entries, truncated })
}

/// 某个文件夹及其全部子级中的条目——「选中/提取/移动/删除文件夹」
/// 等递归交互用（这些条目对应的目录前端未必加载过）。
#[tauri::command(async)]
pub fn dir_entries(path: Vec<String>, state: St) -> Result<Vec<crate::state::EntryDto>, String> {
    let s = lock(&state);
    s.dir_entries_dto(&path)
        .ok_or_else(|| format!("文件夹不存在：{}", path.join("\\")))
}

// -- 密码保护（自定义 AEAD 扩展）---------------------------------

/// 针对 `(password)` 保管库校验密码，并把主密钥保存在状态中，
/// 供后续读取 / 修改使用。
#[tauri::command(async)]
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
/// 打开 / 丢弃。重写整档（可达分钟级）在全局锁**外**进行（同
/// apply_modify），期间界面不被阻塞。
#[tauri::command(async)]
pub fn apply_password_op(
    op: String,
    password: String,
    new_password: Option<String>,
    state: St,
) -> Result<NoteDto, String> {
    let (raw, key_before) = {
        let s = lock(&state);
        if s.raw.is_empty() {
            return Err("请先打开一个归档。".to_string());
        }
        (s.raw.clone(), s.master_key)
    };
    // (输出字节, 摘要, 操作后的主密钥)。
    let (bytes, note, key_after) = match op.as_str() {
        "set" => {
            if password.is_empty() {
                return Err("密码不能为空。".to_string());
            }
            let outcome = mpq::protect(&raw, &password, &mpq::Argon2Params::default())
                .map_err(|e| format!("设置密码失败：{e}"))?;
            let mut note = format!(
                "已启用密码保护（{} 个文件已按扇区密封，容器转为 v4）",
                outcome.report.encrypted
            );
            if !outcome.report.dropped.is_empty() {
                note.push_str(&format!("；自动移除 {}", outcome.report.dropped.join("、")));
            }
            note.push_str("。注意：第三方 MPQ 工具将无法打开该文件。");
            // 保留新生成的主密钥，这样保存后重新载入仍保持解锁状态。
            (outcome.bytes, note, outcome.master_key)
        }
        "change" => {
            let new = new_password.as_deref().ok_or("缺少新密码。")?.to_string();
            if new.is_empty() {
                return Err("新密码不能为空。".to_string());
            }
            let bytes =
                mpq::change_password(&raw, &password, &new, &mpq::Argon2Params::default())
                    .map_err(|e| format!("修改密码失败：{e}"))?;
            // 主密钥未变；沿用快照。
            (
                bytes,
                "已更换密码（仅重新包装主密钥，数据未重写）。".to_string(),
                key_before,
            )
        }
        "remove" => {
            let outcome =
                mpq::unprotect(&raw, &password).map_err(|e| format!("解除密码保护失败：{e}"))?;
            let note = format!(
                "已解除密码保护（{} 个文件已解密）。",
                outcome.report.decrypted
            );
            (outcome.bytes, note, None)
        }
        other => return Err(format!("未知的密码操作 {other}")),
    };
    let mut s = lock(&state);
    s.master_key = key_after;
    s.modify_applied = Some((bytes, note.clone()));
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
    /// MDX 模型条目：base64 原始字节（前端 WebGL 渲染，不可编辑）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelDto>,
}

/// MDX（Warcraft III 模型）预览载荷：base64 的原始文件字节。
#[derive(Serialize)]
pub struct ModelDto {
    pub data: String,
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

/// 读取一个条目用于预览 / 缩略图：优先按 key（`<source>:<index>`，
/// 未命名条目唯一可用的寻址方式），回退按名字。返回（数据，用于
/// 判断预览类型的显示名 —— 未命名条目带识别出的类型后缀）。
fn read_for_preview(
    state: &GuiState,
    key: Option<&str>,
    name: Option<&str>,
) -> Result<(Vec<u8>, String), String> {
    let Some(archive) = &state.archive else {
        return Err("未打开归档".to_string());
    };
    if let Some(k) = key {
        let (src, idx) = crate::state::parse_key(k).ok_or("无效的条目键")?;
        let entry = state
            .entries
            .iter()
            .find(|e| e.source == src && e.block_index == idx)
            .ok_or("条目不存在")?;
        let display = entry.display_name();
        let data = archive.read_entry(entry).map_err(|e| e.to_string())?;
        return Ok((data, display));
    }
    let name = name.ok_or("缺少 name 或 key")?;
    let data = archive.read_file(name).map_err(|e| e.to_string())?;
    Ok((data, name.to_string()))
}

/// 为预览窗口读取一个条目（文本前 256 KiB；图片 / 模型不截断）。
/// 未命名条目以 key 寻址（显示名带识别出的类型后缀，如
/// `<unnamed #3>.blp`，据此走图片 / 模型预览）。full=true 时不截断
/// 地加载整个文本（前端「加载全部」按钮）。
#[tauri::command(async)]
pub fn preview_file(
    key: Option<String>,
    name: Option<String>,
    full: Option<bool>,
    state: St,
) -> Result<PreviewDto, String> {
    const LIMIT: usize = 256 * 1024;
    /// 「加载全部」的体积上限：更大的文本过 IPC 并渲染会明显卡顿。
    const FULL_LIMIT: usize = 16 * 1024 * 1024;
    /// 模型体积上限：base64 后约放大 1/3，再大走 IPC 不划算。
    const MODEL_LIMIT: usize = 32 * 1024 * 1024;
    let s = lock(&state);
    let (data, display) = read_for_preview(&s, key.as_deref(), name.as_deref())?;
    // 图片条目按显示名后缀走图片预览（无 256 KiB 截断，也不可编辑）。
    if let Some(image) = crate::imagepreview::build(&display, &data)? {
        return Ok(PreviewDto {
            text: String::new(),
            size: data.len(),
            truncated: false,
            encoding: "UTF-8".to_string(),
            binary: false,
            image: Some(image),
            model: None,
        });
    }
    // MDX 模型：整文件 base64 给前端 WebGL 渲染（贴图按需另取）。
    if display.to_lowercase().ends_with(".mdx") {
        if data.len() > MODEL_LIMIT {
            return Err(format!(
                "模型过大（{} MiB，上限 32 MiB），请提取后用模型查看器打开",
                data.len() / 1024 / 1024
            ));
        }
        return Ok(PreviewDto {
            text: String::new(),
            size: data.len(),
            truncated: false,
            encoding: "UTF-8".to_string(),
            binary: false,
            image: None,
            model: Some(ModelDto {
                data: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data),
            }),
        });
    }
    if full.unwrap_or(false) && data.len() > FULL_LIMIT {
        return Err(format!(
            "文件过大（{} MiB，上限 16 MiB），请提取后用文本编辑器打开",
            data.len() / 1024 / 1024
        ));
    }
    let limit = if full.unwrap_or(false) { FULL_LIMIT } else { LIMIT };
    let truncated = data.len() > limit;
    let slice = &data[..data.len().min(limit)];
    let d = decode_preview(slice);
    Ok(PreviewDto {
        text: d.text,
        size: data.len(),
        truncated,
        encoding: d.encoding.to_string(),
        binary: d.lossy,
        image: None,
        model: None,
    })
}

/// 读取归档内一个文件的原始字节（base64）——MDX 预览按需加载贴图用。
/// 归档里没有时回退到老客户端 MPQ 链（`war3dir` 设置；补丁优先）。
/// 两者都没有返回 None，由前端回退到无贴图渲染。
#[tauri::command(async)]
pub fn read_file_bytes(
    name: String,
    war3dir: Option<String>,
    state: St,
) -> Result<Option<String>, String> {
    let mut s = lock(&state);
    let from_map = s.archive.as_ref().and_then(|a| a.read_file(&name).ok());
    let data = match from_map {
        Some(d) => d,
        None => match s.read_war3_bytes(&name, war3dir.as_deref().unwrap_or("")) {
            Some(d) => d,
            None => return Ok(None),
        },
    };
    Ok(Some(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &data,
    )))
}

/// 老客户端目录下可用的 War3*.mpq 数量（设置页校验提示用）。
#[tauri::command(async)]
pub fn war3_mpq_count(dir: String) -> usize {
    [
        "War3Patch.mpq",
        "War3x.mpq",
        "War3xlocal.mpq",
        "War3local.mpq",
        "War3.mpq",
    ]
    .iter()
    .filter(|name| std::path::Path::new(&dir).join(name).is_file())
    .count()
}

/// 模型使用的贴图：按 key 读模型、解析 TEXS，返回每条贴图路径及其
/// 来源（map = 当前归档、war3 = 老客户端 MPQ 链、missing = 均未找到）。
#[derive(Serialize)]
pub struct ModelTextureDto {
    pub path: String,
    /// "map" | "war3" | "missing"
    pub source: String,
}

#[tauri::command(async)]
pub fn model_textures(
    key: String,
    war3dir: Option<String>,
    state: St,
) -> Result<Vec<ModelTextureDto>, String> {
    let mut s = lock(&state);
    let (data, _display) = read_for_preview(&s, Some(&key), None)?;
    let mut out = Vec::new();
    for path in mpq::mdx_texture_paths(&data) {
        // TEXS 中 replaceable 槽的路径与归档名可能大小写不同，
        // has_file 的 hash 匹配本身不区分大小写。
        let in_map = s.archive.as_ref().is_some_and(|a| a.has_file(&path));
        let source = if in_map {
            "map".to_string()
        } else if s
            .read_war3_bytes(&path, war3dir.as_deref().unwrap_or(""))
            .is_some()
        {
            "war3".to_string()
        } else {
            "missing".to_string()
        };
        out.push(ModelTextureDto { path, source });
    }
    Ok(out)
}

/// 贴图缩略图（PNG data URL）：当前归档优先，回退老客户端 MPQ 链。
/// 找不到或无法解码返回 None（前端显示占位）。
#[tauri::command(async)]
pub fn texture_thumbnail(
    path: String,
    war3dir: Option<String>,
    state: St,
) -> Result<Option<String>, String> {
    let mut s = lock(&state);
    let from_map = s.archive.as_ref().and_then(|a| a.read_file(&path).ok());
    let data = match from_map {
        Some(d) => d,
        None => match s.read_war3_bytes(&path, war3dir.as_deref().unwrap_or("")) {
            Some(d) => d,
            None => return Ok(None),
        },
    };
    crate::imagepreview::thumbnail(&path, &data)
}

/// 网格视图缩略图：解码并等比缩到 192px 内的 PNG data URL。
/// 非图片或解码失败返回 None（前端回退到图标），不弹错误。
#[tauri::command(async)]
pub fn thumbnail(
    key: Option<String>,
    name: Option<String>,
    state: St,
) -> Result<Option<String>, String> {
    let s = lock(&state);
    let (data, display) = read_for_preview(&s, key.as_deref(), name.as_deref())?;
    crate::imagepreview::thumbnail(&display, &data)
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
#[tauri::command(async)]
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

#[derive(Deserialize)]
pub struct MoveSpec {
    /// 选择键 `"<source>:<block_index>"`——定位要移动的源条目。
    pub key: String,
    /// 移动后的完整归档内名称（已含目标目录前缀）。
    pub name: String,
}

/// 「移动」的数据准备：把选中条目的内容读出并写入临时文件，返回值供
/// 前端按新名字排队为「待导入」（配合删除旧条目，在「修改归档」窗口
/// 应用后生效）。逐条读取：任何一条失败都中止本次移动，避免只移动一半。
#[tauri::command(async)]
pub fn stage_move_files(moves: Vec<MoveSpec>, state: St) -> Result<Vec<StagedDto>, String> {
    let s = lock(&state);
    let Some(archive) = &s.archive else {
        return Err("未打开归档".to_string());
    };
    let dir = std::env::temp_dir().join("mpqtool-move");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建临时目录失败：{e}"))?;
    let mut out = Vec::new();
    for m in &moves {
        let (source, index) = parse_key(&m.key).ok_or_else(|| format!("无效的表键 {}", m.key))?;
        let entry = s
            .entries
            .iter()
            .find(|e| e.source == source && e.block_index == index)
            .ok_or_else(|| format!("找不到条目 {}", m.key))?;
        let old = entry
            .name
            .as_deref()
            .ok_or_else(|| "未命名条目没有名字，无法移动。".to_string())?;
        let data = archive
            .read_file(old)
            .map_err(|e| format!("读取 {old} 失败：{e}"))?;
        // 临时文件按选择键命名（`:` 在 Windows 文件名中非法，换成 `_`）；
        // 同一条目再次移动时后者覆盖前者。
        let file = dir.join(format!("{}.tmp", m.key.replace(':', "_")));
        std::fs::write(&file, &data).map_err(|e| format!("写入临时文件失败：{e}"))?;
        out.push(StagedDto {
            name: m.name.clone(),
            path: file.to_string_lossy().to_string(),
            size: data.len(),
            replaces: archive.find_file(&m.name).is_some(),
        });
    }
    Ok(out)
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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

#[tauri::command(async)]
pub fn save_repaired(path: String, state: St) -> Result<NoteDto, String> {
    let s = lock(&state);
    let fixed = s.repair_fixed.as_ref().ok_or("请先应用修复。")?;
    std::fs::write(&path, fixed).map_err(|e| format!("保存失败：{e}"))?;
    Ok(NoteDto {
        note: format!("已保存 {path}"),
    })
}

/// 把修复后的字节载入界面，而不写入磁盘。
#[tauri::command(async)]
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

        // 与 stat_imports 相同的两段式：锁外扫描 + 短暂加锁补 replaces。
        let stat = |dir: &std::path::Path, prefix: &str| -> Vec<super::ImportStatDto> {
            let s = crate::state::GuiState::default();
            let mut found = Vec::new();
            super::walk_import_dir(dir, prefix, &mut found, &super::ProgressTap::noop());
            found.into_iter().map(|f| f.into_dto(&s)).collect()
        };

        let out = stat(&base.join("img"), "img");
        let mut names: Vec<&str> = out.iter().map(|o| o.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["img\\.hidden", "img\\a.png", "img\\ui\\b.png"]);
        assert!(out.iter().all(|o| o.error.is_none() && !o.replaces));

        // 「只导入子项」在归档根目录：前缀为空，不能出现前导 `\`。
        let out = stat(&base.join("img"), "");
        let mut names: Vec<&str> = out.iter().map(|o| o.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec![".hidden", "a.png", "ui\\b.png"]);

        // 不同子目录下的同名文件：名字必须互相独立，不能串。
        std::fs::create_dir_all(base.join("dup/dirA")).unwrap();
        std::fs::create_dir_all(base.join("dup/dirB")).unwrap();
        std::fs::write(base.join("dup/file.txt"), b"root").unwrap();
        std::fs::write(base.join("dup/dirA/file.txt"), b"A").unwrap();
        std::fs::write(base.join("dup/dirB/file.txt"), b"B").unwrap();
        let out = stat(&base.join("dup"), "");
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
#[tauri::command(async)]
pub fn discard_repair(state: St) {
    lock(&state).repair_fixed = None;
}

// -- 导入 / 删除 -------------------------------------------------------------

/// 导入进度事件（经 Tauri Channel 推送到前端）。
/// phase：`"reading"` 读取待导入文件 / 扫描目录；`"encoding"` 压缩写入
/// 新归档；`"verifying"` 自检回读。total 为 0 表示总数未知（目录扫描
/// 尚在进行），前端只显示已完成数量。
#[derive(Serialize, Deserialize, Clone)]
pub struct ImportProgressDto {
    pub phase: String,
    pub done: u32,
    pub total: u32,
}

/// 进度节流器：仅在跨过阈值时真正发送 Channel 消息——总数已知时约
/// 1%，未知时每 32 项——避免上万条目时 IPC 消息淹没前端。rayon 工作
/// 线程可能并发调用，CAS 保证同一进度只发一次（竞争失败者直接放弃，
/// 下一个阈值仍会触发）。
struct ProgressTap {
    channel: Channel<ImportProgressDto>,
    last: AtomicU32,
}

impl ProgressTap {
    fn new(channel: Channel<ImportProgressDto>) -> Self {
        Self {
            channel,
            last: AtomicU32::new(0),
        }
    }

    /// 测试用：丢弃全部进度事件（不需要真实 webview）。
    #[cfg(test)]
    fn noop() -> Self {
        Self::new(Channel::new(|_| Ok(())))
    }

    fn emit(&self, phase: &str, done: u32, total: u32) {
        let step = if total == 0 { 32 } else { (total / 100).max(1) };
        let prev = self.last.load(Ordering::Relaxed);
        let crossed = done.saturating_sub(prev) >= step || (total > 0 && done == total);
        if !crossed {
            return;
        }
        if self
            .last
            .compare_exchange(prev, done, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            let _ = self.channel.send(ImportProgressDto {
                phase: phase.to_string(),
                done,
                total,
            });
        }
    }
}

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
///
/// 递归扫描（含每项一次 metadata，慢盘 / 网络盘上可达分钟级）在
/// **锁外**进行；扫描完短暂加锁补上 `replaces`（每项一次内存哈希
/// 查找，微秒级）——持锁遍历会让所有其他命令（含主线程上的进度
/// 事件派发）排队到扫描结束。
#[tauri::command(async)]
pub fn stat_imports(
    paths: Vec<String>,
    prefix: Option<String>,
    children_only: Option<bool>,
    on_progress: Channel<ImportProgressDto>,
    state: St,
) -> Vec<ImportStatDto> {
    let tap = ProgressTap::new(on_progress);
    let mut found: Vec<FoundFile> = Vec::new();
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
            walk_import_dir(path, &root, &mut found, &tap);
        } else {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            found.push(stat_one(path, join_arc(pre, &name)));
        }
    }
    let s = lock(&state);
    found.into_iter().map(|f| f.into_dto(&s)).collect()
}

/// `\` 连接归档内路径；`a` 为空时直接返回 `b`（避免前导 `\`）。
fn join_arc(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else {
        format!("{a}\\{b}")
    }
}

/// 扫描发现的一个待导入文件（或读取失败的路径）。磁盘 I/O（遍历 +
/// metadata）在锁外完成；`replaces` 由 [`FoundFile::into_dto`] 稍后
/// 短暂加锁补上。
struct FoundFile {
    name: String,
    path: String,
    size: u64,
    error: Option<String>,
}

impl FoundFile {
    fn into_dto(self, s: &GuiState) -> ImportStatDto {
        let replaces = self.error.is_none()
            && s.archive
                .as_ref()
                .is_some_and(|a| a.find_file(&self.name).is_some());
        ImportStatDto {
            name: self.name,
            path: self.path,
            size: self.size,
            replaces,
            error: self.error,
        }
    }
}

/// 单个文件（或读取失败的路径）的导入信息。
fn stat_one(path: &Path, name: String) -> FoundFile {
    let (size, error) = match std::fs::metadata(path) {
        Ok(m) => (m.len(), None),
        Err(e) => (0, Some(e.to_string())),
    };
    FoundFile {
        name,
        path: path.to_string_lossy().to_string(),
        size,
        error,
    }
}

/// 递归收集 `dir` 下的文件，归档名前缀为 `arc_prefix`（已含文件夹
/// 自身名与可能的导入前缀）。读取失败只影响对应条目，不中断整体。
/// `tap` 按条目数节流上报扫描进度（总数未知，done 为已发现项数）。
fn walk_import_dir(dir: &Path, arc_prefix: &str, out: &mut Vec<FoundFile>, tap: &ProgressTap) {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            out.push(FoundFile {
                name: arc_prefix.to_string(),
                path: dir.to_string_lossy().to_string(),
                size: 0,
                error: Some(format!("读取文件夹失败：{e}")),
            });
            tap.emit("reading", out.len() as u32, 0);
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
            walk_import_dir(&path, &sub, out, tap);
        } else if ft.is_file() {
            let name = join_arc(arc_prefix, &child.file_name().to_string_lossy());
            out.push(stat_one(&path, name));
            tap.emit("reading", out.len() as u32, 0);
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
///
/// 导入文件的磁盘读取与修改引擎（整档重写 + 压缩，可达分钟级）都在
/// 全局锁**外**进行：期间其他命令（浏览 / 预览）以及主线程上的
/// Channel 进度事件派发不会被阻塞；输入在锁内快照，结果稍后短暂
/// 加锁写回。读取 / 编码 / 自检阶段经 `on_progress` 推送进度事件。
#[tauri::command(async)]
pub fn apply_modify(
    additions: Vec<AddSpec>,
    deletions: Vec<DelSpec>,
    reencrypt: bool,
    on_progress: Channel<ImportProgressDto>,
    state: St,
) -> Result<NoteDto, String> {
    // 并行读取全部待导入文件；按原始顺序收集，保持首个错误即中止的
    // 语义与错误文案不变。
    let total = additions.len() as u32;
    let read_tap = ProgressTap::new(on_progress.clone());
    let read_done = AtomicU32::new(0);
    let results: Vec<(String, Result<Vec<u8>, String>)> = additions
        .par_iter()
        .map(|a| {
            let r = std::fs::read(&a.path).map_err(|e| format!("读取失败 {}: {e}", a.path));
            let done = read_done.fetch_add(1, Ordering::Relaxed) + 1;
            read_tap.emit("reading", done, total);
            (a.name.clone(), r)
        })
        .collect();
    let mut adds: Vec<(String, Vec<u8>)> = Vec::with_capacity(results.len());
    for (name, r) in results {
        adds.push((name, r?));
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

    // 引擎启动前还有一段无事件的准备期（克隆归档字节、重建源块布局），
    // 先推一条 preparing，避免进度条停在「读取中 N/N」。
    let _ = on_progress.send(ImportProgressDto {
        phase: "preparing".to_string(),
        done: 0,
        total: 0,
    });
    let tap = ProgressTap::new(on_progress);
    let cb = |phase: ModifyPhase, done: usize, total: usize| {
        let name = match phase {
            ModifyPhase::Encoding => "encoding",
            ModifyPhase::Verifying => "verifying",
        };
        tap.emit(name, done as u32, total as u32);
    };
    let progress = Some(&cb as &(dyn Fn(ModifyPhase, usize, usize) + Sync));

    let (raw, master_key) = {
        let s = lock(&state);
        if s.raw.is_empty() {
            return Err("请先打开一个归档。".to_string());
        }
        (s.raw.clone(), s.master_key)
    };

    let options = ModifyOptions {
        reencrypt_key_v2: reencrypt,
        master_key,
        ..Default::default()
    };
    let outcome = mpq::modify_with_progress(&raw, &dels, &adds, &options, progress)
        .map_err(|e| format!("修改失败：{e}"))?;
    let mut summary = summarize_modify(&outcome.report);
    if !outcome.report.re_encoded.is_empty() {
        summary.push_str(if reencrypt {
            "（key-v2 已按新偏移重新加密）"
        } else {
            "（key-v2 已存为未加密）"
        });
    }
    lock(&state).modify_applied = Some((outcome.bytes, summary.clone()));
    Ok(NoteDto { note: summary })
}

/// 把已应用的修改写入磁盘，并从写出的文件重新载入归档
/// （与 egui 流程一致）。
#[tauri::command(async)]
pub fn save_modified(path: String, state: St) -> Result<OpenResult, String> {
    let mut s = lock(&state);
    let (bytes, _) = s.modify_applied.clone().ok_or("请先应用修改。")?;
    std::fs::write(&path, &bytes).map_err(|e| format!("保存失败：{e}"))?;
    Ok(s.load(PathBuf::from(&path), bytes))
}

/// 把修改后的字节载入界面，而不写入磁盘。
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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
/// 整档重写在全局锁**外**进行（同 apply_modify），期间界面不被阻塞。
#[tauri::command(async)]
pub fn apply_convert(target: u32, state: St) -> Result<ConvertDto, String> {
    let target = format_version(target)?;
    let (raw, master_key) = {
        let s = lock(&state);
        if s.raw.is_empty() {
            return Err("请先打开一个归档。".to_string());
        }
        (s.raw.clone(), s.master_key)
    };

    let source = mpq::Archive::new(std::io::Cursor::new(&raw))
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
        master_key,
        ..Default::default()
    };
    let outcome =
        mpq::modify_with(&raw, &[], &[], &options).map_err(|e| format!("转换失败：{e}"))?;
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
    lock(&state).convert_applied = Some((outcome.bytes, note.clone()));
    Ok(ConvertDto {
        note,
        applied: true,
    })
}

/// 把已应用的转换写入磁盘，并从写出的文件重新载入归档。
#[tauri::command(async)]
pub fn save_converted(path: String, state: St) -> Result<OpenResult, String> {
    let mut s = lock(&state);
    let (bytes, _) = s.convert_applied.clone().ok_or("请先应用转换。")?;
    std::fs::write(&path, &bytes).map_err(|e| format!("保存失败：{e}"))?;
    Ok(s.load(PathBuf::from(&path), bytes))
}

/// 把转换后的字节载入界面，而不写入磁盘。
#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn discard_convert(state: St) {
    lock(&state).convert_applied = None;
}

// -- 对比 / 补丁 / 合并 -----------------------------------------------------

/// diff 结果的单条记录（kind 相对「新」一侧；
/// 语义随调用场景见 [`diff_with_disk`]）。
#[derive(Serialize, Clone)]
pub struct DiffEntryDto {
    pub name: String,
    /// `"added"` | `"changed"` | `"same"` | `"removed"`。
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_size: Option<u64>,
}

/// [`diff_with_disk`] 的完整结果。
#[derive(Serialize)]
pub struct DiffResultDto {
    pub entries: Vec<DiffEntryDto>,
    /// 旧 / 新一侧未参与对比的未命名条目数。
    pub unnamed_old: usize,
    pub unnamed_new: usize,
    /// 内容比较时读取失败、被保守判为「修改」的条目及原因。
    pub read_errors: Vec<String>,
}

/// 重建一份可读取内容的当前归档快照：受密码保护时用状态中保存的
/// 主密钥重新解锁（未解锁则报错）。快照在全局锁外使用。
fn rebuild_current(
    raw: &[u8],
    master_key: Option<[u8; 32]>,
) -> Result<mpq::Archive<std::io::Cursor<&[u8]>>, String> {
    let mut archive = mpq::Archive::new(std::io::Cursor::new(raw))
        .map_err(|e| format!("归档解析失败：{e}（可先尝试「工具 → 修复头信息」）"))?;
    if archive.is_password_protected() {
        let key = master_key.ok_or("当前归档已锁定，请先输入密码解锁。")?;
        archive
            .unlock_with_master_key(&key)
            .map_err(|e| format!("解锁失败：{e}"))?;
    }
    Ok(archive)
}

/// 把当前归档与磁盘上的另一个归档逐文件对比（比较解压后的内容）。
///
/// * `baseline = true`（生成补丁场景）：磁盘文件是旧基线，当前归档是
///   新文件——`added`/`changed` 是要打进补丁的内容；
/// * `baseline = false`（合并场景）：磁盘文件是合并来源，当前归档是
///   目标——`added` 是来源独有的新文件，`changed` 是同名待覆盖项，
///   `removed` 是仅当前归档有、不会被改动的内容。
///
/// 对比（读 + 解压两侧全部具名文件）在全局锁**外**进行，进度经
/// `on_progress` 上报（phase `"diffing"`）。
#[tauri::command(async)]
pub fn diff_with_disk(
    path: String,
    baseline: bool,
    on_progress: Channel<ImportProgressDto>,
    state: St,
) -> Result<DiffResultDto, String> {
    let (raw, master_key) = {
        let s = lock(&state);
        if s.raw.is_empty() {
            return Err("请先打开一个归档。".to_string());
        }
        (s.raw.clone(), s.master_key)
    };

    let disk_bytes = std::fs::read(&path).map_err(|e| format!("读取失败：{e}"))?;
    let disk = mpq::Archive::new(std::io::Cursor::new(&disk_bytes))
        .map_err(|e| format!("对比文件解析失败：{e}"))?;
    if disk.is_password_protected() {
        return Err(
            "对比文件受密码保护，无法读取内容（请先用「工具 → 解除密码保护」处理它）"
                .to_string(),
        );
    }
    let current = rebuild_current(&raw, master_key)?;

    let tap = ProgressTap::new(on_progress);
    let cb = |done: usize, total: usize| tap.emit("diffing", done as u32, total as u32);
    let report = if baseline {
        mpq::diff_archives(&disk, &current, Some(&cb))
    } else {
        mpq::diff_archives(&current, &disk, Some(&cb))
    }
    .map_err(|e| format!("对比失败：{e}"))?;

    Ok(DiffResultDto {
        entries: report
            .entries
            .iter()
            .map(|e| DiffEntryDto {
                name: e.name.clone(),
                kind: match e.kind {
                    mpq::DiffKind::Added => "added",
                    mpq::DiffKind::Changed => "changed",
                    mpq::DiffKind::Same => "same",
                    mpq::DiffKind::Removed => "removed",
                }
                .to_string(),
                old_size: e.old_size,
                new_size: e.new_size,
            })
            .collect(),
        unnamed_old: report.unnamed_old,
        unnamed_new: report.unnamed_new,
        read_errors: report.read_errors,
    })
}

/// 从当前归档提取 `names` 列出的文件，生成一份独立的补丁归档并写入
/// `out_path`。提取与整档重写都在全局锁**外**进行；读取 → 编码 → 自检
/// 的进度经 `on_progress` 上报（与导入相同的阶段名）。
#[tauri::command(async)]
pub fn build_patch(
    names: Vec<String>,
    out_path: String,
    version: u32,
    on_progress: Channel<ImportProgressDto>,
    state: St,
) -> Result<NoteDto, String> {
    if names.is_empty() {
        return Err("没有选择任何文件。".to_string());
    }
    let (raw, master_key) = {
        let s = lock(&state);
        if s.raw.is_empty() {
            return Err("请先打开一个归档。".to_string());
        }
        (s.raw.clone(), s.master_key)
    };
    let current = rebuild_current(&raw, master_key)?;
    let version = format_version(version)?;

    let tap = ProgressTap::new(on_progress);
    let read_cb = |done: usize, total: usize| tap.emit("reading", done as u32, total as u32);
    let modify_cb = |phase: ModifyPhase, done: usize, total: usize| {
        let name = match phase {
            ModifyPhase::Encoding => "encoding",
            ModifyPhase::Verifying => "verifying",
        };
        tap.emit(name, done as u32, total as u32);
    };
    let outcome = tasks::build_patch(&current, &names, version, Some(&read_cb), Some(&modify_cb))
        .map_err(|e| format!("生成补丁失败：{e}"))?;
    std::fs::write(&out_path, &outcome.bytes).map_err(|e| format!("保存失败：{e}"))?;
    Ok(NoteDto {
        note: format!(
            "补丁已生成：{}（{} 个文件，v{}）",
            out_path,
            names.len(),
            version.as_u32()
        ),
    })
}

/// 「合并」的数据准备：从磁盘上的来源归档读出选中名字的内容并写入
/// 临时文件，返回值供前端按原名排队为「待导入」（同名条目在「修改
/// 归档」应用时自动替换——即覆盖）。逐条读取：任何一条失败都中止，
/// 避免只合并一半。
#[tauri::command(async)]
pub fn stage_merge_files(
    source: String,
    names: Vec<String>,
    state: St,
) -> Result<Vec<StagedDto>, String> {
    let bytes = std::fs::read(&source).map_err(|e| format!("读取失败：{e}"))?;
    let archive = mpq::Archive::new(std::io::Cursor::new(&bytes))
        .map_err(|e| format!("来源归档解析失败：{e}"))?;
    if archive.is_password_protected() {
        return Err(
            "来源归档受密码保护，无法读取内容（请先用「工具 → 解除密码保护」处理它）"
                .to_string(),
        );
    }

    let dir = std::env::temp_dir().join("mpqtool-merge");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建临时目录失败：{e}"))?;
    let s = lock(&state);
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(names.len());
    for name in &names {
        let data = archive
            .read_file(name)
            .map_err(|e| format!("读取 {name} 失败：{e}"))?;
        // 临时文件按净化后的归档内名命名（\ / 等替换为 _）；
        // 净化撞车时追加序号，保证不同条目不互相覆盖。
        let mut sanitized: String = name
            .chars()
            .map(|c| {
                matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_')
                    .then_some(c)
                    .unwrap_or('_')
            })
            .collect();
        let mut seq = 1;
        while !used.insert(sanitized.clone()) {
            seq += 1;
            sanitized = format!("{sanitized}_{seq}");
        }
        let file = dir.join(sanitized);
        std::fs::write(&file, &data).map_err(|e| format!("写入临时文件失败：{e}"))?;
        let replaces = s
            .archive
            .as_ref()
            .is_some_and(|a| a.find_file(name).is_some());
        out.push(StagedDto {
            name: name.clone(),
            path: file.to_string_lossy().to_string(),
            size: data.len(),
            replaces,
        });
    }
    Ok(out)
}
