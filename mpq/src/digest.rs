//! 用于 v4 归档校验和的 MD5 辅助函数。
//!
//! v4 头部携带六个 MD5：五个针对哈希表/块表/hi-block/HET/BET 表的
//! *存储*（已加密、可能已压缩）字节，一个针对头部本身
//! （`0x00..0xC0` 字节）。相关语义已对照 StormLib 的
//! `LoadMpqTable` / `SaveMpqTable` / `SaveExtTable` 验证。

use md5::{Digest, Md5};

/// `data` 的 MD5 摘要。
pub(crate) fn md5_digest(data: &[u8]) -> [u8; 16] {
    let mut hasher = Md5::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// 对应 StormLib 的 `IsValidMD5`：全零的 MD5 表示“不存在”，
/// 因此写入时未带校验和的表（字段全为零）可直接通过验证。
pub(crate) fn md5_present(md5: &[u8; 16]) -> bool {
    md5.iter().any(|&b| b != 0)
}
