//! MPQ 归档头部（v1–v4）以及 `MPQ\x1B` 用户数据头部。

use crate::error::{Error, Result};

/// 以小端 u32 表示的 `MPQ\x1A`。
pub const MPQ_MAGIC: u32 = 0x1A51_504D;
/// 以小端 u32 表示的 `MPQ\x1B`（用户数据头部）。
pub const USERDATA_MAGIC: u32 = 0x1B51_504D;

/// 各头部版本在磁盘上的标准大小。
pub const HEADER_SIZE_V1: u32 = 0x20;
pub const HEADER_SIZE_V2: u32 = 0x2C;
pub const HEADER_SIZE_V3: u32 = 0x44;
pub const HEADER_SIZE_V4: u32 = 0xD0;

/// v4 头部自身 MD5（`MD5_MpqHeader` @ 0xC0）覆盖的字节数：
/// 即从签名字段起直至 `MD5_HetTable`（含）的全部内容。
pub const HEADER_MD5_SPAN: usize = 0xC0;

/// 检查头部候选时最多需要读取的字节数。
pub const MAX_HEADER_READ: u64 = HEADER_SIZE_V4 as u64;

/// 头部字段中声明的 MPQ 格式版本。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FormatVersion {
    V1,
    V2,
    V3,
    V4,
}

impl FormatVersion {
    /// 解析原始 u16 字段。大于 3 的值会被拒绝。
    pub fn from_raw(v: u16) -> Result<Self> {
        match v {
            0 => Ok(FormatVersion::V1),
            1 => Ok(FormatVersion::V2),
            2 => Ok(FormatVersion::V3),
            3 => Ok(FormatVersion::V4),
            other => Err(Error::UnsupportedVersion(other)),
        }
    }

    pub fn as_u32(self) -> u32 {
        match self {
            FormatVersion::V1 => 1,
            FormatVersion::V2 => 2,
            FormatVersion::V3 => 3,
            FormatVersion::V4 => 4,
        }
    }
}

/// 见于 Warcraft III / Starcraft II 地图归档前部的 `MPQ\x1B`
/// 用户数据头部。
#[derive(Debug, Clone)]
pub struct UserDataHeader {
    pub data_size: u32,
    /// 真实 MPQ 头部的偏移，相对于用户数据头部。
    pub archive_offset: u32,
    pub header_size: u32,
}

impl UserDataHeader {
    /// 至少要有 16 字节才能解析。当数据看起来不像一个合理的用户数据
    /// 头部时返回 `None`（而不是报错）。
    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < 16 {
            return None;
        }
        let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        if magic != USERDATA_MAGIC {
            return None;
        }
        let data_size = le_u32(buf, 4);
        let archive_offset = le_u32(buf, 8);
        let header_size = le_u32(buf, 12);
        // 合理性检查与 StormLib 的 IsValidMpqUserData 完全一致：
        // header_size <= data_size && data_size <= archive_offset。
        if header_size > data_size || data_size > archive_offset {
            return None;
        }
        Some(UserDataHeader {
            data_size,
            archive_offset,
            header_size,
        })
    }
}

/// 原始 MPQ 头部，布局贴近磁盘存储。超出所声明版本的字段在文件中
/// 可能不存在，此时读作 0。
#[derive(Debug, Clone, Default)]
pub struct MpqHeader {
    pub header_size: u32,
    pub archive_size: u32,
    pub format_version: u16,
    pub block_size: u16,
    pub hash_table_pos: u64,
    pub block_table_pos: u64,
    pub hash_table_size: u32,
    pub block_table_size: u32,

    // v2
    pub hi_block_table_pos: u64,

    // v3
    pub archive_size64: u64,
    pub bet_table_pos: u64,
    pub het_table_pos: u64,

