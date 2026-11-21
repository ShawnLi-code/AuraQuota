use crate::local_discovery::discover_claude_code_local;
use crate::models::{
    AccountQuota, AccountStatus, AuthMethod, PlatformId, SecretCredentials, StoredAccountRecord,
    SubPool, SubscriptionTier,
};
use crate::providers::gemini::extract_percentage;
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

pub const CLAUDE_OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

pub struct ClaudeProvider {
    client: Client,
}

impl ClaudeProvider {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(12))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { client }
    }

    async fn fetch_oauth_usage(
        &self,
        record: &StoredAccountRecord,
        access_token: &str,
    ) -> Result<AccountQuota, (AccountStatus, String)> {
        let oauth_resp = self
            .client
            .get("https://api.anthropic.com/api/oauth/usage")
            .bearer_auth(access_token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| (AccountStatus::Error, format!("网络错误: {e}")))?;

        let status = oauth_resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err((
                AccountStatus::AuthRequired,
                "Claude OAuth 凭据已过期，请点击「重登」通过默认浏览器重新授权".to_string(),
            ));
        }
        if !status.is_success() {
            return Err((
                AccountStatus::Error,
                format!("Claude OAuth 接口返回 HTTP {}", status.as_u16()),
            ));
        }
        let json: Value = oauth_resp
            .json()
            .await
            .map_err(|e| (AccountStatus::Error, format!("JSON 解析失败: {e}")))?;
        parse_claude_usage(record, &json, None)
            .ok_or_else(|| (AccountStatus::Error, "未在响应中找到 Claude 额度字段".to_string()))
    }
}

pub async fn refresh_claude_oauth_token(
    client: &Client,
    refresh_token: &str,
) -> Result<(String, Option<String>), String> {
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLAUDE_OAUTH_CLIENT_ID,
    });
    let resp = client
        .post("https://console.anthropic.com/v1/oauth/token")
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("刷新 Claude Token 网络错误: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Claude Token 刷新返回 HTTP {}", resp.status().as_u16()));
    }
    let json: Value = resp
        .json()
        .await
        .map_err(|e| format!("解析 Claude Token 响应失败: {e}"))?;
    let access_token = json
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Claude 刷新响应缺少 access_token".to_string())?;
    let new_refresh = json
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    Ok((access_token, new_refresh))
}

#[async_trait]
impl QuotaProvider for ClaudeProvider {
    fn platform(&self) -> PlatformId {
        PlatformId::Claude
    }

