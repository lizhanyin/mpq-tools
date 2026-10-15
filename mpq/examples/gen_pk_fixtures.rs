//! 重新生成 `tests/fixtures` 目录下的 PKWARE DCL 测试向量。
//!
//! 运行方式：`cargo run -p mpq --example gen_pk_fixtures`。每个生成的
//! `.bin` 都会立即用 crate 的解码器（`mpq::decompress_imploded`）
//! explode 解压回来加以验证；无法往返（round-trip）的数据一律拒绝
//! 写入。
//!
//! 编码器移植自 PKWARE Data Compression Library 的 "implode" 一侧
//! （即 StormLib `pklib/explode.c` 所对应的压缩器），复用与解码器
//! 相同的固定长度/距离表：贪婪 LZSS 匹配、二进制模式、4 KB 字典：
//!
//! * 头部 `[ctype=0, dsize_bits=6]`，随后是 LSB 在前的位流；
//! * 字面量：`0` + 8 个数据位；
//! * 匹配：`1` + 规范长度码（`LEN_CODE`/`LEN_BITS`，随后是
//!   `len - 2 - LEN_BASE` 的 `EX_LEN_BITS` 个额外位）+ 规范距离码
//!   （`DIST_CODE`/`DIST_BITS`，随后是 `dist - 1` 的低
//!   `dsize_bits` 位）；
//! * 结束标记：长度码值 517（`lc=15`，额外位 `0xFF`）；
//! * 距离绝不回退到输出起点之前，也绝不会超出 4 KB 字典；长度上限
//!   为 518，因此永远不会与结束标记（码 517）冲突。

#![allow(dead_code)]

use std::path::PathBuf;

include!("../src/pkware_tables.rs");

const DSIZE_BITS: u32 = 6; // 4 KB 字典
const DSIZE_MASK: u32 = (1 << DSIZE_BITS) - 1;
const WINDOW: usize = 1 << (DSIZE_BITS + 6); // 64 个距离码 × 2^6 = 4096
const MAX_MATCH: usize = 518; // 长度码 516；517 是结束标记
const MIN_MATCH: usize = 3;

// ---------------------------------------------------------------------------
// LSB 在前的位写入器（解码器以流的第 2 个字节预填位缓冲）

struct BitWriter {
    out: Vec<u8>,
    buff: u32,
    cnt: u32,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter {
            out: vec![0, DSIZE_BITS as u8], // ctype = 二进制，字典大小
            buff: 0,
            cnt: 0,
        }
    }
    fn put(&mut self, value: u32, nbits: u32) {
        self.buff |= value << self.cnt;
        self.cnt += nbits;
        while self.cnt >= 8 {
            self.out.push((self.buff & 0xFF) as u8);
            self.buff >>= 8;
            self.cnt -= 8;
        }
    }
    fn finish(mut self) -> Vec<u8> {
        // 流结束：匹配标志 + 长度码 15（7 个零位）+ 0xFF。
        self.put(1, 1);
        self.put(LEN_CODE[15] as u32, LEN_BITS[15] as u32);
        self.put(0xFF, EX_LEN_BITS[15] as u32);
        if self.cnt > 0 {
            self.out.push((self.buff & 0xFF) as u8);
        }
        self.out
    }
}

// ---------------------------------------------------------------------------
// Implode 编码器

/// 匹配 `len` 字节对应的（长度码索引，额外位值）。
fn length_code(len: usize) -> (usize, u32) {
    let code = (len - 2) as u16;
    for lc in (0..16).rev() {
        if code >= LEN_BASE[lc] {
            return (lc, (code - LEN_BASE[lc]) as u32);
        }
    }
    unreachable!("len {len} below the shortest encodable match");
}

fn put_match(w: &mut BitWriter, len: usize, dist: usize) {
    debug_assert!((MIN_MATCH..=MAX_MATCH).contains(&len));
    debug_assert!(dist >= 1 && dist <= WINDOW);
    w.put(1, 1);
    let (lc, extra) = length_code(len);
    w.put(LEN_CODE[lc] as u32, LEN_BITS[lc] as u32);
    if EX_LEN_BITS[lc] > 0 {
        w.put(extra, EX_LEN_BITS[lc] as u32);
    }
    let d = (dist - 1) as u32;
    let dpc = (d >> DSIZE_BITS) as usize;
    w.put(DIST_CODE[dpc] as u32, DIST_BITS[dpc] as u32);
    w.put(d & DSIZE_MASK, DSIZE_BITS);
}

fn put_literal(w: &mut BitWriter, byte: u8) {
    w.put(0, 1);
    w.put(byte as u32, 8);
}

/// 基于 3 字节哈希链的贪婪 LZSS 前端。
struct Matcher<'a> {
    data: &'a [u8],
    head: Vec<i32>,
    prev: Vec<i32>,
}

const HASH_BITS: u32 = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;

impl<'a> Matcher<'a> {
    fn new(data: &'a [u8]) -> Self {
        Matcher {
            data,
            head: vec![-1; HASH_SIZE],
            prev: vec![-1; data.len()],
        }
    }

