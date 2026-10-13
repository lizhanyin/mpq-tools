//! PKWARE Data Compression Library "explode" (decompression).
//!
//! Faithful port of the public PKWARE DCL format implementation used by
//! StormLib (`pklib/explode.c`). MPQ archives store PKWARE-compressed
//! data with method byte 0x08 (or, under the legacy `IMPLODE` flag,
//! without any method byte at all).

use crate::error::Error;

const CMP_BINARY: u8 = 0;
const CMP_ASCII: u8 = 1;

include!("pkware_tables.rs");

/// Build the canonical-code lookup table used for length and distance codes.
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
    /// ch_bits possibly modified (reduced) by gen_asc_tabs
    ch_bits_asc: [u8; 0x100],
    /// level 1 (8-bit) decode
    l1: [u8; 0x100],
    /// level 2 (4 extra bits)
    l2: [u8; 0x100],
    /// level 3 (2 extra bits)
    l3: [u8; 0x80],
    /// level 4 (8+ extra bits)
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

    // Iterate symbols from 0xFF down to 0. The tables are zero-initialized,
    // exactly like the reference implementation's zeroed work buffer.
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

/// Decompress a PKWARE DCL stream.
///
/// `input` is the raw implode stream (no method byte). Up to `max_out`
/// decompressed bytes are appended to `out`.
///
/// The reference implementation flushes its output ring in 0x1000-byte
/// chunks and can therefore emit trailing garbage after the end-of-stream
/// marker; callers always know the real uncompressed size (MPQ stores it),
/// so we cap the output exactly like StormLib's bounded write callback.
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
        in_pos: 2, // header consumed; bit buffer is loaded from byte 2
        bit_buff: 0,
        extra_bits: 0,
        dsize_bits,
        dsize_mask,
        ctype,
        length_codes,
        dist_pos_codes,
        asc: asc_tables.as_ref(),
    };

    // Prime the 16-bit bit buffer with input[2] like the reference does:
    // bit_buff = in_buff[2]; extra_bits = 0; in_pos = 3. The reference then
    // reads further bytes at the *top* half of the 16-bit buffer.
    ctx.bit_buff = input[2] as u32;
    ctx.in_pos = 3;

    expand(&mut ctx, out, max_out)
}

struct ExplodeCtx<'a> {
    input: &'a [u8],
    in_pos: usize,
    /// 16-bit bit buffer; valid bits are the low `8 + extra_bits` bits.
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
    /// Remove `nbits` from the bit buffer, refilling from the input.
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
        // C computes (extra - nbits) + 8 with unsigned wraparound; the
        // reordered form avoids the intermediate underflow.
        self.extra_bits = (self.extra_bits + 8) - nbits;
        BitsResult::Ok
    }
}

/// Result codes of `decode_lit`.
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

    // ASCII mode
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
    // Ring buffer of 0x2204 bytes; decompression starts writing at 0x1000,
    // flushes 0x1000 bytes whenever it reaches 0x2000.
    let mut ring = vec![0u8; 0x2204];
    let mut output_pos: usize = 0x1000;
    let limit = out.len().saturating_add(max_out);

    fn flush(ring: &mut [u8], output_pos: &mut usize, out: &mut Vec<u8>, limit: usize) {
        let end = *output_pos;
        // Exactly 0x1000 bytes per crossing, like the reference: bytes
        // produced past the boundary stay pending in the ring buffer.
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
            // Byte-by-byte: sequences may overlap (run-length style).
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

    // Final flush: emit whatever is still pending (not the fixed 0x1000
    // the in-loop flush uses).
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
        assert!(explode(&[0, 9, 0, 0, 0], &mut out, 64).is_err()); // bad dict size
        assert!(explode(&[7, 4, 0, 0, 0], &mut out, 64).is_err()); // bad mode
    }

    /// Vectors produced by compiling Blizzard's actual PKWARE DCL code
    /// (StormLib pklib/implode.c). See tests/fixtures.
    #[test]
    fn explodes_reference_vectors() {
        // Note: PKWARE ASCII mode (ctype 1) is never produced by MPQ
        // tools and does not round-trip in the reference pklib either,
        // so only binary-mode vectors are tested.
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
        // The length-code table must place symbol 0 (code 0x05, 3 bits) at
        // indexes 0x05, 0x0D, .. — spot check a few entries.
        let t = gen_decode_tabs(&LEN_CODE, &LEN_BITS, 0x10);
        assert_eq!(t[0x05], 0);
        assert_eq!(t[0x0D], 0);
        assert_eq!(t[0x15], 0);
        // symbol 1 (code 0x03, 2 bits)
        assert_eq!(t[0x03], 1);
        assert_eq!(t[0x07], 1);
    }
}