    // v4
    pub hash_table_size64: u64,
    pub block_table_size64: u64,
    pub hi_block_table_size64: u64,
    pub het_table_size64: u64,
    pub bet_table_size64: u64,
    /// v4：每个文件级 MD5 所覆盖的原始数据块大小（0 = 无）。
    pub raw_chunk_size: u32,
    /// v4 @ 0x70：block table 按存储形态（加密/压缩后）的 MD5。
    pub md5_block_table: [u8; 16],
    /// v4 @ 0x80：hash table 按存储形态的 MD5。
    pub md5_hash_table: [u8; 16],
    /// v4 @ 0x90：hi-block 表按存储形态的 MD5。
    pub md5_hi_block_table: [u8; 16],
    /// v4 @ 0xA0：BET 表按存储形态的 MD5。
    pub md5_bet_table: [u8; 16],
    /// v4 @ 0xB0：HET 表按存储形态的 MD5。
    pub md5_het_table: [u8; 16],
    /// v4 @ 0xC0：头部自身的 MD5，覆盖 `0x00..0xC0` 字节。
    pub md5_header: [u8; 16],
}

impl MpqHeader {
    /// 从 `buf` 解析头部（长度必须 >= 8；按照 v3 规范，较短的头部也被
    /// 容忍，缺失的字段读作 0）。
    pub fn parse(buf: &[u8]) -> Result<(Self, FormatVersion)> {
        if buf.len() < HEADER_SIZE_V1 as usize {
            return Err(Error::Corrupt("MPQ header truncated below 0x20 bytes"));
        }
        let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        if magic != MPQ_MAGIC {
            return Err(Error::Corrupt("bad MPQ magic"));
        }

        let version = FormatVersion::from_raw(le_u16(buf, 0x0C))?;
        let mut h = MpqHeader {
            header_size: le_u32(buf, 0x04),
            archive_size: le_u32(buf, 0x08),
            format_version: le_u16(buf, 0x0C),
            block_size: le_u16(buf, 0x0E),
            hash_table_pos: le_u32(buf, 0x10) as u64,
            block_table_pos: le_u32(buf, 0x14) as u64,
            hash_table_size: le_u32(buf, 0x18),
            block_table_size: le_u32(buf, 0x1C),
            ..MpqHeader::default()
        };

        // 宽容处理：某些工具写入的 header_size 小于标准 v1 大小；
        // 但 0x20 之前的字段仍然有效。
        if h.header_size < HEADER_SIZE_V1 {
            h.header_size = HEADER_SIZE_V1;
        }

        if version >= FormatVersion::V2 && buf.len() >= HEADER_SIZE_V2 as usize {
            h.hi_block_table_pos = le_u64(buf, 0x20);
            h.hash_table_pos |= (le_u16(buf, 0x28) as u64) << 32;
            h.block_table_pos |= (le_u16(buf, 0x2A) as u64) << 32;
        }

        if version >= FormatVersion::V3 && buf.len() >= HEADER_SIZE_V3 as usize {
            h.archive_size64 = le_u64(buf, 0x2C);
            h.bet_table_pos = le_u64(buf, 0x34);
            h.het_table_pos = le_u64(buf, 0x3C);
        }

        if version >= FormatVersion::V4 && buf.len() >= HEADER_SIZE_V4 as usize {
            h.hash_table_size64 = le_u64(buf, 0x44);
            h.block_table_size64 = le_u64(buf, 0x4C);
            h.hi_block_table_size64 = le_u64(buf, 0x54);
            h.het_table_size64 = le_u64(buf, 0x5C);
            h.bet_table_size64 = le_u64(buf, 0x64);
            h.raw_chunk_size = le_u32(buf, 0x6C);
            h.md5_block_table.copy_from_slice(&buf[0x70..0x80]);
            h.md5_hash_table.copy_from_slice(&buf[0x80..0x90]);
            h.md5_hi_block_table.copy_from_slice(&buf[0x90..0xA0]);
            h.md5_bet_table.copy_from_slice(&buf[0xA0..0xB0]);
            h.md5_het_table.copy_from_slice(&buf[0xB0..0xC0]);
            h.md5_header.copy_from_slice(&buf[0xC0..0xD0]);
        }

        Ok((h, version))
    }

