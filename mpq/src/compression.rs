//! MPQ 压缩数据的解压（sector、单单元、各表）。

use std::io::Read;

use crate::error::{Error, Result};
use crate::pkware;

/// zlib（zlib 容器中的 deflate）。
pub const M_HUFFMAN: u8 = 0x01;
pub const M_ZLIB: u8 = 0x02;
pub const M_PKWARE: u8 = 0x08;
pub const M_BZIP2: u8 = 0x10;
/// LZMA；数值 0x12 是一个完整的方法，而非标志组合。
pub const M_LZMA: u8 = 0x12;
pub const M_SPARSE: u8 = 0x20;
#[allow(dead_code)] // 下方方法白名单的一部分
pub const M_ADPCM_MONO: u8 = 0x40;
#[allow(dead_code)] // 下方方法白名单的一部分
pub const M_ADPCM_STEREO: u8 = 0x80;

/// StormLib 的 `SCompDecompress2` 认可的全部方法字节值
/// （严格白名单 —— 不在此集合中的组合均无效）。
const KNOWN_METHODS: [u8; 10] = [
    M_HUFFMAN, // 0x01
    M_ZLIB,    // 0x02
    M_PKWARE,  // 0x08
    M_BZIP2,   // 0x10
    M_LZMA,    // 0x12
    M_SPARSE,  // 0x20
    0x22,      // sparse + zlib
    0x30,      // sparse + bzip2
    0x41,      // huffman + adpcm 单声道
    0x81,      // huffman + adpcm 立体声
];

/// 本库实际能够解码的方法。
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
    // MPQ LZMA 块：[过滤字节（必须为 0）][5 字节 props][8 字节大小][数据流]。
    // 我们为解码器重建一个普通 .lzma 容器。
    if data.len() <= 14 || data[0] != 0 {
        return Err(Error::Corrupt("bad LZMA block header"));
    }
    let mut stream = Vec::with_capacity(data.len() - 1);
    stream.extend_from_slice(&data[1..6]); // props
    stream.extend_from_slice(&data[6..14]); // 未压缩大小
    stream.extend_from_slice(&data[14..]);

    let mut out = Vec::with_capacity(max_out.min(1 << 20));
    let mut cursor = std::io::Cursor::new(&stream);
    lzma_rs::lzma_decompress(&mut cursor, &mut out)
        .map_err(|e| Error::InvalidData(format!("lzma: {e}")))?;
    out.truncate(max_out);
    Ok(out)
}

/// 执行单步解压（无方法字节）。
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

/// 解压带有方法字节的 MPQ 压缩块
/// （`COMPRESS` 标志文件的 sector 数据、单单元、扩展表）。
///
/// 首字节选择单个方法或一条方法链；链按 StormLib 解压表的顺序
/// （bzip2、pkware、zlib、……）逐层剥开，与压缩器叠加时的顺序相反。
pub fn decompress_masked(data: &[u8], max_out: usize) -> Result<Vec<u8>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let mask = data[0];

    // 与 StormLib 的 SCompDecompress2 一样的严格白名单。LZMA (0x12) 是
    // 独立的方法值，绝不是标志组合。
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

    // 回退（宽容处理）：方法字节未知或为 0 —— 假定它完全缺失，
    // 整个块就是一个普通 zlib 流（zlib 块以 0x78 开头，
    // 而 0x78 不在白名单方法之列）。
    match zlib_decompress(data, max_out) {
        Ok(out) => Ok(out),
        Err(_) => Err(Error::UnsupportedCompression(mask)),
    }
}

/// 解压以旧式 `IMPLODE` 标志存储的原始 PKWARE 流
/// （无方法字节）。
pub fn decompress_imploded(data: &[u8], max_out: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(max_out);
    pkware::explode(data, &mut out, max_out)?;
    Ok(out)
}

/// 压缩为原始 zlib 流（[`decompress_masked`] 中 zlib 方法在写入侧的
/// 对应实现；由修改器与密码 AEAD 编码共用）。
pub fn zlib_compress(data: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    enc.write_all(data).expect("in-memory zlib");
    enc.finish().expect("in-memory zlib")
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
