//! Modify MPQ archives: import (add / replace) and delete files.
//!
//! Strategy: every kept file's stored bytes are copied **verbatim**, so the
//! compression method, sector CRCs and encryption of existing files stay
//! untouched; only the container is rewritten:
//!
//! * new files are appended as freshly encoded blocks (zlib per sector,
//!   single-unit for small files, never encrypted)
//! * the classic hash/block tables are rebuilt — encrypted, uncompressed —
//!   and the header (v1–v3) points at the new layout
//! * `(listfile)` is regenerated; `(attributes)` and `(signature)` are
//!   dropped because their contents are invalidated by any modification
//!   (the same thing StormLib does)
//!
//! Consequences of the raw-copy strategy:
//!
//! * HET/BET tables are not rewritten — a modified archive relies on its
//!   classic tables (the header's HET/BET positions are cleared). Archives
//!   that were pure HET/BET get classic tables synthesised from their BET
//!   records and listfile names.
//! * When unnamed entries remain addressable by name (protected maps
//!   without a listfile), the classic hash table's **slot layout is
//!   preserved** (same table size, entries stay in place, deletions become
//!   `DELETED` markers) because the probe position of a hash entry cannot
//!   be recomputed without the name. When every kept entry is named, a
//!   fresh — possibly larger — table is built instead.
//! * Files encrypted with the key-v2 variant derive their key from the
//!   block offset; since their offset changes, they are re-encoded from
//!   their decoded content — by default re-encrypted with the key derived
//!   at their *new* offset ([`ModifyOptions::reencrypt_key_v2`]); disabling
//!   that option stores them compressed but unencrypted.
//! * v4 headers stay v4: the rebuilt classic hash/block tables and the
//!   header get fresh v4 MD5 checksums (over the stored, encrypted bytes —
//!   exactly what StormLib's `SaveMpqTable` computes); `raw_chunk_size` is
//!   zeroed because the per-file chunk MD5 arrays are not carried over.
//!   With [`ModifyOptions::target_version`] (see [`convert`]) the container
//!   is rewritten as the requested version instead, v4 included.
//! * Bytes in front of the archive (the `MPQ\x1B` user-data wrapper of
//!   Warcraft III / StarCraft II maps, or garbage before an embedded
//!   archive) and bytes behind it are preserved.

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
use crate::tables::{
    HashEntry, FILE_COMPRESS, FILE_ENCRYPTED, FILE_EXISTS, FILE_KEY_V2, FILE_SINGLE_UNIT,
    HASH_ENTRY_DELETED, HASH_ENTRY_FREE,
};
use crate::{FileEntry, TableSource};

/// Internal file names an import may not use (they are managed by the
/// modifier / the format itself).
const FORBIDDEN_IMPORT_NAMES: [&str; 4] =
    ["(listfile)", "(attributes)", "(signature)", "(userdata)"];

/// One requested deletion.
#[derive(Debug, Clone)]
pub enum Deletion {
    /// Delete every entry whose name matches (MPQ-normalized: case- and
    /// slash-insensitive). Names that do not exist are ignored.
    ByName(String),
    /// Delete exactly one table entry, identified like the GUI's selection
    /// keys — the only way to address unnamed entries.
    ByKey(TableSource, usize),
}

/// Knobs for [`modify_with`].
#[derive(Debug, Clone, Copy)]
pub struct ModifyOptions {
    /// Re-encrypt key-v2 encrypted files with the key derived at their
    /// *new* block offset (the offset is part of the key, so their bytes
    /// can never be copied verbatim). When false, those files are re-stored
    /// compressed but unencrypted. Ordinary (name-keyed) encrypted files
    /// are always copied encrypted, regardless of this option.
    pub reencrypt_key_v2: bool,
    /// Rewrite the container as this header version (v1–v4). `None` keeps
    /// the source version. Targeting the source's own version is a no-op.
    pub target_version: Option<FormatVersion>,
}

impl Default for ModifyOptions {
    fn default() -> Self {
        ModifyOptions {
            reencrypt_key_v2: true,
            target_version: None,
        }
    }
}

