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
use chrono::{DateTime, Utc};
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

        let credits_url = if api_key.is_some() {
            "https://api.commandcode.ai/alpha/billing/credits"
        } else if cookie.is_some() {
            "https://api.commandcode.ai/internal/billing/credits"
        } else {
            return make_error_quota(
                record,
                AccountStatus::AuthRequired,
                "请提供 Command Code API Key 或登录会话",
            );
        };

        let sub_url = if api_key.is_some() {
            "https://api.commandcode.ai/alpha/billing/subscriptions"
        } else {
            "https://api.commandcode.ai/internal/billing/subscriptions"
        };

        let summary_url = if api_key.is_some() {
            "https://api.commandcode.ai/alpha/usage/summary"
        } else {
            "https://api.commandcode.ai/internal/usage/summary"
        };

        let send_get = |url: &str| {
            let mut req = self.client.get(url);
            if let Some(key) = api_key {
                req = req
                    .bearer_auth(key)
                    .header("Accept", "application/json");
            } else if let Some(ck) = cookie {
                req = req
                    .header("Cookie", ck)
                    .header("Accept", "application/json, text/plain, */*")
                    .header("Origin", "https://commandcode.ai")
                    .header("Referer", "https://commandcode.ai/");
            }
            req
        };

        let resp = match send_get(credits_url).send().await {
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

        // 并发或顺次补充订阅方案接口 (/billing/subscriptions)，用于确定月度总 Grant（如 GOAT = $70）与周期结束时间
        let sub_json = match send_get(sub_url).send().await {
            Ok(sr) if sr.status().is_success() => sr.json::<Value>().await.ok(),
            _ => None,
        };

        let has_windows = credits_json.get("windowLimits").is_some()
            || credits_json.pointer("/credits/windowLimits").is_some()
            || credits_json.pointer("/data/windowLimits").is_some()
            || credits_json.get("windows").is_some()
            || credits_json.pointer("/data/windows").is_some();

        let summary_json = if !has_windows {
            match send_get(summary_url).send().await {
                Ok(sr) if sr.status().is_success() => sr.json::<Value>().await.ok(),
                _ => None,
            }
        } else {
            None
        };

        parse_commandcode_full(
            record,
            &credits_json,
            sub_json.as_ref(),
            summary_json.as_ref(),
        )
        .unwrap_or_else(|| {
            make_error_quota(
                record,
                AccountStatus::Error,
                "无法识别 Command Code 额度数据结构",
            )
        })
    }
}

fn flex_f64(v: &Value) -> Option<f64> {
    if let Some(n) = v.as_f64() {
        if n.is_finite() {
            return Some(n);
        }
    }
    if let Some(s) = v.as_str() {
        let trimmed = s.trim();
        if let Ok(n) = trimmed.parse::<f64>() {
            if n.is_finite() {
                return Some(n);
            }
        }
    }
    None
}

fn flex_reset_countdown(v: &Value) -> Option<String> {
    if let Some(ts) = flex_f64(v) {
        if ts > 0.0 {
            let secs = if ts > 10_000_000_000.0 {
                (ts / 1000.0) as i64
            } else {
                ts as i64
            };
            if let Some(dt) = DateTime::<Utc>::from_timestamp(secs, 0) {
                return Some(format_reset_countdown(&dt.to_rfc3339()));
            }
        }
    }
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(format_reset_countdown)
}

fn catalog_plan_info(raw: &str) -> Option<(SubscriptionTier, &'static str, Option<f64>)> {
    match raw.trim().to_lowercase().as_str() {
        "individual-goat" | "goat" => Some((SubscriptionTier::Paid, "GOAT", Some(70.0))),
        "individual-go" | "individual-go-v1" | "go" => {
            Some((SubscriptionTier::Paid, "Go", Some(10.0)))
        }
        "individual-pro-v1" => Some((SubscriptionTier::Paid, "Pro", Some(80.0))),
        "individual-pro" | "pro" => Some((SubscriptionTier::Paid, "Pro", Some(30.0))),
        "individual-max" | "max" => Some((SubscriptionTier::Paid, "Max", Some(150.0))),
        "individual-ultra" | "ultra" => Some((SubscriptionTier::Paid, "Ultra", Some(300.0))),
        "free" => Some((SubscriptionTier::Free, "Free", None)),
        _ => None,
    }
}

pub fn parse_commandcode_response(
    record: &StoredAccountRecord,
    credits_json: &Value,
    summary_json: Option<&Value>,
) -> Option<AccountQuota> {
    parse_commandcode_full(record, credits_json, None, summary_json)
}

