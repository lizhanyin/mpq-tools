//! 归档对比（diff_archives）的集成测试。

mod common;

use std::io::Cursor;

use common::{build, build_hetbet_with_listfile, ArchiveSpec, FileSpec};
use mpq::{diff_archives, Archive, DiffKind};

type OpenArchive<'a> = Archive<Cursor<&'a [u8]>>;

fn open(bytes: &[u8]) -> OpenArchive<'_> {
    Archive::new(Cursor::new(bytes)).expect("open")
}

fn spec(files: Vec<FileSpec>) -> ArchiveSpec {
    ArchiveSpec {
        version: 0,
        sector_shift: 3,
        with_listfile: true,
        with_attributes: true,
        files,
        ..ArchiveSpec::default()
    }
}

/// 基础分类：相同 / 修改（大小相同内容不同）/ 修改（大小不同）/
/// 新增 / 移除，以及内部文件（(listfile)/(attributes)）不参与。
#[test]
fn classifies_all_kinds() {
    let old = build(&spec(vec![
        FileSpec::text("same.txt", "unchanged\n"),          // 相同
        FileSpec::text("edited.txt", "old content\n"),      // 大小相同内容不同
        FileSpec::text("resized.bin", "short"),             // 大小不同
        FileSpec::text("gone.txt", "only in old\n"),        // 移除
    ]));
    let new = build(&spec(vec![
        FileSpec::text("same.txt", "unchanged\n"),
        FileSpec::text("edited.txt", "new content\n"),
        FileSpec::text("resized.bin", "much longer content now"),
        FileSpec::text("fresh.txt", "only in new\n"),       // 新增
    ]));

    let report = diff_archives(&open(&old), &open(&new), None).expect("diff");
    assert!(report.read_errors.is_empty());

    let find = |name: &str| {
        report
            .entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
            .unwrap_or_else(|| panic!("missing {name} in {:?}", report.entries))
    };
    assert_eq!(find("same.txt").kind, DiffKind::Same);
    assert_eq!(find("edited.txt").kind, DiffKind::Changed);
    assert_eq!(find("resized.bin").kind, DiffKind::Changed);
    assert_eq!(find("fresh.txt").kind, DiffKind::Added);
    assert_eq!(find("gone.txt").kind, DiffKind::Removed);

    // 内部文件绝不出现；大小字段按侧填充。
    assert!(report
        .entries
        .iter()
        .all(|e| !e.name.starts_with('(')));
    let added = find("fresh.txt");
    assert_eq!(added.old_size, None);
    assert_eq!(added.new_size, Some("only in new\n".len() as u64));
    let removed = find("gone.txt");
    assert_eq!(removed.old_size, Some("only in old\n".len() as u64));
    assert_eq!(removed.new_size, None);

    // 结果按名字排序（大小写不敏感）。
    let mut sorted: Vec<&str> = report.entries.iter().map(|e| e.name.as_str()).collect();
    sorted.sort_by_key(|s| s.to_lowercase());
    let names: Vec<&str> = report.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, sorted);

    // 条目数 = 两侧具名文件并集（4 个用户文件；listfile/attributes 排除）。
    assert_eq!(report.entries.len(), 5);
}

/// 压缩 / 加密状态不同但解压后内容相同 → 相同（比较的是解压字节）。
#[test]
fn storage_flags_do_not_matter() {
    let plain = build(&spec(vec![FileSpec::text("data.bin", &"x".repeat(4096))]));
    let packed = build(&spec(vec![FileSpec::text("data.bin", &"x".repeat(4096)).compress()]));
    let locked = build(&spec(vec![
        FileSpec::text("data.bin", &"x".repeat(4096)).compress().encrypt(),
    ]));

    let a = diff_archives(&open(&plain), &open(&packed), None).unwrap();
    assert_eq!(a.entries[0].kind, DiffKind::Same);
    let b = diff_archives(&open(&packed), &open(&locked), None).unwrap();
    assert_eq!(b.entries[0].kind, DiffKind::Same);
}

/// 名字配对遵循 MPQ 规范化：大小写与斜杠方向不影响，
/// 显示名取新侧拼写。
#[test]
fn name_normalization_pairs_entries() {
    let old = build(&spec(vec![
        FileSpec::text("Data\\Table.txt", "v1\n"),
        FileSpec::text("War3Map.j", "script v1\n"),
    ]));
    let new = build(&spec(vec![
        FileSpec::text("data/table.TXT", "v1\n"),
        FileSpec::text("war3map.j", "script v2\n"),
    ]));

    let report = diff_archives(&open(&old), &open(&new), None).unwrap();
    assert_eq!(report.entries.len(), 2);
    // 大小写 / 斜杠变体配对成功：不是 Added + Removed，且显示名是新侧拼写。
    let table = &report.entries[0];
    assert_eq!(table.name, "data/table.TXT");
    assert_eq!(table.kind, DiffKind::Same);
    let script = &report.entries[1];
    assert_eq!(script.name, "war3map.j");
    assert_eq!(script.kind, DiffKind::Changed);
}

/// v4 经典表侧与 HET/BET（v3）侧可以互相对比。
#[test]
fn cross_table_and_version() {
    let old = build(&ArchiveSpec {
        version: 3, // v4
        with_listfile: true,
        files: vec![
            FileSpec::text("keep.txt", "same\n").compress(),
            FileSpec::text("old-only.txt", "bye\n"),
        ],
        ..ArchiveSpec::default()
    });
    let hetbet = build_hetbet_with_listfile(&[
        ("keep.txt".into(), b"same\n".to_vec()),
        ("new-only.txt".into(), b"hi\n".to_vec()),
    ]);

    let report = diff_archives(&open(&old), &open(&hetbet), None).unwrap();
    let find = |name: &str| {
        report
            .entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
            .unwrap()
    };
    assert_eq!(find("keep.txt").kind, DiffKind::Same);
    assert_eq!(find("old-only.txt").kind, DiffKind::Removed);
    assert_eq!(find("new-only.txt").kind, DiffKind::Added);
}

/// 进度回调单调递增、终值等于两侧具名条目总数。
#[test]
fn progress_reports_total() {
    let old = build(&spec(vec![FileSpec::text("a.txt", "a\n")]));
    let new = build(&spec(vec![
        FileSpec::text("a.txt", "a\n"),
        FileSpec::text("b.txt", "b\n"),
    ]));
    let seen = std::sync::atomic::AtomicUsize::new(0);
    let peak = std::sync::atomic::AtomicUsize::new(0);
    let report = diff_archives(&open(&old), &open(&new), Some(&|done, total| {
        assert_eq!(total, 3); // 1 + 2 个具名条目
        seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        peak.fetch_max(done, std::sync::atomic::Ordering::Relaxed);
    }))
    .unwrap();
    assert!(seen.load(std::sync::atomic::Ordering::Relaxed) >= 3);
    assert_eq!(
        peak.load(std::sync::atomic::Ordering::Relaxed),
        3,
        "progress should reach total"
    );
    assert_eq!(report.entries.len(), 2);
}
