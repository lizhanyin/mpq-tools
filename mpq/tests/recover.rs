//! 缺失 `(listfile)` 时的文件名恢复：内置游戏清单试探、
//! `war3map.imp` 导入清单解析，以及外部清单 API
//!（[`Archive::recover_names_with`]）。

mod common;

use common::{build, build_hetbet, ArchiveSpec, FileSpec};
use mpq::Archive;
use std::io::Cursor;

fn open(bytes: &[u8]) -> Archive<Cursor<&[u8]>> {
    Archive::new(Cursor::new(bytes)).expect("archive should open")
}

fn imp_file(entries: &[(&str, &[u8])]) -> FileSpec {
    let mut data = Vec::new();
    for (path, _content) in entries {
        data.push(0x0D); // 自定义导入标记
        data.extend_from_slice(path.as_bytes());
        data.push(0);
    }
    FileSpec {
        name: "war3map.imp".to_string(),
        data,
        ..FileSpec::text("", "")
    }
}

#[test]
fn recovers_known_names_without_listfile() {
    let bytes = build(&ArchiveSpec {
        files: vec![
            FileSpec::text("war3map.j", "// dummy map script\n"),
            FileSpec::text("war3map.w3e", "terrain"),
            FileSpec::text("Units\\AbilityData.slk", "slk data"),
            FileSpec::text("自定义\\无法猜到.txt", "mystery"),
        ],
        ..ArchiveSpec::default() // with_listfile = false
    });
    let archive = open(&bytes);

    // 恢复数应恰好等于三个可猜中的名字；第四个文件无哈希线索。
    assert!(archive
        .warnings()
        .iter()
        .any(|w| w.contains("recovered 3 file name(s)")));

    let entries = archive.entries();
    let name_of = |name: &str| {
        entries
            .iter()
            .find(|e| e.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("name not recovered: {name}"))
    };
    name_of("war3map.j");
    name_of("war3map.w3e");
    name_of("Units\\AbilityData.slk");
    assert!(entries.iter().any(|e| e.name.is_none()));

    // 恢复出的名字可用于正常读取。
    assert_eq!(
        archive.read_file("Units\\AbilityData.slk").unwrap(),
        b"slk data".to_vec()
    );
    assert!(archive.has_file("war3map.j"));
}

#[test]
fn imp_recovers_imported_paths() {
    let icon = FileSpec::text("war3mapImported\\icon.blp", "blp");
    let model = FileSpec::text("war3mapImported\\hero.mdx", "mdx");
    let imp = imp_file(&[
        ("war3mapImported\\icon.blp", b"blp"),
        ("war3mapImported\\hero.mdx", b"mdx"),
    ]);
    let bytes = build(&ArchiveSpec {
        files: vec![icon, model, imp],
        ..ArchiveSpec::default()
    });
    let archive = open(&bytes);

    let entries = archive.entries();
    assert!(entries
        .iter()
        .any(|e| e.name.as_deref() == Some("war3mapImported\\icon.blp")));
    assert!(entries
        .iter()
        .any(|e| e.name.as_deref() == Some("war3mapImported\\hero.mdx")));
    assert_eq!(
        archive
            .read_file("war3mapImported\\icon.blp")
            .unwrap(),
        b"blp".to_vec()
    );
}

#[test]
fn script_references_recover_imported_paths() {
    // 脚本以 JASS 转义（\\）与正斜杠两种形式引用导入资源；
    // 对象数据以二进制内嵌路径引用之。
    let script = concat!(
        "call AddSpecialEffect(\"war3mapImported\\\\fx.mdx\", 0., 0.)\n",
        "call Preload(\"Sound\\\\Custom\\\\bgm.mp3\")\n",
        "call PlayMusic(\"war3mapImported/intro.mid\")\n",
    );
    let mut w3u: Vec<u8> = vec![0, 1, 2, 3];
    w3u.extend_from_slice(b"war3mapImported\\heroModel.mdx\0");
    w3u.extend_from_slice(b"ReplaceableTextures\\Custom\\icon.blp\0");
    let w3u_spec = FileSpec {
        name: "war3map.w3u".to_string(),
        data: w3u,
        ..FileSpec::text("", "")
    };
    let bytes = build(&ArchiveSpec {
        files: vec![
            FileSpec::text("war3map.j", script),
            w3u_spec,
            FileSpec::text("war3mapImported\\fx.mdx", "fx"),
            FileSpec::text("Sound\\Custom\\bgm.mp3", "mp3"),
            FileSpec::text("war3mapImported\\intro.mid", "mid"),
            FileSpec::text("war3mapImported\\heroModel.mdx", "model"),
            FileSpec::text("ReplaceableTextures\\Custom\\icon.blp", "blp"),
            FileSpec::text("war3mapImported\\未引用.ttf", "orphan"),
        ],
        ..ArchiveSpec::default()
    });
    let archive = open(&bytes);

    for name in [
        "war3mapImported\\fx.mdx",
        "Sound\\Custom\\bgm.mp3",
        "war3mapImported\\intro.mid",
        "war3mapImported\\heroModel.mdx",
        "ReplaceableTextures\\Custom\\icon.blp",
    ] {
        assert!(
            archive.entries()
                .iter()
                .any(|e| e.name.as_deref() == Some(name)),
            "script-referenced name not recovered: {name}"
        );
    }
    // 未被引用的文件仍不可知。
    assert!(archive.entries().iter().any(|e| e.name.is_none()));
}

