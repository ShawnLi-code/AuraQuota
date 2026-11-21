use crate::models::{
    AccountQuota, AccountStatus, ExtraBalance, PlatformId, SecretCredentials, StoredAccountRecord,
    SubPool, SubscriptionTier,
};
use crate::providers::{clamp_pct, make_error_quota, now_hh_mm, ProviderContext, QuotaProvider};
use crate::webview_scraper::scrape_hidden_profile;
use async_trait::async_trait;
use reqwest::redirect::Policy;
use reqwest::Client;
use std::path::PathBuf;
use std::time::Duration;

pub struct GeminiProvider {
    client: Client,
}

impl GeminiProvider {
    pub fn new() -> Self {
        let client = Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(10))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { client }
    }
}

#[async_trait]
impl QuotaProvider for GeminiProvider {
    fn platform(&self) -> PlatformId {
        PlatformId::Gemini
    }

    async fn fetch_quota(
        &self,
        record: &StoredAccountRecord,
        creds: Option<&SecretCredentials>,
        ctx: &ProviderContext,
    ) -> AccountQuota {
        if let (Some(app), Some(bridge), Some(profile_dir)) = (
            ctx.app.as_ref(),
            ctx.bridge.as_ref(),
            record.profile_dir.as_deref().filter(|s| !s.is_empty()),
        ) {
            match scrape_hidden_profile(
                app,
                bridge,
                PlatformId::Gemini,
                &record.id,
                PathBuf::from(profile_dir),
                16_000,
            )
            .await
            {
                Ok(payload) => {
                    if payload.get("authRequired").and_then(|v| v.as_bool()) == Some(true) {
                        return make_error_quota(
                            record,
                            AccountStatus::AuthRequired,
                            "Google 会话已过期，请点击「重登」重新登录",
                        );
                    }
                    let url = payload.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    let body_text = payload
                        .get("bodyText")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let profile_hint = payload.get("profileHint").and_then(|v| v.as_str());
                    if let Some(q) =
                        parse_gemini_usage_page_text(record, url, body_text, profile_hint)
                    {
                        return q;
                    }
                    return make_error_quota(
                        record,
                        AccountStatus::Error,
                        "未能从 gemini.google.com/usage 解析出 5h/周用量，请点击「重登」检查页面",
                    );
                }
                Err(err) => {
                    if let Some(cookie) = creds
                        .and_then(|c| c.cookie_header.as_deref())
                        .filter(|s| !s.is_empty())
                    {
                        if self.is_cookie_expired(cookie).await {
                            return make_error_quota(
                                record,
                                AccountStatus::AuthRequired,
                                "Google 会话已过期，请点击「重登」重新登录",
                            );
                        }
                    }
                    return make_error_quota(record, AccountStatus::Error, err);
                }
            }
        }

        let Some(cookie) = creds
            .and_then(|c| c.cookie_header.as_deref())
            .filter(|s| !s.is_empty())
        else {
            return make_error_quota(
                record,
                AccountStatus::AuthRequired,
                "未绑定独立 WebView2 会话，请点击「重登」登录 Google 账号",
            );
        };

        if self.is_cookie_expired(cookie).await {
            return make_error_quota(
                record,
                AccountStatus::AuthRequired,
                "Google 会话已过期，请点击「重登」重新登录",
            );
        }

        make_error_quota(
            record,
            AccountStatus::Error,
            "请通过独立 WebView2 登录窗口同步 Gemini Usage limits",
        )
    }
}

impl GeminiProvider {
    async fn is_cookie_expired(&self, cookie: &str) -> bool {
        let Ok(resp) = self
            .client
            .get("https://gemini.google.com/usage")
            .header("Cookie", cookie)
            .send()
            .await
        else {
            return false;
        };
        let status = resp.status().as_u16();
        let loc = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        (status == 302 || status == 303 || status == 401)
            || loc.contains("accounts.google.com")
            || loc.contains("ServiceLogin")
    }
}

