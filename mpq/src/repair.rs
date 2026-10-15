//! MPQ 头部诊断与修复。
//!
//! 修复头部字段受损归档的*容器*元数据 —— 损坏可能来自下载被截断、
//! 地图保护器工具或有缺陷的写入器：
//!
//! * 非法的 `format_version` / `header_size` / sector 指数
//! * 错误的 `archive_size` / `archive_size64`
//! * hash table / block table 位置越出 EOF，或指向了错误的地方
//!   （通过加密内容扫描重新定位，类似 StormLib 的
//!   "malformed archive" 恢复）
//! * 过大的 `hash_table_size` / `block_table_size`（保护器滥用）
//! * v2/v3 字段（hi-block、HET、BET 位置）越出 EOF
//!
//! 修复绝不触碰文件数据或表内容 —— 只在调用方控制的缓冲区上
//! 原地重写头部字段。

use crate::crypt::{decrypt_bytes, KEY_BLOCK_TABLE, KEY_HASH_TABLE};
use crate::digest;
use crate::error::{Error, Result};
use crate::header::{
    MpqHeader, UserDataHeader, HEADER_MD5_SPAN, HEADER_SIZE_V1, HEADER_SIZE_V2, HEADER_SIZE_V3,
    HEADER_SIZE_V4, MPQ_MAGIC, USERDATA_MAGIC,
};
use crate::tables::{BlockEntry, HashEntry, FILE_EXISTS};

/// 检测到的一个问题。当问题无法自动修复时，`action` 为 `None`。
#[derive(Debug, Clone)]
pub struct RepairIssue {
    pub field: String,
    pub problem: String,
    pub action: Option<String>,
}

impl RepairIssue {
    fn fixed(field: &str, problem: impl Into<String>, action: impl Into<String>) -> Self {
        RepairIssue {
            field: field.to_string(),
            problem: problem.into(),
            action: Some(action.into()),
        }
    }
    fn fatal(field: &str, problem: impl Into<String>) -> Self {
        RepairIssue {
            field: field.to_string(),
            problem: problem.into(),
            action: None,
        }
    }
}

/// 一次 [`repair`] 运行的结果。
#[derive(Debug, Clone)]
pub struct RepairReport {
    /// 修复后的 MPQ 头部在缓冲区中的偏移。
    pub base: u64,
    pub issues: Vec<RepairIssue>,
    /// 是否确实重写过任何字节。
    pub changed: bool,
}

impl RepairReport {
    /// 未能自动修复的问题。
    pub fn unfixed(&self) -> Vec<&RepairIssue> {
        self.issues.iter().filter(|i| i.action.is_none()).collect()
    }
}

// ---------------------------------------------------------------------------
// 字节读取辅助函数

fn rd16(d: &[u8], off: usize) -> u16 {
    if off + 2 > d.len() {
        return 0;
    }
    u16::from_le_bytes([d[off], d[off + 1]])
}
fn rd32(d: &[u8], off: usize) -> u32 {
    if off + 4 > d.len() {
        return 0;
    }
    u32::from_le_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]])
}
fn rd64(d: &[u8], off: usize) -> u64 {
    if off + 8 > d.len() {
        return 0;
    }
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[off..off + 8]);
    u64::from_le_bytes(b)
}
/// 在原始字节中定位 MPQ 头部：偏移 0 处、位于 `MPQ\x1B`
/// user-data 头部之后，或通过 0x200 对齐扫描找到（最多 128 MB）。
pub fn locate_header(data: &[u8]) -> Option<u64> {
    let n = data.len();
    if n < HEADER_SIZE_V1 as usize {
        return None;
    }
    let magic_at = |off: usize| -> bool {
        off + 4 <= n && u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) == MPQ_MAGIC
    };
    if magic_at(0) {
        return Some(0);
    }
    let ud_magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if ud_magic == USERDATA_MAGIC {
        if let Some(ud) = UserDataHeader::parse(&data[..n.min(16)]) {
            let t = ud.archive_offset as usize;
            if magic_at(t) {
                return Some(t as u64);
            }
        }
    }
    let limit = n.min(0x0800_0000);
    let mut off = 0usize;
    while off + HEADER_SIZE_V1 as usize <= limit {
        let mut probe = off;
        while probe + 0x20 <= limit && probe < off + 0x10_0000 {
            if u32::from_le_bytes([data[probe], data[probe + 1], data[probe + 2], data[probe + 3]])
                == USERDATA_MAGIC
            {
                if let Some(ud) = UserDataHeader::parse(&data[probe..n.min(probe + 16)]) {
                    let t = probe + ud.archive_offset as usize;
                    if magic_at(t) {
                        return Some(t as u64);
                    }
                }
            } else if magic_at(probe) {
                if let Ok((h, _)) = MpqHeader::parse(&data[probe..n.min(probe + HEADER_SIZE_V4 as usize)]) {
                    if h.plausible(n as u64, probe as u64) {
                        return Some(probe as u64);
                    }
                }
            }
            probe += 0x200;
        }
        if off + 0x10_0000 > limit {
            break;
        }
        off += 0x10_0000;
    }
    None
}

