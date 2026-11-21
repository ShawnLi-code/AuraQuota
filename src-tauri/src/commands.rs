use crate::auth_webview::{
    capture_auth_webview_session, default_browser_url_for, open_auth_webview_window,
    PendingAuthStore,
};
use crate::local_discovery::{
    discover_antigravity_offline_candidates, discover_claude_code_local, discover_codex_cli_local,
    discover_commandcode_cli_local, discover_opencode_cli_local, LocalDiscoveredCredential,
};
use crate::models::{
    AccountQuota, AccountStatus, AddAccountRequest, AuthMethod, CapturedAuthResult,
    ClaudeOAuthStartResult, DetectLocalResult, LocalScanItem, PlatformId, QuotaSnapshot,
    ScanLocalSummary, SecretCredentials, StoredAccountRecord, SubscriptionTier, WindowMode,
};
use crate::oauth_browser::{
    complete_claude_browser_oauth as finish_claude_oauth,
    run_codex_browser_oauth as run_codex_oauth,
    start_claude_browser_oauth as begin_claude_oauth, BrowserOAuthAccountData, PendingOAuthStore,
};
use crate::poller::AlertTracker;
use crate::providers::antigravity::{discover_local_language_servers, AntigravityProvider};
use crate::providers::{now_hh_mm, parse_scraped_payload, ProviderContext, ProviderRegistry};
use crate::settings::{AppSettings, SettingsStore, DEFAULT_UPDATE_FEED_URL};
use crate::storage::AccountStore;
use crate::tray::TrayState;
use crate::updater::{
    check_for_updates as run_update_check, download_and_install_update, UpdateCheckResult,
};
use crate::webview_scraper::WebviewBridgeStore;
use crate::window_manager::{
    apply_window_mode, set_theme_vibrancy as apply_theme_vibrancy,
    snap_orb_to_edge as do_snap_orb, WindowState,
};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_opener::OpenerExt;

fn build_ctx(
    app: &AppHandle,
    bridge: &Arc<WebviewBridgeStore>,
    store: &Arc<AccountStore>,
) -> ProviderContext {
    ProviderContext {
        app: Some(app.clone()),
        bridge: Some(Arc::clone(bridge)),
        store: Some(Arc::clone(store)),
    }
}

fn emit_snapshot(app: &AppHandle, store: &AccountStore) {
    if let Ok(snap) = store.build_snapshot(300, now_hh_mm()) {
        let _ = app.emit("quota-updated", snap);
    }
}

fn format_discovered_message(label: &str, quota: &AccountQuota) -> String {
    if quota.status == AccountStatus::Ok {
        format!("已同步{label}（剩余 {}%）", quota.primary_pct)
    } else {
        quota
            .status_message
            .clone()
            .unwrap_or_else(|| format!("检测到{label}，但当前凭据已失效（未导入空白账号）"))
    }
}

fn default_plan_for(platform: PlatformId) -> String {
    match platform {
        PlatformId::Antigravity | PlatformId::Gemini => "AI Pro".to_string(),
        PlatformId::Codex => "Plus".to_string(),
        PlatformId::Claude => "Pro".to_string(),
        PlatformId::Commandcode => "GOAT".to_string(),
        PlatformId::Opencode => "Go".to_string(),
    }
}

async fn save_and_refresh_discovered(
    app: &AppHandle,
    store: &Arc<AccountStore>,
    registry: &ProviderRegistry,
    bridge: &Arc<WebviewBridgeStore>,
    discovered: LocalDiscoveredCredential,
    explicit_id: &str,
    alias_override: Option<String>,
) -> Result<AccountQuota, String> {
    let ctx = build_ctx(app, bridge, store);
    let existing = store.get_record(explicit_id)?;

    let req = AddAccountRequest {
        platform: discovered.platform,
        alias: alias_override.unwrap_or_else(|| {
            existing
                .as_ref()
                .map(|r| r.alias.clone())
                .unwrap_or(discovered.alias)
        }),
        email: discovered.email.clone(),
        tier: discovered.tier,
        plan_label: discovered.plan_label.clone(),
        auth_method: discovered.auth_method,
        secret_input: None,
        refresh_token: None,
        profile_id: None,
    };

    // 若库中尚未保存该本机客户端账号，先验证探测是否可用；仅当可用时才落盘入库，避免无效残留文件生成「需重登」废卡
    if existing.is_none() {
        let probe_rec = StoredAccountRecord {
            id: explicit_id.to_string(),
            platform: discovered.platform,
            alias: req.alias.clone(),
            email: req.email.clone(),
            tier: req.tier,
            plan_label: req.plan_label.clone(),
            auth_method: req.auth_method,
            profile_dir: None,
            is_demo: false,
            encrypted_secret: None,
            cached_quota: None,
        };
        if let Some(prov) = registry.provider_for(discovered.platform) {
            let quota = prov
                .fetch_quota(&probe_rec, Some(&discovered.creds), &ctx)
                .await;
            if quota.status != AccountStatus::Ok {
                return Ok(quota);
            }
            let _ = store.add_account(
                &req,
                Some(&discovered.creds),
                Some(explicit_id.to_string()),
                None,
                Some(quota.clone()),
            )?;
            let _ = store.update_cached_quota(&quota);
            emit_snapshot(app, store);
            return Ok(quota);
        }
    }

    let rec = store.add_account(
        &req,
        Some(&discovered.creds),
        Some(explicit_id.to_string()),
        existing.and_then(|r| r.profile_dir),
        None,
    )?;
    let quota = registry.refresh_single(store, &rec, &ctx).await;
    emit_snapshot(app, store);
    Ok(quota)
}

