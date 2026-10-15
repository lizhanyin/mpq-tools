//! PKWARE Data Compression Library 的 "explode"（解压）。
//!
//! 忠实移植自 StormLib 所用的公开 PKWARE DCL 格式实现
//! （`pklib/explode.c`）。MPQ 归档以方法字节 0x08 存储 PKWARE 压缩的
//! 数据（或者在旧式 `IMPLODE` 标志下，完全不带方法字节）。

use crate::error::Error;

const CMP_BINARY: u8 = 0;
const CMP_ASCII: u8 = 1;

include!("pkware_tables.rs");

/// 构建用于长度码与距离码的规范码查找表。
fn gen_decode_tabs(start_indexes: &[u8], length_bits: &[u8], elements: usize) -> [u8; 0x100] {
    let mut positions = [0u8; 0x100];
    for i in 0..elements {
        let length = 1u32 << length_bits[i];
        let mut index = start_indexes[i] as u32;
        while index < 0x100 {
            positions[index as usize] = i as u8;
            index += length;
        }
    }
    positions
}

struct AscTables {
    /// 被 gen_asc_tabs 修改（缩减）过的 ch_bits
    ch_bits_asc: [u8; 0x100],
    /// 第 1 级（8 位）解码
    l1: [u8; 0x100],
    /// 第 2 级（额外 4 位）
    l2: [u8; 0x100],
    /// 第 3 级（额外 2 位）
    l3: [u8; 0x80],
    /// 第 4 级（额外 8 位以上）
    l4: [u8; 0x100],
}

fn gen_asc_tabs() -> AscTables {
    let mut t = AscTables {
        ch_bits_asc: CH_BITS_ASC,
        l1: [0; 0x100],
        l2: [0; 0x100],
        l3: [0; 0x80],
        l4: [0; 0x100],
    };

    // 从 0xFF 向 0 迭代各个符号。这些表初始化为零，
    // 与参考实现中清零的工作缓冲区完全一致。
    for count in (0..=0xFFusize).rev() {
        let code = CH_CODE_ASC[count];
        let bits_asc = t.ch_bits_asc[count];

        if bits_asc <= 8 {
            let add = 1u32 << bits_asc;
            let mut acc = code as u32;
            while acc < 0x100 {
                t.l1[acc as usize] = count as u8;
                acc += add;
            }
        } else if (code & 0xFF) != 0 {
            t.l1[(code & 0xFF) as usize] = 0xFF;
            if code & 0x3F != 0 {
                let bits = bits_asc - 4;
                t.ch_bits_asc[count] = bits;
                let add = 1u32 << bits;
                let mut acc = (code >> 4) as u32;
                while acc < 0x100 {
                    t.l2[acc as usize] = count as u8;
                    acc += add;
                }
            } else {
                let bits = bits_asc - 6;
                t.ch_bits_asc[count] = bits;
                let add = 1u32 << bits;
                let mut acc = (code >> 6) as u32;
                while acc < 0x80 {
                    t.l3[acc as usize] = count as u8;
                    acc += add;
                }
            }
        } else {
            let bits = bits_asc - 8;
            t.ch_bits_asc[count] = bits;
            let add = 1u32 << bits;
            let mut acc = (code >> 8) as u32;
            while acc < 0x100 {
                t.l4[acc as usize] = count as u8;
                acc += add;
            }
        }
    }
    t
}