// ---------------------------------------------------------------------------
// 表评分 / 探测定位

/// 解密位于绝对地址 `abs` 处、共 `entries` 项的 hash table 探测数据，
/// 并为结果的可信度打分（空闲项模式或一致的占用项）。
/// 返回 0..=1 之间的分数。
fn score_hash_table(data: &[u8], abs: usize, entries: usize, block_bound: u32) -> f32 {
    let probe = entries.clamp(1, 16);
    let len = probe * 16;
    if abs >= data.len() || abs + len > data.len() {
        return 0.0;
    }
    let mut buf = data[abs..abs + len].to_vec();
    decrypt_bytes(&mut buf, KEY_HASH_TABLE);

    let mut plausible = 0usize;
    for chunk in buf.chunks_exact(16) {
        let e = HashEntry {
            name1: u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]),
            name2: u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
            locale: u16::from_le_bytes([chunk[8], chunk[9]]),
            platform: chunk[10],
            flags: chunk[11],
            block_index: u32::from_le_bytes([chunk[12], chunk[13], chunk[14], chunk[15]]),
        };
        let free = e.name1 == 0
            && e.name2 == 0
            && e.locale == 0
            && e.platform == 0
            && e.flags == 0
            && e.block_index == u32::MAX;
        let occupied =
            e.name1 != 0 && e.name2 != 0 && e.block_index < block_bound.max(1) && !e.is_free();
        if free || occupied {
            plausible += 1;
        }
    }
    plausible as f32 / probe as f32
}

/// block table 同理。
fn score_block_table(data: &[u8], abs: usize, entries: usize, span: u64) -> f32 {
    let probe = entries.clamp(1, 16);
    let len = probe * 16;
    if abs >= data.len() || abs + len > data.len() {
        return 0.0;
    }
    let mut buf = data[abs..abs + len].to_vec();
    decrypt_bytes(&mut buf, KEY_BLOCK_TABLE);

    let mut plausible = 0usize;
    for chunk in buf.chunks_exact(16) {
        let e = BlockEntry {
            pos: u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]),
            csize: u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
            fsize: u32::from_le_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]),
            flags: u32::from_le_bytes([chunk[12], chunk[13], chunk[14], chunk[15]]),
        };
        let free = e.pos == 0 && e.csize == 0 && e.fsize == 0 && e.flags == 0;
        let ok = e.flags == 0
            || (e.flags & FILE_EXISTS != 0
                && (e.pos as u64) <= span
                && e.csize as u64 <= span + 0x10_0000);
        if free || ok {
            plausible += 1;
        }
    }
    plausible as f32 / probe as f32
}

/// 从头部末尾开始、按 4 字节对齐升序扫描缓冲区，
/// 寻找能解密成可信 hash table / block table 的表。
fn find_table<F>(data: &[u8], from: usize, entries: usize, score: F) -> Option<usize>
where
    F: Fn(&[u8], usize, usize) -> f32,
{
    if data.len() > 0x0800_0000 {
        // 对超大文件限制扫描范围：健康归档的表位于靠近末尾处，
        // 因此除头部附近外，也扫描最后 32 MB。
        let regions = [
            (from, (from + 0x0200_0000).min(data.len())),
            (data.len().saturating_sub(0x0200_0000).max(from), data.len()),
        ];
        for (a, b) in regions {
            if let Some(hit) = scan_range(data, a, b, entries, &score) {
                return Some(hit);
            }
        }
        return None;
    }
    scan_range(data, from, data.len(), entries, &score)
}

fn scan_range<F>(data: &[u8], a: usize, b: usize, entries: usize, score: F) -> Option<usize>
where
    F: Fn(&[u8], usize, usize) -> f32,
{
    let probe_len = entries.clamp(1, 16) * 16;
    let mut off = a - (a % 4);
    while off + probe_len <= b {
        if score(data, off, entries) >= 0.9 {
            return Some(off);
        }
        off += 4;
    }
    None
}