pub fn parse_commandcode_full(
    record: &StoredAccountRecord,
    credits_json: &Value,
    sub_json: Option<&Value>,
    summary_json: Option<&Value>,
) -> Option<AccountQuota> {
    let has_credits_wrapper = credits_json.get("credits").is_some();
    let credits_obj = credits_json
        .get("credits")
        .or_else(|| credits_json.get("data"))
        .unwrap_or(credits_json);
    let summary_data = summary_json.map(|s| s.get("data").unwrap_or(s));

    // Subscription (/billing/subscriptions) 解析
    let sub_envelope_ok = sub_json
        .and_then(|s| s.get("success").and_then(|v| v.as_bool()))
        .unwrap_or(true);
    let sub_explicit_free = sub_json.is_some_and(|s| {
        sub_envelope_ok && s.get("data").is_some_and(|d| d.is_null())
    });
    let sub_data = sub_json
        .filter(|_| sub_envelope_ok)
        .and_then(|s| s.get("data"))
        .filter(|d| d.is_object())
        .or_else(|| sub_json.filter(|s| s.get("planId").is_some()));

    let sub_plan_id = sub_data
        .and_then(|d| {
            d.get("planId")
                .or_else(|| d.get("plan"))
                .or_else(|| d.get("planName"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            credits_obj
                .get("plan")
                .or_else(|| credits_obj.get("planId"))
                .or_else(|| credits_obj.get("planName"))
                .or_else(|| credits_obj.get("tier"))
                .and_then(|v| v.as_str())
        });

    let (tier, plan_label, catalog_grant_usd) = if sub_explicit_free {
        (SubscriptionTier::Free, "Free".to_string(), None)
    } else if let Some(raw_plan) = sub_plan_id {
        if let Some((t, lbl, usd)) = catalog_plan_info(raw_plan) {
            (t, lbl.to_string(), usd)
        } else {
            (record.tier, raw_plan.to_string(), None)
        }
    } else if let Some((t, lbl, usd)) = catalog_plan_info(&record.plan_label) {
        (t, lbl.to_string(), usd)
    } else {
        (record.tier, record.plan_label.clone(), None)
    };

    let sub_remaining = credits_obj
        .get("monthlyCredits")
        .or_else(|| credits_obj.get("subscriptionCredits"))
        .or_else(|| credits_obj.get("planCreditsRemaining"))
        .and_then(flex_f64);

    let sub_limit = credits_obj
        .get("monthlyCreditsGranted")
        .or_else(|| credits_obj.get("subscriptionLimit"))
        .or_else(|| credits_obj.get("monthlyLimit"))
        .or_else(|| credits_obj.get("planCreditsLimit"))
        .and_then(flex_f64)
        .filter(|v| *v > 0.0)
        .or(catalog_grant_usd);

    let purchased = credits_obj
        .get("purchasedCredits")
        .or_else(|| credits_obj.get("extraCredits"))
        .and_then(flex_f64)
        .unwrap_or(0.0);

    let (five_hour_pct, five_hour_reset) =
        extract_window_remaining_from_sources(
            &[
                credits_json.get("windowLimits"),
                credits_obj.get("windowLimits"),
                Some(credits_obj),
                summary_data,
            ],
            &["fiveHour", "five_hour", "5h", "rolling"],
        );

    let (seven_day_pct, seven_day_reset) =
        extract_window_remaining_from_sources(
            &[
                credits_json.get("windowLimits"),
                credits_obj.get("windowLimits"),
                Some(credits_obj),
                summary_data,
            ],
            &["weekly", "sevenDay", "seven_day", "7d"],
        );

    let (monthly_explicit_pct, _) = extract_window_remaining_from_sources(
        &[
            credits_json.get("windowLimits"),
            credits_obj.get("windowLimits"),
            Some(credits_obj),
            summary_data,
        ],
        &["monthly", "month"],
    );

    let monthly_pct = monthly_explicit_pct.or_else(|| match (sub_remaining, sub_limit) {
        (Some(rem), Some(lim)) if lim > 0.0 => Some(clamp_pct((rem / lim) * 100.0)),
        (Some(rem), None) if rem <= 0.0 => Some(0),
        _ => None,
    });

    if !has_credits_wrapper
        && five_hour_pct.is_none()
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
            reset_in: five_hour_reset.clone(),
        });
    }
    if let Some(p7) = seven_day_pct {
        sub_pools.push(SubPool {
            name: "7天周窗口（月额 50%）".to_string(),
            pct: p7,
            detail: None,
            reset_in: seven_day_reset.clone(),
        });
    }

    let monthly_detail = match (sub_remaining, sub_limit) {
        (Some(rem), Some(lim)) => Some(format!("剩余 ${rem:.2} / ${lim:.2}（不含充值余额）")),
        (Some(rem), None) => Some(format!("订阅月额剩余 ${rem:.2}（不含充值余额）")),
        _ => Some("仅统计订阅月度额度".to_string()),
    };

    let period_reset = sub_data
        .and_then(|d| d.get("currentPeriodEnd"))
        .and_then(flex_reset_countdown)
        .or_else(|| {
            credits_obj
                .get("resetsAt")
                .or_else(|| credits_obj.get("expiresAt"))
                .and_then(flex_reset_countdown)
        });

    let final_monthly_pct = monthly_pct.unwrap_or(100);
    sub_pools.push(SubPool {
        name: "月度包含额度".to_string(),
        pct: final_monthly_pct,
        detail: monthly_detail,
        reset_in: period_reset.clone(),
    });

    let primary_pct = five_hour_pct
        .or(seven_day_pct)
        .unwrap_or(final_monthly_pct);

    let reset_in = five_hour_reset
        .or(seven_day_reset)
        .or(period_reset)
        .unwrap_or_else(|| "周期监控中".to_string());

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

