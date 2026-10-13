//! Regenerates the PKWARE DCL test vectors under `tests/fixtures`.
//!
//! Run with `cargo run -p mpq --example gen_pk_fixtures`. Every generated
//! `.bin` is immediately verified by exploding it back through the crate's
//! decoder (`mpq::decompress_imploded`); the tool refuses to write files
//! that do not round-trip.
//!
//! The encoder is a port of the PKWARE Data Compression Library "implode"
//! side (the compressor counterpart of StormLib's `pklib/explode.c`),
//! reusing the same fixed length/distance tables the decoder uses:
//! greedy LZSS matching, binary mode, 4 KB dictionary:
//!
//! * header `[ctype=0, dsize_bits=6]`, then an LSB-first bit stream;
//! * literal `0` + 8 payload bits;
//! * match `1` + canonical length code (`LEN_CODE`/`LEN_BITS`, then
//!   `EX_LEN_BITS` extra bits of `len - 2 - LEN_BASE`) + canonical
//!   distance code (`DIST_CODE`/`DIST_BITS`, then the low
//!   `dsize_bits` bits of `dist - 1`);
//! * end marker: the length-code value 517 (`lc=15`, extra `0xFF`);
//! * distances never reach before the start of the output and never
//!   exceed the 4 KB dictionary; lengths are capped at 518 so they can
//!   never collide with the end marker (code 517).

#![allow(dead_code)]

use std::path::PathBuf;

include!("../src/pkware_tables.rs");

const DSIZE_BITS: u32 = 6; // 4 KB dictionary
const DSIZE_MASK: u32 = (1 << DSIZE_BITS) - 1;
const WINDOW: usize = 1 << (DSIZE_BITS + 6); // 64 dist codes × 2^6 = 4096
const MAX_MATCH: usize = 518; // length code 516; 517 is the end marker
const MIN_MATCH: usize = 3;

// ---------------------------------------------------------------------------
// LSB-first bit writer (the decoder primes itself on byte 2 of the stream)

struct BitWriter {
    out: Vec<u8>,
    buff: u32,
    cnt: u32,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter {
            out: vec![0, DSIZE_BITS as u8], // ctype = binary, dict size
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
        // End of stream: match flag + length code 15 (7 zero bits) + 0xFF.
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
// Impplode encoder

/// (length code index, extra bits value) for a match of `len` bytes.
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

/// Greedy LZSS front end over a 3-byte hash chain.
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

    /// Longest match for `pos` within the dictionary window.
    fn find(&self, pos: usize) -> Option<(usize, usize)> {
        let data = self.data;
        if pos + MIN_MATCH > data.len() {
            return None;
        }
        let window = WINDOW.min(pos); // never reach before the output start
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
// Fixture inputs

/// Deterministic pseudo-random bytes (incompressible; exercises literals).
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

/// Run-heavy data past 0x2000 bytes (exercises long matches, far distances
/// and the decoder's ring-buffer flushes).
fn runs_orig() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend(std::iter::repeat_n(0x5A, 700)); // single run > MAX_MATCH
    data.extend(std::iter::repeat_n(0x00, 600));
    for i in 0..180 {
        data.extend_from_slice(format!("run pattern iteration {i:03}: ABRACADABRA--").as_bytes());
    }
    // A far-distance copy of earlier content (distance ~3.5 KB).
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

    // Verify through the crate's decoder before touching the fixtures.
    let decoded = mpq::decompress_imploded(&stream, orig.len())
        .unwrap_or_else(|e| panic!("{name}: generated stream failed to explode: {e}"));
    assert_eq!(
        decoded.as_slice(),
        orig,
        "{name}: explode(implode(x)) != x — encoder bug"
    );

    let bin_name = name; // "pk_text" etc.
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

    // pk_text.orig is the committed original; only the stream is generated.
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