// ---------------------------------------------------------------------------
// 分析 / 修复

struct Patch {
    offset: usize,
    bytes: Vec<u8>,
}

/// 分析缓冲区并返回发现的全部问题；当 `apply` 置位时，
/// 修复内容也会写入 `data`。
fn run(data: &mut [u8], apply: bool, deep: bool) -> Result<RepairReport> {
    let base = locate_header(data).ok_or(Error::NotAnArchive)? as usize;
    let n = data.len();
    if base + HEADER_SIZE_V1 as usize > n {
        return Err(Error::Corrupt("header truncated below 0x20 bytes"));
    }

    let mut issues: Vec<RepairIssue> = Vec::new();
    let mut patches: Vec<Patch> = Vec::new();
    let h = HEADER_SIZE_V1 as usize; // `base` 处可用的头部字节数

    let mut version = rd16(data, base + 0x0C);
    let mut header_size = rd32(data, base + 0x04) as usize;

    // --- format_version ---------------------------------------------------
    if version > 3 {
        let inferred: u16 = if header_size >= HEADER_SIZE_V3 as usize {
            2
        } else if header_size >= HEADER_SIZE_V2 as usize {
            1
        } else {
            0
        };
        issues.push(RepairIssue::fixed(
            "format_version",
            format!("非法版本号 {version}"),
            format!("按 header_size 推断为 v{}", inferred + 1),
        ));
        patches.push(Patch {
            offset: base + 0x0C,
            bytes: inferred.to_le_bytes().to_vec(),
        });
        version = inferred;
    }

    // --- header_size ------------------------------------------------------
    if version == 0 && header_size != HEADER_SIZE_V1 as usize {
        issues.push(RepairIssue::fixed(
            "header_size",
            format!("v1 头部长度应为 0x20，实际 {header_size:#x}"),
            "改写为 0x20",
        ));
        patches.push(Patch {
            offset: base + 0x04,
            bytes: { HEADER_SIZE_V1 }.to_le_bytes().to_vec(),
        });
        header_size = HEADER_SIZE_V1 as usize;
    } else if version == 1 && header_size != HEADER_SIZE_V2 as usize {
        // StormLib 会把长度错误的 v2 头部降级为 v1。
        issues.push(RepairIssue::fixed(
            "header_size/format_version",
            format!("v2 头部长度应为 0x2C，实际 {header_size:#x}"),
            "降级为 v1 头部（0x20），与 StormLib 一致",
        ));
        patches.push(Patch {
            offset: base + 0x0C,
            bytes: 0u16.to_le_bytes().to_vec(),
        });
        patches.push(Patch {
            offset: base + 0x04,
            bytes: { HEADER_SIZE_V1 }.to_le_bytes().to_vec(),
        });
        version = 0;
        header_size = HEADER_SIZE_V1 as usize;
    } else if version == 3 && header_size != HEADER_SIZE_V4 as usize {
        issues.push(RepairIssue::fixed(
            "header_size",
            format!("v4 头部长度应为 0xD0，实际 {header_size:#x}"),
            "改写为 0xD0",
        ));
        patches.push(Patch {
            offset: base + 0x04,
            bytes: { HEADER_SIZE_V4 }.to_le_bytes().to_vec(),
        });
        header_size = HEADER_SIZE_V4 as usize;
    }
    let header_limit = base + header_size.max(h);

    // --- 扇区指数（sector exponent）------------------------------------------
    let block_size = rd16(data, base + 0x0E);
    if block_size > 0x10 {
        issues.push(RepairIssue::fixed(
            "block_size",
            format!("扇区指数 {block_size} 超出合理范围"),
            "改写为 3（4096 字节扇区）",
        ));
        patches.push(Patch {
            offset: base + 0x0E,
            bytes: 3u16.to_le_bytes().to_vec(),
        });
    }

    // --- hash table / block table ---------------------------------------------
    let mut hash_pos = rd32(data, base + 0x10) as u64;
    let mut hash_hi = if version >= 1 { rd16(data, base + 0x28) as u64 } else { 0 };
    let hash_entries = rd32(data, base + 0x18) as usize;

    let mut block_pos = rd32(data, base + 0x14) as u64;
    let mut block_hi = if version >= 1 { rd16(data, base + 0x2A) as u64 } else { 0 };
    let block_entries = rd32(data, base + 0x1C) as usize;

    let block_bound = block_entries.saturating_mul(4).max(0x100) as u32;
    let span = (n - base) as u64;

    // hash table 部分。
    if hash_entries > 0 {
        let abs = base as u64 + (hash_pos | (hash_hi << 32));
        let mut fixed_abs: Option<u64> = None;
        if abs as usize >= n {
            if deep {
                if let Some(hit) =
                    find_table(data, header_limit, hash_entries, |d, a, e| {
                        score_hash_table(d, a, e, block_bound)
                    })
                {
                    fixed_abs = Some(hit as u64);
                }
            }
            match fixed_abs {
                Some(hit) => {
                    issues.push(RepairIssue::fixed(
                        "hash_table_pos",
                        format!("哈希表位置 {abs:#x} 越界"),
                        format!("扫描定位到 {hit:#x}"),
                    ));
                    hash_pos = hit - base as u64;
                    hash_hi = 0;
                    patches.push(Patch {
                        offset: base + 0x10,
                        bytes: (hash_pos as u32).to_le_bytes().to_vec(),
                    });
                    if version >= 1 {
                        patches.push(Patch {
                            offset: base + 0x28,
                            bytes: 0u16.to_le_bytes().to_vec(),
                        });
                    }
                }
                None => issues.push(RepairIssue::fatal(
                    "hash_table_pos",
                    format!("哈希表位置 {abs:#x} 越界且未能重新定位"),
                )),
            }
        } else if deep {
            // 位置在范围内但仍可能错误（保护器地图）：
            // 校验可解密性，不可信时重新定位。
            let score = score_hash_table(data, abs as usize, hash_entries, block_bound);
            if score < 0.5 {
                if let Some(hit) =
                    find_table(data, header_limit, hash_entries, |d, a, e| {
                        score_hash_table(d, a, e, block_bound)
                    })
                {
                    if hit as u64 != abs {
                        issues.push(RepairIssue::fixed(
                            "hash_table_pos",
                            format!("哈希表在 {abs:#x} 处解密校验失败（得分 {score:.2}）"),
                            format!("重新定位到 {hit:#x}"),
                        ));
                        hash_pos = hit as u64 - base as u64;
                        hash_hi = 0;
                        patches.push(Patch {
                            offset: base + 0x10,
                            bytes: (hash_pos as u32).to_le_bytes().to_vec(),
                        });
                        if version >= 1 {
                            patches.push(Patch {
                                offset: base + 0x28,
                                bytes: 0u16.to_le_bytes().to_vec(),
                            });
                        }
                    }
                }
            }
        }

        // 表项数必须能容纳进文件。
        let abs = base as u64 + (hash_pos | (hash_hi << 32));
        if (abs as usize) < n {
            let end = abs as usize + hash_entries * 16;
            if end > n {
                // hash table 的大小必须是 2 的幂（读取时按模探测），
                // 因此把截断后的数量向下取到 2 的幂。
                let mut trimmed = (n - abs as usize) / 16;
                // 不超过 trimmed 的最大 2 的幂（兼容 stable 的写法）
                while trimmed > 1 && !trimmed.is_power_of_two() {
                    trimmed &= trimmed - 1;
                }
                if trimmed > 0 {
                    issues.push(RepairIssue::fixed(
                        "hash_table_size",
                        format!("{hash_entries} 项越出文件末尾"),
                        format!("截断为 {trimmed} 项（2 的幂）"),
                    ));
                    patches.push(Patch {
                        offset: base + 0x18,
                        bytes: (trimmed as u32).to_le_bytes().to_vec(),
                    });
                } else {
                    issues.push(RepairIssue::fatal(
                        "hash_table_size",
                        "哈希表在文件末尾放不下任何完整项",
                    ));
                }
            }
        }
    }

    // block table 部分。
    if block_entries > 0 {
        let abs = base as u64 + (block_pos | (block_hi << 32));
        let mut fixed_abs: Option<u64> = None;
        if abs as usize >= n {
            if deep {
                if let Some(hit) =
                    find_table(data, header_limit, block_entries, |d, a, e| {
                        score_block_table(d, a, e, span)
                    })
                {
                    fixed_abs = Some(hit as u64);
                }
            }
            match fixed_abs {
                Some(hit) => {
                    issues.push(RepairIssue::fixed(
                        "block_table_pos",
                        format!("块表位置 {abs:#x} 越界"),
                        format!("扫描定位到 {hit:#x}"),
                    ));
                    block_pos = hit - base as u64;
                    block_hi = 0;
                    patches.push(Patch {
                        offset: base + 0x14,
                        bytes: (block_pos as u32).to_le_bytes().to_vec(),
                    });
                    if version >= 1 {
                        patches.push(Patch {
                            offset: base + 0x2A,
                            bytes: 0u16.to_le_bytes().to_vec(),
                        });
                    }
                }
                None => issues.push(RepairIssue::fatal(
                    "block_table_pos",
                    format!("块表位置 {abs:#x} 越界且未能重新定位"),
                )),
            }
        } else if deep {
            let score = score_block_table(data, abs as usize, block_entries, span);
            if score < 0.5 {
                if let Some(hit) =
                    find_table(data, header_limit, block_entries, |d, a, e| {
                        score_block_table(d, a, e, span)
                    })
                {
                    if hit as u64 != abs {
                        issues.push(RepairIssue::fixed(
                            "block_table_pos",
                            format!("块表在 {abs:#x} 处解密校验失败（得分 {score:.2}）"),
                            format!("重新定位到 {hit:#x}"),
                        ));
                        block_pos = hit as u64 - base as u64;
                        block_hi = 0;
                        patches.push(Patch {
                            offset: base + 0x14,
                            bytes: (block_pos as u32).to_le_bytes().to_vec(),
                        });
                        if version >= 1 {
                            patches.push(Patch {
                                offset: base + 0x2A,
                                bytes: 0u16.to_le_bytes().to_vec(),
                            });
                        }
                    }
                }
            }
        }

        let abs = base as u64 + (block_pos | (block_hi << 32));
        if (abs as usize) < n {
            let end = abs as usize + block_entries * 16;
            if end > n {
                let trimmed = (n - abs as usize) / 16;
                if trimmed > 0 {
                    issues.push(RepairIssue::fixed(
                        "block_table_size",
                        format!("{block_entries} 项越出文件末尾"),
                        format!("截断为 {trimmed} 项"),
                    ));
                    patches.push(Patch {
                        offset: base + 0x1C,
                        bytes: (trimmed as u32).to_le_bytes().to_vec(),
                    });
                } else {
                    issues.push(RepairIssue::fatal(
                        "block_table_size",
                        "块表在文件末尾放不下任何完整项",
                    ));
                }
            }
        }
    }

    // --- v2 字段 -----------------------------------------------------------
    if version >= 1 && header_limit >= base + 0x2C {
        let hi_pos = rd64(data, base + 0x20);
        if hi_pos != 0 && base as u64 + hi_pos >= n as u64 {
            issues.push(RepairIssue::fixed(
                "hi_block_table_pos",
                format!("hi-block 表位置 {hi_pos:#x} 越界"),
                "清零",
            ));
            patches.push(Patch {
                offset: base + 0x20,
                bytes: 0u64.to_le_bytes().to_vec(),
            });
        }
    }

    // --- v3 字段 -----------------------------------------------------------
    if version >= 2 && header_limit >= base + 0x44 {
        let bet = rd64(data, base + 0x34);
        let het = rd64(data, base + 0x3C);
        for (name, off, pos) in [("bet_table_pos", 0x34usize, bet), ("het_table_pos", 0x3C, het)] {
            if pos != 0 && base as u64 + pos >= n as u64 {
                issues.push(RepairIssue::fixed(
                    name,
                    format!("位置 {pos:#x} 越界"),
                    "清零（该表将视为不存在）",
                ));
                patches.push(Patch {
                    offset: base + off,
                    bytes: 0u64.to_le_bytes().to_vec(),
                });
            }
        }
        let size64 = rd64(data, base + 0x2C);
        if size64 != span {
            issues.push(RepairIssue::fixed(
                "archive_size64",
                format!("64 位归档大小 {size64:#x} 与实际 {span:#x} 不符"),
                format!("改写为 {span:#x}"),
            ));
            patches.push(Patch {
                offset: base + 0x2C,
                bytes: span.to_le_bytes().to_vec(),
            });
        }
    }

    // --- 归档大小（v1 的 32 位字段，始终检查）---------------------------------
    let size32 = rd32(data, base + 0x08) as u64;
    let expect32 = span.min(u32::MAX as u64);
    if size32 != expect32 {
        issues.push(RepairIssue::fixed(
            "archive_size",
            format!("归档大小 {size32:#x} 与实际 {expect32:#x} 不符"),
            format!("改写为 {expect32:#x}"),
        ));
        patches.push(Patch {
            offset: base + 0x08,
            bytes: (expect32 as u32).to_le_bytes().to_vec(),
        });
    }

    // --- v4 头部 MD5 -----------------------------------------------------------
    // v4 头部对自身 0x00..0xC0 字节附带 MD5，因此上面的任何头部修复
    // 都会使该 MD5 失效（StormLib 会拒绝打开这类归档）。
    // 在应用全部待处理补丁*之后*，针对修复后的头部重新计算该 MD5。
    if version == 3 && base + HEADER_SIZE_V4 as usize <= n {
        let mut window = data[base..base + HEADER_SIZE_V4 as usize].to_vec();
        let mut header_patched = false;
        for p in &patches {
            let rel = p.offset.wrapping_sub(base);
            if rel < HEADER_MD5_SPAN {
                let end = rel + p.bytes.len();
                if end <= window.len() {
                    window[rel..end].copy_from_slice(&p.bytes);
                    header_patched = true;
                }
            }
        }
        let expected = digest::md5_digest(&window[..HEADER_MD5_SPAN]);
        let current: [u8; 16] = window[HEADER_MD5_SPAN..HEADER_MD5_SPAN + 16]
            .try_into()
            .expect("16 bytes");
        if current != expected && (digest::md5_present(&current) || header_patched) {
            let why = if digest::md5_present(&current) {
                "头部 MD5 与头部内容不符（或头部字段已修复）"
            } else {
                "头部字段已修复，原头部 MD5 缺失"
            };
            issues.push(RepairIssue::fixed("MD5_MpqHeader", why, "按修复后的头部重算"));
            patches.push(Patch {
                offset: base + HEADER_MD5_SPAN,
                bytes: expected.to_vec(),
            });
        }
    }

    // --- 应用补丁 ---------------------------------------------------------------
    let changed = !patches.is_empty();
    if apply {
        for p in patches {
            let end = p.offset + p.bytes.len();
            if end <= data.len() {
                data[p.offset..end].copy_from_slice(&p.bytes);
            }
        }
    }

    Ok(RepairReport {
        base: base as u64,
        issues,
        changed,
    })
}

