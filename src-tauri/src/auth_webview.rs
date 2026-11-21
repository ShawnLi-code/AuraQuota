use crate::models::{
    AccountStatus, AuthMethod, CapturedAuthResult, PlatformId, SecretCredentials,
    StoredAccountRecord, SubscriptionTier,
};
use crate::providers::parse_scraped_payload;
use crate::storage::AccountStore;
use crate::webview_scraper::{scrape_existing_webview, usage_url_for, WebviewBridgeStore};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use url::Url;

pub struct PendingAuthEntry {
    pub platform: PlatformId,
    pub profile_dir: PathBuf,
    pub creds: SecretCredentials,
    pub initial_scraped_payload: Option<Value>,
    pub detected_email: Option<String>,
    pub detected_plan_label: Option<String>,
}

#[derive(Default)]
pub struct PendingAuthStore {
    entries: Mutex<HashMap<String, PendingAuthEntry>>,
}

impl PendingAuthStore {
    pub fn insert(&self, session_id: String, entry: PendingAuthEntry) {
        if let Ok(mut guard) = self.entries.lock() {
            guard.insert(session_id, entry);
        }
    }

    pub fn take(&self, session_id: &str) -> Option<PendingAuthEntry> {
        self.entries
            .lock()
            .ok()
            .and_then(|mut guard| guard.remove(session_id))
    }
}

fn login_url_for(platform: PlatformId) -> &'static str {
    usage_url_for(platform)
}

pub fn default_browser_url_for(platform: PlatformId) -> &'static str {
    match platform {
        PlatformId::Gemini => "https://gemini.google.com/usage",
        PlatformId::Antigravity => "https://antigravity.google",
        PlatformId::Claude => "https://claude.ai/settings/usage",
        PlatformId::Codex => "https://chatgpt.com/codex/settings/usage",
        PlatformId::Commandcode => "https://commandcode.ai",
        PlatformId::Opencode => "https://opencode.ai/zen",
    }
}

fn cookie_probe_urls(platform: PlatformId) -> Vec<&'static str> {
    match platform {
        PlatformId::Gemini => {
            vec!["https://gemini.google.com", "https://accounts.google.com"]
        }
        PlatformId::Antigravity => vec!["https://antigravity.google", "https://accounts.google.com"],
        PlatformId::Claude => vec!["https://claude.ai"],
        PlatformId::Codex => vec!["https://chatgpt.com"],
        PlatformId::Commandcode => vec!["https://commandcode.ai", "https://api.commandcode.ai"],
        PlatformId::Opencode => vec!["https://opencode.ai"],
    }
}

pub fn open_auth_webview_window(
    app: &AppHandle,
    store: &AccountStore,
    bridge: &Arc<WebviewBridgeStore>,
    platform: PlatformId,
    account_id: Option<String>,
) -> Result<String, String> {
    let session_id = account_id.unwrap_or_else(|| {
        format!(
            "{:?}-{}",
            platform,
            &uuid::Uuid::new_v4().to_string()[..8]
        )
        .to_lowercase()
    });

    let label = format!("auth-{session_id}");
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(session_id);
    }

    let profile_dir = store.profile_dir_for(&session_id);
    std::fs::create_dir_all(&profile_dir)
        .map_err(|e| format!("创建隔离 WebView2 目录失败: {e}"))?;

    let target_url =
        Url::parse(login_url_for(platform)).map_err(|e| format!("无效登录 URL: {e}"))?;

    let bridge_clone = Arc::clone(bridge);
    WebviewWindowBuilder::new(app, label, WebviewUrl::External(target_url))
        .title(format!("AuraQuota · 登录 {}", platform.display_name()))
        .inner_size(920.0, 740.0)
        .resizable(true)
        .center()
        .data_directory(profile_dir)
        .on_document_title_changed(move |window, title| {
            bridge_clone.deliver_from_title(window.label(), &title);
        })
        .build()
        .map_err(|e| format!("打开内置登录窗失败: {e}"))?;

    Ok(session_id)
}