async fn save_oauth_browser_account(
    app: &AppHandle,
    store: &Arc<AccountStore>,
    registry: &ProviderRegistry,
    bridge: &Arc<WebviewBridgeStore>,
    platform: PlatformId,
    data: BrowserOAuthAccountData,
    alias: Option<String>,
    plan_label: Option<String>,
    tier: Option<SubscriptionTier>,
    rebind_account_id: Option<String>,
) -> Result<AccountQuota, String> {
    let ctx = build_ctx(app, bridge, store);

    if let Some(target_id) = rebind_account_id.as_deref().filter(|s| !s.is_empty()) {
        if let Some(existing) = store.get_record(target_id)? {
            let final_alias = alias
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&existing.alias)
                .to_string();
            let final_email = match data.email {
                Some(ref em)
                    if existing.email.is_empty()
                        || existing.email.ends_with(".local")
                        || existing.email == "local@antigravity"
                        || existing.email == "account@local" =>
                {
                    em.clone()
                }
                _ => existing.email.clone(),
            };
            let req = AddAccountRequest {
                platform,
                alias: final_alias,
                email: final_email,
                tier: data.tier.or(tier).unwrap_or(existing.tier),
                plan_label: data
                    .plan_label
                    .or(plan_label)
                    .unwrap_or_else(|| existing.plan_label.clone()),
                auth_method: AuthMethod::OauthToken,
                secret_input: None,
                refresh_token: None,
                profile_id: None,
            };
            let rec = store.add_account(
                &req,
                Some(&data.creds),
                Some(target_id.to_string()),
                existing.profile_dir,
                None,
            )?;
            let quota = registry.refresh_single(store, &rec, &ctx).await;
            emit_snapshot(app, store);
            return Ok(quota);
        }
    }

    let req = AddAccountRequest {
        platform,
        alias: alias
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{} 账号", platform.display_name())),
        email: data.email.unwrap_or_else(|| "account@local".to_string()),
        tier: data.tier.or(tier).unwrap_or(SubscriptionTier::Paid),
        plan_label: data
            .plan_label
            .or(plan_label)
            .unwrap_or_else(|| default_plan_for(platform)),
        auth_method: AuthMethod::OauthToken,
        secret_input: None,
        refresh_token: None,
        profile_id: None,
    };

    let rec = store.add_account(&req, Some(&data.creds), None, None, None)?;
    let quota = registry.refresh_single(store, &rec, &ctx).await;
    emit_snapshot(app, store);
    Ok(quota)
}

#[tauri::command]
pub async fn get_quota_snapshot(
    store: State<'_, Arc<AccountStore>>,
) -> Result<QuotaSnapshot, String> {
    store.build_snapshot(300, now_hh_mm())
}

#[tauri::command]
pub async fn refresh_all_quotas(
    app: AppHandle,
    store: State<'_, Arc<AccountStore>>,
    registry: State<'_, Arc<ProviderRegistry>>,
    bridge: State<'_, Arc<WebviewBridgeStore>>,
    tracker: State<'_, Arc<AlertTracker>>,
) -> Result<QuotaSnapshot, String> {
    let ctx = build_ctx(&app, &bridge, &store);
    let accounts = registry.refresh_all(&store, &ctx).await?;
    tracker.check_and_notify(&app, &accounts);
    let snap = store.build_snapshot(300, now_hh_mm())?;
    let _ = app.emit("quota-updated", snap.clone());
    Ok(snap)
}

#[tauri::command]
pub async fn refresh_single_account(
    app: AppHandle,
    account_id: String,
    store: State<'_, Arc<AccountStore>>,
    registry: State<'_, Arc<ProviderRegistry>>,
    bridge: State<'_, Arc<WebviewBridgeStore>>,
    tracker: State<'_, Arc<AlertTracker>>,
) -> Result<AccountQuota, String> {
    let record = store
        .get_record(&account_id)?
        .ok_or_else(|| format!("未找到账号 {account_id}"))?;
    let ctx = build_ctx(&app, &bridge, &store);
    let quota = registry.refresh_single(&store, &record, &ctx).await;
    if let Ok(all) = store.list_quotas() {
        tracker.check_and_notify(&app, &all);
    }
    emit_snapshot(&app, &store);
    Ok(quota)
}

#[tauri::command]
pub async fn toggle_pin_account(
    app: AppHandle,
    account_id: String,
    store: State<'_, Arc<AccountStore>>,
) -> Result<QuotaSnapshot, String> {
    store.toggle_pin_account(&account_id)?;
    let snap = store.build_snapshot(300, now_hh_mm())?;
    let _ = app.emit("quota-updated", snap.clone());
    Ok(snap)
}

#[tauri::command]
pub async fn set_active_account(
    app: AppHandle,
    account_id: String,
    store: State<'_, Arc<AccountStore>>,
) -> Result<QuotaSnapshot, String> {
    let _ = store.set_active_account(&account_id, "当前选中焦点", true)?;
    let snap = store.build_snapshot(300, now_hh_mm())?;
    let _ = app.emit("quota-updated", snap.clone());
    Ok(snap)
}

