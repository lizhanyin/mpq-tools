//! mpqtool — inspect and extract MPQ archives.

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use mpq::{Archive, Error, FormatVersion};

#[derive(Parser)]
#[command(
    name = "mpqtool",
    version,
    about = "Inspect and extract Blizzard MPQ archives (v1–v4, v3 protocol focus)",
    long_about = "Reads MPQ archives following the v3 protocol (also opens v1/v2/v4 \
archives through their classic tables). Non-essential data such as (listfile), \
(attributes) or HET/BET tables may be missing — the archive still opens."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
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
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Info { archive } => cmd_info(&archive),
        Command::List { archive, verbose } => cmd_list(&archive, verbose),
        Command::Extract {
            archive,
            files,
            output,
            all_entries,
        } => cmd_extract(&archive, &files, output, all_entries),
        Command::Cat { archive, name } => cmd_cat(&archive, &name),
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
                flag_summary(e.flags),
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

fn flag_summary(flags: u32) -> String {
    let mut s = String::new();
    if flags & mpq::FILE_EXISTS != 0 {
        s.push('E');
    }
    if flags & mpq::FILE_ENCRYPTED != 0 {
        s.push('K');
    }
    if flags & (mpq::FILE_COMPRESS | mpq::FILE_IMPLODE) != 0 {
        s.push('C');
    }
    if flags & mpq::FILE_SINGLE_UNIT != 0 {
        s.push('1');
    }
    if flags & mpq::FILE_SECTOR_CRC != 0 {
        s.push('#');
    }
    if flags & mpq::FILE_PATCH != 0 {
        s.push('P');
    }
    s
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

    // Selection: explicit names, or every named entry, or every entry.
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

    if entries.is_empty() {
        eprintln!(
            "note: nothing to extract (no file names known; try --all-entries)"
        );
        return 0;
    }

    if std::fs::create_dir_all(&out_dir).is_err() {
        eprintln!("error: cannot create output directory {}", out_dir.display());
        return 1;
    }

    let mut ok = 0usize;
    let mut failed = 0usize;
    for entry in &entries {
        let display = entry.display_name();
        let data = match archive.read_entry(entry) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("error: {display}: {e}");
                failed += 1;
                continue;
            }
        };
        let out_path = safe_join(&out_dir, &display);
        if let Some(parent) = out_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&out_path, &data) {
            Ok(()) => {
                println!("{} ({})", out_path.display(), data.len());
                ok += 1;
            }
            Err(e) => {
                eprintln!("error: {}: {e}", out_path.display());
                failed += 1;
            }
        }
    }
    eprintln!("{ok} extracted, {failed} failed");
    if failed == 0 {
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

fn fail(e: Error) -> i32 {
    eprintln!("error: {e}");
    1
}

/// Join an in-archive name onto the output dir, refusing path escapes.
fn safe_join(base: &Path, name: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    let cleaned = name.replace('/', "\\");
    for part in cleaned.split('\\') {
        if part.is_empty() || part == "." || part == ".." {
            continue; // never escape the output directory
        }
        path.push(part);
    }
    path
}
