//! 补丁生成（tasks::build_patch）的集成测试：create + modify 构造
//! 两个归档，diff 取差异后打包，验证补丁内容与来源一致。

use std::io::Cursor;

use mpq::{diff_archives, Archive, CreateOptions, DiffKind};

type Bytes = Vec<u8>;

/// 用「空归档 + modify」从零构造一个含指定文件的归档。
fn make(files: &[(&str, &[u8])]) -> Bytes {
    let base = mpq::create(&CreateOptions::default()).expect("create");
    let adds: Vec<(String, Vec<u8>)> = files
        .iter()
        .map(|(n, d)| (n.to_string(), d.to_vec()))
        .collect();
    mpq::modify(&base, &[], &adds).expect("modify").bytes
}

#[test]
fn patch_contains_only_differences_with_new_content() {
    let old = make(&[
        ("same.txt", b"identical\n"),
        ("b.txt", b"version 1\n"),
        ("old-only.txt", b"kept in old\n"),
        ("d\\big.bin", &[7u8; 9000]),
    ]);
    let new = make(&[
        ("same.txt", b"identical\n"),
        ("b.txt", b"version 2\n"),
        ("new-only.txt", b"fresh\n"),
        ("d\\big.bin", &[7u8; 9000]),
    ]);

    let old_arc = Archive::new(Cursor::new(&old)).unwrap();
    let new_arc = Archive::new(Cursor::new(&new)).unwrap();
    let report = diff_archives(&old_arc, &new_arc, None).unwrap();

    let mut names: Vec<String> = report
        .entries
        .iter()
        .filter(|e| e.kind == DiffKind::Added || e.kind == DiffKind::Changed)
        .map(|e| e.name.clone())
        .collect();
    names.sort();
    assert_eq!(names, ["b.txt", "new-only.txt"]);

    let outcome = mpqtool::tasks::build_patch(&new_arc, &names, mpq::FormatVersion::V1, None, None)
        .expect("build_patch");

    // 补丁可打开，恰好包含差异文件，内容为新版本。
    let patch = Archive::new(Cursor::new(&outcome.bytes)).unwrap();
    assert_eq!(patch.version(), mpq::FormatVersion::V1);
    assert!(!patch.has_file("same.txt"));
    assert!(!patch.has_file("old-only.txt"));
    assert!(!patch.has_file("d\\big.bin"));
    assert_eq!(patch.read_file("b.txt").unwrap(), b"version 2\n");
    assert_eq!(patch.read_file("new-only.txt").unwrap(), b"fresh\n");
    // 全部为新增（基线是空归档）。
    assert_eq!(outcome.report.added.len(), 2);
    assert!(outcome.report.replaced.is_empty());
}

#[test]
fn patch_respects_requested_version() {
    let new = make(&[("x.txt", b"hello\n")]);
    let new_arc = Archive::new(Cursor::new(&new)).unwrap();
    let names = vec!["x.txt".to_string()];
    let outcome =
        mpqtool::tasks::build_patch(&new_arc, &names, mpq::FormatVersion::V4, None, None).unwrap();
    let patch = Archive::new(Cursor::new(&outcome.bytes)).unwrap();
    assert_eq!(patch.version(), mpq::FormatVersion::V4);
    assert_eq!(patch.read_file("x.txt").unwrap(), b"hello\n");
}

#[test]
fn build_patch_fails_on_unknown_name() {
    let new = make(&[("x.txt", b"hello\n")]);
    let new_arc = Archive::new(Cursor::new(&new)).unwrap();
    let err = mpqtool::tasks::build_patch(
        &new_arc,
        &["missing.txt".to_string()],
        mpq::FormatVersion::V1,
        None,
        None,
    );
    assert!(err.is_err());
}

#[test]
fn patch_from_protected_style_roundtrip_via_modify() {
    // 场景串联：patch 应用到 old（靠 modify 同名替换）后等于 new 的
    // 全部内容——这正是「合并 / 打补丁」语义的端到端验证。
    let old = make(&[("a.txt", b"A v1\n"), ("b.txt", b"B v1\n")]);
    let new = make(&[("a.txt", b"A v2\n"), ("b.txt", b"B v1\n"), ("c.txt", b"C\n")]);
    let old_arc = Archive::new(Cursor::new(&old)).unwrap();
    let new_arc = Archive::new(Cursor::new(&new)).unwrap();
    let report = diff_archives(&old_arc, &new_arc, None).unwrap();
    let names: Vec<String> = report
        .entries
        .iter()
        .filter(|e| e.kind == DiffKind::Added || e.kind == DiffKind::Changed)
        .map(|e| e.name.clone())
        .collect();

    let outcome = mpqtool::tasks::build_patch(&new_arc, &names, mpq::FormatVersion::V1, None, None)
        .unwrap();
    // 把补丁里的文件应用回 old：同名替换 + 新增。
    let patch = Archive::new(Cursor::new(&outcome.bytes)).unwrap();
    let mut additions = Vec::new();
    for e in patch.entries() {
        let name = e.name.clone().expect("patch entries are named");
        // (listfile) 等内部文件由修改器自行管理，不可作为导入项。
        if mpq::SPECIAL_FILES.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
            continue;
        }
        additions.push((name, patch.read_entry(&e).unwrap()));
    }
    let merged = mpq::modify(&old, &[], &additions).unwrap();

    // 结果与 new 的文件集合和内容一致。
    let merged_arc = Archive::new(Cursor::new(&merged.bytes)).unwrap();
    assert_eq!(merged_arc.read_file("a.txt").unwrap(), b"A v2\n");
    assert_eq!(merged_arc.read_file("b.txt").unwrap(), b"B v1\n");
    assert_eq!(merged_arc.read_file("c.txt").unwrap(), b"C\n");
    assert_eq!(merged_arc.file_names().len(), new_arc.file_names().len());
    // 顺带验证：删除在合并 / 补丁语义中不可表达，old 独有文件保持存在。
    assert!(merged_arc.has_file("b.txt"));
}
