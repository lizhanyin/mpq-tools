//! 修改 MPQ 归档：导入（新增 / 替换）与删除文件。
//!
//! 策略：所有保留文件的已存储字节都**原样**复制，因此现有文件的压缩方式、
//! sector CRC 与加密状态保持不变；只有容器本身被重写：
//!
//! * 新文件以全新编码的块追加（每个 sector 用 zlib，小文件用单块
//!   single-unit，永不加密）
//! * 经典 hash/block 表被重建 —— 加密、不压缩 —— 头（v1–v3）指向新布局
//! * `(listfile)` 重新生成；`(attributes)` 和 `(signature)` 被丢弃，
//!   因为任何修改都会使其内容失效（StormLib 也是这么做的）
//!
//! 原样复制策略带来的后果：
//!
//! * HET/BET 表不会被重写 —— 修改后的归档依赖其经典表（头中的 HET/BET
//!   位置被清零）。纯 HET/BET 归档会依据其 BET 记录和 listfile 名
//!   合成出经典表。
//! * 当无名条目仍需可按名寻址时（没有 listfile 的受保护地图），经典
//!   hash table 的**槽位布局会被保留**（表大小不变，条目留在原位，删除
//!   变为 `DELETED` 标记），因为缺少名字就无法重新计算 hash 条目的探测
//!   位置。当所有保留条目都有名字时，则改为构建一张全新的——可能更大的——表。
//! * 以 key-v2 变体加密的文件，其密钥派生自块偏移；由于偏移变化，它们会
//!   从解码后的内容重新编码 —— 默认以在*新*偏移处派生的密钥重新加密
//!   （[`ModifyOptions::reencrypt_key_v2`]）；禁用该选项则以压缩但
//!   未加密的方式存储。
//! * v4 头保持 v4：重建的经典 hash/block 表与头都会获得新的 v4 MD5 校验和
//!   （针对存储后的加密字节 —— 与 StormLib 的 `SaveMpqTable` 所计算的
//!   内容完全一致）；`raw_chunk_size` 被清零，因为每个文件的 chunk MD5
//!   数组不会被保留。使用 [`ModifyOptions::target_version`]（见
//!   [`convert`]）时，容器会改为按请求的版本重写，v4 亦然。
//! * 归档之前的字节（Warcraft III / StarCraft II 地图的 `MPQ\x1B`
//!   user-data 包装，或内嵌归档之前的垃圾数据）以及归档之后的字节
//!   都会被保留。
//! * 密码保护（自定义 v4 AEAD 扩展，见 [`protect`]）：修改受保护的归档
//!   需要其主密钥（[`ModifyOptions::master_key`]）；保持原表槽位的块被
//!   原样复制（其密钥派生自索引），位置移动的块在新索引处重新封装，
//!   导入的文件同样会被封装。拒绝将受保护归档转换为 v1–v3 ——
//!   该扩展只存在于 v4 归档中。

use std::collections::{HashMap, HashSet};

use crate::archive::Archive;
use crate::compression::M_ZLIB;
use crate::crypt::{
    encrypt_bytes, file_key, hash_bytes, hash_string, normalize_name, KEY_BLOCK_TABLE,
    KEY_HASH_TABLE, HASH_NAME_A, HASH_NAME_B, HASH_TABLE_INDEX,
};
use crate::digest;
use crate::error::{Error, Result};
use crate::header::{
    FormatVersion, HEADER_MD5_SPAN, HEADER_SIZE_V1, HEADER_SIZE_V2, HEADER_SIZE_V3, HEADER_SIZE_V4,
    MPQ_MAGIC,
};
use crate::passcrypt::{self, Argon2Params, Vault, VAULT_LEN};
use crate::tables::{
    HashEntry, FILE_COMPRESS, FILE_ENCRYPTED, FILE_EXISTS, FILE_KEY_V2, FILE_PASSWORD_AEAD,
    FILE_SINGLE_UNIT, HASH_ENTRY_DELETED, HASH_ENTRY_FREE,
};
use crate::{FileEntry, TableSource};

/// 导入时不可使用的内部文件名（它们由修改器 / 格式本身管理）。
const FORBIDDEN_IMPORT_NAMES: [&str; 5] = [
    "(listfile)",
    "(attributes)",
    "(signature)",
    "(userdata)",
    "(password)",
];

    /// 一次请求的删除操作。
#[derive(Debug, Clone)]
pub enum Deletion {
    /// 删除所有名字匹配的条目（按 MPQ 规范化：不区分大小写和
    /// 斜杠方向）。不存在的名字会被忽略。
    ByName(String),
    /// 精确删除一个表条目，以与 GUI 选择键相同的方式标识 ——
    /// 这是寻址无名条目的唯一途径。
    ByKey(TableSource, usize),
}

/// [`protect`] / [`ModifyOptions::protect`] 所用的密码 + KDF 参数。
#[derive(Debug, Clone)]
pub struct PasswordProtect {
    pub password: String,
    pub params: Argon2Params,
}

/// [`modify_with`] 的可调选项。
#[derive(Debug, Clone)]
pub struct ModifyOptions {
    /// 以在*新*块偏移处派生的密钥重新加密 key-v2 加密文件（偏移是密钥的
    /// 一部分，因此其字节永远无法原样复制）。为 false 时，这些文件以
    /// 压缩但未加密的方式重新存储。普通（以名字为密钥）的加密文件始终
    /// 保持加密复制，与本选项无关。
    pub reencrypt_key_v2: bool,
    /// 以此头版本（v1–v4）重写容器。`None` 保持源版本。
    /// 目标与源版本相同时不做任何事。
    pub target_version: Option<FormatVersion>,
    /// 受保护源归档的已解封主密钥（见 [`Archive::unlock`] /
    /// [`Archive::master_key`]）。修改或转换受保护归档时必需；用于封装
    /// 导入的内容并重新封装位置移动的块。对未受保护的归档忽略。
    pub master_key: Option<[u8; 32]>,
    /// 用此密码保护归档：每个文件都以密码 AEAD 方案重新编码，并添加一个
    /// `(password)` 保险库（隐含 v4 目标）。见 [`protect`]。
    pub protect: Option<PasswordProtect>,
    /// 解除密码保护（需要 [`ModifyOptions::master_key`]，
    /// [`unprotect`] 会从密码派生它）。每个已封装的文件都重新以明文
    /// 编码，保险库被丢弃。
    pub unprotect: bool,
}

impl Default for ModifyOptions {
    fn default() -> Self {
        ModifyOptions {
            reencrypt_key_v2: true,
            target_version: None,
            master_key: None,
            protect: None,
            unprotect: false,
        }
    }
}