#[tauri::command]
pub async fn add_account(
    app: AppHandle,
    mut req: AddAccountRequest,
    store: State<'_, Arc<AccountStore>>,
    registry: State<'_, Arc<ProviderRegistry>>,
    pending: State<'_, Arc<PendingAuthStore>>,
    bridge: State<'_, Arc<WebviewBridgeStore>>,
) -> Result<AccountQuota, String> {
    let mut explicit_id: Option<String> = None;
    let mut profile_dir: Option<String> = None;
    let mut creds = SecretCredentials::default();
    let mut pre_scraped_payload = None;

    if let Some(session_id) = req.profile_id.as_deref().filter(|s| !s.is_empty()) {
        if let Some(captured) = pending.take(session_id) {
            creds = captured.creds;
            explicit_id = Some(session_id.to_string());
            profile_dir = Some(captured.profile_dir.to_string_lossy().to_string());
            pre_scraped_payload = captured.initial_scraped_payload;
            if (req.email.trim().is_empty() || req.email.trim() == "account@local")
                && captured.detected_email.is_some()
            {
                req.email = captured.detected_email.unwrap();
            }
        } else {
            let dir = store.profile_dir_for(session_id);
            profile_dir = Some(dir.to_string_lossy().to_string());
            explicit_id = Some(session_id.to_string());
        }
    }

    if let Some(raw_secret) = req.secret_input.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        match req.auth_method {
            AuthMethod::ApiKey => {
                creds.api_key = Some(raw_secret.to_string());
            }
            AuthMethod::OauthToken => {
                creds.access_token = Some(raw_secret.to_string());
                creds.refresh_token = req.refresh_token.clone();
            }
            AuthMethod::Cookie => {
                if req.platform == PlatformId::Claude && !raw_secret.contains('=') {
                    creds.session_key = Some(raw_secret.to_string());
                } else {
                    creds.cookie_header = Some(raw_secret.to_string());
                }
            }
            _ => {
                creds.cookie_header = Some(raw_secret.to_string());
            }
        }
    }

    let record = store.add_account(&req, Some(&creds), explicit_id, profile_dir, None)?;

    let pre_parsed = pre_scraped_payload
        .as_ref()
        .and_then(|p| parse_scraped_payload(req.platform, &record, p))
        .filter(|q| q.status == AccountStatus::Ok);

    let quota = if let Some(q) = pre_parsed {
        let _ = store.update_cached_quota(&q);
        q
    } else {
        let ctx = build_ctx(&app, &bridge, &store);
        registry.refresh_single(&store, &record, &ctx).await
    };

    emit_snapshot(&app, &store);

    Ok(quota)
}

#[tauri::command]
pub async fn remove_account(
    app: AppHandle,
    account_id: String,
    store: State<'_, Arc<AccountStore>>,
) -> Result<bool, String> {
    let removed = store.remove_account(&account_id)?;
    if removed {
        emit_snapshot(&app, &store);
    }
    Ok(removed)
}

#[tauri::command]
pub async fn clear_demo_accounts(
    store: State<'_, Arc<AccountStore>>,
) -> Result<QuotaSnapshot, String> {
    store.clear_demo_accounts()?;
    store.build_snapshot(300, now_hh_mm())
}

#[tauri::command]
pub async fn restore_demo_accounts(
    store: State<'_, Arc<AccountStore>>,
) -> Result<QuotaSnapshot, String> {
    store.restore_demo_accounts()?;
    store.build_snapshot(300, now_hh_mm())
}

fn agy_account_id_for_email(email: &str) -> String {
    let clean = email.trim().to_lowercase();
    if clean.is_empty() || clean == "local@antigravity" || clean == "account@local" {
        return "agy-local".to_string();
    }
    let slug: String = clean
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("agy-local-{slug}")
}

fn resolve_agy_target_id_and_alias(
    store: &AccountStore,
    detected_email: &str,
    alias_override: Option<&str>,
) -> (String, String) {
    let clean = detected_email.trim();
    let has_real_email =
        !clean.is_empty() && clean != "local@antigravity" && clean != "account@local";

    let existing_records = store.list_records().unwrap_or_default();
    let matched_existing = if has_real_email {
        existing_records.iter().find(|r| {
            r.platform == PlatformId::Antigravity && r.email.trim().eq_ignore_ascii_case(clean)
        })
    } else {
        existing_records
            .iter()
            .find(|r| r.platform == PlatformId::Antigravity && r.id == "agy-local")
    };

    let id = matched_existing
        .map(|r| r.id.clone())
        .unwrap_or_else(|| agy_account_id_for_email(clean));

    let strip_agy_platform_prefix = |raw: &str| -> String {
        let mut s = raw.trim();
        loop {
            if let Some(rest) = s.strip_prefix("反重力 · ") {
                s = rest.trim();
            } else if let Some(rest) = s.strip_prefix("反重力·") {
                s = rest.trim();
            } else {
                break;
            }
        }
        if s.is_empty() {
            "本机".to_string()
        } else {
            s.to_string()
        }
    };

    let alias = alias_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(strip_agy_platform_prefix)
        .or_else(|| matched_existing.map(|r| strip_agy_platform_prefix(&r.alias)))
        .unwrap_or_else(|| {
            if has_real_email {
                let prefix = clean.split('@').next().unwrap_or("本机").trim();
                if prefix.is_empty() {
                    "本机".to_string()
                } else {
                    prefix.to_string()
                }
            } else {
                "本机".to_string()
            }
        });

    if has_real_email && id != "agy-local" {
        if let Some(legacy) = existing_records.iter().find(|r| r.id == "agy-local") {
            let legacy_email = legacy.email.trim();
            if legacy_email.is_empty()
                || legacy_email == "local@antigravity"
                || legacy_email == "account@local"
            {
                let _ = store.remove_account("agy-local");
            }
        }
    }

    (id, alias)
}

