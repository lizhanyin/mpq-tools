//! 基于密码的归档加密（设计文档中的方案②）：一种自定义的 MPQ v4 扩展，
//! 文件内容按 sector 使用 ChaCha20-Poly1305 AEAD 封装，所用密钥由每个
//! 归档唯一的主密钥（master key）派生，而主密钥本身再用
//! Argon2id(password) 包裹。
//!
//! * 只要存在内部文件 [`PASSWORD_FILE_NAME`]，该归档即被视为受密码保护
//!   （仅凭 block 标志并不可靠 —— BET 表只编码了标志位的一个子集）。
//! * vault 是一条固定大小为 [`VAULT_LEN`] 的记录，以普通的、未压缩的
//!   single-unit 文件形式存储，因此修改密码只需原地重写这几个字节 ——
//!   无需重新加密任何数据。
//! * 每个文件的密钥来自 HKDF-SHA256(master, file index)；每个 sector 的
//!   nonce 与 AAD 由 (file index, sector index) 派生，既保留了随机访问
//!   能力，又将每个 sector 绑定到其所在位置。

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;

use crate::error::{Error, Result};
use crate::tables::{FILE_COMPRESS, FILE_ENCRYPTED, FILE_PASSWORD_AEAD, FILE_SINGLE_UNIT};

/// 存放密码 vault 的内部文件名。
pub const PASSWORD_FILE_NAME: &str = "(password)";

/// 自定义 block 标志：该文件的 sector 使用密码 AEAD 方案，
/// 而非 storm 加密（同时也会设置 `FILE_ENCRYPTED`）。
pub(crate) const FLAG: u32 = FILE_PASSWORD_AEAD;

/// 追加在每个已封装 sector 末尾的 AEAD tag 长度。
pub const TAG_LEN: usize = 16;

/// vault 记录恰好为 128 字节：
///
/// ```text
/// 0x00 u32   magic "MPwd"
/// 0x04 u16   方案版本 (1)
/// 0x06 u8    加密算法 id (1 = ChaCha20-Poly1305)
/// 0x07 u8    KDF id (1 = Argon2id)
/// 0x08 u32   argon2 m_cost (KiB)
/// 0x0C u32   argon2 t_cost
/// 0x10 u32   argon2 p_cost
/// 0x14 [16]  argon2 salt
/// 0x24 [32]  master key XOR Argon2id(password, salt)
/// 0x34 [12]  verifier nonce
/// 0x40 [48]  verifier 密文+tag（同一 master 派生密钥下的
///            已知明文 —— 即密码校验）
/// ```
pub const VAULT_LEN: usize = 128;

const MAGIC: u32 = 0x6477_504D; // "MPwd"
const SCHEME_VERSION: u16 = 1;
const CIPHER_CHACHA20_POLY1305: u8 = 1;
const KDF_ARGON2ID: u8 = 1;

/// verifier 封装的已知明文（填充到 32 字节）。
const VERIFIER_PLAIN: [u8; 32] = *b"MPQ-PWD-AEAD-VERIFIER-V1\0\0\0\0\0\0\0\0";

/// 固定的 HKDF salt / info 字符串，用于各次派生之间的域分隔。
const HKDF_SALT: &[u8] = b"mpq-tools/password-aead/v1";
const HKDF_INFO_VERIFIER: &[u8] = b"verifier";

/// Argon2id 成本参数（m 以 KiB 计）。默认值遵循 OWASP 建议
/// （19 MiB、2 次迭代）；测试时调用方可以调低。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Argon2Params {
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl Default for Argon2Params {
    fn default() -> Self {
        Argon2Params {
            m_cost: 19_456,
            t_cost: 2,
            p_cost: 1,
        }
    }
}

impl Argon2Params {
    pub fn new(m_cost: u32, t_cost: u32, p_cost: u32) -> Result<Self> {
        // 借助 argon2 本身做校验，让边界检查只存在于这一处。
        Params::new(m_cost, t_cost, p_cost, Some(32))
            .map_err(|e| Error::InvalidData(format!("无效的 Argon2 参数：{e}")))?;
        Ok(Argon2Params {
            m_cost,
            t_cost,
            p_cost,
        })
    }
}

/// 解析后的 `(password)` vault。
#[derive(Debug, Clone)]
pub struct Vault {
    pub params: Argon2Params,
    salt: [u8; 16],
    wrapped_master: [u8; 32],
    verifier_nonce: [u8; 12],
    verifier_ct: [u8; 48],
}

