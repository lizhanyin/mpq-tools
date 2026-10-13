//! mpqtool — inspect, extract and repair MPQ archives (CLI).
//! The graphical interface lives in the separate `mpqtool-gui` crate.

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{CommandFactory, Parser, Subcommand};
use mpq::{Archive, Error, FormatVersion};
use mpqtool::tasks;

#[derive(Parser)]
#[command(
    name = "mpqtool",
    version,
    about = "Inspect, extract, modify and repair Blizzard MPQ archives (v1–v4, v3 protocol focus)",
    long_about = "Reads MPQ archives following the v3 protocol (also opens v1/v2/v4 \
archives through their classic tables), imports and deletes files, and repairs \
damaged headers. Non-essential data such as (listfile), \
(attributes) or HET/BET tables may be missing — the archive still opens. \
The graphical interface is the separate mpqtool-gui binary."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Show archive header and table summary.
    Info {
        /// Archive file.
        archive: PathBuf,
    },
    /// List files inside the archive.
    List {
        /// Archive file.
        archive: PathBuf,
        /// Show extra detail (flags, locale, table source).
        #[arg(short, long)]
        verbose: bool,
    },
    /// Extract files to a directory (default: all known files).
    Extract {
        /// Archive file.
        archive: PathBuf,
        /// Files to extract (default: all named files).
        files: Vec<String>,
        /// Output directory (default: archive name without extension).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Extract every table entry, including unnamed ones
        /// (unnamed encrypted files cannot be decrypted).
        #[arg(long)]
        all_entries: bool,
    },
    /// Write one file to stdout.
    Cat {
        /// Archive file.
        archive: PathBuf,
        /// File name inside the archive.
        name: String,
    },
    /// Import files into the archive (stored under their file name;
    /// an existing file of the same name is replaced).
    Add {
        /// Archive file.
        archive: PathBuf,
        /// Files to import.
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Output file (default: modify the input in place).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Store re-encoded key-v2 files unencrypted instead of
        /// re-encrypting them at their new offset (default).
        #[arg(long)]
        no_reencrypt_key2: bool,
    },
    /// Delete files from the archive by name.
    Delete {
        /// Archive file.
        archive: PathBuf,
        /// Names of files to delete (as shown by `mpqtool list`).
        #[arg(required = true)]
        names: Vec<String>,
        /// Output file (default: modify the input in place).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Store re-encoded key-v2 files unencrypted instead of
        /// re-encrypting them at their new offset (default).
        #[arg(long)]
        no_reencrypt_key2: bool,
    },
    /// Diagnose and repair damaged header metadata.
    Repair {
        /// Archive file.
        archive: PathBuf,
        /// Only report issues, do not write anything.
        #[arg(long)]
        dry_run: bool,
        /// Modify the input file instead of writing a new one.
        #[arg(long)]
        in_place: bool,
        /// Output file (default: <name>.fixed.<ext>).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Disable the deep scan that relocates misplaced tables.
        #[arg(long)]
        no_deep: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        // No subcommand: summarize usage (the GUI is the mpqtool-gui binary).
        None => {
            Cli::command().print_help().unwrap_or(());
            0
        }
        Some(Command::Info { archive }) => cmd_info(&archive),
        Some(Command::List { archive, verbose }) => cmd_list(&archive, verbose),
        Some(Command::Extract {
            archive,
            files,
            output,
            all_entries,
        }) => cmd_extract(&archive, &files, output, all_entries),
        Some(Command::Cat { archive, name }) => cmd_cat(&archive, &name),
        Some(Command::Add {
            archive,
            files,
            output,
            no_reencrypt_key2,
        }) => cmd_add(&archive, &files, output, !no_reencrypt_key2),
        Some(Command::Delete {
            archive,
            names,
            output,
            no_reencrypt_key2,
        }) => cmd_delete(&archive, &names, output, !no_reencrypt_key2),
        Some(Command::Repair {
            archive,
            dry_run,
            in_place,
            output,
            no_deep,
        }) => cmd_repair(&archive, dry_run, in_place, output, !no_deep),
    };
    std::process::exit(code);
}

fn open_archive(path: &Path) -> mpq::Result<Archive<std::fs::File>> {
    Archive::open(path)
}

fn print_warnings(archive: &Archive<std::fs::File>) {
    for w in archive.warnings() {
        eprintln!("warning: {w}");
    }
}