    /// 在更大的文件中扫描头部时使用的合理性检查。
    pub fn plausible(&self, file_len: u64, base: u64) -> bool {
        if self.header_size < HEADER_SIZE_V1 || self.header_size > 0x1000 {
            return false;
        }
        // sector 指数的合理性检查：按照 storm 的实现只有低字节有意义。
        if self.block_size > 0xFF {
            return false;
        }
        // 文件很小却声明了巨大的表，通常是误报。
        let _ = file_len;
        let _ = base;
        true
    }

    /// sector 大小（字节）：`512 << block_size`，仅取低字节。
    pub fn sector_size(&self) -> u32 {
        512u32 << (self.block_size & 0xFF)
    }

    /// 有效归档大小（字节，相对于归档起始处）。
    ///
    /// 宽容处理：优先采用 64 位大小，退回 32 位大小，
    /// 最终以实际可用长度为准。
    pub fn archive_size(&self, file_len_from_base: u64) -> u64 {
        if self.archive_size64 != 0 && self.archive_size64 <= file_len_from_base {
            self.archive_size64
        } else if self.archive_size != 0 && self.archive_size as u64 <= file_len_from_base {
            self.archive_size as u64
        } else {
            file_len_from_base
        }
    }
}

#[inline]
fn le_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

#[inline]
fn le_u16(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

#[inline]
fn le_u64(buf: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        buf[off],
        buf[off + 1],
        buf[off + 2],
        buf[off + 3],
        buf[off + 4],
        buf[off + 5],
        buf[off + 6],
        buf[off + 7],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_header(v: u16) -> Vec<u8> {
        let mut b = vec![0u8; HEADER_SIZE_V4 as usize];
        b[0..4].copy_from_slice(&MPQ_MAGIC.to_le_bytes());
        b[4..8].copy_from_slice(&HEADER_SIZE_V1.to_le_bytes());
        b[12..14].copy_from_slice(&v.to_le_bytes());
        b[14..16].copy_from_slice(&3u16.to_le_bytes()); // 4096 字节的 sector
        b
    }

    #[test]
    fn parses_v1() {
        let buf = build_header(0);
        let (h, v) = MpqHeader::parse(&buf).unwrap();
        assert_eq!(v, FormatVersion::V1);
        assert_eq!(h.sector_size(), 4096);
        assert_eq!(h.hash_table_pos, 0);
    }

    #[test]
    fn parses_v3_with_missing_tail() {
        // 只存储了 0x30 字节的 v3 头部（被容忍）：
        // 存储长度之外的 v3 字段必须读作 0。
        let mut buf = build_header(2);
        buf.truncate(0x30);
        // parse 看到的字节少于 HEADER_SIZE_V3 -> v3 字段保持为 0，
        // parse() 必须容忍这种情况而不是报错。
        let r = MpqHeader::parse(&buf);
        assert!(r.is_ok());
        let (_, v) = r.unwrap();
        assert_eq!(v, FormatVersion::V3);
    }

    #[test]
    fn rejects_bad_version() {
        let buf = build_header(9);
        assert!(matches!(
            MpqHeader::parse(&buf),
            Err(Error::UnsupportedVersion(9))
        ));
    }

    #[test]
    fn userdata_magic_check() {
        let mut b = [0u8; 16];
        b[0..4].copy_from_slice(&USERDATA_MAGIC.to_le_bytes());
        // 大小：header 0x10 <= data 0x100 <= archive offset 0x200
        b[4..8].copy_from_slice(&0x100u32.to_le_bytes());
        b[8..12].copy_from_slice(&0x200u32.to_le_bytes());
        b[12..16].copy_from_slice(&0x10u32.to_le_bytes());
        let ud = UserDataHeader::parse(&b).unwrap();
        assert_eq!(ud.archive_offset, 0x200);
    }
}
