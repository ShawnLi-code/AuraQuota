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
use crate::providers::antigravity::discover_local_language_servers;
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
    if let Ok(all) = store.list_quotas() {
        let _ = app.emit(
            "quota-updated",
            QuotaSnapshot {
                accounts: all,
                last_updated_at: now_hh_mm(),
                polling_interval_secs: 300,
            },
        );
    }
}

fn format_discovered_message(label: &str, quota: &AccountQuota) -> String {
    if quota.status == AccountStatus::Ok {
        format!("已同步{label}（剩余 {}%）", quota.primary_pct)
    } else {
        quota
            .status_message
            .clone()
            .unwrap_or_else(|| format!("已导入{label}，但当前需要重登或检查网络"))
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
    let req = AddAccountRequest {
        platform: discovered.platform,
        alias: alias_override.unwrap_or(discovered.alias),
        email: discovered.email,
        tier: discovered.tier,
        plan_label: discovered.plan_label,
        auth_method: discovered.auth_method,
        secret_input: None,
        refresh_token: None,
        profile_id: None,
    };
    let rec = store.add_account(
        &req,
        Some(&discovered.creds),
        Some(explicit_id.to_string()),
        None,
        None,
    )?;
    let ctx = build_ctx(app, bridge, store);
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
    let accounts = store.list_quotas()?;
    Ok(QuotaSnapshot {
        accounts,
        last_updated_at: now_hh_mm(),
        polling_interval_secs: 300,
    })
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
    let snap = QuotaSnapshot {
        accounts,
        last_updated_at: now_hh_mm(),
        polling_interval_secs: 300,
    };
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
    Ok(quota)
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

    if let Ok(all) = store.list_quotas() {
        let _ = app.emit(
            "quota-updated",
            QuotaSnapshot {
                accounts: all,
                last_updated_at: now_hh_mm(),
                polling_interval_secs: 300,
            },
        );
    }

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
        if let Ok(all) = store.list_quotas() {
            let _ = app.emit(
                "quota-updated",
                QuotaSnapshot {
                    accounts: all,
                    last_updated_at: now_hh_mm(),
                    polling_interval_secs: 300,
                },
            );
        }
    }
    Ok(removed)
}

#[tauri::command]
pub async fn clear_demo_accounts(
    store: State<'_, Arc<AccountStore>>,
) -> Result<QuotaSnapshot, String> {
    store.clear_demo_accounts()?;
    Ok(QuotaSnapshot {
        accounts: store.list_quotas()?,
        last_updated_at: now_hh_mm(),
        polling_interval_secs: 300,
    })
}

#[tauri::command]
pub async fn restore_demo_accounts(
    store: State<'_, Arc<AccountStore>>,
) -> Result<QuotaSnapshot, String> {
    store.restore_demo_accounts()?;
    Ok(QuotaSnapshot {
        accounts: store.list_quotas()?,
        last_updated_at: now_hh_mm(),
        polling_interval_secs: 300,
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
    let custom_alias = alias
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "本机反重力".to_string());
    let ctx = build_ctx(&app, &bridge, &store);
    let agy_provider = registry
        .provider_for(PlatformId::Antigravity)
        .ok_or_else(|| "未找到反重力适配器".to_string())?;

    // Remove legacy duplicate IDs before saving unified "agy-local"
    let _ = store.remove_account("agy-local-server");
    let _ = store.remove_account("agy-local-offline");

    // 1. Probe live language_server in-memory first (do NOT persist failed attempts)
    if let Ok(Ok(servers)) = tokio::task::spawn_blocking(discover_local_language_servers).await {
        let first = &servers[0];
        let req = AddAccountRequest {
            platform: PlatformId::Antigravity,
            alias: custom_alias.clone(),
            email: "local@antigravity".to_string(),
            tier: SubscriptionTier::Paid,
            plan_label: "AI Pro".to_string(),
            auth_method: AuthMethod::LocalServer,
            secret_input: None,
            refresh_token: None,
            profile_id: None,
        };
        let probe_rec = StoredAccountRecord {
            id: "agy-local".to_string(),
            platform: req.platform,
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
        let quota = agy_provider.fetch_quota(&probe_rec, None, &ctx).await;
        if quota.status == AccountStatus::Ok {
            let _ = store.add_account(
                &req,
                None,
                Some("agy-local".to_string()),
                None,
                Some(quota.clone()),
            )?;
            let _ = store.update_cached_quota(&quota);
            emit_snapshot(&app, &store);
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

    // 2. Fallback to offline state.vscdb / ~/.gemini/* candidates and try each before failing
    match tokio::task::spawn_blocking(discover_antigravity_offline_candidates)
        .await
        .map_err(|e| e.to_string())?
    {
        Ok(candidates) => {
            let mut fallback: Option<(LocalDiscoveredCredential, AccountQuota)> = None;
            for cand in candidates {
                let probe_rec = StoredAccountRecord {
                    id: "agy-local".to_string(),
                    platform: PlatformId::Antigravity,
                    alias: custom_alias.clone(),
                    email: cand.email.clone(),
                    tier: cand.tier,
                    plan_label: cand.plan_label.clone(),
                    auth_method: cand.auth_method,
                    profile_dir: None,
                    is_demo: false,
                    encrypted_secret: None,
                    cached_quota: None,
                };
                let quota = agy_provider
                    .fetch_quota(&probe_rec, Some(&cand.creds), &ctx)
                    .await;
                if quota.status == AccountStatus::Ok {
                    let source_desc = cand.source_desc.clone();
                    let req = AddAccountRequest {
                        platform: PlatformId::Antigravity,
                        alias: custom_alias.clone(),
                        email: quota.email.clone(),
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
                        Some("agy-local".to_string()),
                        None,
                        Some(quota.clone()),
                    )?;
                    let _ = store.update_cached_quota(&quota);
                    emit_snapshot(&app, &store);
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

            if let Some((cand, quota)) = fallback {
                let req = AddAccountRequest {
                    platform: PlatformId::Antigravity,
                    alias: custom_alias,
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
                    Some("agy-local".to_string()),
                    None,
                    Some(quota.clone()),
                )?;
                let _ = store.update_cached_quota(&quota);
                emit_snapshot(&app, &store);
                let msg = quota.status_message.clone().unwrap_or_else(|| {
                    format!("已找到 {}，但同步失败", cand.source_desc)
                });
                return Ok(DetectLocalResult {
                    found: true,
                    message: msg,
                    account: Some(quota),
                });
            }

            Ok(DetectLocalResult {
                found: false,
                message: "未在本机检测到可用的 Antigravity 登录凭证".to_string(),
                account: None,
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
            Ok(DetectLocalResult {
                found: true,
                message: format_discovered_message("本机 Codex 凭据", &quota),
                account: Some(quota),
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
            Ok(DetectLocalResult {
                found: true,
                message: format_discovered_message("本机 Claude 凭据", &quota),
                account: Some(quota),
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
            Ok(DetectLocalResult {
                found: true,
                message: format_discovered_message("本机 Command Code 凭据", &quota),
                account: Some(quota),
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
            Ok(DetectLocalResult {
                found: true,
                message: format_discovered_message("本机 OpenCode 凭据", &quota),
                account: Some(quota),
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
    let agy = detect_antigravity_local(
        app.clone(),
        None,
        store.clone(),
        registry.clone(),
        bridge.clone(),
    )
    .await?;
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
        if let Ok(all) = store.list_quotas() {
            let _ = app.emit(
                "quota-updated",
                QuotaSnapshot {
                    accounts: all,
                    last_updated_at: now_hh_mm(),
                    polling_interval_secs: 300,
                },
            );
        }
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
