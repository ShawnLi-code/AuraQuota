pub mod antigravity;
pub mod claude;
pub mod codex;
pub mod commandcode;
pub mod gemini;
pub mod opencode;

use crate::models::{AccountQuota, AccountStatus, PlatformId, SecretCredentials, StoredAccountRecord};
use crate::storage::AccountStore;
use crate::webview_scraper::WebviewBridgeStore;
use async_trait::async_trait;
use chrono::{DateTime, Local, Utc};
use serde_json::Value;
use std::sync::Arc;
use tauri::AppHandle;
use tokio::task::JoinSet;

#[derive(Clone, Default)]
pub struct ProviderContext {
    pub app: Option<AppHandle>,
    pub bridge: Option<Arc<WebviewBridgeStore>>,
    pub store: Option<Arc<AccountStore>>,
}

#[async_trait]
pub trait QuotaProvider: Send + Sync {
    fn platform(&self) -> PlatformId;
    async fn fetch_quota(
        &self,
        record: &StoredAccountRecord,
        creds: Option<&SecretCredentials>,
        ctx: &ProviderContext,
    ) -> AccountQuota;
}

pub struct ProviderRegistry {
    providers: Vec<Arc<dyn QuotaProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: vec![
                Arc::new(antigravity::AntigravityProvider::new()),
                Arc::new(gemini::GeminiProvider::new()),
                Arc::new(codex::CodexProvider::new()),
                Arc::new(claude::ClaudeProvider::new()),
                Arc::new(commandcode::CommandCodeProvider::new()),
                Arc::new(opencode::OpenCodeProvider::new()),
            ],
        }
    }

    pub fn provider_for(&self, platform: PlatformId) -> Option<Arc<dyn QuotaProvider>> {
        self.providers
            .iter()
            .find(|p| p.platform() == platform)
            .cloned()
    }

    pub async fn refresh_single(
        &self,
        store: &AccountStore,
        record: &StoredAccountRecord,
        ctx: &ProviderContext,
    ) -> AccountQuota {
        if record.is_demo {
            return record.cached_quota.clone().unwrap_or_else(|| {
                make_error_quota(record, AccountStatus::Demo, "示例数据")
            });
        }

        let creds = match store.decrypt_credentials(record) {
            Ok(c) => c,
            Err(e) => {
                let err_q = make_error_quota(record, AccountStatus::AuthRequired, e);
                let _ = store.update_cached_quota(&err_q);
                return err_q;
            }
        };

        let Some(provider) = self.provider_for(record.platform) else {
            return make_error_quota(record, AccountStatus::Error, "未知平台适配器");
        };

        let quota = provider.fetch_quota(record, creds.as_ref(), ctx).await;
        let _ = store.update_cached_quota(&quota);
        quota
    }

    pub async fn refresh_all(
        &self,
        store: &AccountStore,
        ctx: &ProviderContext,
    ) -> Result<Vec<AccountQuota>, String> {
        let records = store.list_records()?;
        let mut set = JoinSet::new();

        for (idx, record) in records.into_iter().enumerate() {
            if record.is_demo {
                let demo_quota = record.cached_quota.clone().unwrap_or_else(|| {
                    make_error_quota(&record, AccountStatus::Demo, "示例数据")
                });
                set.spawn(async move { (idx, demo_quota) });
                continue;
            }

            let provider = self.provider_for(record.platform);
            let creds_res = store.decrypt_credentials(&record);
            let ctx_clone = ctx.clone();

            set.spawn(async move {
                let quota = match (provider, creds_res) {
                    (Some(prov), Ok(creds)) => {
                        prov.fetch_quota(&record, creds.as_ref(), &ctx_clone).await
                    }
                    (Some(_), Err(e)) => {
                        make_error_quota(&record, AccountStatus::AuthRequired, e)
                    }
                    (None, _) => {
                        make_error_quota(&record, AccountStatus::Error, "未找到平台适配器")
                    }
                };
                (idx, quota)
            });
        }

        let mut indexed = Vec::new();
        while let Some(res) = set.join_next().await {
            if let Ok(item) = res {
                indexed.push(item);
            }
        }
        indexed.sort_by_key(|(idx, _)| *idx);

        let mut quotas = Vec::with_capacity(indexed.len());
        for (_, q) in indexed {
            if !q.is_demo {
                let _ = store.update_cached_quota(&q);
            }
            quotas.push(q);
        }

        Ok(quotas)
    }
}

