mod commands;
// The parsing and argument helpers only run on macOS; the tests exercise them everywhere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod crossover;
mod depot_errors;
mod error;
mod github;
mod installer;
mod missions;
mod models;
// Proton only runs on Linux; the tests exercise it everywhere.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod proton;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod proton_launch;
mod storage;

use tauri::{Manager, WebviewWindow};

// The layout is drawn for this window size in logical pixels; larger windows zoom it.
const DESIGN_WIDTH: f64 = 1180.0;
const DESIGN_HEIGHT: f64 = 680.0;

fn fit_zoom(window: &WebviewWindow) {
    let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) else {
        return;
    };
    let size = size.to_logical::<f64>(scale);
    let zoom = (size.width / DESIGN_WIDTH)
        .min(size.height / DESIGN_HEIGHT)
        .max(1.0);
    let _ = window.set_zoom(zoom);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(commands::OperationState::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Only the Proton launch tracks the game process.
            #[cfg(target_os = "linux")]
            app.manage(proton::ProcessTracker {
                processes: tokio::sync::Mutex::new(Vec::new()),
            });
            if let Some(window) = app.get_webview_window("main") {
                fit_zoom(&window);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(
                event,
                tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. }
            ) && let Some(webview) = window.app_handle().get_webview_window(window.label())
            {
                fit_zoom(&webview);
            }
            if matches!(
                event,
                tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
            ) {
                window
                    .app_handle()
                    .state::<commands::OperationState>()
                    .cancel_active();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_snapshot,
            commands::inspect_installation,
            commands::save_preferences,
            commands::get_proton_status,
            commands::list_proton_releases,
            commands::install_proton,
            commands::remove_proton,
            commands::initialize_proton_prefix,
            commands::run_operation,
            commands::send_terminal_input,
            commands::cancel_operation,
            commands::proton_operation_active,
            commands::launch_game,
            commands::game_status,
            commands::get_crossover_status,
            commands::prepare_crossover,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