async fn detect_antigravity_local_inner(
    app: &AppHandle,
    alias: Option<String>,
    persist_expired: bool,
    store: &Arc<AccountStore>,
    registry: &ProviderRegistry,
    bridge: &Arc<WebviewBridgeStore>,
) -> Result<DetectLocalResult, String> {
    let alias_override = alias
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let ctx = build_ctx(app, bridge, store);
    let agy_provider = registry
        .provider_for(PlatformId::Antigravity)
        .ok_or_else(|| "未找到反重力适配器".to_string())?;

    let _ = store.remove_account("agy-local-server");
    let _ = store.remove_account("agy-local-offline");

    let (offline_candidates, offline_err) =
        match tokio::task::spawn_blocking(discover_antigravity_offline_candidates)
            .await
            .map_err(|e| e.to_string())?
        {
            Ok(list) => (list, None),
            Err(msg) => (Vec::new(), Some(msg)),
        };

    // 1. Probe running Antigravity language_server directly first
    if let Ok(Ok(servers)) = tokio::task::spawn_blocking(discover_local_language_servers).await {
        let first = &servers[0];
        let probe_rec = StoredAccountRecord {
            id: "agy-local-probe".to_string(),
            platform: PlatformId::Antigravity,
            alias: "本机反重力".to_string(),
            email: "local@antigravity".to_string(),
            tier: SubscriptionTier::Paid,
            plan_label: "AI Pro".to_string(),
            auth_method: AuthMethod::LocalServer,
            profile_dir: None,
            is_demo: false,
            encrypted_secret: None,
            cached_quota: None,
        };
        let direct = AntigravityProvider::new();
        if let Ok(mut quota) = direct.fetch_from_local_server(&probe_rec).await {
            let mut matching_cand = offline_candidates
                .iter()
                .find(|c| {
                    c.email != "local@antigravity"
                        && quota.email != "local@antigravity"
                        && c.email.eq_ignore_ascii_case(&quota.email)
                })
                .map(|c| c.creds.clone());

            let http = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(4))
                .build()
                .ok();
            let mut resolved_candidates = Vec::with_capacity(offline_candidates.len());

            for c in &offline_candidates {
                let mut eff_email = c.email.clone();
                if eff_email.is_empty() || eff_email == "local@antigravity" {
                    if let (Some(client), Some(tok)) = (&http, c.creds.access_token.as_deref()) {
                        if let Some(real_email) =
                            crate::providers::antigravity::fetch_google_userinfo_email(client, tok)
                                .await
                        {
                            eff_email = real_email;
                        }
                    }
                }
                if matching_cand.is_none()
                    && quota.email != "local@antigravity"
                    && eff_email != "local@antigravity"
                    && eff_email.eq_ignore_ascii_case(&quota.email)
                {
                    matching_cand = Some(c.creds.clone());
                }
                resolved_candidates.push((eff_email, c));
            }

            if (quota.email.is_empty() || quota.email == "local@antigravity")
                && !resolved_candidates.is_empty()
            {
                if let Some((eff_email, c)) = resolved_candidates
                    .iter()
                    .find(|(e, _)| !e.is_empty() && e != "local@antigravity")
                {
                    quota.email = eff_email.clone();
                    if matching_cand.is_none() {
                        matching_cand = Some(c.creds.clone());
                    }
                } else if resolved_candidates.len() == 1 {
                    quota.email = resolved_candidates[0].0.clone();
                    if matching_cand.is_none() {
                        matching_cand = Some(resolved_candidates[0].1.creds.clone());
                    }
                }
            }

            let (target_id, target_alias) =
                resolve_agy_target_id_and_alias(store, &quota.email, alias_override.as_deref());
            quota.id = target_id.clone();
            quota.alias = target_alias.clone();

            let req = AddAccountRequest {
                platform: PlatformId::Antigravity,
                alias: target_alias,
                email: quota.email.clone(),
                tier: quota.tier,
                plan_label: quota.plan_label.clone(),
                auth_method: AuthMethod::LocalServer,
                secret_input: None,
                refresh_token: None,
                profile_id: None,
            };
            let _ = store.add_account(
                &req,
                matching_cand.as_ref(),
                Some(target_id),
                None,
                Some(quota.clone()),
            )?;
            let _ = store.update_cached_quota(&quota);

            // 若离线缓存/凭据管理器中还存有其他独立反重力账号的可用 OAuth 凭据，同步补全或刷新，避免多号切换时丢掉旧号 Token
            if alias_override.is_none() {
                let existing_agy = store
                    .list_records()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|r| r.platform == PlatformId::Antigravity)
                    .collect::<Vec<_>>();
                for (eff_email, cand) in resolved_candidates {
                    if eff_email.is_empty()
                        || eff_email == "local@antigravity"
                        || eff_email.eq_ignore_ascii_case(&quota.email)
                    {
                        continue;
                    }
                    let already_saved = existing_agy
                        .iter()
                        .any(|r| r.email.eq_ignore_ascii_case(&eff_email));
                    if !already_saved && !persist_expired {
                        continue;
                    }
                    let extra_probe = StoredAccountRecord {
                        id: "agy-extra-probe".to_string(),
                        platform: PlatformId::Antigravity,
                        alias: cand.alias.clone(),
                        email: eff_email.clone(),
                        tier: cand.tier,
                        plan_label: cand.plan_label.clone(),
                        auth_method: AuthMethod::OauthToken,
                        profile_dir: None,
                        is_demo: false,
                        encrypted_secret: None,
                        cached_quota: None,
                    };
                    let mut extra_q = agy_provider
                        .fetch_quota(&extra_probe, Some(&cand.creds), &ctx)
                        .await;
                    if extra_q.status == AccountStatus::Ok {
                        let extra_email = if extra_q.email.is_empty()
                            || extra_q.email == "local@antigravity"
                        {
                            eff_email.clone()
                        } else {
                            extra_q.email.clone()
                        };
                        extra_q.email = extra_email.clone();
                        let (eid, ealias) =
                            resolve_agy_target_id_and_alias(store, &extra_email, None);
                        extra_q.id = eid.clone();
                        extra_q.alias = ealias.clone();
                        let ereq = AddAccountRequest {
                            platform: PlatformId::Antigravity,
                            alias: ealias,
                            email: extra_email,
                            tier: extra_q.tier,
                            plan_label: extra_q.plan_label.clone(),
                            auth_method: AuthMethod::OauthToken,
                            secret_input: None,
                            refresh_token: None,
                            profile_id: None,
                        };
                        if store
                            .add_account(
                                &ereq,
                                Some(&cand.creds),
                                Some(eid),
                                None,
                                Some(extra_q.clone()),
                            )
                            .is_ok()
                        {
                            let _ = store.update_cached_quota(&extra_q);
                        }
                    }
                }
            }

            emit_snapshot(app, store);
            return Ok(DetectLocalResult {
                found: true,
                message: format!(
                    "已连接运行中的 {} (PID {}) · 剩余 {}%",
                    first.process_name, first.pid, quota.primary_pct
                ),
                account: Some(quota),
            });
        }
    }

    // 2. Fallback to offline state.vscdb / Windows 凭据管理器 / ~/.gemini/* candidates
    if !offline_candidates.is_empty() {
        let mut fallback: Option<(LocalDiscoveredCredential, AccountQuota)> = None;
        for cand in offline_candidates {
            let probe_rec = StoredAccountRecord {
                id: "agy-local-probe".to_string(),
                platform: PlatformId::Antigravity,
                alias: cand.alias.clone(),
                email: cand.email.clone(),
                tier: cand.tier,
                plan_label: cand.plan_label.clone(),
                auth_method: cand.auth_method,
                profile_dir: None,
                is_demo: false,
                encrypted_secret: None,
                cached_quota: None,
            };
            let mut quota = agy_provider
                .fetch_quota(&probe_rec, Some(&cand.creds), &ctx)
                .await;
            if quota.status == AccountStatus::Ok {
                let final_email = if quota.email.is_empty() || quota.email == "local@antigravity" {
                    cand.email.clone()
                } else {
                    quota.email.clone()
                };
                quota.email = final_email.clone();
                let (target_id, target_alias) =
                    resolve_agy_target_id_and_alias(store, &final_email, alias_override.as_deref());
                quota.id = target_id.clone();
                quota.alias = target_alias.clone();

                let source_desc = cand.source_desc.clone();
                let req = AddAccountRequest {
                    platform: PlatformId::Antigravity,
                    alias: target_alias,
                    email: final_email,
                    tier: quota.tier,
                    plan_label: quota.plan_label.clone(),
                    auth_method: cand.auth_method,
                    secret_input: None,
                    refresh_token: None,
                    profile_id: None,
                };
                let _ = store.add_account(
                    &req,
                    Some(&cand.creds),
                    Some(target_id),
                    None,
                    Some(quota.clone()),
                )?;
                let _ = store.update_cached_quota(&quota);
                emit_snapshot(app, store);
                return Ok(DetectLocalResult {
                    found: true,
                    message: format!(
                        "已同步本机反重力（{source_desc} · 剩余 {}%）",
                        quota.primary_pct
                    ),
                    account: Some(quota),
                });
            }
            if fallback.is_none() {
                fallback = Some((cand, quota));
            }
        }

        if let Some((cand, mut quota)) = fallback {
            let msg = quota.status_message.clone().unwrap_or_else(|| {
                format!(
                    "已发现离线缓存（{}），但登录态已过期；请打开 Antigravity 桌面客户端 / IDE 登录后重试",
                    cand.source_desc
                )
            });

            if !persist_expired {
                return Ok(DetectLocalResult {
                    found: false,
                    message: msg,
                    account: None,
                });
            }

            let (target_id, target_alias) =
                resolve_agy_target_id_and_alias(store, &cand.email, alias_override.as_deref());
            quota.id = target_id.clone();
            quota.alias = target_alias.clone();

            let req = AddAccountRequest {
                platform: PlatformId::Antigravity,
                alias: target_alias,
                email: cand.email,
                tier: cand.tier,
                plan_label: cand.plan_label,
                auth_method: cand.auth_method,
                secret_input: None,
                refresh_token: None,
                profile_id: None,
            };
            let _ = store.add_account(
                &req,
                Some(&cand.creds),
                Some(target_id),
                None,
                Some(quota.clone()),
            )?;
            let _ = store.update_cached_quota(&quota);
            emit_snapshot(app, store);
            return Ok(DetectLocalResult {
                found: true,
                message: msg,
                account: Some(quota),
            });
        }
    }

    Ok(DetectLocalResult {
        found: false,
        message: offline_err.unwrap_or_else(|| {
            "未在本机检测到可用的 Antigravity 登录态，请先打开 Antigravity 客户端 / IDE 登录"
                .to_string()
        }),
        account: None,
    })
}

