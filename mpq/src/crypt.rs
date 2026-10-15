//! Storm 风格的加密、哈希与名称处理。
//!
//! 本模块中的一切都复刻了暴雪 `storm.dll` / StormLib 所用的算法：
//! 0x500 项的 crypt table、经典 hash table 所用的四个哈希函数，
//! 以及施加于各表、sector 偏移表和文件数据的对称流密码。

/// 构建 0x500 项的 storm crypt table（经 `const` 在编译期完成）。
const fn build_crypt_table() -> [u32; 0x500] {
    let mut table = [0u32; 0x500];
    let mut seed: u32 = 0x0010_0001;
    let mut index1: usize = 0;
    while index1 < 0x100 {
        let mut i: usize = 0;
        while i < 5 {
            let index2 = index1 + i * 0x100;
            seed = (seed * 125 + 3) % 0x2AAAAB;
            let temp1 = (seed & 0xFFFF) << 16;
            seed = (seed * 125 + 3) % 0x2AAAAB;
            let temp2 = seed & 0xFFFF;
            table[index2] = temp1 | temp2;
            i += 1;
        }
        index1 += 1;
    }
    table
}

/// storm crypt table，编译期生成。
pub static CRYPT_TABLE: [u32; 0x500] = build_crypt_table();

/// 经典 hash table 索引使用的哈希类型（`hash % table_size`）。
pub const HASH_TABLE_INDEX: u32 = 0x000;
/// 生成 hash table 条目 `name1` 的哈希类型。
pub const HASH_NAME_A: u32 = 0x100;
/// 生成 hash table 条目 `name2` 的哈希类型。
pub const HASH_NAME_B: u32 = 0x200;
/// 生成文件级加密密钥的哈希类型。
pub const HASH_FILE_KEY: u32 = 0x300;
/// 流密码所用 "key2 mix" 子表的偏移。
pub const HASH_KEY2_MIX: u32 = 0x400;

/// 经典 hash table 的加密密钥：`HashString("(hash table)", FILE_KEY)`。
pub const KEY_HASH_TABLE: u32 = 0xC3AF_3770;
/// 经典 block table 的加密密钥：`HashString("(block table)", FILE_KEY)`。
pub const KEY_BLOCK_TABLE: u32 = 0xEC83_B3A3;

/// 经 storm 大写映射表转换单个字节：ASCII `a-z` 映射为 `A-Z`，
/// `/` 映射为 `\`。>= 0x80 的字节原样通过。
#[inline]
pub fn upper_byte(b: u8) -> u8 {
    match b {
        b'a'..=b'z' => b - 0x20,
        b'/' => b'\\',
        _ => b,
    }
}

/// 经 storm 小写映射表转换单个字节：ASCII `A-Z` 映射为 `a-z`，
/// `/` 映射为 `\`。用于 HET/BET 名称哈希。
#[inline]
pub fn lower_byte(b: u8) -> u8 {
    match b {
        b'A'..=b'Z' => b + 0x20,
        b'/' => b'\\',
        _ => b,
    }
}

/// 按经典表看待文件名的方式将其规范化：
/// ASCII 大写化，并把 `/` 转换为 `\`。
pub fn normalize_name(name: &str) -> Vec<u8> {
    name.bytes().map(upper_byte).collect()
}

/// 对已规范化的字节串执行 Storm `HashString`。
///
/// `hash_type` 为 [`HASH_TABLE_INDEX`]、[`HASH_NAME_A`]、
/// [`HASH_NAME_B`] 或 [`HASH_FILE_KEY`] 之一。
pub fn hash_bytes(normalized: &[u8], hash_type: u32) -> u32 {
    let mut seed1: u32 = 0x7FED_7FED;
    let mut seed2: u32 = 0xEEEE_EEEE;

    for &b in normalized {
        let ch = b as u32;
        seed1 = CRYPT_TABLE[(hash_type + ch) as usize] ^ seed1.wrapping_add(seed2);
        seed2 = ch
            .wrapping_add(seed1)
            .wrapping_add(seed2)
            .wrapping_add(seed2 << 5)
            .wrapping_add(3);
    }
    seed1
}