/// 诊断缓冲区而不做修改（repair 的预演）。
pub fn diagnose(data: &[u8]) -> Result<RepairReport> {
    let mut copy = data.to_vec();
    run(&mut copy, false, true)
}

/// 原地修复缓冲区。`deep` 会启用加密内容扫描，
/// 重新定位声明位置错误的 hash table / block table。
pub fn repair(data: &mut [u8], deep: bool) -> Result<RepairReport> {
    run(data, true, deep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locator_finds_plain_userdata_and_scanned() {
        let mut buf = vec![0u8; 0x600];
        // 普通头部位于偏移 0 处
        buf[0..4].copy_from_slice(&MPQ_MAGIC.to_le_bytes());
        assert_eq!(locate_header(&buf), Some(0));

        // 位于 0 处的 userdata 头部，指向 0x200
        let mut buf = vec![0xEEu8; 0x400];
        buf[0..4].copy_from_slice(&USERDATA_MAGIC.to_le_bytes());
        buf[4..8].copy_from_slice(&0x100u32.to_le_bytes()); // 数据大小
        buf[8..12].copy_from_slice(&0x200u32.to_le_bytes()); // 头部偏移
        buf[12..16].copy_from_slice(&0x10u32.to_le_bytes());
        buf[0x200..0x204].copy_from_slice(&MPQ_MAGIC.to_le_bytes());
        buf[0x204..0x208].copy_from_slice(&0x20u32.to_le_bytes());
        assert_eq!(locate_header(&buf), Some(0x200));

        // 内嵌（扫描）：前缀是垃圾数据，头部字段最少但有效
        let mut buf = vec![0x11u8; 0x400];
        buf[0x200..0x204].copy_from_slice(&MPQ_MAGIC.to_le_bytes());
        buf[0x204..0x208].copy_from_slice(&0x20u32.to_le_bytes()); // 头部长度
        buf[0x20C..0x20E].copy_from_slice(&0u16.to_le_bytes()); // 版本 1
        buf[0x20E..0x210].copy_from_slice(&3u16.to_le_bytes()); // 扇区指数
        assert_eq!(locate_header(&buf), Some(0x200));
    }
}
