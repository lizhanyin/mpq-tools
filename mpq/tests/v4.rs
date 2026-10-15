//! v4 support end to end: full header parsing, the six MD5 checksums,
//! per-file raw-chunk MD5s, v4 write-back (modify / create / convert) and
//! repair-time re-hashing of the header.

mod common;

use std::io::Cursor;

use common::{build, build_hetbet_v4, ArchiveSpec, FileSpec};
use mpq::{convert, create, modify, Archive, CreateOptions, Deletion, FormatVersion};

fn open(bytes: &[u8]) -> mpq::Result<Archive<Cursor<&[u8]>>> {
    Archive::new(Cursor::new(bytes))
}

fn spec_v4() -> ArchiveSpec {
    ArchiveSpec {
        version: 3, // v4
        sector_shift: 3,
        with_listfile: true,
        files: vec![
            FileSpec::text("readme.txt", "v4 archive\n"),
            FileSpec::text("data\\table.txt", "1,2,3\n4,5,6\n").compress(),
            FileSpec::text("secret.bin", "classified payload").encrypt(),
        ],
        ..ArchiveSpec::default()
    }
}

fn md5_warnings<'a>(a: &'a Archive<Cursor<&[u8]>>) -> Vec<&'a str> {
    a.warnings().iter().map(|w| w.as_str()).filter(|w| w.contains("MD5")).collect()
}

// -- reading --------------------------------------------------------------------

#[test]
fn v4_opens_and_reads_without_md5_warnings() {
    let bytes = build(&spec_v4());
    let a = open(&bytes).unwrap();
    assert_eq!(a.version(), FormatVersion::V4);
    assert_eq!(a.read_file("readme.txt").unwrap(), b"v4 archive\n");
    assert_eq!(a.read_file("data\\table.txt").unwrap(), b"1,2,3\n4,5,6\n");
    assert_eq!(a.read_file("secret.bin").unwrap(), b"classified payload");
    assert!(md5_warnings(&a).is_empty(), "checksums verify: {:?}", a.warnings());
    // The v4-only header fields actually parsed.
    assert!(a.header().raw_chunk_size == 0);
    assert_eq!(a.header().hash_table_size64, a.header().hash_table_size as u64 * 16);
}

#[test]
fn v4_header_md5_mismatch_only_warns() {
    let mut bytes = build(&spec_v4());
    bytes[0xC0] ^= 0xFF; // inside MD5_MpqHeader itself, outside its 0xC0 span
    let a = open(&bytes).unwrap();
    assert_eq!(a.version(), FormatVersion::V4);
    assert!(md5_warnings(&a).len() == 1, "exactly the header warning: {:?}", a.warnings());
    assert!(md5_warnings(&a)[0].contains("header MD5"));
    // Data is untouched by the broken checksum.
    assert_eq!(a.read_file("readme.txt").unwrap(), b"v4 archive\n");
}

#[test]
fn v4_hash_table_md5_mismatch_warns() {
    let mut bytes = build(&spec_v4());
    bytes[0x80] ^= 0xFF; // inside MD5_HashTable (also inside the header MD5 span)
    let a = open(&bytes).unwrap();
    let w = md5_warnings(&a);
    assert!(w.iter().any(|w| w.contains("hash table")), "hash table warning: {w:?}");
    assert!(w.iter().any(|w| w.contains("header MD5")), "header warning: {w:?}");
}

#[test]
fn v4_hetbet_uses_explicit_sizes_and_md5s() {
    let bytes = build_hetbet_v4(&[
        ("a.txt".into(), b"AAA".to_vec()),
        ("b.txt".into(), b"BBB".to_vec()),
    ]);
    let a = open(&bytes).unwrap();
    assert_eq!(a.version(), FormatVersion::V4);
    assert!(a.uses_het_bet());
    assert_eq!(a.read_file("a.txt").unwrap(), b"AAA");
    assert_eq!(a.read_file("b.txt").unwrap(), b"BBB");
    assert!(md5_warnings(&a).is_empty(), "{:?}", a.warnings());

    // A wrong HET checksum is a warning; the table stays usable.
    let mut corrupt = bytes;
    corrupt[0xB0] ^= 0xFF; // inside MD5_HetTable
    let a = open(&corrupt).unwrap();
    let w = md5_warnings(&a);
    assert!(w.iter().any(|w| w.contains("HET table")), "HET warning: {w:?}");
    assert_eq!(a.read_file("b.txt").unwrap(), b"BBB");
}