/// What a [`modify`] run changed.
#[derive(Debug, Clone, Default)]
pub struct ModifyReport {
    /// Names of files that were newly added.
    pub added: Vec<String>,
    /// Names of files that existed and were replaced by an import.
    pub replaced: Vec<String>,
    /// Display names of deleted entries (unnamed: `<unnamed #i>`).
    pub deleted: Vec<String>,
    /// Internal files removed because a modification invalidates them
    /// (`(attributes)`, `(signature)`).
    pub dropped: Vec<String>,
    /// key-v2 encrypted files that were re-encoded at their new offset.
    pub re_encoded: Vec<String>,
}

/// Result of a successful [`modify`] run.
#[derive(Debug, Clone)]
pub struct ModifyOutcome {
    /// The complete modified file (prefix before the archive included).
    pub bytes: Vec<u8>,
    pub report: ModifyReport,
}

/// How a block is written into the new archive.
enum Payload {
    /// Stored bytes copied verbatim + the original flags.
    Raw { bytes: Vec<u8>, flags: u32 },
    /// Content encoded fresh: zlib sectors / single unit, unencrypted (so
    /// the encoding never depends on the block position).
    Fresh { data: Vec<u8> },
    /// Content re-encoded *and* re-encrypted with the key-v2 key derived
    /// at the block's new offset — position-dependent, so encoded during
    /// layout once the offset is known.
    ReEncrypted { name: String, data: Vec<u8> },
}

/// Classic hash entry fields, for blocks that must be (re-)anchored by
/// name — additions, the rebuilt listfile, and BET-sourced entries.
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

/// One block of the output archive.
struct OutBlock {
    payload: Payload,
    fsize: u32,
    /// Name when known (drives the rebuilt listfile).
    name: Option<String>,
    /// Hash entries to anchor this block by name — additions, the rebuilt
    /// listfile, BET-sourced entries, and (in fresh-table mode) kept
    /// classic entries with their original locale variants.
    hashes: Vec<HashFields>,
    /// Kept classic block whose original hash slots are remapped instead
    /// (only meaningful in slot-preservation mode).
    classic_remap: bool,
}

/// Import files into an archive and/or delete entries, returning the full
/// modified file. See [`modify_with`] for the configurable variant.
pub fn modify(
    data: &[u8],
    deletions: &[Deletion],
    additions: &[(String, Vec<u8>)],
) -> Result<ModifyOutcome> {
    modify_with(data, deletions, additions, &ModifyOptions::default())
}

/// Convert an archive's container to another header version (v1–v4).
///
/// This is [`modify_with`] with no import/delete list and an explicit
/// [`ModifyOptions::target_version`]: every file's stored bytes are copied
/// verbatim into a freshly written container of the target version, so the
/// conversion succeeds whenever the source opens. Cases that cannot be
/// converted fail with the reason in the error:
///
/// * the source does not parse at all (try [`crate::repair`] first)
/// * a layout that no longer fits 32-bit offsets (> 4 GB)
///
/// Converting to the source's own version is a no-op that returns the input
/// unchanged. Consequences of the rewrite are the modifier's: HET/BET tables
/// are replaced by classic tables, `(attributes)`/`(signature)` are dropped
/// (see `report`), key-v2 files are re-encoded at their new offsets, and
/// bytes in front of / behind the archive are preserved. A v4 target gets a
/// fully checksummed v4 header (table MD5s + header MD5).
pub fn convert(data: &[u8], target: FormatVersion) -> Result<ModifyOutcome> {
    let options = ModifyOptions {
        target_version: Some(target),
        ..ModifyOptions::default()
    };
    modify_with(data, &[], &[], &options)
}