#[tauri::command]
pub async fn detect_antigravity_local(
    app: AppHandle,
    alias: Option<String>,
    store: State<'_, Arc<AccountStore>>,
    registry: State<'_, Arc<ProviderRegistry>>,
    bridge: State<'_, Arc<WebviewBridgeStore>>,
) -> Result<DetectLocalResult, String> {
    detect_antigravity_local_inner(&app, alias, false, &store, &registry, &bridge).await
}

#[tauri::command]
pub async fn detect_codex_cli_local(
    app: AppHandle,
    alias: Option<String>,
    store: State<'_, Arc<AccountStore>>,
    registry: State<'_, Arc<ProviderRegistry>>,
    bridge: State<'_, Arc<WebviewBridgeStore>>,
) -> Result<DetectLocalResult, String> {
    match tokio::task::spawn_blocking(discover_codex_cli_local)
        .await
        .map_err(|e| e.to_string())?
    {
        Ok(discovered) => {
            let quota = save_and_refresh_discovered(
                &app,
                &store,
                &registry,
                &bridge,
                discovered,
                "codex-local-cli",
                alias,
            )
            .await?;
            let valid = quota.status == AccountStatus::Ok
                || store.get_record("codex-local-cli")?.is_some();
            Ok(DetectLocalResult {
                found: valid,
                message: format_discovered_message("本机 Codex 凭据", &quota),
                account: if valid { Some(quota) } else { None },
            })
        }
        Err(msg) => Ok(DetectLocalResult {
            found: false,
            message: msg,
            account: None,
        }),
    }
}

