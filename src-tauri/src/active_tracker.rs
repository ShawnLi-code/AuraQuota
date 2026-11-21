use crate::models::PlatformId;
use crate::providers::antigravity::AntigravityProvider;
use crate::providers::now_hh_mm;
use crate::storage::AccountStore;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

pub fn classify_foreground_platform(exe_name: &str, window_title: &str) -> Option<PlatformId> {
    let exe = exe_name.trim().to_lowercase();
    let title = window_title.trim().to_lowercase();

    if exe.is_empty() && title.is_empty() {
        return None;
    }

    // Never override the active target when clicking AuraQuota's own window
    if exe.contains("auraquota") || (exe.is_empty() && title == "auraquota") {
        return None;
    }

    if exe == "antigravity" || exe == "antigravity.exe" {
        return Some(PlatformId::Antigravity);
    }
    if exe.starts_with("codex") {
        return Some(PlatformId::Codex);
    }
    if exe.starts_with("cmdcode") || exe.starts_with("commandcode") {
        return Some(PlatformId::Commandcode);
    }
    if exe.starts_with("opencode") {
        return Some(PlatformId::Opencode);
    }

    let is_browser = [
        "chrome",
        "msedge",
        "firefox",
        "brave",
        "arc",
        "opera",
        "vivaldi",
        "iexplore",
    ]
    .iter()
    .any(|b| exe.contains(b));

    if exe.starts_with("claude") && !is_browser {
        return Some(PlatformId::Claude);
    }

    let is_terminal_or_editor = [
        "windowsterminal",
        "wt",
        "pwsh",
        "powershell",
        "cmd",
        "wezterm",
        "alacritty",
        "mintty",
        "conemu",
        "hyper",
        "code",
        "cursor",
        "windsurf",
    ]
    .iter()
    .any(|t| exe == *t || exe == format!("{t}.exe") || exe.starts_with(t));

    if is_terminal_or_editor {
        if title.contains("command code")
            || title.contains("commandcode")
            || title.contains("cmdcode")
        {
            return Some(PlatformId::Commandcode);
        }
        if title.contains("opencode") {
            return Some(PlatformId::Opencode);
        }
        if title.contains("codex") {
            return Some(PlatformId::Codex);
        }
        if title.contains("claude") {
            return Some(PlatformId::Claude);
        }
        if title.contains("antigravity") {
            return Some(PlatformId::Antigravity);
        }
        return None;
    }

    if is_browser {
        if title.contains("antigravity") {
            return Some(PlatformId::Antigravity);
        }
        if title.contains("gemini") {
            return Some(PlatformId::Gemini);
        }
        if title.contains("codex") {
            return Some(PlatformId::Codex);
        }
        if title.contains("claude") {
            return Some(PlatformId::Claude);
        }
        if title.contains("command code") || title.contains("commandcode") {
            return Some(PlatformId::Commandcode);
        }
        if title.contains("opencode") {
            return Some(PlatformId::Opencode);
        }
    }

    None
}

#[cfg(windows)]
fn read_foreground_exe_and_title() -> Option<(String, String)> {
    use std::path::Path;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    };

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }

        let title_len = GetWindowTextLengthW(hwnd);
        let title = if title_len > 0 {
            let mut buf = vec![0u16; (title_len as usize) + 2];
            let copied = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
            if copied > 0 {
                String::from_utf16_lossy(&buf[..copied as usize])
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let mut pid: u32 = 0;
        let _ = GetWindowThreadProcessId(hwnd, &mut pid);
        let mut exe_name = String::new();
        if pid != 0 {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if !handle.is_null() {
                let mut path_buf = vec![0u16; 1024];
                let mut size = path_buf.len() as u32;
                if QueryFullProcessImageNameW(handle, 0, path_buf.as_mut_ptr(), &mut size) != 0
                    && size > 0
                {
                    let full_path = String::from_utf16_lossy(&path_buf[..size as usize]);
                    if let Some(name) = Path::new(&full_path).file_name().and_then(|s| s.to_str())
                    {
                        exe_name = name.to_string();
                    }
                }
                let _ = CloseHandle(handle);
            }
        }

        Some((exe_name, title))
    }
}

#[cfg(not(windows))]
fn read_foreground_exe_and_title() -> Option<(String, String)> {
    None
}

pub fn pick_best_account_for_platform(
    store: &AccountStore,
    platform: PlatformId,
    agy_active_email: Option<&str>,
) -> Option<String> {
    let records = store.list_records().ok()?;
    let matching: Vec<_> = records
        .iter()
        .filter(|r| !r.is_demo && r.platform == platform)
        .collect();
    if matching.is_empty() {
        return None;
    }

    if platform == PlatformId::Antigravity {
        if let Some(em) = agy_active_email
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "local@antigravity")
        {
            if let Some(exact) = matching.iter().find(|r| r.email.eq_ignore_ascii_case(em)) {
                return Some(exact.id.clone());
            }
        }
    }

    let (_, current_active, _) = store.current_active_state();
    if let Some(cur_id) = current_active {
        if matching.iter().any(|r| r.id == cur_id) {
            return Some(cur_id);
        }
    }

    let last_map = store.last_active_timestamps();
    matching
        .into_iter()
        .max_by_key(|r| {
            let ok_score = r
                .cached_quota
                .as_ref()
                .is_some_and(|q| q.status == crate::models::AccountStatus::Ok)
                as i64;
            let ts = last_map.get(&r.id).copied().unwrap_or(0);
            (ok_score, ts)
        })
        .map(|r| r.id.clone())
}

