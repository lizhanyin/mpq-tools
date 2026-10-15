//! Jenkins `lookup3` `hashlittle2` —— MPQ v3+ 归档的 HET 与 BET 表
//! 所使用的 64 位名称哈希。
//!
//! 移植自 Bob Jenkins 的公有领域代码 `lookup3.c`。这里只需要可移植的
//! 逐字节路径：原实现中所有路径产生的结果完全相同。

use crate::crypt::lower_byte;

#[inline(always)]
fn rot(x: u32, k: u32) -> u32 {
    x.rotate_left(k)
}

#[inline(always)]
fn mix(a: &mut u32, b: &mut u32, c: &mut u32) {
    *a = a.wrapping_sub(*c);
    *a ^= rot(*c, 4);
    *c = c.wrapping_add(*b);
    *b = b.wrapping_sub(*a);
    *b ^= rot(*a, 6);
    *a = a.wrapping_add(*c);
    *c = c.wrapping_sub(*b);
    *c ^= rot(*b, 8);
    *b = b.wrapping_add(*a);
    *a = a.wrapping_sub(*c);
    *a ^= rot(*c, 16);
    *c = c.wrapping_add(*b);
    *b = b.wrapping_sub(*a);
    *b ^= rot(*a, 19);
    *a = a.wrapping_add(*c);
    *c = c.wrapping_sub(*b);
    *c ^= rot(*b, 4);
    *b = b.wrapping_add(*a);
}

#[inline(always)]
fn final_mix(a: &mut u32, b: &mut u32, c: &mut u32) {
    *c ^= *b;
    *c = c.wrapping_sub(rot(*b, 14));
    *a ^= *c;
    *a = a.wrapping_sub(rot(*c, 11));
    *b ^= *a;
    *b = b.wrapping_sub(rot(*a, 25));
    *c ^= *b;
    *c = c.wrapping_sub(rot(*b, 16));
    *a ^= *c;
    *a = a.wrapping_sub(rot(*c, 4));
    *b ^= *a;
    *b = b.wrapping_sub(rot(*a, 14));
    *c ^= *b;
    *c = c.wrapping_sub(rot(*b, 24));
}

/// `hashlittle2(key, len, pc, pb)`；返回最终值 `(c, b)`。
fn hashlittle2(key: &[u8], pc: u32, pb: u32) -> (u32, u32) {
    let mut a: u32;
    let mut b: u32;
    let mut c: u32;

    // a = b = c = 0xdeadbeef + 长度 + pc; c += pb
    let init = 0xdead_beef_u32.wrapping_add(key.len() as u32).wrapping_add(pc);
    a = init;
    b = init;
    c = init.wrapping_add(pb);

    let mut k = key;
    while k.len() > 12 {
        a = a
            .wrapping_add(u32::from_le_bytes([k[0], k[1], k[2], k[3]]));
        b = b
            .wrapping_add(u32::from_le_bytes([k[4], k[5], k[6], k[7]]));
        c = c
            .wrapping_add(u32::from_le_bytes([k[8], k[9], k[10], k[11]]));
        mix(&mut a, &mut b, &mut c);
        k = &k[12..];
    }

    // 最后一个（可能不完整的）块，各种情况依次向下执行。
    let n = k.len();
    if n == 0 {
        return (c, b);
    }
    if n >= 12 {
        c = c.wrapping_add((k[11] as u32) << 24);
    }
    if n >= 11 {
        c = c.wrapping_add((k[10] as u32) << 16);
    }
    if n >= 10 {
        c = c.wrapping_add((k[9] as u32) << 8);
    }
    if n >= 9 {
        c = c.wrapping_add(k[8] as u32);
    }
    if n >= 8 {
        b = b.wrapping_add((k[7] as u32) << 24);
    }
    if n >= 7 {
        b = b.wrapping_add((k[6] as u32) << 16);
    }
    if n >= 6 {
        b = b.wrapping_add((k[5] as u32) << 8);
    }
    if n >= 5 {
        b = b.wrapping_add(k[4] as u32);
    }
    if n >= 4 {
        a = a.wrapping_add((k[3] as u32) << 24);
    }
    if n >= 3 {
        a = a.wrapping_add((k[2] as u32) << 16);
    }
    if n >= 2 {
        a = a.wrapping_add((k[1] as u32) << 8);
    }
    // 走到这里必然有 n >= 1
    a = a.wrapping_add(k[0] as u32);

    final_mix(&mut a, &mut b, &mut c);
    (c, b)
}

/// HET/BET 表使用的 64 位文件名哈希。
///
/// 文件名先经 storm 小写表归一化
/// （ASCII 大写转小写、`/` 转 `\`），再以 `pc = 2`、`pb = 1`
/// 为种子用 `hashlittle2` 计算哈希。
/// 结果：`(b << 32) | c`。
pub fn file_name_hash(name: &str) -> u64 {
    let normalized: Vec<u8> = name.bytes().map(lower_byte).collect();
    let (c, b) = hashlittle2(&normalized, 2, 1);
    ((b as u64) << 32) | c as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    // 参考值的产生方式：编译 Bob Jenkins 的原始 lookup3.c，并以与
    // StormLib 的 HashStringJenkins 完全一致的方式驱动 hashlittle2
    // （小写表，pc=2，pb=1）。
    #[test]
    fn jenkins_reference_values() {
        assert_eq!(file_name_hash("(listfile)"), 0x3bc43bb0b2f3866a);
        assert_eq!(file_name_hash(""), 0xdeadbef1deadbef2);
        assert_eq!(file_name_hash("abc"), 0xcbc05d12b47025a5);
        assert_eq!(file_name_hash("war3map.j"), 0x5252117e4beda79d);
        assert_eq!(file_name_hash("exactly12chr"), 0x97b18367a6e9e2a8);
        assert_eq!(file_name_hash("exactly13chrs"), 0x4e7480ccc6359c94);
    }

    #[test]
    fn slash_and_case_normalization() {
        assert_eq!(file_name_hash("ABC\\DEF"), file_name_hash("abc/def"));
    }
}
