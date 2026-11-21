use crate::models::WindowMode;
use crate::providers::{now_hh_mm, ProviderContext, ProviderRegistry};
use crate::storage::AccountStore;
use crate::webview_scraper::WebviewBridgeStore;
use crate::window_manager::{apply_window_mode, WindowState};
use std::sync::Arc;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;

pub const TRAY_ID: &str = "auraquota-tray";
const ID_TOGGLE: &str = "tray_toggle";
const ID_ORB: &str = "tray_orb";
const ID_REFRESH: &str = "tray_refresh";
const ID_AUTOSTART: &str = "tray_autostart";
const ID_CHECK_UPDATE: &str = "tray_check_update";
const ID_QUIT: &str = "tray_quit";

pub struct TrayState {
    pub autostart_item: CheckMenuItem<tauri::Wry>,
}

pub fn setup_tray(app: &AppHandle) -> Result<(), String> {
    let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);

    let toggle_item = MenuItem::with_id(app, ID_TOGGLE, "展开 / 收起", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let orb_item = MenuItem::with_id(app, ID_ORB, "切换悬浮球", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let refresh_item = MenuItem::with_id(app, ID_REFRESH, "立即刷新", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let sep1 = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    let autostart_item = CheckMenuItem::with_id(
        app,
        ID_AUTOSTART,
        "开机自启",
        true,
        autostart_enabled,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;
    let sep2 = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    let update_item =
        MenuItem::with_id(app, ID_CHECK_UPDATE, "检查更新...", true, None::<&str>)
            .map_err(|e| e.to_string())?;
    let quit_item =
        MenuItem::with_id(app, ID_QUIT, "退出", true, None::<&str>).map_err(|e| e.to_string())?;

    let menu = Menu::with_items(
        app,
        &[
            &toggle_item,
            &orb_item,
            &refresh_item,
            &sep1,
            &autostart_item,
            &sep2,
            &update_item,
            &quit_item,
        ],
    )
    .map_err(|e| e.to_string())?;

    app.manage(TrayState { autostart_item });

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "缺少托盘默认图标".to_string())?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("AuraQuota · AI 额度悬浮监控")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            match id {
                ID_TOGGLE => {
                    if let Some(win) = app.get_webview_window("main") {
                        let win_state = app.state::<WindowState>();
                        let current = win_state
                            .mode
                            .lock()
                            .map(|g| *g)
                            .unwrap_or(WindowMode::Panel);
                        let next = if current == WindowMode::Panel {
                            WindowMode::Edge
                        } else {
                            WindowMode::Panel
                        };
                        let _ = apply_window_mode(&win, &win_state, next);
                        let _ = win.set_focus();
                    }
                }
                ID_ORB => {
                    if let Some(win) = app.get_webview_window("main") {
                        let win_state = app.state::<WindowState>();
                        let current = win_state
                            .mode
                            .lock()
                            .map(|g| *g)
                            .unwrap_or(WindowMode::Panel);
                        let next = if current == WindowMode::Orb {
                            WindowMode::Panel
                        } else {
                            WindowMode::Orb
                        };
                        let _ = apply_window_mode(&win, &win_state, next);
                    }
                }
                ID_REFRESH => {
                    let handle = app.clone();
                    tauri::async_runtime::spawn(async move {
                        let store = handle.state::<Arc<AccountStore>>();
                        let registry = handle.state::<Arc<ProviderRegistry>>();
                        let bridge = handle.state::<Arc<WebviewBridgeStore>>();
                        let ctx = ProviderContext {
                            app: Some(handle.clone()),
                            bridge: Some(Arc::clone(&bridge)),
                            store: Some(Arc::clone(&store)),
                        };
                        if let Ok(_accounts) = registry.refresh_all(&store, &ctx).await {
                            let poll_secs = handle
                                .try_state::<Arc<crate::settings::SettingsStore>>()
                                .map(|s| s.get().polling_interval_secs)
                                .unwrap_or(15);
                            if let Ok(snap) = store.build_snapshot(poll_secs, now_hh_mm()) {
                                let _ = handle.emit("quota-updated", snap);
                            }
                        }
                    });
                }
                ID_AUTOSTART => {
                    let manager = app.autolaunch();
                    let currently_enabled = manager.is_enabled().unwrap_or(false);
                    let target = !currently_enabled;
                    let res = if target {
                        manager.enable()
                    } else {
                        manager.disable()
                    };
                    if res.is_ok() {
                        if let Some(tray_state) = app.try_state::<TrayState>() {
                            let _ = tray_state.autostart_item.set_checked(target);
                        }
                    }
                }
                ID_CHECK_UPDATE => {
                    if let Some(win) = app.get_webview_window("main") {
                        let win_state = app.state::<WindowState>();
                        let _ = apply_window_mode(&win, &win_state, WindowMode::Panel);
                        let _ = win.set_focus();
                        let _ = app.emit("open-settings-updates", ());
                    }
                }
                ID_QUIT => {
                    app.exit(0);
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(win) = app.get_webview_window("main") {
                    let win_state = app.state::<WindowState>();
                    let current = win_state
                        .mode
                        .lock()
                        .map(|g| *g)
                        .unwrap_or(WindowMode::Panel);
                    let next = if current == WindowMode::Panel {
                        WindowMode::Edge
                    } else {
                        WindowMode::Panel
                    };
                    let _ = apply_window_mode(&win, &win_state, next);
                    let _ = win.set_focus();
                }
            }
        })
        .build(app)
        .map_err(|e| format!("构建系统托盘失败: {e}"))?;

    Ok(())
}