/// [`modify`] with explicit [`ModifyOptions`].
pub fn modify_with(
    data: &[u8],
    deletions: &[Deletion],
    additions: &[(String, Vec<u8>)],
    options: &ModifyOptions,
) -> Result<ModifyOutcome> {
    let archive = Archive::new(std::io::Cursor::new(data))?;
    let base = archive.archive_offset() as usize;
    let sector_size = archive.sector_size();
    let source_version = archive.version();
    let version = options.target_version.unwrap_or(source_version);
    // Rewriting to the source's own version would only shuffle bytes.
    let converting = options.target_version.is_some_and(|t| t != source_version);
    let sector_shift = archive.header().block_size;

    if !converting && deletions.is_empty() && additions.is_empty() {
        return Ok(ModifyOutcome {
            bytes: data.to_vec(),
            report: ModifyReport::default(),
        });
    }

    // --- normalize additions (a later duplicate supersedes earlier) -------
    let mut adds: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, bytes) in additions {
        let name = sanitize_name(name)?;
        match adds.iter().position(|(n, _)| same_name(n, &name)) {
            Some(i) => adds[i] = (name, bytes.clone()),
            None => adds.push((name, bytes.clone())),
        }
    }

    // --- snapshot the source tables ----------------------------------------
    // (source, index) -> name, from the unified entry list.
    let name_of: HashMap<(TableSource, usize), String> = archive
        .entries()
        .iter()
        .filter_map(|e| e.name.clone().map(|n| ((e.source, e.block_index), n)))
        .collect();

    let classic = archive.classic_tables();
    let bet = archive.bet_table();
    let use_classic_blocks = classic.is_some_and(|c| !c.block_table.is_empty());

    // Classic hash entries grouped by the block they point at (occupied
    // entries only) — used to match unnamed-but-addressable files.
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

    // --- enumerate source blocks -------------------------------------------
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
                continue; // ghost / free entry
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

    // --- decide which source blocks are dropped -----------------------------
    let mut dropped_keys: HashSet<(TableSource, usize)> = HashSet::new();
    let mut report = ModifyReport::default();

    for del in deletions {
        match del {
            Deletion::ByKey(s, i) => {
                if let Some(b) = src.iter().find(|b| b.source == *s && b.index == *i) {
                    report
                        .deleted
                        .push(b.name.clone().unwrap_or_else(|| format!("<unnamed #{i}>")));
                    dropped_keys.insert((*s, *i));
                }
            }
            Deletion::ByName(n) => {
                for b in &src {
                    if b.name.as_deref().is_some_and(|bn| same_name(bn, n))
                        && dropped_keys.insert((b.source, b.index))
                    {
                        report.deleted.push(b.name.clone().unwrap_or_default());
                    }
                }
                // Unnamed-but-addressable entries cannot be matched by
                // name here; the archive lookup below covers them.
                if let Some(e) = archive.find_file(n) {
                    if dropped_keys.insert((e.source, e.block_index)) {
                        report.deleted.push(n.clone());
                    }
                }
            }
        }
    }

    // Internal files the modifier manages or invalidates.
    for b in &src {
        if let Some(n) = &b.name {
            if n == "(attributes)" || n == "(signature)" {
                report.dropped.push(n.clone());
                dropped_keys.insert((b.source, b.index));
            } else if n == "(listfile)" {
                // Always rebuilt below; never copied.
                dropped_keys.insert((b.source, b.index));
            }
        }
    }

    // Additions replace matching entries — by attached name, by classic
    // hash pair (unnamed but addressable), or via the archive lookup
    // (HET/BET-sourced entries).
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

    // --- build output blocks ------------------------------------------------
    let mut blocks: Vec<OutBlock> = Vec::new();
    // old classic block index -> new block index (hash slot remapping)
    let mut remap: HashMap<usize, usize> = HashMap::new();
    // Re-encoded key-v2 files (name, original content) for the self-check.
    let mut reencoded: Vec<(String, Vec<u8>)> = Vec::new();

    for b in &src {
        if dropped_keys.contains(&(b.source, b.index)) {
            continue;
        }
        let start = (base as u64 + b.offset) as usize;
        let end = start.saturating_add(b.csize as usize).min(data.len());
        let raw = data[start.min(end)..end].to_vec();

        // key-v2 files derive their key from the block offset, which
        // changes: re-encode from decoded content instead of copying.
        let payload = if b.flags & FILE_KEY_V2 != 0 && b.name.is_some() {
            let entry = FileEntry {
                name: b.name.clone(),
                locale: 0,
                block_index: b.index,
                offset: b.offset,
                compressed_size: b.csize,
                size: b.fsize,
                flags: b.flags,
                source: b.source,
            };
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
                // Already unreadable in the source; copying keeps it that
                // way rather than failing the whole modification.
                Err(_) => Payload::Raw { bytes: raw, flags: b.flags },
            }
        } else {
            Payload::Raw { bytes: raw, flags: b.flags }
        };

        // Kept classic blocks keep their original hash slots (remapped)
        // when slot preservation is required; named BET blocks must be
        // re-anchored by name in the new classic hash table.
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
        blocks.push(OutBlock {
            payload: Payload::Fresh { data: bytes.clone() },
            fsize: bytes.len() as u32,
            name: Some(name.clone()),
            hashes: vec![HashFields::of(name)],
            classic_remap: false,
        });
    }

    // Rebuilt listfile: every kept / added name except MPQ internals.
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

    // A conversion must not silently orphan files: a BET-sourced entry whose
    // name cannot be recovered (pure HET/BET archive without a listfile)
    // would keep its bytes but lose name addressing in the rebuilt classic
    // tables. A plain modify tolerates that; an explicit version conversion
    // refuses with the reason instead.
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
        // A zero-file source cannot even carry a rebuilt listfile. When only
        // the container version changes, emit an empty target-version
        // archive instead of failing.
        if converting && adds.is_empty() {
            let hash_count =
                crate::create::normalize_hash_size(archive.header().hash_table_size.max(4));
            let inner = crate::create::empty_archive(version, hash_count, sector_shift);
            let mut out = Vec::with_capacity(base + inner.len());
            out.extend_from_slice(&data[..base]);
            out.extend_from_slice(&inner);
            // Bytes behind the original archive are preserved, like below.
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
            });
        }
        return Err(Error::InvalidData(
            "修改后归档不包含任何文件；至少需要保留一个条目".to_string(),
        ));
    }

    // --- lay out blocks, encode fresh ones -----------------------------------
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
    for b in &blocks {
        // The block position must be known before encoding: re-encrypted
        // key-v2 blocks derive their key from it.
        let pos = cursor;
        let (bytes, flags) = match &b.payload {
            Payload::Raw { bytes, flags } => (bytes.clone(), *flags),
            Payload::Fresh { data } => encode_fresh(data, sector_size),
            Payload::ReEncrypted { name, data } => {
                encode_fresh_reencrypted(name, data, sector_size, pos)
            }
        };
        cursor += bytes.len() as u64;
        cursor = (cursor + 3) & !3; // dword alignment, like real archives
        placed.push(Placed { pos, bytes, flags });
    }

    // --- hash table ------------------------------------------------------------
    // The probe position of a hash entry depends on the file name, so
    // entries whose names are unknown can only survive if the original
    // table's slot layout is preserved. When every kept entry is named,
    // a fresh (possibly larger) table is built instead — required whenever
    // the original table would overflow.
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
            continue; // covered by the remapped original slots
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

    // --- serialize tables --------------------------------------------------------
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

    // --- assemble -------------------------------------------------------------------
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

    // Bytes behind the original archive (appended data) are preserved.
    let old_end =
        base + archive.header().archive_size((data.len() - base) as u64) as usize;
    if old_end < data.len() {
        out.extend_from_slice(&data[old_end..]);
    }

    // --- self-check: the result must open and serve the additions ---------------
    let check = Archive::new(std::io::Cursor::new(&out))?;
    if let Some(t) = options.target_version {
        if check.version() != t {
            return Err(Error::InvalidData(
                "转换校验失败：写出的版本与目标不一致".to_string(),
            ));
        }
    }
    for (name, bytes) in &adds {
        let read = check.read_file(name)?;
        if &read != bytes {
            return Err(Error::InvalidData(format!("校验失败：{name} 内容不一致")));
        }
    }
    // Re-encoded key-v2 files must read back byte-identical content,
    // whether they were re-encrypted or stored plainly.
    for (name, content) in &reencoded {
        let read = check
            .read_file(name)
            .map_err(|e| Error::InvalidData(format!("校验失败：{name} 重编码后无法读取：{e}")))?;
        if &read != content {
            return Err(Error::InvalidData(format!("校验失败：{name} 内容不一致")));
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
    })
}

