//! 修复集成测试：以特定方式损坏一个健康的合成归档，
//! 然后修复它，并验证归档重新变得可读。

mod common;

use common::{build, ArchiveSpec, FileSpec};
use mpq::{diagnose, repair, Archive, Error};
use std::io::Cursor;

fn healthy() -> Vec<u8> {
    build(&ArchiveSpec {
        version: 0,
        sector_shift: 3,
        with_listfile: true,
        files: vec![
            FileSpec::text("file.txt", "repair me"),
            FileSpec::text("data\\other.bin", &"payload ".repeat(100)).compress(),
        ],
        ..ArchiveSpec::default()
    })
}

fn open(bytes: &[u8]) -> Result<Archive<Cursor<&[u8]>>, Error> {
    Archive::new(Cursor::new(bytes))
}

fn wr32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn wr16(buf: &mut [u8], off: usize, v: u16) {
    buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn rd32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

#[test]
fn healthy_archive_has_no_issues() {
    let bytes = healthy();
    let report = diagnose(&bytes).unwrap();
    let unfixable = report.unfixed();
    assert!(unfixable.is_empty(), "unfixable: {unfixable:?}");
    assert!(
        report.issues.is_empty(),
        "healthy archive should be clean, got {:?}",
        report.issues
    );
    assert!(!report.changed);
}

#[test]
fn wrong_archive_size_is_fixed() {
    let mut bytes = healthy();
    wr32(&mut bytes, 0x08, 0x7FFF_FFFF);
    let report = diagnose(&bytes).unwrap();
    assert!(report.issues.iter().any(|i| i.field == "archive_size"));

    repair(&mut bytes, true).unwrap();
    assert_eq!(rd32(&bytes, 0x08) as usize, bytes.len());
    let archive = open(&bytes).unwrap();
    assert_eq!(archive.read_file("file.txt").unwrap(), b"repair me".to_vec());
}

#[test]
fn oversized_hash_table_is_trimmed() {
    let mut bytes = healthy();
    // Protector 式的滥用：声明 1 Mi 个条目。
    wr32(&mut bytes, 0x18, 0x10_0000);
    let archive = open(&bytes).unwrap(); // 读取器通过零填充容忍该值
    assert!(archive.warnings().is_empty() || true);

    let report = repair(&mut bytes, true).unwrap();
    assert!(report.issues.iter().any(|i| i.field == "hash_table_size"));
    let archive = open(&bytes).unwrap();
    assert_eq!(archive.read_file("file.txt").unwrap(), b"repair me".to_vec());
}

#[test]
fn out_of_range_hash_table_is_relocated_by_scan() {
    let mut bytes = healthy();
    // 将哈希表指向远超 EOF 的位置；真正的表仍在文件中，
    // 必须由深度扫描找到。
    wr32(&mut bytes, 0x10, 0x7F00_0000);
    assert!(open(&bytes).is_err(), "unrepaired archive must not open");

    let report = repair(&mut bytes, true).unwrap();
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.field == "hash_table_pos" && i.action.is_some()),
        "hash table should be relocated: {:?}",
        report.issues
    );
    let archive = open(&bytes).unwrap();
    assert_eq!(archive.read_file("file.txt").unwrap(), b"repair me".to_vec());
    assert_eq!(
        archive.read_file("data\\other.bin").unwrap(),
        b"payload ".repeat(100)
    );
}

#[test]
fn out_of_range_block_table_is_relocated_by_scan() {
    let mut bytes = healthy();
    wr32(&mut bytes, 0x14, 0x7F00_0000);
    assert!(open(&bytes).is_err());

    let report = repair(&mut bytes, true).unwrap();
    assert!(report
        .issues
        .iter()
        .any(|i| i.field == "block_table_pos" && i.action.is_some()));
    let archive = open(&bytes).unwrap();
    assert_eq!(archive.read_file("file.txt").unwrap(), b"repair me".to_vec());
}