/// 解压 PKWARE DCL 数据流。
///
/// `input` 是原始的 implode 流（不含方法字节）。最多 `max_out`
/// 个解压后的字节会被追加到 `out`。
///
/// 参考实现按 0x1000 字节为块冲刷输出环形缓冲区，因此可能在流结束
/// 标记之后输出尾随垃圾；而调用方总是知道真实的未压缩大小
/// （MPQ 中有存储），所以我们像 StormLib 的有界写回调一样
/// 对输出做精确截断。
pub fn explode(input: &[u8], out: &mut Vec<u8>, max_out: usize) -> crate::error::Result<()> {
    if input.len() <= 4 {
        return Err(Error::Corrupt("pkware stream too short"));
    }

    let ctype = input[0];
    let dsize_bits = input[1] as u32;
    if !(4..=6).contains(&dsize_bits) {
        return Err(Error::Corrupt("pkware invalid dictionary size"));
    }
    if ctype != CMP_BINARY && ctype != CMP_ASCII {
        return Err(Error::Corrupt("pkware invalid mode"));
    }

    let dsize_mask: u32 = 0xFFFF >> (16 - dsize_bits);

    let length_codes = gen_decode_tabs(&LEN_CODE, &LEN_BITS, 0x10);
    let dist_pos_codes = gen_decode_tabs(&DIST_CODE, &DIST_BITS, 0x40);
    let asc_tables = if ctype == CMP_ASCII {
        Some(gen_asc_tabs())
    } else {
        None
    };

    let mut ctx = ExplodeCtx {
        input,
        in_pos: 2, // 头部已消费；位缓冲区从第 2 字节开始加载
        bit_buff: 0,
        extra_bits: 0,
        dsize_bits,
        dsize_mask,
        ctype,
        length_codes,
        dist_pos_codes,
        asc: asc_tables.as_ref(),
    };

    // 像参考实现那样用 input[2] 预载 16 位位缓冲区：
    // bit_buff = in_buff[2]; extra_bits = 0; in_pos = 3。随后参考实现
    // 在 16 位缓冲区的*高*半部分继续读入后续字节。
    ctx.bit_buff = input[2] as u32;
    ctx.in_pos = 3;

    expand(&mut ctx, out, max_out)
}

struct ExplodeCtx<'a> {
    input: &'a [u8],
    in_pos: usize,
    /// 16 位位缓冲区；有效位是低 `8 + extra_bits` 位。
    bit_buff: u32,
    extra_bits: u32,
    dsize_bits: u32,
    dsize_mask: u32,
    ctype: u8,
    length_codes: [u8; 0x100],
    dist_pos_codes: [u8; 0x100],
    asc: Option<&'a AscTables>,
}

enum BitsResult {
    Ok,
    StreamEnd,
}

impl ExplodeCtx<'_> {
    /// 从位缓冲区中移除 `nbits` 位，必要时从输入补充。
    fn waste_bits(&mut self, nbits: u32) -> BitsResult {
        if nbits <= self.extra_bits {
            self.extra_bits -= nbits;
            self.bit_buff >>= nbits;
            return BitsResult::Ok;
        }

        self.bit_buff >>= self.extra_bits;
        if self.in_pos >= self.input.len() {
            return BitsResult::StreamEnd;
        }

        self.bit_buff |= (self.input[self.in_pos] as u32) << 8;
        self.in_pos += 1;
        self.bit_buff >>= nbits - self.extra_bits;
        // C 里按无符号回绕计算 (extra - nbits) + 8；调整顺序的写法
        // 避免了中间过程的下溢。
        self.extra_bits = (self.extra_bits + 8) - nbits;
        BitsResult::Ok
    }
}

/// `decode_lit` 的结果码。
const LIT_ERROR: u32 = 0x306;
const LIT_END: u32 = 0x305;

