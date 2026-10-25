//! BlendParser desktop app: wires the core use cases to the window.

mod commands;
mod dto;
mod profiles;
mod state;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_dialog::init()).plugin(tauri_plugin_process::init());
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    builder
        .manage(state::AppState::default())
        .setup(|app| {
            use tauri::Manager;
            let path = app.path().app_config_dir()?.join("printers.json");
            *app.state::<state::AppState>().printers.lock().expect("printer store") = profiles::Store::load(path);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::blender_status,
            commands::initial_file,
            commands::set_blender_path,
            commands::open_blend,
            commands::set_palette,
            commands::suggest_palette,
            commands::reduce_palette,
            commands::merge_slots,
            commands::set_height,
            commands::set_mesh_ops,
            commands::set_color_ops,
            commands::printers,
            commands::set_printer,
            commands::save_printer,
            commands::delete_printer,
            commands::preview_mesh,
            commands::pick,
            commands::paint,
            commands::end_stroke,
            commands::undo,
            commands::export_3mf,
            commands::export_color_zip,
        ])
        .run(tauri::generate_context!())
        .expect("error while running BlendParser");
}
