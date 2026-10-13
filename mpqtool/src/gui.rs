//! Graphical interface (egui): browse, preview, extract and repair.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use mpq::{Archive, FileEntry};

/// Body text size the UI starts with (egui's default).
const DEFAULT_FONT_SIZE: f32 = 13.0;
/// Range the font size can be adjusted to and restored from.
const FONT_SIZE_MIN: f32 = 10.0;
const FONT_SIZE_MAX: f32 = 28.0;
/// Storage key for the persisted font size.
const KEY_FONT_SIZE: &str = "font_size";

/// Scale every built-in text style so body text measures `size` points,
/// keeping egui's default proportions (Heading larger, Small smaller).
/// Recomputed from the pristine defaults each call, so applying twice
/// never compounds.
fn apply_font_size(ctx: &egui::Context, size: f32) {
    let base = &egui::Style::default().text_styles;
    let factor = size / base[&egui::TextStyle::Body].size;
    ctx.all_styles_mut(|style| {
        style.text_styles = base
            .iter()
            .map(|(k, f)| (k.clone(), egui::FontId::new(f.size * factor, f.family.clone())))
            .collect();
    });
}

/// Launch the GUI window; returns when the window is closed.
pub fn run() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("mpqtool — MPQ 归档浏览器")
            .with_inner_size([1020.0, 660.0]),
        ..Default::default()
    };
    eframe::run_native(
        "mpqtool",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_fonts(font_definitions());
            let mut app = MpqApp::default();
            if let Some(size) = cc
                .storage
                .and_then(|s| s.get_string(KEY_FONT_SIZE))
                .and_then(|v| v.trim().parse::<f32>().ok())
            {
                let size = size.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
                apply_font_size(&cc.egui_ctx, size);
                app.settings.font_size = size;
            }
            Ok(Box::new(app))
        }),
    )
}

/// egui's bundled fonts have no CJK glyphs, and neither they nor the Windows
/// CJK fonts cover the symbols used in the UI (📁 ▸ ▾ ✕ …) — missing glyphs
/// render as tofu (◻). Load system fonts as fallbacks, in priority order:
/// bundled fonts → CJK font → Segoe UI Emoji (covers all symbols above).
/// Falls back silently (tofu) when a font file is absent.
fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();

    let cjk_candidates = [
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/msyh.ttf",
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/simsun.ttc",
    ];
    let emoji_candidates = [
        "C:/Windows/Fonts/seguiemj.ttf", // Segoe UI Emoji
    ];

    let mut add_font = |name: &str, candidates: &[&str]| {
        if let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) {
            fonts
                .font_data
                .insert(name.to_string(), Arc::new(egui::FontData::from_owned(bytes)));
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(family).or_default().push(name.to_string());
            }
        }
    };

    add_font("cjk", &cjk_candidates);
    add_font("segoe-emoji", &emoji_candidates);
    fonts
}

struct Settings {
    /// Body text size in points; every other text style scales with it.
    font_size: f32,
    /// Whether the settings window is shown.
    open: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: DEFAULT_FONT_SIZE,
            open: false,
        }
    }
}

#[derive(Default)]
struct MpqApp {
    path: Option<PathBuf>,
    /// Raw bytes of the loaded archive (kept for the repair flow).
    raw: Option<Vec<u8>>,
    archive: Option<Archive<Cursor<Vec<u8>>>>,
    entries: Vec<FileEntry>,
    /// Virtual directory tree synthesized from the listfile paths.
    tree: crate::tasks::TreeNode,
    /// Modification times from `(attributes)`, by block index.
    times: std::collections::HashMap<usize, u64>,
    /// Current directory (path segments; empty = archive root).
    cwd: Vec<String>,
    /// Canonical (lowercase) paths of folders expanded in the tree panel.
    expanded: std::collections::BTreeSet<String>,
    filter: String,
    /// Selected files, keyed by table source + block index — stable across
    /// navigation, immune to duplicate display names (same name, different
    /// locale).
    selected: std::collections::BTreeSet<(mpq::TableSource, usize)>,
    log: String,
    preview: Option<Preview>,
    repair_ui: Option<RepairUi>,
    /// Pending (not yet applied) import / delete changes.
    modify_ui: Option<ModifyUi>,
    settings: Settings,
}

struct Preview {
    name: String,
    text: String,
}

/// A file queued for import.
struct PendingAdd {
    /// In-archive name (base name of the source file).
    name: String,
    data: Vec<u8>,
    /// Whether it will replace an existing entry of the same name.
    replaces: bool,
}

/// An entry queued for deletion.
struct PendingDel {
    /// Entry display name (None-name entries become `<unnamed #i>`).
    display: String,
    /// Deletion addressing: by name when known (deletes every locale
    /// variant), otherwise by table key (the only way to address unnamed
    /// entries).
    name: Option<String>,
    key: (mpq::TableSource, usize),
}

/// State of the "修改归档" window: pending changes, the applied result and
/// a status note — modelled after [`RepairUi`].
struct ModifyUi {
    additions: Vec<PendingAdd>,
    deletions: Vec<PendingDel>,
    /// Bytes + report after 应用修改; further edits are frozen then.
    applied: Option<(Vec<u8>, mpq::ModifyReport)>,
    note: String,
}

struct RepairUi {
    source_path: PathBuf,
    issues: Vec<mpq::RepairIssue>,
    /// Bytes after applying the repair.
    fixed: Option<Vec<u8>>,
    note: String,
}

impl MpqApp {
    fn logln(&mut self, msg: impl Into<String>) {
        self.log.push_str(&msg.into());
        self.log.push('\n');
    }

