//! [`Archive`] 类型：打开 MPQ 归档，查找并读取文件。

use std::collections::VecDeque;
use std::cell::RefCell;
use std::io::{Read, Seek, SeekFrom};
use std::ops::DerefMut;

use crate::attrs::Attributes;
use crate::compression;
use crate::crypt;
use crate::digest;
use crate::error::{Error, Result};
use crate::header::{
    FormatVersion, MpqHeader, UserDataHeader, HEADER_MD5_SPAN, MAX_HEADER_READ, MPQ_MAGIC,
    USERDATA_MAGIC,
};
use crate::hetbet::{load_ext_table, BetTable, HetTable, BET_SIGNATURE, HET_SIGNATURE};
use crate::jenkins;
use crate::crypt::{KEY_BLOCK_TABLE, KEY_HASH_TABLE};
use crate::knownnames;
use crate::passcrypt::{self, TAG_LEN};
use crate::tables::{
    load_mpq_table, parse_block_table, parse_hash_table, ClassicTables, FILE_COMPRESS,
    FILE_COMPRESS_MASK, FILE_DELETE_MARKER, FILE_ENCRYPTED, FILE_EXISTS, FILE_IMPLODE,
    FILE_KEY_V2, FILE_PASSWORD_AEAD, FILE_SECTOR_CRC, FILE_SINGLE_UNIT,
};

/// 可能存在也可能不存在的内部文件名（均为可选）。
pub const SPECIAL_FILES: [&str; 5] = [
    "(listfile)",
    "(attributes)",
    "(signature)",
    "(userdata)",
    "(password)",
];

/// 文件条目的数据来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TableSource {
    /// 经典 block table（对应索引存于 `block_index`）。
    Classic,
    /// BET 表（对应索引存于 `block_index`）。
    Bet,
}

/// 归档内的一个文件，统一表示经典表与 BET 表两种来源。
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// 名字（已知时），来自 `(listfile)`、探测出的内部文件名或
    /// 脚本 / 对象数据中恢复的资源路径。
    pub name: Option<String>,
    /// 未命名条目按内容识别出的类型后缀（如 `"mdx"`）——
    /// 见 [`Archive`] 打开时的类型探测。
    pub guessed_ext: Option<&'static str>,
    pub locale: u16,
    /// 在源表中的索引。
    pub block_index: usize,
    /// 相对归档起始处的数据偏移。
    pub offset: u64,
    pub compressed_size: u32,
    pub size: u32,
    pub flags: u32,
    pub source: TableSource,
}

impl FileEntry {
    pub fn exists(&self) -> bool {
        self.flags & FILE_EXISTS != 0
    }
    pub fn is_deleted_marker(&self) -> bool {
        self.flags & FILE_DELETE_MARKER != 0
    }
    pub fn is_single_unit(&self) -> bool {
        self.flags & FILE_SINGLE_UNIT != 0
    }
    pub fn is_encrypted(&self) -> bool {
        self.flags & FILE_ENCRYPTED != 0
    }
    /// 密码 AEAD 扩展：sector 以归档主密钥派生的 ChaCha20-Poly1305
    /// 封装（而非 storm 加密）。
    pub fn is_password_aead(&self) -> bool {
        self.flags & FILE_PASSWORD_AEAD != 0
    }
    pub fn is_compressed(&self) -> bool {
        self.flags & FILE_COMPRESS_MASK != 0
    }
    /// 用于显示的名字；未知时为 `<unnamed #index>`，若按内容识别出
    /// 类型则附带后缀（`<unnamed #index>.mdx`）。
    pub fn display_name(&self) -> String {
        match &self.name {
            Some(n) => n.clone(),
            None => match self.guessed_ext {
                Some(ext) => format!("<unnamed #{}>.{}", self.block_index, ext),
                None => format!("<unnamed #{}>", self.block_index),
            },
        }
    }
}

/// 一个已打开的 MPQ 归档。
pub struct Archive<R: Read + Seek> {
    reader: RefCell<R>,
    /// MPQ 头在所在文件中的偏移。
    base: u64,
    file_len: u64,
    header: MpqHeader,
    version: FormatVersion,
    sector_size: u32,
    classic: Option<ClassicTables>,
    het: Option<HetTable>,
    bet: Option<BetTable>,
    names: Vec<String>,
    /// 未命名条目按内容识别的类型后缀（按表来源 + 索引缓存，
    /// 打开时探测一次）。
    guessed: std::collections::HashMap<(TableSource, usize), &'static str>,
    /// 已扫过 TEXS 的模型条目（避免迭代恢复中重复读取）。
    scanned_models: std::collections::HashSet<(TableSource, usize)>,
    attributes: Option<Attributes>,
    warnings: Vec<String>,
    /// 读取文件数据时遇到的非致命问题（有界环形缓冲）。
    read_warnings: RefCell<VecDeque<String>>,
    /// 受密码保护归档（自定义 AEAD 扩展）的已解封主密钥；
    /// 锁定时为 `None`。
    master_key: Option<[u8; 32]>,
}

impl Archive<std::fs::File> {
    /// 从文件路径打开归档。
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let file = std::fs::File::open(path.as_ref())?;
        Archive::new(file)
    }
}

impl<R: Read + Seek> Archive<R> {
    /// 从任意 reader 打开归档。MPQ 头的查找位置：流的开头、
    /// `MPQ\x1B` user-data 头之后，或通过扫描（最多 128 MB，
    /// 按 0x200 对齐）寻找内嵌归档。
    pub fn new(mut reader: R) -> Result<Self> {
        let file_len = reader.seek(SeekFrom::End(0))?;

        let base = Self::locate_header(&mut reader, file_len)?;
        reader.seek(SeekFrom::Start(base))?;
        let mut header_buf = vec![0u8; MAX_HEADER_READ as usize];
        read_exact_or_eof(&mut reader, &mut header_buf)?;

        let (header, version) = MpqHeader::parse(&header_buf)?;

        let mut archive = Archive {
            reader: RefCell::new(reader),
            base,
            file_len,
            header,
            version,
            sector_size: 0,
            classic: None,
            het: None,
            bet: None,
            names: Vec::new(),
            guessed: std::collections::HashMap::new(),
            scanned_models: std::collections::HashSet::new(),
            attributes: None,
            warnings: Vec::new(),
            read_warnings: RefCell::new(VecDeque::new()),
            master_key: None,
        };
        archive.sector_size = archive.header.sector_size();
        archive.load_tables()?;
        archive.load_optional_metadata();
        archive.guess_unnamed_kinds();
        // 类型探测后补一轮模型 TEXS × 资源惯例启发式的迭代：
        // 未命名模型的贴图引用 → 贴图名 → 同名反推其它模型 →
        // 新模型的 TEXS → …… 收敛到不动点。
        for _ in 0..4 {
            let a = archive.scan_model_textures();
            let b = archive.heuristic_round();
            if a + b == 0 {
                break;
            }
        }
        Ok(archive)
    }