/// 对原始名称执行 Storm `HashString`（内部先规范化）。
pub fn hash_string(name: &str, hash_type: u32) -> u32 {
    hash_bytes(&normalize_name(name), hash_type)
}

/// 用 storm 流密码就地解密 `data`（以小端 u32 为单位）。
/// `data.len()` 应为 4 的倍数；末尾不足的字节保持原样
/// （真实归档总是对齐的）。
pub fn decrypt_bytes(data: &mut [u8], key: u32) {
    let mut key1 = key;
    let mut key2: u32 = 0xEEEE_EEEE;

    for chunk in data.chunks_exact_mut(4) {
        let value = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        key2 = key2.wrapping_add(CRYPT_TABLE[(HASH_KEY2_MIX + (key1 & 0xFF)) as usize]);
        let plain = value ^ key1.wrapping_add(key2);
        key1 = ((!key1) << 21)
            .wrapping_add(0x1111_1111)
            | (key1 >> 11);
        key2 = plain
            .wrapping_add(key2)
            .wrapping_add(key2 << 5)
            .wrapping_add(3);
        chunk.copy_from_slice(&plain.to_le_bytes());
    }
}

/// 就地加密 `data`（[`decrypt_bytes`] 的逆操作）。
pub fn encrypt_bytes(data: &mut [u8], key: u32) {
    let mut key1 = key;
    let mut key2: u32 = 0xEEEE_EEEE;

    for chunk in data.chunks_exact_mut(4) {
        let plain = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        key2 = key2.wrapping_add(CRYPT_TABLE[(HASH_KEY2_MIX + (key1 & 0xFF)) as usize]);
        let cipher = plain ^ key1.wrapping_add(key2);
        key1 = ((!key1) << 21)
            .wrapping_add(0x1111_1111)
            | (key1 >> 11);
        key2 = plain
            .wrapping_add(key2)
            .wrapping_add(key2 << 5)
            .wrapping_add(3);
        chunk.copy_from_slice(&cipher.to_le_bytes());
    }
}

/// 派生文件级加密密钥。
///
/// * `name` – 归档内的完整文件名
/// * `fix_key` – 块带有 `KEY_V2`/`FIX_KEY` 标志
/// * `block_pos` – 文件块相对于归档起始处的偏移
/// * `file_size` – 未压缩的文件大小
///
/// storm 算法只对基础文件名（剥离路径后）做哈希，
/// key-v2 文件再应用 `(key + offset) ^ size`。
pub fn file_key(name: &str, fix_key: bool, block_pos: u64, file_size: u32) -> u32 {
    let plain = name.rsplit(['\\', '/']).next().unwrap_or(name);
    let mut key = hash_string(plain, HASH_FILE_KEY);
    if fix_key {
        key = key.wrapping_add(block_pos as u32) ^ file_size;
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 这两个表密钥是 StormLib 文档中记载的常量；命中它们即证明
    /// crypt table 与哈希函数逐字节精确。
    #[test]
    fn hash_matches_stormlib_constants() {
        assert_eq!(hash_string("(hash table)", HASH_FILE_KEY), KEY_HASH_TABLE);
        assert_eq!(hash_string("(block table)", HASH_FILE_KEY), KEY_BLOCK_TABLE);
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let original: Vec<u8> = (0..100u32).map(|i| (i * 7 + 3) as u8).collect();
        let mut buf = original.clone();
        encrypt_bytes(&mut buf, 0x1234_5678);
        assert_ne!(buf, original);
        decrypt_bytes(&mut buf, 0x1234_5678);
        assert_eq!(buf, original);
    }

    #[test]
    fn uppercase_and_slash_normalization() {
        assert_eq!(normalize_name("abc/def.txt"), b"ABC\\DEF.TXT");
        assert_eq!(hash_string("abc\\def.txt", 1), hash_string("ABC/DEF.txt", 1));
    }
}