pub fn parse_scraped_payload(
    platform: PlatformId,
    record: &StoredAccountRecord,
    payload: &Value,
) -> Option<AccountQuota> {
    if payload.get("authRequired").and_then(|v| v.as_bool()) == Some(true) {
        return Some(make_error_quota(
            record,
            AccountStatus::AuthRequired,
            "登录会话已过期，请点击「重登」重新登录",
        ));
    }

    let url = payload.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let body_text = payload
        .get("bodyText")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let profile_hint = payload.get("profileHint").and_then(|v| v.as_str());

    match platform {
        PlatformId::Gemini => {
            gemini::parse_gemini_usage_page_text(record, url, body_text, profile_hint)
        }
        PlatformId::Antigravity => None,
        PlatformId::Claude => {
            if let Some(usage) = payload.get("apiUsage") {
                let org = payload.get("org");
                if let Some(q) = claude::parse_claude_usage(record, usage, org) {
                    return Some(q);
                }
            }
            claude::parse_claude_usage_dom_text(record, body_text)
        }
        PlatformId::Codex => {
            let mut rec = record.clone();
            if let Some(em) = payload
                .get("email")
                .and_then(|v| v.as_str())
                .filter(|s| s.contains('@'))
            {
                if rec.email.is_empty() || rec.email == "account@local" {
                    rec.email = em.to_string();
                }
            }
            if let Some(usage) = payload.get("apiUsage") {
                if let Some(q) = codex::parse_codex_usage(&rec, usage) {
                    return Some(q);
                }
            }
            codex::parse_codex_usage_dom_text(&rec, body_text)
        }
        PlatformId::Commandcode => {
            let credits = payload.get("creditsJson")?;
            let summary = payload.get("summaryJson");
            commandcode::parse_commandcode_response(record, credits, summary)
        }
        PlatformId::Opencode => {
            let usage = payload.get("apiUsage")?;
            let bal = payload
                .pointer("/billingJson/balance")
                .or_else(|| payload.pointer("/billingJson/data/balance"))
                .and_then(|v| v.as_f64())
                .map(|b| format!("${b:.2}"));
            opencode::parse_opencode_usage(record, usage, bal)
        }
    }
}

pub fn clamp_pct(v: f64) -> u8 {
    if v.is_nan() {
        return 0;
    }
    v.round().clamp(0.0, 100.0) as u8
}

pub fn now_hh_mm() -> String {
    Local::now().format("%H:%M").to_string()
}

pub fn format_reset_countdown(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "待同步".to_string();
    }

    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        let secs = dt
            .with_timezone(&Utc)
            .signed_duration_since(Utc::now())
            .num_seconds();
        return format_seconds_remaining(secs);
    }

    if let Ok(num) = trimmed.parse::<i64>() {
        let secs = if num > 1_000_000_000_000 {
            (num / 1000) - Utc::now().timestamp()
        } else if num > 1_000_000_000 {
            num - Utc::now().timestamp()
        } else {
            num
        };
        return format_seconds_remaining(secs);
    }

    trimmed.to_string()
}

pub fn format_seconds_remaining(secs: i64) -> String {
    if secs <= 60 {
        return "即将重置".to_string();
    }
    let total_mins = secs / 60;
    let days = total_mins / (60 * 24);
    let hours = (total_mins % (60 * 24)) / 60;
    let mins = total_mins % 60;

    if days > 0 {
        format!("{days}天{hours}小时后重置")
    } else if hours > 0 {
        format!("{hours}小时{mins}分后重置")
    } else {
        format!("{mins}分钟后重置")
    }
}

pub fn make_error_quota(
    record: &StoredAccountRecord,
    status: AccountStatus,
    message: impl Into<String>,
) -> AccountQuota {
    let msg = message.into();
    if let Some(mut cached) = record.cached_quota.clone() {
        cached.status = status;
        cached.status_message = Some(msg);
        cached.last_synced_at = Some(now_hh_mm());
        return cached;
    }

    AccountQuota {
        id: record.id.clone(),
        platform: record.platform,
        alias: record.alias.clone(),
        email: record.email.clone(),
        tier: record.tier,
        plan_label: record.plan_label.clone(),
        primary_pct: 0,
        reset_in: "同步失败".to_string(),
        sub_pools: Vec::new(),
        extra_balance: None,
        status,
        status_message: Some(msg),
        auth_method: record.auth_method,
        last_synced_at: Some(now_hh_mm()),
        is_demo: record.is_demo,
    }
}
