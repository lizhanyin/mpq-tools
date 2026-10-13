//! Classic hash table, block table and hi-block table.

use crate::compression;
use crate::crypt;
use crate::error::{Error, Result};

/// Sentinel: the hash slot has never been used; terminates searches.
pub const HASH_ENTRY_FREE: u32 = 0xFFFF_FFFF;
/// Sentinel: the hash slot pointed at a file that was deleted.
pub const HASH_ENTRY_DELETED: u32 = 0xFFFF_FFFE;

/// File flag: file exists.
pub const FILE_EXISTS: u32 = 0x8000_0000;
/// File flag: stored as one unit instead of sectors.
pub const FILE_SINGLE_UNIT: u32 = 0x0100_0000;
/// File flag: deletion marker (MPQ patches).
pub const FILE_DELETE_MARKER: u32 = 0x0200_0000;
/// File flag: sector offset table is followed by per-sector checksums.
pub const FILE_SECTOR_CRC: u32 = 0x0400_0000;
/// File flag: observed on STANDARD.SNP `(signature)`.
pub const FILE_SIGNATURE: u32 = 0x1000_0000;
/// File flag: incremental patch file.
pub const FILE_PATCH: u32 = 0x0010_0000;
/// File flag: data is encrypted.
pub const FILE_ENCRYPTED: u32 = 0x0001_0000;
/// File flag: "fixed key" — key derived as `(name_key + offset) ^ size`.
pub const FILE_KEY_V2: u32 = 0x0002_0000;
/// File flag: compressed via the multi-method scheme (method byte).
pub const FILE_COMPRESS: u32 = 0x0000_0200;
/// File flag: compressed with PKWARE implode (no method byte).
pub const FILE_IMPLODE: u32 = 0x0000_0100;
/// Mask of both compression flags.
pub const FILE_COMPRESS_MASK: u32 = 0x0000_FF00;

/// One classic hash table entry (decrypted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HashEntry {
    pub name1: u32,
    pub name2: u32,
    pub locale: u16,
    pub platform: u8,
    pub flags: u8,
    pub block_index: u32,
}

impl HashEntry {
    #[inline]
    pub fn is_free(&self) -> bool {
        self.block_index >= HASH_ENTRY_DELETED
    }
}

/// One classic block table entry (decrypted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockEntry {
    /// Offset of the file block, relative to the archive start.
    pub pos: u32,
    /// Compressed (archived) size.
    pub csize: u32,
    /// Uncompressed size.
    pub fsize: u32,
    pub flags: u32,
}

/// The three classic tables, fully decoded.
#[derive(Debug, Clone, Default)]
pub struct ClassicTables {
    pub hash_table: Vec<HashEntry>,
    pub block_table: Vec<BlockEntry>,
    /// High 16 bits of each block offset (may be empty).
    pub hi_block: Vec<u16>,
}

impl ClassicTables {
    /// Look up a file by name (locale-agnostic, first match wins).
    ///
    /// Returns the hash entry and the corresponding block entry.
    pub fn find(&self, name: &str) -> Option<(&HashEntry, &BlockEntry)> {
        if self.hash_table.is_empty() {
            return None;
        }
        let normalized = crypt::normalize_name(name);
        let start = crypt::hash_bytes(&normalized, crypt::HASH_TABLE_INDEX);
        let name1 = crypt::hash_bytes(&normalized, crypt::HASH_NAME_A);
        let name2 = crypt::hash_bytes(&normalized, crypt::HASH_NAME_B);

        // The hash table size is supposed to be a power of two; use modulo
        // so that odd-but-parseable archives still open.
        let size = self.hash_table.len() as u32;
        let mut index = start % size;
        let start_index = index;

        loop {
            let entry = &self.hash_table[index as usize];
            if entry.name1 == name1
                && entry.name2 == name2
                && !entry.is_free()
                && (entry.block_index as usize) < self.block_table.len()
            {
                return Some((entry, &self.block_table[entry.block_index as usize]));
            }
            if entry.block_index == HASH_ENTRY_FREE {
                return None;
            }
            index = (index + 1) % size;
            if index == start_index {
                return None;
            }
        }
    }

    /// Merge a hi-block word into a block offset (48-bit result).
    pub fn block_offset(&self, block_index: usize) -> u64 {
        let block = &self.block_table[block_index];
        let hi = self.hi_block.get(block_index).copied().unwrap_or(0) as u64;
        (hi << 32) | block.pos as u64
    }
}

