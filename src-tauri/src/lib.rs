pub mod active_tracker;
pub mod auth_webview;
pub mod commands;
pub mod crypto_dpapi;
pub mod local_discovery;
pub mod models;
pub mod oauth_browser;
pub mod poller;
pub mod providers;
pub mod settings;
pub mod storage;
pub mod tray;
pub mod updater;
pub mod webview_scraper;
pub mod window_manager;

use auth_webview::PendingAuthStore;
use models::WindowMode;
use oauth_browser::PendingOAuthStore;
use poller::{spawn_background_poller, AlertTracker};
use providers::ProviderRegistry;
use settings::SettingsStore;
use std::sync::Arc;
use std::time::Duration;
use storage::AccountStore;
use tauri::{Emitter, Manager};
use tauri_plugin_autostart::MacosLauncher;
use webview_scraper::WebviewBridgeStore;
use window_manager::{apply_window_mode, set_theme_vibrancy, WindowState};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .manage(WindowState::default())
        .setup(|app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("获取应用数据目录失败: {e}"))?;

            let settings_store = Arc::new(SettingsStore::from_app_data_dir(&app_data_dir)?);
            let initial_settings = settings_store.get();

            let store = Arc::new(AccountStore::from_app_data_dir(&app_data_dir)?);
            let registry = Arc::new(ProviderRegistry::new());
            let pending = Arc::new(PendingAuthStore::default());
            let pending_oauth = Arc::new(PendingOAuthStore::default());
            let bridge = Arc::new(WebviewBridgeStore::default());
            let tracker = Arc::new(AlertTracker::default());

            app.manage(Arc::clone(&settings_store));
            app.manage(Arc::clone(&store));
            app.manage(Arc::clone(&registry));
            app.manage(pending);
            app.manage(pending_oauth);
            app.manage(Arc::clone(&bridge));
            app.manage(Arc::clone(&tracker));

            if let Some(main_win) = app.get_webview_window("main") {
                let win_state = app.state::<WindowState>();
                set_theme_vibrancy(&main_win, &win_state, &initial_settings.theme);
                let _ = apply_window_mode(&main_win, &win_state, WindowMode::Panel);
            }

            tray::setup_tray(app.handle())?;
            spawn_background_poller(
                app.handle().clone(),
                Arc::clone(&store),
                registry,
                bridge,
                tracker,
                settings_store,
            );
            active_tracker::spawn_active_window_watcher(app.handle().clone(), Arc::clone(&store));

            if initial_settings.auto_check_updates && !initial_settings.update_feed_url.is_empty() {
                let handle = app.handle().clone();
                let feed = initial_settings.update_feed_url.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    let res = updater::check_for_updates(&feed).await;
                    if res.has_update {
                        let _ = handle.emit("update-available", res);
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_quota_snapshot,
            commands::refresh_all_quotas,
            commands::refresh_single_account,
            commands::toggle_pin_account,
            commands::set_active_account,
            commands::add_account,
            commands::remove_account,
            commands::clear_demo_accounts,
            commands::restore_demo_accounts,
            commands::detect_antigravity_local,
            commands::detect_codex_cli_local,
            commands::detect_claude_cli_local,
            commands::detect_commandcode_cli_local,
            commands::detect_opencode_cli_local,
            commands::scan_local_accounts,
            commands::open_in_default_browser,
            commands::open_auth_webview,
            commands::capture_auth_webview,
            commands::start_antigravity_browser_oauth,
            commands::start_codex_browser_oauth,
            commands::start_claude_browser_oauth,
            commands::complete_claude_browser_oauth,
            commands::cancel_browser_oauth,
            commands::set_window_mode,
            commands::snap_orb_to_edge,
            commands::set_theme_vibrancy,
            commands::get_autostart_enabled,
            commands::set_autostart_enabled,
            commands::get_app_settings,
            commands::save_app_settings,
            commands::check_for_updates,
            commands::install_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running AuraQuota");
}
