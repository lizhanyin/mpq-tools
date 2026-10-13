//! Performance bench for archive modification. Not run by default:
//!
//! ```console
//! cargo test --release -p mpq --test bench_modify -- --ignored --nocapture
//! ```

mod common;

use common::{ArchiveSpec, FileSpec};
use std::time::Instant;

/// ~1 MB of semi-repetitive text (compresses like real script/data files).
fn text_mb(seed: u32, mb: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(mb * 1024 * 1024);
    let mut i = seed;
    while out.len() < mb * 1024 * 1024 {
        let line = format!(
            "record {i:08} value {:010} padding padding padding tail\n",
            (i.wrapping_mul(2_654_435_761) as u64) % 10_000_000_000u64
        );
        out.extend_from_slice(line.as_bytes());
        i = i.wrapping_add(1);
    }
    out
}

#[test]
#[ignore]
fn modify_60mb_archive_replace_and_add_20mb() {
    const N_KEEP: u32 = 50;
    const N_REPLACE: u32 = 10;
    const N_ADD: u32 = 10;

    let mut files = Vec::new();
    for i in 0..N_KEEP + N_REPLACE {
        files.push(
            FileSpec::bytes(&format!("data\\file{i:02}.bin"), text_mb(i, 1)).compress(),
        );
    }
    let spec = ArchiveSpec {
        version: 0,
        sector_shift: 3,
        with_listfile: true,
        with_attributes: false,
        files,
        ..Default::default()
    };

    let t0 = Instant::now();
    let src = common::build(&spec);
    println!(
        "build source: {} MB of content -> {} MB on disk in {:?}",
        N_KEEP + N_REPLACE,
        src.len() / 1024 / 1024,
        t0.elapsed()
    );

    // Replace the last 10 files and add 10 new ones — 20 MB of fresh
    // content against a 60 MB archive.
    let mut additions = Vec::new();
    for i in 0..N_ADD {
        additions.push((format!("new\\add{i:02}.bin"), text_mb(1000 + i, 1)));
    }
    for i in 0..N_REPLACE {
        additions.push((
            format!("data\\file{:02}.bin", N_KEEP + i),
            text_mb(2000 + i, 1),
        ));
    }

    let t1 = Instant::now();
    let out = mpq::modify(&src, &[], &additions).expect("modify");
    println!(
        "modify ({} add + {} replace = 20 MB new content): {} MB -> {} MB in {:?}",
        N_ADD,
        N_REPLACE,
        src.len() / 1024 / 1024,
        out.bytes.len() / 1024 / 1024,
        t1.elapsed()
    );

    // Reference point: how long just compressing the 20 MB would take.
    let t2 = Instant::now();
    for (_, data) in &additions {
        let z = common::zlib(data);
        std::hint::black_box(z);
    }
    println!("reference: zlib the same 20 MB alone in {:?}", t2.elapsed());
}