#[test]
fn v4_raw_chunk_md5s_are_verified_on_read() {
    let mut s = spec_v4();
    s.raw_chunk_size = 8; // small chunks: several per file
    let bytes = build(&s);
    let a = open(&bytes).unwrap();
    assert_eq!(a.header().raw_chunk_size, 8);
    assert_eq!(a.read_file("readme.txt").unwrap(), b"v4 archive\n");
    assert!(a.read_warnings().is_empty(), "intact chunk MD5s: {:?}", a.read_warnings());

    // Corrupt the first chunk MD5 of the first block (header is 0xD0, the
    // stored data of readme.txt is 11 raw bytes, so the array starts at
    // 0xD0 + 11).
    let mut corrupt = bytes;
    corrupt[0xD0 + 11] ^= 0xFF;
    let a = open(&corrupt).unwrap();
    assert_eq!(a.read_file("readme.txt").unwrap(), b"v4 archive\n", "data intact");
    assert!(
        a.read_warnings().iter().any(|w| w.contains("chunk") && w.contains("MD5")),
        "chunk mismatch reported: {:?}",
        a.read_warnings()
    );
    // Other files verify cleanly.
    assert_eq!(a.read_file("secret.bin").unwrap(), b"classified payload");
}

// -- writing --------------------------------------------------------------------

#[test]
fn modify_preserves_v4_and_recomputes_md5s() {
    let src = build(&spec_v4());
    let out = modify(
        &src,
        &[Deletion::ByName("data\\table.txt".into())],
        &[("new.txt".into(), b"fresh v4 content".to_vec())],
    )
    .unwrap();
    let a = open(&out.bytes).unwrap();
    assert_eq!(a.version(), FormatVersion::V4, "output stays v4");
    assert!(md5_warnings(&a).is_empty(), "recomputed checksums verify: {:?}", a.warnings());
    assert_eq!(a.read_file("new.txt").unwrap(), b"fresh v4 content");
    assert_eq!(a.read_file("secret.bin").unwrap(), b"classified payload");
    assert!(!a.has_file("data\\table.txt"));
    assert!(out.report.deleted.len() == 1);
}

#[test]
fn create_v4_empty_opens_and_accepts_imports() {
    let bytes = create(&CreateOptions {
        version: FormatVersion::V4,
        ..Default::default()
    })
    .unwrap();
    let a = open(&bytes).unwrap();
    assert_eq!(a.version(), FormatVersion::V4);
    assert!(a.entries().is_empty());
    assert!(md5_warnings(&a).is_empty(), "{:?}", a.warnings());

    let out = modify(&bytes, &[], &[("hello.txt".into(), b"hello v4".to_vec())]).unwrap();
    let a = open(&out.bytes).unwrap();
    assert_eq!(a.version(), FormatVersion::V4);
    assert_eq!(a.read_file("hello.txt").unwrap(), b"hello v4");
    assert!(md5_warnings(&a).is_empty(), "{:?}", a.warnings());
}

#[test]
fn convert_between_v4_and_older_versions() {
    // v1 -> v4: the produced container carries valid v4 checksums.
    let mut s = spec_v4();
    s.version = 0; // v1 source
    let src = build(&s);
    let out = convert(&src, FormatVersion::V4).unwrap();
    let a = open(&out.bytes).unwrap();
    assert_eq!(a.version(), FormatVersion::V4);
    assert!(md5_warnings(&a).is_empty(), "{:?}", a.warnings());
    assert_eq!(a.read_file("readme.txt").unwrap(), b"v4 archive\n");

    // v4 -> v1: explicit downgrade still works.
    let v4 = build(&spec_v4());
    let out = convert(&v4, FormatVersion::V1).unwrap();
    let a = open(&out.bytes).unwrap();
    assert_eq!(a.version(), FormatVersion::V1);
    assert_eq!(a.read_file("secret.bin").unwrap(), b"classified payload");

    // v4 -> v4 is a no-op returning the input unchanged.
    let out = convert(&v4, FormatVersion::V4).unwrap();
    assert_eq!(out.bytes, v4);
}

// -- repair ---------------------------------------------------------------------

#[test]
fn repair_recomputes_v4_header_md5() {
    // Repaired header fields invalidate MD5_MpqHeader; repair must
    // recompute it so the result verifies cleanly. Appended junk changes
    // the real archive span, so the rewritten size fields differ from what
    // the builder originally hashed.
    let mut src = build(&spec_v4());
    src.extend_from_slice(&[0xAA; 16]);
    let report = mpq::repair(&mut src, false).unwrap();
    assert!(
        report.issues.iter().any(|i| i.field == "archive_size"),
        "{:?}",
        report.issues
    );
    assert!(
        report.issues.iter().any(|i| i.field == "MD5_MpqHeader"),
        "rehash reported: {:?}",
        report.issues
    );
    let a = open(&src).unwrap();
    assert!(md5_warnings(&a).is_empty(), "repaired archive verifies: {:?}", a.warnings());
    assert_eq!(a.read_file("readme.txt").unwrap(), b"v4 archive\n");

    // A corrupt header MD5 alone is diagnosed and fixed.
    let mut src = build(&spec_v4());
    src[0xC0] ^= 0xFF;
    let report = mpq::repair(&mut src, false).unwrap();
    assert!(report.issues.iter().any(|i| i.field == "MD5_MpqHeader"));
    let a = open(&src).unwrap();
    assert!(md5_warnings(&a).is_empty(), "{:?}", a.warnings());
}
