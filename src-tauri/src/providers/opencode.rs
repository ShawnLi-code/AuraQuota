use crate::models::{
    AccountQuota, AccountStatus, AuthMethod, ExtraBalance, PlatformId, SecretCredentials,
    StoredAccountRecord, SubPool,
};
use crate::providers::{
    clamp_pct, format_reset_countdown, make_error_quota, now_hh_mm, parse_scraped_payload,
    ProviderContext, QuotaProvider,
};
use crate::webview_scraper::scrape_hidden_profile;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

pub struct OpenCodeProvider {
    client: Client,
}

impl OpenCodeProvider {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent("AuraQuota/0.1")
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { client }
    }
}

#[async_trait]
impl QuotaProvider for OpenCodeProvider {
    fn platform(&self) -> PlatformId {
        PlatformId::Opencode
    }

    async fn fetch_quota(
        &self,
        record: &StoredAccountRecord,
        creds: Option<&SecretCredentials>,
        ctx: &ProviderContext,
    ) -> AccountQuota {
        if record.auth_method == AuthMethod::WebviewSession {
            if let (Some(app), Some(bridge), Some(profile_dir)) = (
                ctx.app.as_ref(),
                ctx.bridge.as_ref(),
                record.profile_dir.as_deref().filter(|s| !s.is_empty()),
            ) {
                if let Ok(payload) = scrape_hidden_profile(
                    app,
                    bridge,
                    PlatformId::Opencode,
                    &record.id,
                    PathBuf::from(profile_dir),
                    16_000,
                )
                .await
                {
                    if let Some(q) = parse_scraped_payload(PlatformId::Opencode, record, &payload) {
                        return q;
                    }
                }
            }
        }

        let Some(c) = creds else {
            return make_error_quota(
                record,
                AccountStatus::AuthRequired,
                "缺少 OpenCode API Key 或 Console 会话",
            );
        };

        let api_key = c.api_key.as_deref().filter(|s| !s.is_empty());
        let cookie = c.cookie_header.as_deref().filter(|s| !s.is_empty());

        let mut req = self
            .client
            .get("https://opencode.ai/zen/go/v1/usage")
            .header("Accept", "application/json");

        if let Some(key) = api_key {
            req = req.bearer_auth(key);
        } else if let Some(ck) = cookie {
            req = req.header("Cookie", ck);
        } else {
            return make_error_quota(
                record,
                AccountStatus::AuthRequired,
                "请配置 OpenCode API Key 或登录 Console",
            );
        }

        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                return make_error_quota(record, AccountStatus::Error, format!("网络错误: {e}"));
            }
        };

        let status = resp.status();
        if status.as_u16() == 402 {
            let mut q = make_error_quota(record, AccountStatus::Ok, "Console / Zen 余额已耗尽");
            q.extra_balance = Some(ExtraBalance {
                label: "Console / Zen 余额".to_string(),
                amount: "$0.00 (余额不足)".to_string(),
                note: Some("接口返回 HTTP 402".to_string()),
            });
            return q;
        }
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return make_error_quota(
                record,
                AccountStatus::AuthRequired,
                "OpenCode 凭据已过期，请重新配置",
            );
        }
        if !status.is_success() {
            return make_error_quota(
                record,
                AccountStatus::Error,
                format!("OpenCode 接口返回 HTTP {}", status.as_u16()),
            );
        }

        let json: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                return make_error_quota(record, AccountStatus::Error, format!("JSON 解析失败: {e}"));
            }
        };

        let balance_override = if let Some(ck) = cookie {
            self.probe_console_balance(ck).await
        } else {
            None
        };

        parse_opencode_usage(record, &json, balance_override).unwrap_or_else(|| {
            make_error_quota(record, AccountStatus::Error, "无法解析 OpenCode Go 额度字段")
        })
    }
}

impl OpenCodeProvider {
    async fn probe_console_balance(&self, cookie: &str) -> Option<String> {
        let resp = self
            .client
            .get("https://opencode.ai/console/api/billing/status")
            .header("Cookie", cookie)
            .header("Accept", "application/json")
            .send()
            .await
            .ok()?;

        if !resp.status().is_success() {
            return None;
        }
        let val: Value = resp.json().await.ok()?;
        val.get("balance")
            .or_else(|| val.pointer("/data/balance"))
            .and_then(|v| v.as_f64())
            .map(|b| format!("${b:.2}"))
    }
}