#[tauri::command]
pub async fn detect_claude_cli_local(
    app: AppHandle,
    alias: Option<String>,
    store: State<'_, Arc<AccountStore>>,
    registry: State<'_, Arc<ProviderRegistry>>,
    bridge: State<'_, Arc<WebviewBridgeStore>>,
) -> Result<DetectLocalResult, String> {
    match tokio::task::spawn_blocking(discover_claude_code_local)
        .await
        .map_err(|e| e.to_string())?
    {
        Ok(discovered) => {
            let quota = save_and_refresh_discovered(
                &app,
                &store,
                &registry,
                &bridge,
                discovered,
                "claude-local-cli",
                alias,
            )
            .await?;
            let valid = quota.status == AccountStatus::Ok
                || store.get_record("claude-local-cli")?.is_some();
            Ok(DetectLocalResult {
                found: valid,
                message: format_discovered_message("本机 Claude 凭据", &quota),
                account: if valid { Some(quota) } else { None },
            })
        }
        Err(msg) => Ok(DetectLocalResult {
            found: false,
            message: msg,
            account: None,
        }),
    }
}

#[tauri::command]
pub async fn detect_commandcode_cli_local(
    app: AppHandle,
    alias: Option<String>,
    store: State<'_, Arc<AccountStore>>,
    registry: State<'_, Arc<ProviderRegistry>>,
    bridge: State<'_, Arc<WebviewBridgeStore>>,
) -> Result<DetectLocalResult, String> {
    match tokio::task::spawn_blocking(discover_commandcode_cli_local)
        .await
        .map_err(|e| e.to_string())?
    {
        Ok(discovered) => {
            let quota = save_and_refresh_discovered(
                &app,
                &store,
                &registry,
                &bridge,
                discovered,
                "cc-local-cli",
                alias,
            )
            .await?;
            let valid =
                quota.status == AccountStatus::Ok || store.get_record("cc-local-cli")?.is_some();
            Ok(DetectLocalResult {
                found: valid,
                message: format_discovered_message("本机 Command Code 凭据", &quota),
                account: if valid { Some(quota) } else { None },
            })
        }
        Err(msg) => Ok(DetectLocalResult {
            found: false,
            message: msg,
            account: None,
        }),
    }
}

