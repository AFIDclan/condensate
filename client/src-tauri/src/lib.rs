mod commands;
mod state;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_local_data_dir()?;
            app.manage(state::AppState::load(&config_dir, &data_dir));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::session,
            commands::login,
            commands::register,
            commands::logout,
            commands::library,
            commands::store,
            commands::game,
            commands::add_to_library,
            commands::remove_from_library,
            commands::install,
            commands::uninstall,
            commands::play,
            commands::open_game_folder,
            commands::create_invite,
            commands::list_invites,
            commands::update_profile,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Condensate");
}
