//! Round-trip integration tests: archives built by the in-test writer
//! must open and read back correctly through the public API.

mod common;

use common::{build, ArchiveSpec, FileSpec};
use mpq::{Archive, Error};
use std::io::Cursor;

fn open(bytes: &[u8]) -> Archive<Cursor<&[u8]>> {
    match Archive::new(Cursor::new(bytes)) {
        Ok(a) => a,
        Err(e) => panic!("archive should open: {e}"),
    }
}

fn open_owned(bytes: Vec<u8>) -> Archive<Cursor<Vec<u8>>> {
    match Archive::new(Cursor::new(bytes)) {
        Ok(a) => a,
        Err(e) => panic!("archive should open: {e}"),
    }
}

fn lorem(prefix: &str, repeat: usize) -> String {
    prefix.repeat(repeat)
}

fn crc32_of(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[test]
fn v1_stored_sectors() {
    let spec = ArchiveSpec {
        version: 0,
        sector_shift: 3,
        files: vec![
            FileSpec::text("war3map.j", "// dummy map script\n"),
            FileSpec::text(
                "data\\big.txt",
                &lorem("the quick brown fox jumps over the lazy dog. ", 400),
            ),
        ],
        ..ArchiveSpec::default()
    };
    let bytes = build(&spec);
    let archive = open(&bytes);
    assert_eq!(archive.version(), mpq::FormatVersion::V1);
    assert_eq!(archive.sector_size(), 4096);
    assert_eq!(
        archive.read_file("war3map.j").unwrap(),
        b"// dummy map script\n".to_vec()
    );
    let big = archive.read_file("data\\big.txt").unwrap();
    assert_eq!(
        big.len(),
        "the quick brown fox jumps over the lazy dog. ".len() * 400
    );
    assert!(big.starts_with(b"the quick brown fox"));
}

#[test]
fn v1_compressed_multi_sector_with_listfile() {
    let content = lorem("StarCraft II is a real-time strategy game. ", 1000); // ~42 KB
    let spec = ArchiveSpec {
        version: 0,
        sector_shift: 3,
        with_listfile: true,
        files: vec![FileSpec::text("arr\\units.xml", &content).compress()],
        ..ArchiveSpec::default()
    };
    let bytes = build(&spec);
    let archive = open(&bytes);
    let names = archive.file_names();
    assert!(names.contains(&"arr\\units.xml".to_string()));
    let expected: Vec<u8> = content.into_bytes();
    assert_eq!(archive.read_file("arr\\units.xml").unwrap(), expected);
    // Slash normalization on lookup.
    assert_eq!(archive.read_file("arr/units.xml").unwrap(), expected);
}

#[test]
fn v1_encrypted_compressed_sectors() {
    let content = lorem("encrypted sector content block ", 300);
    let spec = ArchiveSpec {
        version: 0,
        sector_shift: 3,
        files: vec![FileSpec::text("secret.txt", &content).compress().encrypt()],
        ..ArchiveSpec::default()
    };
    let archive = open_owned(build(&spec));
    assert_eq!(archive.read_file("secret.txt").unwrap(), content.into_bytes());
}

#[test]
fn v1_encrypted_stored_sectors() {
    let content = lorem("plain stored encrypted ", 400); // > 1 sector, multiple of 4
    let spec = ArchiveSpec {
        version: 0,
        sector_shift: 3,
        files: vec![FileSpec::text("stored.bin", &content).encrypt()],
        ..ArchiveSpec::default()
    };
    let archive = open_owned(build(&spec));
    assert_eq!(archive.read_file("stored.bin").unwrap(), content.into_bytes());
}

#[test]
fn v1_single_unit_variants() {
    let content: Vec<u8> = lorem("single unit payload ", 20).into_bytes();
    let spec = ArchiveSpec {
        version: 0,
        files: vec![
            FileSpec::bytes("plain.bin", content.clone()).single_unit(),
            FileSpec::bytes("zlib.bin", content.clone())
                .single_unit()
                .compress(),
            FileSpec::bytes("enc.bin", content.clone())
                .single_unit()
                .compress()
                .encrypt(),
            FileSpec::bytes("encfix.bin", content.clone())
                .single_unit()
                .compress()
                .encrypt()
                .fix_key(),
        ],
        ..ArchiveSpec::default()
    };
    let archive = open_owned(build(&spec));
    for name in ["plain.bin", "zlib.bin", "enc.bin", "encfix.bin"] {
        assert_eq!(
            archive.read_file(name).unwrap(),
            content.clone(),
            "file {name}"
        );
    }
}

#[test]
fn fix_key_sector_file() {
    let content = lorem("key v2 sector data ", 500);
    let spec = ArchiveSpec {
        version: 0,
        sector_shift: 3,
        files: vec![
            FileSpec::text("patched.dat", &content)
                .compress()
                .encrypt()
                .fix_key(),
        ],
        ..ArchiveSpec::default()
    };
    let archive = open_owned(build(&spec));
    assert_eq!(archive.read_file("patched.dat").unwrap(), content.into_bytes());
}

#[test]
fn v2_header_with_hi_block() {
    let spec = ArchiveSpec {
        version: 1,
        with_hi_block: true,
        sector_shift: 2,
        files: vec![FileSpec::text("a.txt", "hello v2").compress()],
        ..ArchiveSpec::default()
    };
    let archive = open_owned(build(&spec));
    assert_eq!(archive.version(), mpq::FormatVersion::V2);
    assert_eq!(archive.read_file("a.txt").unwrap(), b"hello v2".to_vec());
}

#[test]
fn v3_header_opens_with_classic_tables() {
    let spec = ArchiveSpec {
        version: 2,
        with_listfile: true,
        sector_shift: 3,
        files: vec![FileSpec::text("v3file.txt", "cataclysm era data").compress()],
        ..ArchiveSpec::default()
    };
    let archive = open_owned(build(&spec));
    assert_eq!(archive.version(), mpq::FormatVersion::V3);
    assert!(
        !archive.uses_het_bet(),
        "no HET/BET in this synthetic archive"
    );
    assert_eq!(
        archive.read_file("v3file.txt").unwrap(),
        b"cataclysm era data".to_vec()
    );
}

#[test]
fn v3_het_bet_archive_without_classic_tables() {
    let files = vec![
        ("alpha.txt".to_string(), b"first file content".to_vec()),
        (
            r"data\nested\beta.bin".to_string(),
            b"0123456789".repeat(50),
        ),
        ("third.lst".to_string(), vec![0xAB; 7000]), // crosses sector boundary
    ];
    let bytes = common::build_hetbet(&files);
    let archive = open(&bytes);

    assert_eq!(archive.version(), mpq::FormatVersion::V3);
    assert!(archive.uses_het_bet(), "HET/BET tables in use");
    for (name, data) in &files {
        assert!(archive.has_file(name), "has {name}");
        assert_eq!(&archive.read_file(name).unwrap(), data, "content of {name}");
    }
    assert!(!archive.has_file("missing.txt"));

    let entries = archive.entries();
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().all(|e| e.source == mpq::TableSource::Bet));
}

