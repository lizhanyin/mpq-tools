//! `(attributes)` — optional per-file metadata stored inside the archive.

use crate::error::{Error, Result};

pub const ATTR_CRC32: u32 = 0x1;
pub const ATTR_FILETIME: u32 = 0x2;
pub const ATTR_MD5: u32 = 0x4;
pub const ATTR_PATCH_BIT: u32 = 0x8;

/// Per-file metadata decoded from `(attributes)`.
#[derive(Debug, Clone, Default)]
pub struct FileAttributes {
    pub crc32: Option<u32>,
    pub file_time: Option<u64>,
    pub md5: Option<[u8; 16]>,
}

/// Parsed `(attributes)` content.
#[derive(Debug, Clone, Default)]
pub struct Attributes {
    pub version: u32,
    pub file_count: usize,
    pub crc32: Vec<u32>,
    pub file_times: Vec<u64>,
    pub md5s: Vec<[u8; 16]>,
    pub patch_bits: Vec<u8>,
}

impl Attributes {
    /// Parse the raw (already decompressed) `(attributes)` data for
    /// `file_count` files. Tolerant: unknown versions or truncated arrays
    /// produce an error the caller may downgrade to a warning.
    pub fn parse(data: &[u8], file_count: usize) -> Result<Self> {
        if data.len() < 8 {
            return Err(Error::Corrupt("(attributes) too small"));
        }
        let version = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let flags = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        if version != 100 {
            return Err(Error::Corrupt("(attributes) version is not 100"));
        }

        let mut pos = 8usize;
        let need = |pos: usize, n: usize| -> Result<()> {
            if data.len() < pos + n {
                Err(Error::Corrupt("(attributes) arrays truncated"))
            } else {
                Ok(())
            }
        };

        let mut attrs = Attributes {
            version,
            file_count,
            ..Attributes::default()
        };

        if flags & ATTR_CRC32 != 0 {
            need(pos, file_count * 4)?;
            for i in 0..file_count {
                let o = pos + i * 4;
                attrs
                    .crc32
                    .push(u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]));
            }
            pos += file_count * 4;
        }
        if flags & ATTR_FILETIME != 0 {
            need(pos, file_count * 8)?;
            for i in 0..file_count {
                let o = pos + i * 8;
                let mut v = [0u8; 8];
                v.copy_from_slice(&data[o..o + 8]);
                attrs.file_times.push(u64::from_le_bytes(v));
            }
            pos += file_count * 8;
        }
        if flags & ATTR_MD5 != 0 {
            need(pos, file_count * 16)?;
            for i in 0..file_count {
                let o = pos + i * 16;
                let mut m = [0u8; 16];
                m.copy_from_slice(&data[o..o + 16]);
                attrs.md5s.push(m);
            }
            pos += file_count * 16;
        }
        if flags & ATTR_PATCH_BIT != 0 {
            let bytes = file_count.div_ceil(8);
            need(pos, bytes)?;
            attrs.patch_bits.extend_from_slice(&data[pos..pos + bytes]);
        }
        Ok(attrs)
    }

    /// Attributes for file index `i`, if the arrays cover it.
    pub fn file(&self, index: usize) -> FileAttributes {
        FileAttributes {
            crc32: self.crc32.get(index).copied(),
            file_time: self.file_times.get(index).copied(),
            md5: self.md5s.get(index).copied(),
        }
    }
}