fn random(bytes: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf)
        .map_err(|e| Error::InvalidData(format!("无法获取随机数：{e}")))?;
    Ok(buf)
}

/// Argon2id(password, salt, params) → 32 字节的密钥加密密钥（KEK）。
fn argon2_kek(password: &[u8], params: &Argon2Params, salt: &[u8; 16]) -> Result<[u8; 32]> {
    let params =
        Params::new(params.m_cost, params.t_cost, params.p_cost, Some(32)).map_err(|e| {
            Error::InvalidData(format!("无效的 Argon2 参数（存于 (password) 文件）：{e}"))
        })?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; 32];
    argon
        .hash_password_into(password, salt, &mut out)
        .map_err(|e| Error::InvalidData(format!("Argon2id 计算失败：{e}")))?;
    Ok(out)
}

/// 对 master key 执行 HKDF-SHA256 扩展，附带 `info`。
fn hkdf_expand(master: &[u8; 32], info: &[u8]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(HKDF_SALT), master);
    let mut okm = [0u8; 32];
    hk.expand(info, &mut okm)
        .expect("32-byte output is valid for SHA-256");
    okm
}

/// 每个文件的 AEAD 密钥：HKDF(master, "file-key" || 大端序 index)。
pub fn derive_file_key(master: &[u8; 32], file_index: usize) -> [u8; 32] {
    let mut info = b"file-key".to_vec();
    info.extend_from_slice(&(file_index as u32).to_be_bytes());
    hkdf_expand(master, &info)
}

/// 由 (file index, sector index) 确定性生成的 12 字节 nonce。
fn nonce_of(file_index: usize, sector: u32) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..4].copy_from_slice(&(file_index as u32).to_be_bytes());
    n[4..8].copy_from_slice(&sector.to_be_bytes());
    n[8..].copy_from_slice(b"MPQ1");
    n
}

/// 将 sector 绑定到其 (file index, sector index) 位置的 AAD。
fn aad_of(file_index: usize, sector: u32) -> Vec<u8> {
    let mut aad = b"mpqaead".to_vec();
    aad.extend_from_slice(&(file_index as u32).to_be_bytes());
    aad.extend_from_slice(&sector.to_be_bytes());
    aad
}

/// 封装单个 sector（或整个 single-unit 载荷，此时 `sector = 0`）。
/// 返回 明文 || 16 字节 tag。
pub(crate) fn seal_sector(
    key: &[u8; 32],
    file_index: usize,
    sector: u32,
    plain: &[u8],
) -> Vec<u8> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .encrypt(
            Nonce::from_slice(&nonce_of(file_index, sector)),
            Payload {
                msg: plain,
                aad: &aad_of(file_index, sector),
            },
        )
        .expect("AEAD seal into a fresh buffer cannot fail")
}

/// 解封单个 sector；任何密钥错误、字节被篡改或 sector 错位，
/// 都会在这一步的 tag 校验中失败。
pub(crate) fn open_sector(
    key: &[u8; 32],
    file_index: usize,
    sector: u32,
    ct: &[u8],
) -> Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(
            Nonce::from_slice(&nonce_of(file_index, sector)),
            Payload {
                msg: ct,
                aad: &aad_of(file_index, sector),
            },
        )
        .map_err(|_| {
            Error::Crypto(format!(
                "sector authentication failed (file index {file_index}, sector {sector}) — \
                 密码错误或数据被改动"
            ))
        })
}

/// 封装 vault 的 verifier 常量（一次性 AEAD，使用保存下来的随机
/// nonce；每个归档的 key 与 nonce 都是全新生成的）。
fn seal_verifier(key: &[u8; 32], nonce: &[u8; 12], plain: &[u8]) -> Vec<u8> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plain,
                aad: b"mpqaead-verifier",
            },
        )
        .expect("AEAD seal into a fresh buffer cannot fail")
}

/// [`seal_verifier`] 的 verifier 侧对应操作；`true` 表示密钥正确。
fn open_verifier(key: &[u8; 32], nonce: &[u8; 12], ct: &[u8]) -> bool {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ct,
                aad: b"mpqaead-verifier",
            },
        )
        .is_ok()
}