    fn load_bytes(&mut self, path: PathBuf, bytes: Vec<u8>) {
        let raw = bytes.clone();
        match Archive::new(Cursor::new(bytes)) {
            Ok(archive) => {
                let entries = archive.entries();
                let tree = crate::tasks::build_tree(&entries);
                let times = crate::tasks::file_time_map(&archive);
                self.path = Some(path.clone());
                self.log.clear();
                self.logln(format!(
                    "已打开 {}（{} 个条目）",
                    path.display(),
                    entries.len()
                ));
                for w in archive.warnings() {
                    self.logln(format!("warning: {w}"));
                }
                self.entries = entries;
                self.tree = tree;
                self.times = times;
                self.cwd.clear();
                self.expanded.clear();
                self.selected.clear();
                self.archive = Some(archive);
                self.raw = Some(raw);
            }
            Err(e) => {
                // Tolerant fallback: maybe only the header is damaged —
                // offer the repair flow right away.
                self.log.clear();
                self.logln(format!("打开失败：{e}"));
                self.logln("可以尝试「工具 → 修复头信息」。");
                self.archive = None;
                self.entries.clear();
                self.path = Some(path);
                self.raw = Some(raw);
            }
        }
    }

    fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("打开 MPQ 归档")
            .add_filter(
                "MPQ 归档",
                &["mpq", "w3x", "w3m", "w3n", "scx", "sc2map", "s2ma", "rep"],
            )
            .add_filter("所有文件", &["*"])
            .pick_file()
        {
            match std::fs::read(&path) {
                Ok(bytes) => self.load_bytes(path, bytes),
                Err(e) => self.logln(format!("读取失败：{e}")),
            }
        }
    }

    fn extract_selected(&mut self) {
        let Some(archive) = &self.archive else { return };
        if self.selected.is_empty() {
            self.logln("未选择任何文件。");
            return;
        }
        let Some(dir) = rfd::FileDialog::new().set_title("选择输出目录").pick_folder() else {
            return;
        };
        let picked: Vec<FileEntry> = self
            .entries
            .iter()
            .filter(|e| self.selected.contains(&(e.source, e.block_index)))
            .cloned()
            .collect();
        let result = crate::tasks::extract_entries(archive, &picked, &dir);
        for msg in result.messages {
            self.logln(msg);
        }
        self.logln(format!(
            "提取完成：{} 成功，{} 失败 → {}",
            result.ok,
            result.failed,
            dir.display()
        ));
    }

    fn extract_all(&mut self) {
        let Some(archive) = &self.archive else { return };
        let Some(dir) = rfd::FileDialog::new().set_title("选择输出目录").pick_folder() else {
            return;
        };
        let picked: Vec<FileEntry> = self
            .entries
            .iter()
            .filter(|e| e.name.is_some())
            .cloned()
            .collect();
        let result = crate::tasks::extract_entries(archive, &picked, &dir);
        for msg in result.messages {
            self.logln(msg);
        }
        self.logln(format!(
            "提取完成：{} 成功，{} 失败 → {}",
            result.ok,
            result.failed,
            dir.display()
        ));
    }

    fn preview(&mut self, name: &str) {
        let Some(archive) = &self.archive else { return };
        match archive.read_file(name) {
            Ok(data) => {
                const LIMIT: usize = 256 * 1024;
                let slice = &data[..data.len().min(LIMIT)];
                let text = String::from_utf8_lossy(slice).to_string();
                let note = if data.len() > LIMIT {
                    format!("（仅显示前 {} 字节，共 {} 字节）", LIMIT / 1024, data.len())
                } else {
                    format!("（{} 字节）", data.len())
                };
                self.preview = Some(Preview {
                    name: format!("{name} {note}"),
                    text,
                });
            }
            Err(e) => self.logln(format!("预览失败 {name}: {e}")),
        }
    }

    fn start_repair(&mut self) {
        let Some(path) = self.path.clone() else {
            self.logln("请先打开一个归档。");
            return;
        };
        let Some(bytes) = self.raw.clone() else { return };
        match mpq::diagnose(&bytes) {
            Ok(report) => {
                let note = if report.issues.is_empty() {
                    "未发现头信息问题。".to_string()
                } else {
                    String::new()
                };
                self.repair_ui = Some(RepairUi {
                    source_path: path,
                    issues: report.issues,
                    fixed: None,
                    note,
                });
            }
            Err(e) => self.logln(format!("诊断失败：{e}")),
        }
    }

    fn apply_repair(&mut self) {
        let Some(ui) = &mut self.repair_ui else { return };
        let Some(mut bytes) = self.raw.clone() else { return };
        match mpq::repair(&mut bytes, true) {
            Ok(report) => {
                ui.note = if report.changed {
                    format!(
                        "已应用 {} 项修复；{} 项无法自动修复。",
                        report.issues.len() - report.unfixed().len(),
                        report.unfixed().len()
                    )
                } else {
                    "没有需要修改的字段。".to_string()
                };
                ui.fixed = Some(bytes);
            }
            Err(e) => ui.note = format!("修复失败：{e}"),
        }
    }

    fn save_repaired(&mut self) {
        let Some(ui) = &mut self.repair_ui else { return };
        let Some(fixed) = ui.fixed.clone() else {
            ui.note = "请先应用修复。".to_string();
            return;
        };
        let suggested = crate::tasks::repaired_path(&ui.source_path);
        let Some(out) = rfd::FileDialog::new()
            .set_title("保存修复后的归档")
            .set_file_name(suggested.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default())
            .save_file()
        else {
            return;
        };
        match std::fs::write(&out, &fixed) {
            Ok(()) => {
                ui.note = format!("已保存 {}", out.display());
                self.logln(format!("修复副本已保存到 {}", out.display()));
            }
            Err(e) => ui.note = format!("保存失败：{e}"),
        }
    }

    fn reopen_repaired(&mut self) {
        let Some(ui) = &self.repair_ui else { return };
        let Some(fixed) = ui.fixed.clone() else { return };
        let path = crate::tasks::repaired_path(&ui.source_path);
        self.load_bytes(path, fixed);
        self.repair_ui = None;
    }

    // -- import / delete -----------------------------------------------------

    /// Whether changes were already applied and the pending list is frozen.
    fn modify_frozen(&self) -> bool {
        self.modify_ui
            .as_ref()
            .is_some_and(|m| m.applied.is_some())
    }

    /// Queue files from disk for import (multi-select dialog); opens the
    /// 修改归档 window with the pending list.
    fn import_files(&mut self) {
        let Some(paths) = rfd::FileDialog::new()
            .set_title("选择要导入的文件")
            .pick_files()
        else {
            return;
        };

        let mut ui = self.modify_ui.take().unwrap_or(ModifyUi {
            additions: Vec::new(),
            deletions: Vec::new(),
            applied: None,
            note: String::new(),
        });
        if ui.applied.is_some() {
            // Should not happen: the button is disabled while frozen.
            self.modify_ui = Some(ui);
            return;
        }

        let mut queued = 0;
        for path in paths {
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string())
            else {
                continue;
            };
            match std::fs::read(&path) {
                Ok(data) => {
                    let replaces = Self::replaces_existing(self.archive.as_ref(), &name, &ui);
                    self.logln(format!(
                        "待导入 {name}（{} 字节，{}）",
                        data.len(),
                        if replaces { "替换已有文件" } else { "新增" }
                    ));
                    // A second import of the same name supersedes the first.
                    match ui
                        .additions
                        .iter()
                        .position(|a| a.name.eq_ignore_ascii_case(&name))
                    {
                        Some(i) => {
                            ui.additions[i] = PendingAdd {
                                name,
                                data,
                                replaces,
                            };
                        }
                        None => ui.additions.push(PendingAdd {
                            name,
                            data,
                            replaces,
                        }),
                    }
                    queued += 1;
                }
                Err(e) => self.logln(format!("读取失败 {}: {e}", path.display())),
            }
        }
        if queued > 0 {
            self.logln(format!(
                "已加入导入队列（{queued} 项），在「修改归档」窗口中应用。"
            ));
        }
        self.modify_ui = Some(ui);
    }

    /// Whether `name` would replace an existing archive entry or a file
    /// already queued for import (ASCII case-insensitive, like MPQ).
    fn replaces_existing(
        archive: Option<&Archive<Cursor<Vec<u8>>>>,
        name: &str,
        pending: &ModifyUi,
    ) -> bool {
        archive.is_some_and(|a| a.find_file(name).is_some())
            || pending
                .additions
                .iter()
                .any(|a| a.name.eq_ignore_ascii_case(name))
    }

    /// Queue the currently selected entries for deletion.
    fn delete_selected(&mut self) {
        if self.selected.is_empty() {
            self.logln("未选择任何文件。");
            return;
        }
        let picked: Vec<(mpq::TableSource, usize)> =
            self.selected.iter().copied().collect();

        let mut ui = self.modify_ui.take().unwrap_or(ModifyUi {
            additions: Vec::new(),
            deletions: Vec::new(),
            applied: None,
            note: String::new(),
        });
        if ui.applied.is_some() {
            self.modify_ui = Some(ui);
            return;
        }
        let mut queued = 0;
        for key in picked {
            if ui.deletions.iter().any(|d| d.key == key) {
                continue;
            }
            let Some(entry) = self
                .entries
                .iter()
                .find(|e| (e.source, e.block_index) == key)
            else {
                continue;
            };
            ui.deletions.push(PendingDel {
                display: entry.display_name(),
                name: entry.name.clone(),
                key,
            });
            queued += 1;
        }
        if queued > 0 {
            self.logln(format!(
                "已标记 {queued} 项待删除，在「修改归档」窗口中应用。"
            ));
        }
        self.modify_ui = Some(ui);
    }

    fn apply_modify(&mut self) {
        let Some(ui) = &mut self.modify_ui else { return };
        let Some(raw) = self.raw.clone() else { return };
        let deletions: Vec<mpq::Deletion> = ui
            .deletions
            .iter()
            .map(|d| match &d.name {
                Some(n) => mpq::Deletion::ByName(n.clone()),
                None => mpq::Deletion::ByKey(d.key.0, d.key.1),
            })
            .collect();
        let additions: Vec<(String, Vec<u8>)> = ui
            .additions
            .iter()
            .map(|a| (a.name.clone(), a.data.clone()))
            .collect();
        match mpq::modify(&raw, &deletions, &additions) {
            Ok(outcome) => {
                ui.note = summarize_modify(&outcome.report);
                ui.applied = Some((outcome.bytes, outcome.report));
            }
            Err(e) => ui.note = format!("修改失败：{e}"),
        }
    }

    /// Save the applied modification; the archive is reloaded from the
    /// written file afterwards.
    fn save_modified(&mut self) {
        let Some(ui) = &self.modify_ui else { return };
        let Some((bytes, _)) = &ui.applied else { return };
        let bytes = bytes.clone();
        let suggested = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(out) = rfd::FileDialog::new()
            .set_title("保存修改后的归档")
            .set_file_name(suggested)
            .save_file()
        else {
            return;
        };
        match std::fs::write(&out, &bytes) {
            Ok(()) => {
                self.logln(format!("修改已保存到 {}", out.display()));
                let path = out.clone();
                self.load_bytes(path, bytes);
                self.modify_ui = None;
            }
            Err(e) => {
                if let Some(ui) = &mut self.modify_ui {
                    ui.note = format!("保存失败：{e}");
                }
            }
        }
    }

    /// Open the applied modification in the UI without saving it to disk.
    fn open_modified(&mut self) {
        let Some(ui) = &self.modify_ui else { return };
        let Some((bytes, report)) = &ui.applied else { return };
        let bytes = bytes.clone();
        let report = report.clone();
        let path = self.path.clone().unwrap_or_else(|| {
            PathBuf::from("modified.mpq")
        });
        self.load_bytes(path, bytes);
        self.logln(format!("已载入修改结果（{}）", summarize_modify(&report)));
        self.modify_ui = None;
    }
}