// -- helpers ------------------------------------------------------------------

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

/// MPQ-name equality: normalized (uppercased, `\` separators) byte equality.
fn same_name(a: &str, b: &str) -> bool {
    normalize_name(a) == normalize_name(b)
}

/// Normalize an imported name: `/` → `\`, drop empty / `.` / `..` parts.
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

/// Insert a name into a classic hash table by open probing. An occupied
/// slot with the same name pair and locale is replaced. Returns false when
/// the table is full.
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

/// Encode content as a new stored block: zlib per sector when it helps,
/// single unit for small files, plain sectors when incompressible.
/// Returns (stored bytes, flags); the file size is the caller's authority.
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
        // Nothing compresses: plain stored sectors at fixed offsets.
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

/// [`encode_fresh`] plus key-v2 encryption with the key derived from the
/// block's new offset — the layout a key-v2 file must get when its bytes
/// cannot be copied. Mirrors the on-disk conventions the reader expects:
///
/// * single unit: whole payload encrypted with the file key
/// * compressed sectors: offset table (dword-aligned by construction)
///   encrypted with `key - 1`, each sector independently with `key + i`,
///   sectors padded to dword boundaries (offsets include the padding)
/// * incompressible sectors: stored raw at fixed offsets, each encrypted
///   with `key + i`
///
/// A sector only counts as compressible when it stays smaller than the
/// original even after padding (the reader tells compressed sectors apart
/// by `stored_len < uncompressed_len`), hence the +4 slack in the test.
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
        // Plain stored sectors at fixed offsets, each with its own key.
        let mut payload = Vec::with_capacity(data.len());
        for (i, mut s) in sectors.into_iter().enumerate() {
            encrypt_bytes(&mut s, key.wrapping_add(i as u32));
            payload.extend_from_slice(&s);
        }
        return (payload, ENC);
    }

    // Compressed layout: dword-pad every sector (offsets include the
    // padding), encrypt the offset table with key-1 and each sector with
    // key+i.
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