    // -- 打开辅助函数 ---------------------------------------------------

    /// 在流中查找 MPQ 头所在的字节偏移。
    fn locate_header(reader: &mut R, file_len: u64) -> Result<u64> {
        if file_len < 0x20 {
            return Err(Error::NotAnArchive);
        }

        let mut probe = vec![0u8; MAX_HEADER_READ as usize];
        let read_at = |reader: &mut R, off: u64, buf: &mut [u8]| -> Result<()> {
            reader.seek(SeekFrom::Start(off))?;
            read_exact_or_eof(reader, buf)?;
            Ok(())
        };
        let magic_of = |buf: &[u8]| u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);

        // 1. 偏移 0 处的普通归档。
        read_at(reader, 0, &mut probe)?;
        if magic_of(&probe) == MPQ_MAGIC {
            return Ok(0);
        }

        // 2. 偏移 0 处的 user data 头，指向真正的头。
        if let Some(ud) = UserDataHeader::parse(&probe) {
            let target = ud.archive_offset as u64;
            if target + 0x20 <= file_len {
                read_at(reader, target, &mut probe)?;
                if magic_of(&probe) == MPQ_MAGIC {
                    return Ok(target);
                }
            }
        }

        // 3. 扫描内嵌归档（按 0x200 对齐，与 storm.dll 相同）。
        //    每次读取 1 MiB 窗口，并保留 0x200 字节的重叠，
        //    使靠近窗口末尾的候选仍可被解析。
        let limit = file_len.min(0x0800_0000);
        const CHUNK: usize = 0x10_0000;
        let mut window = vec![0u8; CHUNK + 0x200];
        let mut offset: u64 = 0;

        'outer: while offset + 0x20 <= limit {
            let want = ((limit - offset) as usize).min(CHUNK);
            let read_len = want + 0x200;
            read_at(reader, offset, &mut window[..read_len])?;