fn decode_lit(ctx: &mut ExplodeCtx) -> u32 {
    if ctx.bit_buff & 1 != 0 {
        if matches!(ctx.waste_bits(1), BitsResult::StreamEnd) {
            return LIT_ERROR;
        }

        let length_code = ctx.length_codes[(ctx.bit_buff & 0xFF) as usize] as usize;

        if matches!(ctx.waste_bits(LEN_BITS[length_code] as u32), BitsResult::StreamEnd) {
            return LIT_ERROR;
        }

        let extra_bits_count = EX_LEN_BITS[length_code] as u32;
        let mut code = length_code as u32;
        if extra_bits_count != 0 {
            let extra = ctx.bit_buff & ((1 << extra_bits_count) - 1);
            if matches!(ctx.waste_bits(extra_bits_count), BitsResult::StreamEnd)
                && (length_code as u32 + extra) != 0x10E {
                    return LIT_ERROR;
                }
            code = LEN_BASE[length_code] as u32 + extra;
        }

        return code + 0x100;
    }

    if matches!(ctx.waste_bits(1), BitsResult::StreamEnd) {
        return LIT_ERROR;
    }

    if ctx.ctype == CMP_BINARY {
        let b = ctx.bit_buff & 0xFF;
        if matches!(ctx.waste_bits(8), BitsResult::StreamEnd) {
            return LIT_ERROR;
        }
        return b;
    }

    // ASCII 模式
    let asc = ctx.asc.unwrap();
    
    let value = if ctx.bit_buff & 0xFF != 0 {
        let mut v = asc.l1[(ctx.bit_buff & 0xFF) as usize] as u32;
        if v == 0xFF {
            if ctx.bit_buff & 0x3F != 0 {
                if matches!(ctx.waste_bits(4), BitsResult::StreamEnd) {
                    return LIT_ERROR;
                }
                v = asc.l2[(ctx.bit_buff & 0xFF) as usize] as u32;
            } else {
                if matches!(ctx.waste_bits(6), BitsResult::StreamEnd) {
                    return LIT_ERROR;
                }
                v = asc.l3[(ctx.bit_buff & 0x7F) as usize] as u32;
            }
        }
        v as usize
    } else {
        if matches!(ctx.waste_bits(8), BitsResult::StreamEnd) {
            return LIT_ERROR;
        }
        asc.l4[(ctx.bit_buff & 0xFF) as usize] as usize
    };

    let bits = asc.ch_bits_asc[value];
    if matches!(ctx.waste_bits(bits as u32), BitsResult::StreamEnd) {
        return LIT_ERROR;
    }
    value as u32
}

fn decode_dist(ctx: &mut ExplodeCtx, rep_length: u32) -> u32 {
    let dist_pos_code = ctx.dist_pos_codes[(ctx.bit_buff & 0xFF) as usize] as u32;
    let dist_pos_bits = DIST_BITS[dist_pos_code as usize] as u32;
    if matches!(ctx.waste_bits(dist_pos_bits), BitsResult::StreamEnd) {
        return 0;
    }

    let distance = if rep_length == 2 {
        let d = (dist_pos_code << 2) | (ctx.bit_buff & 0x03);
        if matches!(ctx.waste_bits(2), BitsResult::StreamEnd) {
            return 0;
        }
        d
    } else {
        let d = (dist_pos_code << ctx.dsize_bits) | (ctx.bit_buff & ctx.dsize_mask);
        if matches!(ctx.waste_bits(ctx.dsize_bits), BitsResult::StreamEnd) {
            return 0;
        }
        d
    };
    distance + 1
}

