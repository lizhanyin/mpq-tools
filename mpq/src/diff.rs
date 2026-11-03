//! 对比两个 MPQ 归档，计算逐文件的差异。
//!
//! 以 MPQ 名字规范化（大小写不敏感、`/` 与 `\` 等价）为键配对两侧的
//! 具名条目：仅新侧有的为「新增」，两侧都有而解压后内容不同的是
//! 「修改」，逐字节相同的是「相同」，仅旧侧有的是「移除」。内部文件
//! （`(listfile)`、`(attributes)` 等，见 [`SPECIAL_FILES`]）与未命名
//! 条目不参与对比——前者由修改器自动管理，后者没有名字无法配对
//! （数量单独计入 [`DiffReport`]）。
//!
//! 内容比较读的是**解压后**的字节，因此压缩方式 / 加密状态的变化
//! 不影响判定。解压后大小不同的条目直接判「修改」（快路径，无需
//! 读取）；大小相同才读取两侧内容逐字节比较。单侧读取失败的条目
//! 保守地按「修改」处理并记入 [`DiffReport::read_errors`]——宁可多
//! 报差异，绝不把读不到的文件当作相同。
//!
//! 典型用途（由上层工具组合）：
//! * 生成补丁：取「新增 + 修改」文件的新版本写入独立的 patch 归档，
//!   靠游戏 / 工具的补丁优先加载覆盖旧归档；
//! * 合并归档：「新增」直接导入目标，「修改」按用户选择覆盖。

use std::collections::BTreeMap;
use std::io::{Read, Seek};

use crate::archive::Archive;
use crate::crypt::normalize_name;
use crate::FileEntry;
use crate::SPECIAL_FILES;

/// 单个文件名相对「新」侧的差异类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    /// 仅新归档有（旧归档中没有）。
    Added,
    /// 两侧都有，解压后内容不同。
    Changed,
    /// 两侧都有且内容逐字节相同。
    Same,
    /// 仅旧归档有。
    Removed,
}

/// 一个名字的差异记录。
#[derive(Debug, Clone)]
pub struct DiffEntry {
    /// 显示名（Added/Changed/Same 取新侧拼写，Removed 取旧侧拼写；
    /// 两侧拼写不同时以新侧为准）。
    pub name: String,
    pub kind: DiffKind,
    /// 旧侧解压后大小；该侧没有此文件时为 `None`。
    pub old_size: Option<u64>,
    /// 新侧解压后大小；该侧没有此文件时为 `None`。
    pub new_size: Option<u64>,
}

/// [`diff_archives`] 的完整结果。
#[derive(Debug, Clone, Default)]
pub struct DiffReport {
    /// 全部差异，按名字排序（大小写不敏感，稳定）。
    pub entries: Vec<DiffEntry>,
    /// 旧 / 新侧未参与对比的未命名条目数。
    pub unnamed_old: usize,
    pub unnamed_new: usize,
    /// 内容比较时读取失败、被保守判为「修改」的条目及原因。
    pub read_errors: Vec<String>,
}

/// 一侧参与对比的条目：规范化名 →（显示名、解压后大小、条目）。
/// 同名的多个条目（如 locale 变体）保留首个——差异以名字为粒度。
struct Side {
    by_name: BTreeMap<Vec<u8>, (String, u64, FileEntry)>,
    unnamed: usize,
}

/// 收集一侧的具名条目（排除内部文件），统计未命名条目数。
fn collect_side<R: Read + Seek>(archive: &Archive<R>) -> Side {
    let mut side = Side {
        by_name: BTreeMap::new(),
        unnamed: 0,
    };
    for entry in archive.entries() {
        // 名字先克隆：条目本体随后要移入 map，不能继续借用。
        let Some(name) = entry.name.clone() else {
            side.unnamed += 1;
            continue;
        };
        if SPECIAL_FILES.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
            continue;
        }
        let norm = normalize_name(&name);
        let size = entry.size as u64;
        side.by_name
            .entry(norm)
            .or_insert_with(|| (name, size, entry));
    }
    side
}