    fn hash3(&self, pos: usize) -> usize {
        let d = self.data;
        (((d[pos] as usize) << 10) ^ ((d[pos + 1] as usize) << 5) ^ (d[pos + 2] as usize))
            & (HASH_SIZE - 1)
    }

    fn insert(&mut self, pos: usize) {
        if pos + 3 > self.data.len() {
            return;
        }
        let h = self.hash3(pos);
        self.prev[pos] = self.head[h];
        self.head[h] = pos as i32;
    }

    /// 在字典窗口内寻找 `pos` 处的最长匹配。
    fn find(&self, pos: usize) -> Option<(usize, usize)> {
        let data = self.data;
        if pos + MIN_MATCH > data.len() {
            return None;
        }
        let window = WINDOW.min(pos); // 绝不回退到输出起点之前
        let mut best: Option<(usize, usize)> = None;
        let mut cand = self.head[self.hash3(pos)];
        while cand >= 0 {
            let p = cand as usize;
            let dist = pos - p;
            if dist > window {
                break;
            }
            let max_len = MAX_MATCH.min(data.len() - pos);
            let mut len = 0;
            while len < max_len && data[p + len] == data[pos + len] {
                len += 1;
            }
            if len >= MIN_MATCH && best.map_or(true, |(b, _)| len > b) {
                best = Some((len, dist));
            }
            cand = self.prev[p];
        }
        best
    }
}

fn implode(data: &[u8]) -> Vec<u8> {
    let mut w = BitWriter::new();
    let mut m = Matcher::new(data);
    let mut i = 0;
    while i < data.len() {
        match m.find(i) {
            Some((len, dist)) => {
                put_match(&mut w, len, dist);
                for p in i..i + len {
                    m.insert(p);
                }
                i += len;
            }
            None => {
                put_literal(&mut w, data[i]);
                m.insert(i);
                i += 1;
            }
        }
    }
    w.finish()
}

// ---------------------------------------------------------------------------
// 测试向量输入

/// 确定性的伪随机字节（不可压缩；用于覆盖字面量编码路径）。
fn rand_orig() -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut out = Vec::with_capacity(300);
    for _ in 0..300 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push((state >> 33) as u8);
    }
    out
}

/// 超过 0x2000 字节的长行程数据（用于覆盖长匹配、远距离
/// 以及解码器的环形缓冲刷新）。
fn runs_orig() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend(std::iter::repeat_n(0x5A, 700)); // 单个行程 > MAX_MATCH
    data.extend(std::iter::repeat_n(0x00, 600));
    for i in 0..180 {
        data.extend_from_slice(format!("run pattern iteration {i:03}: ABRACADABRA--").as_bytes());
    }
    // 对先前内容的一次远距离复制（距离约 3.5 KB）。
    let far_source = 700 + 600;
    let far_len = 500;
    let far_copy = data[far_source..far_source + far_len].to_vec();
    data.extend_from_slice(&far_copy);
    data.extend(std::iter::repeat_n(0xFF, 800));
    data
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn emit(dir: &PathBuf, name: &str, orig: &[u8]) {
    let stream = implode(orig);

    // 写入测试向量之前，先通过 crate 的解码器验证。
    let decoded = mpq::decompress_imploded(&stream, orig.len())
        .unwrap_or_else(|e| panic!("{name}: generated stream failed to explode: {e}"));
    assert_eq!(
        decoded.as_slice(),
        orig,
        "{name}: explode(implode(x)) != x — encoder bug"
    );

    let bin_name = name; // "pk_text" 等
    std::fs::write(dir.join(format!("{bin_name}.bin")), &stream).unwrap();
    std::fs::write(dir.join(format!("{bin_name}.orig")), orig).unwrap();
    println!(
        "{name}: {} -> {} bytes ({:.0}%)",
        orig.len(),
        stream.len(),
        100.0 * stream.len() as f64 / orig.len().max(1) as f64
    );
}

fn main() {
    let dir = fixtures_dir();
    std::fs::create_dir_all(&dir).unwrap();

    // pk_text.orig 是已提交的原始文件；只有压缩流是生成的。
    let text = std::fs::read(dir.join("pk_text.orig"))
        .expect("pk_text.orig must exist (it is committed)");
    {
        let stream = implode(&text);
        let decoded = mpq::decompress_imploded(&stream, text.len())
            .unwrap_or_else(|e| panic!("pk_text: generated stream failed to explode: {e}"));
        assert_eq!(decoded, text, "pk_text: round-trip mismatch");
        std::fs::write(dir.join("pk_text.bin"), &stream).unwrap();
        println!(
            "pk_text: {} -> {} bytes ({:.0}%)",
            text.len(),
            stream.len(),
            100.0 * stream.len() as f64 / text.len() as f64
        );
    }

    emit(&dir, "pk_rand", &rand_orig());
    emit(&dir, "pk_runs", &runs_orig());
}
