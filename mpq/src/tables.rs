//! 经典 hash table、block table 与 hi-block table。

use crate::compression;
use crate::crypt;
use crate::error::{Error, Result};

/// 哨兵值：该 hash 槽位从未被使用；查找会在此终止。
pub const HASH_ENTRY_FREE: u32 = 0xFFFF_FFFF;
/// 哨兵值：该 hash 槽位指向的文件已被删除。
pub const HASH_ENTRY_DELETED: u32 = 0xFFFF_FFFE;

/// 文件标志：文件存在。
pub const FILE_EXISTS: u32 = 0x8000_0000;
/// 文件标志：作为单一单元存储，而不是按 sector 划分。
pub const FILE_SINGLE_UNIT: u32 = 0x0100_0000;
/// 文件标志：删除标记（MPQ 补丁）。
pub const FILE_DELETE_MARKER: u32 = 0x0200_0000;
/// 文件标志：sector 偏移表之后跟着每个 sector 的校验和。
pub const FILE_SECTOR_CRC: u32 = 0x0400_0000;
/// 文件标志：在 STANDARD.SNP 的 `(signature)` 上观察到。
pub const FILE_SIGNATURE: u32 = 0x1000_0000;
/// 文件标志：增量补丁文件。
pub const FILE_PATCH: u32 = 0x0010_0000;
/// 文件标志：数据已加密。
pub const FILE_ENCRYPTED: u32 = 0x0001_0000;
/// 文件标志："fixed key" —— 密钥按 `(name_key + offset) ^ size` 派生。
pub const FILE_KEY_V2: u32 = 0x0002_0000;
/// 文件标志：通过多方法方案压缩（带方法字节）。
pub const FILE_COMPRESS: u32 = 0x0000_0200;
/// 文件标志：使用 PKWARE implode 压缩（不带方法字节）。
pub const FILE_IMPLODE: u32 = 0x0000_0100;
/// 两种压缩标志的掩码。
pub const FILE_COMPRESS_MASK: u32 = 0x0000_FF00;
/// 自定义扩展（标准格式未使用）：文件的 sector 使用密码 AEAD 方案
/// 封装（ChaCha20-Poly1305，其主密钥包裹在 `(password)` vault 中），
/// 而不是 storm 加密。始终与 [`FILE_ENCRYPTED`] 同时设置。注意
/// BET 表只编码了标志位的一个子集，因此 `(password)` 文件的存在
/// 才是受保护归档的权威标志。
pub const FILE_PASSWORD_AEAD: u32 = 0x0800_0000;

/// 一个经典的 hash table 表项（已解密）。
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

/// 一个经典的 block table 表项（已解密）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockEntry {
    /// 文件块的偏移，相对于归档起始处。
    pub pos: u32,
    /// 压缩（存档）后的大小。
    pub csize: u32,
    /// 未压缩大小。
    pub fsize: u32,
    pub flags: u32,
}

/// 三张经典表，均已完整解码。
#[derive(Debug, Clone, Default)]
pub struct ClassicTables {
    pub hash_table: Vec<HashEntry>,
    pub block_table: Vec<BlockEntry>,
    /// 每个块偏移的高 16 位（可能为空）。
    pub hi_block: Vec<u16>,
}

impl ClassicTables {
    /// 按名称查找文件（不区分 locale，首个匹配生效）。
    ///
    /// 返回对应的 hash 表项和 block 表项。
    pub fn find(&self, name: &str) -> Option<(&HashEntry, &BlockEntry)> {
        if self.hash_table.is_empty() {
            return None;
        }
        let normalized = crypt::normalize_name(name);
        let start = crypt::hash_bytes(&normalized, crypt::HASH_TABLE_INDEX);
        let name1 = crypt::hash_bytes(&normalized, crypt::HASH_NAME_A);
        let name2 = crypt::hash_bytes(&normalized, crypt::HASH_NAME_B);

        // hash table 的大小本应是 2 的幂；这里使用取模，
        // 让格式古怪但仍可解析的归档也能打开。
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

    /// 将 hi-block 的 16 位字合并进块偏移（得到 48 位结果）。
    pub fn block_offset(&self, block_index: usize) -> u64 {
        let block = &self.block_table[block_index];
        let hi = self.hi_block.get(block_index).copied().unwrap_or(0) as u64;
        (hi << 32) | block.pos as u64
    }
}

/// 读取、解密并（在需要时）解压一张经典表。
///
/// * `stored_size` – 表在文件中占用的字节数；当它小于
///   `table_size` 时表示表已被压缩（v2+ 的行为）。
/// * `table_size` – 未压缩大小（`entries * 16`）。
///
/// 应用了与 StormLib `LoadMpqTable` 一致的容错处理：
/// 越过 EOF 的读取以零填充。
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

/// 将已解密的 hash table 缓冲区解析为表项。
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

/// 将已解密的 block table 缓冲区解析为表项。
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
        // 超过 4 GB 的归档把每个文件偏移的高 16 位存放在
        // hi-block 表中；没有 hi 字的表项仍保持 32 位。
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
        // (0x5 << 32) | 0xABCD0001 —— 真实存在的超过 4 GB 的文件偏移
        assert_eq!(tables.block_offset(1), 0x0005_ABCD_0001);

        // 缺失 hi 字（表不存在或更短）时回退为普通 32 位。
        let no_hi = ClassicTables {
            block_table: vec![BlockEntry { pos: 0x1000, ..Default::default() }],
            ..Default::default()
        };
        assert_eq!(no_hi.block_offset(0), 0x1000);
    }
}
