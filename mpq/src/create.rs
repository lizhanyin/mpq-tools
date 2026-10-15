//! Create new, empty MPQ archives (header versions v1–v4).
//!
//! The result is a minimal, spec-conforming container: the version's header,
//! a fully-encrypted all-free classic hash table, and an encrypted block
//! table holding a single free (ghost) entry. A zero-entry block table would
//! be rejected by readers that require a non-empty file table (this library
//! included); a free block is the format's own "unused slot" marker, so the
//! archive opens everywhere and [`crate::modify`] can later import files
//! into it like into any other archive. v4 containers carry full MD5
//! checksums (tables + header) via [`crate::modify::finish_v4_header`].

use std::io::Cursor;

use crate::archive::Archive;
use crate::crypt::{encrypt_bytes, KEY_BLOCK_TABLE, KEY_HASH_TABLE};
use crate::error::{Error, Result};
use crate::header::{FormatVersion, HEADER_SIZE_V1, HEADER_SIZE_V2, HEADER_SIZE_V3, HEADER_SIZE_V4};
use crate::modify::{finish_v4_header, write_header};
use crate::tables::HASH_ENTRY_FREE;

/// Knobs for [`create`].
#[derive(Debug, Clone, Copy)]
pub struct CreateOptions {
    /// Header version to write (v1–v4).
    pub version: FormatVersion,
    /// Classic hash table size in entries — the capacity for file names.
    /// Rounded up to a power of two in `4..=1 Mi` entries.
    pub hash_table_size: u32,
    /// Sector size exponent (sector = `512 << shift`), like the header field.
    pub sector_shift: u16,
}

impl Default for CreateOptions {
    fn default() -> Self {
        CreateOptions {
            version: FormatVersion::V1,
            hash_table_size: 1024,
            sector_shift: 3, // 4096-byte sectors, the near-universal choice
        }
    }
}

/// Build a new, empty MPQ archive as in-memory bytes. The result opens with
/// [`Archive::new`] and is ready for imports via [`crate::modify`].
pub fn create(options: &CreateOptions) -> Result<Vec<u8>> {
    if options.sector_shift > 30 {
        return Err(Error::InvalidData(format!(
            "无效的扇区指数 {}（应在 0–30）",
            options.sector_shift
        )));
    }
    let hash_count = normalize_hash_size(options.hash_table_size);
    let bytes = empty_archive(options.version, hash_count, options.sector_shift);

    // Self-check: the empty container must open as the requested version
    // with no entries.
    let check = Archive::new(Cursor::new(&bytes))?;
    if check.version() != options.version || !check.entries().is_empty() {
        return Err(Error::Corrupt("新建归档自检失败"));
    }
    Ok(bytes)
}

/// Clamp a requested hash table size to a legal power of two.
pub(crate) fn normalize_hash_size(requested: u32) -> u32 {
    requested.clamp(4, 1 << 20).next_power_of_two()
}

/// The standard on-disk header size of a version.
pub(crate) fn header_size_of(version: FormatVersion) -> u32 {
    match version {
        FormatVersion::V1 => HEADER_SIZE_V1,
        FormatVersion::V2 => HEADER_SIZE_V2,
        FormatVersion::V3 => HEADER_SIZE_V3,
        FormatVersion::V4 => HEADER_SIZE_V4,
    }
}

/// Assemble the empty container: header | hash table | block table.
/// `hash_count` must already be normalized.
pub(crate) fn empty_archive(
    version: FormatVersion,
    hash_count: u32,
    sector_shift: u16,
) -> Vec<u8> {
    // Free classic hash entry: name1/name2 = FFFFFFFF, locale/platform/
    // flags 0, block index HASH_ENTRY_FREE.
    let mut free_entry = [0u8; 16];
    free_entry[0..4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    free_entry[4..8].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    free_entry[12..16].copy_from_slice(&HASH_ENTRY_FREE.to_le_bytes());

    let mut hash_buf = Vec::with_capacity(hash_count as usize * 16);
    for _ in 0..hash_count {
        hash_buf.extend_from_slice(&free_entry);
    }
    encrypt_bytes(&mut hash_buf, KEY_HASH_TABLE);

    // One free block entry (all fields 0): a placeholder unused block.
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
