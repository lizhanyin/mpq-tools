//! Decompression of MPQ-compressed data (sectors, single units, tables).

use std::io::Read;

use crate::error::{Error, Result};
use crate::pkware;

/// zlib (deflate in the zlib container).
pub const M_HUFFMAN: u8 = 0x01;
pub const M_ZLIB: u8 = 0x02;
pub const M_PKWARE: u8 = 0x08;
pub const M_BZIP2: u8 = 0x10;
/// LZMA; numeric value 0x12 is a whole method, not a flag combination.
pub const M_LZMA: u8 = 0x12;
pub const M_SPARSE: u8 = 0x20;
#[allow(dead_code)] // part of the method whitelist below
pub const M_ADPCM_MONO: u8 = 0x40;
#[allow(dead_code)] // part of the method whitelist below
pub const M_ADPCM_STEREO: u8 = 0x80;

/// Every method byte value StormLib's `SCompDecompress2` recognizes
/// (a strict whitelist — combinations outside this set are invalid).
const KNOWN_METHODS: [u8; 10] = [
    M_HUFFMAN, // 0x01
    M_ZLIB,    // 0x02
    M_PKWARE,  // 0x08
    M_BZIP2,   // 0x10
    M_LZMA,    // 0x12
    M_SPARSE,  // 0x20
    0x22,      // sparse + zlib
    0x30,      // sparse + bzip2
    0x41,      // huffman + adpcm mono
    0x81,      // huffman + adpcm stereo
];

/// Methods this library can actually decode.
const PROCESSABLE: [u8; 4] = [M_ZLIB, M_PKWARE, M_BZIP2, M_LZMA];

fn zlib_decompress(data: &[u8], max_out: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(data)
        .take(max_out as u64)
        .read_to_end(&mut out)
        .map_err(|e| Error::InvalidData(format!("zlib: {e}")))?;
    Ok(out)
}

fn bzip2_decompress(data: &[u8], max_out: usize) -> Result<Vec<u8>> {
    let mut reader = bzip2_rs::DecoderReader::new(data);
    let mut out = Vec::new();
    reader
        .read_to_end(&mut out)
        .map_err(|e| Error::InvalidData(format!("bzip2: {e}")))?;
    out.truncate(max_out);
    Ok(out)
}

fn lzma_decompress(data: &[u8], max_out: usize) -> Result<Vec<u8>> {
    // MPQ LZMA block: [filter byte (must be 0)][5-byte props][8-byte size][stream].
    // We rebuild a plain .lzma container for the decoder.
    if data.len() <= 14 || data[0] != 0 {
        return Err(Error::Corrupt("bad LZMA block header"));
    }
    let mut stream = Vec::with_capacity(data.len() - 1);
    stream.extend_from_slice(&data[1..6]); // props
    stream.extend_from_slice(&data[6..14]); // uncompressed size
    stream.extend_from_slice(&data[14..]);

    let mut out = Vec::with_capacity(max_out.min(1 << 20));
    let mut cursor = std::io::Cursor::new(&stream);
    lzma_rs::lzma_decompress(&mut cursor, &mut out)
        .map_err(|e| Error::InvalidData(format!("lzma: {e}")))?;
    out.truncate(max_out);
    Ok(out)
}

/// Apply a single decompression step (no method byte).
fn decompress_one(method: u8, data: &[u8], max_out: usize) -> Result<Vec<u8>> {
    match method {
        M_ZLIB => zlib_decompress(data, max_out),
        M_PKWARE => {
            let mut out = Vec::with_capacity(max_out);
            pkware::explode(data, &mut out, max_out)?;
            Ok(out)
        }
        M_BZIP2 => bzip2_decompress(data, max_out),
        M_LZMA => lzma_decompress(data, max_out),
        _ => Err(Error::UnsupportedCompression(method)),
    }
}

/// Decompress an MPQ compressed block that carries a method byte
/// (sector data of `COMPRESS`-flagged files, single units, ext tables).
///
/// The first byte selects one method or a chain of methods; chains are
/// unwrapped in StormLib's decompression-table order
/// (bzip2, pkware, zlib, ...), which is the reverse of how the compressor
/// stacks them.
pub fn decompress_masked(data: &[u8], max_out: usize) -> Result<Vec<u8>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let mask = data[0];

    // Strict whitelist like StormLib's SCompDecompress2. LZMA (0x12) is a
    // standalone method value, never a flag combination.
    if KNOWN_METHODS.contains(&mask) {
        if !PROCESSABLE.contains(&mask) {
            return Err(Error::UnsupportedCompression(mask));
        }
        if mask == M_LZMA {
            return lzma_decompress(&data[1..], max_out);
        }
        let mut current: Vec<u8> = data[1..].to_vec();
        for method in [M_BZIP2, M_PKWARE, M_ZLIB] {
            if mask & method != 0 {
                current = decompress_one(method, &current, max_out)?;
            }
        }
        return Ok(current);
    }

    // Fallback (tolerance): the method byte is unknown or zero — assume it
    // is missing entirely and the whole block is a plain zlib stream (a
    // zlib block starts with 0x78, which is not a whitelisted method).
    match zlib_decompress(data, max_out) {
        Ok(out) => Ok(out),
        Err(_) => Err(Error::UnsupportedCompression(mask)),
    }
}

/// Decompress a raw PKWARE stream stored under the legacy `IMPLODE` flag
/// (no method byte).
pub fn decompress_imploded(data: &[u8], max_out: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(max_out);
    pkware::explode(data, &mut out, max_out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zlib_compress(data: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn zlib_roundtrip_via_mask() {
        let plain = b"hello hello hello compressed data 1234567890".repeat(4);
        let mut block = vec![M_ZLIB];
        block.extend_from_slice(&zlib_compress(&plain));
        let out = decompress_masked(&block, plain.len()).unwrap();
        assert_eq!(out, plain);
    }

    #[test]
    fn pkware_via_mask() {
        let stream = include_bytes!("../tests/fixtures/pk_text.bin");
        let plain = include_bytes!("../tests/fixtures/pk_text.orig");
        let mut block = vec![M_PKWARE];
        block.extend_from_slice(stream);
        let out = decompress_masked(&block, plain.len()).unwrap();
        assert_eq!(out.as_slice(), &plain[..]);
    }

    #[test]
    fn missing_mask_byte_falls_back_to_zlib() {
        let plain = b"no mask byte here, plain zlib follows";
        let block = zlib_compress(plain);
        let out = decompress_masked(&block, plain.len()).unwrap();
        assert_eq!(out, plain);
    }

    #[test]
    fn unsupported_methods_reported() {
        let block = vec![M_SPARSE, 1, 2, 3];
        assert!(matches!(
            decompress_masked(&block, 16),
            Err(Error::UnsupportedCompression(0x20))
        ));
    }
}
