//! # mpq — Mo'PaQ archive reading library
//!
//! Reads Blizzard MPQ archives up to and including the v3 protocol
//! (v1/v2/v3 headers; v4 headers parse and open through their classic
//! tables, with the v4-only MD5/HET extras ignored).
//!
//! Features:
//! * classic hash / block / hi-block tables (encrypted, possibly compressed)
//! * HET + BET tables of v3 archives
//! * `(listfile)`, `(attributes)` — both optional
//! * sector and single-unit files, encryption incl. key-v2,
//!   zlib / PKWARE / bzip2 / LZMA decompression
//! * `MPQ\x1B` user-data wrappers (Warcraft III / StarCraft II maps) and
//!   embedded archive detection via 0x200-aligned scanning
//!
//! The library is deliberately tolerant: anything non-essential (listfile,
//! attributes, HET/BET, hi-block table, mismatched archive size) may be
//! missing or damaged — the archive still opens and reads whatever is
//! readable, with warnings available via [`Archive::warnings`].
//!
//! ```
//! use mpq::Archive;
//!
//! let archive = Archive::open("tests/fixtures/mini_v1.mpq").unwrap();
//! let names: Vec<&str> = archive.file_names().iter().map(|s| s.as_str()).collect();
//! assert!(names.contains(&"war3map.j"));
//! let data = archive.read_file("war3map.j").unwrap();
//! assert_eq!(data, b"// dummy map script\n");
//! ```

mod archive;
mod attrs;
mod compression;
mod crypt;
mod error;
mod header;
mod hetbet;
mod jenkins;
mod pkware;
mod repair;
mod tables;

pub use archive::{Archive, FileEntry, TableSource, SPECIAL_FILES};
pub use attrs::{Attributes, FileAttributes};
pub use crypt::{
    decrypt_bytes, encrypt_bytes, file_key, hash_string, HASH_FILE_KEY, HASH_NAME_A,
    HASH_NAME_B, HASH_TABLE_INDEX,
};
pub use header::{FormatVersion, MpqHeader, UserDataHeader, MPQ_MAGIC, USERDATA_MAGIC};
pub use jenkins::file_name_hash;
pub use hetbet::{BetTable, HetTable};
pub use tables::{
    BlockEntry, ClassicTables, HashEntry, FILE_COMPRESS, FILE_COMPRESS_MASK, FILE_DELETE_MARKER,
    FILE_ENCRYPTED, FILE_EXISTS, FILE_IMPLODE, FILE_KEY_V2, FILE_PATCH, FILE_SECTOR_CRC,
    FILE_SIGNATURE, FILE_SINGLE_UNIT,
};

pub use crate::error::{Error, Result};

/// Decompress helpers exposed for advanced users / tooling.
pub use compression::{decompress_imploded, decompress_masked};

/// Header diagnosis and repair.
pub use repair::{diagnose, locate_header, repair, RepairIssue, RepairReport};
