//! 归档修改（导入 / 删除）的集成测试。

mod common;

use std::io::Cursor;

use common::{build, build_hetbet, ArchiveSpec, FileSpec};
use mpq::{Archive, Deletion};

type OpenArchive<'a> = Archive<Cursor<&'a [u8]>>;

/// 打开字节并返回归档以及规范化后的名称列表。
fn open(bytes: &[u8]) -> mpq::Result<(OpenArchive<'_>, Vec<String>)> {
    let archive = Archive::new(Cursor::new(bytes))?;
    let names = archive
        .file_names()
        .iter()
        .map(|s| s.replace('\\', "/"))
        .collect();
    Ok((archive, names))
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

/// 修改辅助函数：对原始字节执行添加 / 删除，返回新字节。
fn modify(
    bytes: &[u8],
    del: &[Deletion],
    add: &[(String, Vec<u8>)],
) -> mpq::ModifyOutcome {
    mpq::modify(bytes, del, add).expect("modify")
}

#[test]
fn add_and_delete_v1() {
    let src = build(&spec(0));
    let out = modify(
        &src,
        &[Deletion::ByName("readme.txt".into())],
        &[("new\\imported.txt".into(), b"fresh content".to_vec())],
    );

    assert_eq!(out.report.deleted, ["readme.txt"]);
    assert_eq!(out.report.added, ["new\\imported.txt"]);
    assert!(out.report.dropped.contains(&"(attributes)".to_string()));
    assert!(out.report.replaced.is_empty());

    let (archive, names) = open(&out.bytes).unwrap();
    assert_eq!(names.len(), 3); // table.txt, secret.bin, imported.txt
    assert!(names.contains(&"data/table.txt".to_string()));
    assert!(names.contains(&"new/imported.txt".to_string()));
    assert!(!archive.has_file("readme.txt"));
    // (attributes) 被丢弃：修改会使它的索引失效。
    assert!(!archive.has_file("(attributes)"));
    assert!(archive.attributes().is_none());

    // 保留的文件内容保持完全一致（原始字节复制）。
    assert_eq!(archive.read_file("data\\table.txt").unwrap(), b"1,2,3\n4,5,6\n");
    assert_eq!(archive.read_file("secret.bin").unwrap(), b"top secret payload");
    // 导入的文件可以读回。
    assert_eq!(archive.read_file("new\\imported.txt").unwrap(), b"fresh content");
}

#[test]
fn replace_existing_file() {
    let src = build(&spec(1));
    let out = modify(
        &src,
        &[],
        &[(
            "Data/Table.txt".into(), // 斜杠 + 不同大小写仍必须匹配
            b"replaced".to_vec(),
        )],
    );

    // 报告显示清理后（规范化）的名称。
    assert_eq!(out.report.replaced, ["Data\\Table.txt"]);
    assert!(out.report.added.is_empty());

    let (archive, names) = open(&out.bytes).unwrap();
    assert_eq!(names.len(), 3); // 数量相同，其中一个被替换
    assert_eq!(archive.read_file("data\\table.txt").unwrap(), b"replaced");
}

#[test]
fn prefix_userdata_and_versions_preserved() {
    for (version, userdata, garbage) in [(0, false, 0usize), (1, true, 0), (2, false, 0x200)] {
        let mut s = spec(version);
        s.userdata = userdata;
        s.prefix_garbage = garbage;
        let src = build(&s);
        let out = modify(&src, &[], &[("x.txt".into(), b"x".to_vec())]);

        // 包装头 / 垃圾前缀必须逐字节保留。
        assert_eq!(
            &out.bytes[..archive_base(&src)],
            &src[..archive_base(&src)],
            "prefix lost for version {version}"
        );
        let (archive, _) = open(&out.bytes).unwrap();
        // 版本保持不变（v1 仍为 v1，v2/v3 亦然）。
        assert_eq!(archive.version().as_u32(), version as u32 + 1);
        assert_eq!(archive.read_file("x.txt").unwrap(), b"x");
        assert_eq!(archive.read_file("readme.txt").unwrap(), b"keep me\n");
    }
}

/// 构建的归档内 MPQ 头部的偏移（0x200 对齐扫描）。
fn archive_base(src: &[u8]) -> usize {
    mpq::locate_header(src).unwrap() as usize
}

#[test]
fn hetbet_archive_gets_classic_tables() {
    let files = vec![
        ("alpha.txt".to_string(), b"AAA".to_vec()),
        ("beta\\b.txt".to_string(), b"BBB".to_vec()),
    ];
    let src = build_hetbet(&files);
    // 修改前：即使没有 listfile，HET 查找也能解析名称。
    assert!(Archive::new(Cursor::new(&src)).unwrap().has_file("alpha.txt"));

    let out = modify(
        &src,
        &[Deletion::ByName("alpha.txt".into())],
        &[("gamma.txt".into(), b"G".to_vec())],
    );

    let (archive, names) = open(&out.bytes).unwrap();
    assert!(!archive.uses_het_bet()); // HET/BET 被经典表取代
    // beta\b.txt 仍是一个未命名数据块：纯 HET/BET 归档没有可用于
    // 构建经典 hash 条目的名称（未命名的经典 block 不会出现在读取器
    // 的列表中，但其字节保留下来）。
    assert_eq!(names, ["gamma.txt".to_string()]);
    assert_eq!(
        archive.header().block_table_size,
        3, // beta（未命名）+ gamma + (listfile)
        "beta data block must survive"
    );
    assert_eq!(archive.read_file("gamma.txt").unwrap(), b"G");
    assert!(!archive.has_file("alpha.txt"));
}

#[test]
fn unnamed_entries_survive_without_listfile() {
    // 没有 listfile：条目未命名，但仍可通过原始 hash 槽位寻址。
    // 修改器必须保留这一点。
    let mut s = spec(0);
    s.with_listfile = false;
    s.with_attributes = false;
    let src = build(&s);

    let out = modify(&src, &[], &[("later.txt".into(), b"added".to_vec())]);

    let (archive, names) = open(&out.bytes).unwrap();
    // 新增文件进入重建的 listfile；readme.txt 虽不在源 listfile
    //（源没有 listfile），但内置游戏清单恢复了它的名字，修改器
    // 随之写进重建的 listfile。data\table.txt / secret.bin 仍无名。
    assert_eq!(names, ["readme.txt".to_string(), "later.txt".to_string()]);
    // 先前未命名的文件仍然可以用原名称读取 — 证明 hash 槽位被
    // 保留而非重建。
    assert_eq!(archive.read_file("readme.txt").unwrap(), b"keep me\n");
    assert_eq!(archive.read_file("data\\table.txt").unwrap(), b"1,2,3\n4,5,6\n");
    assert_eq!(archive.read_file("secret.bin").unwrap(), b"top secret payload");
    assert_eq!(archive.read_file("later.txt").unwrap(), b"added");
}

#[test]
fn delete_unnamed_entry_by_key() {
    let mut s = spec(0);
    s.with_listfile = false;
    s.with_attributes = false;
    let src = build(&s);

    let archive = Archive::new(Cursor::new(&src)).unwrap();
    let target = archive
        .entries()
        .into_iter()
        .find(|e| e.size as usize == "top secret payload".len())
        .expect("entry");
    let key = (target.source, target.block_index);

    let out = modify(&src, &[Deletion::ByKey(key.0, key.1)], &[]);
    assert_eq!(out.report.deleted.len(), 1);

    let (archive, _) = open(&out.bytes).unwrap();
    assert!(!archive.has_file("secret.bin"));
    assert_eq!(archive.read_file("readme.txt").unwrap(), b"keep me\n");
}

#[test]
fn delete_by_name_in_listfile_less_archive() {
    // 受保护的映射：没有 listfile，但名称可通过哈希表寻址。
    // 按名称删除必须有效，且其他（仍未命名的）条目保持可寻址。
    let mut s = spec(0);
    s.with_listfile = false;
    s.with_attributes = false;
    let src = build(&s);

    let out = modify(&src, &[Deletion::ByName("DATA\\table.txt".into())], &[]);
    assert_eq!(out.report.deleted, ["DATA\\table.txt"]);

    let (archive, _) = open(&out.bytes).unwrap();
    assert!(!archive.has_file("data\\table.txt"));
    assert_eq!(archive.read_file("readme.txt").unwrap(), b"keep me\n");
    assert_eq!(archive.read_file("secret.bin").unwrap(), b"top secret payload");
}

#[test]
fn key_v2_files_are_re_encrypted_at_new_offset() {
    let s = ArchiveSpec {
        version: 1,
        sector_shift: 3,
        with_listfile: true,
        with_attributes: false,
        with_hi_block: false,
        userdata: false,
        prefix_garbage: 0,
        zero_archive_size: false,
        raw_chunk_size: 0,
        files: vec![
            FileSpec::text("filler.txt", "filler").compress(),
            FileSpec::text("fixed.bin", "key-v2 payload").encrypt().fix_key().compress(),
        ],
    };
    let src = build(&s);
    let out = modify(&src, &[], &[("appended.bin".into(), b"tail".to_vec())]);

    assert_eq!(out.report.re_encoded, ["fixed.bin"]);
    let (archive, _) = open(&out.bytes).unwrap();
    // 在新偏移处可读，内容相同…
    assert_eq!(archive.read_file("fixed.bin").unwrap(), b"key-v2 payload");
    assert_eq!(archive.read_file("filler.txt").unwrap(), b"filler");
    // …并且仍以 key-v2 加密（默认选项）。
    let e = archive.find_file("fixed.bin").unwrap();
    assert!(e.flags & mpq::FILE_ENCRYPTED != 0);
    assert!(e.flags & mpq::FILE_KEY_V2 != 0);
}

#[test]
fn key_v2_plaintext_option() {
    let s = ArchiveSpec {
        version: 0,
        sector_shift: 3,
        with_listfile: true,
        with_attributes: false,
        with_hi_block: false,
        userdata: false,
        prefix_garbage: 0,
        zero_archive_size: false,
        raw_chunk_size: 0,
        files: vec![
            FileSpec::text("filler.txt", "filler").compress(),
            FileSpec::text("fixed.bin", "key-v2 payload").encrypt().fix_key().compress(),
        ],
    };
    let src = build(&s);
    let out = mpq::modify_with(
        &src,
        &[],
        &[("appended.bin".into(), b"tail".to_vec())],
        &mpq::ModifyOptions {
            reencrypt_key_v2: false,
            ..Default::default()
        },
    )
    .expect("modify");

    assert_eq!(out.report.re_encoded, ["fixed.bin"]);
    let (archive, _) = open(&out.bytes).unwrap();
    assert_eq!(archive.read_file("fixed.bin").unwrap(), b"key-v2 payload");
    let e = archive.find_file("fixed.bin").unwrap();
    assert_eq!(e.flags & mpq::FILE_ENCRYPTED, 0, "should be stored plainly");
}

#[test]
fn key_v2_multisector_re_encryption() {
    // 多 sector 的 key-v2 文件，混合可压缩与不可压缩区域：覆盖
    // sector 偏移表（key-1）、每个 sector 的密钥（key+i）以及
    // dword 填充的 sector。
    let mut content = Vec::new();
    for i in 0..4000u32 {
        content.extend_from_slice(format!("line {i:06} of the payload\n").as_bytes());
    }
    let noise: Vec<u8> = (0..5000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
    content.extend_from_slice(&noise);
    content.extend_from_slice(b"tail");

    let s = ArchiveSpec {
        version: 0,
        sector_shift: 3, // 4096 字节的 sector
        with_listfile: true,
        with_attributes: false,
        with_hi_block: false,
        userdata: false,
        prefix_garbage: 0,
        zero_archive_size: false,
        raw_chunk_size: 0,
        files: vec![
            FileSpec::bytes("filler.bin", b"filler".to_vec()),
            FileSpec::bytes("big_fixed.bin", content.clone())
                .encrypt()
                .fix_key()
                .compress(),
        ],
    };
    let src = build(&s);
    let out = modify(&src, &[], &[("appended.bin".into(), b"tail".to_vec())]);

    let (archive, _) = open(&out.bytes).unwrap();
    assert_eq!(archive.read_file("big_fixed.bin").unwrap(), content);
    let e = archive.find_file("big_fixed.bin").unwrap();
    assert!(e.flags & mpq::FILE_ENCRYPTED != 0);
    assert!(e.flags & mpq::FILE_KEY_V2 != 0);
}

#[test]
fn large_imports_and_many_files() {
    let mut s = spec(0);
    // 一个跨多个 sector 的文件，外加大量不可压缩数据。
    let mut big = Vec::new();
    for i in 0..20_000u32 {
        big.extend_from_slice(&i.to_le_bytes());
    }
    let noise: Vec<u8> = (0..30_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
    s.files.push(FileSpec::bytes("big.bin", big.clone()));
    s.files.push(FileSpec::bytes("noise.bin", noise.clone()));
    let src = build(&s);

    let mut additions: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..40 {
        additions.push((format!("imported\\file{i:02}.txt"), format!("content {i}").into_bytes()));
    }
    let out = modify(&src, &[Deletion::ByName("noise.bin".into())], &additions);

    let (archive, _) = open(&out.bytes).unwrap();
    assert_eq!(archive.read_file("big.bin").unwrap(), big);
    assert!(!archive.has_file("noise.bin"));
    for i in 0..40 {
        let name = format!("imported\\file{i:02}.txt");
        assert_eq!(
            archive.read_file(&name).unwrap(),
            format!("content {i}").into_bytes()
        );
    }
}

#[test]
fn empty_file_import_and_incompressible_sector_fallback() {
    let src = build(&spec(0));
    let incompressible: Vec<u8> = (0..4096u32).map(|i| (i * 71 + 13) as u8).collect();
    let out = modify(
        &src,
        &[],
        &[
            ("empty.dat".into(), Vec::new()),
            ("random.bin".into(), incompressible.clone()),
        ],
    );

    let (archive, _) = open(&out.bytes).unwrap();
    assert_eq!(archive.read_file("empty.dat").unwrap(), Vec::<u8>::new());
    assert_eq!(archive.read_file("random.bin").unwrap(), incompressible);
}

#[test]
fn deleting_everything_is_rejected() {
    let src = build(&spec(0));
    let result = mpq::modify(
        &src,
        &[
            Deletion::ByName("readme.txt".into()),
            Deletion::ByName("data\\table.txt".into()),
            Deletion::ByName("secret.bin".into()),
        ],
        &[],
    );
    assert!(result.is_err());
}

#[test]
fn no_changes_returns_input_unchanged() {
    let src = build(&spec(0));
    let out = modify(&src, &[], &[]);
    assert_eq!(out.bytes, src);
}

#[test]
fn forbidden_and_invalid_import_names() {
    let src = build(&spec(0));
    assert!(mpq::modify(&src, &[], &[("(listfile)".into(), b"x".to_vec())]).is_err());
    // 目录穿越部分被剥离而非报错（与防路径逃逸的提取采用相同的
    // 规范化）；开头的 "ok" 段保留。
    let out = modify(&src, &[], &[("ok/../name.txt".into(), b"x".to_vec())]);
    let (archive, _) = open(&out.bytes).unwrap();
    assert_eq!(archive.read_file("ok\\name.txt").unwrap(), b"x");
}

#[test]
fn trailing_bytes_behind_archive_are_kept() {
    let mut src = build(&spec(0));
    src.extend_from_slice(b"TRAILING-JUNK");
    let out = modify(&src, &[], &[("x.txt".into(), b"x".to_vec())]);
    assert!(out.bytes.ends_with(b"TRAILING-JUNK"));
    let (archive, _) = open(&out.bytes).unwrap();
    assert_eq!(archive.read_file("x.txt").unwrap(), b"x");
}