/// 一次 [`modify`] 运行所做的变更。
#[derive(Debug, Clone, Default)]
pub struct ModifyReport {
    /// 新增文件的名称。
    pub added: Vec<String>,
    /// 已存在并被导入替换的文件的名称。
    pub replaced: Vec<String>,
    /// 被删除条目的显示名（无名条目：`<unnamed #i>`）。
    pub deleted: Vec<String>,
    /// 因修改而失效、被移除的内部文件
    /// （`(attributes)`、`(signature)`）。
    pub dropped: Vec<String>,
    /// 在新偏移处重新编码的 key-v2 加密文件。
    pub re_encoded: Vec<String>,
    /// 由明文切换为密码 AEAD 的文件数（保护 / 不变量修复）。
    pub encrypted: usize,
    /// 由密码 AEAD 切换为明文的文件数（解除保护）。
    pub decrypted: usize,
}

/// [`modify`] 运行成功的结果。
#[derive(Debug, Clone)]
pub struct ModifyOutcome {
    /// 完整的修改后文件（包含归档前的前缀）。
    pub bytes: Vec<u8>,
    pub report: ModifyReport,
    /// 针对 [`protect`]：新生成的主密钥，调用方可借此在重新加载后
    /// 保持归档处于已解锁状态。
    pub master_key: Option<[u8; 32]>,
}

/// 一个块写入新归档的方式。
enum Payload {
    /// 原样复制的已存储字节 + 原始标志。
    Raw { bytes: Vec<u8>, flags: u32 },
    /// 内容全新编码：zlib sectors / 单块，不加密（因此编码永不依赖
    /// 块的位置）。
    Fresh { data: Vec<u8> },
    /// 内容重新编码*并*用以块新偏移派生的 key-v2 密钥重新加密 ——
    /// 依赖位置，因此在布局阶段得知偏移后才编码。
    ReEncrypted { name: String, data: Vec<u8> },
    /// 内容以密码 AEAD 方案封装 —— 依赖索引，因此在布局阶段得知新块
    /// 索引后才编码。
    Aead { data: Vec<u8> },
}

/// 经典 hash 条目字段，用于必须（重新）按名字锚定的块 ——
/// 新增文件、重建的 listfile 以及来自 BET 的条目。
#[derive(Clone, Copy)]
struct HashFields {
    name1: u32,
    name2: u32,
    locale: u16,
    platform: u8,
}

impl HashFields {
    fn of(name: &str) -> Self {
        HashFields {
            name1: hash_string(name, HASH_NAME_A),
            name2: hash_string(name, HASH_NAME_B),
            locale: 0,
            platform: 0,
        }
    }
}

/// 输出归档中的一个块。
struct OutBlock {
    payload: Payload,
    fsize: u32,
    /// 已知时的名字（驱动重建的 listfile）。
    name: Option<String>,
    /// 按名字锚定此块的 hash 条目 —— 新增文件、重建的 listfile、
    /// 来自 BET 的条目，以及（在新表模式下）带有原始 locale 变体的
    /// 保留经典条目。
    hashes: Vec<HashFields>,
    /// 保留的经典块，其原始 hash 槽位改为通过重映射保留
    /// （仅在槽位保留模式下有意义）。
    classic_remap: bool,
}

/// 向归档导入文件和/或删除条目，返回完整的修改后文件。
/// 可配置的变体见 [`modify_with`]。
pub fn modify(
    data: &[u8],
    deletions: &[Deletion],
    additions: &[(String, Vec<u8>)],
) -> Result<ModifyOutcome> {
    modify_with(data, deletions, additions, &ModifyOptions::default())
}

/// 将归档的容器转换为另一种头版本（v1–v4）。
///
/// 等价于不带导入/删除列表、并显式指定 [`ModifyOptions::target_version`]
/// 的 [`modify_with`]：每个文件的已存储字节都被原样复制进按目标版本全新
/// 写出的容器，因此只要源能打开，转换就能成功。无法转换的情况会以原因
/// 报错失败：
///
/// * 源完全无法解析（先尝试 [`crate::repair`]）
/// * 布局超出 32 位偏移所能表示的范围（> 4 GB）
///
/// 转换到源自身的版本是无操作，会原样返回输入。重写的后果与修改器相同：
/// HET/BET 表被经典表取代，`(attributes)`/`(signature)` 被丢弃
/// （见 `report`），key-v2 文件在其新偏移处重新编码，归档前后的字节
/// 均被保留。目标是 v4 时会得到带完整校验和的 v4 头
/// （表 MD5 + 头 MD5）。
pub fn convert(data: &[u8], target: FormatVersion) -> Result<ModifyOutcome> {
    let options = ModifyOptions {
        target_version: Some(target),
        ..ModifyOptions::default()
    };
    modify_with(data, &[], &[], &options)
}

/// 用密码保护归档（设计文档中的方案②）：每个可读文件都按 sector 以
/// ChaCha20-Poly1305 重新加密，密钥派生自一个新生成的主密钥，主密钥以
/// Argon2id 包装后存入新的 `(password)` 内部文件。容器变为 v4（必要时
/// 升级），以适用完整的校验和集合。结果是只有本库才能打开的私有扩展。
///
/// 源已受保护时失败（请使用 [`change_password`]），或任一文件无法先解码
/// 时失败（保护不得静默遗留明文）。
pub fn protect(
    data: &[u8],
    password: &str,
    params: &Argon2Params,
) -> Result<ModifyOutcome> {
    let probe = Archive::new(std::io::Cursor::new(data))?;
    if probe.is_password_protected() {
        return Err(Error::InvalidData(
            "归档已启用密码保护；如需更换密码请使用 change_password（修改密码）".to_string(),
        ));
    }
    let options = ModifyOptions {
        protect: Some(PasswordProtect {
            password: password.to_string(),
            params: *params,
        }),
        ..ModifyOptions::default()
    };
    modify_with(data, &[], &[], &options)
}

/// 解除密码保护：先校验密码，然后将每个已封装的文件重新编码为明文，
/// 并丢弃 `(password)` 保险库。
pub fn unprotect(data: &[u8], password: &str) -> Result<ModifyOutcome> {
    let mut probe = Archive::new(std::io::Cursor::new(data))?;
    if !probe.is_password_protected() {
        return Err(Error::InvalidData(
            "归档未启用密码保护，无需解除".to_string(),
        ));
    }
    probe.unlock(password)?;
    let master = probe.master_key().expect("unlock set the master key");
    let options = ModifyOptions {
        master_key: Some(master),
        unprotect: true,
        ..ModifyOptions::default()
    };
    modify_with(data, &[], &[], &options)
}

