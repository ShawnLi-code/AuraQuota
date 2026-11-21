use crate::local_discovery::discover_codex_cli_local;
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
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use reqwest::Client;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

pub const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

pub struct CodexProvider {
    client: Client,
}

impl CodexProvider {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(12))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { client }
    }

    async fn fetch_wham_usage(
        &self,
        record: &StoredAccountRecord,
        creds: &SecretCredentials,
    ) -> Result<AccountQuota, (AccountStatus, String)> {
        let mut req = self
            .client
            .get("https://chatgpt.com/backend-api/wham/usage")
            .header("Accept", "application/json");

        if let Some(token) = creds.access_token.as_deref().filter(|s| !s.is_empty()) {
            req = req.bearer_auth(token);
            if let Some(acc_id) = creds.account_id.as_deref().filter(|s| !s.is_empty()) {
                req = req.header("ChatGPT-Account-Id", acc_id);
            }
        } else if let Some(cookie) = creds.cookie_header.as_deref().filter(|s| !s.is_empty()) {
            req = req.header("Cookie", cookie);
        } else {
            return Err((
                AccountStatus::AuthRequired,
                "缺少 Codex OAuth Token 或登录会话".to_string(),
            ));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| (AccountStatus::Error, format!("网络错误: {e}")))?;

        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err((
                AccountStatus::AuthRequired,
                "Codex 登录凭据已过期，请点击「重登」通过默认浏览器一键授权".to_string(),
            ));
        }
        if !status.is_success() {
            return Err((
                AccountStatus::Error,
                format!("Codex API 返回 HTTP {}", status.as_u16()),
            ));
        }

        let json: Value = resp
            .json()
            .await
            .map_err(|e| (AccountStatus::Error, format!("JSON 解析失败: {e}")))?;

        parse_codex_usage(record, &json)
            .ok_or_else(|| (AccountStatus::Error, "无法解析 Codex 额度结构".to_string()))
    }
}

#[async_trait]
impl QuotaProvider for CodexProvider {
    fn platform(&self) -> PlatformId {
        PlatformId::Codex
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
                    PlatformId::Codex,
                    &record.id,
                    PathBuf::from(profile_dir),
                    16_000,
                )
                .await
                {
                    if let Some(q) = parse_scraped_payload(PlatformId::Codex, record, &payload) {
                        return q;
                    }
                }
            }
        }

        let Some(creds_ref) = creds else {
            return make_error_quota(record, AccountStatus::AuthRequired, "缺少 Codex 凭据");
        };

        match self.fetch_wham_usage(record, creds_ref).await {
            Ok(q) => q,
            Err((AccountStatus::AuthRequired, err_msg)) => {
                // 1. Try OAuth refresh_token auto-renewal
                if let Some(rt) = creds_ref
                    .refresh_token
                    .as_deref()
                    .filter(|s| !s.is_empty())
                {
                    if let Ok((new_tok, rotated_rt, id_tok)) =
                        refresh_codex_oauth_token(&self.client, rt).await
                    {
                        let (em_opt, acc_opt, _) = id_tok
                            .as_deref()
                            .map(parse_openai_jwt_claims)
                            .unwrap_or_else(|| parse_openai_jwt_claims(&new_tok));

                        let mut next_creds = creds_ref.clone();
                        next_creds.access_token = Some(new_tok);
                        if let Some(nrt) = rotated_rt {
                            next_creds.refresh_token = Some(nrt);
                        }
                        if next_creds.account_id.is_none() {
                            next_creds.account_id = acc_opt;
                        }
                        if let Some(store) = ctx.store.as_ref() {
                            let _ = store.update_credentials(&record.id, &next_creds, None);
                        }

                        let mut updated_rec = record.clone();
                        if let Some(em) = em_opt {
                            if updated_rec.email.is_empty()
                                || updated_rec.email == "account@local"
                                || updated_rec.email == "cli@codex.local"
                            {
                                updated_rec.email = em;
                            }
                        }
                        if let Ok(q) = self.fetch_wham_usage(&updated_rec, &next_creds).await {
                            return q;
                        }
                    }
                }

                // 2. If LocalCli, re-check ~/.codex/auth.json in case user refreshed via Codex CLI
                if record.auth_method == AuthMethod::LocalCli {
                    if let Ok(Ok(disk)) = tokio::task::spawn_blocking(discover_codex_cli_local).await {
                        let same_tok = disk.creds.access_token == creds_ref.access_token;
                        let mut candidate = disk.creds;
                        if !same_tok {
                            if let Ok(q) = self.fetch_wham_usage(record, &candidate).await {
                                if let Some(store) = ctx.store.as_ref() {
                                    let _ = store.update_credentials(&record.id, &candidate, None);
                                }
                                return q;
                            }
                        }
                        if let Some(rt) = candidate
                            .refresh_token
                            .as_deref()
                            .filter(|s| !s.is_empty())
                            .filter(|s| creds_ref.refresh_token.as_deref() != Some(*s))
                        {
                            if let Ok((new_tok, rotated_rt, _)) =
                                refresh_codex_oauth_token(&self.client, rt).await
                            {
                                candidate.access_token = Some(new_tok);
                                if let Some(nrt) = rotated_rt {
                                    candidate.refresh_token = Some(nrt);
                                }
                                if let Ok(q) = self.fetch_wham_usage(record, &candidate).await {
                                    if let Some(store) = ctx.store.as_ref() {
                                        let _ = store.update_credentials(
                                            &record.id,
                                            &candidate,
                                            None,
                                        );
                                    }
                                    return q;
                                }
                            }
                        }
                    }
                }

                make_error_quota(record, AccountStatus::AuthRequired, err_msg)
            }
            Err((st, msg)) => make_error_quota(record, st, msg),
        }
    }
}

