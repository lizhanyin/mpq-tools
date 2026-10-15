//! Integration tests for archive creation and version conversion.

mod common;

use std::io::Cursor;

use common::{build, build_hetbet, build_hetbet_with_listfile, ArchiveSpec, FileSpec};
use mpq::{convert, create, Archive, CreateOptions, Deletion, FormatVersion, ModifyOptions};

const ALL_VERSIONS: [FormatVersion; 4] = [
    FormatVersion::V1,
    FormatVersion::V2,
    FormatVersion::V3,
    FormatVersion::V4,
];

fn open(bytes: &[u8]) -> mpq::Result<Archive<Cursor<&[u8]>>> {
    Archive::new(Cursor::new(bytes))
}

fn spec(version: u16) -> ArchiveSpec {
    ArchiveSpec {
        version,
        sector_shift: 3,
        with_listfile: true,
        with_attributes: true,
        with_hi_block: version >= 1,
        userdata: false,
        prefix_garbage: 0,
        zero_archive_size: false,
        raw_chunk_size: 0,
        files: vec![
            FileSpec::text("readme.txt", "keep me\n"),
            FileSpec::text("data\\table.txt", "1,2,3\n4,5,6\n").compress(),
            FileSpec::text("secret.bin", "top secret payload").encrypt(),
        ],
    }
}

/// Read every named user entry (MPQ internals excluded) into pairs.
fn snapshot(archive: &Archive<Cursor<&[u8]>>) -> Vec<(String, Vec<u8>)> {
    archive
        .entries()
        .iter()
        .filter(|e| e.name.as_deref().is_some_and(|n| !n.starts_with('(')))
        .filter_map(|e| {
            e.name
                .clone()
                .map(|n| (n, archive.read_entry(e).expect("read entry")))
        })
        .collect()
}

// -- create -------------------------------------------------------------------

#[test]
fn create_each_version_opens_empty() {
    for v in ALL_VERSIONS {
        let bytes = create(&CreateOptions {
            version: v,
            ..Default::default()
        })
        .unwrap();
        let a = open(&bytes).unwrap();
        assert_eq!(a.version(), v, "created version");
        assert!(a.entries().is_empty(), "no entries for {v:?}");
        assert!(!a.uses_het_bet());
        assert_eq!(a.sector_size(), 4096);
        if v == FormatVersion::V4 {
            // The v4 checksums must be present and the header full-size.
            assert_eq!(a.header().header_size, 0xD0);
            assert_ne!(a.header().md5_header, [0u8; 16], "header MD5 written");
            assert_ne!(a.header().md5_hash_table, [0u8; 16]);
            assert_ne!(a.header().md5_block_table, [0u8; 16]);
        }
    }
}

#[test]
fn create_normalizes_hash_size() {
    let bytes = create(&CreateOptions {
        hash_table_size: 100, // not a power of two
        ..Default::default()
    })
    .unwrap();
    let a = open(&bytes).unwrap();
    assert_eq!(a.header().hash_table_size, 128);
}

#[test]
fn create_then_import_files() {
    let src = create(&CreateOptions {
        version: FormatVersion::V1,
        hash_table_size: 16,
        ..Default::default()
    })
    .unwrap();

    let big: Vec<u8> = (0..30_000u32).map(|i| (i % 251) as u8).collect();
    let out = mpq::modify(
        &src,
        &[],
        &[
            ("hello.txt".into(), b"hi there".to_vec()),
            ("d\\big.bin".into(), big.clone()),
        ],
    )
    .unwrap();

    let a = open(&out.bytes).unwrap();
    assert_eq!(a.version(), FormatVersion::V1, "import keeps the version");
    assert_eq!(a.read_file("hello.txt").unwrap(), b"hi there");
    assert_eq!(a.read_file("d\\big.bin").unwrap(), big);
    assert!(a.has_file("(listfile)"));
    assert_eq!(a.file_names().len(), 2, "listfile names only");
}

// -- convert ------------------------------------------------------------------

#[test]
fn convert_v3_to_each_version_preserves_content() {
    let src = build(&spec(2)); // v3 source with (attributes)
    let reference = snapshot(&open(&src).unwrap());

    // A rewrite happens for every *different* target (same-version v3 is
    // the documented no-op, covered by convert_same_version_is_noop).
    for target in [FormatVersion::V1, FormatVersion::V2, FormatVersion::V4] {
        let out = convert(&src, target).unwrap();
        let a = open(&out.bytes).unwrap();
        assert_eq!(a.version(), target, "target header written");
        assert!(!a.has_file("(attributes)"), "attributes dropped");
        assert!(!out.report.dropped.is_empty());
        for (name, content) in &reference {
            assert_eq!(
                &a.read_file(name).unwrap(),
                content,
                "{name} survives conversion to {target:?}"
            );
        }
    }
}