    async fn fetch_quota(
        &self,
        record: &StoredAccountRecord,
        creds: Option<&SecretCredentials>,
        ctx: &ProviderContext,
    ) -> AccountQuota {
        // Support Claude Code CLI OAuth usage endpoint (https://api.anthropic.com/api/oauth/usage)
        if let Some(access_token) = creds
            .and_then(|c| c.access_token.as_deref())
            .filter(|s| !s.is_empty())
        {
            match self.fetch_oauth_usage(record, access_token).await {
                Ok(q) => return q,
                Err((AccountStatus::AuthRequired, err_msg)) => {
                    // 1. Try auto-refreshing with refresh_token
                    if let (Some(c), Some(rt)) = (
                        creds,
                        creds
                            .and_then(|c| c.refresh_token.as_deref())
                            .filter(|s| !s.is_empty()),
                    ) {
                        if let Ok((new_tok, rotated_rt)) =
                            refresh_claude_oauth_token(&self.client, rt).await
                        {
                            let mut next_creds = c.clone();
                            next_creds.access_token = Some(new_tok.clone());
                            if let Some(nrt) = rotated_rt {
                                next_creds.refresh_token = Some(nrt);
                            }
                            if let Some(store) = ctx.store.as_ref() {
                                let _ = store.update_credentials(&record.id, &next_creds, None);
                            }
                            if let Ok(q) = self.fetch_oauth_usage(record, &new_tok).await {
                                return q;
                            }
                        }
                    }

                    // 2. If LocalCli, re-check ~/.claude/.credentials.json in case Claude Code renewed on disk
                    if record.auth_method == AuthMethod::LocalCli {
                        if let Ok(Ok(disk)) =
                            tokio::task::spawn_blocking(discover_claude_code_local).await
                        {
                            let mut cand = disk.creds;
                            if cand.access_token.as_deref() != Some(access_token) {
                                if let Some(disk_tok) = cand.access_token.as_deref() {
                                    if let Ok(q) = self.fetch_oauth_usage(record, disk_tok).await {
                                        if let Some(store) = ctx.store.as_ref() {
                                            let _ =
                                                store.update_credentials(&record.id, &cand, None);
                                        }
                                        return q;
                                    }
                                }
                            }
                            if let Some(rt) = cand
                                .refresh_token
                                .as_deref()
                                .filter(|s| !s.is_empty())
                                .filter(|s| {
                                    creds.and_then(|c| c.refresh_token.as_deref()) != Some(*s)
                                })
                            {
                                if let Ok((new_tok, rotated_rt)) =
                                    refresh_claude_oauth_token(&self.client, rt).await
                                {
                                    cand.access_token = Some(new_tok.clone());
                                    if let Some(nrt) = rotated_rt {
                                        cand.refresh_token = Some(nrt);
                                    }
                                    if let Ok(q) = self.fetch_oauth_usage(record, &new_tok).await {
                                        if let Some(store) = ctx.store.as_ref() {
                                            let _ =
                                                store.update_credentials(&record.id, &cand, None);
                                        }
                                        return q;
                                    }
                                }
                            }
                        }
                    }

                    return make_error_quota(record, AccountStatus::AuthRequired, err_msg);
                }
                Err((st, msg)) => return make_error_quota(record, st, msg),
            }
        }

        // When logged in via isolated WebView2, execute same-origin fetch inside WebView2 first
        if record.auth_method == AuthMethod::WebviewSession {
            if let (Some(app), Some(bridge), Some(profile_dir)) = (
                ctx.app.as_ref(),
                ctx.bridge.as_ref(),
                record.profile_dir.as_deref().filter(|s| !s.is_empty()),
            ) {
                if let Ok(payload) = scrape_hidden_profile(
                    app,
                    bridge,
                    PlatformId::Claude,
                    &record.id,
                    PathBuf::from(profile_dir),
                    16_000,
                )
                .await
                {
                    if let Some(q) = parse_scraped_payload(PlatformId::Claude, record, &payload) {
                        return q;
                    }
                }
            }
        }

        let cookie_header = match build_claude_cookie(creds) {
            Some(c) => c,
            None => {
                return make_error_quota(
                    record,
                    AccountStatus::AuthRequired,
                    "缺少 Claude sessionKey Cookie",
                );
            }
        };

        let orgs_resp = match self
            .client
            .get("https://claude.ai/api/organizations")
            .header("Cookie", &cookie_header)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return make_error_quota(record, AccountStatus::Error, format!("网络错误: {e}"));
            }
        };

        if orgs_resp.status().as_u16() == 401 || orgs_resp.status().as_u16() == 403 {
            return make_error_quota(
                record,
                AccountStatus::AuthRequired,
                "sessionKey 已失效或触发风控，请点击「重登」通过内置登录窗同步",
            );
        }

        let orgs_json: Value = match orgs_resp.json().await {
            Ok(v) => v,
            Err(e) => {
                return make_error_quota(
                    record,
                    AccountStatus::AuthRequired,
                    format!("组织信息解析失败，请点击「重登」同步会话: {e}"),
                );
            }
        };

        let org_obj = orgs_json
            .as_array()
            .and_then(|arr| arr.first())
            .cloned()
            .unwrap_or(Value::Null);

        let Some(org_id) = org_obj.get("uuid").and_then(|v| v.as_str()) else {
            return make_error_quota(
                record,
                AccountStatus::AuthRequired,
                "未获取到 Claude 组织 ID，请重新登录",
            );
        };

        let usage_url = format!("https://claude.ai/api/organizations/{org_id}/usage?cedar_ember=1");
        let usage_resp = match self
            .client
            .get(&usage_url)
            .header("Cookie", &cookie_header)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return make_error_quota(record, AccountStatus::Error, format!("拉取额度失败: {e}"));
            }
        };

        if !usage_resp.status().is_success() {
            return make_error_quota(
                record,
                AccountStatus::Error,
                format!("Claude usage 接口返回 HTTP {}", usage_resp.status().as_u16()),
            );
        }

        let usage_json: Value = match usage_resp.json().await {
            Ok(v) => v,
            Err(e) => {
                return make_error_quota(record, AccountStatus::Error, format!("JSON 解析失败: {e}"));
            }
        };

        parse_claude_usage(record, &usage_json, Some(&org_obj)).unwrap_or_else(|| {
            make_error_quota(record, AccountStatus::Error, "未在响应中找到 Claude 额度字段")
        })
    }
}