/// One-line summary of a modification report (used in the window and log).
fn summarize_modify(report: &mpq::ModifyReport) -> String {
    let mut parts = Vec::new();
    if !report.added.is_empty() {
        parts.push(format!("新增 {}", report.added.len()));
    }
    if !report.replaced.is_empty() {
        parts.push(format!("替换 {}", report.replaced.len()));
    }
    if !report.deleted.is_empty() {
        parts.push(format!("删除 {}", report.deleted.len()));
    }
    if !report.dropped.is_empty() {
        parts.push(format!(
            "自动移除 {}",
            report
                .dropped
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("、")
        ));
    }
    if !report.re_encoded.is_empty() {
        parts.push(format!("重新编码 {}", report.re_encoded.len()));
    }
    if parts.is_empty() {
        "没有变化。".to_string()
    } else {
        parts.join("；") + "。"
    }
}

enum MenuAction {
    Open,
    ExtractAll,
    Import,
    Quit,
    Repair,
    Settings,
}

/// One folder row in the left-hand tree (recursively draws children).
///
/// Kept a free function so the borrow of `expanded` / `nav` stays disjoint
/// from the other `MpqApp` fields the surrounding closures read.
fn draw_tree_node(
    ui: &mut egui::Ui,
    node: &crate::tasks::TreeNode,
    path: &[String],
    cwd: &[String],
    needle: &str,
    expanded: &mut std::collections::BTreeSet<String>,
    nav: &mut Option<Vec<String>>,
) {
    if !needle.is_empty() && !crate::tasks::subtree_matches(node, needle) {
        return;
    }

    let key = path
        .iter()
        .map(|s| s.to_lowercase())
        .collect::<Vec<_>>()
        .join("\\");
    // While searching, matching branches are forced open.
    let open = !needle.is_empty() || expanded.contains(&key);
    let icon = if node.name == crate::tasks::SYSTEM_DIR {
        "🗂"
    } else if node.name == crate::tasks::UNNAMED_DIR {
        "❔"
    } else {
        "📁"
    };
    let arrow = if node.dirs.is_empty() {
        " "
    } else if open {
        "▾"
    } else {
        "▸"
    };
    let label = format!("{arrow} {icon} {} ({})", node.name, node.file_count);
    let active = cwd == path;

    ui.horizontal(|ui| {
        ui.add_space((path.len().saturating_sub(1)) as f32 * 14.0);
        let resp = ui.selectable_label(active, egui::RichText::new(label).small());
        if resp.clicked() {
            *nav = Some(path.to_vec());
            if !node.dirs.is_empty() && needle.is_empty() {
                if expanded.contains(&key) {
                    expanded.remove(&key);
                } else {
                    expanded.insert(key);
                }
            }
        }
    });

    if open {
        for d in &node.dirs {
            let mut child_path = path.to_vec();
            child_path.push(d.name.clone());
            draw_tree_node(ui, d, &child_path, cwd, needle, expanded, nav);
        }
    }
}