fn cmd_info(path: &Path) -> i32 {
    let archive = match open_archive(path) {
        Ok(a) => a,
        Err(e) => return fail(e),
    };
    let h = archive.header();

    println!("Archive:        {}", path.display());
    println!(
        "Format:         MPQ v{} ({})",
        archive.version().as_u32(),
        match archive.version() {
            FormatVersion::V1 => "classic",
            FormatVersion::V2 => "Burning Crusade",
            FormatVersion::V3 => "Cataclysm, HET/BET capable",
            FormatVersion::V4 => "v4 header, v3 tables",
        }
    );
    println!("Archive offset: {:#x}", archive.archive_offset());
    println!("Sector size:    {}", archive.sector_size());
    println!(
        "Tables:         hash={} block={}{}{}",
        h.hash_table_size,
        h.block_table_size,
        if archive.uses_het_bet() { " + HET + BET" } else { "" },
        if h.hi_block_table_pos != 0 { " + hi-block" } else { "" },
    );
    println!("Archive size:   {:#x}", h.archive_size);
    if archive.version() >= FormatVersion::V3 {
        println!("Archive size64: {:#x}", h.archive_size64);
    }
    println!("Listfile:       {} name(s)", archive.file_names().len());
    println!(
        "Attributes:     {}",
        match archive.attributes() {
            Some(a) => format!(
                "v{} (crc32: {}, filetime: {}, md5: {})",
                a.version,
                !a.crc32.is_empty(),
                !a.file_times.is_empty(),
                !a.md5s.is_empty()
            ),
            None => "absent".to_string(),
        }
    );
    print_warnings(&archive);
    0
}

fn cmd_list(path: &Path, verbose: bool) -> i32 {
    let archive = match open_archive(path) {
        Ok(a) => a,
        Err(e) => return fail(e),
    };
    print_warnings(&archive);
    let entries = archive.entries();

    if verbose {
        println!(
            "{:<6} {:>10} {:>10} {:>5} {:<4} NAME",
            "FLAGS", "SIZE", "CSIZE", "LOC", "SRC"
        );
        for e in &entries {
            println!(
                "{:<6} {:>10} {:>10} {:>5} {:<4} {}",
                tasks::flag_summary(e.flags),
                e.size,
                e.compressed_size,
                e.locale,
                match e.source {
                    mpq::TableSource::Classic => "blk",
                    mpq::TableSource::Bet => "bet",
                },
                e.display_name()
            );
        }
    } else {
        for e in &entries {
            println!("{}", e.display_name());
        }
    }
    eprintln!("{} file(s)", entries.len());
    0
}

fn cmd_extract(
    path: &Path,
    files: &[String],
    output: Option<PathBuf>,
    all_entries: bool,
) -> i32 {
    let archive = match open_archive(path) {
        Ok(a) => a,
        Err(e) => return fail(e),
    };
    print_warnings(&archive);

    let out_dir = output.unwrap_or_else(|| {
        let mut p = PathBuf::from(path);
        p.set_extension("");
        p
    });

    let entries: Vec<_> = if !files.is_empty() {
        let mut picked = Vec::new();
        for name in files {
            match archive.find_file(name) {
                Some(e) => picked.push(e),
                None => {
                    eprintln!("error: file not found: {name}");
                    return 1;
                }
            }
        }
        picked
    } else {
        archive
            .entries()
            .into_iter()
            .filter(|e| all_entries || e.name.is_some())
            .collect()
    };

    let result = tasks::extract_entries(&archive, &entries, &out_dir);
    for msg in &result.messages {
        println!("{msg}");
    }
    eprintln!("{} extracted, {} failed", result.ok, result.failed);
    if result.failed == 0 {
        0
    } else {
        1
    }
}

fn cmd_cat(path: &Path, name: &str) -> i32 {
    let archive = match open_archive(path) {
        Ok(a) => a,
        Err(e) => return fail(e),
    };
    let data = match archive.read_file(name) {
        Ok(d) => d,
        Err(e) => return fail(e),
    };
    let mut stdout = std::io::stdout().lock();
    if stdout.write_all(&data).is_err() {
        return 1;
    }
    0
}

fn cmd_repair(
    path: &Path,
    dry_run: bool,
    in_place: bool,
    output: Option<PathBuf>,
    deep: bool,
) -> i32 {
    let mut bytes = match tasks::read_file(path) {
        Ok(b) => b,
        Err(e) => return fail(Error::Io(e)),
    };

    if dry_run {
        match mpq::diagnose(&bytes) {
            Ok(report) => {
                print_issues(&report);
                if report.issues.is_empty() {
                    println!("未发现问题。");
                }
                0
            }
            Err(e) => fail(e),
        }
    } else {
        let report = match mpq::repair(&mut bytes, deep) {
            Ok(r) => r,
            Err(e) => return fail(e),
        };
        print_issues(&report);
        if report.issues.is_empty() {
            println!("未发现问题，无需修复。");
            return 0;
        }
        if report.changed {
            // Verify the repaired bytes actually open.
            match Archive::new(std::io::Cursor::new(&bytes)) {
                Ok(a) => {
                    for w in a.warnings() {
                        eprintln!("note: {w}");
                    }
                }
                Err(e) => eprintln!("warning: 修复后仍无法打开: {e}"),
            }
        }

        let out_path = if in_place {
            path.to_path_buf()
        } else {
            output.unwrap_or_else(|| tasks::repaired_path(path))
        };
        match std::fs::write(&out_path, &bytes) {
            Ok(()) => {
                println!("已写入 {}", out_path.display());
                0
            }
            Err(e) => {
                eprintln!("error: 无法写入 {}: {e}", out_path.display());
                1
            }
        }
    }
}

