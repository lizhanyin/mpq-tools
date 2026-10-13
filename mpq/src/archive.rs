//! The [`Archive`] type: opening MPQ archives, finding and reading files.

use std::collections::VecDeque;
use std::cell::RefCell;
use std::io::{Read, Seek, SeekFrom};
use std::ops::DerefMut;

use crate::attrs::Attributes;
use crate::compression;
use crate::crypt;
use crate::error::{Error, Result};
use crate::header::{FormatVersion, MpqHeader, UserDataHeader, MAX_HEADER_READ, MPQ_MAGIC, USERDATA_MAGIC};
use crate::hetbet::{load_ext_table, BetTable, HetTable, BET_SIGNATURE, HET_SIGNATURE};
use crate::jenkins;
use crate::crypt::{KEY_BLOCK_TABLE, KEY_HASH_TABLE};
use crate::tables::{
    load_mpq_table, parse_block_table, parse_hash_table, ClassicTables, FILE_COMPRESS,
    FILE_COMPRESS_MASK, FILE_DELETE_MARKER, FILE_ENCRYPTED, FILE_EXISTS, FILE_IMPLODE,
    FILE_KEY_V2, FILE_SECTOR_CRC, FILE_SINGLE_UNIT,
};

/// Names of internal files that may or may not be present (all optional).
pub const SPECIAL_FILES: [&str; 4] = ["(listfile)", "(attributes)", "(signature)", "(userdata)"];

/// Where a file entry's data came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TableSource {
    /// Classic block table (index into it stored in `block_index`).
    Classic,
    /// BET table (index into it stored in `block_index`).
    Bet,
}

/// A file inside the archive, unified across classic and BET tables.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Name if known (from `(listfile)` or a probed internal name).
    pub name: Option<String>,
    pub locale: u16,
    /// Index into the source table.
    pub block_index: usize,
    /// Data offset relative to the archive start.
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
    pub fn is_compressed(&self) -> bool {
        self.flags & FILE_COMPRESS_MASK != 0
    }
    /// Name for display; `<unnamed #index>` when unknown.
    pub fn display_name(&self) -> String {
        match &self.name {
            Some(n) => n.clone(),
            None => format!("<unnamed #{}>", self.block_index),
        }
    }
}

/// An opened MPQ archive.
pub struct Archive<R: Read + Seek> {
    reader: RefCell<R>,
    /// Offset of the MPQ header inside the containing file.
    base: u64,
    file_len: u64,
    header: MpqHeader,
    version: FormatVersion,
    sector_size: u32,
    classic: Option<ClassicTables>,
    het: Option<HetTable>,
    bet: Option<BetTable>,
    names: Vec<String>,
    attributes: Option<Attributes>,
    warnings: Vec<String>,
    /// Non-fatal issues hit while reading file data (bounded ring).
    read_warnings: RefCell<VecDeque<String>>,
}

impl Archive<std::fs::File> {
    /// Open an archive from a file path.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let file = std::fs::File::open(path.as_ref())?;
        Archive::new(file)
    }
}