impl Vault {
    /// 生成一个全新的 vault：随机的 master key、salt 和 verifier nonce。
    /// 返回序列化后的记录以及 master key。
    pub fn generate(password: &[u8], params: &Argon2Params) -> Result<(Vault, [u8; 32])> {
        let master: [u8; 32] = random(32)?.try_into().expect("32 bytes");
        let salt: [u8; 16] = random(16)?.try_into().expect("16 bytes");
        let verifier_nonce: [u8; 12] = random(12)?.try_into().expect("12 bytes");
        let vault = Self::with_master(password, params, &master, salt, verifier_nonce)?;
        Ok((vault, master))
    }

    /// 用 `password` 包裹 `master`（修改密码时也用它重新包裹已有的
    /// master key —— 数据本身保持不动）。
    fn with_master(
        password: &[u8],
        params: &Argon2Params,
        master: &[u8; 32],
        salt: [u8; 16],
        verifier_nonce: [u8; 12],
    ) -> Result<Vault> {
        let kek = argon2_kek(password, params, &salt)?;
        let mut wrapped_master = *master;
        for (w, k) in wrapped_master.iter_mut().zip(kek) {
            *w ^= k;
        }
        // verifier 是用 master 派生密钥封装的已知常量：即打开归档时的
        // 密码校验（解包出错 → 密钥错误 → 在这里 tag 校验失败，
        // 而不是之后读到乱码数据）。
        let verifier_key = hkdf_expand(master, HKDF_INFO_VERIFIER);
        let verifier_ct = seal_verifier(&verifier_key, &verifier_nonce, &VERIFIER_PLAIN)
            .try_into()
            .expect("48-byte sealed verifier");
        Ok(Vault {
            params: *params,
            salt,
            wrapped_master,
            verifier_nonce,
            verifier_ct,
        })
    }

    /// 用新密码和全新的 salt 重新包裹已有的 master key
    /// （供 [`crate::change_password`] 使用）。
    pub fn rewindrap(password: &[u8], params: &Argon2Params, master: &[u8; 32]) -> Result<Vault> {
        let salt: [u8; 16] = random(16)?.try_into().expect("16 bytes");
        let verifier_nonce: [u8; 12] = random(12)?.try_into().expect("12 bytes");
        Self::with_master(password, params, master, salt, verifier_nonce)
    }

    pub fn serialize(&self) -> [u8; VAULT_LEN] {
        let mut out = [0u8; VAULT_LEN];
        out[..4].copy_from_slice(&MAGIC.to_le_bytes());
        out[4..6].copy_from_slice(&SCHEME_VERSION.to_le_bytes());
        out[6] = CIPHER_CHACHA20_POLY1305;
        out[7] = KDF_ARGON2ID;
        out[8..12].copy_from_slice(&self.params.m_cost.to_le_bytes());
        out[12..16].copy_from_slice(&self.params.t_cost.to_le_bytes());
        out[16..20].copy_from_slice(&self.params.p_cost.to_le_bytes());
        out[20..36].copy_from_slice(&self.salt);
        out[36..68].copy_from_slice(&self.wrapped_master);
        out[68..80].copy_from_slice(&self.verifier_nonce);
        out[80..128].copy_from_slice(&self.verifier_ct);
        out
    }

    /// 从 `(password)` 文件内容中解析 vault（允许末尾有多余字节）。
    pub fn parse(bytes: &[u8]) -> Result<Vault> {
        let bad = |what: &str| Error::InvalidData(format!("(password) 文件损坏：{what}"));
        if bytes.len() < VAULT_LEN {
            return Err(bad("长度不足"));
        }
        let b = &bytes[..VAULT_LEN];
        if u32::from_le_bytes(b[0..4].try_into().unwrap()) != MAGIC {
            return Err(bad("magic 不匹配"));
        }
        match u16::from_le_bytes(b[4..6].try_into().unwrap()) {
            SCHEME_VERSION => {}
            v => return Err(Error::UnsupportedScheme(v)),
        }
        if b[6] != CIPHER_CHACHA20_POLY1305 || b[7] != KDF_ARGON2ID {
            return Err(bad("未知的加密/KDF 标识"));
        }
        let params = Argon2Params {
            m_cost: u32::from_le_bytes(b[8..12].try_into().unwrap()),
            t_cost: u32::from_le_bytes(b[12..16].try_into().unwrap()),
            p_cost: u32::from_le_bytes(b[16..20].try_into().unwrap()),
        };
        if Params::new(params.m_cost, params.t_cost, params.p_cost, Some(32)).is_err() {
            return Err(bad("Argon2 参数非法"));
        }
        Ok(Vault {
            params,
            salt: b[20..36].try_into().unwrap(),
            wrapped_master: b[36..68].try_into().unwrap(),
            verifier_nonce: b[68..80].try_into().unwrap(),
            verifier_ct: b[80..128].try_into().unwrap(),
        })
    }