#[tauri::command]
pub async fn detect_opencode_cli_local(
    app: AppHandle,
    alias: Option<String>,
    store: State<'_, Arc<AccountStore>>,
    registry: State<'_, Arc<ProviderRegistry>>,
    bridge: State<'_, Arc<WebviewBridgeStore>>,
) -> Result<DetectLocalResult, String> {
    match tokio::task::spawn_blocking(discover_opencode_cli_local)
        .await
        .map_err(|e| e.to_string())?
    {
        Ok(discovered) => {
            let quota = save_and_refresh_discovered(
                &app,
                &store,
                &registry,
                &bridge,
                discovered,
                "oc-local-cli",
                alias,
            )
            .await?;
            let valid =
                quota.status == AccountStatus::Ok || store.get_record("oc-local-cli")?.is_some();
            Ok(DetectLocalResult {
                found: valid,
                message: format_discovered_message("本机 OpenCode 凭据", &quota),
                account: if valid { Some(quota) } else { None },
            })
        }
        Err(msg) => Ok(DetectLocalResult {
            found: false,
            message: msg,
            account: None,
        }),
    }
}

#[tauri::command]
pub async fn scan_local_accounts(
    app: AppHandle,
    store: State<'_, Arc<AccountStore>>,
    registry: State<'_, Arc<ProviderRegistry>>,
    bridge: State<'_, Arc<WebviewBridgeStore>>,
) -> Result<ScanLocalSummary, String> {
    let agy =
        detect_antigravity_local_inner(&app, None, false, &store, &registry, &bridge).await?;
    let claude = detect_claude_cli_local(
        app.clone(),
        None,
        store.clone(),
        registry.clone(),
        bridge.clone(),
    )
    .await?;
    let codex = detect_codex_cli_local(
        app.clone(),
        None,
        store.clone(),
        registry.clone(),
        bridge.clone(),
    )
    .await?;
    let cc = detect_commandcode_cli_local(
        app.clone(),
        None,
        store.clone(),
        registry.clone(),
        bridge.clone(),
    )
    .await?;
    let oc = detect_opencode_cli_local(
        app.clone(),
        None,
        store.clone(),
        registry.clone(),
        bridge.clone(),
    )
    .await?;

    let items = vec![
        LocalScanItem {
            platform: PlatformId::Antigravity,
            found: agy.found,
            source: "language_server / state.vscdb".to_string(),
            message: agy.message,
            account: agy.account,
        },
        LocalScanItem {
            platform: PlatformId::Claude,
            found: claude.found,
            source: "~/.claude/.credentials.json".to_string(),
            message: claude.message,
            account: claude.account,
        },
        LocalScanItem {
            platform: PlatformId::Codex,
            found: codex.found,
            source: "~/.codex/auth.json".to_string(),
            message: codex.message,
            account: codex.account,
        },
        LocalScanItem {
            platform: PlatformId::Commandcode,
            found: cc.found,
            source: "~/.commandcode/auth.json".to_string(),
            message: cc.message,
            account: cc.account,
        },
        LocalScanItem {
            platform: PlatformId::Opencode,
            found: oc.found,
            source: "~/.local/share/opencode/auth.json".to_string(),
            message: oc.message,
            account: oc.account,
        },
    ];

    let found_count = items.iter().filter(|i| i.found).count();
    if found_count > 0 {
        emit_snapshot(&app, &store);
    }

    Ok(ScanLocalSummary { items, found_count })
}

#[tauri::command]
pub async fn open_in_default_browser(
    app: AppHandle,
    platform: PlatformId,
) -> Result<(), String> {
    let url = default_browser_url_for(platform);
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("打开系统默认浏览器失败: {e}"))
}

#[tauri::command]
pub async fn open_auth_webview(
    app: AppHandle,
    platform: PlatformId,
    account_id: Option<String>,
    store: State<'_, Arc<AccountStore>>,
    bridge: State<'_, Arc<WebviewBridgeStore>>,
) -> Result<String, String> {
    open_auth_webview_window(&app, &store, &bridge, platform, account_id)
}

#[tauri::command]
pub async fn capture_auth_webview(
    app: AppHandle,
    platform: PlatformId,
    session_id: String,
    rebind_account_id: Option<String>,
    store: State<'_, Arc<AccountStore>>,
    pending: State<'_, Arc<PendingAuthStore>>,
    registry: State<'_, Arc<ProviderRegistry>>,
    bridge: State<'_, Arc<WebviewBridgeStore>>,
) -> Result<CapturedAuthResult, String> {
    let result =
        capture_auth_webview_session(&app, &store, &pending, &bridge, platform, &session_id).await?;

    if let Some(target_id) = rebind_account_id {
        if let Some(entry) = pending.take(&session_id) {
            store.update_credentials(
                &target_id,
                &entry.creds,
                Some(entry.profile_dir.to_string_lossy().to_string()),
            )?;
            if let Some(rec) = store.get_record(&target_id)? {
                let pre_parsed = entry
                    .initial_scraped_payload
                    .as_ref()
                    .and_then(|p| parse_scraped_payload(rec.platform, &rec, p))
                    .filter(|q| q.status == AccountStatus::Ok);
                if let Some(q) = pre_parsed {
                    let _ = store.update_cached_quota(&q);
                } else {
                    let ctx = build_ctx(&app, &bridge, &store);
                    let _ = registry.refresh_single(&store, &rec, &ctx).await;
                }
            }
        }
    }

    Ok(result)
}