fn extract_window_remaining_from_sources(
    sources: &[Option<&Value>],
    keys: &[&str],
) -> (Option<u8>, Option<String>) {
    for src_opt in sources {
        let Some(src) = src_opt else { continue };
        let containers = [
            Some(*src),
            src.get("windowLimits"),
            src.get("windows"),
            src.get("usage"),
        ];
        for container in containers.into_iter().flatten() {
            for key in keys {
                if let Some(obj) = container.get(*key) {
                    let reset = obj
                        .get("resetAt")
                        .or_else(|| obj.get("resetsAt"))
                        .and_then(flex_reset_countdown);
                    if let (Some(cap), used_opt) = (
                        obj.get("cap").and_then(flex_f64),
                        obj.get("used").and_then(flex_f64),
                    ) {
                        if cap > 0.0 {
                            let used = used_opt.unwrap_or(0.0).max(0.0);
                            return (Some(clamp_pct(((cap - used) / cap) * 100.0)), reset);
                        }
                    }
                    if let Some(rem) = obj.get("remainingPercent").and_then(flex_f64) {
                        return (Some(clamp_pct(rem)), reset);
                    }
                    if let Some(used) = obj
                        .get("usedPercent")
                        .or_else(|| obj.get("percent"))
                        .and_then(flex_f64)
                    {
                        return (Some(clamp_pct(100.0 - used)), reset);
                    }
                }
            }
        }
    }
    (None, None)
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

    #[test]
    fn parses_real_commandcode_credits_nested_and_root_window_limits() {
        let rec = StoredAccountRecord {
            id: "cc-real".into(),
            platform: PlatformId::Commandcode,
            alias: "GOAT".into(),
            email: "user@example.com".into(),
            tier: SubscriptionTier::Paid,
            plan_label: "GOAT".into(),
            auth_method: AuthMethod::WebviewSession,
            profile_dir: None,
            is_demo: false,
            encrypted_secret: None,
            cached_quota: None,
        };

        // 真实 /internal/billing/credits 嵌套 windowLimits + 字符串数字 + 订阅接口 individual-goat ($70)
        let nested_credits = json!({
            "credits": {
                "monthlyCredits": 52.5,
                "purchasedCredits": 12.0,
                "premiumMonthlyCredits": 0,
                "opensourceMonthlyCredits": 0,
                "windowLimits": {
                    "fiveHour": {
                        "cap": "4",
                        "used": "1",
                        "resetAt": "1780200000"
                    },
                    "weekly": {
                        "cap": 20,
                        "used": 4,
                        "resetAt": 1780300000000i64
                    }
                }
            }
        });
        let sub = json!({
            "success": true,
            "data": {
                "planId": "individual-goat",
                "status": "active",
                "currentPeriodEnd": "2099-01-01T00:00:00Z"
            }
        });

        let quota =
            parse_commandcode_full(&rec, &nested_credits, Some(&sub), None).expect("nested");
        assert_eq!(quota.plan_label, "GOAT");
        assert_eq!(quota.primary_pct, 75);
        assert_eq!(quota.sub_pools[0].pct, 75); // 5h: (4-1)/4 = 75%
        assert_eq!(quota.sub_pools[1].pct, 80); // weekly: (20-4)/20 = 80%
        assert_eq!(quota.sub_pools[2].pct, 75); // monthly: 52.5 / 70.0 = 75%
        assert_eq!(
            quota.extra_balance.as_ref().expect("purchased").amount,
            "$12.00"
        );

        // 真实顶层 windowLimits 结构
        let root_credits = json!({
            "credits": {
                "monthlyCredits": 8.5,
                "purchasedCredits": 0
            },
            "windowLimits": {
                "fiveHour": { "cap": 3, "used": 0.75 },
                "weekly": { "cap": 15, "used": 1.5 }
            }
        });
        let quota_root =
            parse_commandcode_full(&rec, &root_credits, None, None).expect("root");
        assert_eq!(quota_root.primary_pct, 75);
        assert_eq!(quota_root.sub_pools[1].pct, 90);
    }
}
