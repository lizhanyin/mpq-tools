//! 反向对拍：用 Rust mpq 库读取 Java 移植版（nineyin-mpq）写出的归档，
//! 逐文件比对 sha256。清单格式与 gen_java_fixtures 一致：名字<TAB>sha256<TAB>大小。
//!
//! 用法：cargo run --example verify_java_archives -- <目录（含 java_v1..4.mpq 与 java_manifest.txt）>

use std::path::PathBuf;

use mpq::Archive;
use sha2::{Digest as _, Sha256};

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("用法: verify_java_archives <目录>"));
    let manifest_text = std::fs::read_to_string(dir.join("java_manifest.txt"))
        .expect("缺少 java_manifest.txt");
    let manifest: Vec<(String, String, usize)> = manifest_text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let parts: Vec<&str> = l.trim().split('\t').collect();
            (parts[0].to_string(), parts[1].to_string(), parts[2].parse().unwrap())
        })
        .collect();

    let mut failures = 0usize;
    for name in ["java_v1.mpq", "java_v2.mpq", "java_v3.mpq", "java_v4.mpq"] {
        let path = dir.join(name);
        if !path.exists() {
            eprintln!("跳过缺失的 {name}");
            continue;
        }
        let data = std::fs::read(&path).unwrap();
        match Archive::new(std::io::Cursor::new(&data)) {
            Ok(archive) => {
                let version = archive.version();
                let mut ok = 0;
                for (file, want_sha, want_len) in &manifest {
                    match archive.read_file(file) {
                        Ok(bytes) => {
                            let got = Sha256::digest(&bytes);
                            let got_hex: String = got.iter().map(|b| format!("{b:02x}")).collect();
                            if got_hex == *want_sha && bytes.len() == *want_len {
                                ok += 1;
                            } else {
                                failures += 1;
                                eprintln!("{name}:{file} 哈希或大小不一致");
                            }
                        }
                        Err(e) => {
                            failures += 1;
                            eprintln!("{name}:{file} 读取失败：{e}");
                        }
                    }
                }
                let entries = archive.entries().len();
                println!("{name}: v{:?}, {ok}/{} 文件一致, 条目 {entries}", version, manifest.len());
                for w in archive.warnings() {
                    println!("  警告: {w}");
                }
            }
            Err(e) => {
                failures += 1;
                eprintln!("{name}: 打开失败：{e}");
            }
        }
    }

    if failures == 0 {
        println!("全部通过：Rust 端可完整读取 Java 写出的归档");
    } else {
        eprintln!("失败 {failures} 项");
        std::process::exit(1);
    }
}
