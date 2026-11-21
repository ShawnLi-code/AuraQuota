use crate::models::{AccountQuota, AccountStatus, QuotaSnapshot};
use crate::providers::{now_hh_mm, ProviderContext, ProviderRegistry};
use crate::storage::AccountStore;
use crate::webview_scraper::WebviewBridgeStore;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;

const POLL_INTERVAL_SECS: u64 = 300;
const LOW_QUOTA_THRESHOLD: u8 = 20;

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
) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
        tokio::time::sleep(Duration::from_secs(2)).await;

        loop {
            interval.tick().await;

            let has_real_accounts = store
                .list_records()
                .map(|list| list.iter().any(|r| !r.is_demo))
                .unwrap_or(false);

            if !has_real_accounts {
                continue;
            }

            let ctx = ProviderContext {
                app: Some(app.clone()),
                bridge: Some(Arc::clone(&bridge)),
                store: Some(Arc::clone(&store)),
            };

            if let Ok(accounts) = registry.refresh_all(&store, &ctx).await {
                tracker.check_and_notify(&app, &accounts);
                let snap = QuotaSnapshot {
                    accounts,
                    last_updated_at: now_hh_mm(),
                    polling_interval_secs: POLL_INTERVAL_SECS,
                };
                let _ = app.emit("quota-updated", snap);
            }
        }
    });
}