    /// `master` 是否为该归档的 master key（通过 verifier 校验）。
    pub fn verify_master(&self, master: &[u8; 32]) -> bool {
        let key = hkdf_expand(master, HKDF_INFO_VERIFIER);
        open_verifier(&key, &self.verifier_nonce, &self.verifier_ct)
    }

    /// 用密码解出 master key；密码错误会在这里的 verifier 校验中失败
    /// （绝不会静默返回错误的密钥）。
    pub fn open(&self, password: &[u8]) -> Result<[u8; 32]> {
        let kek = argon2_kek(password, &self.params, &self.salt)?;
        let mut master = self.wrapped_master;
        for (m, k) in master.iter_mut().zip(kek) {
            *m ^= k;
        }
        if !self.verify_master(&master) {
            return Err(Error::PasswordMismatch);
        }
        Ok(master)
    }
}

/// 将内容编码为密码 AEAD 存储块，与经典的 [`crate::modify`] 全新编码
/// 保持一致，但对每个 sector 都进行封装：
///
/// * 空文件：纯空的 single unit（无需封装）
/// * 小于一个 sector：single unit，封装一次
/// * 其他情况：明文（未加密）的 sector 偏移表 —— 始终存在，
///   即使没有任何内容被压缩，因为封装后的 sector 是
///   变长的（明文 + tag）
///
/// 读取侧的对应逻辑：sector 解压判定使用*解密后*的
/// 长度（`plain < expected`），与经典文件的启发式相同。
pub(crate) fn encode_fresh_aead(
    data: &[u8],
    sector_size: u32,
    file_index: usize,
    master: &[u8; 32],
) -> (Vec<u8>, u32) {
    let key = derive_file_key(master, file_index);
    let fsize = data.len() as u32;
    let flags = FLAG | FILE_ENCRYPTED;

    if fsize == 0 {
        return (Vec::new(), FILE_SINGLE_UNIT);
    }

    if fsize < sector_size {
        let z = crate::compression::zlib_compress(data);
        let compressed = z.len() + 1 < data.len();
        let payload = if compressed {
            let mut b = Vec::with_capacity(z.len() + 1);
            b.push(crate::compression::M_ZLIB);
            b.extend_from_slice(&z);
            b
        } else {
            data.to_vec()
        };
        let sealed = seal_sector(&key, file_index, 0, &payload);
        let mut f = flags | FILE_SINGLE_UNIT;
        if compressed {
            f |= FILE_COMPRESS;
        }
        return (sealed, f);
    }

    let nsec = fsize.div_ceil(sector_size) as usize;
    let mut sectors: Vec<Vec<u8>> = Vec::with_capacity(nsec);
    for chunk in data.chunks(sector_size as usize) {
        let z = crate::compression::zlib_compress(chunk);
        if z.len() + 1 < chunk.len() {
            let mut s = Vec::with_capacity(z.len() + 1);
            s.push(crate::compression::M_ZLIB);
            s.extend_from_slice(&z);
            sectors.push(s);
        } else {
            sectors.push(chunk.to_vec());
        }
    }

    let table_len = (nsec + 1) * 4;
    let mut block = Vec::with_capacity(table_len + sectors.iter().map(|s| s.len() + TAG_LEN).sum::<usize>());
    let mut acc = table_len as u32;
    block.extend_from_slice(&acc.to_le_bytes());
    for s in &sectors {
        acc += (s.len() + TAG_LEN) as u32;
        block.extend_from_slice(&acc.to_le_bytes());
    }
    for (i, s) in sectors.iter().enumerate() {
        block.extend_from_slice(&seal_sector(&key, file_index, i as u32, s));
    }
    (block, flags | FILE_COMPRESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weak() -> Argon2Params {
        Argon2Params::new(64, 1, 1).unwrap()
    }

    #[test]
    fn vault_roundtrip_and_wrong_password() {
        let (vault, master) = Vault::generate(b"secret", &weak()).unwrap();
        let bytes = vault.serialize();
        assert_eq!(bytes.len(), VAULT_LEN);

        let parsed = Vault::parse(&bytes).unwrap();
        assert_eq!(parsed.open(b"secret").unwrap(), master);
        assert!(matches!(parsed.open(b"wrong"), Err(Error::PasswordMismatch)));
        assert!(parsed.verify_master(&master));
        let mut other = master;
        other[0] ^= 1;
        assert!(!parsed.verify_master(&other));
    }

    #[test]
    fn vault_rewrap_keeps_master() {
        let (vault, master) = Vault::generate(b"old", &weak()).unwrap();
        let rewrapped = Vault::rewindrap(b"new", &weak(), &master).unwrap();
        assert_eq!(rewrapped.open(b"new").unwrap(), master);
        assert!(rewrapped.open(b"old").is_err());
        assert_ne!(vault.serialize()[20..36], rewrapped.serialize()[20..36]); // 新 salt
    }

    #[test]
    fn vault_parse_rejects_garbage() {
        assert!(Vault::parse(&[0u8; VAULT_LEN]).is_err());
        let (vault, _) = Vault::generate(b"x", &weak()).unwrap();
        let mut bytes = vault.serialize();
        bytes[4] = 9; // 未知方案版本
        assert!(matches!(Vault::parse(&bytes), Err(Error::UnsupportedScheme(_))));
    }

    #[test]
    fn file_keys_are_distinct_and_deterministic() {
        let master = [7u8; 32];
        assert_eq!(derive_file_key(&master, 3), derive_file_key(&master, 3));
        assert_ne!(derive_file_key(&master, 3), derive_file_key(&master, 4));
    }

    #[test]
    fn sector_seal_open_and_tamper() {
        let key = derive_file_key(&[1u8; 32], 5);
        let ct = seal_sector(&key, 5, 2, b"hello sector");
        assert_eq!(ct.len(), 12 + TAG_LEN);
        assert_eq!(open_sector(&key, 5, 2, &ct).unwrap(), b"hello sector");

        // 位置错误（AAD/nonce 绑定）：错位的 sector 会失败。
        assert!(open_sector(&key, 5, 3, &ct).is_err());
        assert!(open_sector(&key, 6, 2, &ct).is_err());
        // 密钥错误与数据篡改都会导致 tag 校验失败。
        assert!(open_sector(&derive_file_key(&[2u8; 32], 5), 5, 2, &ct).is_err());
        let mut tampered = ct.clone();
        tampered[0] ^= 0x80;
        assert!(open_sector(&key, 5, 2, &tampered).is_err());
    }

    #[test]
    fn encode_fresh_aead_layout() {
        let master = [9u8; 32];
        // 小于一个 sector 的 single unit（"tiny" 只有 4 字节，不可压缩，
        // 因此封装时不带 compress 标志）。
        let (bytes, flags) = encode_fresh_aead(b"tiny", 4096, 0, &master);
        assert_eq!(flags, FLAG | FILE_ENCRYPTED | FILE_SINGLE_UNIT);
        let key = derive_file_key(&master, 0);
        let plain = open_sector(&key, 0, 0, &bytes).unwrap();
        assert_eq!(plain, b"tiny");

        // 多 sector 加偏移表；做解压往返验证。
        let data: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let (bytes, flags) = encode_fresh_aead(&data, 4096, 1, &master);
        assert_eq!(flags, FLAG | FILE_ENCRYPTED | FILE_COMPRESS);
        let key = derive_file_key(&master, 1);
        let nsec = 10_000usize.div_ceil(4096);
        let offsets: Vec<u32> = bytes[..(nsec + 1) * 4]
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(offsets[0] as usize, (nsec + 1) * 4);
        assert_eq!(offsets[nsec] as usize, bytes.len());
        let mut out = Vec::new();
        for i in 0..nsec {
            let ct = &bytes[offsets[i] as usize..offsets[i + 1] as usize];
            let plain = open_sector(&key, 1, i as u32, ct).unwrap();
            let expected = 4096u32.min(10_000 - i as u32 * 4096);
            if (plain.len() as u32) < expected {
                let d = crate::compression::decompress_masked(&plain, expected as usize).unwrap();
                out.extend_from_slice(&d);
            } else {
                out.extend_from_slice(&plain);
            }
        }
        assert_eq!(out, data);
    }
}