fn antigravity_state_vscdb_mtime() -> Option<std::time::SystemTime> {
    #[cfg(windows)]
    {
        let appdata = std::env::var_os("APPDATA")?;
        let path = Path::new(&appdata)
            .join("Antigravity")
            .join("User")
            .join("globalStorage")
            .join("state.vscdb");
        return std::fs::metadata(path).ok()?.modified().ok();
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")?;
        let path = Path::new(&home)
            .join("Library")
            .join("Application Support")
            .join("Antigravity")
            .join("User")
            .join("globalStorage")
            .join("state.vscdb");
        return std::fs::metadata(path).ok()?.modified().ok();
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        None
    }
}

pub fn spawn_active_window_watcher(app: AppHandle, store: Arc<AccountStore>) {
    tauri::async_runtime::spawn(async move {
        let mut last_agy_probe = Instant::now() - Duration::from_secs(60);
        let mut last_vscdb_mtime: Option<std::time::SystemTime> = None;
        let mut cached_agy_email: Option<String> = None;
        let agy_provider = AntigravityProvider::new();

        loop {
            tokio::time::sleep(Duration::from_millis(2500)).await;

            let Some((exe, title)) = read_foreground_exe_and_title() else {
                continue;
            };
            let Some(platform) = classify_foreground_platform(&exe, &title) else {
                continue;
            };

            if platform == PlatformId::Antigravity
                && last_agy_probe.elapsed() >= Duration::from_secs(8)
            {
                let agy_count = store
                    .list_records()
                    .map(|list| {
                        list.iter()
                            .filter(|r| !r.is_demo && r.platform == PlatformId::Antigravity)
                            .count()
                    })
                    .unwrap_or(0);
                if agy_count > 1 {
                    last_agy_probe = Instant::now();
                    let mut probed_email: Option<String> = None;
                    if let Ok(records) = store.list_records() {
                        if let Some(sample) = records
                            .into_iter()
                            .find(|r| !r.is_demo && r.platform == PlatformId::Antigravity)
                        {
                            let mut probe = sample;
                            probe.email = "local@antigravity".to_string();
                            if let Ok(q) = agy_provider.fetch_from_local_server(&probe).await {
                                if !q.email.is_empty() && q.email != "local@antigravity" {
                                    probed_email = Some(q.email);
                                }
                            }
                        }
                    }

                    // Fallback when Antigravity runs in standalone mode without CLI --csrf_token
                    if probed_email.is_none() {
                        let cur_mtime = antigravity_state_vscdb_mtime();
                        if cur_mtime.is_some() && cur_mtime != last_vscdb_mtime {
                            last_vscdb_mtime = cur_mtime;
                            if let Ok(Ok(cands)) = tokio::task::spawn_blocking(
                                crate::local_discovery::discover_antigravity_offline_candidates,
                            )
                            .await
                            {
                                if let Some(first_email) = cands
                                    .into_iter()
                                    .map(|c| c.email)
                                    .find(|em| !em.is_empty() && em != "local@antigravity")
                                {
                                    probed_email = Some(first_email);
                                }
                            }
                        }
                    }

                    if let Some(em) = probed_email {
                        cached_agy_email = Some(em);
                    }
                }
            }

            let Some(target_id) =
                pick_best_account_for_platform(&store, platform, cached_agy_email.as_deref())
            else {
                continue;
            };

            let reason = format!("{} 使用中", platform.display_name());
            if let Ok(true) = store.set_active_account(&target_id, &reason, false) {
                let poll_secs = app
                    .try_state::<Arc<crate::settings::SettingsStore>>()
                    .map(|s| s.get().polling_interval_secs)
                    .unwrap_or(15);
                if let Ok(snap) = store.build_snapshot(poll_secs, now_hh_mm()) {
                    let _ = app.emit("quota-updated", snap);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_active_ai_platforms_from_foreground_app() {
        assert_eq!(
            classify_foreground_platform("Antigravity.exe", "AuraQuota Workspace"),
            Some(PlatformId::Antigravity)
        );
        assert_eq!(
            classify_foreground_platform("auraquota.exe", "AuraQuota"),
            None
        );
        assert_eq!(
            classify_foreground_platform("WindowsTerminal.exe", "codex - PowerShell"),
            Some(PlatformId::Codex)
        );
        assert_eq!(
            classify_foreground_platform("wt.exe", "Command Code · GOAT"),
            Some(PlatformId::Commandcode)
        );
        assert_eq!(
            classify_foreground_platform("msedge.exe", "Google Gemini - Microsoft Edge"),
            Some(PlatformId::Gemini)
        );
        assert_eq!(
            classify_foreground_platform("chrome.exe", "Claude - Google Chrome"),
            Some(PlatformId::Claude)
        );
    }
}
