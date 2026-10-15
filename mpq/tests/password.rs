//! 密码加密（方案②，自定义 v4 AEAD 扩展）端到端测试：
//! protect / unlock / read 的往返、错误密码拒绝、
//! 密码修改、unprotect、受保护归档上的修改、篡改检测，
//! 以及对转换到 v1–v3 的拒绝。

mod common;

use std::io::Cursor;

use common::{build, ArchiveSpec, FileSpec};
use mpq::{
    change_password, convert, protect, unprotect, Argon2Params, Archive, Deletion, FormatVersion,
    ModifyOptions, PASSWORD_FILE_NAME,
};

/// 便宜的 Argon2 参数，让测试套件保持快速（真实调用方应使用
/// 更强的 [`Argon2Params::default`]）。
fn weak() -> Argon2Params {
    Argon2Params::new(64, 1, 1).unwrap()
}

fn open(bytes: &[u8]) -> mpq::Result<Archive<Cursor<&[u8]>>> {
    Archive::new(Cursor::new(bytes))
}

fn spec() -> ArchiveSpec {
    ArchiveSpec {
        version: 0, // v1 — protect 必须把容器升级到 v4
        sector_shift: 3,
        with_listfile: true,
        files: vec![
            FileSpec::text("readme.txt", "small file\n"),
            FileSpec::text(
                "data\\big.bin",
                &"0123456789abcdef".repeat(1024), // 16 KiB → 多 sector
            )
            .compress(),
            FileSpec::text("plain.bin", "\0\0\0\0"), // 不可压缩的尾部
        ],
        ..ArchiveSpec::default()
    }
}

fn protected() -> Vec<u8> {
    let bytes = build(&spec());
    protect(&bytes, "hunter2", &weak()).unwrap().bytes
}

/// 源归档中每个有名称的文件，用于往返比较。
fn contents(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let a = open(bytes).unwrap();
    a.entries()
        .iter()
        .filter(|e| e.name.is_some() && e.name.as_deref() != Some("(listfile)"))
        .map(|e| {
            (
                e.name.clone().unwrap(),
                a.read_entry(e).expect("source is readable"),
            )
        })
        .collect()
}

// -- protect / read 往返 -----------------------------------------------------

#[test]
fn protect_upgrades_to_v4_and_reads_back_after_unlock() {
    let src = build(&spec());
    let expected = contents(&src);

    let enc = protect(&src, "hunter2", &weak()).unwrap();
    assert_eq!(enc.report.encrypted, expected.len());
    assert!(enc.master_key.is_some());

    let mut a = open(&enc.bytes).unwrap();
    assert_eq!(a.version(), FormatVersion::V4);
    assert!(a.is_password_protected());
    assert!(!a.is_unlocked());

    a.unlock("hunter2").unwrap();
    assert!(a.is_unlocked());
    for (name, data) in &expected {
        assert_eq!(&a.read_file(name).unwrap(), data, "roundtrip of {name}");
    }
    // vault 本身存在，有名称且为明文。
    assert!(a.has_file(PASSWORD_FILE_NAME));
}

#[test]
fn names_are_visible_while_locked_but_reads_require_password() {
    let enc = protected();
    let a = open(&enc).unwrap();
    let names: Vec<_> = a.file_names().to_vec();
    assert!(names.contains(&"readme.txt".to_string()));

    let err = a.read_file("readme.txt").unwrap_err();
    assert!(matches!(err, mpq::Error::PasswordRequired), "got: {err}");
}

#[test]
fn wrong_password_is_rejected_immediately() {
    let enc = protected();
    let mut a = open(&enc).unwrap();
    let err = a.unlock("wrong").unwrap_err();
    assert!(matches!(err, mpq::Error::PasswordMismatch), "got: {err}");
    assert!(!a.is_unlocked());
}

// -- 篡改检测 ----------------------------------------------------------