fn print_issues(report: &mpq::RepairReport) {
    for issue in &report.issues {
        match &issue.action {
            Some(action) => println!("[可修复] {}: {} → {}", issue.field, issue.problem, action),
            None => println!("[无法修复] {}: {}", issue.field, issue.problem),
        }
    }
}

// -- add / delete --------------------------------------------------------------

/// Shared driver for the add / delete subcommands: read the archive, apply
/// the modification (the library self-verifies the result), print the
/// report and write the output.
fn run_modify(
    path: &Path,
    deletions: Vec<mpq::Deletion>,
    additions: Vec<(String, Vec<u8>)>,
    output: Option<PathBuf>,
    reencrypt_key2: bool,
) -> i32 {
    // Reject deletions of names that do not exist (like `zip -d`).
    for del in &deletions {
        if let mpq::Deletion::ByName(name) = del {
            match open_archive(path) {
                Ok(a) => {
                    if !a.has_file(name) {
                        eprintln!("error: file not found: {name}");
                        return 1;
                    }
                }
                Err(e) => return fail(e),
            }
        }
    }

    let bytes = match tasks::read_file(path) {
        Ok(b) => b,
        Err(e) => return fail(Error::Io(e)),
    };
    let options = mpq::ModifyOptions {
        reencrypt_key_v2: reencrypt_key2,
    };
    let outcome = match mpq::modify_with(&bytes, &deletions, &additions, &options) {
        Ok(o) => o,
        Err(e) => return fail(e),
    };
    print_modify_report(&outcome.report, reencrypt_key2);

    let out_path = output.unwrap_or_else(|| path.to_path_buf());
    match std::fs::write(&out_path, &outcome.bytes) {
        Ok(()) => {
            println!(
                "已写入 {}（{} 字节）",
                out_path.display(),
                outcome.bytes.len()
            );
            0
        }
        Err(e) => {
            eprintln!("error: 无法写入 {}: {e}", out_path.display());
            1
        }
    }
}

fn print_modify_report(report: &mpq::ModifyReport, reencrypted: bool) {
    if !report.added.is_empty() {
        println!("新增 {}：{}", report.added.len(), report.added.join("、"));
    }
    if !report.replaced.is_empty() {
        println!(
            "替换 {}：{}",
            report.replaced.len(),
            report.replaced.join("、")
        );
    }
    if !report.deleted.is_empty() {
        println!(
            "删除 {}：{}",
            report.deleted.len(),
            report.deleted.join("、")
        );
    }
    if !report.dropped.is_empty() {
        println!(
            "自动移除（修改后失效）：{}",
            report.dropped.join("、")
        );
    }
    if !report.re_encoded.is_empty() {
        println!(
            "重新编码（key-v2，{}）：{}",
            if reencrypted {
                "按新偏移重新加密"
            } else {
                "存为未加密"
            },
            report.re_encoded.join("、")
        );
    }
}

fn cmd_add(
    path: &Path,
    files: &[PathBuf],
    output: Option<PathBuf>,
    reencrypt_key2: bool,
) -> i32 {
    let mut additions: Vec<(String, Vec<u8>)> = Vec::new();
    for file in files {
        let name = match file.file_name() {
            Some(n) => n.to_string_lossy().to_string(),
            None => {
                eprintln!("error: 无法从路径取得文件名 {}", file.display());
                return 1;
            }
        };
        match tasks::read_file(file) {
            Ok(data) => {
                println!("导入 {}（{} 字节）", file.display(), data.len());
                additions.push((name, data));
            }
            Err(e) => {
                eprintln!("error: 读取失败 {}: {e}", file.display());
                return 1;
            }
        }
    }
    run_modify(path, Vec::new(), additions, output, reencrypt_key2)
}

fn cmd_delete(
    path: &Path,
    names: &[String],
    output: Option<PathBuf>,
    reencrypt_key2: bool,
) -> i32 {
    let deletions: Vec<mpq::Deletion> = names
        .iter()
        .map(|n| mpq::Deletion::ByName(n.clone()))
        .collect();
    run_modify(path, deletions, Vec::new(), output, reencrypt_key2)
}

fn fail(e: Error) -> i32 {
    eprintln!("error: {e}");
    1
}
