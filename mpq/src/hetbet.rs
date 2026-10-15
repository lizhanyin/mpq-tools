//! HET 和 BET 表 —— v3+ 的扩展 hash/block 表。
//!
//! 布局与查找语义遵循 StormLib 的 `SBaseFileTable.cpp`。
//! 两张表都以 12 字节的扩展头部（`HET\x1A` /
//! `BET\x1A`）开头，使用经典的 hash-/block-table 密钥加密，
//! 并可能以压缩形式存储。

use crate::compression;
use crate::crypt;
use crate::error::{Error, Result};
use crate::tables::{BlockEntry, FILE_EXISTS};

pub const HET_SIGNATURE: u32 = 0x1A54_4548; // 'HET\x1A'
pub const BET_SIGNATURE: u32 = 0x1A54_4542; // 'BET\x1A'

/// 加载扩展表（HET/BET）：校验签名、解密，必要时解压。
/// `size` 是 MPQ 头部给出的磁盘字节大小。
pub fn load_ext_table<R: std::io::Read + std::io::Seek>(
    reader: &mut R,
    offset: u64,
    size: u64,
    signature: u32,
    key: u32,
) -> Result<Vec<u8>> {
    if offset == 0 || size == 0 {
        return Err(Error::Corrupt("ext table has zero offset/size"));
    }
    let mut raw = vec![0u8; size as usize];
    reader.seek(std::io::SeekFrom::Start(offset))?;
    read_exact_or_eof(reader, &mut raw)?;

    if raw.len() < 12 {
        return Err(Error::Corrupt("ext table too small"));
    }
    let sig = le_u32(&raw, 0);
    let version = le_u32(&raw, 4);
    let data_size = le_u32(&raw, 8) as usize;

    if sig != signature {
        return Err(Error::Corrupt("ext table signature mismatch"));
    }
    if version != 1 {
        return Err(Error::Corrupt("ext table version is not 1"));
    }

    // 当由头部位置推算出的磁盘大小超出实际时（表按意外顺序布局、
    // 尾部有多余数据等），以声明的 data_size 为准。
    if data_size + 12 < raw.len() {
        raw.truncate(data_size + 12);
    }

    // 解密 12 字节头部之后的所有内容。
    crypt::decrypt_bytes(&mut raw[12..], key);

    // data_size 大于磁盘大小意味着负载已被压缩。
    if data_size + 12 > raw.len() {
        let decompressed = compression::decompress_masked(&raw[12..], data_size)?;
        let mut table = Vec::with_capacity(12 + data_size);
        table.extend_from_slice(&raw[..12]);
        table.extend_from_slice(&decompressed);
        if table.len() < 12 + data_size {
            return Err(Error::Corrupt("ext table decompressed short"));
        }
        Ok(table)
    } else {
        Ok(raw)
    }
}

fn read_exact_or_eof<R: std::io::Read>(reader: &mut R, buf: &mut [u8]) -> Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = reader.read(&mut buf[filled..])?;
        if n == 0 {
            // 表被截断：缺失的尾部保留为 0。
            return Ok(());
        }
        filled += n;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// HET 表

/// 已解析的 HET 表。
#[derive(Debug, Clone)]
pub struct HetTable {
    /// 每个槽位一个探测字节；0 = 空闲槽位。
    pub name_hash1: Vec<u8>,
    /// 按位打包的文件索引，每个槽位 `index_size_total` 位。
    pub bet_indexes: Vec<u8>,
    pub entry_count: u32,
    pub total_count: u32,
    pub name_hash_bit_size: u32,
    pub index_size_total: u32,
    pub index_size: u32,
}

