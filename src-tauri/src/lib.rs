//! BlendParser desktop app: wires the core use cases to the window.

mod commands;
mod dto;
mod error;
mod prefs;
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
            let dir = app.path().app_config_dir()?;
            let state = app.state::<state::AppState>();
            *state.printers.lock().expect("printer store") = profiles::Store::load(dir.join("printers.json"));
            *state.prefs.lock().expect("settings") = prefs::Prefs::load(dir.join("settings.json"));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::blender_status,
            commands::initial_file,
            commands::set_blender_path,
            commands::open_blend,
            commands::inspect_model,
            commands::set_palette,
            commands::suggest_palette,
            commands::reduce_palette,
            commands::merge_slots,
            commands::set_size,
            commands::undo_size,
            commands::reset_size,
            commands::fit_to_plate,
            commands::set_offset,
            commands::rotate,
            commands::reset_rotation,
            commands::cut_model,
            commands::preview_cut,
            commands::undo_cut,
            commands::detect_pieces,
            commands::separate_pieces,
            commands::propose_cuts,
            commands::select_region,
            commands::selection_op,
            commands::preview_region,
            commands::apply_region,
            commands::select_by_colour,
            commands::refine_to_paint,
            commands::set_mesh_ops,
            commands::analyze_printability,
            commands::fix_mesh,
            commands::orient_for_print,
            commands::set_rotation,
            commands::set_color_ops,
            commands::printers,
            commands::set_printer,
            commands::save_printer,
            commands::delete_printer,
            commands::prefs,
            commands::set_waste,
            commands::set_plate,
            commands::set_flush,
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