pub fn parse_gemini_usage_page_text(
    record: &StoredAccountRecord,
    url: &str,
    body_text: &str,
    profile_hint: Option<&str>,
) -> Option<AccountQuota> {
    let url_lower = url.to_lowercase();
    let lower = body_text.to_lowercase();

    if url_lower.contains("accounts.google.com") || url_lower.contains("servicelogin") {
        return Some(make_error_quota(
            record,
            AccountStatus::AuthRequired,
            "Google 会话已过期，请点击「重登」重新登录",
        ));
    }

    let lines: Vec<String> = body_text
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    if lines.is_empty() {
        return None;
    }

    let (tier, plan_label) = detect_gemini_plan(&lines, record.tier, &record.plan_label);

    let current_window = extract_usage_window(
        &lines,
        &["current usage", "当前用量", "当前使用", "5小时", "5-hour"],
    );

    let weekly_window = extract_usage_window(
        &lines,
        &["weekly limit", "每周限额", "周限额", "weekly usage"],
    );

    if current_window.is_none() && weekly_window.is_none() {
        if lower.contains("sign in") && !lower.contains("usage limits") {
            return Some(make_error_quota(
                record,
                AccountStatus::AuthRequired,
                "Google 会话已过期，请点击「重登」重新登录",
            ));
        }
        return None;
    }

    let mut sub_pools = Vec::new();
    let mut primary_pct = 100u8;
    let mut primary_reset = "运行正常".to_string();

    if let Some((used_pct, reset_opt)) = &current_window {
        let rem = clamp_pct(100.0 - *used_pct);
        let reset_text = reset_opt
            .clone()
            .unwrap_or_else(|| "5小时滑动重置".to_string());
        primary_pct = rem;
        primary_reset = reset_text.clone();
        sub_pools.push(SubPool {
            name: "5小时当前窗口 (Current usage)".to_string(),
            pct: rem,
            detail: Some(format!("已使用 {}%", clamp_pct(*used_pct))),
            reset_in: Some(reset_text),
        });
    }

    if let Some((used_pct, reset_opt)) = &weekly_window {
        let rem = clamp_pct(100.0 - *used_pct);
        let reset_text = reset_opt
            .clone()
            .unwrap_or_else(|| "每周周期重置".to_string());
        if current_window.is_none() {
            primary_pct = rem;
            primary_reset = reset_text.clone();
        }
        sub_pools.push(SubPool {
            name: "7天周限额 (Weekly limit)".to_string(),
            pct: rem,
            detail: Some(format!("已使用 {}%", clamp_pct(*used_pct))),
            reset_in: Some(reset_text),
        });
    }

    let extra_balance = extract_extra_usage(&lines);

    let email = if record.email.is_empty() || record.email == "account@local" {
        profile_hint
            .and_then(find_email_in_text)
            .or_else(|| find_email_in_text(body_text))
            .unwrap_or_else(|| record.email.clone())
    } else {
        record.email.clone()
    };

    Some(AccountQuota {
        id: record.id.clone(),
        platform: PlatformId::Gemini,
        alias: record.alias.clone(),
        email,
        tier,
        plan_label,
        primary_pct,
        reset_in: primary_reset,
        sub_pools,
        extra_balance,
        status: AccountStatus::Ok,
        status_message: None,
        auth_method: record.auth_method,
        last_synced_at: Some(now_hh_mm()),
        is_demo: false,
    })
}

