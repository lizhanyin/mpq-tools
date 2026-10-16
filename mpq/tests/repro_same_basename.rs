//! 复现：不同目录存在同名文件时「只导入子项」批量添加。
//! adds 形如 dirA\file.txt 与 dirB\file.txt（基名相同、目录不同），
//! 一次 modify 全部写入后必须都能按各自完整路径读回。

mod common;

use std::io::Cursor;

use common::{build, build_hetbet, ArchiveSpec, FileSpec};
use mpq::Archive;

type OpenArchive<'a> = Archive<Cursor<&'a [u8]>>;

fn base_spec() -> ArchiveSpec {    ArchiveSpec {
        version: 0,
        sector_shift: 3,
        with_listfile: true,
        with_attributes: true,
        with_hi_block: false,
        userdata: false,
        prefix_garbage: 0,
        zero_archive_size: false,
        raw_chunk_size: 0,
        files: vec![FileSpec::text("readme.txt", "keep me\n")],
    }
}

fn check_same_basename_pairs(src: &[u8]) {
    let adds = vec![
        ("dirA\\file.txt".to_string(), b"content A".to_vec()),
        ("dirB\\file.txt".to_string(), b"content B".to_vec()),
        ("nested\\dirA\\file.txt".to_string(), b"content C".to_vec()),
    ];
    let out = mpq::modify(src, &[], &adds).expect("modify");

    let archive = Archive::new(Cursor::new(&out.bytes[..])).expect("reopen");
    assert_eq!(
        archive.read_file("dirA\\file.txt").expect("read dirA"),
        b"content A",
        "dirA\\file.txt 内容错误（可能与其它同名文件串了）"
    );
    assert_eq!(
        archive.read_file("dirB\\file.txt").expect("read dirB"),
        b"content B",
        "dirB\\file.txt 内容错误（可能与其它同名文件串了）"
    );
    assert_eq!(
        archive.read_file("nested\\dirA\\file.txt").expect("read nested"),
        b"content C",
        "nested\\dirA\\file.txt 内容错误（可能与其它同名文件串了）"
    );

    // 三个名字都必须出现在条目列表里（不能互相顶掉）。
    let names = archive.file_names();
    for want in ["dirA\\file.txt", "dirB\\file.txt", "nested\\dirA\\file.txt"] {
        assert!(
            names.iter().any(|n| n.eq_ignore_ascii_case(want)),
            "条目列表缺少 {want}，现有：{names:?}"
        );
    }
}

#[test]
fn same_basename_different_dirs_classic() {
    check_same_basename_pairs(&build(&base_spec()));
}

#[test]
fn same_basename_different_dirs_hetbet() {
    let src = build_hetbet(&[("readme.txt".to_string(), b"keep me\n".to_vec())]);
    check_same_basename_pairs(&src);
}
