use crate::models::{
    AccountQuota, AccountStatus, AuthMethod, ExtraBalance, PlatformId, SecretCredentials,
    StoredAccountRecord, SubPool, SubscriptionTier,
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

pub struct CommandCodeProvider {
    client: Client,
}

impl CommandCodeProvider {
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
impl QuotaProvider for CommandCodeProvider {
    fn platform(&self) -> PlatformId {
        PlatformId::Commandcode
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
                    PlatformId::Commandcode,
                    &record.id,
                    PathBuf::from(profile_dir),
                    16_000,
                )
                .await
                {
                    if let Some(q) =
                        parse_scraped_payload(PlatformId::Commandcode, record, &payload)
                    {
                        return q;
                    }
                }
            }
        }

        let Some(c) = creds else {
            return make_error_quota(
                record,
                AccountStatus::AuthRequired,
                "缺少 Command Code 凭据",
            );
        };

        let api_key = c.api_key.as_deref().filter(|s| !s.is_empty());
        let cookie = c.cookie_header.as_deref().filter(|s| !s.is_empty());

        let (credits_url, summary_url, is_bearer) = if api_key.is_some() {
            (
                "https://api.commandcode.ai/alpha/billing/credits",
                None,
                true,
            )
        } else if cookie.is_some() {
            (
                "https://api.commandcode.ai/internal/billing/credits",
                Some("https://api.commandcode.ai/internal/usage/summary"),
                false,
            )
        } else {
            return make_error_quota(
                record,
                AccountStatus::AuthRequired,
                "请提供 Command Code API Key 或登录会话",
            );
        };

        let mut req = self
            .client
            .get(credits_url)
            .header("Accept", "application/json");
        if is_bearer {
            req = req.bearer_auth(api_key.unwrap());
        } else {
            req = req.header("Cookie", cookie.unwrap());
        }

        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                return make_error_quota(record, AccountStatus::Error, format!("网络错误: {e}"));
            }
        };

        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return make_error_quota(
                record,
                AccountStatus::AuthRequired,
                "Command Code 登录已失效，请重新提取会话",
            );
        }
        if !status.is_success() {
            return make_error_quota(
                record,
                AccountStatus::Error,
                format!("Command Code API 返回 HTTP {}", status.as_u16()),
            );
        }

        let credits_json: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                return make_error_quota(record, AccountStatus::Error, format!("JSON 解析失败: {e}"));
            }
        };

        let summary_json = if let Some(s_url) = summary_url {
            if let Ok(sr) = self
                .client
                .get(s_url)
                .header("Cookie", cookie.unwrap())
                .header("Accept", "application/json")
                .send()
                .await
            {
                sr.json::<Value>().await.ok()
            } else {
                None
            }
        } else {
            None
        };

        parse_commandcode_response(record, &credits_json, summary_json.as_ref()).unwrap_or_else(
            || {
                make_error_quota(
                    record,
                    AccountStatus::Error,
                    "无法识别 Command Code 额度数据结构",
                )
            },
        )
    }
}