#[test]
fn userdata_wrapper_is_located() {
    let spec = ArchiveSpec {
        version: 0,
        userdata: true,
        files: vec![FileSpec::text("map\\war3map.w3e", "terrain")],
        ..ArchiveSpec::default()
    };
    let archive = open_owned(build(&spec));
    assert_eq!(archive.archive_offset(), 0x10);
    assert_eq!(
        archive.read_file("map\\war3map.w3e").unwrap(),
        b"terrain".to_vec()
    );
}

#[test]
fn embedded_archive_found_by_scan() {
    let spec = ArchiveSpec {
        version: 0,
        prefix_garbage: 0x200,
        files: vec![FileSpec::text("inner.txt", "found by scanning")],
        ..ArchiveSpec::default()
    };
    let archive = open_owned(build(&spec));
    assert_eq!(archive.archive_offset(), 0x200);
    assert_eq!(
        archive.read_file("inner.txt").unwrap(),
        b"found by scanning".to_vec()
    );
}

#[test]
fn missing_listfile_still_lists_entries() {
    let spec = ArchiveSpec {
        version: 0,
        files: vec![FileSpec::text("nameless.bin", "no listfile here")],
        ..ArchiveSpec::default()
    };
    let archive = open_owned(build(&spec));
    assert!(archive.file_names().is_empty());
    assert!(archive.warnings().iter().any(|w| w.contains("listfile")));

    let entries = archive.entries();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].name.is_none());
    assert_eq!(entries[0].display_name(), "<unnamed #0>");
    // Uncompressed file reads fine even without a name.
    assert_eq!(
        archive.read_entry(&entries[0]).unwrap(),
        b"no listfile here".to_vec()
    );
}

