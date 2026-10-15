//! v4 支持端到端测试：完整头部解析、六项 MD5 校验和、
//! 每文件 raw-chunk MD5、v4 写回（modify / create / convert），
//! 以及修复时对头部的重新哈希。

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

// -- 读取 --------------------------------------------------------------------

#[test]
fn v4_opens_and_reads_without_md5_warnings() {
    let bytes = build(&spec_v4());
    let a = open(&bytes).unwrap();
    assert_eq!(a.version(), FormatVersion::V4);
    assert_eq!(a.read_file("readme.txt").unwrap(), b"v4 archive\n");
    assert_eq!(a.read_file("data\\table.txt").unwrap(), b"1,2,3\n4,5,6\n");
    assert_eq!(a.read_file("secret.bin").unwrap(), b"classified payload");
    assert!(md5_warnings(&a).is_empty(), "checksums verify: {:?}", a.warnings());
    // v4 专属头部字段确实被解析了。
    assert!(a.header().raw_chunk_size == 0);
    assert_eq!(a.header().hash_table_size64, a.header().hash_table_size as u64 * 16);
}

#[test]
fn v4_header_md5_mismatch_only_warns() {
    let mut bytes = build(&spec_v4());
    bytes[0xC0] ^= 0xFF; // 位于 MD5_MpqHeader 自身之内，在其 0xC0 覆盖范围之外
    let a = open(&bytes).unwrap();
    assert_eq!(a.version(), FormatVersion::V4);
    assert!(md5_warnings(&a).len() == 1, "exactly the header warning: {:?}", a.warnings());
    assert!(md5_warnings(&a)[0].contains("header MD5"));
    // 数据未受损坏校验和的影响。
    assert_eq!(a.read_file("readme.txt").unwrap(), b"v4 archive\n");
}

#[test]
fn v4_hash_table_md5_mismatch_warns() {
    let mut bytes = build(&spec_v4());
    bytes[0x80] ^= 0xFF; // 位于 MD5_HashTable 内（也在头部 MD5 覆盖范围内）
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

    // 错误的 HET 校验和只产生警告；表仍然可用。
    let mut corrupt = bytes;
    corrupt[0xB0] ^= 0xFF; // 位于 MD5_HetTable 内
    let a = open(&corrupt).unwrap();
    let w = md5_warnings(&a);
    assert!(w.iter().any(|w| w.contains("HET table")), "HET warning: {w:?}");
    assert_eq!(a.read_file("b.txt").unwrap(), b"BBB");
}

#[test]
fn v4_raw_chunk_md5s_are_verified_on_read() {
    let mut s = spec_v4();
    s.raw_chunk_size = 8; // 小 chunk：每个文件有多个
    let bytes = build(&s);
    let a = open(&bytes).unwrap();
    assert_eq!(a.header().raw_chunk_size, 8);
    assert_eq!(a.read_file("readme.txt").unwrap(), b"v4 archive\n");
    assert!(a.read_warnings().is_empty(), "intact chunk MD5s: {:?}", a.read_warnings());

    // 破坏第一个 block 的第一个 chunk MD5（头部为 0xD0，readme.txt
    // 的存储数据是 11 个原始字节，因此该数组从 0xD0 + 11 开始）。
    let mut corrupt = bytes;
    corrupt[0xD0 + 11] ^= 0xFF;
    let a = open(&corrupt).unwrap();
    assert_eq!(a.read_file("readme.txt").unwrap(), b"v4 archive\n", "data intact");
    assert!(
        a.read_warnings().iter().any(|w| w.contains("chunk") && w.contains("MD5")),
        "chunk mismatch reported: {:?}",
        a.read_warnings()
    );
    // 其他文件校验通过。
    assert_eq!(a.read_file("secret.bin").unwrap(), b"classified payload");
}

// -- 写入 --------------------------------------------------------------------

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
    // v1 -> v4：生成的容器带有有效的 v4 校验和。
    let mut s = spec_v4();
    s.version = 0; // v1 源
    let src = build(&s);
    let out = convert(&src, FormatVersion::V4).unwrap();
    let a = open(&out.bytes).unwrap();
    assert_eq!(a.version(), FormatVersion::V4);
    assert!(md5_warnings(&a).is_empty(), "{:?}", a.warnings());
    assert_eq!(a.read_file("readme.txt").unwrap(), b"v4 archive\n");

    // v4 -> v1：显式降级仍然可行。
    let v4 = build(&spec_v4());
    let out = convert(&v4, FormatVersion::V1).unwrap();
    let a = open(&out.bytes).unwrap();
    assert_eq!(a.version(), FormatVersion::V1);
    assert_eq!(a.read_file("secret.bin").unwrap(), b"classified payload");

    // v4 -> v4 是无操作，原样返回输入。
    let out = convert(&v4, FormatVersion::V4).unwrap();
    assert_eq!(out.bytes, v4);
}

// -- 修复 ---------------------------------------------------------------------

#[test]
fn repair_recomputes_v4_header_md5() {
    // 修复头部字段会使 MD5_MpqHeader 失效；repair 必须重新计算它，
    // 让结果能干净地通过校验。追加的垃圾数据改变了归档的真实跨度，
    // 因此重写后的大小字段与构建器最初哈希的内容不同。
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

    // 仅头部 MD5 损坏的情况会被诊断并修复。
    let mut src = build(&spec_v4());
    src[0xC0] ^= 0xFF;
    let report = mpq::repair(&mut src, false).unwrap();
    assert!(report.issues.iter().any(|i| i.field == "MD5_MpqHeader"));
    let a = open(&src).unwrap();
    assert!(md5_warnings(&a).is_empty(), "{:?}", a.warnings());
}