/// 更改归档的密码：先校验旧密码，然后用新密码和一个新盐值重新包装主密钥。
/// 保险库是一段固定大小的明文记录，因此只在原位重写这些字节 ——
/// 不触碰任何文件数据（v4 的表/头校验和也保持有效，因为它们都不覆盖
/// 文件内容）。
pub fn change_password(
    data: &[u8],
    old_password: &str,
    new_password: &str,
    params: &Argon2Params,
) -> Result<Vec<u8>> {
    let mut archive = Archive::new(std::io::Cursor::new(data))?;
    if !archive.is_password_protected() {
        return Err(Error::InvalidData(
            "归档未启用密码保护，无法修改密码".to_string(),
        ));
    }
    archive.unlock(old_password)?;
    let master = archive.master_key().expect("unlock set the master key");

    let entry = archive
        .find_file(passcrypt::PASSWORD_FILE_NAME)
        .ok_or_else(|| Error::InvalidData("(password) 文件不存在".to_string()))?;
    if !entry.is_single_unit()
        || entry.is_compressed()
        || entry.is_encrypted()
        || entry.compressed_size as usize != VAULT_LEN
    {
        return Err(Error::InvalidData(
            "(password) 文件布局异常（非 128 字节明文单块），无法原位更换密码；\
             可先解除密码保护后重新设置".to_string(),
        ));
    }

    let vault = Vault::rewindrap(new_password.as_bytes(), params, &master)?;
    let mut out = data.to_vec();
    let at = archive.archive_offset() as usize + entry.offset as usize;
    out[at..at + VAULT_LEN].copy_from_slice(&vault.serialize());

    // 自检：拼接后的归档必须能用新密码打开并解锁（且拒绝旧密码）。
    let mut check = Archive::new(std::io::Cursor::new(&out))?;
    check.unlock(new_password)?;
    Ok(out)
}