#[tauri::command]
pub async fn start_codex_browser_oauth(
    app: AppHandle,
    alias: Option<String>,
    plan_label: Option<String>,
    tier: Option<SubscriptionTier>,
    rebind_account_id: Option<String>,
    store: State<'_, Arc<AccountStore>>,
    registry: State<'_, Arc<ProviderRegistry>>,
    bridge: State<'_, Arc<WebviewBridgeStore>>,
    pending_oauth: State<'_, Arc<PendingOAuthStore>>,
) -> Result<AccountQuota, String> {
    let data = run_codex_oauth(&app, &pending_oauth).await?;
    save_oauth_browser_account(
        &app,
        &store,
        &registry,
        &bridge,
        PlatformId::Codex,
        data,
        alias,
        plan_label,
        tier,
        rebind_account_id,
    )
    .await
}

#[tauri::command]
pub async fn start_claude_browser_oauth(
    app: AppHandle,
    pending_oauth: State<'_, Arc<PendingOAuthStore>>,
) -> Result<ClaudeOAuthStartResult, String> {
    begin_claude_oauth(&app, &pending_oauth)
}

#[tauri::command]
pub async fn complete_claude_browser_oauth(
    app: AppHandle,
    flow_id: String,
    auth_code: String,
    alias: Option<String>,
    plan_label: Option<String>,
    tier: Option<SubscriptionTier>,
    rebind_account_id: Option<String>,
    store: State<'_, Arc<AccountStore>>,
    registry: State<'_, Arc<ProviderRegistry>>,
    bridge: State<'_, Arc<WebviewBridgeStore>>,
    pending_oauth: State<'_, Arc<PendingOAuthStore>>,
) -> Result<AccountQuota, String> {
    let data = finish_claude_oauth(&pending_oauth, &flow_id, &auth_code).await?;
    save_oauth_browser_account(
        &app,
        &store,
        &registry,
        &bridge,
        PlatformId::Claude,
        data,
        alias,
        plan_label,
        tier,
        rebind_account_id,
    )
    .await
}

#[tauri::command]
pub async fn cancel_browser_oauth(
    pending_oauth: State<'_, Arc<PendingOAuthStore>>,
) -> Result<(), String> {
    pending_oauth.cancel_active_loopback();
    Ok(())
}

#[tauri::command]
pub async fn set_window_mode(
    app: AppHandle,
    mode: WindowMode,
    state: State<'_, WindowState>,
) -> Result<(), String> {
    let win = app
        .get_webview_window("main")
        .ok_or_else(|| "主窗口不存在".to_string())?;
    apply_window_mode(&win, &state, mode)
}

#[tauri::command]
pub async fn snap_orb_to_edge(
    app: AppHandle,
    state: State<'_, WindowState>,
) -> Result<(f64, f64), String> {
    let win = app
        .get_webview_window("main")
        .ok_or_else(|| "主窗口不存在".to_string())?;
    do_snap_orb(&win, &state)
}

#[tauri::command]
pub async fn set_theme_vibrancy(
    app: AppHandle,
    theme: String,
    state: State<'_, WindowState>,
) -> Result<(), String> {
    let win = app
        .get_webview_window("main")
        .ok_or_else(|| "主窗口不存在".to_string())?;
    apply_theme_vibrancy(&win, &state, &theme);
    Ok(())
}

#[tauri::command]
pub async fn get_autostart_enabled(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_autostart_enabled(app: AppHandle, enabled: bool) -> Result<bool, String> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable().map_err(|e| e.to_string())?;
    } else {
        manager.disable().map_err(|e| e.to_string())?;
    }
    if let Some(tray_state) = app.try_state::<TrayState>() {
        let _ = tray_state.autostart_item.set_checked(enabled);
    }
    Ok(enabled)
}

#[tauri::command]
pub async fn get_app_settings(
    settings: State<'_, Arc<SettingsStore>>,
) -> Result<AppSettings, String> {
    Ok(settings.get_for_ui())
}

#[tauri::command]
pub async fn save_app_settings(
    app: AppHandle,
    next: AppSettings,
    settings: State<'_, Arc<SettingsStore>>,
    win_state: State<'_, WindowState>,
) -> Result<AppSettings, String> {
    let saved = settings.save(next)?;
    if let Some(win) = app.get_webview_window("main") {
        apply_theme_vibrancy(&win, &win_state, &saved.theme);
    }
    Ok(saved)
}

#[tauri::command]
pub async fn check_for_updates(
    feed_url: Option<String>,
    settings: State<'_, Arc<SettingsStore>>,
) -> Result<UpdateCheckResult, String> {
    let url = feed_url
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            let s = settings.get().update_feed_url;
            if s.trim().is_empty() {
                DEFAULT_UPDATE_FEED_URL.to_string()
            } else {
                s
            }
        });
    Ok(run_update_check(&url).await)
}

#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    download_url: String,
    sha256: Option<String>,
) -> Result<(), String> {
    download_and_install_update(&app, &download_url, sha256.as_deref()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_distinct_non_overwriting_agy_ids_per_email() {
        let id_a = agy_account_id_for_email("alice@example.com");
        let id_b = agy_account_id_for_email("bob@example.com");
        assert_ne!(id_a, id_b);
        assert!(id_a.starts_with("agy-local-"));
        assert!(id_b.starts_with("agy-local-"));
        assert_eq!(agy_account_id_for_email("local@antigravity"), "agy-local");
    }
}