fn detect_gemini_plan(
    lines: &[String],
    fallback_tier: SubscriptionTier,
    fallback_label: &str,
) -> (SubscriptionTier, String) {
    for (idx, line) in lines.iter().enumerate() {
        let low = line.to_lowercase();
        if low.starts_with("get ") || low.contains("upgrade") || low.contains("升级") {
            continue;
        }
        if low.contains("usage limits") || low.contains("使用限额") || low.contains("用量限额") {
            let window = lines[idx..lines.len().min(idx + 3)].join(" ").to_lowercase();
            if window.contains("ultra") {
                return (SubscriptionTier::Paid, "AI Ultra".to_string());
            }
            if window.contains("pro") {
                return (SubscriptionTier::Paid, "AI Pro".to_string());
            }
            if window.contains("plus") {
                return (SubscriptionTier::Paid, "AI Plus".to_string());
            }
            if window.contains("free") {
                return (SubscriptionTier::Free, "Free".to_string());
            }
        }
        if low == "ultra" || low == "ai ultra" {
            return (SubscriptionTier::Paid, "AI Ultra".to_string());
        }
        if low == "pro" || low == "ai pro" {
            return (SubscriptionTier::Paid, "AI Pro".to_string());
        }
        if low == "plus" || low == "ai plus" {
            return (SubscriptionTier::Paid, "AI Plus".to_string());
        }
    }
    (fallback_tier, fallback_label.to_string())
}

fn extract_usage_window(lines: &[String], anchors: &[&str]) -> Option<(f64, Option<String>)> {
    for (idx, line) in lines.iter().enumerate() {
        let low = line.to_lowercase();
        if !anchors.iter().any(|a| low.contains(a)) {
            continue;
        }
        let end = (idx + 4).min(lines.len());
        let mut used_pct: Option<f64> = None;
        let mut reset_str: Option<String> = None;

        for candidate in &lines[idx..end] {
            if candidate != line {
                let c_low = candidate.to_lowercase();
                if c_low.contains("weekly limit")
                    || c_low.contains("每周限额")
                    || c_low.contains("extra usage")
                    || c_low.contains("额外用量")
                {
                    break;
                }
            }
            if used_pct.is_none() {
                used_pct = extract_percentage(candidate);
            }
            if reset_str.is_none() {
                reset_str = extract_reset_line(candidate);
            }
        }

        if let Some(pct) = used_pct {
            return Some((pct, reset_str));
        }
    }
    None
}

pub fn extract_percentage(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'%' {
            let mut start = i;
            while start > 0 {
                let ch = bytes[start - 1];
                if ch.is_ascii_digit() || ch == b'.' {
                    start -= 1;
                } else if ch == b' ' && start == i {
                    start -= 1;
                } else {
                    break;
                }
            }
            let num_str = text[start..i].trim();
            if let Ok(val) = num_str.parse::<f64>() {
                if (0.0..=100.0).contains(&val) {
                    return Some(val);
                }
            }
        }
    }
    None
}

fn extract_reset_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let low = trimmed.to_lowercase();
    if low.starts_with("resets at ") {
        return Some(format!("{} 重置", trimmed[10..].trim()));
    }
    if low.starts_with("resets ") {
        return Some(format!("{} 重置", trimmed[7..].trim()));
    }
    if low.contains("重置") {
        return Some(trimmed.to_string());
    }
    None
}

fn extract_extra_usage(lines: &[String]) -> Option<ExtraBalance> {
    for (idx, line) in lines.iter().enumerate() {
        let low = line.to_lowercase();
        if low.contains("extra usage") || low.contains("额外用量") {
            let segment = lines[idx..lines.len().min(idx + 3)].join(" ");
            let seg_low = segment.to_lowercase();
            if seg_low.contains("off") || seg_low.contains("关闭") || seg_low.contains("未开启") {
                return None;
            }
            let value_line = lines
                .get(idx + 1)
                .map(|s| s.trim_end_matches('>').trim())
                .unwrap_or("已开启");
            if !value_line.is_empty() && value_line != ">" {
                return Some(ExtraBalance {
                    label: "额外用量 (Extra usage)".to_string(),
                    amount: value_line.to_string(),
                    note: Some("套餐外按量额度".to_string()),
                });
            }
        }
    }
    None
}