/// Serialize the archive header (v1–v4; v4 fields beyond v3 are zero and
/// filled in by [`finish_v4_header`]).
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
        h.extend_from_slice(&0u64.to_le_bytes()); // hi-block table pos
        h.extend_from_slice(&0u16.to_le_bytes()); // hash table pos hi
        h.extend_from_slice(&0u16.to_le_bytes()); // block table pos hi
    }
    if version >= FormatVersion::V3 {
        h.extend_from_slice(&archive_size.to_le_bytes()); // archive size 64
        h.extend_from_slice(&0u64.to_le_bytes()); // BET pos
        h.extend_from_slice(&0u64.to_le_bytes()); // HET pos
    }
    // The v4-only fields (table size64s, raw chunk size, MD5s) start zeroed;
    // finish_v4_header fills the checksums afterwards.
    h.resize(header_size as usize, 0);
    debug_assert_eq!(h.len(), header_size as usize);
    h
}

/// Fill the v4-only header fields: explicit stored table sizes, table MD5s
/// over the *stored* (encrypted — here always uncompressed) bytes, and the
/// header's own MD5 over bytes `0x00..0xC0`, mirroring StormLib's
/// `SaveMPQTables`. HET/BET/hi-block are absent (zero), and no per-file
/// chunk MD5s are written, so `raw_chunk_size` stays 0.
pub(crate) fn finish_v4_header(header: &mut [u8], hash_buf: &[u8], block_buf: &[u8]) {
    debug_assert_eq!(header.len(), HEADER_SIZE_V4 as usize);
    let h: &mut [u8] = header;
    h[0x44..0x4C].copy_from_slice(&(hash_buf.len() as u64).to_le_bytes());
    h[0x4C..0x54].copy_from_slice(&(block_buf.len() as u64).to_le_bytes());
    // 0x54 hi-block, 0x5C HET, 0x64 BET table size64 stay 0, as does
    // raw_chunk_size at 0x6C.
    h[0x70..0x80].copy_from_slice(&digest::md5_digest(block_buf)); // MD5_BlockTable
    h[0x80..0x90].copy_from_slice(&digest::md5_digest(hash_buf)); // MD5_HashTable
    // MD5_HiBlockTable/BetTable/HetTable (0x90..0xC0) stay 0: no such tables.
    let header_md5 = digest::md5_digest(&h[..HEADER_MD5_SPAN]);
    h[HEADER_MD5_SPAN..0xD0].copy_from_slice(&header_md5);
}