/// Read, decrypt and (if needed) decompress a classic table.
///
/// * `stored_size` – bytes the table occupies in the file; when it is
///   smaller than `table_size` the table is compressed (v2+ behaviour).
/// * `table_size` – uncompressed size (`entries * 16`).
///
/// Tolerances applied, mirroring StormLib's `LoadMpqTable`:
/// reads that run past EOF are zero-filled.
pub fn load_mpq_table<R: std::io::Read + std::io::Seek>(
    reader: &mut R,
    offset: u64,
    file_len: u64,
    stored_size: Option<u64>,
    table_size: usize,
    key: u32,
) -> Result<Vec<u8>> {
    if table_size == 0 {
        return Ok(Vec::new());
    }
    let stored = stored_size.unwrap_or(table_size as u64).min(table_size as u64) as usize;

    if offset >= file_len {
        return Err(Error::Corrupt("table offset beyond end of file"));
    }

    let available = (file_len - offset).min(stored as u64) as usize;
    let mut raw = vec![0u8; stored];
    reader
        .seek(std::io::SeekFrom::Start(offset))
        .and_then(|_| reader.read_exact(&mut raw[..available]))?;

    if key != 0 {
        crypt::decrypt_bytes(&mut raw, key);
    }

    if stored < table_size {
        let table = compression::decompress_masked(&raw, table_size)?;
        if table.len() < table_size {
            return Err(Error::Corrupt("decompressed table too small"));
        }
        Ok(table)
    } else {
        Ok(raw)
    }
}

/// Parse the decrypted hash table buffer into entries.
pub fn parse_hash_table(buf: &[u8]) -> Result<Vec<HashEntry>> {
    let mut entries = Vec::with_capacity(buf.len() / 16);
    for chunk in buf.chunks_exact(16) {
        entries.push(HashEntry {
            name1: u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]),
            name2: u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
            locale: u16::from_le_bytes([chunk[8], chunk[9]]),
            platform: chunk[10],
            flags: chunk[11],
            block_index: u32::from_le_bytes([chunk[12], chunk[13], chunk[14], chunk[15]]),
        });
    }
    Ok(entries)
}

/// Parse the decrypted block table buffer into entries.
pub fn parse_block_table(buf: &[u8]) -> Result<Vec<BlockEntry>> {
    let mut entries = Vec::with_capacity(buf.len() / 16);
    for chunk in buf.chunks_exact(16) {
        entries.push(BlockEntry {
            pos: u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]),
            csize: u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
            fsize: u32::from_le_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]),
            flags: u32::from_le_bytes([chunk[12], chunk[13], chunk[14], chunk[15]]),
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sixteen_byte_entries() {
        let mut buf = vec![0u8; 32];
        buf[12..16].copy_from_slice(&HASH_ENTRY_FREE.to_le_bytes());
        let ht = parse_hash_table(&buf).unwrap();
        assert_eq!(ht.len(), 2);
        assert!(ht[0].is_free());

        let bt = parse_block_table(&buf).unwrap();
        assert_eq!(bt.len(), 2);
        assert_eq!(bt[0].flags, HASH_ENTRY_FREE);
    }

    #[test]
    fn hi_block_merges_into_48_bit_offsets() {
        // >4 GB archives store the high 16 bits of each file offset in the
        // hi-block table; entries without a hi word stay 32-bit.
        let tables = ClassicTables {
            block_table: vec![
                BlockEntry { pos: 0x0000_1234, ..Default::default() },
                BlockEntry { pos: 0xABCD_0001, ..Default::default() },
            ],
            hi_block: vec![0x0001, 0x0005],
            ..Default::default()
        };
        // (0x1 << 32) | 0x1234
        assert_eq!(tables.block_offset(0), 0x1_0000_1234);
        // (0x5 << 32) | 0xABCD0001 — a genuine >4 GB file offset
        assert_eq!(tables.block_offset(1), 0x0005_ABCD_0001);

        // Missing hi words (table absent / shorter) fall back to plain 32-bit.
        let no_hi = ClassicTables {
            block_table: vec![BlockEntry { pos: 0x1000, ..Default::default() }],
            ..Default::default()
        };
        assert_eq!(no_hi.block_offset(0), 0x1000);
    }
}
