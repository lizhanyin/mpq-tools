//! MPQ archive header (v1–v4) and the `MPQ\x1B` user-data header.

use crate::error::{Error, Result};

/// `MPQ\x1A` as a little-endian u32.
pub const MPQ_MAGIC: u32 = 0x1A51_504D;
/// `MPQ\x1B` (user data header) as a little-endian u32.
pub const USERDATA_MAGIC: u32 = 0x1B51_504D;

/// Standard on-disk size of each header version.
pub const HEADER_SIZE_V1: u32 = 0x20;
pub const HEADER_SIZE_V2: u32 = 0x2C;
pub const HEADER_SIZE_V3: u32 = 0x44;
pub const HEADER_SIZE_V4: u32 = 0xD0;

/// Bytes covered by the v4 header's own MD5 (`MD5_MpqHeader` @ 0xC0):
/// everything from the signature up to and including `MD5_HetTable`.
pub const HEADER_MD5_SPAN: usize = 0xC0;

/// Maximum bytes we ever need to look at a header candidate.
pub const MAX_HEADER_READ: u64 = HEADER_SIZE_V4 as u64;

/// MPQ format version as declared in the header field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FormatVersion {
    V1,
    V2,
    V3,
    V4,
}

impl FormatVersion {
    /// Parse the raw u16 field. Values above 3 are rejected.
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

/// The `MPQ\x1B` user data header found in front of Warcraft III /
/// Starcraft II map archives.
#[derive(Debug, Clone)]
pub struct UserDataHeader {
    pub data_size: u32,
    /// Offset of the real MPQ header, relative to the user data header.
    pub archive_offset: u32,
    pub header_size: u32,
}

impl UserDataHeader {
    /// Parse from at least 16 bytes. Returns `None` (not an error) when the
    /// data does not look like a sane user data header.
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
        // Sanity exactly like StormLib's IsValidMpqUserData:
        // header_size <= data_size && data_size <= archive_offset.
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

/// Raw MPQ header, kept close to the on-disk layout. Fields beyond the
/// declared version may be absent in the file and then read as 0.
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
    /// v4: size of raw data chunks covered by each per-file MD5 (0 = none).
    pub raw_chunk_size: u32,
    /// v4 @ 0x70: MD5 of the block table as stored (encrypted/compressed).
    pub md5_block_table: [u8; 16],
    /// v4 @ 0x80: MD5 of the hash table as stored.
    pub md5_hash_table: [u8; 16],
    /// v4 @ 0x90: MD5 of the hi-block table as stored.
    pub md5_hi_block_table: [u8; 16],
    /// v4 @ 0xA0: MD5 of the BET table as stored.
    pub md5_bet_table: [u8; 16],
    /// v4 @ 0xB0: MD5 of the HET table as stored.
    pub md5_het_table: [u8; 16],
    /// v4 @ 0xC0: MD5 of the header itself, covering bytes `0x00..0xC0`.
    pub md5_header: [u8; 16],
}

impl MpqHeader {
    /// Parse a header from `buf` (length must be >= 8; shorter headers are
    /// tolerated per the v3 spec, missing fields become 0).
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

        // Tolerance: some tools write header_size smaller than the standard
        // v1 size; the fields up to 0x20 are still valid.
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

    /// Sanity check used while scanning for the header inside a larger file.
    pub fn plausible(&self, file_len: u64, base: u64) -> bool {
        if self.header_size < HEADER_SIZE_V1 || self.header_size > 0x1000 {
            return false;
        }
        // Sector exponent sanity: only the low byte is meaningful per storm.
        if self.block_size > 0xFF {
            return false;
        }
        // Huge declared tables with a tiny file are usually false positives.
        let _ = file_len;
        let _ = base;
        true
    }

    /// Sector size in bytes: `512 << block_size`, low byte only.
    pub fn sector_size(&self) -> u32 {
        512u32 << (self.block_size & 0xFF)
    }

    /// Effective archive size in bytes (relative to archive start).
    ///
    /// Tolerant: prefers the 64-bit size, falls back to the 32-bit size,
    /// and finally yields the actual available length.
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
        b[14..16].copy_from_slice(&3u16.to_le_bytes()); // 4096-byte sectors
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
        // v3 header where only 0x30 bytes were stored (tolerated):
        // v3 fields beyond the stored length must read as 0.
        let mut buf = build_header(2);
        buf.truncate(0x30);
        // parse sees fewer than HEADER_SIZE_V3 bytes -> v3 fields stay 0,
        // which parse() must tolerate rather than fail.
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
        // sizes: header 0x10 <= data 0x100 <= archive offset 0x200
        b[4..8].copy_from_slice(&0x100u32.to_le_bytes());
        b[8..12].copy_from_slice(&0x200u32.to_le_bytes());
        b[12..16].copy_from_slice(&0x10u32.to_le_bytes());
        let ud = UserDataHeader::parse(&b).unwrap();
        assert_eq!(ud.archive_offset, 0x200);
    }
}