impl HetTable {
    pub fn parse(table: &[u8]) -> Result<Self> {
        if table.len() < 12 + 32 {
            return Err(Error::Corrupt("HET table too small"));
        }
        let table_size = le_u32(table, 12) as usize;
        let entry_count = le_u32(table, 16);
        let total_count = le_u32(table, 20);
        let name_hash_bit_size = le_u32(table, 24);
        let index_size_total = le_u32(table, 28);
        let index_size_extra = le_u32(table, 32);
        let index_size = le_u32(table, 36);
        let index_table_size = le_u32(table, 40) as usize;

        if total_count == 0 || name_hash_bit_size == 0 || name_hash_bit_size > 64 {
            return Err(Error::Corrupt("HET header fields out of range"));
        }
        // 有效大小必须能放进总大小之内。
        if index_size > index_size_total || index_size_total == 0 || index_size > 32 {
            return Err(Error::Corrupt("HET index sizes out of range"));
        }
        let _ = index_size_extra;

        let data = &table[44..];
        let hashes_len = total_count as usize;
        let indexes_computed = (total_count as usize * index_size_total as usize).div_ceil(8);
        // 声明的索引表大小应等于计算值；
        // 头部给的值合适就采用，否则退回计算值。
        let indexes_len = if index_table_size > 0
            && data.len() >= hashes_len + index_table_size
            && index_table_size.abs_diff(indexes_computed) <= 8
        {
            index_table_size
        } else {
            indexes_computed
        };
        if data.len() < hashes_len || data.len() < hashes_len + indexes_len {
            return Err(Error::Corrupt("HET table body truncated"));
        }
        // 宽容处理：StormLib 要求 header+body 与 table_size 严格相等；
        // 我们只要求主体足够大。
        let _ = table_size;

        Ok(HetTable {
            name_hash1: data[..hashes_len].to_vec(),
            bet_indexes: data[hashes_len..hashes_len + indexes_len].to_vec(),
            entry_count,
            total_count,
            name_hash_bit_size,
            index_size_total,
            index_size,
        })
    }

    fn and_mask(&self) -> u64 {
        if self.name_hash_bit_size == 64 {
            u64::MAX
        } else {
            (1u64 << self.name_hash_bit_size) - 1
        }
    }

    fn or_mask(&self) -> u64 {
        1u64 << (self.name_hash_bit_size - 1)
    }

    /// 用完整（掩码后）的名称哈希探测 HET 表；按探测顺序返回候选
    /// 文件索引。
    pub fn lookup(&self, hash: u64) -> Vec<u32> {
        let file_hash = (hash & self.and_mask()) | self.or_mask();
        let hash1 = (file_hash >> (self.name_hash_bit_size - 8)) as u8;

        let total = self.total_count as u64;
        let mut slot = file_hash % total;
        let start = slot;
        let mut result = Vec::new();

        loop {
            let probe = self.name_hash1[slot as usize];
            if probe == 0 {
                break; // 空闲槽位终止探测
            }
            if probe == hash1 {
                let idx = get_bits(
                    &self.bet_indexes,
                    slot as usize * self.index_size_total as usize,
                    self.index_size as usize,
                );
                result.push(idx as u32);
            }
            slot = (slot + 1) % total;
            if slot == start {
                break;
            }
        }
        result
    }
}

// ---------------------------------------------------------------------------
// BET 表

/// 单条 BET 文件记录，已解析成经典结构的字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BetFileRecord {
    pub file_pos: u64,
    pub file_size: u32,
    pub cmp_size: u32,
    pub flags: u32,
}

/// 已解析的 BET 表。
#[derive(Debug, Clone)]
pub struct BetTable {
    pub flags: Vec<u32>,
    pub file_table: Vec<u8>,
    pub name_hashes: Vec<u8>,
    pub entry_count: u32,
    pub table_entry_size: u32,
    pub bit_index_file_pos: u32,
    pub bit_index_file_size: u32,
    pub bit_index_cmp_size: u32,
    pub bit_index_flag_index: u32,
    pub bit_count_file_pos: u32,
    pub bit_count_file_size: u32,
    pub bit_count_cmp_size: u32,
    pub bit_count_flag_index: u32,
    pub bit_total_name_hash2: u32,
    pub bit_count_name_hash2: u32,
}

impl BetTable {
    pub fn parse(table: &[u8]) -> Result<Self> {
        if table.len() < 12 + 76 {
            return Err(Error::Corrupt("BET table too small"));
        }
        let entry_count = le_u32(table, 16);
        let flag_count = le_u32(table, 84) as usize;

        if entry_count > 50_000_000 {
            return Err(Error::Corrupt("BET entry count out of range"));
        }

        let mut pos = 88usize;
        let data = &table[pos..];
        if data.len() < flag_count * 4 {
            return Err(Error::Corrupt("BET flags array truncated"));
        }
        let mut flags = Vec::with_capacity(flag_count);
        for i in 0..flag_count {
            flags.push(le_u32(data, i * 4));
        }
        pos += flag_count * 4;

        let table_entry_size = le_u32(table, 24);
        let file_table_bits = entry_count as usize * table_entry_size as usize;
        let file_table_bytes = file_table_bits.div_ceil(8);
        if table.len() < pos + file_table_bytes {
            return Err(Error::Corrupt("BET file table truncated"));
        }
        let file_table = table[pos..pos + file_table_bytes].to_vec();
        pos += file_table_bytes;

        let bit_total_name_hash2 = le_u32(table, 68);
        let hash_bytes = (entry_count as usize * bit_total_name_hash2 as usize).div_ceil(8);
        if table.len() < pos + hash_bytes {
            return Err(Error::Corrupt("BET name hash array truncated"));
        }
        let name_hashes = table[pos..pos + hash_bytes].to_vec();

        Ok(BetTable {
            flags,
            file_table,
            name_hashes,
            entry_count,
            table_entry_size,
            bit_index_file_pos: le_u32(table, 28),
            bit_index_file_size: le_u32(table, 32),
            bit_index_cmp_size: le_u32(table, 36),
            bit_index_flag_index: le_u32(table, 40),
            bit_count_file_pos: le_u32(table, 48),
            bit_count_file_size: le_u32(table, 52),
            bit_count_cmp_size: le_u32(table, 56),
            bit_count_flag_index: le_u32(table, 60),
            bit_total_name_hash2,
            bit_count_name_hash2: le_u32(table, 76),
        })
    }