            let mut in_off = 0usize;
            while in_off + 0x20 <= want {
                let w = &window[in_off..];
                if w.len() < 4 {
                    break 'outer;
                }

                let magic = u32::from_le_bytes([
                    w.first().copied().unwrap_or_default(),
                    w.get(1).copied().unwrap_or_default(),
                    w.get(2).copied().unwrap_or_default(),
                    w.get(3).copied().unwrap_or_default(),
                ]);
                if magic == USERDATA_MAGIC {
                    if let Some(ud) = UserDataHeader::parse(w) {
                        let target = offset + in_off as u64 + ud.archive_offset as u64;
                        if target + 0x20 <= file_len {
                            read_at(reader, target, &mut probe)?;
                            if magic_of(&probe) == MPQ_MAGIC {
                                return Ok(target);
                            }
                        }
                    }
                } else if magic == MPQ_MAGIC {
                    if let Ok((h, _)) = MpqHeader::parse(w) {
                        if h.plausible(file_len, offset + in_off as u64) {
                            return Ok(offset + in_off as u64);
                        }
                    }
                }
                in_off += 0x200;
            }
            if want < CHUNK {
                break 'outer;
            }
            offset += CHUNK as u64;
        }

        Err(Error::NotAnArchive)
    }

    /// 加载 hash/block/hi-block 与 HET/BET 表，并按表逐一容错：
    /// 只要仍有一个可用的哈希索引（经典 hash 或 HET）和一个文件表
    /// （block 或 BET），任何单个表的失败都降级为警告。
    fn load_tables(&mut self) -> Result<()> {
        let header = self.header.clone();
        let file_len = self.file_len;
        let base = self.base;
        let archive_end = base + header.archive_size(file_len - base.min(file_len));

        // v4：头自带针对 0x00..0xC0 字节的 MD5。
        // StormLib 在不匹配时会拒绝打开；我们只发出警告（修复路径可能
        // 重写头字段，按设计这会使该校验和失效）。
        if self.version == FormatVersion::V4
            && digest::md5_present(&header.md5_header)
            && base + HEADER_MD5_SPAN as u64 <= file_len
        {
            let raw = self.read_at_plain(base, HEADER_MD5_SPAN).unwrap_or_default();
            if digest::md5_digest(&raw) != header.md5_header {
                self.warn("v4 header MD5 mismatch (bytes 0x00-0xC0)");
            }
        }

        // 计算每张经典表在磁盘上占用的字节数。
        let (hash_stored, block_stored) = self.stored_table_sizes(archive_end);

        // --- 经典表 -------------------------------------------------
        let mut classic = ClassicTables::default();

        if header.hash_table_size != 0 {
            let table_size = header.hash_table_size as usize * 16;
            let loaded = load_mpq_table(
                &mut *self.reader.borrow_mut(),
                base + header.hash_table_pos,
                file_len,
                hash_stored,
                table_size,
                KEY_HASH_TABLE,
            );
            match loaded {
                Ok(buf) => match parse_hash_table(&buf) {
                    Ok(t) => {
                        classic.hash_table = t;
                        self.verify_stored_md5(
                            "hash table",
                            base + header.hash_table_pos,
                            hash_stored.unwrap_or(table_size as u64),
                            &header.md5_hash_table,
                        );
                    }
                    Err(e) => self.warn(format!("hash table unusable: {e}")),
                },
                Err(e) => self.warn(format!("hash table unreadable: {e}")),
            }
        }

        if header.block_table_size != 0 {
            let table_size = header.block_table_size as usize * 16;
            let loaded = load_mpq_table(
                &mut *self.reader.borrow_mut(),
                base + header.block_table_pos,
                file_len,
                block_stored,
                table_size,
                KEY_BLOCK_TABLE,
            );
            match loaded {
                Ok(buf) => match parse_block_table(&buf) {
                    Ok(t) => {
                        classic.block_table = t;
                        self.verify_stored_md5(
                            "block table",
                            base + header.block_table_pos,
                            block_stored.unwrap_or(table_size as u64),
                            &header.md5_block_table,
                        );
                    }
                    Err(e) => self.warn(format!("block table unusable: {e}")),
                },
                Err(e) => self.warn(format!("block table unreadable: {e}")),
            }
        }

        if header.hi_block_table_pos != 0 && !classic.block_table.is_empty() {
            let count = classic.block_table.len();
            match self.read_at_plain(base + header.hi_block_table_pos, count * 2) {
                Ok(buf) => {
                    self.verify_stored_md5(
                        "hi-block table",
                        base + header.hi_block_table_pos,
                        (count * 2) as u64,
                        &header.md5_hi_block_table,
                    );
                    classic.hi_block = buf
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect();
                }
                Err(e) => self.warn(format!("hi-block table unreadable: {e}")),
            }
        }

        let have_classic_index = !classic.hash_table.is_empty();
        let have_classic_files = !classic.block_table.is_empty();
        self.classic = if have_classic_index || have_classic_files {
            Some(classic)
        } else {
            None
        };

        // --- HET/BET 表（v3+） -------------------------------------------
        if self.version >= FormatVersion::V3 {
            if header.het_table_pos != 0 {
                let size = self.het_stored_size(archive_end);
                let loaded = load_ext_table(
                    &mut *self.reader.borrow_mut(),
                    base + header.het_table_pos,
                    size,
                    HET_SIGNATURE,
                    KEY_HASH_TABLE,
                );
                match loaded {
                    Ok(buf) => match HetTable::parse(&buf) {
                        Ok(t) => {
                            self.verify_stored_md5(
                                "HET table",
                                base + header.het_table_pos,
                                size,
                                &header.md5_het_table,
                            );
                            self.het = Some(t);
                        }
                        Err(e) => self.warn(format!("HET table unusable: {e}")),
                    },
                    Err(e) => self.warn(format!("HET table unreadable: {e}")),
                }
            }
            if header.bet_table_pos != 0 {
                let size = self.bet_stored_size(archive_end);
                let loaded = load_ext_table(
                    &mut *self.reader.borrow_mut(),
                    base + header.bet_table_pos,
                    size,
                    BET_SIGNATURE,
                    KEY_BLOCK_TABLE,
                );
                match loaded {
                    Ok(buf) => match BetTable::parse(&buf) {
                        Ok(t) => {
                            self.verify_stored_md5(
                                "BET table",
                                base + header.bet_table_pos,
                                size,
                                &header.md5_bet_table,
                            );
                            self.bet = Some(t);
                        }
                        Err(e) => self.warn(format!("BET table unusable: {e}")),
                    },
                    Err(e) => self.warn(format!("BET table unreadable: {e}")),
                }
            }
        }

        // --- 可用性检查 ------------------------------------------------------
        let has_index = self.classic.as_ref().is_some_and(|c| !c.hash_table.is_empty())
            || self.het.is_some();
        let has_files = self.classic.as_ref().is_some_and(|c| !c.block_table.is_empty())
            || self.bet.is_some();

        if !has_index{
            return Err(Error::Corrupt(
                "no usable hash index (classic hash table or HET table)",
            ));
        }
        if !has_files {
            return Err(Error::Corrupt(
                "no usable file table (classic block table or BET table)",
            ));
        }

        // 只有 HET 索引而没有 BET 文件表时无法解析名字。
        if self.classic.as_ref().is_none_or(|c| c.hash_table.is_empty())
            && self.het.is_some()
            && self.bet.is_none()
        {
            return Err(Error::Corrupt("HET table present but BET table missing"));
        }

        if self.sector_size == 0 {
            self.sector_size = 512;
            self.warn("invalid sector exponent; assuming 512-byte sectors");
        }
        Ok(())
    }

    /// 经典 hash/block 表在磁盘上的大小，按版本以 StormLib 的方式推导：
    /// v1 使用未压缩大小，v2/v3 由相邻表之间的间隙计算，v4 直接存储。
    fn stored_table_sizes(&self, archive_end: u64) -> (Option<u64>, Option<u64>) {
        let h = &self.header;
        let hash_size = h.hash_table_size as u64 * 16;
        let block_size = h.block_table_size as u64 * 16;
        let base = self.base;

        match self.version {
            FormatVersion::V1 => (None, None),
            FormatVersion::V4 => {
                let hs = if h.hash_table_size64 != 0 && h.hash_table_size64 <= hash_size {
                    Some(h.hash_table_size64)
                } else {
                    None
                };
                let bs = if h.block_table_size64 != 0 && h.block_table_size64 <= block_size {
                    Some(h.block_table_size64)
                } else {
                    None
                };
                (hs, bs)
            }
            FormatVersion::V2 | FormatVersion::V3 => {
                // 表按升序排列：hash、block、hi-block；大小即为它们
                // 之间的间隙。只有压缩表才需要关心存储大小，
                // 且它总是 <= 未压缩大小。
                let hash_pos = base + h.hash_table_pos;
                let block_pos = base + h.block_table_pos;
                let hi_pos = if h.hi_block_table_pos != 0 {
                    base + h.hi_block_table_pos
                } else {
                    archive_end
                };

                let hash_stored = if h.hash_table_pos != 0 && block_pos > hash_pos {
                    Some((block_pos - hash_pos).min(hash_size))
                } else {
                    None
                };
                let block_stored = if h.block_table_pos != 0 && hi_pos > block_pos {
                    Some((hi_pos - block_pos).min(block_size))
                } else {
                    None
                };
                (hash_stored, block_stored)
            }
        }
    }

    fn het_stored_size(&self, archive_end: u64) -> u64 {
        let h = &self.header;
        match self.version {
            FormatVersion::V4 => h.het_table_size64,
            _ => {
                let upper = self.lower_table_boundary(archive_end);
                upper.saturating_sub(h.het_table_pos)
            }
        }
    }

    fn bet_stored_size(&self, archive_end: u64) -> u64 {
        let h = &self.header;
        match self.version {
            FormatVersion::V4 => h.bet_table_size64,
            _ => {
                // BET 紧贴在下一个边界（hash table 或归档结尾）之下。
                let upper = if h.hash_table_pos != 0 {
                    h.hash_table_pos
                } else {
                    self.lower_table_boundary(archive_end)
                };
                upper.saturating_sub(h.bet_table_pos)
            }
        }
    }

    /// 各“上界”（hash / block / hi-block / 归档结尾）中的最小值。
    fn lower_table_boundary(&self, archive_end: u64) -> u64 {
        let h = &self.header;
        let mut boundary = archive_end;
        if h.hash_table_pos != 0 {
            boundary = boundary.min(h.hash_table_pos);
        }
        if h.block_table_pos != 0 {
            boundary = boundary.min(h.block_table_pos);
        }
        if h.hi_block_table_pos != 0 {
            boundary = boundary.min(h.hi_block_table_pos);
        }
        boundary
    }

    fn read_at_plain(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        if offset >= self.file_len {
            return Err(Error::Corrupt("read beyond end of file"));
        }
        let available = ((self.file_len - offset) as usize).min(len);
        let mut buf = vec![0u8; len];
        let mut reader = self.reader.borrow_mut();
        reader.seek(SeekFrom::Start(offset))?;
        read_exact_or_eof(&mut reader.deref_mut(), &mut buf[..available])?;
        drop(reader);
        Ok(buf)
    }

    /// 针对**存储后**的字节（文件中加密 / 可能已压缩的形态）校验 v4 表
    /// MD5，与 StormLib 的 `LoadMpqTable` / `SaveMpqTable` 完全一致。
    /// 不匹配只发出警告 —— 表本身仍可能正常解码（宽容打开的理念）。
    /// 缺失（全零）的 MD5 和被截断的读取会被跳过。
    fn verify_stored_md5(&mut self, what: &str, offset: u64, stored: u64, expected: &[u8; 16]) {
        if self.version != FormatVersion::V4 || !digest::md5_present(expected) || stored == 0 {
            return;
        }
        if offset >= self.file_len || self.file_len - offset < stored {
            return;
        }
        if let Ok(raw) = self.read_at_plain(offset, stored as usize) {
            if digest::md5_digest(&raw) != *expected {
                self.warn(format!("{what}: v4 table MD5 mismatch"));
            }
        }
    }

    /// 加载 `(listfile)` 和 `(attributes)` —— 两者都完全可选。
    fn load_optional_metadata(&mut self) {
        // 先读 listfile：read_file 只要求表状态完整。
        if self.has_file("(listfile)") {
            match self.read_file("(listfile)") {
                Ok(data) => {
                    let text = String::from_utf8_lossy(&data);
                    self.names = text
                        .split([';', '\r', '\n'])
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                Err(e) => self.warn(format!("(listfile) present but unreadable: {e}")),
            }
        }
        if self.names.is_empty() {
            let recovered = self.recover_names();
            if recovered > 0 {
                self.warn(format!(
                    "no (listfile); recovered {recovered} file name(s) via built-in game file lists"
                ));
            } else {
                self.warn("no (listfile); file names unknown");
            }
        } else {
            // 受保护地图也可能只保留删减过的 listfile：静默补上
            // 未列出但能对上 hash 表的标准文件名。
            self.recover_names();
        }

        if self.has_file("(attributes)") {
            let count = self.file_table_len();
            match self.read_file("(attributes)") {
                Ok(data) => match Attributes::parse(&data, count) {
                    Ok(a) => self.attributes = Some(a),
                    Err(e) => self.warn(format!("(attributes) unusable: {e}")),
                },
                Err(e) => self.warn(format!("(attributes) present but unreadable: {e}")),
            }
        }
    }

    /// 用内置游戏文件名清单试探 hash 表以恢复文件名（缺失或删减
    /// listfile 时）。对 Warcraft III 地图还会读取 `war3map.imp`
    /// （世界编辑器的导入清单），从中恢复导入文件的路径。
    /// 返回新恢复出的名字数。
    fn recover_names(&mut self) -> usize {
        let before = self.names.len();

        // 精炼清单先试：其规范拼写优先登记；MPQ hash 查找是大小写
        // 不敏感的，靠小写去重避免大小写变体重复占用。
        for list in [
            knownnames::WAR3_FILES,
            knownnames::WAR3_DATA_FILES,
            knownnames::MISC_GAME_FILES,
        ] {
            for name in list {
                self.try_recover_name(name);
            }
        }
        for name in knownnames::BUNDLED_LISTFILE.iter() {
            self.try_recover_name(name);
        }

        // 导入清单存在时（哪怕刚由上面的试探发现），其内容本身就是
        // 一批候选文件名 —— 这是恢复 war3mapImported\... 等
        // 自定义路径的唯一途径。
        if let Ok(data) = self.read_file("war3map.imp") {
            for path in knownnames::parse_imp(&data) {
                self.try_recover_name(&path);
            }
        }

        // 已命名文件中的资源路径引用（.j/.lua/.ai 脚本的字符串常量、
        // 对象数据 w3u/w3t/... 内嵌的模型与图标路径、声音定义 w3s、
        // 字符串表 wts 等）—— 导入文件名的又一来源。MDX 模型则解析
        // 其 TEXS chunk 内嵌的贴图路径（模型引用贴图的权威来源）。
        // 用工作列表：扫描中新恢复的名字（如脚本引用的模型）若本身
        // 可扫描，也入队继续 —— 引用链条可以任意深。
        let mut queue: Vec<String> = self
            .names
            .iter()
            .filter(|n| {
                knownnames::is_scannable(n) || n.to_lowercase().ends_with(".mdx")
            })
            .cloned()
            .collect();
        let mut i = 0usize;
        while i < queue.len() {
            let name = queue[i].clone();
            i += 1;
            let names_before = self.names.len();
            let Ok(data) = self.read_file(&name) else { continue };
            let paths = if name.to_lowercase().ends_with(".mdx") {
                knownnames::mdx_texture_paths(&data)
            } else {
                knownnames::extract_candidate_paths(&data)
            };
            for path in paths {
                // 模型引用做 .mdl↔.mdx 互替（引擎等价）。
                self.try_recover_model_variants(&path);
            }
            for n in &self.names[names_before..] {
                let enqueueable = knownnames::is_scannable(n)
                    || n.to_lowercase().ends_with(".mdx");
                if enqueueable && !queue.iter().any(|q| q.eq_ignore_ascii_case(n)) {
                    queue.push(n.clone());
                }
            }
        }

        // War3 资源惯例启发式，迭代到不动点（新恢复的名字可能再
        // 引出下一层资源）：同名多扩展 × 同目录/根目录双向反推
        //（模型↔贴图互换角色），以及禁用图标的 dis 前缀变体。
        for _ in 0..4 {
            if self.heuristic_round() == 0 {
                break;
            }
        }

        self.names.len() - before
    }

    /// 一轮资源惯例启发式，返回新恢复的名字数：
    /// * 同名规律 —— 对每个已命名的资源（模型 / 贴图皆可），以其
    ///   「完整路径 + 各扩展名」与「基础名（根目录）+ 各扩展名」
    ///   双向试探（hero.mdx ↔ hero.blp、贴图反推模型同理）；
    /// * 禁用图标 —— `xxx.blp` 与同目录 `disxxx.blp` 成对
    ///   （`CommandButtons\BTN*` ↔ `CommandButtonsDisabled\DISBTN*`）。
    fn heuristic_round(&mut self) -> usize {
        let before = self.names.len();

        let resources: Vec<String> = self
            .names
            .iter()
            .filter(|n| {
                let l = n.to_lowercase();
                l.ends_with(".mdx")
                    || l.ends_with(".mdl")
                    || l.ends_with(".blp")
                    || l.ends_with(".tga")
            })
            .cloned()
            .collect();
        for resource in resources {
            let Some((stem_full, _)) = resource.rsplit_once('.') else {
                continue;
            };
            // 基础名（去目录）：War3 资源按「基础名 + 扩展名」匹配，
            // 贴图可以与模型不同目录（改图常见布局是模型在
            // war3mapImported\、贴图在根目录），根目录变体一并试探。
            let base = stem_full
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or(stem_full);
            for ext in ["blp", "tga", "png", "jpg", "dds", "mdx", "mdl"] {
                let candidate = format!("{stem_full}.{ext}");
                self.try_recover_name(&candidate);
                let rooted = format!("{base}.{ext}");
                self.try_recover_name(&rooted);
            }
        }

        let icons: Vec<String> = self
            .names
            .iter()
            .filter(|n| {
                let l = n.to_lowercase();
                l.ends_with(".blp") || l.ends_with(".tga")
            })
            .cloned()
            .collect();
        for icon in icons {
            let (dir, file) = match icon.rsplit_once('\\') {
                Some((d, f)) => (format!("{d}\\"), f.to_string()),
                None => (String::new(), icon.clone()),
            };
            let alt = if file.to_lowercase().starts_with("dis") {
                format!("{dir}{}", &file[3..])
            } else {
                format!("{dir}dis{file}")
            };
            self.try_recover_name(&alt);
        }

        self.names.len() - before
    }

    /// 试探单个候选名：hash 表命中且尚未收录时登记。
    /// MPQ 系统文件（`(listfile)` 等）除外 —— 它们由 [`Archive::entries`]
    /// 经 [`SPECIAL_FILES`] 单独标注，不属于 `file_names()`。
    fn try_recover_name(&mut self, name: &str) {
        if SPECIAL_FILES.iter().any(|s| s.eq_ignore_ascii_case(name)) {
            return;
        }
        if self.has_file(name)
            && !self
                .names
                .iter()
                .any(|n| n.eq_ignore_ascii_case(name))
        {
            self.names.push(name.to_string());
        }
    }

    /// 试探候选名及其**模型扩展名互替**变体（`.mdl` ↔ `.mdx`）。
    /// War3 引擎查找模型引用时两种扩展等价（数据表里写 `x.mdl`、
    /// MPQ 里存的常是 `x.mdx`），对象数据 / SLK 的引用因此需要
    /// 双向试探才能对上 hash 表。
    fn try_recover_model_variants(&mut self, name: &str) {
        self.try_recover_name(name);
        let swap = |ext: &str, other: &str, name: &str| {
            let lower = name.to_lowercase();
            let suffix = format!(".{ext}");
            lower
                .ends_with(&suffix)
                .then(|| format!("{}.{other}", &name[..name.len() - suffix.len()]))
        };
        if let Some(alt) = swap("mdl", "mdx", name).or_else(|| swap("mdx", "mdl", name)) {
            self.try_recover_name(&alt);
        }
    }

    /// 用外部候选文件名清单试探 hash 表恢复文件名 —— 例如
    /// MPQMaster 的 Listfiles 文本或其他游戏 / 社区维护的清单。
    /// 内置魔兽清单之外的补充途径；返回新恢复出的名字数。
    pub fn recover_names_with<'a, I>(&mut self, names: I) -> usize
    where
        I: IntoIterator<Item = &'a str>,
    {
        let before = self.names.len();
        for name in names {
            self.try_recover_name(name);
        }
        self.names.len() - before
    }

    /// 对仍未命名的条目按内容识别类型（魔法字节），把后缀缓存到
    /// [`FileEntry::guessed_ext`]。跳过加密条目（key 不可知）与
    /// 过大的条目（探测预算有限）。
    fn guess_unnamed_kinds(&mut self) {
        /// 单个条目与总读取的预算上限。
        const PER_FILE_LIMIT: u32 = 4 << 20;
        const TOTAL_LIMIT: u64 = 64 << 20;

        let targets: Vec<(TableSource, usize)> = self
            .entries()
            .into_iter()
            .filter(|e| e.name.is_none() && e.exists() && !e.is_encrypted() && e.size > 0)
            .filter(|e| e.compressed_size <= PER_FILE_LIMIT)
            .map(|e| (e.source, e.block_index))
            .collect();

        let mut total: u64 = 0;
        for key in targets {
            if total > TOTAL_LIMIT {
                break;
            }
            // 经 self.entries() 复原该条目再读取。
            let entry = match key.0 {
                TableSource::Classic => {
                    // locale 无关紧要：只影响元数据，不影响内容。
                    self.file_entry_from_classic(key.1, 0)
                }
                TableSource::Bet => self.file_entry_from_bet(key.1),
            };
            let Some(entry) = entry else { continue };
            total += entry.compressed_size as u64;
            if let Ok(data) = self.read_entry(&entry) {
                if let Some(ext) = knownnames::detect_ext(&data) {
                    self.guessed.insert(key, ext);
                }
            }
        }
    }

    /// 扫描模型条目的 TEXS 贴图引用（模型引用贴图的权威来源）：
    /// 具名模型按名读取；未命名但按内容识别为 `.mdx` 的条目按
    /// key 读取。已扫过的条目（按 key）缓存跳过。返回新恢复数。
    fn scan_model_textures(&mut self) -> usize {
        let before = self.names.len();
        let targets: Vec<(TableSource, usize)> = self
            .entries()
            .into_iter()
            .filter(|e| {
                e.exists()
                    && (e.name
                        .as_ref()
                        .is_some_and(|n| n.to_lowercase().ends_with(".mdx"))
                        || e.guessed_ext == Some("mdx"))
            })
            .filter(|e| !self.scanned_models.contains(&(e.source, e.block_index)))
            .map(|e| (e.source, e.block_index))
            .collect();
        for key in targets {
            self.scanned_models.insert(key);
            let entry = match key.0 {
                TableSource::Classic => self.file_entry_from_classic(key.1, 0),
                TableSource::Bet => self.file_entry_from_bet(key.1),
            };
            let Some(entry) = entry else { continue };
            if let Ok(data) = self.read_entry(&entry) {
                for path in knownnames::mdx_texture_paths(&data) {
                    self.try_recover_name(&path);
                }
            }
        }
        self.names.len() - before
    }

    fn file_table_len(&self) -> usize {
        if let Some(c) = &self.classic {
            if !c.block_table.is_empty() {
                return c.block_table.len();
            }
        }
        self.bet.as_ref().map_or(0, |b| b.entry_count as usize)
    }

    fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    // -- 公开访问器 ----------------------------------------------------

    /// 此归档的 MPQ 格式版本（1–4）。
    pub fn version(&self) -> FormatVersion {
        self.version
    }

    /// 解析出的（原始）MPQ 头。
    pub fn header(&self) -> &MpqHeader {
        &self.header
    }

    /// sector 大小（字节）。
    pub fn sector_size(&self) -> u32 {
        self.sector_size
    }

    /// 归档在所在文件中的偏移。
    pub fn archive_offset(&self) -> u64 {
        self.base
    }

    /// 打开归档时发现的非致命问题。
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// 读取文件数据时遇到的非致命问题（例如 sector CRC 不匹配）；
    /// 保留最近 64 条。
    pub fn read_warnings(&self) -> Vec<String> {
        self.read_warnings.borrow().iter().cloned().collect()
    }

    /// 来自 `(listfile)` 的文件名（缺失时可能为空）。
    pub fn file_names(&self) -> &[String] {
        &self.names
    }

    /// 是否仍有未命名的文件条目 —— 用于决定是否值得应用外部
    /// listfile 清单（完整 listfile 的归档无需再试探）。
    pub fn has_unnamed_entries(&self) -> bool {
        self.entries().iter().any(|e| e.name.is_none())
    }

    /// 解析出的 `(attributes)`（存在且可读时）。
    pub fn attributes(&self) -> Option<&Attributes> {
        self.attributes.as_ref()
    }

    /// 解码后的经典表（crate 内部：供修改器使用，以保留原始的
    /// hash 表槽位、locale 和无名块）。
    pub(crate) fn classic_tables(&self) -> Option<&ClassicTables> {
        self.classic.as_ref()
    }

    /// 解码后的 BET 表（crate 内部：供修改器使用）。
    pub(crate) fn bet_table(&self) -> Option<&BetTable> {
        self.bet.as_ref()
    }

    /// 是否正在使用 HET/BET 表对。
    pub fn uses_het_bet(&self) -> bool {
        self.het.is_some() && self.bet.is_some()
    }

    // -- 密码保护（自定义 AEAD 扩展） --------------------------

    /// 归档是否带有 `(password)` 保险库，即其文件是否以密码 AEAD 方案
    /// 封装。无论如何文件*名字*始终可读；读取内容需要
    /// [`Archive::unlock`]。
    pub fn is_password_protected(&self) -> bool {
        self.has_file(passcrypt::PASSWORD_FILE_NAME)
    }

    /// 是否持有正确的主密钥（此前成功执行过
    /// [`Archive::unlock`] / [`Archive::unlock_with_master_key`]）。
    pub fn is_unlocked(&self) -> bool {
        self.master_key.is_some()
    }

    /// 已解封的主密钥（归档已解锁时）。
    pub fn master_key(&self) -> Option<[u8; 32]> {
        self.master_key
    }

    /// 读取并解析 `(password)` 保险库（与归档当前的锁定状态无关 ——
    /// 保险库本身以明文存储）。
    fn password_vault(&self) -> Result<passcrypt::Vault> {
        let bytes = self.read_file(passcrypt::PASSWORD_FILE_NAME)?;
        passcrypt::Vault::parse(&bytes)
    }

    /// 针对保险库的 AEAD 校验器验证密码，并保留已解封的主密钥供后续
    /// 读取使用。密码错误时以 [`Error::PasswordMismatch`] 失败。
    pub fn unlock(&mut self, password: &str) -> Result<()> {
        let master = self.password_vault()?.open(password.as_bytes())?;
        self.master_key = Some(master);
        Ok(())
    }

    /// 安装从别处获得的主密钥（例如 GUI 在重新加载后保留的密钥）；
    /// 它必须通过校验器验证，否则归档保持锁定并返回
    /// [`Error::PasswordMismatch`]。
    pub fn unlock_with_master_key(&mut self, master: &[u8; 32]) -> Result<()> {
        let vault = self.password_vault()?;
        if !vault.verify_master(master) {
            return Err(Error::PasswordMismatch);
        }
        self.master_key = Some(*master);
        Ok(())
    }

    // -- 查找 ---------------------------------------------------------------

    fn file_entry_from_classic(&self, index: usize, locale: u16) -> Option<FileEntry> {
        let classic = self.classic.as_ref()?;
        let block = classic.block_table.get(index)?;
        Some(FileEntry {
            name: None,
            guessed_ext: self.guessed.get(&(TableSource::Classic, index)).copied(),
            locale,
            block_index: index,
            offset: classic.block_offset(index),
            compressed_size: block.csize,
            size: block.fsize,
            flags: block.flags,
            source: TableSource::Classic,
        })
    }

    fn file_entry_from_bet(&self, index: usize) -> Option<FileEntry> {
        let bet = self.bet.as_ref()?;
        if index as u32 >= bet.entry_count {
            return None;
        }
        let rec = bet.record(index as u32);
        Some(FileEntry {
            name: None,
            guessed_ext: self.guessed.get(&(TableSource::Bet, index)).copied(),
            locale: 0,
            block_index: index,
            offset: rec.file_pos,
            compressed_size: rec.cmp_size,
            size: rec.file_size,
            flags: rec.flags,
            source: TableSource::Bet,
        })
    }

    /// 按名字查找文件；返回未附带名字的条目。
    fn find_entry(&self, name: &str) -> Option<FileEntry> {
        // 经典 hash table 优先（与 StormLib 一致）。
        if let Some(classic) = &self.classic {
            if !classic.hash_table.is_empty() {
                if let Some((hash, _block)) = classic.find(name) {
                    let idx = hash.block_index as usize;
                    if idx < classic.block_table.len() {
                        return self.file_entry_from_classic(idx, hash.locale);
                    }
                }
            }
        }

        // HET+BET 回退路径。
        if let (Some(het), Some(bet)) = (&self.het, &self.bet) {
            let file_hash = jenkins::file_name_hash(name);
            let masked = (file_hash & Self::het_and_mask(het)) | Self::het_or_mask(het);
            let hash1 = (masked >> (het.name_hash_bit_size - 8)) as u8;
            for index in het.lookup(file_hash) {
                if bet.full_name_hash(hash1, index) == masked {
                    return self.file_entry_from_bet(index as usize);
                }
            }
        }
        None
    }

    fn het_and_mask(het: &HetTable) -> u64 {
        if het.name_hash_bit_size == 64 {
            u64::MAX
        } else {
            (1u64 << het.name_hash_bit_size) - 1
        }
    }

    fn het_or_mask(het: &HetTable) -> u64 {
        1u64 << (het.name_hash_bit_size - 1)
    }

    /// 归档中是否存在该名字的文件。
    pub fn has_file(&self, name: &str) -> bool {
        self.find_entry(name).is_some_and(|e| e.exists())
    }

    /// 按名字查找文件；返回附带名字的条目，
    /// 可直接交给 [`Archive::read_entry`] 使用。
    pub fn find_file(&self, name: &str) -> Option<FileEntry> {
        let mut entry = self.find_entry(name)?;
        entry.name = Some(name.to_string());
        Some(entry)
    }

    /// 按名字读取文件。
    pub fn read_file(&self, name: &str) -> Result<Vec<u8>> {
        let mut entry = self
            .find_entry(name)
            .ok_or_else(|| Error::FileNotFound(name.to_string()))?;
        entry.name = Some(name.to_string());
        self.read_entry(&entry)
    }

    /// 列出所有文件：文件表中的条目，已知名字的附带名字。
    pub fn entries(&self) -> Vec<FileEntry> {
        let mut result: Vec<FileEntry> = Vec::new();

        match (&self.classic, &self.bet) {
            (Some(classic), _) if !classic.block_table.is_empty() => {
                // 经典表：遍历 hash 条目，从而正确处理 locale 信息
                // 和重复名字。
                let mut seen = vec![false; classic.block_table.len()];
                for hash in &classic.hash_table {
                    if hash.is_free() {
                        continue;
                    }
                    let idx = hash.block_index as usize;
                    if idx >= classic.block_table.len() || seen[idx] {
                        continue;
                    }
                    seen[idx] = true;
                    if let Some(entry) = self.file_entry_from_classic(idx, hash.locale) {
                        result.push(entry);
                    }
                }
            }
            (_, Some(bet)) => {
                for i in 0..bet.entry_count {
                    if let Some(entry) = self.file_entry_from_bet(i as usize) {
                        result.push(entry);
                    }
                }
            }
            _ => {}
        }

        // 附上名字：先处理 (listfile) 条目，再处理已知的特殊文件。
        for name in self
            .names
            .iter()
            .map(|s| s.as_str())
            .chain(SPECIAL_FILES)
        {
            if let Some(mut entry) = self.find_entry(name) {
                if entry.exists() {
                    entry.name = Some(name.to_string());
                    if let Some(slot) = result
                        .iter_mut()
                        .find(|e| e.block_index == entry.block_index && e.source == entry.source)
                    {
                        if slot.name.is_none() {
                            slot.name = entry.name.clone();
                        }
                    }
                }
            }
        }
        result
    }

    /// 按给定的 [`FileEntry`]（来自 [`Archive::entries`]）读取文件。
    pub fn read_entry(&self, entry: &FileEntry) -> Result<Vec<u8>> {
        if !entry.exists() {
            if entry.is_deleted_marker() {
                return Err(Error::FileDeleted);
            }
            return Err(Error::FileNotFound(
                entry.name.clone().unwrap_or_default(),
            ));
        }

        self.verify_chunk_md5s(entry);

        // 密码 AEAD 文件（自定义扩展）从不使用 storm 加密：
        // 其密钥派生自主密钥 + 块索引。
        let pwd_aead = entry.flags & FILE_PASSWORD_AEAD != 0;
        let aead_key: Option<[u8; 32]> = if pwd_aead {
            match self.master_key {
                Some(ref master) => Some(passcrypt::derive_file_key(master, entry.block_index)),
                None => return Err(Error::PasswordRequired),
            }
        } else {
            None
        };

        let key = if entry.is_encrypted() && !pwd_aead {
            match &entry.name {
                Some(name) => crypt::file_key(
                    name,
                    entry.flags & FILE_KEY_V2 != 0,
                    entry.offset,
                    entry.size,
                ),
                None => return Err(Error::UnknownFileKey),
            }
        } else {
            0
        };

        let raw_start = self.base + entry.offset;

        if entry.is_single_unit() {
            let raw = self.read_at_plain(raw_start, entry.compressed_size as usize)?;
            let mut data = raw;
            if let Some(aead) = aead_key {
                data = passcrypt::open_sector(&aead, entry.block_index, 0, &data)?;
            } else if entry.is_encrypted() {
                crypt::decrypt_bytes(&mut data, key);
            }
            if entry.is_compressed() {
                data = if entry.flags & FILE_COMPRESS != 0 {
                    compression::decompress_masked(&data, entry.size as usize)?
                } else if entry.flags & FILE_IMPLODE != 0 {
                    compression::decompress_imploded(&data, entry.size as usize)?
                } else {
                    data
                };
            }
            data.truncate(entry.size as usize);
            return Ok(data);
        }

        // 基于 sector 的文件。
        let sector_size = self.sector_size as u64;
        let nsec = (entry.size as u64).div_ceil(sector_size) as u32;
        let compressed = entry.is_compressed();
        let encrypted = entry.is_encrypted();
        let has_crc = entry.flags & FILE_SECTOR_CRC != 0;

        // sector 偏移表只存在于压缩文件中。密码 AEAD 块以未加密方式
        // 存储它（只有 sector 本身是机密）。
        let offsets: Vec<u32> = if compressed {
            let table_dwords = nsec as usize + 1 + if has_crc { nsec as usize } else { 0 };
            let mut raw = self.read_at_plain(raw_start, table_dwords * 4)?;
            if encrypted && !pwd_aead {
                crypt::decrypt_bytes(&mut raw, key.wrapping_sub(1));
            }
            raw.chunks_exact(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()
        } else {
            Vec::new()
        };

        if compressed {
            // 按 StormLib 做健全性检查：偏移单调递增，sector 大小合理。
            // AEAD sector 带有 16 字节标签，因此留有余量。
            let arch_size = entry.compressed_size;
            let max_sector = if pwd_aead {
                self.sector_size + TAG_LEN as u32
            } else {
                self.sector_size
            };
            for i in 0..nsec as usize {
                let a = offsets.get(i).copied().unwrap_or(u32::MAX);
                let b = offsets.get(i + 1).copied().unwrap_or(u32::MAX);
                if b < a || (b - a) > max_sector || b > arch_size {
                    return Err(Error::Corrupt("implausible sector offset table"));
                }
            }
        }

        // 已封装的 sector 是变长的（明文 + 标签）：没有偏移表就无法
        // 得知其边界。写入方总是以压缩方式（即带偏移表）存储 AEAD 文件。
        if pwd_aead && !compressed {
            return Err(Error::Corrupt(
                "password AEAD file without sector offset table",
            ));
        }

        let mut out: Vec<u8> = Vec::with_capacity(entry.size as usize);
        for i in 0..nsec {
            let expected = (entry.size as u64 - i as u64 * sector_size).min(sector_size) as u32;

            let (raw_off, raw_len) = if compressed {
                let a = offsets[i as usize];
                let b = offsets[(i + 1) as usize];
                (a as u64, b.saturating_sub(a))
            } else {
                (i as u64 * sector_size, expected)
            };

            if raw_len == 0 {
                // 稀疏 / 缺失的 sector：以零填充。
                out.extend(std::iter::repeat_n(0u8, expected as usize));
                continue;
            }

            let mut raw = self.read_at_plain(raw_start + raw_off, raw_len as usize)?;
            if let Some(aead) = aead_key {
                raw = passcrypt::open_sector(&aead, entry.block_index, i, &raw)?;
            } else if encrypted {
                crypt::decrypt_bytes(&mut raw, key.wrapping_add(i));
            }

            let sector = if compressed && raw.len() < expected as usize {
                if entry.flags & FILE_COMPRESS != 0 {
                    compression::decompress_masked(&raw, expected as usize)?
                } else {
                    compression::decompress_imploded(&raw, expected as usize)?
                }
            } else {
                raw
            };

            if sector.len() < expected as usize {
                out.extend(std::iter::repeat_n(0, expected as usize - sector.len()));
            }
            out.extend_from_slice(&sector[..sector.len().min(expected as usize)]);

            // 可选的逐 sector 校验和：解压后数据的 CRC32，
            // 不匹配会被报告，但对整个读取而言不是致命错误。
            if has_crc && self.read_warnings.borrow().len() < 64 {
                let crc_index = (nsec as usize + 1) + i as usize;
                if let Some(&stored) = offsets.get(crc_index) {
                    if stored != 0 && stored != 0xFFFF_FFFF {
                        let take = expected as usize;
                        let sector_crc = crc32(&sector[..sector.len().min(take)]);
                        if sector_crc != stored {
                            self.read_warnings
                                .borrow_mut()
                                .push_back(format!("{}: sector {i} CRC mismatch", entry.display_name()));
                        }
                    }
                }
            }
        }

        out.truncate(entry.size as usize);
        Ok(out)
    }

    /// v4 raw-chunk 校验和：当头设置了 `raw_chunk_size` 时，每个文件的
    /// 已存储（原始）数据之后跟着 `ceil(csize / chunk)` 个 MD5，
    /// 针对每块*存储后*的字节（不计入 `csize` —— 与 StormLib 的
    /// `WriteMemDataMD5` 写出的布局相同）。不匹配会记为读取警告，
    /// 与 sector CRC 一样。越过文件末尾的数组会被跳过，
    /// 而不是针对零填充做校验。
    fn verify_chunk_md5s(&self, entry: &FileEntry) {
        let chunk = self.header.raw_chunk_size;
        if self.version != FormatVersion::V4
            || chunk == 0
            || entry.compressed_size == 0
            || self.read_warnings.borrow().len() >= 64
        {
            return;
        }
        let start = self.base + entry.offset;
        let raw_len = entry.compressed_size as u64;
        let count = raw_len.div_ceil(chunk as u64) as usize;
        let md5_array = raw_len as usize + count * 16;
        if start + md5_array as u64 > self.file_len {
            return;
        }
        let Ok(all) = self.read_at_plain(start, md5_array) else {
            return;
        };
        let name = entry.display_name();
        for i in 0..count {
            let data = &all[i * chunk as usize..((i + 1) * chunk as usize).min(raw_len as usize)];
            let expected: [u8; 16] = all[raw_len as usize + i * 16..raw_len as usize + (i + 1) * 16]
                .try_into()
                .expect("slice of 16");
            if digest::md5_digest(data) != expected {
                self.read_warnings
                    .borrow_mut()
                    .push_back(format!("{name}: raw chunk {i} MD5 mismatch"));
            }
        }
    }

    /// 取回底层 reader（位置未定义）。
    pub fn into_inner(self) -> R {
        self.reader.into_inner()
    }
}

fn read_exact_or_eof<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = reader.read(&mut buf[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(())
}

/// 标准 CRC-32（IEEE），用于可选的 sector 校验和。
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    crc ^ 0xFFFF_FFFF
}