impl eframe::App for MpqApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_dropped_files(ui.ctx());
        self.draw_menu(ui);
        self.draw_status(ui);
        self.draw_main(ui);
        self.draw_log(ui);
        let ctx = ui.ctx().clone();
        self.draw_drop_overlay(&ctx);
        self.draw_preview(&ctx);
        self.draw_repair(&ctx);
        self.draw_modify(&ctx);
        self.draw_settings(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(KEY_FONT_SIZE, self.settings.font_size.to_string());
    }
}

impl MpqApp {
    /// React to files dragged & dropped onto the window. The winit backend
    /// surfaces drop events for one frame, so a plain per-frame check is
    /// enough (re-dropping the same file later re-triggers it).
    fn poll_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        let Some(first) = dropped.first() else { return };
        let path = first.path().to_path_buf();
        match first.bytes() {
            Ok(bytes) => {
                let extras = dropped.len().saturating_sub(1);
                self.load_bytes(path, bytes);
                self.logln("（通过拖拽打开）");
                if extras > 0 {
                    self.logln(format!(
                        "（忽略了另外 {extras} 个拖入的文件，一次只能打开一个归档）"
                    ));
                }
            }
            Err(e) => self.logln(format!("读取拖入文件失败：{e}")),
        }
    }

    /// Highlight the window while files are dragged over it.
    fn draw_drop_overlay(&self, ctx: &egui::Context) {
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if !hovering {
            return;
        }
        let screen = ctx.viewport_rect();
        let pos = screen.center() - egui::vec2(150.0, 26.0);
        egui::Area::new(egui::Id::new("drop_overlay"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(egui::Color32::from_rgba_unmultiplied(28, 96, 170, 235))
                    .corner_radius(10.0)
                    .inner_margin(egui::Margin::symmetric(26, 14))
                    .show(ui, |ui| {
                        ui.heading("📂 松开鼠标以打开归档");
                    });
            });
    }

    fn draw_menu(&mut self, ui: &mut egui::Ui) {
        let mut action = None;
        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                egui::containers::menu::MenuButton::new("文件").ui(ui, |ui| {
                    if ui.button("打开归档…").clicked() {
                        action = Some(MenuAction::Open);
                    }
                    if ui
                        .add_enabled(self.archive.is_some(), egui::Button::new("提取全部…"))
                        .clicked()
                    {
                        action = Some(MenuAction::ExtractAll);
                    }
                    if ui
                        .add_enabled(
                            self.archive.is_some() && !self.modify_frozen(),
                            egui::Button::new("导入文件到归档…"),
                        )
                        .clicked()
                    {
                        action = Some(MenuAction::Import);
                    }
                    if ui.button("退出").clicked() {
                        action = Some(MenuAction::Quit);
                    }
                });
                egui::containers::menu::MenuButton::new("工具").ui(ui, |ui| {
                    if ui
                        .add_enabled(self.path.is_some(), egui::Button::new("修复头信息…"))
                        .clicked()
                    {
                        action = Some(MenuAction::Repair);
                    }
                });
                egui::containers::menu::MenuButton::new("设置").ui(ui, |ui| {
                    if ui.button("界面设置…").clicked() {
                        action = Some(MenuAction::Settings);
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(
                        self.path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "未打开归档".to_string()),
                    );
                });
            });
        });
        match action {
            Some(MenuAction::Open) => self.open_dialog(),
            Some(MenuAction::ExtractAll) => self.extract_all(),
            Some(MenuAction::Import) => self.import_files(),
            Some(MenuAction::Quit) => {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Some(MenuAction::Repair) => self.start_repair(),
            Some(MenuAction::Settings) => self.settings.open = true,
            None => {}
        }
    }

    fn draw_status(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(a) = &self.archive {
                    let h = a.header();
                    ui.label(format!("MPQ v{}", a.version().as_u32()));
                    ui.separator();
                    ui.label(format!("扇区 {} B", a.sector_size()));
                    ui.separator();
                    ui.label(format!(
                        "hash {} / block {}",
                        h.hash_table_size, h.block_table_size
                    ));
                    if a.uses_het_bet() {
                        ui.label("+ HET/BET");
                    }
                    ui.separator();
                    ui.label(format!("listfile {} 项", a.file_names().len()));
                    ui.separator();
                    ui.label(format!("警告 {}", a.warnings().len()));
                } else {
                    ui.weak("打开一个 .mpq / .w3x / .sc2map 文件开始");
                }
            });
        });
    }

    fn draw_main(&mut self, ui: &mut egui::Ui) {
        if self.archive.is_none() {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.4);
                    ui.heading("MPQ 归档浏览器");
                    ui.add_space(12.0);
                    if ui.button("打开归档…").clicked() {
                        self.open_dialog();
                    }
                    ui.add_space(8.0);
                    ui.weak("或直接把 .mpq / .w3x / .sc2map 文件拖进窗口");
                });
            });
            return;
        }

        // ---- rows for the right-hand table, assembled up front so the
        // drawing closures never need `&mut self` as a whole ----------------
        let searching = !self.filter.trim().is_empty();
        let needle = self.filter.trim().to_lowercase();

        enum Row {
            Dir {
                name: String,
                files: usize,
                size: u64,
                csize: u64,
                all: bool,
                keys: Vec<(mpq::TableSource, usize)>,
            },
            File {
                base: String,
                full: Option<String>,
                size: u32,
                csize: u32,
                flags: u32,
                locale: u16,
                source: mpq::TableSource,
                key: (mpq::TableSource, usize),
                mtime: Option<u64>,
            },
        }

        let mut rows: Vec<Row> = Vec::new();
        if searching {
            for e in crate::tasks::search_tree(&self.tree, &needle) {
                let full = e.name.clone();
                rows.push(Row::File {
                    base: full
                        .clone()
                        .unwrap_or_else(|| e.display_name()),
                    full,
                    size: e.size,
                    csize: e.compressed_size,
                    flags: e.flags,
                    locale: e.locale,
                    source: e.source,
                    key: (e.source, e.block_index),
                    mtime: self.times.get(&e.block_index).copied(),
                });
            }
        } else if let Some(dir) = crate::tasks::node_at(&self.tree, &self.cwd) {
            for d in &dir.dirs {
                let keys = crate::tasks::collect_keys(d);
                let all = !keys.is_empty() && keys.iter().all(|k| self.selected.contains(k));
                rows.push(Row::Dir {
                    name: d.name.clone(),
                    files: d.file_count,
                    size: d.total_size,
                    csize: d.total_csize,
                    all,
                    keys,
                });
            }
            for f in &dir.files {
                let full = f.name.clone();
                let base = match &full {
                    Some(n) => n.rsplit(['\\', '/']).next().unwrap_or(n).to_string(),
                    None => f.display_name(),
                };
                rows.push(Row::File {
                    base,
                    full,
                    size: f.size,
                    csize: f.compressed_size,
                    flags: f.flags,
                    locale: f.locale,
                    source: f.source,
                    key: (f.source, f.block_index),
                    mtime: self.times.get(&f.block_index).copied(),
                });
            }
        }

        // ---- deferred intents, applied once the panels are drawn ----------
        #[derive(Clone, Copy, PartialEq)]
        enum ToolAct {
            Extract,
            Delete,
            SelectAll,
            SelectNone,
        }
        let mut nav: Option<Vec<String>> = None;
        let mut toggle_dir: Option<(Vec<(mpq::TableSource, usize)>, bool)> = None;
        let mut toggle_file: Option<((mpq::TableSource, usize), bool)> = None;
        let mut preview_name: Option<String> = None;
        let mut tool: Option<ToolAct> = None;

        // ---- left panel: folder tree ---------------------------------------
        egui::Panel::left("tree")
            .resizable(true)
            .default_size(230.0)
            .min_size(150.0)
            .show(ui, |ui| {
                ui.strong(format!("文件夹（{} 项）", self.tree.file_count));
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let root_active = self.cwd.is_empty() && !searching;
                        if ui
                            .selectable_label(root_active, "🏠 归档根目录")
                            .on_hover_text("回到归档顶层")
                            .clicked()
                        {
                            nav = Some(Vec::new());
                        }
                        for d in &self.tree.dirs {
                            let child = vec![d.name.clone()];
                            draw_tree_node(
                                ui,
                                d,
                                &child,
                                &self.cwd,
                                &needle,
                                &mut self.expanded,
                                &mut nav,
                            );
                        }
                    });
            });

        // ---- right panel: toolbar, breadcrumb, file table ------------------
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                let delete_enabled =
                    !self.selected.is_empty() && !self.modify_frozen();
                if ui
                    .add_enabled(
                        !self.selected.is_empty(),
                        egui::Button::new("提取选中"),
                    )
                    .clicked()
                {
                    tool = Some(ToolAct::Extract);
                }
                if ui
                    .add_enabled(delete_enabled, egui::Button::new("删除选中"))
                    .on_hover_text("标记选中条目待删除，随后在「修改归档」窗口应用")
                    .clicked()
                {
                    tool = Some(ToolAct::Delete);
                }
                if ui.button("全选").clicked() {
                    tool = Some(ToolAct::SelectAll);
                }
                if ui.button("全不选").clicked() {
                    tool = Some(ToolAct::SelectNone);
                }
                ui.separator();
                ui.label("过滤：");
                ui.text_edit_singleline(&mut self.filter);
                if !self.filter.is_empty() && ui.button("✕").clicked() {
                    self.filter.clear();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(format!("已选 {} 项", self.selected.len()));
                });
            });
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                if searching {
                    ui.strong(format!("搜索 “{}”：{} 个匹配", self.filter.trim(), rows.len()));
                } else {
                    let at_root = self.cwd.is_empty();
                    if ui
                        .add_enabled(!at_root, egui::Button::new("⬆ 上一级"))
                        .clicked()
                    {
                        nav = Some(self.cwd[..self.cwd.len() - 1].to_vec());
                    }
                    if ui.button("🏠").clicked() {
                        nav = Some(Vec::new());
                    }
                    for (i, seg) in self.cwd.iter().enumerate() {
                        ui.label("›");
                        if ui.button(seg).clicked() {
                            nav = Some(self.cwd[..=i].to_vec());
                        }
                    }
                    if let Some(dir) = crate::tasks::node_at(&self.tree, &self.cwd) {
                        ui.weak(format!(
                            "（{} 个子文件夹，{} 个文件）",
                            dir.dirs.len(),
                            dir.files.len()
                        ));
                    }
                }
            });
            ui.add_space(4.0);

            const MAX_ROWS: usize = 5000;
            let truncated = rows.len() > MAX_ROWS;

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_rows(ui, 20.0, rows.len().min(MAX_ROWS), |ui, range| {
                    egui::Grid::new("files")
                        .striped(true)
                        .min_col_width(24.0)
                        .show(ui, |ui| {
                            ui.strong("✔");
                            ui.strong("名称");
                            ui.strong("大小");
                            ui.strong("压缩后");
                            ui.strong("属性");
                            ui.strong("修改时间(UTC)");
                            ui.strong("地区");
                            ui.strong("来源");
                            ui.end_row();

                            for idx in range {
                                match &rows[idx] {
                                    Row::Dir {
                                        name,
                                        files,
                                        size,
                                        csize,
                                        all,
                                        keys,
                                    } => {
                                        let mut checked = *all;
                                        if ui
                                            .checkbox(&mut checked, "")
                                            .on_hover_text("选中此文件夹下的全部文件")
                                            .changed()
                                        {
                                            toggle_dir = Some((keys.clone(), checked));
                                        }
                                        let label = egui::RichText::new(format!(
                                            "📁 {name}（{files} 项）"
                                        ))
                                        .family(egui::FontFamily::Monospace);
                                        let resp = ui
                                            .add(egui::Label::new(label).selectable(true))
                                            .interact(egui::Sense::click())
                                            .on_hover_text("双击进入文件夹");
                                        if resp.double_clicked() {
                                            let mut target = self.cwd.clone();
                                            target.push(name.clone());
                                            nav = Some(target);
                                        }
                                        ui.label(size.to_string());
                                        ui.label(csize.to_string());
                                        ui.label("");
                                        ui.label("");
                                        ui.label("");
                                        ui.label("");
                                    }
                                    Row::File {
                                        base,
                                        full,
                                        size,
                                        csize,
                                        flags,
                                        locale,
                                        source,
                                        key,
                                        mtime,
                                    } => {
                                        let mut checked = self.selected.contains(key);
                                        if ui.checkbox(&mut checked, "").changed() {
                                            toggle_file = Some((*key, checked));
                                        }
                                        let name_label = if full.is_some() {
                                            base.clone()
                                        } else {
                                            format!("⚠ {}", base)
                                        };
                                        let label = egui::RichText::new(name_label)
                                            .family(egui::FontFamily::Monospace);
                                        let resp = ui
                                            .add(egui::Label::new(label).selectable(true))
                                            .interact(egui::Sense::click());
                                        if resp.double_clicked() {
                                            if let Some(name) = full {
                                                preview_name = Some(name.clone());
                                            } else {
                                                self.logln(
                                                    "未命名文件无法预览（缺少名字无法解密/定位）",
                                                );
                                            }
                                        }
                                        ui.label(size.to_string());
                                        ui.label(csize.to_string());
                                        ui.label(crate::tasks::flag_summary(*flags));
                                        ui.label(match mtime {
                                            Some(t) => crate::tasks::format_filetime(*t),
                                            None => "—".to_string(),
                                        });
                                        ui.label(locale.to_string());
                                        ui.label(match source {
                                            mpq::TableSource::Classic => "blk",
                                            mpq::TableSource::Bet => "bet",
                                        });
                                    }
                                }
                                ui.end_row();
                            }
                        });
                });
            if truncated {
                ui.weak(format!(
                    "（结果过多，仅显示前 {MAX_ROWS} 项，请用过滤缩小范围）"
                ));
            }
        });

        // ---- apply deferred intents ----------------------------------------
        if let Some((keys, set)) = toggle_dir {
            if set {
                self.selected.extend(keys);
            } else {
                for k in &keys {
                    self.selected.remove(k);
                }
            }
        }
        if let Some((key, set)) = toggle_file {
            if set {
                self.selected.insert(key);
            } else {
                self.selected.remove(&key);
            }
        }
        if let Some(target) = nav {
            self.cwd = target;
        }
        if let Some(name) = preview_name {
            self.preview(&name);
        }
        match tool {
            Some(ToolAct::Extract) => self.extract_selected(),
            Some(ToolAct::Delete) => self.delete_selected(),
            Some(ToolAct::SelectAll) => {
                let all: Vec<FileEntry> = if searching {
                    crate::tasks::search_tree(&self.tree, &needle)
                } else if let Some(dir) = crate::tasks::node_at(&self.tree, &self.cwd) {
                    crate::tasks::collect_files(dir)
                } else {
                    Vec::new()
                };
                self.selected.extend(all.iter().map(|e| (e.source, e.block_index)));
            }
            Some(ToolAct::SelectNone) => self.selected.clear(),
            None => {}
        }
    }

    fn draw_log(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("log")
            .resizable(true)
            .default_size(120.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("日志");
                    if ui.small_button("清空").clicked() {
                        self.log.clear();
                    }
                });
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.log)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .interactive(false),
                        );
                    });
            });
    }

    fn draw_preview(&mut self, ctx: &egui::Context) {
        let mut open = self.preview.is_some();
        if !open {
            return;
        }
        let mut close = false;
        egui::Window::new("预览")
            .open(&mut open)
            .default_size([640.0, 480.0])
            .show(ctx, |ui| {
                if let Some(p) = &self.preview {
                    ui.strong(&p.name);
                    ui.separator();
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut p.text.as_str())
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .interactive(false),
                        );
                    });
                }
                if ui.button("关闭").clicked() {
                    close = true;
                }
            });
        if close || !open {
            self.preview = None;
        }
    }

    fn draw_repair(&mut self, ctx: &egui::Context) {
        #[derive(Clone, Copy, PartialEq)]
        enum RepairAction {
            Apply,
            Save,
            Reopen,
        }

        let mut open = self.repair_ui.is_some();
        if !open {
            return;
        }
        let mut close = false;
        let mut action = None;
        egui::Window::new("修复头信息")
            .open(&mut open)
            .default_size([560.0, 420.0])
            .show(ctx, |ui| {
                let Some(r) = &self.repair_ui else { return };
                ui.label(format!("来源：{}", r.source_path.display()));
                ui.separator();

                if r.issues.is_empty() {
                    ui.label("未发现头信息问题。");
                } else {
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for issue in &r.issues {
                            ui.horizontal(|ui| {
                                match &issue.action {
                                    Some(act) => {
                                        ui.colored_label(
                                            egui::Color32::from_rgb(200, 140, 0),
                                            "可修复",
                                        );
                                        ui.monospace(&issue.field);
                                        ui.label(format!("{} → {}", issue.problem, act));
                                    }
                                    None => {
                                        ui.colored_label(
                                            egui::Color32::from_rgb(190, 60, 60),
                                            "无法修复",
                                        );
                                        ui.monospace(&issue.field);
                                        ui.label(&issue.problem);
                                    }
                                }
                            });
                        }
                    });
                    ui.separator();
                }

                ui.horizontal(|ui| {
                    let can_apply = r.fixed.is_none() && !r.issues.is_empty();
                    if ui
                        .add_enabled(can_apply, egui::Button::new("应用修复"))
                        .clicked()
                    {
                        action = Some(RepairAction::Apply);
                    }
                    let can_save = r.fixed.is_some();
                    if ui
                        .add_enabled(can_save, egui::Button::new("保存修复副本…"))
                        .clicked()
                    {
                        action = Some(RepairAction::Save);
                    }
                    if ui
                        .add_enabled(can_save, egui::Button::new("在界面中打开修复结果"))
                        .clicked()
                    {
                        action = Some(RepairAction::Reopen);
                    }
                    if ui.button("关闭").clicked() {
                        close = true;
                    }
                });
                if !r.note.is_empty() {
                    ui.add_space(6.0);
                    ui.label(&r.note);
                }
            });
        match action {
            Some(RepairAction::Apply) => self.apply_repair(),
            Some(RepairAction::Save) => self.save_repaired(),
            Some(RepairAction::Reopen) => {
                self.reopen_repaired();
                return;
            }
            None => {}
        }
        if close || !open {
            self.repair_ui = None;
        }
    }

    fn draw_modify(&mut self, ctx: &egui::Context) {
        #[derive(Clone, Copy, PartialEq)]
        enum ModifyAction {
            Apply,
            Save,
            Reopen,
        }

        let mut open = self.modify_ui.is_some();
        if !open {
            return;
        }
        let mut close = false;
        let mut action = None;
        let mut remove_add: Option<usize> = None;
        let mut remove_del: Option<usize> = None;

        egui::Window::new("修改归档")
            .open(&mut open)
            .default_size([560.0, 440.0])
            .show(ctx, |ui| {
                let Some(m) = &self.modify_ui else { return };
                let frozen = m.applied.is_some();

                ui.label(format!(
                    "来源：{}",
                    self.path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "（未保存）".to_string())
                ));
                ui.separator();

                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    ui.strong(format!("待导入（{} 项）", m.additions.len()));
                    if m.additions.is_empty() {
                        ui.weak("（无）— 用「文件 → 导入文件到归档…」添加");
                    }
                    for (i, a) in m.additions.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let tag = if a.replaces { "替换" } else { "新增" };
                            ui.label(tag);
                            ui.monospace(&a.name);
                            ui.weak(format!("{} 字节", a.data.len()));
                            if !frozen && ui.small_button("✕").clicked() {
                                remove_add = Some(i);
                            }
                        });
                    }
                    ui.add_space(8.0);
                    ui.strong(format!("待删除（{} 项）", m.deletions.len()));
                    if m.deletions.is_empty() {
                        ui.weak("（无）— 勾选文件后点「删除选中」");
                    }
                    for (i, d) in m.deletions.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label("删除");
                            ui.monospace(&d.display);
                            if !frozen && ui.small_button("✕").clicked() {
                                remove_del = Some(i);
                            }
                        });
                    }
                    ui.add_space(4.0);
                    ui.weak(
                        "应用时保留文件将原样拷贝；(listfile) 重建，(attributes)/(signature) 自动移除。",
                    );
                });
                ui.separator();

                ui.horizontal(|ui| {
                    let can_apply = !frozen
                        && (!m.additions.is_empty() || !m.deletions.is_empty());
                    if ui
                        .add_enabled(can_apply, egui::Button::new("应用修改"))
                        .clicked()
                    {
                        action = Some(ModifyAction::Apply);
                    }
                    let has_result = m.applied.is_some();
                    if ui
                        .add_enabled(has_result, egui::Button::new("保存到文件…"))
                        .on_hover_text("选择保存位置（默认覆盖原文件）后自动重新载入")
                        .clicked()
                    {
                        action = Some(ModifyAction::Save);
                    }
                    if ui
                        .add_enabled(has_result, egui::Button::new("在界面中打开结果"))
                        .on_hover_text("不写盘，直接在界面中浏览修改结果")
                        .clicked()
                    {
                        action = Some(ModifyAction::Reopen);
                    }
                    if ui.button("关闭").clicked() {
                        close = true;
                    }
                });
                if !m.note.is_empty() {
                    ui.add_space(6.0);
                    ui.label(&m.note);
                }
            });

        // Apply deferred intents (borrow-safe, like the other windows).
        if let Some(i) = remove_add {
            if let Some(m) = &mut self.modify_ui {
                if m.applied.is_none() {
                    m.additions.remove(i);
                }
            }
        }
        if let Some(i) = remove_del {
            if let Some(m) = &mut self.modify_ui {
                if m.applied.is_none() {
                    m.deletions.remove(i);
                }
            }
        }
        match action {
            Some(ModifyAction::Apply) => self.apply_modify(),
            Some(ModifyAction::Save) => self.save_modified(),
            Some(ModifyAction::Reopen) => {
                self.open_modified();
                return;
            }
            None => {}
        }
        if close || !open {
            self.modify_ui = None;
        }
    }

    fn draw_settings(&mut self, ctx: &egui::Context) {
        if !self.settings.open {
            return;
        }
        let mut open = self.settings.open;
        let before = self.settings.font_size;
        egui::Window::new("设置")
            .open(&mut open)
            .resizable(false)
            .default_size([380.0, 220.0])
            .show(ctx, |ui| {
                ui.heading("字体大小");
                ui.add_space(6.0);
                ui.add(
                    egui::Slider::new(&mut self.settings.font_size, FONT_SIZE_MIN..=FONT_SIZE_MAX)
                        .step_by(0.5)
                        .text("pt"),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("预设：");
                    for (label, size) in
                        [("小", 11.0), ("中", 13.0), ("大", 16.0), ("特大", 19.0)]
                    {
                        let active = (self.settings.font_size - size).abs() < 0.01;
                        if ui.selectable_label(active, label).clicked() {
                            self.settings.font_size = size;
                        }
                    }
                    if ui.button("恢复默认").clicked() {
                        self.settings.font_size = DEFAULT_FONT_SIZE;
                    }
                });
                ui.add_space(10.0);
                ui.label("预览：MPQ 归档浏览器 — 文件列表 ABCabc 0123456789");
                ui.monospace("预览（等宽）：war3map.j  SECTOR=4096");
                ui.add_space(6.0);
                ui.weak("字号立即应用于整个界面，并自动保存。");
            });
        self.settings.open = open;
        if self.settings.font_size != before {
            apply_font_size(ctx, self.settings.font_size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Icons used by the UI plus a CJK sample must rasterize to real glyphs
    /// after the fallback fonts are loaded.
    ///
    /// Neither `has_glyph` nor advance widths can tell a real glyph from the
    /// replacement glyph here: the replacement face is NotoEmoji, which also
    /// owns several of the icons, and its emoji all share one advance width.
    /// So compare pixels: rasterize each char in a fresh font atlas and
    /// check its bitmap differs from the bitmap of a guaranteed-missing
    /// (unassigned) codepoint.
    #[test]
    #[cfg(windows)]
    fn icon_and_cjk_glyphs_resolve() {
        fn rasterized(c: char, family: egui::FontFamily) -> egui::epaint::ColorImage {
            let mut fonts =
                egui::epaint::text::Fonts::new(Default::default(), font_definitions());
            let galley = fonts.with_pixels_per_point(1.0).layout(
                c.to_string(),
                egui::FontId::new(14.0, family),
                egui::Color32::WHITE,
                f32::INFINITY,
            );
            assert!(
                galley.rows.iter().any(|r| !r.glyphs.is_empty()),
                "{c:?} produced no glyphs at all"
            );
            fonts.image()
        }

        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let tofu = rasterized('\u{0378}', family.clone());
            for c in ['📁', '📂', '🏠', '🗂', '❔', '⬆', '▸', '▾', '⚠', '✔', '✕', '›', '文'] {
                let img = rasterized(c, family.clone());
                assert!(
                    img.size != tofu.size || img.pixels != tofu.pixels,
                    "{c:?} in {family:?} rasterizes as the tofu replacement glyph"
                );
            }
        }
    }
}
