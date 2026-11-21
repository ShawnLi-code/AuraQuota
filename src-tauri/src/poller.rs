use crate::models::{AccountQuota, AccountStatus, AuthMethod, PlatformId};
use crate::providers::{now_hh_mm, ProviderContext, ProviderRegistry};
use crate::settings::SettingsStore;
use crate::storage::AccountStore;
use crate::webview_scraper::WebviewBridgeStore;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;
use tokio::task::JoinSet;

const POLL_TICK_SECS: u64 = 5;
const LOW_QUOTA_THRESHOLD: u8 = 20;

/// Computes per-account background polling cooldown based on active focus, auth transport, and last status.
pub fn compute_account_cooldown_secs(
    base_secs: u64,
    is_focused: bool,
    platform: PlatformId,
    auth_method: AuthMethod,
    last_status: Option<AccountStatus>,
) -> u64 {
    let base = base_secs.clamp(5, 3600);

    // Avoid hammering already-expired credentials when the user isn't actively triggering refresh
    if matches!(last_status, Some(AccountStatus::AuthRequired)) {
        return base.max(300);
    }

    // Hidden WebView2 scraping is heavier and more sensitive to burst traffic
    if auth_method == AuthMethod::WebviewSession {
        return if is_focused {
            base.max(60)
        } else {
            base.max(300)
        };
    }

    // Antigravity local language_server uses localhost RPC with virtually zero cloud rate-limit cost
    let is_local_agy = platform == PlatformId::Antigravity
        && matches!(auth_method, AuthMethod::LocalServer | AuthMethod::LocalCli);
    if is_local_agy {
        if matches!(last_status, Some(AccountStatus::Offline)) && !is_focused {
            return base.max(180);
        }
        return if is_focused { base } else { base.max(120) };
    }

    // Cloud API / OAuth endpoints allow rapid focused updates while sparing background slots
    if is_focused {
        base.max(10)
    } else {
        base.max(180)
    }
}

#[derive(Default)]
pub struct AlertTracker {
    notified_ids: Mutex<HashSet<String>>,
}

impl AlertTracker {
    pub fn check_and_notify(&self, app: &AppHandle, accounts: &[AccountQuota]) {
        let Ok(mut guard) = self.notified_ids.lock() else {
            return;
        };

        for acc in accounts {
            if acc.is_demo || acc.status != AccountStatus::Ok {
                continue;
            }

            if acc.primary_pct <= LOW_QUOTA_THRESHOLD {
                if guard.insert(acc.id.clone()) {
                    let title = "AuraQuota · 额度偏低提醒";
                    let body = format!(
                        "{} · {} 剩余 {}%（{}）",
                        acc.platform.display_name(),
                        acc.alias,
                        acc.primary_pct,
                        acc.reset_in
                    );
                    let _ = app.notification().builder().title(title).body(&body).show();
                }
            } else if acc.primary_pct > 25 {
                guard.remove(&acc.id);
            }
        }
    }
}

pub fn spawn_background_poller(
    app: AppHandle,
    store: Arc<AccountStore>,
    registry: Arc<ProviderRegistry>,
    bridge: Arc<WebviewBridgeStore>,
    tracker: Arc<AlertTracker>,
    settings: Arc<SettingsStore>,
) {
    tauri::async_runtime::spawn(async move {
        let mut last_polled: HashMap<String, Instant> = HashMap::new();
        tokio::time::sleep(Duration::from_secs(3)).await;

        loop {
            let base_secs = settings.get().polling_interval_secs.clamp(5, 3600);
            let records = store.list_records().unwrap_or_default();
            let real_records: Vec<_> = records.into_iter().filter(|r| !r.is_demo).collect();

            if real_records.is_empty() {
                last_polled.clear();
                tokio::time::sleep(Duration::from_secs(POLL_TICK_SECS)).await;
                continue;
            }

            let (pinned_id, active_id, _) = store.current_active_state();
            let focus_id = pinned_id.or(active_id);

            let live_ids: HashSet<String> = real_records.iter().map(|r| r.id.clone()).collect();
            last_polled.retain(|id, _| live_ids.contains(id));

            let now = Instant::now();
            let mut due_records = Vec::new();
            for rec in real_records {
                let is_focused = focus_id.as_deref() == Some(rec.id.as_str());
                let last_status = rec.cached_quota.as_ref().map(|q| q.status);
                let cooldown = compute_account_cooldown_secs(
                    base_secs,
                    is_focused,
                    rec.platform,
                    rec.auth_method,
                    last_status,
                );
                let is_due = match last_polled.get(&rec.id) {
                    Some(prev) => now.saturating_duration_since(*prev).as_secs() >= cooldown,
                    None => true,
                };
                if is_due {
                    due_records.push(rec);
                }
            }

            if !due_records.is_empty() {
                let ctx = ProviderContext {
                    app: Some(app.clone()),
                    bridge: Some(Arc::clone(&bridge)),
                    store: Some(Arc::clone(&store)),
                };
                let mut set = JoinSet::new();
                for rec in due_records {
                    let rec_id = rec.id.clone();
                    let reg = Arc::clone(&registry);
                    let st = Arc::clone(&store);
                    let c = ctx.clone();
                    set.spawn(async move {
                        let _ = reg.refresh_single(&st, &rec, &c).await;
                        rec_id
                    });
                }
                while let Some(res) = set.join_next().await {
                    if let Ok(rec_id) = res {
                        last_polled.insert(rec_id, Instant::now());
                    }
                }

                if let Ok(all) = store.list_quotas() {
                    tracker.check_and_notify(&app, &all);
                }
                if let Ok(snap) = store.build_snapshot(base_secs, now_hh_mm()) {
                    let _ = app.emit("quota-updated", snap);
                }
            }

            tokio::time::sleep(Duration::from_secs(POLL_TICK_SECS)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_cooldown_prioritizes_focused_local_and_protects_heavy_or_expired() {
        // 5s setting: focused Antigravity local RPC gets full 5s cadence
        assert_eq!(
            compute_account_cooldown_secs(
                5,
                true,
                PlatformId::Antigravity,
                AuthMethod::LocalServer,
                Some(AccountStatus::Ok)
            ),
            5
        );

        // Focused cloud API respects minimum 10s guard even on 5s setting
        assert_eq!(
            compute_account_cooldown_secs(
                5,
                true,
                PlatformId::Codex,
                AuthMethod::OauthToken,
                Some(AccountStatus::Ok)
            ),
            10
        );

        // Focused heavy WebView2 stays throttled >= 60s
        assert_eq!(
            compute_account_cooldown_secs(
                5,
                true,
                PlatformId::Gemini,
                AuthMethod::WebviewSession,
                Some(AccountStatus::Ok)
            ),
            60
        );

        // Unfocused cloud accounts slow down to >= 180s
        assert_eq!(
            compute_account_cooldown_secs(
                15,
                false,
                PlatformId::Commandcode,
                AuthMethod::ApiKey,
                Some(AccountStatus::Ok)
            ),
            180
        );

        // Expired accounts stop spamming regardless of focus
        assert_eq!(
            compute_account_cooldown_secs(
                5,
                true,
                PlatformId::Antigravity,
                AuthMethod::OauthToken,
                Some(AccountStatus::AuthRequired)
            ),
            300
        );
    }
}
