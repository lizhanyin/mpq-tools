//! mpqtool — 检查、提取和修复 MPQ 归档（CLI）。
//! 图形界面位于独立的 `mpqtool-gui` crate 中。

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Args, CommandFactory, Parser, Subcommand};
use mpq::{Archive, Error, FormatVersion};
use mpqtool::tasks;

/// 供操作（可能）受密码保护归档的子命令共享的 `--password` 参数。
#[derive(Args)]
struct PasswordArg {
    /// 受密码保护归档的密码（自定义 v4 AEAD 扩展；
    /// 见 `mpqtool encrypt`）。
    #[arg(short = 'p', long = "password", value_name = "PASSWORD")]
    password: Option<String>,
}

#[derive(Parser)]
#[command(
    name = "mpqtool",
    version,
    about = "Inspect, extract, modify, convert and repair Blizzard MPQ archives (v1–v4)",
    long_about = "Reads MPQ archives following the v3 protocol, with full v4 support \
(header fields, MD5 checksum verification for tables, header and raw data \
chunks, and v4 write-back). Imports and deletes files, converts between \
container versions, and repairs damaged headers. Password protection \
(encrypt/decrypt/passwd) is a custom v4 extension only this tool understands. \
Non-essential data such as (listfile), (attributes) or HET/BET tables may be \
missing — the archive still opens. The graphical interface is the separate \
mpqtool-gui binary."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// 显示归档头部与各表的摘要。
    Info {
        /// 归档文件。
        archive: PathBuf,
        #[command(flatten)]
        pw: PasswordArg,
    },
    /// 列出归档内的文件。
    List {
        /// 归档文件。
        archive: PathBuf,
        /// 显示额外细节（flags、locale、表来源）。
        #[arg(short, long)]
        verbose: bool,
        /// 外部 listfile 清单（文件或目录，可多次给出），用于恢复
        /// 缺失 `(listfile)` 归档的文件名。未给出时自动发现归档旁 /
        /// 可执行文件旁的 `Listfiles` 目录。
        #[arg(long = "listfile", value_name = "FILE")]
        listfile: Vec<PathBuf>,
        #[command(flatten)]
        pw: PasswordArg,
    },
    /// 将文件提取到目录（默认：所有已知文件）。
    Extract {
        /// 归档文件。
        archive: PathBuf,
        /// 要提取的文件（默认：所有具名文件）。
        files: Vec<String>,
        /// 输出目录（默认：不带扩展名的归档名）。
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// 提取每一个表条目，包括未命名的条目
        ///（未命名的加密文件无法解密）。
        #[arg(long)]
        all_entries: bool,
        /// 外部 listfile 清单（文件或目录，可多次给出），用于恢复
        /// 缺失 `(listfile)` 归档的文件名。未给出时自动发现归档旁 /
        /// 可执行文件旁的 `Listfiles` 目录。
        #[arg(long = "listfile", value_name = "FILE")]
        listfile: Vec<PathBuf>,
        #[command(flatten)]
        pw: PasswordArg,
    },
    /// 将单个文件写到标准输出。
    Cat {
        /// 归档文件。
        archive: PathBuf,
        /// 归档内的文件名。
        name: String,
        #[command(flatten)]
        pw: PasswordArg,
    },
    /// 将文件导入归档（以其文件名存储；
    /// 同名的现有文件会被替换）。
    Add {
        /// 归档文件。
        archive: PathBuf,
        /// 要导入的文件。
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// 输出文件（默认：原地修改输入）。
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// 将重新编码的 key-v2 文件存为未加密，
        /// 而不是（默认行为）在其新偏移处重新加密。
        #[arg(long)]
        no_reencrypt_key2: bool,
        #[command(flatten)]
        pw: PasswordArg,
    },
    /// 按名称从归档中删除文件。
    Delete {
        /// 归档文件。
        archive: PathBuf,
        /// 要删除的文件名（如 `mpqtool list` 所示）。
        #[arg(required = true)]
        names: Vec<String>,
        /// 输出文件（默认：原地修改输入）。
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// 将重新编码的 key-v2 文件存为未加密，
        /// 而不是（默认行为）在其新偏移处重新加密。
        #[arg(long)]
        no_reencrypt_key2: bool,
        #[command(flatten)]
        pw: PasswordArg,
    },
    /// 用密码保护归档（自定义 v4 AEAD 扩展：
    /// 逐 sector 的 ChaCha20-Poly1305，Argon2id 包装的主密钥）。
    /// 只有本工具能打开结果。会将容器升级为 v4。
    Encrypt {
        /// 归档文件。
        archive: PathBuf,
        /// 用于保护的密码。
        #[arg(short = 'p', long = "password", value_name = "PASSWORD")]
        password: String,
        /// 输出文件（默认：原地修改输入）。
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// 移除密码保护（需要密码）。
    Decrypt {
        /// 归档文件。
        archive: PathBuf,
        /// 归档当前的密码。
        #[arg(short = 'p', long = "password", value_name = "PASSWORD")]
        password: String,
        /// 输出文件（默认：原地修改输入）。
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// 更改归档的密码（重新包装主密钥；数据不会被重新加密）。
    Passwd {
        /// 归档文件。
        archive: PathBuf,
        /// 当前密码。
        old_password: String,
        /// 新密码。
        new_password: String,
        /// 输出文件（默认：原地修改输入）。
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// 诊断并修复损坏的头部元数据。
    Repair {
        /// 归档文件。
        archive: PathBuf,
        /// 仅报告问题，不写入任何内容。
        #[arg(long)]
        dry_run: bool,
        /// 修改输入文件而不是写入新文件。
        #[arg(long)]
        in_place: bool,
        /// 输出文件（默认：<name>.fixed.<ext>）。
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// 禁用重新定位错位表的深度扫描。
        #[arg(long)]
        no_deep: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        // 无子命令：显示用法摘要（GUI 是 mpqtool-gui 二进制）。
        None => {
            Cli::command().print_help().unwrap_or(());
            0
        }
        Some(Command::Info { archive, pw }) => cmd_info(&archive, pw.password.as_deref()),
        Some(Command::List {
            archive,
            verbose,
            listfile,
            pw,
        }) => cmd_list(&archive, verbose, listfile, pw.password.as_deref()),
        Some(Command::Extract {
            archive,
            files,
            output,
            all_entries,
            listfile,
            pw,
        }) => cmd_extract(&archive, &files, output, all_entries, listfile, pw.password.as_deref()),
        Some(Command::Cat { archive, name, pw }) => cmd_cat(&archive, &name, pw.password.as_deref()),
        Some(Command::Add {
            archive,
            files,
            output,
            no_reencrypt_key2,
            pw,
        }) => cmd_add(&archive, &files, output, !no_reencrypt_key2, pw.password.as_deref()),
        Some(Command::Delete {
            archive,
            names,
            output,
            no_reencrypt_key2,
            pw,
        }) => cmd_delete(&archive, &names, output, !no_reencrypt_key2, pw.password.as_deref()),
        Some(Command::Encrypt { archive, password, output }) => {
            cmd_encrypt(&archive, &password, output)
        }
        Some(Command::Decrypt { archive, password, output }) => {
            cmd_decrypt(&archive, &password, output)
        }
        Some(Command::Passwd { archive, old_password, new_password, output }) => {
            cmd_passwd(&archive, &old_password, &new_password, output)
        }
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

/// 打开归档，受保护时用 `password` 解锁。`strict` 决定归档受保护但
/// 未提供密码时的行为：读取类命令以明确的错误失败；列举类命令
/// 仅发出警告（没有密码时文件名仍然可见）。
fn open_archive(
    path: &Path,
    password: Option<&str>,
    strict: bool,
) -> mpq::Result<Archive<std::fs::File>> {
    let mut archive = Archive::open(path)?;
    if archive.is_password_protected() {
        match password {
            Some(p) => archive.unlock(p)?,
            None => {
                if strict {
                    eprintln!("error: 归档受密码保护（自定义 v4 加密扩展）；需要 --password 提供密码");
                    std::process::exit(1);
                } else {
                    eprintln!(
                        "note: 归档受密码保护；未提供 --password，文件名可见但内容不可读取"
                    );
                }
            }
        }
    }
    Ok(archive)
}

fn print_warnings(archive: &Archive<std::fs::File>) {
    for w in archive.warnings() {
        eprintln!("warning: {w}");
    }
}

/// 打开归档（见 [`open_archive`]），并应用外部 listfile 清单：
/// 显式给出的路径直接使用；否则当存在未命名条目时，自动发现
/// 归档旁 / 可执行文件旁的 `Listfiles` 目录（MPQMaster 布局）。
fn open_archive_with_listfiles(
    path: &Path,
    password: Option<&str>,
    strict: bool,
    listfile: &[PathBuf],
) -> mpq::Result<Archive<std::fs::File>> {
    let mut archive = open_archive(path, password, strict)?;
    let mut lists: Vec<PathBuf> = listfile.to_vec();
    if lists.is_empty() && archive.has_unnamed_entries() {
        lists = tasks::discover_listfiles(path);
        if !lists.is_empty() {
            eprintln!(
                "note: 自动发现外部清单目录 Listfiles/（{} 个文件）",
                lists.len()
            );
        }
    }
    if !lists.is_empty() {
        let (recovered, errors) = tasks::apply_listfiles(&mut archive, &lists);
        for e in errors {
            eprintln!("warning: 清单不可读: {e}");
        }
        if recovered > 0 {
            eprintln!("note: 外部清单恢复 {recovered} 个文件名");
        }
    }
    Ok(archive)
}

fn cmd_info(path: &Path, password: Option<&str>) -> i32 {
    let archive = match open_archive(path, password, false) {
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
            FormatVersion::V4 => "Cataclysm+, MD5-checksummed header/tables",
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
    if archive.is_password_protected() {
        println!(
            "Password:       protected (custom AEAD extension){}",
            if archive.is_unlocked() {
                " — unlocked"
            } else {
                " — locked, contents need --password"
            }
        );
    }
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

fn cmd_list(path: &Path, verbose: bool, listfile: Vec<PathBuf>, password: Option<&str>) -> i32 {
    let archive = match open_archive_with_listfiles(path, password, false, &listfile) {
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
    listfile: Vec<PathBuf>,
    password: Option<&str>,
) -> i32 {
    let archive = match open_archive_with_listfiles(path, password, true, &listfile) {
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

fn cmd_cat(path: &Path, name: &str, password: Option<&str>) -> i32 {
    let archive = match open_archive(path, password, true) {
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
            // 验证修复后的字节确实能打开。
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

/// add / delete 子命令的共享驱动：读取归档，应用修改（库会自行
/// 验证结果），打印报告并写出输出文件。
fn run_modify(
    path: &Path,
    password: Option<&str>,
    deletions: Vec<mpq::Deletion>,
    additions: Vec<(String, Vec<u8>)>,
    output: Option<PathBuf>,
    reencrypt_key2: bool,
) -> i32 {
    // 受保护的归档做任何修改都需要主密钥；这个探测用的归档
    // 也服务于下面的缺失名称检查。
    let mut master_key = None;
    let probe = match open_archive(path, password, true) {
        Ok(a) => a,
        Err(e) => return fail(e),
    };
    if probe.is_password_protected() {
        master_key = probe.master_key();
    }

    // 拒绝删除不存在的名称（类似 `zip -d`）。
    for del in &deletions {
        if let mpq::Deletion::ByName(name) = del {
            if !probe.has_file(name) {
                eprintln!("error: file not found: {name}");
                return 1;
            }
        }
    }
    drop(probe);

    let bytes = match tasks::read_file(path) {
        Ok(b) => b,
        Err(e) => return fail(Error::Io(e)),
    };
    let options = mpq::ModifyOptions {
        reencrypt_key_v2: reencrypt_key2,
        master_key,
        ..Default::default()
    };
    let outcome = match mpq::modify_with(&bytes, &deletions, &additions, &options) {
        Ok(o) => o,
        Err(e) => return fail(e),
    };
    print_modify_report(&outcome.report, reencrypt_key2);
    print_encryption_notes(&outcome.report);

    write_outcome(path, output, &outcome.bytes)
}

fn print_encryption_notes(report: &mpq::ModifyReport) {
    if report.encrypted > 0 {
        println!("已按密码方案加密 {} 个文件", report.encrypted);
    }
    if report.decrypted > 0 {
        println!("已解密 {} 个文件", report.decrypted);
    }
}

/// 写出修改结果（默认原地写入，与 add/delete 一致）。
fn write_outcome(path: &Path, output: Option<PathBuf>, bytes: &[u8]) -> i32 {
    let out_path = output.unwrap_or_else(|| path.to_path_buf());
    match std::fs::write(&out_path, bytes) {
        Ok(()) => {
            println!("已写入 {}（{} 字节）", out_path.display(), bytes.len());
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
    password: Option<&str>,
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
    run_modify(path, password, Vec::new(), additions, output, reencrypt_key2)
}

fn cmd_delete(
    path: &Path,
    names: &[String],
    output: Option<PathBuf>,
    reencrypt_key2: bool,
    password: Option<&str>,
) -> i32 {
    let deletions: Vec<mpq::Deletion> = names
        .iter()
        .map(|n| mpq::Deletion::ByName(n.clone()))
        .collect();
    run_modify(path, password, deletions, Vec::new(), output, reencrypt_key2)
}

// -- 密码保护 ---------------------------------------------------------

fn cmd_encrypt(path: &Path, password: &str, output: Option<PathBuf>) -> i32 {
    let bytes = match tasks::read_file(path) {
        Ok(b) => b,
        Err(e) => return fail(Error::Io(e)),
    };
    let outcome = match mpq::protect(&bytes, password, &mpq::Argon2Params::default()) {
        Ok(o) => o,
        Err(e) => return fail(e),
    };
    println!(
        "已启用密码保护：{} 个文件按扇区密封，容器为v4。",
        outcome.report.encrypted
    );
    for w in &outcome.report.dropped {
        println!("自动移除（加密后失效）：{w}");
    }
    write_outcome(path, output, &outcome.bytes)
}

fn cmd_decrypt(path: &Path, password: &str, output: Option<PathBuf>) -> i32 {
    let bytes = match tasks::read_file(path) {
        Ok(b) => b,
        Err(e) => return fail(Error::Io(e)),
    };
    let outcome = match mpq::unprotect(&bytes, password) {
        Ok(o) => o,
        Err(e) => return fail(e),
    };
    println!(
        "已解除密码保护：{} 个文件解密，(password) 已移除。",
        outcome.report.decrypted
    );
    write_outcome(path, output, &outcome.bytes)
}

fn cmd_passwd(
    path: &Path,
    old_password: &str,
    new_password: &str,
    output: Option<PathBuf>,
) -> i32 {
    let bytes = match tasks::read_file(path) {
        Ok(b) => b,
        Err(e) => return fail(Error::Io(e)),
    };
    let out = match mpq::change_password(&bytes, old_password, new_password, &mpq::Argon2Params::default())
    {
        Ok(o) => o,
        Err(e) => return fail(e),
    };
    println!("已更换密码。");
    write_outcome(path, output, &out)
}

fn fail(e: Error) -> i32 {
    eprintln!("error: {e}");
    1
}
