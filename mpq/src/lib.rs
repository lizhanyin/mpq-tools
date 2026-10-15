//! # mpq — Mo'PaQ 归档库
//!
//! 读取遵循 v3 协议（v1/v2/v3 头部）的暴雪 MPQ 归档，并完整支持 v4：v4
//! 头部会连同全部字段一起解析，读取时会校验六个 v4 MD5 校验和（五张表 +
//! 头部本身）以及每个文件的原始块 MD5（不匹配只告警，绝不阻止打开），
//! 且 v4 容器可以写回 —— [`modify`] 保持 v4 版本并重算所有 MD5，
//! [`create`] / [`convert`] 也接受 v4 目标。可以按指定版本创建新的空归档
//! （[`create`]），也可以在 v1–v4 之间转换既有归档（[`convert`]）。
//!
//! 功能：
//! * 经典 hash table / block table / hi-block 表（加密，可能压缩）
//! * v3/v4 归档的 HET + BET 表
//! * v4 MD5 校验（各表、头部、文件原始块）
//! * `(listfile)`、`(attributes)` —— 两者皆为可选
//! * sector 与单单元（single-unit）文件，加密（含 key-v2），
//!   zlib / PKWARE / bzip2 / LZMA 解压
//! * `MPQ\x1B` 用户数据包装（Warcraft III / StarCraft II 地图），以及
//!   通过 0x200 对齐扫描检测内嵌归档
//! * 修改：[`modify`] 添加 / 替换、删除文件，同时逐字节原样复制
//!   每个保留文件的已存储数据
//! * 密码保护（自定义 v4 扩展，见 [`protect`]）：每个文件的各 sector 都用
//!   ChaCha20-Poly1305 封装，封装密钥为按归档生成的主密钥，经
//!   Argon2id(password) 包装后存放在 `(password)` 保险库中；
//!   [`unprotect`] / [`change_password`] 可撤销或重新包装。只有本库
//!   能打开受保护的归档（第三方工具无法打开）。
//!
//! 本库刻意保持宽容：任何非必需的内容（listfile、attributes、HET/BET、
//! hi-block 表、不匹配的归档大小）缺失或损坏都不影响 —— 归档仍能打开，
//! 读取一切可读的部分，警告可通过 [`Archive::warnings`] 获取。
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
mod create;
mod crypt;
mod digest;
mod error;
mod header;
mod hetbet;
mod jenkins;
mod modify;
mod passcrypt;
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
pub use passcrypt::{Argon2Params, PASSWORD_FILE_NAME};
pub use tables::{
    BlockEntry, ClassicTables, HashEntry, FILE_COMPRESS, FILE_COMPRESS_MASK, FILE_DELETE_MARKER,
    FILE_ENCRYPTED, FILE_EXISTS, FILE_IMPLODE, FILE_KEY_V2, FILE_PASSWORD_AEAD, FILE_PATCH,
    FILE_SECTOR_CRC, FILE_SIGNATURE, FILE_SINGLE_UNIT,
};

pub use crate::error::{Error, Result};

/// 解压辅助函数，供高级用户 / 工具使用。
pub use compression::{decompress_imploded, decompress_masked};

/// 头部诊断与修复。
pub use repair::{diagnose, locate_header, repair, RepairIssue, RepairReport};

/// 归档修改：导入（添加 / 替换）与删除文件，以及容器版本转换。
pub use modify::{
    change_password, convert, modify, modify_with, protect, unprotect, Deletion, ModifyOptions,
    ModifyOutcome, ModifyReport, PasswordProtect,
};

/// 创建新的空归档。
pub use create::{create, CreateOptions};
