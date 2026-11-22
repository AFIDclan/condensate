mod commands;
mod game_api;
mod realtime;
mod state;

use tauri::{Emitter, Manager};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be first: a second launch (e.g. from the CLI asking for a login) focuses this window.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
            let _ = app.emit("refresh-session", ());
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--minimized"])))
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_local_data_dir()?;
            let state = state::AppState::load(&config_dir, &data_dir);
            state.record_app_path();
            app.manage(state);
            app.manage(game_api::start(app.handle(), &config_dir)?);
            #[cfg(target_os = "linux")]
            std::thread::spawn(install_cli_from_appimage);
            // Launched at sign-in ("Start with Windows"): stay out of the way.
            if std::env::args().any(|a| a == "--minimized") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.minimize();
                }
            }
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
            commands::join_lobby,
            commands::open_game_folder,
            commands::create_invite,
            commands::list_invites,
            commands::update_profile,
            commands::upload_image,
            commands::friends,
            commands::search_users,
            commands::add_friend,
            commands::remove_friend,
            commands::conversations,
            commands::messages,
            commands::send_message,
            commands::mark_read,
            commands::profile,
            commands::my_games,
            commands::create_game,
            commands::update_game,
            commands::delete_version,
            commands::list_tokens,
            commands::create_token,
            commands::revoke_token,
            commands::set_library_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Condensate");
}

/// The .deb puts the bundled CLI in /usr/bin. An AppImage only has it inside its temporary
/// mount, so copy it to ~/.local/bin (on the PATH by default on most distros), refreshing the
/// copy whenever the app updates.
#[cfg(target_os = "linux")]
fn install_cli_from_appimage() {
    use std::os::unix::fs::PermissionsExt;
    if std::env::var_os("APPIMAGE").is_none() {
        return;
    }
    let Some(src) = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join("condensate"))) else { return };
    let Some(home) = std::env::var_os("HOME") else { return };
    let dir = std::path::Path::new(&home).join(".local/bin");
    let dest = dir.join("condensate");
    let Ok(bytes) = std::fs::read(&src) else { return };
    if std::fs::read(&dest).is_ok_and(|old| old == bytes) {
        return;
    }
    let tmp = dir.join(".condensate.tmp");
    let result = std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::write(&tmp, &bytes))
        .and_then(|_| std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)))
        .and_then(|_| std::fs::rename(&tmp, &dest));
    if let Err(e) = result {
        eprintln!("could not install the condensate CLI to {}: {e}", dest.display());
    }
}