/// 对比两个归档的全部具名文件。
///
/// `progress(done, total)` 在处理期间回调：`total` 为两侧具名条目总数，
/// 快速判定（名字 / 大小即可分类）的条目瞬时累计，需要读取内容比较的
/// 条目逐个累计——因此大归档下进度条会先快进再匀速推进。回调可能
/// 频繁触发，节流由调用方负责。
pub fn diff_archives<R1: Read + Seek, R2: Read + Seek>(
    old: &Archive<R1>,
    new: &Archive<R2>,
    progress: Option<&(dyn Fn(usize, usize) + Sync)>,
) -> crate::Result<DiffReport> {
    let old_side = collect_side(old);
    let new_side = collect_side(new);
    let total = old_side.by_name.len() + new_side.by_name.len();

    let mut report = DiffReport {
        unnamed_old: old_side.unnamed,
        unnamed_new: new_side.unnamed,
        ..DiffReport::default()
    };
    let mut done = 0usize;

    // 大小相同、需要读取内容才能分辨的配对（两侧条目）。
    let mut probe: Vec<(FileEntry, FileEntry)> = Vec::new();

    // --- 新侧：新增 / 修改（快路径）/ 待读取比较 -------------------------
    for (norm, (display, new_size, new_entry)) in &new_side.by_name {
        match old_side.by_name.get(norm) {
            None => report.entries.push(DiffEntry {
                name: display.clone(),
                kind: DiffKind::Added,
                old_size: None,
                new_size: Some(*new_size),
            }),
            Some((_, old_size, old_entry)) if old_size != new_size => {
                report.entries.push(DiffEntry {
                    name: display.clone(),
                    kind: DiffKind::Changed,
                    old_size: Some(*old_size),
                    new_size: Some(*new_size),
                })
            }
            Some((_, _, old_entry)) => {
                probe.push((old_entry.clone(), new_entry.clone()));
            }
        }
        done += 1;
        if let Some(cb) = progress {
            cb(done.min(total), total);
        }
    }

    // --- 仅旧侧：移除 ------------------------------------------------------
    for (norm, (display, old_size, _)) in &old_side.by_name {
        if !new_side.by_name.contains_key(norm) {
            report.entries.push(DiffEntry {
                name: display.clone(),
                kind: DiffKind::Removed,
                old_size: Some(*old_size),
                new_size: None,
            });
        }
        done += 1;
        if let Some(cb) = progress {
            cb(done.min(total), total);
        }
    }

    // --- 大小相同的配对：读取两侧解压字节逐字节比较 -----------------------
    // （Archive 内部以 RefCell 持有读取器，不是 Sync，无法直接交给
    // rayon 并行；串行读取，进度逐条上报。）
    for (old_entry, new_entry) in probe {
        let display = new_entry
            .name
            .clone()
            .or_else(|| old_entry.name.clone())
            .unwrap_or_default();
        let (old_size, new_size) = (old_entry.size as u64, new_entry.size as u64);
        let outcome = (|| -> crate::Result<bool> {
            let old_bytes = old.read_entry(&old_entry)?;
            let new_bytes = new.read_entry(&new_entry)?;
            Ok(old_bytes == new_bytes)
        })();
        let kind = match outcome {
            Ok(true) => DiffKind::Same,
            Ok(false) => DiffKind::Changed,
            Err(e) => {
                report.read_errors.push(format!("{display}: {e}"));
                DiffKind::Changed // 读不到内容时保守按修改处理
            }
        };
        report.entries.push(DiffEntry {
            name: display,
            kind,
            old_size: Some(old_size),
            new_size: Some(new_size),
        });
        done += 1;
        if let Some(cb) = progress {
            cb(done.min(total), total);
        }
    }

    // 按名字排序（大小写不敏感；以小写名为键稳定排序）。
    report.entries.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(report)
}