#[test]
fn flipped_byte_in_sealed_sector_fails_the_tag() {
    let enc = protected();
    let mut a = open(&enc).unwrap();
    a.unlock("hunter2").unwrap();
    let entry = a.find_file("data\\big.bin").unwrap();

    let mut tampered = enc.clone();
    let at = a.archive_offset() as usize + entry.offset as usize + 40;
    tampered[at] ^= 0x40;

    let mut b = open(&tampered).unwrap();
    b.unlock("hunter2").unwrap();
    let err = b.read_file("data\\big.bin").unwrap_err();
    assert!(
        matches!(err, mpq::Error::Crypto(_)),
        "AEAD must catch the tamper, got: {err}"
    );
    // 其他文件不受影响（每个 sector 有独立的 tag）。
    assert_eq!(b.read_file("readme.txt").unwrap(), b"small file\n");
}

// -- 密码修改 ------------------------------------------------------------

#[test]
fn change_password_rewraps_without_reencrypting() {
    let enc = protected();
    let changed = change_password(&enc, "hunter2", "n3w-pw", &weak()).unwrap();

    // 长度相同：只有固定大小的 vault 记录被重写。
    assert_eq!(changed.len(), enc.len());
    // （字节不同 — 换了新 salt。）
    assert_ne!(changed, enc);

    let mut a = open(&changed).unwrap();
    assert!(a.unlock("hunter2").is_err(), "old password refused");
    a.unlock("n3w-pw").unwrap();
    assert_eq!(a.read_file("readme.txt").unwrap(), b"small file\n");
    let big = a.read_file("data\\big.bin").unwrap();
    assert_eq!(big.len(), 16 * 1024);

    // 旧密码错误时，在任何重写发生之前就会被拒绝。
    let err = change_password(&enc, "nope", "x", &weak()).unwrap_err();
    assert!(matches!(err, mpq::Error::PasswordMismatch), "got: {err}");
}

// -- unprotect -----------------------------------------------------------------

#[test]
fn unprotect_roundtrips_to_plain() {
    let src = build(&spec());
    let expected = contents(&src);
    let enc = protect(&src, "hunter2", &weak()).unwrap().bytes;

    let plain = unprotect(&enc, "hunter2").unwrap();
    assert_eq!(plain.report.decrypted, expected.len());
    let a = open(&plain.bytes).unwrap();
    assert!(!a.is_password_protected());
    assert!(!a.has_file(PASSWORD_FILE_NAME));
    for (name, data) in &expected {
        assert_eq!(&a.read_file(name).unwrap(), data, "unprotected {name}");
    }

    let err = unprotect(&enc, "wrong", ).unwrap_err();
    assert!(matches!(err, mpq::Error::PasswordMismatch), "got: {err}");
    let err = unprotect(&src, "hunter2").unwrap_err();
    assert!(matches!(err, mpq::Error::InvalidData(_)), "got: {err}");
}

// -- 在受保护归档上执行修改 ----------------------------------------------

#[test]
fn modify_adds_and_deletes_on_protected_archive() {
    let src = build(&spec());
    let enc = protect(&src, "hunter2", &weak()).unwrap().bytes;

    // 解锁以获取供修改器使用的主密钥。
    let mut probe = open(&enc).unwrap();
    probe.unlock("hunter2").unwrap();
    let master = probe.master_key().unwrap();

    let outcome = mpq::modify_with(
        &enc,
        &[Deletion::ByName("plain.bin".to_string())],
        &[("added.txt".to_string(), b"brand new".to_vec())],
        &ModifyOptions {
            master_key: Some(master),
            ..Default::default()
        },
    )
    .unwrap();

    let mut a = open(&outcome.bytes).unwrap();
    assert!(a.is_password_protected());
    a.unlock("hunter2").unwrap();
    assert_eq!(a.read_file("readme.txt").unwrap(), b"small file\n");
    assert_eq!(a.read_file("data\\big.bin").unwrap().len(), 16 * 1024);
    assert_eq!(a.read_file("added.txt").unwrap(), b"brand new");
    assert!(!a.has_file("plain.bin"));
    // 新增的文件确实被密封，而不是明文存储。
    let entry = a.find_file("added.txt").unwrap();
    assert!(entry.is_password_aead());
    let entry = a.find_file("readme.txt").unwrap();
    assert!(entry.is_password_aead());

    // 没有主密钥时，修改会被拒绝并给出明确的错误。
    let err = mpq::modify_with(
        &enc,
        &[],
        &[("x.txt".to_string(), b"x".to_vec())],
        &ModifyOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(err, mpq::Error::InvalidData(_)), "got: {err}");
}