pub async fn capture_auth_webview_session(
    app: &AppHandle,
    store: &AccountStore,
    pending: &PendingAuthStore,
    bridge: &WebviewBridgeStore,
    platform: PlatformId,
    session_id: &str,
) -> Result<CapturedAuthResult, String> {
    let label = format!("auth-{session_id}");
    let win = app
        .get_webview_window(&label)
        .ok_or_else(|| "未找到打开的登录窗口，请先点击「打开官方登录窗」".to_string())?;

    // Scrape live DOM / same-origin API inside the already authenticated WebView2 before closing
    let scraped_payload = scrape_existing_webview(&win, bridge, platform, 5_500).await;

    let mut cookie_map: BTreeMap<String, String> = BTreeMap::new();
    for raw_url in cookie_probe_urls(platform) {
        if let Ok(url) = Url::parse(raw_url) {
            if let Ok(cookies) = win.cookies_for_url(url) {
                for ck in cookies {
                    cookie_map.insert(ck.name().to_string(), ck.value().to_string());
                }
            }
        }
    }

    let temp_rec = StoredAccountRecord {
        id: session_id.to_string(),
        platform,
        alias: "临时".to_string(),
        email: "account@local".to_string(),
        tier: SubscriptionTier::Paid,
        plan_label: "Pro".to_string(),
        auth_method: AuthMethod::WebviewSession,
        profile_dir: Some(store.profile_dir_for(session_id).to_string_lossy().to_string()),
        is_demo: false,
        encrypted_secret: None,
        cached_quota: None,
    };

    let parsed_preview = scraped_payload
        .as_ref()
        .and_then(|p| parse_scraped_payload(platform, &temp_rec, p));

    let scrape_ok = parsed_preview
        .as_ref()
        .is_some_and(|q| q.status == AccountStatus::Ok);

    if scraped_payload
        .as_ref()
        .and_then(|p| p.get("authRequired"))
        .and_then(|v| v.as_bool())
        == Some(true)
    {
        return Err("检测到当前页面仍在登录界面，请先在登录窗口完成登录后再点击提取".to_string());
    }

    if cookie_map.is_empty() && !scrape_ok {
        return Err("尚未检测到有效登录状态，请先在登录窗口完成账号登录后再点击提取".to_string());
    }

    let cookie_count = cookie_map.len();
    let cookie_header = if cookie_map.is_empty() {
        None
    } else {
        Some(
            cookie_map
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; "),
        )
    };

    let session_key = if platform == PlatformId::Claude {
        cookie_map.get("sessionKey").cloned()
    } else {
        None
    };

    if platform == PlatformId::Claude && session_key.is_none() && !scrape_ok {
        return Err(
            "未检测到 Claude sessionKey Cookie，请确认已完成登录并进入 Usage 页面后再提取"
                .to_string(),
        );
    }

    let detected_email = parsed_preview
        .as_ref()
        .map(|q| q.email.clone())
        .filter(|e| e != "account@local" && e.contains('@'));
    let detected_plan_label = parsed_preview.as_ref().map(|q| q.plan_label.clone());
    let detected_primary_pct = parsed_preview
        .as_ref()
        .filter(|q| q.status == AccountStatus::Ok)
        .map(|q| q.primary_pct);

    let profile_dir = store.profile_dir_for(session_id);
    let creds = SecretCredentials {
        session_key,
        cookie_header,
        ..Default::default()
    };

    pending.insert(
        session_id.to_string(),
        PendingAuthEntry {
            platform,
            profile_dir,
            creds,
            initial_scraped_payload: scraped_payload,
            detected_email: detected_email.clone(),
            detected_plan_label: detected_plan_label.clone(),
        },
    );

    let _ = win.close();

    Ok(CapturedAuthResult {
        session_id: session_id.to_string(),
        captured: true,
        cookie_count,
        detected_tier_hint: detected_plan_label.clone(),
        detected_email,
        detected_plan_label,
        detected_primary_pct,
    })
}