impl<R: Read + Seek> Archive<R> {
    /// Open an archive from any reader. The MPQ header is searched at the
    /// start of the stream, behind a `MPQ\x1B` user-data header, or by
    /// scanning (up to 128 MB, 0x200-aligned) for embedded archives.
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
            attributes: None,
            warnings: Vec::new(),
            read_warnings: RefCell::new(VecDeque::new()),
        };
        archive.sector_size = archive.header.sector_size();
        archive.load_tables()?;
        archive.load_optional_metadata();
        Ok(archive)
    }

    // -- opening helpers ---------------------------------------------------

    /// Find the byte offset of the MPQ header within the stream.
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

        // 1. Plain archive at offset 0.
        read_at(reader, 0, &mut probe)?;
        if magic_of(&probe) == MPQ_MAGIC {
            return Ok(0);
        }

        // 2. User data header at offset 0 pointing at the real header.
        if let Some(ud) = UserDataHeader::parse(&probe) {
            let target = ud.archive_offset as u64;
            if target + 0x20 <= file_len {
                read_at(reader, target, &mut probe)?;
                if magic_of(&probe) == MPQ_MAGIC {
                    return Ok(target);
                }
            }
        }

        // 3. Scan (0x200-aligned, like storm.dll) for an embedded archive.
        //    A 1 MiB window is read at a time, with 0x200 bytes of overlap
        //    so candidates near the window end can still be parsed.
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

    /// Load hash/block/hi-block and HET/BET tables with per-table tolerance:
    /// every table failure downgrades to a warning as long as one usable
    /// hash-index (classic hash or HET) and one file table (block or BET)
    /// remain.
    fn load_tables(&mut self) -> Result<()> {
        let header = self.header.clone();
        let file_len = self.file_len;
        let base = self.base;
        let archive_end = base + header.archive_size(file_len - base.min(file_len));

        // Work out how many bytes each classic table occupies on disk.
        let (hash_stored, block_stored) = self.stored_table_sizes(archive_end);

        // --- classic tables -------------------------------------------------
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
                    Ok(t) => classic.hash_table = t,
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
                    Ok(t) => classic.block_table = t,
                    Err(e) => self.warn(format!("block table unusable: {e}")),
                },
                Err(e) => self.warn(format!("block table unreadable: {e}")),
            }
        }

        if header.hi_block_table_pos != 0 && !classic.block_table.is_empty() {
            let count = classic.block_table.len();
            match self.read_at_plain(base + header.hi_block_table_pos, count * 2) {
                Ok(buf) => {
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

        // --- HET/BET tables (v3+) -------------------------------------------
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
                        Ok(t) => self.het = Some(t),
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
                        Ok(t) => self.bet = Some(t),
                        Err(e) => self.warn(format!("BET table unusable: {e}")),
                    },
                    Err(e) => self.warn(format!("BET table unreadable: {e}")),
                }
            }
        }

        // --- viability ------------------------------------------------------
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

        // A HET index without a BET file table cannot resolve names.
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

    /// On-disk sizes of the classic hash/block tables, derived per version
    /// the way StormLib does: v1 uses the uncompressed size, v2/v3 compute
    /// them from the gaps between neighbouring tables, v4 stores them.
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
                // Tables are laid out in ascending order: hash, block,
                // hi-block; sizes are the gaps between them. Compressed
                // tables are the only case where the stored size matters,
                // and it is always <= the uncompressed size.
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
                // BET sits directly below whatever comes next (hash table
                // or the archive end).
                let upper = if h.hash_table_pos != 0 {
                    h.hash_table_pos
                } else {
                    self.lower_table_boundary(archive_end)
                };
                upper.saturating_sub(h.bet_table_pos)
            }
        }
    }

    /// The lowest of the "upper" boundaries (hash / block / hi-block / end).
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

    /// Load `(listfile)` and `(attributes)` — both entirely optional.
    fn load_optional_metadata(&mut self) {
        // listfile first: read_file needs an intact table state only.
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
            self.warn("no (listfile); file names unknown");
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

    // -- public accessors ----------------------------------------------------

    /// MPQ format version (1–4) of this archive.
    pub fn version(&self) -> FormatVersion {
        self.version
    }

    /// The parsed (raw) MPQ header.
    pub fn header(&self) -> &MpqHeader {
        &self.header
    }

    /// Sector size in bytes.
    pub fn sector_size(&self) -> u32 {
        self.sector_size
    }

    /// Offset of the archive within the containing file.
    pub fn archive_offset(&self) -> u64 {
        self.base
    }

    /// Non-fatal issues noticed while opening the archive.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Non-fatal issues hit while reading file data (e.g. sector CRC
    /// mismatches); keeps the most recent 64 entries.
    pub fn read_warnings(&self) -> Vec<String> {
        self.read_warnings.borrow().iter().cloned().collect()
    }

    /// File names from `(listfile)` (may be empty when absent).
    pub fn file_names(&self) -> &[String] {
        &self.names
    }

    /// Parsed `(attributes)` if present and readable.
    pub fn attributes(&self) -> Option<&Attributes> {
        self.attributes.as_ref()
    }

    /// Decoded classic tables (crate-internal: used by the modifier to
    /// preserve original hash-table slots, locales and unnamed blocks).
    pub(crate) fn classic_tables(&self) -> Option<&ClassicTables> {
        self.classic.as_ref()
    }

    /// Decoded BET table (crate-internal: used by the modifier).
    pub(crate) fn bet_table(&self) -> Option<&BetTable> {
        self.bet.as_ref()
    }

    /// Whether a HET/BET table pair is in use.
    pub fn uses_het_bet(&self) -> bool {
        self.het.is_some() && self.bet.is_some()
    }

    // -- lookup ---------------------------------------------------------------

    fn file_entry_from_classic(&self, index: usize, locale: u16) -> Option<FileEntry> {
        let classic = self.classic.as_ref()?;
        let block = classic.block_table.get(index)?;
        Some(FileEntry {
            name: None,
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
            locale: 0,
            block_index: index,
            offset: rec.file_pos,
            compressed_size: rec.cmp_size,
            size: rec.file_size,
            flags: rec.flags,
            source: TableSource::Bet,
        })
    }

    /// Look a file up by name; returns the entry without a name attached.
    fn find_entry(&self, name: &str) -> Option<FileEntry> {
        // Classic hash table takes precedence (mirrors StormLib).
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

        // HET+BET fallback.
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

    /// Whether a file with this name exists in the archive.
    pub fn has_file(&self, name: &str) -> bool {
        self.find_entry(name).is_some_and(|e| e.exists())
    }

    /// Look a file up by name; returns the entry with the name attached
    /// so it can be fed to [`Archive::read_entry`].
    pub fn find_file(&self, name: &str) -> Option<FileEntry> {
        let mut entry = self.find_entry(name)?;
        entry.name = Some(name.to_string());
        Some(entry)
    }

    /// Read a file by name.
    pub fn read_file(&self, name: &str) -> Result<Vec<u8>> {
        let mut entry = self
            .find_entry(name)
            .ok_or_else(|| Error::FileNotFound(name.to_string()))?;
        entry.name = Some(name.to_string());
        self.read_entry(&entry)
    }

    /// List all files: entries from the file table with names attached
    /// where known.
    pub fn entries(&self) -> Vec<FileEntry> {
        let mut result: Vec<FileEntry> = Vec::new();

        match (&self.classic, &self.bet) {
            (Some(classic), _) if !classic.block_table.is_empty() => {
                // Classic table: iterate hash entries so that locale info
                // and duplicate names are respected.
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

        // Attach names: (listfile) entries first, then known special files.
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

    /// Read a file given its [`FileEntry`] (from [`Archive::entries`]).
    pub fn read_entry(&self, entry: &FileEntry) -> Result<Vec<u8>> {
        if !entry.exists() {
            if entry.is_deleted_marker() {
                return Err(Error::FileDeleted);
            }
            return Err(Error::FileNotFound(
                entry.name.clone().unwrap_or_default(),
            ));
        }

        let key = if entry.is_encrypted() {
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
            if entry.is_encrypted() {
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

        // Sector-based file.
        let sector_size = self.sector_size as u64;
        let nsec = (entry.size as u64).div_ceil(sector_size) as u32;
        let compressed = entry.is_compressed();
        let encrypted = entry.is_encrypted();
        let has_crc = entry.flags & FILE_SECTOR_CRC != 0;

        // Sector offset table only exists for compressed files.
        let offsets: Vec<u32> = if compressed {
            let table_dwords = nsec as usize + 1 + if has_crc { nsec as usize } else { 0 };
            let mut raw = self.read_at_plain(raw_start, table_dwords * 4)?;
            if encrypted {
                crypt::decrypt_bytes(&mut raw, key.wrapping_sub(1));
            }
            raw.chunks_exact(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()
        } else {
            Vec::new()
        };

        if compressed {
            // Sanity per StormLib: monotonic offsets, plausible sector sizes.
            let arch_size = entry.compressed_size;
            for i in 0..nsec as usize {
                let a = offsets.get(i).copied().unwrap_or(u32::MAX);
                let b = offsets.get(i + 1).copied().unwrap_or(u32::MAX);
                if b < a || (b - a) > self.sector_size || b > arch_size {
                    return Err(Error::Corrupt("implausible sector offset table"));
                }
            }
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
                // Sparse / absent sector: zero-fill.
                out.extend(std::iter::repeat_n(0u8, expected as usize));
                continue;
            }

            let mut raw = self.read_at_plain(raw_start + raw_off, raw_len as usize)?;
            if encrypted {
                crypt::decrypt_bytes(&mut raw, key.wrapping_add(i));
            }

            let sector = if compressed && raw_len < expected {
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

            // Optional per-sector checksums: CRC32 of the decompressed data,
            // mismatch is reported but not fatal for the whole read.
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

    /// Recover the underlying reader (position unspecified).
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

/// Standard CRC-32 (IEEE), used for the optional sector checksums.
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
