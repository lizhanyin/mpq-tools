//! 创建新的空 MPQ 归档（头部版本 v1–v4）。
//!
//! 结果是一个符合规范的极简容器：对应版本的头部、一张完全加密且
//! 全为空闲项的经典哈希表，以及一张加密的块表，其中只含单个空闲
//! （幽灵）项。零项的块表会被要求文件表非空的读取器拒绝（本库
//! 也不例外）；空闲块是该格式自带的“未使用槽位”标记，因此归档
//! 在任何地方都能打开，之后 [`crate::modify`] 便能像向其他归档导入
//! 文件那样向其中导入文件。v4 容器还会通过
//! [`crate::modify::finish_v4_header`] 携带完整的 MD5 校验和
//! （各表 + 头部）。

use std::io::Cursor;

use crate::archive::Archive;
use crate::crypt::{encrypt_bytes, KEY_BLOCK_TABLE, KEY_HASH_TABLE};
use crate::error::{Error, Result};
use crate::header::{FormatVersion, HEADER_SIZE_V1, HEADER_SIZE_V2, HEADER_SIZE_V3, HEADER_SIZE_V4};
use crate::modify::{finish_v4_header, write_header};
use crate::tables::HASH_ENTRY_FREE;

/// [`create`] 的各项配置。
#[derive(Debug, Clone, Copy)]
pub struct CreateOptions {
    /// 要写入的头部版本（v1–v4）。
    pub version: FormatVersion,
    /// 经典哈希表的大小（以项数计）——即文件名的容量。
    /// 会在 `4..=1 Mi` 项的范围内向上取整为 2 的幂。
    pub hash_table_size: u32,
    /// 扇区大小指数（扇区 = `512 << shift`），与头部字段的含义相同。
    pub sector_shift: u16,
}

impl Default for CreateOptions {
    fn default() -> Self {
        CreateOptions {
            version: FormatVersion::V1,
            hash_table_size: 1024,
            sector_shift: 3, // 4096 字节扇区，几乎通用的选择
        }
    }
}

/// 在内存字节中构建一个新的空 MPQ 归档。结果可用 [`Archive::new`]
/// 打开，并可随后通过 [`crate::modify`] 导入文件。
pub fn create(options: &CreateOptions) -> Result<Vec<u8>> {
    if options.sector_shift > 30 {
        return Err(Error::InvalidData(format!(
            "无效的扇区指数 {}（应在 0–30）",
            options.sector_shift
        )));
    }
    let hash_count = normalize_hash_size(options.hash_table_size);
    let bytes = empty_archive(options.version, hash_count, options.sector_shift);

    // 自检：空容器必须能以所请求的版本打开，且不含任何条目。
    let check = Archive::new(Cursor::new(&bytes))?;
    if check.version() != options.version || !check.entries().is_empty() {
        return Err(Error::Corrupt("新建归档自检失败"));
    }
    Ok(bytes)
}

/// 将请求的哈希表大小收敛为合法的 2 的幂。
pub(crate) fn normalize_hash_size(requested: u32) -> u32 {
    requested.clamp(4, 1 << 20).next_power_of_two()
}

/// 某个版本在磁盘上的标准头部大小。
pub(crate) fn header_size_of(version: FormatVersion) -> u32 {
    match version {
        FormatVersion::V1 => HEADER_SIZE_V1,
        FormatVersion::V2 => HEADER_SIZE_V2,
        FormatVersion::V3 => HEADER_SIZE_V3,
        FormatVersion::V4 => HEADER_SIZE_V4,
    }
}

/// 组装空容器：头部 | 哈希表 | 块表。
/// `hash_count` 必须已完成归一化。
pub(crate) fn empty_archive(
    version: FormatVersion,
    hash_count: u32,
    sector_shift: u16,
) -> Vec<u8> {
    // 空闲的经典哈希项：name1/name2 = FFFFFFFF，locale/platform/
    // flags 为 0，块索引为 HASH_ENTRY_FREE。
    let mut free_entry = [0u8; 16];
    free_entry[0..4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    free_entry[4..8].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    free_entry[12..16].copy_from_slice(&HASH_ENTRY_FREE.to_le_bytes());

    let mut hash_buf = Vec::with_capacity(hash_count as usize * 16);
    for _ in 0..hash_count {
        hash_buf.extend_from_slice(&free_entry);
    }
    encrypt_bytes(&mut hash_buf, KEY_HASH_TABLE);

    // 一个空闲块项（所有字段为 0）：占位用的未使用块。
    let mut block_buf = vec![0u8; 16];
    encrypt_bytes(&mut block_buf, KEY_BLOCK_TABLE);

    let hash_pos = header_size_of(version);
    let block_pos = hash_pos + hash_buf.len() as u32;
    let total = block_pos as u64 + block_buf.len() as u64;
    let mut header = write_header(
        version,
        sector_shift,
        hash_pos,
        hash_count,
        block_pos,
        1,
        total,
    );
    if version == FormatVersion::V4 {
        finish_v4_header(&mut header, &hash_buf, &block_buf);
    }

    let mut out = Vec::with_capacity(total as usize);
    out.extend_from_slice(&header);
    out.extend_from_slice(&hash_buf);
    out.extend_from_slice(&block_buf);
    out
}