pub fn parse_commandcode_response(
    record: &StoredAccountRecord,
    credits_json: &Value,
    summary_json: Option<&Value>,
) -> Option<AccountQuota> {
    let data = credits_json.get("data").unwrap_or(credits_json);
    let summary_data = summary_json.map(|s| s.get("data").unwrap_or(s));

    let sub_remaining = data
        .get("subscriptionCredits")
        .or_else(|| data.get("monthlyCredits"))
        .or_else(|| data.get("planCreditsRemaining"))
        .and_then(|v| v.as_f64());

    let sub_limit = data
        .get("subscriptionLimit")
        .or_else(|| data.get("monthlyLimit"))
        .or_else(|| data.get("planCreditsLimit"))
        .and_then(|v| v.as_f64());

    let purchased = data
        .get("purchasedCredits")
        .or_else(|| data.get("extraCredits"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    let five_hour_pct = extract_window_remaining(data, &["fiveHour", "five_hour", "5h", "rolling"])
        .or_else(|| {
            summary_data.and_then(|s| {
                extract_window_remaining(s, &["fiveHour", "five_hour", "5h", "rolling"])
            })
        });

    let seven_day_pct = extract_window_remaining(data, &["weekly", "sevenDay", "seven_day", "7d"])
        .or_else(|| {
            summary_data.and_then(|s| {
                extract_window_remaining(s, &["weekly", "sevenDay", "seven_day", "7d"])
            })
        });

    let monthly_pct = extract_window_remaining(data, &["monthly", "month"])
        .or_else(|| {
            summary_data.and_then(|s| extract_window_remaining(s, &["monthly", "month"]))
        })
        .or_else(|| match (sub_remaining, sub_limit) {
            (Some(rem), Some(lim)) if lim > 0.0 => Some(clamp_pct((rem / lim) * 100.0)),
            _ => None,
        });

    if five_hour_pct.is_none()
        && seven_day_pct.is_none()
        && monthly_pct.is_none()
        && sub_remaining.is_none()
    {
        return None;
    }

    let mut sub_pools = Vec::new();
    if let Some(p5) = five_hour_pct {
        sub_pools.push(SubPool {
            name: "5小时窗口（月额 20%）".to_string(),
            pct: p5,
            detail: None,
            reset_in: None,
        });
    }
    if let Some(p7) = seven_day_pct {
        sub_pools.push(SubPool {
            name: "7天周窗口（月额 50%）".to_string(),
            pct: p7,
            detail: None,
            reset_in: None,
        });
    }

    let monthly_detail = match (sub_remaining, sub_limit) {
        (Some(rem), Some(lim)) => Some(format!("剩余 ${rem:.2} / ${lim:.2}（不含充值余额）")),
        (Some(rem), None) => Some(format!("订阅月额剩余 ${rem:.2}（不含充值余额）")),
        _ => Some("仅统计订阅月度额度".to_string()),
    };

    let final_monthly_pct = monthly_pct.unwrap_or(100);
    sub_pools.push(SubPool {
        name: "月度包含额度".to_string(),
        pct: final_monthly_pct,
        detail: monthly_detail,
        reset_in: None,
    });

    let primary_pct = five_hour_pct
        .or(seven_day_pct)
        .unwrap_or(final_monthly_pct);

    let reset_in = data
        .get("resetsAt")
        .or_else(|| data.get("expiresAt"))
        .and_then(|v| v.as_str())
        .map(format_reset_countdown)
        .unwrap_or_else(|| "周期监控中".to_string());

    let plan_raw = data
        .get("plan")
        .or_else(|| data.get("planName"))
        .or_else(|| data.get("tier"))
        .and_then(|v| v.as_str())
        .unwrap_or(&record.plan_label);

    let (tier, plan_label) = match plan_raw.to_lowercase().as_str() {
        "goat" => (SubscriptionTier::Paid, "GOAT".to_string()),
        "go" => (SubscriptionTier::Paid, "Go".to_string()),
        "pro" => (SubscriptionTier::Paid, "Pro".to_string()),
        "max" => (SubscriptionTier::Paid, "Max".to_string()),
        "free" => (SubscriptionTier::Free, "Free".to_string()),
        _ => (record.tier, record.plan_label.clone()),
    };

    let extra_balance = Some(ExtraBalance {
        label: "充值余额 (Purchased Credits)".to_string(),
        amount: format!("${purchased:.2}"),
        note: Some("可结转不过期 · 独立于月度套餐额度".to_string()),
    });

    Some(AccountQuota {
        id: record.id.clone(),
        platform: PlatformId::Commandcode,
        alias: record.alias.clone(),
        email: record.email.clone(),
        tier,
        plan_label,
        primary_pct,
        reset_in,
        sub_pools,
        extra_balance,
        status: AccountStatus::Ok,
        status_message: None,
        auth_method: record.auth_method,
        last_synced_at: Some(now_hh_mm()),
        is_demo: false,
    })
}

fn extract_window_remaining(root: &Value, keys: &[&str]) -> Option<u8> {
    let windows = root.get("windows").or_else(|| root.get("usage")).unwrap_or(root);
    for key in keys {
        if let Some(obj) = windows.get(*key) {
            if let Some(rem) = obj.get("remainingPercent").and_then(|v| v.as_f64()) {
                return Some(clamp_pct(rem));
            }
            if let Some(used) = obj
                .get("usedPercent")
                .or_else(|| obj.get("percent"))
                .and_then(|v| v.as_f64())
            {
                return Some(clamp_pct(100.0 - used));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AuthMethod;
    use serde_json::json;

    #[test]
    fn separates_purchased_credits_from_monthly_quota() {
        let rec = StoredAccountRecord {
            id: "cc-1".into(),
            platform: PlatformId::Commandcode,
            alias: "GOAT 主力号".into(),
            email: "goat@example.com".into(),
            tier: SubscriptionTier::Paid,
            plan_label: "GOAT".into(),
            auth_method: AuthMethod::ApiKey,
            profile_dir: None,
            is_demo: false,
            encrypted_secret: None,
            cached_quota: None,
        };

        let credits = json!({
            "object": "billing_credit_summary",
            "data": {
                "plan": "goat",
                "totalCredits": 125.00,
                "subscriptionCredits": 15.00,
                "subscriptionLimit": 100.00,
                "purchasedCredits": 110.00,
                "windows": {
                    "fiveHour": { "usedPercent": 26.0 },
                    "weekly": { "usedPercent": 34.0 }
                }
            }
        });

        let quota = parse_commandcode_response(&rec, &credits, None).expect("should parse");
        assert_eq!(quota.primary_pct, 74);
        assert_eq!(quota.sub_pools[0].pct, 74);
        assert_eq!(quota.sub_pools[1].pct, 66);
        assert_eq!(quota.sub_pools[2].name, "月度包含额度");
        assert_eq!(quota.sub_pools[2].pct, 15);
        let extra = quota.extra_balance.expect("must have separate extra_balance");
        assert_eq!(extra.amount, "$110.00");
        assert!(extra.label.contains("Purchased Credits"));
    }
}
