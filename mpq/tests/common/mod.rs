//! 仅用于测试的 MPQ 归档构建器。
//!
//! 为经典表（hash/block/hi-block 表）生成字节级精确的归档，覆盖 v1–v4
//! 各头部版本，支持 zlib 压缩、storm 加密、key-v2、single-unit 文件、
//! sector CRC、`(listfile)`、`(attributes)`、`MPQ\x1B` user-data 包装、
//! v4 的 MD5 校验字段（各表 + 头部），以及可选的每文件 raw-chunk MD5。

#![allow(dead_code)]

use std::io::Write as _;

use md5::{Digest as _, Md5};
use mpq::{encrypt_bytes, file_key, hash_string, HASH_NAME_A, HASH_NAME_B, HASH_TABLE_INDEX};

pub fn md5_of(data: &[u8]) -> [u8; 16] {
    let mut hasher = Md5::new();
    hasher.update(data);
    hasher.finalize().into()
}

pub const KEY_HASH_TABLE: u32 = 0xC3AF_3770;
pub const KEY_BLOCK_TABLE: u32 = 0xEC83_B3A3;

pub const FILE_EXISTS: u32 = 0x8000_0000;
pub const FILE_SINGLE_UNIT: u32 = 0x0100_0000;
pub const FILE_SECTOR_CRC: u32 = 0x0400_0000;
pub const FILE_ENCRYPTED: u32 = 0x0001_0000;
pub const FILE_KEY_V2: u32 = 0x0002_0000;
pub const FILE_COMPRESS: u32 = 0x0000_0200;
pub const FILE_IMPLODE: u32 = 0x0000_0100;

/// 单个存储文件的布局方式。
#[derive(Clone)]
pub struct FileSpec {
    pub name: String,
    pub data: Vec<u8>,
    pub compress: bool,
    pub encrypt: bool,
    pub single_unit: bool,
    pub fix_key: bool,
    /// 预编码的原始负载（以 method-0x02 zlib 内容存储）；
    /// 覆盖 `data`。
    pub raw_payload: Option<Vec<u8>>,
    /// 预编码的 PKWARE implode 流及其未压缩大小；
    /// 以旧式 IMPLODE 标志存储，不带方法字节。
    pub imploded: Option<(Vec<u8>, u32)>,
}

impl FileSpec {
    pub fn text(name: &str, data: &str) -> Self {
        FileSpec::bytes(name, data.as_bytes().to_vec())
    }
    pub fn bytes(name: &str, data: Vec<u8>) -> Self {
        FileSpec {
            name: name.to_string(),
            data,
            compress: false,
            encrypt: false,
            single_unit: false,
            fix_key: false,
            raw_payload: None,
            imploded: None,
        }
    }
    pub fn compress(mut self) -> Self {
        self.compress = true;
        self
    }
    pub fn encrypt(mut self) -> Self {
        self.encrypt = true;
        self
    }
    pub fn single_unit(mut self) -> Self {
        self.single_unit = true;
        self
    }
    pub fn fix_key(mut self) -> Self {
        self.fix_key = true;
        self
    }
    /// 以旧式 PKWARE implode 文件存储：`stream` 为原始 DCL 流，
    /// `real_size` 为未压缩大小。
    pub fn imploded(mut self, stream: Vec<u8>, real_size: u32) -> Self {
        self.imploded = Some((stream, real_size));
        self
    }
}

#[derive(Default, Clone)]
pub struct ArchiveSpec {
    /// 0 = v1，1 = v2，2 = v3，3 = v4。
    pub version: u16,
    /// sector 大小的指数（sector = 512 << shift）。
    pub sector_shift: u16,
    pub with_listfile: bool,
    /// 包含带 CRC32 的 `(attributes)` 文件。
    pub with_attributes: bool,
    /// 写入 hi-block 表（v2+）。
    pub with_hi_block: bool,
    /// 用 `MPQ\x1B` user-data 头包装。
    pub userdata: bool,
    /// 归档前部的垃圾字节（测试头部扫描）。
    pub prefix_garbage: usize,
    /// 在归档大小字段写入 0（容错测试）。
    pub zero_archive_size: bool,
    /// v4：每个 block 后面写入的每文件 raw-data MD5 的 chunk 大小
    /// （0 = 不写）。
    pub raw_chunk_size: u32,
    pub files: Vec<FileSpec>,
}

pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn put_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn next_pow2(n: usize) -> usize {
    let mut p = 4;
    while p < n {
        p <<= 1;
    }
    p
}

/// 构建完整归档的原始字节。
pub fn build(spec: &ArchiveSpec) -> Vec<u8> {
    let mut files = spec.files.clone();
    if spec.with_listfile {
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        files.push(
            FileSpec::text("(listfile)", &names.join("\r\n")).compress(),
        );
    }
    if spec.with_attributes {
        // CRC 数组覆盖每个 block 条目：真实文件、(listfile)，
        // 以及 (attributes) 文件自身对应的 0。
        let mut attr = Vec::new();
        put_u32(&mut attr, 100);
        put_u32(&mut attr, 1); // CRC32 标志
        for f in &spec.files {
            put_u32(&mut attr, crc32(&f.data));
        }
        if spec.with_listfile {
            let names: Vec<&str> = spec.files.iter().map(|f| f.name.as_str()).collect();
            put_u32(&mut attr, crc32(names.join("
").as_bytes()));
        }
        put_u32(&mut attr, 0); // (attributes) 文件自身
        files.push(FileSpec::text("(attributes)", "").raw_payload_compressed(attr));
    }

    let sector_size = 512u32 << spec.sector_shift;
    let header_size: u32 = match spec.version {
        0 => 0x20,
        1 => 0x2C,
        2 => 0x44,
        _ => 0xD0, // v4
    };
    // v4 raw-chunk MD5：每个 block 后跟 ceil(stored / chunk) 个摘要，
    // 不计入 block 表的 csize（StormLib WriteMemDataMD5）。
    let chunk_overhead = |stored: usize| {
        if spec.raw_chunk_size > 0 {
            stored.div_ceil(spec.raw_chunk_size as usize) * 16
        } else {
            0
        }
    };

    // 大小从不依赖密钥（加密不改变长度），因此用占位位置做一次
    // 测量即可得到精确长度。
    let measured: Vec<usize> = files
        .iter()
        .map(|f| {
            let stored = encode_file(f, 0, sector_size).0.len();
            stored + chunk_overhead(stored)
        })
        .collect();

    // 分配各 block 的位置（key-v2 派生需要）；像真实归档一样把
    // 每个 block 填充到 dword 对齐。
    let mut cursor = header_size as u64;
    let mut block_positions = Vec::with_capacity(files.len());
    for len in &measured {
        block_positions.push(cursor);
        cursor += *len as u64;
        cursor = (cursor + 3) & !3;
    }

    // 编码各文件 block。
    let mut blocks: Vec<(u64, u32, u32, u32)> = Vec::with_capacity(files.len());
    let mut block_data: Vec<Vec<u8>> = Vec::with_capacity(files.len());
    for (i, f) in files.iter().enumerate() {
        let pos = block_positions[i];
        let (data, csize, fsize, mut flags) =
            encode_file(f, pos, sector_size);
        debug_assert_eq!(data.len() + chunk_overhead(data.len()), measured[i]);
        flags |= FILE_EXISTS;
        if f.compress && f.imploded.is_none() {
            flags |= FILE_COMPRESS;
        }
        if f.encrypt {
            flags |= FILE_ENCRYPTED;
        }
        if f.fix_key {
            flags |= FILE_KEY_V2;
        }
        if f.single_unit || f.raw_payload.is_some() || f.imploded.is_some() {
            flags |= FILE_SINGLE_UNIT;
        }
        debug_assert_eq!(data.len(), csize as usize);
        block_data.push(data);
        blocks.push((pos, csize, fsize, flags));
    }

    // 哈希表。
    let hash_size = next_pow2(files.len() * 2);
    let mut hash_entries = vec![[0u8; 16]; hash_size];
    for e in hash_entries.iter_mut() {
        e[12..16].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    }
    for (i, f) in files.iter().enumerate() {
        let start = hash_string(&f.name, HASH_TABLE_INDEX) as usize % hash_size;
        let mut slot = start;
        while u32::from_le_bytes([
            hash_entries[slot][12],
            hash_entries[slot][13],
            hash_entries[slot][14],
            hash_entries[slot][15],
        ]) != 0xFFFF_FFFF
        {
            slot = (slot + 1) % hash_size;
        }
        let e = &mut hash_entries[slot];
        e[0..4].copy_from_slice(&hash_string(&f.name, HASH_NAME_A).to_le_bytes());
        e[4..8].copy_from_slice(&hash_string(&f.name, HASH_NAME_B).to_le_bytes());
        e[8] = 0; // locale 低字节
        e[9] = 0; // locale 高字节
        e[10] = 0; // platform
        e[11] = 0; // flags
        e[12..16].copy_from_slice(&(i as u32).to_le_bytes());
    }
    let mut hash_table: Vec<u8> = hash_entries.concat();
    encrypt_bytes(&mut hash_table, KEY_HASH_TABLE);

    // 块表。
    let mut block_table = Vec::new();
    for (pos, csize, fsize, flags) in &blocks {
        put_u32(&mut block_table, *pos as u32);
        put_u32(&mut block_table, *csize);
        put_u32(&mut block_table, *fsize);
        put_u32(&mut block_table, *flags);
    }
    encrypt_bytes(&mut block_table, KEY_BLOCK_TABLE);

    let hash_pos = cursor;
    cursor += hash_table.len() as u64;
    let block_pos = cursor;
    cursor += block_table.len() as u64;
    let hi_pos = if spec.with_hi_block {
        let p = cursor;
        cursor += blocks.len() as u64 * 2;
        Some(p)
    } else {
        None
    };
    let archive_size = cursor;

    // 头部。
    let mut header = Vec::new();
    header.extend_from_slice(b"MPQ\x1a");
    put_u32(&mut header, header_size);
    if spec.zero_archive_size {
        put_u32(&mut header, 0);
    } else {
        put_u32(&mut header, archive_size as u32);
    }
    put_u16(&mut header, spec.version);
    put_u16(&mut header, spec.sector_shift);
    put_u32(&mut header, hash_pos as u32);
    put_u32(&mut header, block_pos as u32);
    put_u32(&mut header, hash_size as u32);
    put_u32(&mut header, blocks.len() as u32);
    if spec.version >= 1 {
        put_u64(&mut header, hi_pos.unwrap_or(0));
        put_u16(&mut header, 0); // 哈希表位置高 16 位
        put_u16(&mut header, 0); // 块表位置高 16 位
    }
    if spec.version >= 2 {
        put_u64(&mut header, archive_size);
        put_u64(&mut header, 0); // BET 表位置
        put_u64(&mut header, 0); // HET 表位置
    }
    if spec.version >= 3 {
        // 仅 v4：显式的表存储大小 + 对存储（加密后）字节的 MD5，
        // 然后是头部自身对 0x00..0xC0 的 MD5。
        put_u64(&mut header, hash_table.len() as u64); // 哈希表 size64
        put_u64(&mut header, block_table.len() as u64); // 块表 size64
        put_u64(
            &mut header,
            hi_pos.map(|_| blocks.len() as u64 * 2).unwrap_or(0),
        );
        put_u64(&mut header, 0); // HET size64
        put_u64(&mut header, 0); // BET size64
        put_u32(&mut header, spec.raw_chunk_size);
        header.extend_from_slice(&md5_of(&block_table)); // 0x70 MD5_BlockTable
        header.extend_from_slice(&md5_of(&hash_table)); // 0x80 MD5_HashTable
        let hi_bytes = vec![0u8; blocks.len() * 2]; // hi 字全为零
        if hi_pos.is_some() {
            header.extend_from_slice(&md5_of(&hi_bytes)); // 0x90 MD5_HiBlock
        } else {
            header.extend_from_slice(&[0u8; 16]);
        }
        header.extend_from_slice(&[0u8; 16]); // 0xA0 MD5_BetTable
        header.extend_from_slice(&[0u8; 16]); // 0xB0 MD5_HetTable
        header.extend_from_slice(&md5_of(&header[..0xC0])); // 0xC0 头部 MD5
    }
    debug_assert_eq!(header.len(), header_size as usize);

    let mut out = Vec::new();
    // 垃圾前缀：像内嵌归档一样保持归档 0x200 对齐。
    while out.len() < spec.prefix_garbage {
        out.push(0xEE);
    }
    if spec.userdata {
        out.extend_from_slice(b"MPQ\x1b");
        put_u32(&mut out, 0x10); // user data 大小
        put_u32(&mut out, 0); // 头部偏移，稍后修补
        put_u32(&mut out, 0x10); // user data 头大小
        let header_off = out.len() as u32;
        out[8..12].copy_from_slice(&header_off.to_le_bytes());
    }
    let archive_start = out.len();
    // 把归档相对偏移修正为相对归档起点：它们本来就是（cursor 从
    // header_size 起步），而头部就写在 archive_start 处，因此无需调整。
    out.extend_from_slice(&header);
    for (i, data) in block_data.iter().enumerate() {
        out.extend_from_slice(data);
        if spec.raw_chunk_size > 0 {
            // 每个 chunk 的 MD5 紧跟在存储数据之后（不计入 csize），
            // 然后照常填充到 dword 对齐。
            let chunk = spec.raw_chunk_size as usize;
            for c in data.chunks(chunk) {
                out.extend_from_slice(&md5_of(c));
            }
        }
        let pad = (4 - measured[i] % 4) % 4;
        out.extend(std::iter::repeat_n(0, pad));
    }
    out.extend_from_slice(&hash_table);
    out.extend_from_slice(&block_table);
    if hi_pos.is_some() {
        for _ in 0..blocks.len() {
            put_u16(&mut out, 0);
        }
    }
    debug_assert_eq!(out.len() - archive_start, archive_size as usize);
    out
}

impl FileSpec {
    /// 存储负载已经是 zlib 压缩内容的文件
    /// （用于 `(attributes)` 等）。
    fn raw_payload_compressed(mut self, payload: Vec<u8>) -> Self {
        self.compress = true;
        self.raw_payload = Some(payload);
        self
    }
}

/// 编码单个文件的存储 block。
fn encode_file(f: &FileSpec, block_pos: u64, sector_size: u32) -> (Vec<u8>, u32, u32, u32) {
    if let Some(raw) = &f.raw_payload {
        let z = zlib(raw);
        let mut block = vec![0x02u8];
        block.extend_from_slice(&z);
        if f.encrypt {
            let key = file_key(&f.name, f.fix_key, block_pos, raw.len() as u32);
            encrypt_bytes(&mut block, key);
        }
        let csize = block.len() as u32;
        return (block, csize, raw.len() as u32, 0);
    }

    if let Some((stream, real_size)) = &f.imploded {
        let mut payload = stream.clone();
        if f.encrypt {
            let key = file_key(&f.name, f.fix_key, block_pos, *real_size);
            encrypt_bytes(&mut payload, key);
        }
        let csize = payload.len() as u32;
        return (payload, csize, *real_size, FILE_IMPLODE);
    }

    let fsize = f.data.len() as u32;
    let key = if f.encrypt {
        file_key(&f.name, f.fix_key, block_pos, fsize)
    } else {
        0
    };

    if f.single_unit {
        let mut payload = if f.compress {
            let mut b = vec![0x02u8];
            b.extend_from_slice(&zlib(&f.data));
            b
        } else {
            f.data.clone()
        };
        if f.encrypt {
            encrypt_bytes(&mut payload, key);
        }
        let csize = payload.len() as u32;
        return (payload, csize, fsize, 0);
    }

    if !f.compress {
        // sector 按固定偏移存储，无 sector 表；每个 sector 用各自的
        // 密钥（key + index）加密。
        let mut payload = Vec::with_capacity(f.data.len());
        for (i, chunk) in f.data.chunks(sector_size as usize).enumerate() {
            let mut c = chunk.to_vec();
            if f.encrypt {
                encrypt_bytes(&mut c, key.wrapping_add(i as u32));
            }
            payload.extend_from_slice(&c);
        }
        let csize = payload.len() as u32;
        return (payload, csize, fsize, 0);
    }

    // 压缩 sector 布局。
    let nsec = fsize.div_ceil(sector_size).max(1) as usize;
    let mut sectors: Vec<Vec<u8>> = Vec::with_capacity(nsec);
    let mut crcs: Vec<u32> = Vec::with_capacity(nsec);
    for chunk in f.data.chunks(sector_size as usize) {
        let z = zlib(chunk);
        let sector = if z.len() < chunk.len() {
            let mut s = vec![0x02u8];
            s.extend_from_slice(&z);
            s
        } else {
            chunk.to_vec()
        };
        // 加密后的 sector 填充到 dword 对齐（偏移量包含填充）。
        let mut sector = sector;
        if f.encrypt && sector.len() % 4 != 0 {
            let pad_to = sector.len() + 4 - sector.len() % 4;
            sector.resize(pad_to, 0);
        }
        crcs.push(crc32(chunk));
        sectors.push(sector);
    }
    let table_len = (nsec + 1) * 4 + nsec * 4; // 偏移量 + CRC
    let mut offsets: Vec<u32> = Vec::with_capacity(nsec + 1);
    offsets.push(table_len as u32);
    let mut acc = table_len as u32;
    for s in &sectors {
        acc += s.len() as u32;
        offsets.push(acc);
    }

    let mut table: Vec<u8> = Vec::with_capacity(table_len);
    for o in &offsets {
        put_u32(&mut table, *o);
    }
    for c in &crcs {
        put_u32(&mut table, *c);
    }
    if f.encrypt {
        encrypt_bytes(&mut table, key.wrapping_sub(1));
    }

    let mut block = table;
    for (i, mut s) in sectors.into_iter().enumerate() {
        if f.encrypt {
            encrypt_bytes(&mut s, key.wrapping_add(i as u32));
        }
        block.extend_from_slice(&s);
    }
    let csize = block.len() as u32;
    (block, csize, fsize, FILE_SECTOR_CRC)
}

// ---------------------------------------------------------------------------
// v3 HET + BET 归档构建器（无经典 hash/block 表）

use mpq::file_name_hash;

const HET_SIG: u32 = 0x1A54_4548;
const BET_SIG: u32 = 0x1A54_4542;

fn bits_needed(v: u64) -> u32 {
    let mut n = 1u32;
    let mut x = v;
    while x > 1 {
        x >>= 1;
        n += 1;
    }
    n
}

fn set_bits(buf: &mut [u8], offset: usize, bits: usize, value: u64) {
    for i in 0..bits {
        if (value >> i) & 1 == 1 {
            let p = offset + i;
            buf[p / 8] |= 1 << (p % 8);
        }
    }
}

/// 构建一个 v3 归档，其唯一名称索引是 HET，唯一文件表是 BET
/// （经典 hash/block 表声明为空）。
pub fn build_hetbet(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    build_hetbet_version(files, 2)
}

/// 同上，但使用 v4 头部（显式的 HET/BET 存储大小 + MD5）。
pub fn build_hetbet_v4(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    build_hetbet_version(files, 3)
}

/// 在 [`build_hetbet`] 基础上追加一个列出各文件名的 `(listfile)` 条目 —
/// 真实的 v3 归档带有它，这样名称能在经典表重写后保留下来。
pub fn build_hetbet_with_listfile(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut all: Vec<(String, Vec<u8>)> = files.to_vec();
    let names: Vec<&str> = files.iter().map(|f| f.0.as_str()).collect();
    all.push(("(listfile)".to_string(), names.join("\r\n").into_bytes()));
    build_hetbet(&all)
}

pub fn build_hetbet_version(files: &[(String, Vec<u8>)], version: u16) -> Vec<u8> {
    let sector_size = 4096u32;
    let header_size: u64 = if version >= 3 { 0xD0 } else { 0x44 };

    // 编码所有文件 block（未压缩，sector 直接存储）。
    let mut cursor = header_size;
    let mut blocks = Vec::new();
    for (name, data) in files {
        let pos = cursor;
        cursor += data.len() as u64;
        cursor = (cursor + 3) & !3;
        let flags = FILE_EXISTS;
        blocks.push((name, data, pos, data.len() as u32, flags));
    }

    // --- BET 表 ---
    let n = blocks.len();
    let max_pos = blocks.iter().map(|b| b.2).max().unwrap_or(0);
    let max_size = blocks.iter().map(|b| b.1.len() as u64).max().unwrap_or(0);
    let bc_pos = bits_needed(max_pos);
    let bc_size = bits_needed(max_size);
    let bc_csize = bits_needed(max_size); // 存储时 csize == fsize
    let mut flag_list: Vec<u32> = Vec::new();
    for b in &blocks {
        if !flag_list.contains(&b.4) {
            flag_list.push(b.4);
        }
    }
    let bc_flag = bits_needed(flag_list.len() as u64);
    let entry_size = (bc_pos + bc_size + bc_csize + bc_flag) as usize;

    let records_bytes = (n * entry_size).div_ceil(8);
    let hash_bits = 56usize;
    let hash_bytes = (n * hash_bits).div_ceil(8);
    let mut bet_body = Vec::new();
    let table_size = 76 + flag_list.len() * 4 + records_bytes + hash_bytes;
    put_u32(&mut bet_body, table_size as u32); // +0x0C dwTableSize
    put_u32(&mut bet_body, n as u32); // 条目数
    put_u32(&mut bet_body, 0); // unknown08
    put_u32(&mut bet_body, entry_size as u32); // 表条目大小（位）
    put_u32(&mut bet_body, 0); // 文件位置的位偏移
    put_u32(&mut bet_body, bc_pos); // 文件大小的位偏移
    put_u32(&mut bet_body, bc_pos + bc_size); // 压缩大小的位偏移
    put_u32(&mut bet_body, bc_pos + bc_size + bc_csize); // 标志的位偏移
    put_u32(&mut bet_body, bc_pos + bc_size + bc_csize + bc_flag); // 未知字段的位偏移
    put_u32(&mut bet_body, bc_pos); // 文件位置的位数
    put_u32(&mut bet_body, bc_size); // 文件大小的位数
    put_u32(&mut bet_body, bc_csize); // 压缩大小的位数
    put_u32(&mut bet_body, bc_flag); // 标志的位数
    put_u32(&mut bet_body, 0); // 未知字段的位数
    put_u32(&mut bet_body, hash_bits as u32); // 名称 hash2 的总位数
    put_u32(&mut bet_body, 0); // 名称 hash2 的额外位数
    put_u32(&mut bet_body, hash_bits as u32); // 名称 hash2 的位数
    put_u32(&mut bet_body, hash_bytes as u32); // 名称 hash 数组大小
    put_u32(&mut bet_body, flag_list.len() as u32); // 标志数量
    debug_assert_eq!(bet_body.len(), 76);
    for f in &flag_list {
        put_u32(&mut bet_body, *f);
    }
    let mut records = vec![0u8; records_bytes];
    let mut hashes = vec![0u8; hash_bytes];
    for (i, b) in blocks.iter().enumerate() {
        let base = i * entry_size;
        set_bits(&mut records, base, bc_pos as usize, b.2);
        set_bits(&mut records, base + bc_pos as usize, bc_size as usize, b.1.len() as u64);
        set_bits(
            &mut records,
            base + (bc_pos + bc_size) as usize,
            bc_csize as usize,
            b.1.len() as u64,
        );
        let flag_index = flag_list.iter().position(|f| *f == b.4).unwrap();
        set_bits(
            &mut records,
            base + (bc_pos + bc_size + bc_csize) as usize,
            bc_flag as usize,
            flag_index as u64,
        );
        let h = file_name_hash(b.0) & ((1u64 << hash_bits) - 1);
        set_bits(&mut hashes, i * hash_bits, hash_bits, h);
    }
    bet_body.extend_from_slice(&records);
    bet_body.extend_from_slice(&hashes);
    let mut bet = Vec::new();
    put_u32(&mut bet, BET_SIG);
    put_u32(&mut bet, 1);
    put_u32(&mut bet, table_size as u32);
    bet.extend_from_slice(&bet_body);
    encrypt_bytes(&mut bet[12..], KEY_BLOCK_TABLE);

    // --- HET 表 ---
    let total_count = next_pow2(n * 4 / 3);
    let index_bits = bits_needed(n as u64) as usize;
    let index_bytes = (total_count * index_bits).div_ceil(8);
    let mut probe = vec![0u8; total_count];
    let mut indexes = vec![0u8; index_bytes];
    for (i, b) in blocks.iter().enumerate() {
        let h = file_name_hash(b.0) | (1u64 << 63); // OrMask
        let b1 = (h >> 56) as u8;
        let mut slot = (h % total_count as u64) as usize;
        while probe[slot] != 0 {
            slot = (slot + 1) % total_count;
        }
        probe[slot] = b1;
        set_bits(&mut indexes, slot * index_bits, index_bits, i as u64);
    }
    let het_table_size = 32 + total_count + index_bytes;
    let mut het_body = Vec::new();
    put_u32(&mut het_body, het_table_size as u32);
    put_u32(&mut het_body, n as u32); // 条目数
    put_u32(&mut het_body, total_count as u32); // 总槽位数
    put_u32(&mut het_body, 64); // 名称 hash 位大小
    put_u32(&mut het_body, index_bits as u32); // 索引总位数
    put_u32(&mut het_body, 0); // 索引额外位数
    put_u32(&mut het_body, index_bits as u32); // 索引位数
    put_u32(&mut het_body, index_bytes as u32); // 索引表大小
    het_body.extend_from_slice(&probe);
    het_body.extend_from_slice(&indexes);
    let mut het = Vec::new();
    put_u32(&mut het, HET_SIG);
    put_u32(&mut het, 1);
    put_u32(&mut het, het_table_size as u32);
    het.extend_from_slice(&het_body);
    encrypt_bytes(&mut het[12..], KEY_HASH_TABLE);

    // --- 组装（v3 规范顺序：先 HET，后 BET） ---
    let het_pos = cursor;
    cursor += het.len() as u64;
    let bet_pos = cursor;
    cursor += bet.len() as u64;
    let archive_size = cursor;

    let mut out = Vec::new();
    out.extend_from_slice(b"MPQ\x1a");
    put_u32(&mut out, header_size as u32);
    put_u32(&mut out, archive_size as u32);
    put_u16(&mut out, version);
    put_u16(&mut out, 3); // 4096 字节的 sector
    put_u32(&mut out, 0); // 哈希表位置
    put_u32(&mut out, 0); // 块表位置
    put_u32(&mut out, 0); // 哈希表大小
    put_u32(&mut out, 0); // 块表大小
    put_u64(&mut out, 0); // hi block 位置
    put_u16(&mut out, 0);
    put_u16(&mut out, 0);
    put_u64(&mut out, archive_size); // 归档大小（64 位）
    put_u64(&mut out, bet_pos); // BET 位置
    put_u64(&mut out, het_pos); // HET 位置
    if version >= 3 {
        // 仅 v4：为（仅有的）存在的表写入存储大小 + MD5。
        put_u64(&mut out, 0); // 哈希表 size64
        put_u64(&mut out, 0); // 块表 size64
        put_u64(&mut out, 0); // hi-block size64
        put_u64(&mut out, het.len() as u64); // HET size64
        put_u64(&mut out, bet.len() as u64); // BET size64
        put_u32(&mut out, 0); // raw chunk 大小
        out.extend_from_slice(&[0u8; 16]); // MD5_BlockTable
        out.extend_from_slice(&[0u8; 16]); // MD5_HashTable
        out.extend_from_slice(&[0u8; 16]); // MD5_HiBlockTable
        out.extend_from_slice(&md5_of(&bet)); // MD5_BetTable
        out.extend_from_slice(&md5_of(&het)); // MD5_HetTable
        out.extend_from_slice(&md5_of(&out[..0xC0])); // 头部 MD5
    }
    debug_assert_eq!(out.len(), header_size as usize);
    for (_, data, _, _, _) in &blocks {
        out.extend_from_slice(data);
        let pad = (4 - data.len() % 4) % 4;
        out.extend(std::iter::repeat_n(0, pad));
    }
    out.extend_from_slice(&het);
    out.extend_from_slice(&bet);
    let _ = sector_size;
    out
}