#[test]
fn external_listfile_recovers_custom_names() {
    let bytes = build(&ArchiveSpec {
        files: vec![FileSpec::text("custom\\secret.txt", "s")],
        ..ArchiveSpec::default()
    });
    let mut archive = open(&bytes);
    assert!(archive.entries().iter().all(|e| e.name.is_none()));

    assert_eq!(archive.recover_names_with(["no\\such.file"]), 0);
    assert_eq!(archive.recover_names_with(["custom\\secret.txt"]), 1);
    // 大小写变体不得重复登记。
    assert_eq!(archive.recover_names_with(["CUSTOM\\SECRET.TXT"]), 0);

    let entries = archive.entries();
    assert_eq!(
        entries[0].name.as_deref(),
        Some("custom\\secret.txt")
    );
}

#[test]
fn het_bet_archive_recovers_names() {
    let bytes = build_hetbet(&[
        ("war3map.j".to_string(), b"script".to_vec()),
        ("谜之文件.bin".to_string(), b"??".to_vec()),
    ]);
    let archive = open(&bytes);
    assert!(archive.uses_het_bet());

    let entries = archive.entries();
    assert!(entries
        .iter()
        .any(|e| e.name.as_deref() == Some("war3map.j")));
    assert!(entries.iter().any(|e| e.name.is_none()));
}

/// 构造一个最小 MDX：VERS + TEXS（引用给定贴图路径）。
/// TEXS 记录定长 268 字节：id(4) + path[256] + flags(4) + 保留(4)。
fn mini_mdx(texture_paths: &[&str]) -> Vec<u8> {
    let mut mdx = Vec::new();
    mdx.extend_from_slice(b"MDLXVERS");
    mdx.extend_from_slice(&4u32.to_le_bytes());
    mdx.extend_from_slice(&4u32.to_le_bytes());
    let mut texs = Vec::new();
    for p in texture_paths {
        let mut rec = vec![0u8; 268];
        rec[4..4 + p.len()].copy_from_slice(p.as_bytes());
        texs.extend_from_slice(&rec);
    }
    mdx.extend_from_slice(b"TEXS");
    mdx.extend_from_slice(&(texs.len() as u32).to_le_bytes());
    mdx.extend_from_slice(&texs);
    mdx
}

#[test]
fn mdx_texs_and_same_name_textures_recover() {
    // 三条恢复链：
    // 1) 脚本引用模型 → 模型名恢复；具名模型 TEXS → 被引用贴图恢复；
    // 2) 同名规律（模型名 + 贴图扩展名）→ 同名贴图恢复；
    // 3) 未命名模型经内容类型识别为 .mdx 后，其 TEXS 同样被扫描
    //    （scan_unnamed_model_textures）→ 引用的贴图恢复。
    let script = "call AddSpecialEffect(\"war3mapImported\\\\有名模型.mdx\", 0., 0.)\n";
    let mut named_model = FileSpec::text("", "");
    named_model.name = "war3mapImported\\有名模型.mdx".to_string();
    named_model.data = mini_mdx(&["war3mapImported\\tex0.blp"]);

    let mut orphan_model = FileSpec::text("", "");
    orphan_model.name = "war3mapImported\\无名模型.mdx".to_string(); // 无任何引用
    orphan_model.data = mini_mdx(&["war3mapImported\\hidden.blp"]);

    let bytes = build(&ArchiveSpec {
        files: vec![
            FileSpec::text("war3map.j", script),
            named_model,
            orphan_model,
            // 由具名模型的 TEXS 恢复。
            FileSpec::text("war3mapImported\\tex0.blp", "blp"),
            // 由未命名模型的 TEXS 恢复（类型识别为 .mdx 后扫描）。
            FileSpec::text("war3mapImported\\hidden.blp", "blp"),
            // 由同名启发式恢复（模型名 + 贴图扩展名逐一试探）。
            FileSpec::text("war3mapImported\\有名模型.blp", "blp"),
            FileSpec::text("war3mapImported\\有名模型.tga", "tga"),
        ],
        ..ArchiveSpec::default()
    });
    let archive = open(&bytes);

    let entries = archive.entries();
    let has_name = |n: &str| entries.iter().any(|e| e.name.as_deref() == Some(n));
    assert!(has_name("war3mapImported\\tex0.blp"), "TEXS 引用未恢复");
    assert!(
        has_name("war3mapImported\\hidden.blp"),
        "未命名模型的 TEXS 引用未恢复"
    );
    assert!(has_name("war3mapImported\\有名模型.blp"), "同名贴图未恢复");
    assert!(has_name("war3mapImported\\有名模型.tga"), "同名 .tga 未恢复");
}

