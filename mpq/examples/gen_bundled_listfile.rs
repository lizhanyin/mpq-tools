//! 生成嵌入 crate 的合并 listfile blob（开发工具，不参与日常构建）。
//! 用法:
//! cargo run -p mpq --example gen-bundled-listfile -- <输出.zlib> <输入.txt>...
//!
//! 合并多个 listfile 文本（如 MPQMaster 的 Listfiles 目录），按行去重、
//! 排序（保证可重现），以 zlib 最佳压缩写出。

use std::collections::HashSet;
use std::io::Write;

use flate2::write::ZlibEncoder;
use flate2::Compression;

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().expect("usage: gen-bundled-listfile <out.zlib> <in.txt>...");
    let inputs: Vec<String> = args.collect();
    assert!(!inputs.is_empty(), "no inputs");

    let mut seen = HashSet::new();
    let mut names: Vec<String> = Vec::new();
    for path in &inputs {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {path}: {e}"));
        for line in text.lines() {
            let name = line.trim();
            if name.is_empty() || !seen.insert(name.to_ascii_lowercase()) {
                continue;
            }
            names.push(name.to_string());
        }
    }
    names.sort_unstable();

    let text = names.join("\r\n");
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
    enc.write_all(text.as_bytes()).expect("compress");
    let blob = enc.finish().expect("finish");
    std::fs::write(&out, blob).unwrap_or_else(|e| panic!("write {out}: {e}"));
    eprintln!(
        "{} names from {} inputs -> {} bytes (zlib), text {} bytes",
        names.len(),
        inputs.len(),
        std::fs::metadata(&out).unwrap().len(),
        text.len()
    );
}
