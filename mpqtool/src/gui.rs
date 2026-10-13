//! Graphical interface (egui): browse, preview, extract and repair.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use mpq::{Archive, FileEntry};

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
            install_cjk_fonts(cc);
            Ok(Box::new(MpqApp::default()))
        }),
    )
}

/// egui's bundled fonts have no CJK glyphs; load a system font so the
/// Chinese UI renders correctly. Falls back silently (tofu) if none found.
fn install_cjk_fonts(cc: &eframe::CreationContext<'_>) {
    let candidates = [
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/msyh.ttf",
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/simsun.ttc",
    ];
    let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("cjk".into(), Arc::new(egui::FontData::from_owned(bytes)));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("cjk".into());
    }
    cc.egui_ctx.set_fonts(fonts);
}

#[derive(Default)]
struct MpqApp {
    path: Option<PathBuf>,
    /// Raw bytes of the loaded archive (kept for the repair flow).
    raw: Option<Vec<u8>>,
    archive: Option<Archive<Cursor<Vec<u8>>>>,
    entries: Vec<FileEntry>,
    filter: String,
    /// Selected display names (stable across filter changes).
    selected: std::collections::BTreeSet<String>,
    log: String,
    preview: Option<Preview>,
    repair_ui: Option<RepairUi>,
}

struct Preview {
    name: String,
    text: String,
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
                self.path = Some(path.clone());
                let entries = archive.entries();
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

    fn filtered_entries(&self) -> Vec<&FileEntry> {
        let needle = self.filter.to_lowercase();
        self.entries
            .iter()
            .filter(|e| needle.is_empty() || e.display_name().to_lowercase().contains(&needle))
            .collect()
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
            .filter(|e| self.selected.contains(&e.display_name()))
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
        let Some(ui) = &mut self.repair_ui else { return };
        let Some(fixed) = ui.fixed.clone() else { return };
        let path = crate::tasks::repaired_path(&ui.source_path);
        self.load_bytes(path, fixed);
        self.repair_ui = None;
    }
}

enum MenuAction {
    Open,
    ExtractAll,
    Quit,
    Repair,
}

impl eframe::App for MpqApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw_menu(ui);
        self.draw_status(ui);
        self.draw_main(ui);
        self.draw_log(ui);
        let ctx = ui.ctx().clone();
        self.draw_preview(&ctx);
        self.draw_repair(&ctx);
    }
}

impl MpqApp {
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
            Some(MenuAction::Quit) => {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Some(MenuAction::Repair) => self.start_repair(),
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
        egui::CentralPanel::default().show(ui, |ui| {
            if self.archive.is_none() {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.4);
                    ui.heading("MPQ 归档浏览器");
                    ui.add_space(12.0);
                    if ui.button("打开归档…").clicked() {
                        self.open_dialog();
                    }
                });
                return;
            }

            ui.horizontal(|ui| {
                if ui.button("提取选中").clicked() {
                    self.extract_selected();
                }
                if ui.button("全选").clicked() {
                    for e in &self.entries {
                        self.selected.insert(e.display_name());
                    }
                }
                if ui.button("全不选").clicked() {
                    self.selected.clear();
                }
                ui.separator();
                ui.label("过滤：");
                ui.text_edit_singleline(&mut self.filter);
                if !self.filter.is_empty() && ui.button("✕").clicked() {
                    self.filter.clear();
                }
            });
            ui.add_space(4.0);

            // Own the rows so `self` stays free inside the drawing closure.
            let rows: Vec<FileEntry> = self
                .filtered_entries()
                .into_iter()
                .cloned()
                .collect();
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
                            ui.strong("标志");
                            ui.strong("地区");
                            ui.strong("来源");
                            ui.end_row();

                            for idx in range {
                                let entry = &rows[idx];
                                let display = entry.display_name();
                                let mut checked = self.selected.contains(&display);
                                if ui.checkbox(&mut checked, "").changed() {
                                    if checked {
                                        self.selected.insert(display.clone());
                                    } else {
                                        self.selected.remove(&display);
                                    }
                                }
                                let name_label = if entry.name.is_some() {
                                    display.clone()
                                } else {
                                    format!("⚠ {}", display)
                                };
                                let label = egui::RichText::new(name_label)
                                    .family(egui::FontFamily::Monospace);
                                let resp = ui
                                    .add(egui::Label::new(label).selectable(true))
                                    .interact(egui::Sense::click());
                                if resp.double_clicked() {
                                    self.preview(&display);
                                }
                                ui.label(entry.size.to_string());
                                ui.label(entry.compressed_size.to_string());
                                ui.label(crate::tasks::flag_summary(entry.flags));
                                ui.label(entry.locale.to_string());
                                ui.label(match entry.source {
                                    mpq::TableSource::Classic => "blk",
                                    mpq::TableSource::Bet => "bet",
                                });
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
}