#[test]
fn model_texture_chain_reaches_fixed_point() {
    // 三级链（全部从零引用起步）：
    // 1) 未命名模型 A（类型识别为 .mdx）的 TEXS 引用贴图 t1.blp
    //    → 贴图恢复；
    // 2) t1.blp → 同名反推（根目录）模型 t1.mdx（另一未命名模型 B）
    //    → 模型 B 恢复；
    // 3) 模型 B 的 TEXS 引用 t2.blp → 贴图恢复。
    // 迭代收敛：任何一步新恢复的名字都会喂给下一轮。
    let mut unnamed_a = FileSpec::text("", "");
    unnamed_a.name = "谜之甲.mdx".to_string(); // 无任何引用
    unnamed_a.data = mini_mdx(&["war3mapImported\\t1.blp"]);

    let mut unnamed_b = FileSpec::text("", "");
    unnamed_b.name = "t1.mdx".to_string(); // 只能经 t1.blp 反推得到
    unnamed_b.data = mini_mdx(&["war3mapImported\\t2.blp"]);

    let bytes = build(&ArchiveSpec {
        files: vec![
            unnamed_a,
            unnamed_b,
            FileSpec::text("war3mapImported\\t1.blp", "blp1"),
            FileSpec::text("war3mapImported\\t2.blp", "blp2"),
        ],
        ..ArchiveSpec::default()
    });
    let archive = open(&bytes);

    let entries = archive.entries();
    let has_name = |n: &str| entries.iter().any(|e| e.name.as_deref() == Some(n));
    assert!(has_name("t1.mdx"), "同名反推模型未恢复");
    assert!(has_name("war3mapImported\\t1.blp"), "链上贴图 t1 未恢复");
    assert!(has_name("war3mapImported\\t2.blp"), "链上贴图 t2 未恢复");
}

#[test]
fn unnamed_entries_get_guessed_ext_from_content() {
    // 未被任何清单 / 引用命名的文件，按内容识别出类型后缀。
    let mut mdx = FileSpec::text("", "");
    mdx.name = "war3mapImported\\谜之模型.mdx".to_string();
    mdx.data = b"MDLX\x04\x00\x00\x00rest of model".to_vec();

    let mut wav = FileSpec::text("", "");
    wav.name = "谜之音效.wav".to_string();
    wav.data = b"RIFF\x24\x00\x00\x00WAVEfmt data".to_vec();

    let mut mystery = FileSpec::text("", "");
    mystery.name = "完全无法识别.bin".to_string();
    mystery.data = vec![0xDE, 0xAD, 0xBE, 0xEF, 1, 2, 3];

    let bytes = build(&ArchiveSpec {
        files: vec![mdx, wav, mystery],
        ..ArchiveSpec::default()
    });
    let archive = open(&bytes);

    let entries = archive.entries();
    let by_size = |size: usize| {
        entries
            .iter()
            .find(|e| e.size as usize == size)
            .unwrap_or_else(|| panic!("no entry of size {size}"))
    };
    let mdx_e = by_size(b"MDLX\x04\x00\x00\x00rest of model".len());
    assert_eq!(mdx_e.name, None);
    assert_eq!(mdx_e.guessed_ext, Some("mdx"));
    assert_eq!(mdx_e.display_name(), format!("<unnamed #{}>.mdx", mdx_e.block_index));

    let wav_e = by_size(b"RIFF\x24\x00\x00\x00WAVEfmt data".len());
    assert_eq!(wav_e.guessed_ext, Some("wav"));

    let bin_e = by_size(7);
    assert_eq!(bin_e.guessed_ext, None);
    assert_eq!(bin_e.display_name(), format!("<unnamed #{}>", bin_e.block_index));
}

#[test]
fn complete_listfile_skips_recovery_warning() {
    let bytes = build(&ArchiveSpec {
        with_listfile: true,
        files: vec![FileSpec::text("war3map.j", "// s\n")],
        ..ArchiveSpec::default()
    });
    let archive = open(&bytes);
    // 完整 listfile：没有“缺失 listfile”的警告；嵌入清单里的
    // "(listfile)" 等系统文件名不会被重复登记进 file_names()。
    assert!(archive
        .warnings()
        .iter()
        .all(|w| !w.contains("no (listfile)")));
    assert_eq!(archive.file_names(), ["war3map.j".to_string()]);
    assert!(!archive.has_unnamed_entries());
}