#[test]
fn convert_up_then_down_roundtrip() {
    let src = build(&spec(0)); // v1
    let reference = snapshot(&open(&src).unwrap());

    let up = convert(&src, FormatVersion::V4).unwrap();
    assert_eq!(open(&up.bytes).unwrap().version(), FormatVersion::V4);
    let down = convert(&up.bytes, FormatVersion::V1).unwrap();
    let a = open(&down.bytes).unwrap();
    assert_eq!(a.version(), FormatVersion::V1);
    for (name, content) in &reference {
        assert_eq!(&a.read_file(name).unwrap(), content, "{name} roundtrip");
    }
}

#[test]
fn convert_same_version_is_noop() {
    let src = build(&spec(1)); // v2
    let out = convert(&src, FormatVersion::V2).unwrap();
    assert_eq!(out.bytes, src, "same-version conversion returns input");
}

#[test]
fn convert_unparseable_input_errors() {
    let err = convert(b"not an mpq at all", FormatVersion::V3).unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[test]
fn convert_preserves_userdata_wrapper() {
    let mut s = spec(1);
    s.userdata = true;
    let src = build(&s);
    let base = open(&src).unwrap().archive_offset() as usize;

    let out = convert(&src, FormatVersion::V3).unwrap();
    // The bytes in front of the archive survive byte-for-byte.
    assert_eq!(&out.bytes[..base], &src[..base]);
    let a = open(&out.bytes).unwrap();
    assert_eq!(a.version(), FormatVersion::V3);
    assert_eq!(a.archive_offset() as usize, base);
    assert_eq!(a.read_file("readme.txt").unwrap(), b"keep me\n");
}

#[test]
fn convert_hetbet_with_listfile_to_classic() {
    let src = build_hetbet_with_listfile(&[
        ("a.txt".into(), b"AAA".to_vec()),
        ("b.txt".into(), b"BBB".to_vec()),
    ]);
    assert!(open(&src).unwrap().uses_het_bet());

    // v3→v3 is the no-op case; every other target rewrites to classic
    // tables with names anchored from the listfile.
    for target in [FormatVersion::V1, FormatVersion::V2, FormatVersion::V4] {
        let out = convert(&src, target).unwrap();
        let a = open(&out.bytes).unwrap();
        assert_eq!(a.version(), target);
        assert!(!a.uses_het_bet(), "classic tables only for {target:?}");
        assert_eq!(a.read_file("a.txt").unwrap(), b"AAA");
        assert_eq!(a.read_file("b.txt").unwrap(), b"BBB");
    }
}

#[test]
fn convert_hetbet_without_listfile_refuses_with_reason() {
    // Pure HET/BET archive without a (listfile): the file names cannot be
    // recovered, so a classic-table rewrite would orphan every file. The
    // conversion must refuse and say why.
    let src = build_hetbet(&[("a.txt".into(), b"AAA".to_vec())]);
    let err = convert(&src, FormatVersion::V1).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("无法转换"), "refusal wording: {msg}");
    assert!(msg.contains("(listfile)"), "reason names the listfile: {msg}");
}

#[test]
fn convert_empty_archive_between_versions() {
    let src = create(&CreateOptions {
        version: FormatVersion::V1,
        ..Default::default()
    })
    .unwrap();
    for target in [FormatVersion::V2, FormatVersion::V4] {
        let out = convert(&src, target).unwrap();
        let a = open(&out.bytes).unwrap();
        assert_eq!(a.version(), target);
        assert!(a.entries().is_empty());

        // The converted empty archive still accepts imports.
        let out2 = mpq::modify(&out.bytes, &[], &[("x.txt".into(), b"x".to_vec())]).unwrap();
        let a2 = open(&out2.bytes).unwrap();
        assert_eq!(a2.version(), target, "import keeps {target:?}");
        assert_eq!(a2.read_file("x.txt").unwrap(), b"x");
    }
}

#[test]
fn convert_deleted_entries_stay_deleted() {
    let src = build(&spec(0));
    let shrunk = mpq::modify(&src, &[Deletion::ByName("readme.txt".into())], &[]).unwrap();
    let out = convert(&shrunk.bytes, FormatVersion::V3).unwrap();
    let a = open(&out.bytes).unwrap();
    assert!(!a.has_file("readme.txt"));
    assert!(a.has_file("data\\table.txt"));
}

#[test]
fn modify_with_target_version_and_import_in_one_pass() {
    let src = build(&spec(0)); // v1
    let options = ModifyOptions {
        target_version: Some(FormatVersion::V3),
        ..Default::default()
    };
    let out =
        mpq::modify_with(&src, &[], &[("new.txt".into(), b"n".to_vec())], &options).unwrap();
    let a = open(&out.bytes).unwrap();
    assert_eq!(a.version(), FormatVersion::V3);
    assert_eq!(a.read_file("new.txt").unwrap(), b"n");
    assert_eq!(a.read_file("readme.txt").unwrap(), b"keep me\n");
}
