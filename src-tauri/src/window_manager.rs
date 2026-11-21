use crate::models::WindowMode;
use std::sync::Mutex;
use tauri::{Emitter, LogicalPosition, LogicalSize, Position, Size, WebviewWindow};

const PANEL_WIDTH: f64 = 396.0;
const PANEL_HEIGHT: f64 = 576.0;
const ORB_SIZE: f64 = 72.0;
const EDGE_WIDTH: f64 = 36.0;
const EDGE_HEIGHT: f64 = 110.0;

pub struct WindowState {
    pub mode: Mutex<WindowMode>,
    pub theme: Mutex<String>,
    pub last_orb_pos: Mutex<Option<(f64, f64)>>,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            mode: Mutex::new(WindowMode::Panel),
            theme: Mutex::new("dark".to_string()),
            last_orb_pos: Mutex::new(None),
        }
    }
}

#[derive(Clone, Copy)]
struct LogicalWorkArea {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

fn get_work_area(window: &WebviewWindow) -> LogicalWorkArea {
    if let Ok(Some(monitor)) = window
        .current_monitor()
        .and_then(|m| match m {
            Some(mon) => Ok(Some(mon)),
            None => window.primary_monitor(),
        })
    {
        let scale = monitor.scale_factor();
        let wa = monitor.work_area();
        let left = wa.position.x as f64 / scale;
        let top = wa.position.y as f64 / scale;
        let width = wa.size.width as f64 / scale;
        let height = wa.size.height as f64 / scale;
        return LogicalWorkArea {
            left,
            top,
            right: left + width,
            bottom: top + height,
        };
    }

    LogicalWorkArea {
        left: 0.0,
        top: 0.0,
        right: 1920.0,
        bottom: 1040.0,
    }
}

#[cfg(windows)]
fn set_dwm_corner_preference(window: &WebviewWindow, round: bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND,
    };

    if let Ok(handle) = window.window_handle() {
        if let RawWindowHandle::Win32(win32) = handle.as_raw() {
            let pref: i32 = if round { DWMWCP_ROUND } else { DWMWCP_DONOTROUND };
            unsafe {
                let _ = DwmSetWindowAttribute(
                    win32.hwnd.get() as _,
                    DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                    &pref as *const i32 as *const _,
                    std::mem::size_of::<i32>() as u32,
                );
            }
        }
    }
}

#[cfg(not(windows))]
fn set_dwm_corner_preference(_window: &WebviewWindow, _round: bool) {}

#[cfg(windows)]
fn apply_panel_vibrancy(window: &WebviewWindow, theme: &str) {
    use window_vibrancy::apply_acrylic;
    let tint = if theme == "light" {
        Some((244, 247, 252, 215))
    } else {
        Some((11, 14, 23, 210))
    };
    let _ = window.set_shadow(true);
    let _ = apply_acrylic(window, tint);
    set_dwm_corner_preference(window, true);
}

#[cfg(not(windows))]
fn apply_panel_vibrancy(_window: &WebviewWindow, _theme: &str) {}

#[cfg(windows)]
fn clear_panel_vibrancy(window: &WebviewWindow) {
    use window_vibrancy::clear_acrylic;
    let _ = window.set_shadow(false);
    let _ = clear_acrylic(window);
    set_dwm_corner_preference(window, false);
}

#[cfg(not(windows))]
fn clear_panel_vibrancy(_window: &WebviewWindow) {}

pub fn apply_window_mode(
    window: &WebviewWindow,
    state: &WindowState,
    mode: WindowMode,
) -> Result<(), String> {
    let area = get_work_area(window);
    let theme = state
        .theme
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| "dark".to_string());

    match mode {
        WindowMode::Panel => {
            let x = (area.right - PANEL_WIDTH - 16.0).max(area.left + 8.0);
            let y = area.top + 16.0;
            let _ = window.set_size(Size::Logical(LogicalSize::new(PANEL_WIDTH, PANEL_HEIGHT)));
            let _ = window.set_position(Position::Logical(LogicalPosition::new(x, y)));
            apply_panel_vibrancy(window, &theme);
        }
        WindowMode::Orb => {
            clear_panel_vibrancy(window);
            let saved = state.last_orb_pos.lock().ok().and_then(|g| *g);
            let (raw_x, raw_y) =
                saved.unwrap_or((area.right - ORB_SIZE - 20.0, area.top + 24.0));
            let x = raw_x.clamp(area.left + 8.0, (area.right - ORB_SIZE - 8.0).max(area.left + 8.0));
            let y = raw_y.clamp(area.top + 8.0, (area.bottom - ORB_SIZE - 8.0).max(area.top + 8.0));
            let _ = window.set_size(Size::Logical(LogicalSize::new(ORB_SIZE, ORB_SIZE)));
            let _ = window.set_position(Position::Logical(LogicalPosition::new(x, y)));
        }
        WindowMode::Edge => {
            clear_panel_vibrancy(window);
            let x = (area.right - EDGE_WIDTH).max(area.left);
            let y = area.top + 96.0;
            let _ = window.set_size(Size::Logical(LogicalSize::new(EDGE_WIDTH, EDGE_HEIGHT)));
            let _ = window.set_position(Position::Logical(LogicalPosition::new(x, y)));
        }
    }

    if let Ok(mut guard) = state.mode.lock() {
        *guard = mode;
    }

    let _ = window.show();
    let _ = window.emit("window-mode-changed", mode);
    Ok(())
}

pub fn snap_orb_to_edge(window: &WebviewWindow, state: &WindowState) -> Result<(f64, f64), String> {
    let area = get_work_area(window);
    let scale = window.scale_factor().unwrap_or(1.0);
    let pos = window
        .outer_position()
        .map_err(|e| format!("读取窗口位置失败: {e}"))?;

    let cur_x = pos.x as f64 / scale;
    let cur_y = pos.y as f64 / scale;
    let mid_x = area.left + (area.right - area.left) / 2.0;

    let snap_x = if cur_x + ORB_SIZE / 2.0 < mid_x {
        area.left + 10.0
    } else {
        (area.right - ORB_SIZE - 10.0).max(area.left + 10.0)
    };
    let snap_y = cur_y.clamp(
        area.top + 10.0,
        (area.bottom - ORB_SIZE - 10.0).max(area.top + 10.0),
    );

    let _ = window.set_position(Position::Logical(LogicalPosition::new(snap_x, snap_y)));
    if let Ok(mut guard) = state.last_orb_pos.lock() {
        *guard = Some((snap_x, snap_y));
    }
    Ok((snap_x, snap_y))
}

pub fn set_theme_vibrancy(window: &WebviewWindow, state: &WindowState, theme: &str) {
    if let Ok(mut guard) = state.theme.lock() {
        *guard = theme.to_string();
    }
    let current_mode = state
        .mode
        .lock()
        .map(|g| *g)
        .unwrap_or(WindowMode::Panel);
    if current_mode == WindowMode::Panel {
        apply_panel_vibrancy(window, theme);
    }
}