pub fn parse_opencode_usage(
    record: &StoredAccountRecord,
    json: &Value,
    balance_override: Option<String>,
) -> Option<AccountQuota> {
    let usage = json.get("usage").unwrap_or(json);

    let rolling = usage.get("rolling");
    let weekly = usage.get("weekly");
    let monthly = usage.get("monthly");

    if rolling.is_none() && weekly.is_none() && monthly.is_none() {
        return None;
    }

    let mut sub_pools = Vec::new();
    let mut primary_pct = None;
    let mut primary_reset = None;

    if let Some(r) = rolling {
        let used = r.get("percent").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let rem = clamp_pct(100.0 - used);
        let reset = r
            .get("resetsAt")
            .and_then(|v| v.as_str())
            .map(format_reset_countdown)
            .unwrap_or_else(|| "5小时滑动重置".to_string());
        primary_pct = Some(rem);
        primary_reset = Some(reset.clone());
        sub_pools.push(SubPool {
            name: "5小时窗口（月额 20%）".to_string(),
            pct: rem,
            detail: None,
            reset_in: Some(reset),
        });
    }

    if let Some(w) = weekly {
        let used = w.get("percent").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let rem = clamp_pct(100.0 - used);
        let reset = w
            .get("resetsAt")
            .and_then(|v| v.as_str())
            .map(format_reset_countdown);
        if primary_pct.is_none() {
            primary_pct = Some(rem);
            primary_reset = reset.clone();
        }
        sub_pools.push(SubPool {
            name: "7天周窗口（月额 50%）".to_string(),
            pct: rem,
            detail: None,
            reset_in: reset,
        });
    }

    if let Some(m) = monthly {
        let used = m.get("percent").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let rem = clamp_pct(100.0 - used);
        let reset = m
            .get("resetsAt")
            .and_then(|v| v.as_str())
            .map(format_reset_countdown);
        if primary_pct.is_none() {
            primary_pct = Some(rem);
            primary_reset = reset.clone();
        }
        sub_pools.push(SubPool {
            name: "月度额度（100%）".to_string(),
            pct: rem,
            detail: None,
            reset_in: reset,
        });
    }

    let json_balance = json
        .get("balance")
        .or_else(|| json.pointer("/console/balance"))
        .and_then(|v| v.as_f64())
        .map(|b| format!("${b:.2}"));

    let extra_balance = match balance_override.or(json_balance) {
        Some(amt) => Some(ExtraBalance {
            label: "Console / Zen 余额".to_string(),
            amount: amt,
            note: Some("按量计费账户余额".to_string()),
        }),
        None => Some(ExtraBalance {
            label: "Console / Zen 余额".to_string(),
            amount: "API Key 模式未公开余额".to_string(),
            note: Some("可通过内置登录窗接入 Console 查看具体余额".to_string()),
        }),
    };

    Some(AccountQuota {
        id: record.id.clone(),
        platform: PlatformId::Opencode,
        alias: record.alias.clone(),
        email: record.email.clone(),
        tier: record.tier,
        plan_label: record.plan_label.clone(),
        primary_pct: primary_pct.unwrap_or(100),
        reset_in: primary_reset.unwrap_or_else(|| "运行正常".to_string()),
        sub_pools,
        extra_balance,
        status: AccountStatus::Ok,
        status_message: None,
        auth_method: record.auth_method,
        last_synced_at: Some(now_hh_mm()),
        is_demo: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AuthMethod, SubscriptionTier};
    use serde_json::json;

    #[test]
    fn parses_opencode_rolling_weekly_monthly() {
        let rec = StoredAccountRecord {
            id: "oc-1".into(),
            platform: PlatformId::Opencode,
            alias: "Go 常用号".into(),
            email: "go@example.com".into(),
            tier: SubscriptionTier::Paid,
            plan_label: "Go".into(),
            auth_method: AuthMethod::ApiKey,
            profile_dir: None,
            is_demo: false,
            encrypted_secret: None,
            cached_quota: None,
        };

        let payload = json!({
            "usage": {
                "rolling": { "status": "ok", "percent": 42, "resetsAt": "2099-01-01T18:00:00Z" },
                "weekly": { "status": "ok", "percent": 29, "resetsAt": "2099-01-05T00:00:00Z" },
                "monthly": { "status": "ok", "percent": 15, "resetsAt": "2099-02-01T00:00:00Z" }
            }
        });

        let quota = parse_opencode_usage(&rec, &payload, Some("$24.50".into())).expect("parse");
        assert_eq!(quota.primary_pct, 58);
        assert_eq!(quota.sub_pools.len(), 3);
        assert_eq!(quota.sub_pools[1].pct, 71);
        assert_eq!(quota.sub_pools[2].pct, 85);
        assert_eq!(quota.extra_balance.unwrap().amount, "$24.50");
    }
}