pub fn find_email_in_text(text: &str) -> Option<String> {
    for token in text.split(|c: char| {
        c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '(' | ')' | ',' | ';')
    }) {
        if token.contains('@') && token.contains('.') && token.len() >= 5 && token.len() <= 80 {
            let parts: Vec<&str> = token.split('@').collect();
            if parts.len() == 2 && !parts[0].is_empty() && parts[1].contains('.') {
                return Some(token.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AuthMethod;

    fn sample_gem_record() -> StoredAccountRecord {
        StoredAccountRecord {
            id: "gem-1".into(),
            platform: PlatformId::Gemini,
            alias: "主力号".into(),
            email: "account@local".into(),
            tier: SubscriptionTier::Free,
            plan_label: "Free".into(),
            auth_method: AuthMethod::WebviewSession,
            profile_dir: Some("webview_profiles/gem-1".into()),
            is_demo: false,
            encrypted_secret: None,
            cached_quota: None,
        }
    }

    #[test]
    fn parses_official_usage_limits_english_screenshot() {
        let rec = sample_gem_record();
        let text = r#"
Usage limits
PRO
Your plan's limits determine how much you can use Gemini over time. Advanced models and features can take up more usage. Learn more
Updated just now
Current usage
33% used
Resets at 3:30 PM
Weekly limit
2% used
Resets Nov 25 at 12:30 AM
Get 5x more usage with AI Ultra
£79.99/month
Upgrade
Usage settings
Extra usage
Off
"#;
        let q = parse_gemini_usage_page_text(
            &rec,
            "https://gemini.google.com/usage",
            text,
            Some("Google Account: k.sbeil@example.com"),
        )
        .expect("should parse official usage limits");

        assert_eq!(q.status, AccountStatus::Ok);
        assert_eq!(q.plan_label, "AI Pro");
        assert_eq!(q.tier, SubscriptionTier::Paid);
        assert_eq!(q.primary_pct, 67);
        assert_eq!(q.reset_in, "3:30 PM 重置");
        assert_eq!(q.sub_pools.len(), 2);
        assert_eq!(q.sub_pools[0].pct, 67);
        assert_eq!(q.sub_pools[1].pct, 98);
        assert_eq!(
            q.sub_pools[1].reset_in.as_deref(),
            Some("Nov 25 at 12:30 AM 重置")
        );
        assert!(q.extra_balance.is_none());
        assert_eq!(q.email, "k.sbeil@example.com");
    }

    #[test]
    fn parses_official_usage_limits_chinese() {
        let rec = sample_gem_record();
        let text = r#"
使用限额 PRO
当前用量
已使用 45%
将在 18:20 重置
每周限额
已使用 12%
将在 11月28日 重置
额外用量
关闭
"#;
        let q = parse_gemini_usage_page_text(&rec, "https://gemini.google.com/usage", text, None)
            .expect("should parse Chinese UI");
        assert_eq!(q.plan_label, "AI Pro");
        assert_eq!(q.primary_pct, 55);
        assert_eq!(q.sub_pools[1].pct, 88);
        assert!(q.extra_balance.is_none());
    }

    #[test]
    fn parses_extra_usage_when_enabled() {
        let rec = sample_gem_record();
        let text = r#"
Usage limits ULTRA
Current usage
10% used
Resets at 4:00 PM
Weekly limit
5% used
Resets Dec 1
Extra usage
$14.20 remaining
"#;
        let q = parse_gemini_usage_page_text(&rec, "https://gemini.google.com/usage", text, None)
            .expect("should parse");
        assert_eq!(q.plan_label, "AI Ultra");
        assert_eq!(q.primary_pct, 90);
        assert_eq!(q.extra_balance.unwrap().amount, "$14.20 remaining");
    }

    #[test]
    fn detects_expired_google_login_redirect() {
        let rec = sample_gem_record();
        let q = parse_gemini_usage_page_text(
            &rec,
            "https://accounts.google.com/v3/signin/ServiceLogin",
            "Sign in",
            None,
        )
        .expect("should return AuthRequired");
        assert_eq!(q.status, AccountStatus::AuthRequired);
    }
}
