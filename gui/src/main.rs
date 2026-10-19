//! mpqtool-gui —— mpqtool 的 Tauri 2 图形界面。
//!
//! 前端（gui/ui）是使用 Vite 构建的 Vue 3 + Naive UI 应用
//! （`npm run build` → ui/dist）；本 crate 承载其调用的
//! 由 mpq 提供能力的各条命令。

// 避免 Windows 的 release 构建额外弹出一个控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod imagepreview;
mod state;

use std::sync::Mutex;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Mutex::new(state::GuiState::default()))
        .invoke_handler(tauri::generate_handler![
            commands::open_archive,
            commands::reload_archive,
            commands::close_archive,
            commands::unlock_archive,
            commands::apply_password_op,
            commands::write_password_note,
            commands::preview_file,
            commands::thumbnail,
            commands::read_file_bytes,
            commands::stage_edited_file,
            commands::stage_move_files,
            commands::extract,
            commands::diagnose_archive,
            commands::apply_repair,
            commands::save_repaired,
            commands::open_repaired,
            commands::discard_repair,
            commands::stat_imports,
            commands::apply_modify,
            commands::save_modified,
            commands::open_modified,
            commands::discard_modify,
            commands::create_archive,
            commands::apply_convert,
            commands::save_converted,
            commands::open_converted,
            commands::discard_convert,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