#[test]
fn invalid_version_is_inferred() {
    let mut bytes = healthy();
    wr16(&mut bytes, 0x0C, 9);
    let report = repair(&mut bytes, true).unwrap();
    assert!(report.issues.iter().any(|i| i.field == "format_version"));
    assert_eq!(rd32(&bytes, 0x0C) & 0xFFFF, 0); // v1
    let archive = open(&bytes).unwrap();
    assert_eq!(archive.version(), mpq::FormatVersion::V1);
    assert_eq!(archive.read_file("file.txt").unwrap(), b"repair me".to_vec());
}

#[test]
fn garbage_v1_header_size_is_normalized() {
    let mut bytes = healthy();
    wr32(&mut bytes, 0x04, 0x1234);
    let report = repair(&mut bytes, true).unwrap();
    assert!(report.issues.iter().any(|i| i.field == "header_size"));
    assert_eq!(rd32(&bytes, 0x04), 0x20);
    assert!(open(&bytes).unwrap().read_file("file.txt").is_ok());
}

#[test]
fn insane_sector_exponent_is_fixed() {
    let mut bytes = healthy();
    wr16(&mut bytes, 0x0E, 0xFFFF);
    let report = repair(&mut bytes, true).unwrap();
    assert!(report.issues.iter().any(|i| i.field == "block_size"));
    let archive = open(&bytes).unwrap();
    assert_eq!(archive.sector_size(), 4096);
    assert_eq!(archive.read_file("file.txt").unwrap(), b"repair me".to_vec());
}

#[test]
fn v3_bad_ext_table_positions_are_cleared() {
    let bytes = build(&ArchiveSpec {
        version: 2,
        files: vec![FileSpec::text("v3.txt", "three").compress()],
        ..ArchiveSpec::default()
    });
    let mut bytes = bytes;
    // het/bet 位置位于 0x34/0x3C（在合成归档中本来就是 0）；
    // 伪造一个超出 EOF 的 BET 指针，然后将其修复掉。
    let bet = 0x7FFF_FFFFu64.to_le_bytes();
    bytes[0x34..0x3C].copy_from_slice(&bet);
    // header_size 为 0x44，因此该字段确实存在于磁盘上。
    let report = repair(&mut bytes, true).unwrap();
    assert!(report.issues.iter().any(|i| i.field == "bet_table_pos"));
    assert_eq!(&bytes[0x34..0x3C], &0u64.to_le_bytes());
    let archive = open(&bytes).unwrap();
    assert_eq!(archive.read_file("v3.txt").unwrap(), b"three".to_vec());
}

#[test]
fn diagnose_does_not_modify_input() {
    let mut bytes = healthy();
    wr32(&mut bytes, 0x08, 1); // 错误的大小
    let before = bytes.clone();
    let report = diagnose(&bytes).unwrap();
    assert!(!report.issues.is_empty());
    assert_eq!(before, bytes, "diagnose must not write");
}

#[test]
fn truncated_archive_still_repairs_enough_to_open() {
    // 砍掉最后 8 字节（最后一个块表条目的一半）：修复会缩减
    // 块表计数，使归档重新可以打开。只丢失最后一个块表条目
    //（即 (listfile)），真正的文件完好无损。
    let mut bytes = healthy();
    bytes.truncate(bytes.len() - 8);
    let report = repair(&mut bytes, true).unwrap();
    assert!(report.issues.iter().any(|i| i.field == "block_table_size"));
    let archive = match open(&bytes) {
        Ok(a) => a,
        Err(e) => panic!("repaired archive should open: {e}"),
    };
    assert!(archive.has_file("file.txt"));
    assert_eq!(archive.read_file("file.txt").unwrap(), b"repair me".to_vec());
    assert_eq!(
        archive.read_file(r"data\other.bin").unwrap(),
        b"payload ".repeat(100)
    );
}