#[test]
fn deleting_the_vault_directly_is_refused() {
    let enc = protected();
    let err = mpq::modify(
        &enc,
        &[Deletion::ByName("(password)".to_string())],
        &[],
    )
    .unwrap_err();
    assert!(matches!(err, mpq::Error::InvalidData(_)), "got: {err}");
}

// -- 版本转换策略 --------------------------------------------------

#[test]
fn converting_protected_archive_to_v1_v3_is_refused() {
    let enc = protected();
    for target in [FormatVersion::V1, FormatVersion::V2, FormatVersion::V3] {
        let err = convert(&enc, target).unwrap_err();
        assert!(matches!(err, mpq::Error::InvalidData(_)), "got: {err}");
    }
    // 转换到 v4（携带主密钥）仍然可行。
    let mut probe = open(&enc).unwrap();
    probe.unlock("hunter2").unwrap();
    let out = mpq::modify_with(
        &enc,
        &[],
        &[],
        &ModifyOptions {
            target_version: Some(FormatVersion::V4),
            master_key: probe.master_key(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(open(&out.bytes).unwrap().version(), FormatVersion::V4);
}

// -- single-unit 与空文件边界情况 --------------------------------------------

#[test]
fn protect_of_v4_and_single_unit_files() {
    let src = build(&ArchiveSpec {
        version: 3, // 已经是 v4
        sector_shift: 3,
        with_listfile: true,
        files: vec![
            FileSpec::text("tiny.txt", "tiny").single_unit(),
            FileSpec::text("empty.txt", ""),
            FileSpec::text("su-comp.txt", "compressible compressible compressible")
                .single_unit()
                .compress(),
        ],
        ..ArchiveSpec::default()
    });
    let expected = contents(&src);
    let enc = protect(&src, "pw", &weak()).unwrap();
    assert_eq!(open(&enc.bytes).unwrap().version(), FormatVersion::V4);

    let mut a = open(&enc.bytes).unwrap();
    a.unlock("pw").unwrap();
    for (name, data) in &expected {
        assert_eq!(&a.read_file(name).unwrap(), data, "roundtrip of {name}");
    }
    // 空文件以明文空 single-unit 的形式往返。
    assert_eq!(a.read_file("empty.txt").unwrap(), b"");
}

#[test]
fn unprotect_of_empty_protected_archive() {
    let empty = mpq::create(&mpq::CreateOptions {
        version: FormatVersion::V4,
        ..Default::default()
    })
    .unwrap();
    let enc = protect(&empty, "pw", &weak()).unwrap().bytes;
    assert!(open(&enc).unwrap().is_password_protected());

    let plain = unprotect(&enc, "pw").unwrap();
    let a = open(&plain.bytes).unwrap();
    assert!(!a.is_password_protected());
    assert!(a.entries().is_empty());
}

#[test]
fn relocked_reload_keeps_master_key_valid() {
    // GUI 重新加载流程：先前解锁得到的主密钥对同一个（未变更的）
    // vault 仍然校验通过。
    let enc = protected();
    let mut a = open(&enc).unwrap();
    a.unlock("hunter2").unwrap();
    let master = a.master_key().unwrap();

    let mut b = open(&enc).unwrap();
    assert!(b.unlock_with_master_key(&master).is_ok());
    assert!(b.read_file("readme.txt").is_ok());

    let mut wrong = master;
    wrong[0] ^= 1;
    let mut c = open(&enc).unwrap();
    assert!(c.unlock_with_master_key(&wrong).is_err());
    assert!(!c.is_unlocked());
}