/// 带显式 [`ModifyOptions`] 参数的 [`modify`]。
pub fn modify_with(
    data: &[u8],
    deletions: &[Deletion],
    additions: &[(String, Vec<u8>)],
    options: &ModifyOptions,
) -> Result<ModifyOutcome> {
    let mut archive = Archive::new(std::io::Cursor::new(data))?;
    let base = archive.archive_offset() as usize;
    let sector_size = archive.sector_size();
    let source_version = archive.version();
    let protected_src = archive.is_password_protected();
    // protect() 隐含 v4 容器（该扩展的基础版本）。
    let version = if options.protect.is_some() {
        FormatVersion::V4
    } else {
        options.target_version.unwrap_or(source_version)
    };
    // 重写为源自身的版本只会白白搬动字节。
    let converting = options.target_version.is_some_and(|t| t != source_version)
        || (options.protect.is_some() && source_version != FormatVersion::V4);
    let sector_shift = archive.header().block_size;

    if !converting
        && deletions.is_empty()
        && additions.is_empty()
        && options.protect.is_none()
        && !options.unprotect
    {
        return Ok(ModifyOutcome {
            bytes: data.to_vec(),
            report: ModifyReport::default(),
            master_key: None,
        });
    }

    // --- 密码方案设置 ----------------------------------------------
    if protected_src && options.protect.is_some() {
        return Err(Error::InvalidData(
            "归档已启用密码保护；如需更换密码请使用 change_password（修改密码）".to_string(),
        ));
    }
    if protected_src && version < FormatVersion::V4 {
        return Err(Error::InvalidData(
            "归档已启用密码加密（自定义 v4 扩展），无法转为 v1–v3；请先解除密码保护".to_string(),
        ));
    }
    let (effective_master, vault_bytes) = match &options.protect {
        Some(spec) => {
            let (vault, master) = Vault::generate(spec.password.as_bytes(), &spec.params)?;
            (Some(master), Some(vault))
        }
        None => (options.master_key, None),
    };
    if protected_src && effective_master.is_none() {
        return Err(Error::InvalidData(
            "该归档已启用密码加密：修改或转换前需要先输入密码（解锁归档）".to_string(),
        ));
    }
    if protected_src {
        let master = effective_master.expect("checked above");
        archive
            .unlock_with_master_key(&master)
            .map_err(|_| Error::InvalidData("主密钥与归档的 (password) 校验不匹配".to_string()))?;
    }

    // --- 规范化导入项（后面的重复项覆盖先前的） -------
    let mut adds: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, bytes) in additions {
        let name = sanitize_name(name)?;
        match adds.iter().position(|(n, _)| same_name(n, &name)) {
            Some(i) => adds[i] = (name, bytes.clone()),
            None => adds.push((name, bytes.clone())),
        }
    }

    // --- 快照源表 ----------------------------------------
    // (source, index) -> 名字，来自统一的条目列表。
    let name_of: HashMap<(TableSource, usize), String> = archive
        .entries()
        .iter()
        .filter_map(|e| e.name.clone().map(|n| ((e.source, e.block_index), n)))
        .collect();

    let classic = archive.classic_tables();
    let bet = archive.bet_table();
    let use_classic_blocks = classic.is_some_and(|c| !c.block_table.is_empty());

    // 经典 hash 条目按其指向的块分组（仅含已占用的条目）——
    // 用于匹配无名但可寻址的文件。
    let mut hash_by_block: HashMap<usize, Vec<HashEntry>> = HashMap::new();
    if let Some(c) = classic {
        for entry in c.hash_table.iter() {
            if !entry.is_free() {
                hash_by_block
                    .entry(entry.block_index as usize)
                    .or_default()
                    .push(*entry);
            }
        }
    }

    // --- 枚举源块 -------------------------------------------
    struct SrcBlock {
        source: TableSource,
        index: usize,
        offset: u64,
        csize: u32,
        fsize: u32,
        flags: u32,
        name: Option<String>,
    }
    let mut src: Vec<SrcBlock> = Vec::new();
    if use_classic_blocks {
        let c = classic.unwrap();
        for (i, block) in c.block_table.iter().enumerate() {
            if block.flags & FILE_EXISTS == 0 {
                continue; // 幽灵/空闲条目
            }
            src.push(SrcBlock {
                source: TableSource::Classic,
                index: i,
                offset: c.block_offset(i),
                csize: block.csize,
                fsize: block.fsize,
                flags: block.flags,
                name: name_of.get(&(TableSource::Classic, i)).cloned(),
            });
        }
    } else if let Some(bet) = bet {
        for i in 0..bet.entry_count as usize {
            let rec = bet.record(i as u32);
            if rec.flags & FILE_EXISTS == 0 {
                continue;
            }
            src.push(SrcBlock {
                source: TableSource::Bet,
                index: i,
                offset: rec.file_pos,
                csize: rec.cmp_size,
                fsize: rec.file_size,
                flags: rec.flags,
                name: name_of.get(&(TableSource::Bet, i)).cloned(),
            });
        }
    }

    // --- 决定丢弃哪些源块 -----------------------------
    let mut dropped_keys: HashSet<(TableSource, usize)> = HashSet::new();
    let mut report = ModifyReport::default();

    for del in deletions {
        match del {
            Deletion::ByKey(s, i) => {
                if let Some(b) = src.iter().find(|b| b.source == *s && b.index == *i) {
                    if b.name.as_deref().is_some_and(|n| same_name(n, "(password)")) {
                        return Err(Error::InvalidData(
                            "不能删除 (password)（密码数据）；如需去掉密码保护请使用「解除密码保护」\
                             （unprotect）".to_string(),
                        ));
                    }
                    report
                        .deleted
                        .push(b.name.clone().unwrap_or_else(|| format!("<unnamed #{i}>")));
                    dropped_keys.insert((*s, *i));
                }
            }
            Deletion::ByName(n) => {
                if same_name(n, "(password)") {
                    return Err(Error::InvalidData(
                        "不能删除 (password)（密码数据）；如需去掉密码保护请使用「解除密码保护」\
                         （unprotect）".to_string(),
                    ));
                }
                for b in &src {
                    if b.name.as_deref().is_some_and(|bn| same_name(bn, n))
                        && dropped_keys.insert((b.source, b.index))
                    {
                        report.deleted.push(b.name.clone().unwrap_or_default());
                    }
                }
                // 无名但可寻址的条目在这里无法按名字匹配；
                // 下面的归档查找会覆盖它们。
                if let Some(e) = archive.find_file(n) {
                    if dropped_keys.insert((e.source, e.block_index)) {
                        report.deleted.push(n.clone());
                    }
                }
            }
        }
    }

    // 由修改器管理或因修改而失效的内部文件。
    for b in &src {
        if let Some(n) = &b.name {
            if n == "(attributes)" || n == "(signature)" {
                report.dropped.push(n.clone());
                dropped_keys.insert((b.source, b.index));
            } else if n == "(listfile)" {
                // 总是在下面重建；从不复制。
                dropped_keys.insert((b.source, b.index));
            }
        }
    }

    // 导入项会替换匹配的条目 —— 按附带的名字、按经典 hash 对
    // （无名但可寻址），或通过归档查找（来自 HET/BET 的条目）。
    for (name, _) in &adds {
        let norm = normalize_name(name);
        let pair = (hash_bytes(&norm, HASH_NAME_A), hash_bytes(&norm, HASH_NAME_B));
        let mut replaced_any = false;
        for b in &src {
            let name_match = b.name.as_deref().is_some_and(|bn| same_name(bn, name));
            let hash_match = b.source == TableSource::Classic
                && b.name.is_none()
                && hash_by_block
                    .get(&b.index)
                    .is_some_and(|entries| entries.iter().any(|e| (e.name1, e.name2) == pair));
            if name_match || hash_match {
                dropped_keys.insert((b.source, b.index));
                replaced_any = true;
            }
        }
        if let Some(e) = archive.find_file(name) {
            if dropped_keys.insert((e.source, e.block_index)) {
                replaced_any = true;
            }
        }
        if replaced_any {
            report.replaced.push(name.clone());
        } else {
            report.added.push(name.clone());
        }
    }

    // --- 构建输出块 ------------------------------------------------
    let mut blocks: Vec<OutBlock> = Vec::new();
    // 旧经典块索引 -> 新块索引（hash 槽位重映射）
    let mut remap: HashMap<usize, usize> = HashMap::new();
    // 重新编码的 key-v2 文件（名字, 原始内容），供自检使用。
    let mut reencoded: Vec<(String, Vec<u8>)> = Vec::new();
    // 重新封装的 AEAD 块（新块索引, 原始内容），同上 ——
    // 无名文件没有可用于读回的名字。
    let mut aead_reencoded: Vec<(usize, Vec<u8>)> = Vec::new();

    fn entry_of(b: &SrcBlock) -> FileEntry {
        FileEntry {
            name: b.name.clone(),
            guessed_ext: None,
            locale: 0,
            block_index: b.index,
            offset: b.offset,
            compressed_size: b.csize,
            size: b.fsize,
            flags: b.flags,
            source: b.source,
        }
    }

    for b in &src {
        if dropped_keys.contains(&(b.source, b.index)) {
            continue;
        }
        let start = (base as u64 + b.offset) as usize;
        let end = start.saturating_add(b.csize as usize).min(data.len());
        let raw = data[start.min(end)..end].to_vec();
        let is_vault = b.name.as_deref() == Some(passcrypt::PASSWORD_FILE_NAME);

        if is_vault && options.unprotect {
            // 解除保护时，保险库连同整个方案一并丢弃。
            report.dropped.push(passcrypt::PASSWORD_FILE_NAME.to_string());
            continue;
        }

        // key-v2 文件的密钥派生自块偏移，而偏移会变化：改为从解码后的
        // 内容重新编码，而不是原样复制。密码 AEAD 文件的密钥同样派生自
        // 块*索引* —— 保持原槽位的块仍可原样复制。
        let payload = if is_vault {
            // 保险库是与位置无关的明文数据：总是原样复制，
            // 从不重新封装。
            Payload::Raw { bytes: raw, flags: b.flags }
        } else if b.flags & FILE_PASSWORD_AEAD != 0 {
            if options.unprotect {
                match archive.read_entry(&entry_of(b)) {
                    Ok(content) => {
                        report.decrypted += 1;
                        Payload::Fresh { data: content }
                    }
                    Err(e) => {
                        return Err(Error::InvalidData(format!(
                            "无法解除密码保护：读取 {} 失败：{e}",
                            b.name
                                .clone()
                                .unwrap_or_else(|| format!("<unnamed #{}>", b.index))
                        )))
                    }
                }
            } else if use_classic_blocks && blocks.len() == b.index {
                // 同一表槽位 → 相同的 HKDF 文件密钥：原样复制。
                Payload::Raw { bytes: raw, flags: b.flags }
            } else {
                match archive.read_entry(&entry_of(b)) {
                    Ok(content) => {
                        aead_reencoded.push((blocks.len(), content.clone()));
                        Payload::Aead { data: content }
                    }
                    // 源中本就无法读取；原样复制保持现状，
                    // 而不是让整个修改失败。
                    Err(_) => Payload::Raw { bytes: raw, flags: b.flags },
                }
            }
        } else if options.protect.is_some() {
            // 保护时必须封装所有可读内容；无法先解码的文件应当报错，
            // 而不是静默遗留明文。
            match archive.read_entry(&entry_of(b)) {
                Ok(content) => {
                    report.encrypted += 1;
                    aead_reencoded.push((blocks.len(), content.clone()));
                    Payload::Aead { data: content }
                }
                Err(e) => {
                    return Err(Error::InvalidData(format!(
                        "无法加密归档：读取 {} 失败：{e}（加密前必须能读取全部文件；\
                         可先提取全部再重建归档）",
                        b.name
                            .clone()
                            .unwrap_or_else(|| format!("<unnamed #{}>", b.index))
                    )))
                }
            }
        } else if protected_src && effective_master.is_some() {
            // 受保护归档中的明文块：重新封装它，以维持
            // “受保护 = 全部已封装”这一不变量。
            match archive.read_entry(&entry_of(b)) {
                Ok(content) => {
                    report.encrypted += 1;
                    aead_reencoded.push((blocks.len(), content.clone()));
                    Payload::Aead { data: content }
                }
                Err(_) => Payload::Raw { bytes: raw, flags: b.flags },
            }
        } else if b.flags & FILE_KEY_V2 != 0 && b.name.is_some() {
            let entry = entry_of(b);
            match archive.read_entry(&entry) {
                Ok(content) => {
                    let name = b.name.clone().unwrap_or_default();
                    report.re_encoded.push(name.clone());
                    reencoded.push((name.clone(), content.clone()));
                    if options.reencrypt_key_v2 {
                        Payload::ReEncrypted { name, data: content }
                    } else {
                        Payload::Fresh { data: content }
                    }
                }
                // 源中本就无法读取；原样复制保持现状，
                // 而不是让整个修改失败。
                Err(_) => Payload::Raw { bytes: raw, flags: b.flags },
            }
        } else {
            Payload::Raw { bytes: raw, flags: b.flags }
        };

        // 需要槽位保留时，保留的经典块维持其原始 hash 槽位（重映射）；
        // 有名字的 BET 块必须在新经典 hash table 中按名字重新锚定。
        let hashes = match b.source {
            TableSource::Classic => hash_by_block
                .get(&b.index)
                .map(|entries| {
                    entries
                        .iter()
                        .map(|e| HashFields {
                            name1: e.name1,
                            name2: e.name2,
                            locale: e.locale,
                            platform: e.platform,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            TableSource::Bet => b.name.as_deref().map(|n| vec![HashFields::of(n)]).unwrap_or_default(),
        };
        if b.source == TableSource::Classic {
            remap.insert(b.index, blocks.len());
        }
        blocks.push(OutBlock {
            fsize: b.fsize,
            name: b.name.clone(),
            hashes,
            classic_remap: b.source == TableSource::Classic,
            payload,
        });
    }

    for (name, bytes) in &adds {
        // 在受保护（或正在保护）的归档内，导入的内容与其他内容
        // 一样被封装。
        let payload = if effective_master.is_some() {
            Payload::Aead { data: bytes.clone() }
        } else {
            Payload::Fresh { data: bytes.clone() }
        };
        blocks.push(OutBlock {
            payload,
            fsize: bytes.len() as u32,
            name: Some(name.clone()),
            hashes: vec![HashFields::of(name)],
            classic_remap: false,
        });
    }

    // 重建的 listfile：除 MPQ 内部文件外的所有保留 / 新增名字。
    let listfile_names: Vec<&str> = blocks
        .iter()
        .filter_map(|b| b.name.as_deref())
        .filter(|n| !FORBIDDEN_IMPORT_NAMES.contains(n))
        .collect();
    if !listfile_names.is_empty() {
        let content = listfile_names.join("\r\n").into_bytes();
        blocks.push(OutBlock {
            fsize: content.len() as u32,
            name: Some("(listfile)".to_string()),
            hashes: vec![HashFields::of("(listfile)")],
            classic_remap: false,
            payload: Payload::Fresh { data: content },
        });
    }

    // 保护时的密码保险库：一个明文的 128 字节单块（single-unit）块。
    // `change_password` 依赖这一精确布局来原位重新包装主密钥，
    // 而不触碰任何其他内容。
    if let Some(vault) = &vault_bytes {
        let bytes = vault.serialize().to_vec();
        blocks.push(OutBlock {
            fsize: VAULT_LEN as u32,
            name: Some(passcrypt::PASSWORD_FILE_NAME.to_string()),
            hashes: vec![HashFields::of(passcrypt::PASSWORD_FILE_NAME)],
            classic_remap: false,
            payload: Payload::Raw { bytes, flags: FILE_SINGLE_UNIT },
        });
    }

    // 转换不得静默产生孤儿文件：名字无法恢复的 BET 来源条目
    // （没有 listfile 的纯 HET/BET 归档）会保住其字节，却在重建的
    // 经典表中失去按名寻址能力。普通修改可以容忍这一点；
    // 显式的版本转换则应给出原因并拒绝。
    if converting {
        let orphaned = blocks
            .iter()
            .filter(|b| b.name.is_none() && b.hashes.is_empty() && !b.classic_remap)
            .count();
        if orphaned > 0 {
            return Err(Error::InvalidData(format!(
                "无法转换：源归档使用 HET/BET 表且缺少 (listfile)，{orphaned} 个文件的名字\
                 无法恢复，转为经典表后将无法按名访问（数据仍在但无法寻址）。\
                 可先「提取全部」再新建目标版本的归档重新导入。"
            )));
        }
    }

    if blocks.is_empty() {
        // 零文件的源连重建的 listfile 都无法承载。当只有容器变化
        // （或从原本为空的归档上解除保护）时，改为生成一个空的目标版本
        // 归档，而不是报错。
        if (converting || options.unprotect) && adds.is_empty() {
            let hash_count =
                crate::create::normalize_hash_size(archive.header().hash_table_size.max(4));
            let inner = crate::create::empty_archive(version, hash_count, sector_shift);
            let mut out = Vec::with_capacity(base + inner.len());
            out.extend_from_slice(&data[..base]);
            out.extend_from_slice(&inner);
            // 原归档之后的字节被保留，与下文相同。
            let old_end =
                base + archive.header().archive_size((data.len() - base) as u64) as usize;
            if old_end < data.len() {
                out.extend_from_slice(&data[old_end..]);
            }
            let check = Archive::new(std::io::Cursor::new(&out))?;
            if check.version() != version {
                return Err(Error::InvalidData("转换校验失败：版本不一致".to_string()));
            }
            return Ok(ModifyOutcome {
                bytes: out,
                report,
                master_key: None,
            });
        }
        return Err(Error::InvalidData(
            "修改后归档不包含任何文件；至少需要保留一个条目".to_string(),
        ));
    }

    // --- 布置块，编码新块 -----------------------------------
    let header_size = match version {
        FormatVersion::V1 => HEADER_SIZE_V1 as u64,
        FormatVersion::V2 => HEADER_SIZE_V2 as u64,
        FormatVersion::V3 => HEADER_SIZE_V3 as u64,
        FormatVersion::V4 => HEADER_SIZE_V4 as u64,
    };
    struct Placed {
        pos: u64,
        bytes: Vec<u8>,
        flags: u32,
    }
    let mut cursor = header_size;
    let mut placed: Vec<Placed> = Vec::with_capacity(blocks.len());
    for (idx, b) in blocks.iter().enumerate() {
        // 编码前必须先确定块的位置：重新加密的 key-v2 块以它派生密钥，
        // AEAD 块则以块索引派生密钥。
        let pos = cursor;
        let (bytes, flags) = match &b.payload {
            Payload::Raw { bytes, flags } => (bytes.clone(), *flags),
            Payload::Fresh { data } => encode_fresh(data, sector_size),
            Payload::ReEncrypted { name, data } => {
                encode_fresh_reencrypted(name, data, sector_size, pos)
            }
            Payload::Aead { data } => {
                let master = effective_master.expect("Aead payload implies a master key");
                passcrypt::encode_fresh_aead(data, sector_size, idx, &master)
            }
        };
        cursor += bytes.len() as u64;
        cursor = (cursor + 3) & !3; // dword 对齐，与真实归档一致
        placed.push(Placed { pos, bytes, flags });
    }

    // --- hash table ------------------------------------------------------------
    // hash 条目的探测位置取决于文件名，因此名字未知的条目只有在原表
    // 槽位布局被保留时才能存活。当所有保留条目都有名字时，改为构建
    // 一张全新的（可能更大的）表 —— 只要原表会溢出就必须如此。
    let slot_preservation = classic.is_some_and(|c| {
        !c.hash_table.is_empty()
            && c.hash_table.iter().any(|e| {
                !e.is_free()
                    && src.iter().any(|b| {
                        b.source == TableSource::Classic
                            && b.index == e.block_index as usize
                            && !dropped_keys.contains(&(b.source, b.index))
                            && b.name.is_none()
                    })
            })
    });
    let mut hash_entries: Vec<HashEntry> = if slot_preservation {
        let orig = &classic.expect("checked above").hash_table;
        let mut entries = orig.clone();
        for e in entries.iter_mut() {
            if e.is_free() {
                continue;
            }
            e.block_index = match remap.get(&(e.block_index as usize)) {
                Some(&new) => new as u32,
                None => HASH_ENTRY_DELETED,
            };
        }
        entries
    } else {
        let inserts: usize = blocks.iter().map(|b| b.hashes.len()).sum();
        vec![free_hash_entry(); next_pow2(inserts.saturating_mul(2)).max(4)]
    };
    for (i, b) in blocks.iter().enumerate() {
        if slot_preservation && b.classic_remap {
            continue; // 已由重映射后的原始槽位覆盖
        }
        let Some(name) = b.name.as_deref() else { continue };
        for &fields in &b.hashes {
            if !insert_into_hash(&mut hash_entries, name, fields, i as u32) {
                return Err(Error::InvalidData(
                    "原始哈希表已满，无法插入新文件（且缺少名单无法重建更大的表）".to_string(),
                ));
            }
        }
    }

    // --- 序列化表 --------------------------------------------------------
    let mut hash_buf = Vec::with_capacity(hash_entries.len() * 16);
    for e in &hash_entries {
        hash_buf.extend_from_slice(&e.name1.to_le_bytes());
        hash_buf.extend_from_slice(&e.name2.to_le_bytes());
        hash_buf.extend_from_slice(&e.locale.to_le_bytes());
        hash_buf.push(e.platform);
        hash_buf.push(e.flags);
        hash_buf.extend_from_slice(&e.block_index.to_le_bytes());
    }
    encrypt_bytes(&mut hash_buf, KEY_HASH_TABLE);

    let mut block_buf = Vec::with_capacity(placed.len() * 16);
    for (b, p) in blocks.iter().zip(&placed) {
        if p.pos > u32::MAX as u64 {
            return Err(Error::InvalidData(
                "归档超过 4 GB，无法用 32 位偏移写出".to_string(),
            ));
        }
        block_buf.extend_from_slice(&(p.pos as u32).to_le_bytes());
        block_buf.extend_from_slice(&(p.bytes.len() as u32).to_le_bytes());
        block_buf.extend_from_slice(&b.fsize.to_le_bytes());
        block_buf.extend_from_slice(&(p.flags | FILE_EXISTS).to_le_bytes());
    }
    encrypt_bytes(&mut block_buf, KEY_BLOCK_TABLE);

    let hash_pos = cursor;
    cursor += hash_buf.len() as u64;
    let block_pos = cursor;
    cursor += block_buf.len() as u64;
    if cursor > u32::MAX as u64 {
        return Err(Error::InvalidData("归档超过 4 GB，无法写出".to_string()));
    }

    // --- 组装 -------------------------------------------------------------------
    let mut header = write_header(
        version,
        sector_shift,
        hash_pos as u32,
        hash_entries.len() as u32,
        block_pos as u32,
        placed.len() as u32,
        cursor,
    );
    if version == FormatVersion::V4 {
        finish_v4_header(&mut header, &hash_buf, &block_buf);
    }

    let mut out = Vec::with_capacity(base + cursor as usize);
    out.extend_from_slice(&data[..base]);
    out.extend_from_slice(&header);
    for p in &placed {
        out.extend_from_slice(&p.bytes);
        let pad = (4 - p.bytes.len() % 4) % 4;
        out.extend(std::iter::repeat_n(0u8, pad));
    }
    out.extend_from_slice(&hash_buf);
    out.extend_from_slice(&block_buf);

    // 原归档之后的字节（追加的数据）被保留。
    let old_end =
        base + archive.header().archive_size((data.len() - base) as u64) as usize;
    if old_end < data.len() {
        out.extend_from_slice(&data[old_end..]);
    }

    // --- 自检：结果必须能打开并正确读出新增内容 ---------------
    let mut check = Archive::new(std::io::Cursor::new(&out))?;
    if let Some(t) = options.target_version {
        if check.version() != t {
            return Err(Error::InvalidData(
                "转换校验失败：写出的版本与目标不一致".to_string(),
            ));
        }
    }
    if let Some(master) = effective_master {
        if check.is_password_protected() {
            check
                .unlock_with_master_key(&master)
                .map_err(|_| Error::InvalidData("校验失败：主密钥不匹配".to_string()))?;
        }
    }
    for (name, bytes) in &adds {
        let read = check.read_file(name)?;
        if &read != bytes {
            return Err(Error::InvalidData(format!("校验失败：{name} 内容不一致")));
        }
    }
    // 重新编码的 key-v2 文件必须能读回逐字节一致的内容，
    // 无论它们是被重新加密还是明文存储。
    for (name, content) in &reencoded {
        let read = check
            .read_file(name)
            .map_err(|e| Error::InvalidData(format!("校验失败：{name} 重编码后无法读取：{e}")))?;
        if &read != content {
            return Err(Error::InvalidData(format!("校验失败：{name} 内容不一致")));
        }
    }
    // 重新封装的 AEAD 块同理，按表索引寻址（无名文件无法按名读取）。
    for (new_index, content) in &aead_reencoded {
        let entry = check
            .entries()
            .into_iter()
            .find(|e| e.source == TableSource::Classic && e.block_index == *new_index);
        let entry = entry.ok_or_else(|| {
            Error::InvalidData(format!("校验失败：块 #{new_index} 加密后无法寻址"))
        })?;
        let read = check.read_entry(&entry).map_err(|e| {
            Error::InvalidData(format!("校验失败：块 #{new_index} 加密后无法读取：{e}"))
        })?;
        if &read != content {
            return Err(Error::InvalidData(format!(
                "校验失败：块 #{new_index} 内容不一致"
            )));
        }
    }
    for del in deletions {
        if let Deletion::ByName(n) = del {
            if !adds.iter().any(|(an, _)| same_name(an, n)) && check.has_file(n) {
                return Err(Error::InvalidData(format!("校验失败：{n} 仍然存在")));
            }
        }
    }

    Ok(ModifyOutcome {
        bytes: out,
        report,
        master_key: effective_master.filter(|_| options.protect.is_some()),
    })
}

// -- 辅助函数 ------------------------------------------------------------------

fn free_hash_entry() -> HashEntry {
    HashEntry {
        name1: 0xFFFF_FFFF,
        name2: 0xFFFF_FFFF,
        locale: 0,
        platform: 0,
        flags: 0,
        block_index: HASH_ENTRY_FREE,
    }
}

/// MPQ 名字相等：规范化（大写、`\` 分隔符）后的字节相等。
fn same_name(a: &str, b: &str) -> bool {
    normalize_name(a) == normalize_name(b)
}

/// 规范化导入的名字：`/` → `\`，去掉空 / `.` / `..` 部分。
fn sanitize_name(name: &str) -> Result<String> {
    if FORBIDDEN_IMPORT_NAMES.contains(&name) {
        return Err(Error::InvalidData(format!("不允许使用保留名 {name}")));
    }
    let normalized = name.replace('/', "\\");
    let cleaned: Vec<&str> = normalized
        .split('\\')
        .filter(|p| !p.is_empty() && *p != "." && *p != "..")
        .collect();
    if cleaned.is_empty() {
        return Err(Error::InvalidData(format!("无效的文件名 “{name}”")));
    }
    Ok(cleaned.join("\\"))
}

fn next_pow2(n: usize) -> usize {
    n.max(1).next_power_of_two()
}

/// 以开放探测法将一个名字插入经典 hash table。名字对与 locale 相同的
/// 已占用槽位会被替换。表已满时返回 false。
fn insert_into_hash(
    entries: &mut [HashEntry],
    name: &str,
    fields: HashFields,
    block_index: u32,
) -> bool {
    let norm = normalize_name(name);
    let start = (hash_bytes(&norm, HASH_TABLE_INDEX) as usize) % entries.len();
    for off in 0..entries.len() {
        let slot = (start + off) % entries.len();
        let e = &entries[slot];
        let same = !e.is_free()
            && e.name1 == fields.name1
            && e.name2 == fields.name2
            && e.locale == fields.locale;
        if same || e.is_free() {
            entries[slot] = HashEntry {
                name1: fields.name1,
                name2: fields.name2,
                locale: fields.locale,
                platform: fields.platform,
                flags: 0,
                block_index,
            };
            return true;
        }
    }
    false
}

/// 将内容编码为新的存储块：有收益时每个 sector 用 zlib，小文件用
/// 单块（single unit），不可压缩时用明文 sector。
/// 返回 (存储字节, flags)；文件大小以调用方为准。
fn encode_fresh(data: &[u8], sector_size: u32) -> (Vec<u8>, u32) {
    let fsize = data.len() as u32;

    if fsize == 0 {
        return (Vec::new(), FILE_SINGLE_UNIT);
    }

    if fsize < sector_size {
        let z = zlib(data);
        if z.len() + 1 < data.len() {
            let mut b = Vec::with_capacity(z.len() + 1);
            b.push(M_ZLIB);
            b.extend_from_slice(&z);
            return (b, FILE_SINGLE_UNIT | FILE_COMPRESS);
        }
        return (data.to_vec(), FILE_SINGLE_UNIT);
    }

    let nsec = fsize.div_ceil(sector_size) as usize;
    let mut sectors: Vec<Vec<u8>> = Vec::with_capacity(nsec);
    for chunk in data.chunks(sector_size as usize) {
        let z = zlib(chunk);
        if z.len() + 1 < chunk.len() {
            let mut s = Vec::with_capacity(z.len() + 1);
            s.push(M_ZLIB);
            s.extend_from_slice(&z);
            sectors.push(s);
        } else {
            sectors.push(chunk.to_vec());
        }
    }
    if sectors.iter().map(|s| s.len()).sum::<usize>() >= data.len() {
        // 没有任何内容可压缩：按固定偏移明文存储 sector。
        return (data.to_vec(), 0);
    }

    let table_len = (nsec + 1) * 4;
    let mut block = Vec::with_capacity(table_len + data.len());
    let mut acc = table_len as u32;
    block.extend_from_slice(&acc.to_le_bytes());
    for s in &sectors {
        acc += s.len() as u32;
        block.extend_from_slice(&acc.to_le_bytes());
    }
    for s in &sectors {
        block.extend_from_slice(s);
    }
    (block, FILE_COMPRESS)
}

fn zlib(data: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    enc.write_all(data).expect("in-memory zlib");
    enc.finish().expect("in-memory zlib")
}

/// [`encode_fresh`] 再加上以块新偏移派生密钥的 key-v2 加密 ——
/// 当文件字节无法原样复制时，key-v2 文件必须采用这种布局。
/// 与读取方所期望的磁盘布局约定保持一致：
///
/// * single unit：整个负载用文件密钥加密
/// * 压缩 sector：偏移表（构造上已按 dword 对齐）用 `key - 1` 加密，
///   每个 sector 独立用 `key + i` 加密，sector 填充到 dword 边界
///   （偏移包含填充）
/// * 不可压缩 sector：按固定偏移原样存储，每个用 `key + i` 加密
///
/// 一个 sector 只有在填充后仍小于原始数据时才算可压缩（读取方以
/// `stored_len < uncompressed_len` 区分压缩 sector），因此测试中
/// 留有 +4 的余量。
fn encode_fresh_reencrypted(
    name: &str,
    data: &[u8],
    sector_size: u32,
    pos: u64,
) -> (Vec<u8>, u32) {
    const ENC: u32 = FILE_ENCRYPTED | FILE_KEY_V2;
    let fsize = data.len() as u32;
    let key = file_key(name, true, pos, fsize);

    if fsize == 0 {
        return (Vec::new(), FILE_SINGLE_UNIT);
    }

    if fsize < sector_size {
        let z = zlib(data);
        let compressed = z.len() + 1 < data.len();
        let mut payload = if compressed {
            let mut b = Vec::with_capacity(z.len() + 1);
            b.push(M_ZLIB);
            b.extend_from_slice(&z);
            b
        } else {
            data.to_vec()
        };
        encrypt_bytes(&mut payload, key);
        let mut flags = ENC | FILE_SINGLE_UNIT;
        if compressed {
            flags |= FILE_COMPRESS;
        }
        return (payload, flags);
    }

    let nsec = fsize.div_ceil(sector_size) as usize;
    let mut sectors: Vec<Vec<u8>> = Vec::with_capacity(nsec);
    let mut compressed_any = false;
    for chunk in data.chunks(sector_size as usize) {
        let z = zlib(chunk);
        if z.len() + 4 < chunk.len() {
            compressed_any = true;
            let mut s = Vec::with_capacity(z.len() + 1);
            s.push(M_ZLIB);
            s.extend_from_slice(&z);
            sectors.push(s);
        } else {
            sectors.push(chunk.to_vec());
        }
    }
    if !compressed_any {
        // 按固定偏移明文存储 sector，每个使用各自的密钥。
        let mut payload = Vec::with_capacity(data.len());
        for (i, mut s) in sectors.into_iter().enumerate() {
            encrypt_bytes(&mut s, key.wrapping_add(i as u32));
            payload.extend_from_slice(&s);
        }
        return (payload, ENC);
    }

    // 压缩布局：每个 sector 填充到 dword 边界（偏移包含填充），
    // 偏移表用 key-1 加密，每个 sector 用 key+i 加密。
    for s in sectors.iter_mut() {
        if s.len() % 4 != 0 {
            s.resize(s.len() + 4 - s.len() % 4, 0);
        }
    }
    let table_len = (nsec + 1) * 4;
    let mut block = Vec::with_capacity(table_len + sectors.iter().map(|s| s.len()).sum::<usize>());
    let mut acc = table_len as u32;
    block.extend_from_slice(&acc.to_le_bytes());
    for s in &sectors {
        acc += s.len() as u32;
        block.extend_from_slice(&acc.to_le_bytes());
    }
    encrypt_bytes(&mut block[..table_len], key.wrapping_sub(1));
    for (i, s) in sectors.iter().enumerate() {
        let mut sec = s.clone();
        encrypt_bytes(&mut sec, key.wrapping_add(i as u32));
        block.extend_from_slice(&sec);
    }
    (block, ENC | FILE_COMPRESS)
}

/// 序列化归档头（v1–v4；v3 之外的 v4 字段先置零，由
/// [`finish_v4_header`] 填充）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_header(
    version: FormatVersion,
    sector_shift: u16,
    hash_pos: u32,
    hash_size: u32,
    block_pos: u32,
    block_count: u32,
    archive_size: u64,
) -> Vec<u8> {
    let (header_size, raw_version) = match version {
        FormatVersion::V1 => (HEADER_SIZE_V1, 0u16),
        FormatVersion::V2 => (HEADER_SIZE_V2, 1),
        FormatVersion::V3 => (HEADER_SIZE_V3, 2),
        FormatVersion::V4 => (HEADER_SIZE_V4, 3),
    };
    let mut h = Vec::with_capacity(header_size as usize);
    h.extend_from_slice(&MPQ_MAGIC.to_le_bytes());
    h.extend_from_slice(&header_size.to_le_bytes());
    h.extend_from_slice(&(archive_size.min(u32::MAX as u64) as u32).to_le_bytes());
    h.extend_from_slice(&raw_version.to_le_bytes());
    h.extend_from_slice(&sector_shift.to_le_bytes());
    h.extend_from_slice(&hash_pos.to_le_bytes());
    h.extend_from_slice(&block_pos.to_le_bytes());
    h.extend_from_slice(&hash_size.to_le_bytes());
    h.extend_from_slice(&block_count.to_le_bytes());
    if version >= FormatVersion::V2 {
        h.extend_from_slice(&0u64.to_le_bytes()); // hi-block 表位置
        h.extend_from_slice(&0u16.to_le_bytes()); // hash table 位置高位
        h.extend_from_slice(&0u16.to_le_bytes()); // block table 位置高位
    }
    if version >= FormatVersion::V3 {
        h.extend_from_slice(&archive_size.to_le_bytes()); // 64 位归档大小
        h.extend_from_slice(&0u64.to_le_bytes()); // BET 位置
        h.extend_from_slice(&0u64.to_le_bytes()); // HET 位置
    }
    // v4 独有字段（table size64、raw_chunk_size、各 MD5）起始为零；
    // 随后由 finish_v4_header 填入校验和。
    h.resize(header_size as usize, 0);
    debug_assert_eq!(h.len(), header_size as usize);
    h
}

/// 填充 v4 独有的头字段：显式的表存储大小、针对*存储后*（加密 ——
/// 此处始终未压缩）字节计算的表 MD5，以及头自身在 `0x00..0xC0` 字节上
/// 的 MD5，与 StormLib 的 `SaveMPQTables` 一致。HET/BET/hi-block 不存在
/// （为零），也不写入每个文件的 chunk MD5，因此 `raw_chunk_size`
/// 保持为 0。
pub(crate) fn finish_v4_header(header: &mut [u8], hash_buf: &[u8], block_buf: &[u8]) {
    debug_assert_eq!(header.len(), HEADER_SIZE_V4 as usize);
    let h: &mut [u8] = header;
    h[0x44..0x4C].copy_from_slice(&(hash_buf.len() as u64).to_le_bytes());
    h[0x4C..0x54].copy_from_slice(&(block_buf.len() as u64).to_le_bytes());
    // 0x54 处的 hi-block、0x5C 处的 HET、0x64 处的 BET table size64
    // 保持为 0，0x6C 处的 raw_chunk_size 同样为 0。
    h[0x70..0x80].copy_from_slice(&digest::md5_digest(block_buf)); // MD5_BlockTable（块表 MD5）
    h[0x80..0x90].copy_from_slice(&digest::md5_digest(hash_buf)); // MD5_HashTable（hash 表 MD5）
    // MD5_HiBlockTable/BetTable/HetTable（0x90..0xC0）保持为 0：不存在这些表。
    let header_md5 = digest::md5_digest(&h[..HEADER_MD5_SPAN]);
    h[HEADER_MD5_SPAN..0xD0].copy_from_slice(&header_md5);
}