#[test]
fn attributes_file_is_parsed() {
    let spec = ArchiveSpec {
        version: 0,
        with_attributes: true,
        with_listfile: true,
        files: vec![FileSpec::text("attr.txt", "with attributes")],
        ..ArchiveSpec::default()
    };
    let archive = open_owned(build(&spec));
    let attrs = archive.attributes().expect("(attributes) parsed");
    // The CRC array covers every block entry (attr.txt, listfile, attributes).
    assert_eq!(attrs.crc32.len(), 3);
    assert_eq!(attrs.crc32[0], crc32_of(b"with attributes"));
    assert!(archive.has_file("(attributes)"));
    assert!(archive.has_file("(listfile)"));
}

#[test]
fn wrong_archive_size_field_still_opens() {
    // A zeroed archive size field must not prevent opening (v1 archives
    // often carry garbage there; storm ignores the value).
    let spec = ArchiveSpec {
        version: 0,
        zero_archive_size: true,
        files: vec![FileSpec::text("cut.txt", "size field is garbage")],
        ..ArchiveSpec::default()
    };
    let bytes = build(&spec);
    let archive = match Archive::new(Cursor::new(&bytes)) {
        Ok(a) => a,
        Err(e) => panic!("tolerant open: {e}"),
    };
    assert_eq!(
        archive.read_file("cut.txt").unwrap(),
        b"size field is garbage".to_vec()
    );
}

#[test]
fn not_an_archive_rejected() {
    let err = match Archive::new(Cursor::new(b"random garbage bytes not an mpq" as &[u8])) {
        Err(e) => e,
        Ok(_) => panic!("garbage must not open"),
    };
    assert!(matches!(err, Error::NotAnArchive));
}

#[test]
fn missing_file_reports_not_found() {
    let spec = ArchiveSpec {
        version: 0,
        files: vec![FileSpec::text("present.txt", "x")],
        ..ArchiveSpec::default()
    };
    let archive = open_owned(build(&spec));
    assert!(matches!(
        archive.read_file("absent.txt"),
        Err(Error::FileNotFound(_))
    ));
}

#[test]
fn pkware_imploded_file() {
    // A file stored with the legacy IMPLODE flag: raw PKWARE DCL stream
    // from the fixtures (produced by Blizzard's compressor), no method byte.
    let stream = include_bytes!("fixtures/pk_text.bin").to_vec();
    let expected = include_bytes!("fixtures/pk_text.orig").to_vec();
    let spec = ArchiveSpec {
        version: 0,
        files: vec![FileSpec::text("legacy.pk", "")
            .imploded(stream, expected.len() as u32)
            .single_unit()],
        ..ArchiveSpec::default()
    };
    let archive = open_owned(build(&spec));
    assert_eq!(archive.read_file("legacy.pk").unwrap(), expected);
}

#[test]
fn many_files_and_locale_probing() {
    let mut files = Vec::new();
    for i in 0..40 {
        files.push(FileSpec::text(
            &format!("data\\file{i:02}.txt"),
            &format!("content number {i} padded 0123456789"),
        ));
    }
    let spec = ArchiveSpec {
        version: 0,
        sector_shift: 3,
        with_listfile: true,
        files,
        ..ArchiveSpec::default()
    };
    let archive = open_owned(build(&spec));
    assert_eq!(archive.file_names().len(), 40);
    for i in 0..40 {
        let name = format!("data\\file{i:02}.txt");
        assert_eq!(
            archive.read_file(&name).unwrap(),
            format!("content number {i} padded 0123456789").into_bytes(),
            "{name}"
        );
    }
}

/// Regenerate the small fixture used by the crate-level doc example.
/// Runs as part of the test suite; writing the same bytes is idempotent.
#[test]
fn write_doc_fixture() {
    use std::io::Write;
    let spec = ArchiveSpec {
        version: 0,
        sector_shift: 3,
        with_listfile: true,
        files: vec![FileSpec::text("war3map.j", "// dummy map script\n")],
        ..ArchiveSpec::default()
    };
    let bytes = build(&spec);
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mini_v1.mpq");
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(&bytes).unwrap();
}

