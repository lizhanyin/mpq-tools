//! 为 Java 移植版（nineyin-mpq）生成对拍夹具：
//!
//! * gen_v1..v4.mpq —— create + modify 产出的标准归档（zlib、经典表），
//!   附 manifest.txt（名字<TAB>sha256<TAB>大小）；
//! * enc_v1.mpq —— 手工构造的 v1 归档，覆盖加密（名字密钥 / key-v2）、
//!   sector CRC、PKWARE implode、明文存储等读取路径，并在本例内用
//!   mpq::Archive 回读自检。
//!
//! 用法：cargo run --example gen_java_fixtures -- <输出目录>

use std::io::Write as _;
use std::path::PathBuf;

use mpq::{
    create, modify, encrypt_bytes, file_key, hash_string, Archive, CreateOptions, FormatVersion,
    HashEntry, FILE_COMPRESS, FILE_ENCRYPTED, FILE_EXISTS, FILE_IMPLODE, FILE_KEY_V2,
    FILE_SECTOR_CRC, FILE_SINGLE_UNIT, HASH_FILE_KEY, HASH_NAME_A, HASH_NAME_B,
    HASH_TABLE_INDEX, MPQ_MAGIC,
};
use sha2::{Digest as _, Sha256};

/// 未导出的哨兵/密钥常量，按库内定义本地重声明。
const HASH_ENTRY_FREE: u32 = 0xFFFF_FFFF;

fn main() {
    let out_dir = PathBuf::from(std::env::args().nth(1).expect("用法: gen_java_fixtures <输出目录>"));
    std::fs::create_dir_all(&out_dir).unwrap();

    // --- 确定性内容 ---------------------------------------------------
    let files: Vec<(String, Vec<u8>)> = vec![
        ("hello.txt".into(), b"hello mpq\r\nsecond line\r\n".to_vec()),
        ("empty.dat".into(), Vec::new()),
        ("docs/readme.md".into(), readme_bytes()),
        ("data/pattern.bin".into(), pattern_bytes(300_000, 0x1234_5678)),
        ("blob/noise.bin".into(), noise_bytes(200_000, 0xdead_beef)),
        ("deep/dir/tree/leaf.txt".into(), "deep leaf".repeat(500).into_bytes()),
    ];

    let mut manifest = String::new();
    for (name, data) in &files {
        manifest.push_str(&format!("{}\t{}\t{}\n", name, sha256_hex(data), data.len()));
    }
    std::fs::write(out_dir.join("manifest.txt"), &manifest).unwrap();

    for (v, name) in [
        (FormatVersion::V1, "gen_v1.mpq"),
        (FormatVersion::V2, "gen_v2.mpq"),
        (FormatVersion::V3, "gen_v3.mpq"),
        (FormatVersion::V4, "gen_v4.mpq"),
    ] {
        let base = create(&CreateOptions {
            version: v,
            hash_table_size: 64,
            sector_shift: 3,
        })
        .unwrap();
        let out = modify(&base, &[], &files).unwrap();
        // 自检：Rust 端必须能读回全部内容。
        let check = Archive::new(std::io::Cursor::new(&out.bytes)).unwrap();
        assert_eq!(check.version(), v);
        for (n, d) in &files {
            assert_eq!(&check.read_file(n).unwrap(), d, "{name}:{n}");
        }
        std::fs::write(out_dir.join(name), &out.bytes).unwrap();
        println!("写出 {name} ({} 字节)", out.bytes.len());
    }

    // --- 手工加密/CRC/PKWARE 归档 --------------------------------------
    let enc = build_encrypted_archive();
    {
        let check = Archive::new(std::io::Cursor::new(&enc.0)).unwrap();
        for (n, d) in &enc.1 {
            assert_eq!(&check.read_file(n).unwrap(), d, "enc_v1:{n}");
        }
    }
    std::fs::write(out_dir.join("enc_v1.mpq"), &enc.0).unwrap();
    println!("写出 enc_v1.mpq ({} 字节)", enc.0.len());
}

