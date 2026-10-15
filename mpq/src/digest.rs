//! MD5 helpers for the v4 archive checksums.
//!
//! v4 headers carry six MD5s: five over the *stored* (encrypted, possibly
//! compressed) bytes of the hash / block / hi-block / HET / BET tables and
//! one over the header itself (bytes `0x00..0xC0`). Semantics verified
//! against StormLib's `LoadMpqTable` / `SaveMpqTable` / `SaveExtTable`.

use md5::{Digest, Md5};

/// MD5 digest of `data`.
pub(crate) fn md5_digest(data: &[u8]) -> [u8; 16] {
    let mut hasher = Md5::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// StormLib's `IsValidMD5`: an all-zero MD5 means "not present", so a table
/// written without checksums (zeroed fields) verifies trivially.
pub(crate) fn md5_present(md5: &[u8; 16]) -> bool {
    md5.iter().any(|&b| b != 0)
}
