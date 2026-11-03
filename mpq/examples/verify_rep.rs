//! 全量读取归档并计算与 Java VerifyAll 相同顺序的聚合 sha256。
//! 用法: cargo run --release --example verify_rep -- <归档路径>

use mpq::Archive;
use sha2::{Digest as _, Sha256};

fn main() {
    let path = std::env::args().nth(1).expect("用法: verify_rep <归档>");
    let start = std::time::Instant::now();
    let mut archive = Archive::new(std::fs::File::open(&path).unwrap()).unwrap();
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut failures = 0u64;
    let count = archive.entries().len();
    for e in archive.entries() {
        if !e.exists() { continue; }
        match archive.read_entry(&e) {
            Ok(data) => { total += data.len() as u64; digest.update(&data); }
            Err(err) => { failures += 1; if failures <= 10 { eprintln!("失败 {}: {err}", e.display_name()); } }
        }
    }
    println!("条目数: {}", count);
    println!("失败: {failures}");
    println!("读取告警: {}", archive.read_warnings().len());
    println!("打开告警: {}", archive.warnings().len());
    println!("内容聚合 sha256: {:064x}", digest.finalize());
    println!("解压总量: {:.1} MB, 耗时 {:.1}s", total as f64 / 1e6, start.elapsed().as_secs_f64());
}