    /// 将文件记录 `index` 解码为经典结构的字段。
    pub fn record(&self, index: u32) -> BetFileRecord {
        let base = index as usize * self.table_entry_size as usize;
        let mut rec = BetFileRecord {
            file_pos: 0,
            file_size: 0,
            cmp_size: 0,
            flags: 0,
        };
        if self.bit_count_file_pos > 0 {
            rec.file_pos = get_bits(
                &self.file_table,
                base + self.bit_index_file_pos as usize,
                self.bit_count_file_pos as usize,
            );
        }
        if self.bit_count_file_size > 0 {
            rec.file_size = get_bits(
                &self.file_table,
                base + self.bit_index_file_size as usize,
                self.bit_count_file_size as usize,
            ) as u32;
        }
        if self.bit_count_cmp_size > 0 {
            rec.cmp_size = get_bits(
                &self.file_table,
                base + self.bit_index_cmp_size as usize,
                self.bit_count_cmp_size as usize,
            ) as u32;
        }
        if self.bit_count_flag_index > 0 && !self.flags.is_empty() {
            let flag_index = get_bits(
                &self.file_table,
                base + self.bit_index_flag_index as usize,
                self.bit_count_flag_index as usize,
            ) as usize;
            if flag_index < self.flags.len() {
                rec.flags = self.flags[flag_index];
            }
        }
        rec
    }

    /// 文件记录 `index` 所存储的（部分）名称哈希。
    pub fn name_hash2(&self, index: u32) -> u64 {
        get_bits(
            &self.name_hashes,
            index as usize * self.bit_total_name_hash2 as usize,
            self.bit_count_name_hash2 as usize,
        )
    }

    /// 结合在 HET 表中观察到的探测字节，为某条记录重建完整的
    /// HET 风格名称哈希。
    pub fn full_name_hash(&self, hash1: u8, index: u32) -> u64 {
        ((hash1 as u64) << self.bit_count_name_hash2) | self.name_hash2(index)
    }

    /// 将记录转换为经典 block table 条目。
    pub fn to_block_entry(&self, index: u32) -> BlockEntry {
        let rec = self.record(index);
        BlockEntry {
            pos: rec.file_pos as u32,
            csize: rec.cmp_size,
            fsize: rec.file_size,
            flags: rec.flags | FILE_EXISTS,
        }
    }
}

// ---------------------------------------------------------------------------
// 位操作辅助 —— LSB 优先的打包方式，与 StormLib 的 TMPQBits 完全一致。

/// 从绝对位偏移 `offset` 起读取 `bits` 位（LSB 优先）。
pub fn get_bits(data: &[u8], offset: usize, bits: usize) -> u64 {
    if bits == 0 || bits > 64 {
        return 0;
    }
    let mut result: u64 = 0;
    for i in 0..bits {
        let bit_pos = offset + i;
        let byte = data.get(bit_pos / 8).copied().unwrap_or(0);
        let bit = (byte >> (bit_pos % 8)) & 1;
        result |= (bit as u64) << i;
    }
    result
}

#[inline]
fn le_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_reader_lsb_first() {
        let data = [0b1010_0101u8, 0b0000_0011];
        assert_eq!(get_bits(&data, 0, 8), 0xA5);
        assert_eq!(get_bits(&data, 8, 8), 0x03);
        assert_eq!(get_bits(&data, 4, 8), 0x3A); // 0x0A 高半字节 | 0x03 低半字节
        assert_eq!(get_bits(&data, 0, 4), 0x5);
        assert_eq!(get_bits(&data, 0, 12), 0x3A5);
    }
}