pub async fn refresh_codex_oauth_token(
    client: &Client,
    refresh_token: &str,
) -> Result<(String, Option<String>, Option<String>), String> {
    let resp = client
        .post("https://auth.openai.com/oauth/token")
        .header("Accept", "application/json")
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", CODEX_OAUTH_CLIENT_ID),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .map_err(|e| format!("刷新 Codex Token 网络错误: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Codex Token 刷新返回 HTTP {}", resp.status().as_u16()));
    }
    let json: Value = resp
        .json()
        .await
        .map_err(|e| format!("解析 Codex Token 响应失败: {e}"))?;
    let access_token = json
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Codex 刷新响应缺少 access_token".to_string())?;
    let new_refresh = json
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let id_token = json
        .get("id_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    Ok((access_token, new_refresh, id_token))
}

pub fn parse_openai_jwt_claims(jwt: &str) -> (Option<String>, Option<String>, Option<String>) {
    let mut parts = jwt.split('.');
    let _header = match parts.next() {
        Some(v) => v,
        None => return (None, None, None),
    };
    let payload_b64 = match parts.next() {
        Some(v) => v,
        None => return (None, None, None),
    };
    let Ok(decoded) = URL_SAFE_NO_PAD.decode(payload_b64) else {
        return (None, None, None);
    };
    let Ok(payload) = serde_json::from_slice::<Value>(&decoded) else {
        return (None, None, None);
    };

    let email = payload
        .get("email")
        .or_else(|| payload.pointer("/https://api.openai.com/profile/email"))
        .and_then(|v| v.as_str())
        .filter(|s| s.contains('@'))
        .map(str::to_string);

    let account_id = payload
        .pointer("/https://api.openai.com/auth/chatgpt_account_id")
        .or_else(|| payload.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let plan_type = payload
        .pointer("/https://api.openai.com/auth/chatgpt_plan_type")
        .or_else(|| payload.get("chatgpt_plan_type"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    (email, account_id, plan_type)
}

pub fn import_local_codex_cli_auth() -> Result<(String, Option<String>, Option<String>), String> {
    let codex_home = std::env::var("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("USERPROFILE").map(|home| PathBuf::from(home).join(".codex")))
        .map_err(|_| "无法定位本机 Codex 配置目录".to_string())?;

    let auth_path = codex_home.join("auth.json");
    if !auth_path.exists() {
        return Err(format!("未找到 Codex CLI 凭证文件: {}", auth_path.display()));
    }

    let content = std::fs::read_to_string(&auth_path)
        .map_err(|e| format!("读取 Codex auth.json 失败: {e}"))?;
    let json: Value =
        serde_json::from_str(&content).map_err(|e| format!("解析 Codex auth.json 失败: {e}"))?;

    let token = json
        .pointer("/tokens/access_token")
        .or_else(|| json.get("access_token"))
        .or_else(|| json.get("OPENAI_API_KEY"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "auth.json 中未包含 access_token".to_string())?;

    let account_id = json
        .pointer("/tokens/account_id")
        .or_else(|| json.get("account_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let email = decode_jwt_email(&token).or_else(|| {
        json.pointer("/tokens/id_token")
            .and_then(|v| v.as_str())
            .and_then(decode_jwt_email)
    });

    Ok((token, account_id, email))
}

fn decode_jwt_email(jwt: &str) -> Option<String> {
    let mut parts = jwt.split('.');
    let _header = parts.next()?;
    let payload_b64 = parts.next()?;
    let decoded = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let payload: Value = serde_json::from_slice(&decoded).ok()?;
    payload
        .get("email")
        .or_else(|| payload.pointer("/https://api.openai.com/profile/email"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

pub fn parse_codex_usage(record: &StoredAccountRecord, json: &Value) -> Option<AccountQuota> {
    let rate_limit = json.get("rate_limit").unwrap_or(json);
    let primary = rate_limit.get("primary_window");
    let secondary = rate_limit.get("secondary_window");

    if primary.is_none() && secondary.is_none() {
        return None;
    }

    let mut sub_pools = Vec::new();
    let mut primary_pct = 100u8;
    let mut primary_reset = "运行正常".to_string();

    if let Some(p) = primary {
        let used = p.get("used_percent").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let rem = clamp_pct(100.0 - used);
        let reset = extract_window_reset(p);
        primary_pct = rem;
        primary_reset = reset.clone();
        sub_pools.push(SubPool {
            name: "5小时滑动窗口".to_string(),
            pct: rem,
            detail: None,
            reset_in: Some(reset),
        });
    }

    if let Some(s) = secondary {
        let used = s.get("used_percent").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let rem = clamp_pct(100.0 - used);
        let reset = extract_window_reset(s);
        if primary.is_none() {
            primary_pct = rem;
            primary_reset = reset.clone();
        }
        sub_pools.push(SubPool {
            name: "7天周额度".to_string(),
            pct: rem,
            detail: None,
            reset_in: Some(reset),
        });
    }

    let plan_raw = json
        .get("plan_type")
        .and_then(|v| v.as_str())
        .unwrap_or(&record.plan_label);
    let (tier, plan_label) = match plan_raw.to_lowercase().as_str() {
        "pro" => (SubscriptionTier::Paid, "Pro".to_string()),
        "plus" => (SubscriptionTier::Paid, "Plus".to_string()),
        "team" | "business" => (SubscriptionTier::Paid, "Team".to_string()),
        "free" => (SubscriptionTier::Free, "Free".to_string()),
        _ => (record.tier, record.plan_label.clone()),
    };

    Some(AccountQuota {
        id: record.id.clone(),
        platform: PlatformId::Codex,
        alias: record.alias.clone(),
        email: record.email.clone(),
        tier,
        plan_label,
        primary_pct,
        reset_in: primary_reset,
        sub_pools,
        extra_balance: None,
        status: AccountStatus::Ok,
        status_message: None,
        auth_method: record.auth_method,
        last_synced_at: Some(now_hh_mm()),
        is_demo: false,
    })
}

pub fn parse_codex_usage_dom_text(
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
        let snip_low = snippet.to_lowercase();
        let is_remaining = snip_low.contains("left")
            || snip_low.contains("remaining")
            || snip_low.contains("剩余");

        if five_h.is_none() && (low.contains("5-hour") || low.contains("5 hour") || low.contains("5小时")) {
            if let Some(pct) = extract_percentage(&snippet) {
                five_h = Some(if is_remaining {
                    clamp_pct(pct)
                } else {
                    clamp_pct(100.0 - pct)
                });
            }
        }
        if weekly.is_none() && (low.contains("weekly") || low.contains("7-day") || low.contains("每周")) {
            if let Some(pct) = extract_percentage(&snippet) {
                weekly = Some(if is_remaining {
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
            name: "5小时滑动窗口".to_string(),
            pct: p5,
            detail: None,
            reset_in: None,
        });
    }
    if let Some(pw) = weekly {
        sub_pools.push(SubPool {
            name: "7天周额度".to_string(),
            pct: pw,
            detail: None,
            reset_in: None,
        });
    }

    Some(AccountQuota {
        id: record.id.clone(),
        platform: PlatformId::Codex,
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

fn extract_window_reset(win: &Value) -> String {
    if let Some(secs) = win.get("reset_after_seconds").and_then(|v| v.as_i64()) {
        return crate::providers::format_seconds_remaining(secs);
    }
    if let Some(ts) = win.get("reset_at").and_then(|v| v.as_i64()) {
        return format_reset_countdown(&ts.to_string());
    }
    if let Some(s) = win.get("reset_at").and_then(|v| v.as_str()) {
        return format_reset_countdown(s);
    }
    "窗口监控中".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AuthMethod;
    use serde_json::json;

    #[test]
    fn parses_codex_5h_and_7d_windows() {
        let rec = StoredAccountRecord {
            id: "codex-1".into(),
            platform: PlatformId::Codex,
            alias: "主力工作号".into(),
            email: "work@example.com".into(),
            tier: SubscriptionTier::Free,
            plan_label: "Free".into(),
            auth_method: AuthMethod::LocalCli,
            profile_dir: None,
            is_demo: false,
            encrypted_secret: None,
            cached_quota: None,
        };

        let payload = json!({
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 32.0,
                    "reset_after_seconds": 9720
                },
                "secondary_window": {
                    "used_percent": 19.0,
                    "reset_after_seconds": 360000
                }
            }
        });

        let quota = parse_codex_usage(&rec, &payload).expect("should parse");
        assert_eq!(quota.plan_label, "Plus");
        assert_eq!(quota.tier, SubscriptionTier::Paid);
        assert_eq!(quota.primary_pct, 68);
        assert_eq!(quota.sub_pools[0].pct, 68);
        assert_eq!(quota.sub_pools[1].pct, 81);
    }
}
