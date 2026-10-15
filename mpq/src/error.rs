//! MPQ 库的错误类型。

use std::fmt;

/// 整个 crate 通用的 Result 别名。
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// MPQ 库可能产生的所有错误。
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// 底层 I/O 失败。
    Io(std::io::Error),
    /// 在数据流中找不到 MPQ 头部。
    NotAnArchive,
    /// 头部声明了本库无法识别的格式版本。
    UnsupportedVersion(u16),
    /// 某个表或头部字段不一致/已损坏。
    Corrupt(&'static str),
    /// 某个表或头部字段不一致/已损坏（动态消息）。
    InvalidData(String),
    /// 在归档中找不到请求的文件。
    FileNotFound(String),
    /// 文件存在，但已被标记为删除。
    FileDeleted,
    /// 文件使用了不支持的压缩方法。
    UnsupportedCompression(u8),
    /// 文件已加密，且无法推导出可用的密钥。
    UnknownFileKey,
    /// 某个数据块未通过 MD5/CRC 校验。
    ChecksumMismatch(String),
    /// 归档受密码保护，但尚未提供密码
    /// （自定义 AEAD 扩展）。
    PasswordRequired,
    /// 提供的密码（或主密钥）未通过验证。
    PasswordMismatch,
    /// AEAD 认证失败——密钥错误，或数据已被篡改/损坏。
    Crypto(String),
    /// `(password)` 保险库使用了本库无法识别的方案版本。
    UnsupportedScheme(u16),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "i/o error: {e}"),
            Error::NotAnArchive => write!(f, "no MPQ archive header found in this file"),
            Error::UnsupportedVersion(v) => {
                write!(f, "unsupported MPQ format version field: {v}")
            }
            Error::Corrupt(what) => write!(f, "corrupt archive: {what}"),
            Error::InvalidData(msg) => write!(f, "invalid data: {msg}"),
            Error::FileNotFound(name) => write!(f, "file not found in archive: {name}"),
            Error::FileDeleted => write!(f, "file is marked as deleted"),
            Error::UnsupportedCompression(m) => {
                write!(f, "unsupported compression method 0x{m:02x}")
            }
            Error::UnknownFileKey => write!(f, "file is encrypted and the key could not be derived"),
            Error::ChecksumMismatch(msg) => write!(f, "checksum mismatch: {msg}"),
            Error::PasswordRequired => write!(
                f,
                "archive is password-protected; supply the password first"
            ),
            Error::PasswordMismatch => {
                write!(f, "password verification failed (wrong password?)")
            }
            Error::Crypto(msg) => write!(f, "crypto error: {msg}"),
            Error::UnsupportedScheme(v) => write!(
                f,
                "unsupported password-encryption scheme version: {v}"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
