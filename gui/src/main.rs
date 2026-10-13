//! mpqtool-gui — Tauri 2 graphical interface for mpqtool.
//!
//! The frontend (gui/ui) is static HTML/JS with no build step; this crate
//! hosts the mpq-powered commands it invokes.

// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod state;

use std::sync::Mutex;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Mutex::new(state::GuiState::default()))
        .invoke_handler(tauri::generate_handler![
            commands::open_archive,
            commands::reload_archive,
            commands::preview_file,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