fn expand(
    ctx: &mut ExplodeCtx,
    out: &mut Vec<u8>,
    max_out: usize,
) -> crate::error::Result<()> {
    // 0x2204 字节的环形缓冲区；解压从 0x1000 处开始写入，
    // 每当到达 0x2000 就冲刷出 0x1000 字节。
    let mut ring = vec![0u8; 0x2204];
    let mut output_pos: usize = 0x1000;
    let limit = out.len().saturating_add(max_out);

    fn flush(ring: &mut [u8], output_pos: &mut usize, out: &mut Vec<u8>, limit: usize) {
        let end = *output_pos;
        // 每次越过边界精确输出 0x1000 字节，与参考实现一致：
        // 越过边界的字节会继续留在环形缓冲区中待处理。
        let take = 0x1000.min(limit - out.len());
        out.extend_from_slice(&ring[0x1000..0x1000 + take]);
        let keep = end - 0x1000;
        ring.copy_within(0x1000..end, 0);
        *output_pos = keep;
    }

    loop {
        let literal = decode_lit(ctx);
        if literal >= LIT_END {
            if literal == LIT_END {
                break;
            }
            return Err(Error::Corrupt("pkware stream malformed"));
        }

        if literal >= 0x100 {
            let rep_length = literal - 0xFE;
            let minus_dist = decode_dist(ctx, rep_length);
            if std::env::var_os("MPQ_PKWARE_TRACE").is_some() {
                eprintln!("OP rep len={} dist={} abs={}", rep_length, minus_dist, out.len() + output_pos - 0x1000);
            }
            if minus_dist == 0 {
                return Err(Error::Corrupt("pkware stream malformed (distance)"));
            }
            if minus_dist as usize > output_pos {
                return Err(Error::Corrupt("pkware distance before start"));
            }

            let target = output_pos;
            let source = output_pos - minus_dist as usize;
            output_pos += rep_length as usize;
            // 逐字节复制：序列可能重叠（游程式的重复）。
            let mut t = target;
            let mut s = source;
            while t < output_pos {
                ring[t] = ring[s];
                t += 1;
                s += 1;
            }
        } else {
            if std::env::var_os("MPQ_PKWARE_TRACE").is_some() {
                eprintln!("OP lit {literal:#04x} abs={}", out.len() + output_pos - 0x1000);
            }
            ring[output_pos] = literal as u8;
            output_pos += 1;
        }

        if output_pos >= 0x2000 {
            flush(&mut ring, &mut output_pos, out, limit);
        }
    }

    // 最终冲刷：输出所有仍待处理的内容（而不是循环内冲刷
    // 使用的固定 0x1000 字节）。
    let end = output_pos;
    let take = (end - 0x1000).min(limit - out.len());
    out.extend_from_slice(&ring[0x1000..0x1000 + take]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_streams() {
        let mut out = Vec::new();
        assert!(explode(&[], &mut out, 64).is_err());
        assert!(explode(&[0, 9, 0, 0, 0], &mut out, 64).is_err()); // 非法字典大小
        assert!(explode(&[7, 4, 0, 0, 0], &mut out, 64).is_err()); // 非法模式
    }

    /// 测试向量来自 tests/fixtures：`pk_text.orig` 是已提交的原始文件；
    /// 各数据流（以及 `pk_rand`/`pk_runs` 这两对）来自
    /// examples/gen_pk_fixtures.rs 中移植的 implode 编码器，
    /// 它会重新生成这些向量，并逐一与本解码器做往返验证。
    #[test]
    fn explodes_reference_vectors() {
        // 注意：MPQ 工具从不产生 PKWARE ASCII 模式（ctype 1），
        // 参考的 pklib 也无法对该模式做往返转换，
        // 因此这里只测试二进制模式的向量。
        let vectors: [(&[u8], &[u8], &str); 3] = [
            (
                include_bytes!("../tests/fixtures/pk_text.bin"),
                include_bytes!("../tests/fixtures/pk_text.orig"),
                "binary text",
            ),
            (
                include_bytes!("../tests/fixtures/pk_rand.bin"),
                include_bytes!("../tests/fixtures/pk_rand.orig"),
                "random data",
            ),
            (
                include_bytes!("../tests/fixtures/pk_runs.bin"),
                include_bytes!("../tests/fixtures/pk_runs.orig"),
                "runs",
            ),
        ];
        for (stream, plain, note) in vectors {
            let mut out = Vec::new();
            explode(stream, &mut out, plain.len()).unwrap();
            assert_eq!(out.as_slice(), plain, "vector {note}");
        }
    }

    #[test]
    fn decode_tabs_match_reference_shape() {
        // 长度码表必须把符号 0（码 0x05，3 位）放在索引
        // 0x05、0x0D、……处 —— 抽查几个表项。
        let t = gen_decode_tabs(&LEN_CODE, &LEN_BITS, 0x10);
        assert_eq!(t[0x05], 0);
        assert_eq!(t[0x0D], 0);
        assert_eq!(t[0x15], 0);
        // 符号 1（码 0x03，2 位）
        assert_eq!(t[0x03], 1);
        assert_eq!(t[0x07], 1);
    }
}