fn build_claude_cookie(creds: Option<&SecretCredentials>) -> Option<String> {
    let c = creds?;
    if let Some(full) = c.cookie_header.as_deref().filter(|s| !s.is_empty()) {
        return Some(full.to_string());
    }
    if let Some(sk) = c.session_key.as_deref().filter(|s| !s.is_empty()) {
        if sk.contains("sessionKey=") {
            return Some(sk.to_string());
        }
        return Some(format!("sessionKey={sk}"));
    }
    None
}

pub fn parse_claude_usage(
    record: &StoredAccountRecord,
    json: &Value,
    org_meta: Option<&Value>,
) -> Option<AccountQuota> {
    let mut sub_pools = Vec::new();
    let mut primary_pct = None;
    let mut primary_reset = None;

    if let Some(fh) = json.get("five_hour").filter(|v| !v.is_null()) {
        if let Some(util) = fh.get("utilization").and_then(|v| v.as_f64()) {
            let rem = clamp_pct(100.0 - util);
            let reset = fh
                .get("resets_at")
                .and_then(|v| v.as_str())
                .map(format_reset_countdown)
                .unwrap_or_else(|| "5小时滑动重置".to_string());
            primary_pct = Some(rem);
            primary_reset = Some(reset.clone());
            sub_pools.push(SubPool {
                name: "5小时会话限额".to_string(),
                pct: rem,
                detail: None,
                reset_in: Some(reset),
            });
        }
    }

    if let Some(sd) = json.get("seven_day").filter(|v| !v.is_null()) {
        if let Some(util) = sd.get("utilization").and_then(|v| v.as_f64()) {
            let rem = clamp_pct(100.0 - util);
            let reset = sd
                .get("resets_at")
                .and_then(|v| v.as_str())
                .map(format_reset_countdown)
                .unwrap_or_else(|| "7天周期重置".to_string());
            if primary_pct.is_none() {
                primary_pct = Some(rem);
                primary_reset = Some(reset.clone());
            }
            sub_pools.push(SubPool {
                name: "周总额度".to_string(),
                pct: rem,
                detail: None,
                reset_in: Some(reset),
            });
        }
    }

    if let Some(opus) = json.get("seven_day_opus").filter(|v| !v.is_null()) {
        if let Some(util) = opus.get("utilization").and_then(|v| v.as_f64()) {
            let rem = clamp_pct(100.0 - util);
            sub_pools.push(SubPool {
                name: "Opus 周专项额度".to_string(),
                pct: rem,
                detail: None,
                reset_in: None,
            });
        }
    }

    if sub_pools.is_empty() {
        return None;
    }

    let rate_tier = org_meta
        .and_then(|o| o.get("rate_limit_tier"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();

    let (tier, plan_label) = if rate_tier.contains("max") {
        (SubscriptionTier::Paid, "Max".to_string())
    } else if rate_tier.contains("pro") {
        (SubscriptionTier::Paid, "Pro".to_string())
    } else if rate_tier.contains("free") {
        (SubscriptionTier::Free, "Free".to_string())
    } else {
        (record.tier, record.plan_label.clone())
    };

    Some(AccountQuota {
        id: record.id.clone(),
        platform: PlatformId::Claude,
        alias: record.alias.clone(),
        email: record.email.clone(),
        tier,
        plan_label,
        primary_pct: primary_pct.unwrap_or(100),
        reset_in: primary_reset.unwrap_or_else(|| "运行正常".to_string()),
        sub_pools,
        extra_balance: None,
        status: AccountStatus::Ok,
        status_message: None,
        auth_method: record.auth_method,
        last_synced_at: Some(now_hh_mm()),
        is_demo: false,
    })
}

pub fn parse_claude_usage_dom_text(
    record: &StoredAccountRecord,
    body_text: &str,
) -> Option<AccountQuota> {
    let lines: Vec<String> = body_text
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }

    let mut five_h = None;
    let mut weekly = None;
    for (idx, line) in lines.iter().enumerate() {
        let low = line.to_lowercase();
        let snippet = lines[idx..lines.len().min(idx + 3)].join(" ");
        if five_h.is_none()
            && (low.contains("current session") || low.contains("5-hour") || low.contains("5 小时"))
        {
            if let Some(pct) = extract_percentage(&snippet) {
                five_h = Some(if snippet.to_lowercase().contains("left") || snippet.to_lowercase().contains("remaining") {
                    clamp_pct(pct)
                } else {
                    clamp_pct(100.0 - pct)
                });
            }
        }
        if weekly.is_none() && (low.contains("weekly") || low.contains("每周") || low.contains("all models")) {
            if let Some(pct) = extract_percentage(&snippet) {
                weekly = Some(if snippet.to_lowercase().contains("left") || snippet.to_lowercase().contains("remaining") {
                    clamp_pct(pct)
                } else {
                    clamp_pct(100.0 - pct)
                });
            }
        }
    }

    if five_h.is_none() && weekly.is_none() {
        return None;
    }

    let mut sub_pools = Vec::new();
    if let Some(p5) = five_h {
        sub_pools.push(SubPool {
            name: "5小时会话限额".to_string(),
            pct: p5,
            detail: None,
            reset_in: None,
        });
    }
    if let Some(pw) = weekly {
        sub_pools.push(SubPool {
            name: "周总额度".to_string(),
            pct: pw,
            detail: None,
            reset_in: None,
        });
    }

    Some(AccountQuota {
        id: record.id.clone(),
        platform: PlatformId::Claude,
        alias: record.alias.clone(),
        email: record.email.clone(),
        tier: record.tier,
        plan_label: record.plan_label.clone(),
        primary_pct: five_h.or(weekly).unwrap_or(100),
        reset_in: "浏览器同步".to_string(),
        sub_pools,
        extra_balance: None,
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
    use crate::models::AuthMethod;
    use serde_json::json;

    #[test]
    fn parses_claude_five_hour_and_seven_day() {
        let rec = StoredAccountRecord {
            id: "claude-1".into(),
            platform: PlatformId::Claude,
            alias: "主力号".into(),
            email: "studio@example.com".into(),
            tier: SubscriptionTier::Paid,
            plan_label: "Pro".into(),
            auth_method: AuthMethod::Cookie,
            profile_dir: None,
            is_demo: false,
            encrypted_secret: None,
            cached_quota: None,
        };

        let org = json!({ "uuid": "org-123", "rate_limit_tier": "default_claude_max_5x" });
        let usage = json!({
            "five_hour": {
                "utilization": 78.0,
                "resets_at": "2099-01-01T01:00:00Z"
            },
            "seven_day": {
                "utilization": 46.0,
                "resets_at": "2099-01-06T00:00:00Z"
            }
        });

        let quota = parse_claude_usage(&rec, &usage, Some(&org)).expect("should parse");
        assert_eq!(quota.plan_label, "Max");
        assert_eq!(quota.primary_pct, 22);
        assert_eq!(quota.sub_pools[0].pct, 22);
        assert_eq!(quota.sub_pools[1].pct, 54);
    }
}
