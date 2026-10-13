//! Storm-style encryption, hashing and name handling.
//!
//! Everything in this module mirrors the algorithms used by Blizzard's
//! `storm.dll` / StormLib: the 0x500-entry crypt table, the four hash
//! functions used for the classic hash table, and the symmetric stream
//! cipher applied to tables, sector offset tables and file data.

/// Build the 0x500-entry storm crypt table (compile-time via `const`).
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

/// The storm crypt table, generated at compile time.
pub static CRYPT_TABLE: [u32; 0x500] = build_crypt_table();

/// Hash type for the classic hash table index (`hash % table_size`).
pub const HASH_TABLE_INDEX: u32 = 0x000;
/// Hash type producing `name1` of a hash table entry.
pub const HASH_NAME_A: u32 = 0x100;
/// Hash type producing `name2` of a hash table entry.
pub const HASH_NAME_B: u32 = 0x200;
/// Hash type producing the per-file encryption key.
pub const HASH_FILE_KEY: u32 = 0x300;
/// Offset of the "key2 mix" sub-table used by the stream cipher.
pub const HASH_KEY2_MIX: u32 = 0x400;

/// Encryption key of the classic hash table: `HashString("(hash table)", FILE_KEY)`.
pub const KEY_HASH_TABLE: u32 = 0xC3AF_3770;
/// Encryption key of the classic block table: `HashString("(block table)", FILE_KEY)`.
pub const KEY_BLOCK_TABLE: u32 = 0xEC83_B3A3;

/// Map a byte through the storm uppercase table: ASCII `a-z` to `A-Z`
/// and `/` to `\`. Bytes >= 0x80 are passed through unchanged.
#[inline]
pub fn upper_byte(b: u8) -> u8 {
    match b {
        b'a'..=b'z' => b - 0x20,
        b'/' => b'\\',
        _ => b,
    }
}

/// Map a byte through the storm lowercase table: ASCII `A-Z` to `a-z`
/// and `/` to `\`. Used by the HET/BET name hash.
#[inline]
pub fn lower_byte(b: u8) -> u8 {
    match b {
        b'A'..=b'Z' => b + 0x20,
        b'/' => b'\\',
        _ => b,
    }
}

/// Normalize a file name the way the classic tables see it:
/// ASCII-uppercased with `/` converted to `\`.
pub fn normalize_name(name: &str) -> Vec<u8> {
    name.bytes().map(upper_byte).collect()
}

/// Storm `HashString` over an already normalized byte string.
///
/// `hash_type` is one of [`HASH_TABLE_INDEX`], [`HASH_NAME_A`],
/// [`HASH_NAME_B`] or [`HASH_FILE_KEY`].
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

/// Storm `HashString` over a raw name (normalized internally).
pub fn hash_string(name: &str, hash_type: u32) -> u32 {
    hash_bytes(&normalize_name(name), hash_type)
}

/// Decrypt `data` (little-endian u32 units) in place with the storm stream
/// cipher. `data.len()` should be a multiple of 4; any trailing bytes are
/// left untouched (real archives always align).
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

/// Encrypt `data` in place (inverse of [`decrypt_bytes`]).
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

/// Derive the per-file encryption key.
///
/// * `name` – the full in-archive file name
/// * `fix_key` – the block has the `KEY_V2`/`FIX_KEY` flag
/// * `block_pos` – file block offset relative to the archive start
/// * `file_size` – uncompressed file size
///
/// The storm algorithm hashes only the base name (path stripped),
/// and `(key + offset) ^ size` is applied for key-v2 files.
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

    /// The two table keys are documented constants in StormLib; hitting
    /// them proves the crypt table + hash function are byte-exact.
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