fn readme_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..80 {
        out.extend_from_slice(
            format!("## 第 {i} 行\n九阴真经资源打包测试文本 line {i:04}\r\n").as_bytes(),
        );
    }
    out
}

/// 半随机但可压缩的内容（游程 + 递变）。
fn pattern_bytes(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed | 1;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        let run = 1 + (x & 0x3F) as usize;
        let byte = (x >> 8) as u8;
        out.extend(std::iter::repeat_n(byte, run.min(len - out.len())));
    }
    out
}

/// 不可压缩的伪随机内容。
fn noise_bytes(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed | 1;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        out.push((x >> 16) as u8);
    }
    out
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

const SECTOR: usize = 4096;

/// 手工构造加密归档，返回 (归档字节, 期望内容列表)。
fn build_encrypted_archive() -> (Vec<u8>, Vec<(&'static str, Vec<u8>)>) {
    let mut expect: Vec<(&'static str, Vec<u8>)> = Vec::new();
    let mut placed: Vec<(usize, Vec<u8>, u32, u32)> = Vec::new(); // (pos, bytes, fsize, flags)
    let mut cursor = 0x20usize; // v1 头大小

    let mut place = |placed: &mut Vec<(usize, Vec<u8>, u32, u32)>,
                     cursor: &mut usize,
                     bytes: Vec<u8>,
                     fsize: u32,
                     flags: u32| {
        let pos = *cursor;
        *cursor += bytes.len();
        *cursor = (*cursor + 3) & !3;
        placed.push((pos, bytes, fsize, flags));
    };

    // 1) 明文小文件（single unit，不压缩）。
    let plain: Vec<u8> = b"plain stored single-unit file\n".to_vec();
    place(&mut placed, &mut cursor, plain.clone(), plain.len() as u32,
        FILE_EXISTS | FILE_SINGLE_UNIT);
    expect.push(("plain.txt", plain));

    // 2) 名字密钥加密 + zlib 单块。
    let secret = pattern_bytes(3000, 0x0bad_f00d);
    let key = file_key("secret.txt", false, 0, secret.len() as u32);
    let mut payload = vec![0x02u8];
    payload.extend_from_slice(&zlib(&secret));
    encrypt_bytes(&mut payload, key);
    place(&mut placed, &mut cursor, payload, secret.len() as u32,
        FILE_EXISTS | FILE_SINGLE_UNIT | FILE_COMPRESS | FILE_ENCRYPTED);
    expect.push(("secret.txt", secret));

    // 3) key-v2 加密 + 压缩 sector（偏移表 key-1、sector key+i、dword 填充），
    //    布局与 mpq::modify::encode_fresh_reencrypted 一致。
    let fixed_src = pattern_bytes(13_000, 0x0f11_0f11);
    {
        let pos = cursor as u64;
        let (bytes, flags) = encode_key_v2("fixed.bin", &fixed_src, pos, true);
        place(&mut placed, &mut cursor, bytes, fixed_src.len() as u32, flags | FILE_EXISTS);
    }
    expect.push(("fixed.bin", fixed_src));

    // 4) key-v2 加密 + 不可压缩（固定偏移明文 sector，逐个 key+i 加密）。
    let rand_src = noise_bytes(9_500, 0x5eed_5eed);
    {
        let key = file_key("rand.bin", true, cursor as u64, rand_src.len() as u32);
        let mut payload = rand_src.clone();
        for (i, chunk) in payload.chunks_mut(SECTOR).enumerate() {
            encrypt_bytes(chunk, key.wrapping_add(i as u32));
        }
        place(&mut placed, &mut cursor, payload, rand_src.len() as u32,
            FILE_EXISTS | FILE_ENCRYPTED | FILE_KEY_V2);
    }
    expect.push(("rand.bin", rand_src));

    // 5) 压缩 + sector CRC。
    let crc_src = pattern_bytes(12_000, 0xcccc_0001);
    {
        let (bytes, flags) = encode_with_crc(&crc_src);
        place(&mut placed, &mut cursor, bytes, crc_src.len() as u32, flags | FILE_EXISTS);
    }
    expect.push(("crc.bin", crc_src));

    // 6) PKWARE implode 单块（无方法字节，流取自现有夹具）。
    let pk_stream = std::fs::read("tests/fixtures/pk_text.bin").unwrap();
    let pk_plain = std::fs::read("tests/fixtures/pk_text.orig").unwrap();
    place(&mut placed, &mut cursor, pk_stream, pk_plain.len() as u32,
        FILE_EXISTS | FILE_SINGLE_UNIT | FILE_IMPLODE);
    expect.push(("pk.txt", pk_plain));

    // --- 哈希表 / 块表 ------------------------------------------------
    const HASH_SLOTS: usize = 16;
    let mut hash_entries = vec![
        HashEntry { name1: HASH_ENTRY_FREE, name2: HASH_ENTRY_FREE, locale: 0, platform: 0,
                    flags: 0, block_index: HASH_ENTRY_FREE };
        HASH_SLOTS
    ];

    let names = ["plain.txt", "secret.txt", "fixed.bin", "rand.bin", "crc.bin", "pk.txt"];
    for (block_index, name) in names.iter().enumerate() {
        let start = (hash_string(name, HASH_TABLE_INDEX) as usize) % HASH_SLOTS;
        for off in 0..HASH_SLOTS {
            let slot = (start + off) % HASH_SLOTS;
            if hash_entries[slot].block_index == HASH_ENTRY_FREE {
                hash_entries[slot] = HashEntry {
                    name1: hash_string(name, HASH_NAME_A),
                    name2: hash_string(name, HASH_NAME_B),
                    locale: 0,
                    platform: 0,
                    flags: 0,
                    block_index: block_index as u32,
                };
                break;
            }
        }
    }

    let key_hash_table = hash_string("(hash table)", HASH_FILE_KEY);
    let key_block_table = hash_string("(block table)", HASH_FILE_KEY);

    let mut hash_buf = Vec::with_capacity(HASH_SLOTS * 16);
    for e in &hash_entries {
        hash_buf.extend_from_slice(&e.name1.to_le_bytes());
        hash_buf.extend_from_slice(&e.name2.to_le_bytes());
        hash_buf.extend_from_slice(&e.locale.to_le_bytes());
        hash_buf.push(e.platform);
        hash_buf.push(e.flags);
        hash_buf.extend_from_slice(&e.block_index.to_le_bytes());
    }
    encrypt_bytes(&mut hash_buf, key_hash_table);

    let mut block_buf = Vec::with_capacity(placed.len() * 16);
    for (pos, bytes, fsize, flags) in &placed {
        block_buf.extend_from_slice(&(*pos as u32).to_le_bytes());
        block_buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        block_buf.extend_from_slice(&fsize.to_le_bytes());
        block_buf.extend_from_slice(&flags.to_le_bytes());
    }
    encrypt_bytes(&mut block_buf, key_block_table);

    let hash_pos = cursor;
    cursor += hash_buf.len();
    let block_pos = cursor;
    cursor += block_buf.len();

    // --- 头 + 组装 ------------------------------------------------------
    let mut header = vec![0u8; 0x20];
    header[0..4].copy_from_slice(&MPQ_MAGIC.to_le_bytes());
    header[4..8].copy_from_slice(&0x20u32.to_le_bytes());
    header[8..12].copy_from_slice(&(cursor as u32).to_le_bytes());
    header[12..14].copy_from_slice(&0u16.to_le_bytes()); // v1
    header[14..16].copy_from_slice(&3u16.to_le_bytes()); // 4096 字节扇区
    header[16..20].copy_from_slice(&(hash_pos as u32).to_le_bytes());
    header[20..24].copy_from_slice(&(block_pos as u32).to_le_bytes());
    header[24..28].copy_from_slice(&(HASH_SLOTS as u32).to_le_bytes());
    header[28..32].copy_from_slice(&(placed.len() as u32).to_le_bytes());

    let mut out = Vec::with_capacity(cursor);
    out.extend_from_slice(&header);
    for (pos, bytes, _, _) in &placed {
        out.extend(std::iter::repeat_n(0u8, pos - out.len())); // 对齐填充
        out.extend_from_slice(bytes);
    }
    out.extend(std::iter::repeat_n(0u8, hash_pos - out.len()));
    out.extend_from_slice(&hash_buf);
    out.extend_from_slice(&block_buf);
    assert_eq!(out.len(), cursor);

    (out, expect)
}

/// 与 mpq::modify::encode_fresh_reencrypted 一致的 key-v2 编码。
fn encode_key_v2(name: &str, data: &[u8], pos: u64, compress: bool) -> (Vec<u8>, u32) {
    let enc_flags = FILE_ENCRYPTED | FILE_KEY_V2;
    let key = file_key(name, true, pos, data.len() as u32);
    let _ = compress;

    if data.len() < SECTOR {
        let z = zlib(data);
        let compressed = z.len() + 1 < data.len();
        let mut payload = if compressed {
            let mut b = vec![0x02u8];
            b.extend_from_slice(&z);
            b
        } else {
            data.to_vec()
        };
        encrypt_bytes(&mut payload, key);
        return (payload,
            enc_flags | FILE_SINGLE_UNIT | if compressed { FILE_COMPRESS } else { 0 });
    }

    let nsec = data.len().div_ceil(SECTOR);
    let mut sectors: Vec<Vec<u8>> = data
        .chunks(SECTOR)
        .map(|chunk| {
            let z = zlib(chunk);
            if z.len() + 4 < chunk.len() {
                let mut s = vec![0x02u8];
                s.extend_from_slice(&z);
                s
            } else {
                chunk.to_vec()
            }
        })
        .collect();

    // 先全部填充到 dword 边界，再按填充后的大小计算偏移表（偏移含填充）。
    for s in sectors.iter_mut() {
        if s.len() % 4 != 0 {
            s.resize(s.len() + 4 - s.len() % 4, 0);
        }
    }

    let table_len = (nsec + 1) * 4;
    let mut block =
        Vec::with_capacity(table_len + sectors.iter().map(|s| s.len()).sum::<usize>());
    let mut acc = table_len as u32;
    block.extend_from_slice(&acc.to_le_bytes());
    for s in &sectors {
        acc += s.len() as u32;
        block.extend_from_slice(&acc.to_le_bytes());
    }
    encrypt_bytes(&mut block[..table_len], key.wrapping_sub(1));
    for (i, s) in sectors.iter_mut().enumerate() {
        encrypt_bytes(s, key.wrapping_add(i as u32));
        block.extend_from_slice(s);
    }
    (block, enc_flags | FILE_COMPRESS)
}

/// zlib sector + 每 sector CRC（偏移表后跟 nsec 个 CRC dword）。
fn encode_with_crc(data: &[u8]) -> (Vec<u8>, u32) {
    let nsec = data.len().div_ceil(SECTOR);
    let sectors: Vec<Vec<u8>> = data
        .chunks(SECTOR)
        .map(|chunk| {
            let z = zlib(chunk);
            if z.len() + 1 < chunk.len() {
                let mut s = vec![0x02u8];
                s.extend_from_slice(&z);
                s
            } else {
                chunk.to_vec()
            }
        })
        .collect();

    let table_len = (nsec + 1 + nsec) * 4;
    let mut block =
        Vec::with_capacity(table_len + sectors.iter().map(|s| s.len()).sum::<usize>());
    let mut acc = table_len as u32;
    block.extend_from_slice(&acc.to_le_bytes());
    for s in &sectors {
        acc += s.len() as u32;
        block.extend_from_slice(&acc.to_le_bytes());
    }
    for chunk in data.chunks(SECTOR) {
        block.extend_from_slice(&crc32_of(chunk).to_le_bytes());
    }
    for s in &sectors {
        block.extend_from_slice(s);
    }
    (block, FILE_SECTOR_CRC | FILE_COMPRESS)
}

fn crc32_of(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    crc ^ 0xFFFF_FFFF
}
