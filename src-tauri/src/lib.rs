//! BlendParser desktop app: wires the core use cases to the window.

mod commands;
mod dto;
mod error;
mod filaments;
mod prefs;
mod profiles;
mod saved;
mod state;

#[cfg(feature = "bench")]
#[doc(hidden)]
pub mod bench;

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
            *state.filaments.lock().expect("filaments") = filaments::FilamentStore::load(dir.join("filaments.json"));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::blender_status,
            commands::initial_file,
            commands::set_blender_path,
            commands::open_blend,
            commands::open_project,
            commands::save_project,
            commands::inspect_model,
            commands::set_palette,
            commands::filaments,
            commands::set_filament_owned,
            commands::save_filament,
            commands::delete_filament,
            commands::set_inventory_only,
            commands::set_slot_filament,
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
            commands::propose_split,
            commands::preview_split,
            commands::split_for_printer,
            commands::add_plate,
            commands::move_to_plate,
            commands::clone_object,
            commands::clone_copies,
            commands::arrange_plates,
            commands::clear_plate,
            commands::suggest_connectors,
            commands::preview_connected_cut,
            commands::cut_connected,
            commands::detect_pieces,
            commands::separate_pieces,
            commands::propose_cuts,
            commands::select_region,
            commands::selection_op,
            commands::suggest_regions,
            commands::set_selection,
            commands::preview_region,
            commands::apply_region,
            commands::select_by_colour,
            commands::refine_to_paint,
            commands::set_mesh_ops,
            commands::analyze_printability,
            commands::fix_mesh,
            commands::mesh_issues,
            commands::smooth_mesh,
            commands::hollow_mesh,
            commands::orient_for_print,
            commands::plan_orientation,
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
            commands::measure_thickness,
            commands::wire_mesh,
            commands::paint,
            commands::bucket_pick,
            commands::bucket,
            commands::end_stroke,
            commands::undo,
            commands::export_3mf,
            commands::export_color_zip,
            commands::cancel_color_zip,
            commands::export_color_assembly,
            commands::export_color_shells_zip,
        ])
        .run(tauri::generate_context!())
        .expect("error while running BlendParser");
}
