use crate::local_discovery::discover_antigravity_offline_candidates;
use crate::models::{
    AccountQuota, AccountStatus, AuthMethod, PlatformId, SecretCredentials, StoredAccountRecord,
    SubPool, SubscriptionTier,
};
use crate::providers::{
    clamp_pct, format_reset_countdown, make_error_quota, now_hh_mm, ProviderContext, QuotaProvider,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeSet, HashSet};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct LocalServerInfo {
    pub pid: u32,
    pub process_name: String,
    pub csrf_token: String,
    pub ports: Vec<u16>,
    pub priority_score: i32,
}

pub struct AntigravityProvider {
    loopback_client: Client,
    cloud_client: Client,
}

impl AntigravityProvider {
    pub fn new() -> Self {
        // Bypass system proxies so Clash/v2ray won't hijack 127.0.0.1 / [::1]
        let loopback_client = Client::builder()
            .no_proxy()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(4))
            .build()
            .unwrap_or_else(|_| Client::new());

        let cloud_client = Client::builder()
            .user_agent("antigravity/1.15.8 windows/amd64")
            .timeout(Duration::from_secs(12))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            loopback_client,
            cloud_client,
        }
    }

    async fn fetch_from_local_server(
        &self,
        record: &StoredAccountRecord,
    ) -> Result<AccountQuota, (AccountStatus, String)> {
        let servers = tokio::task::spawn_blocking(discover_local_language_servers)
            .await
            .map_err(|e| (AccountStatus::Offline, format!("本地检测任务异常: {e}")))?
            .map_err(|e| (AccountStatus::Offline, e))?;

        let rpc_paths = [
            "/exa.language_server_pb.LanguageServerService/GetUserStatus",
            "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary",
            "/exa.language_server_pb.LanguageServerService/GetCommandModelConfigs",
        ];
        let request_bodies = [
            r#"{"metadata":{"ideName":"antigravity","extensionName":"antigravity","locale":"en"}}"#,
            "{}",
        ];

        let mut total_ports = 0usize;
        let mut last_http_status: Option<u16> = None;

        for info in &servers {
            total_ports += info.ports.len();
            for port in &info.ports {
                for host in ["127.0.0.1", "[::1]"] {
                    for scheme in ["https", "http"] {
                        'path_loop: for path in &rpc_paths {
                            for body in &request_bodies {
                                let url = format!("{scheme}://{host}:{port}{path}");
                                let resp = self
                                    .loopback_client
                                    .post(&url)
                                    .header("X-Codeium-Csrf-Token", &info.csrf_token)
                                    .header("Connect-Protocol-Version", "1")
                                    .header("Content-Type", "application/json")
                                    .body(*body)
                                    .send()
                                    .await;

                                match resp {
                                    Ok(r) => {
                                        let status = r.status();
                                        if status.is_success() {
                                            if let Ok(json) = r.json::<Value>().await {
                                                if let Some(quota) =
                                                    parse_antigravity_quota(record, &json)
                                                {
                                                    return Ok(quota);
                                                }
                                            }
                                        } else {
                                            last_http_status = Some(status.as_u16());
                                        }
                                    }
                                    Err(e) => {
                                        if e.is_connect() || e.is_timeout() {
                                            break 'path_loop;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let status_note = last_http_status
            .map(|c| format!("（HTTP {c}）"))
            .unwrap_or_default();

        Err((
            AccountStatus::Offline,
            format!(
                "已扫描 {} 个 language_server 进程（共 {} 个端口）但未匹配到接口{status_note}",
                servers.len(),
                total_ports
            ),
        ))
    }

    async fn load_code_assist(
        &self,
        base_url: &str,
        token: &str,
    ) -> (Option<String>, Option<String>) {
        let url = format!("{base_url}/v1internal:loadCodeAssist");
        let Ok(resp) = self
            .cloud_client
            .post(&url)
            .bearer_auth(token)
            .header("Content-Type", "application/json")
            .body(r#"{"metadata":{"ideType":"ANTIGRAVITY","platform":"WINDOWS","pluginType":"GEMINI"}}"#)
            .send()
            .await
        else {
            return (None, None);
        };

        if !resp.status().is_success() {
            return (None, None);
        }

        let json: Value = resp.json().await.unwrap_or(Value::Null);
        let project_id = json
            .get("cloudaicompanionProject")
            .and_then(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.get("id").and_then(|id| id.as_str()).map(str::to_string))
            })
            .filter(|s| !s.is_empty());

        let tier_name = json
            .pointer("/paidTier/name")
            .or_else(|| json.pointer("/paidTier/id"))
            .or_else(|| json.pointer("/currentTier/name"))
            .or_else(|| json.pointer("/currentTier/id"))
            .and_then(|v| v.as_str())
            .map(str::to_string);

        (project_id, tier_name)
    }

    async fn fetch_from_oauth(
        &self,
        record: &StoredAccountRecord,
        token: &str,
    ) -> Result<AccountQuota, (AccountStatus, String)> {
        let base_urls = [
            "https://cloudcode-pa.googleapis.com",
            "https://daily-cloudcode-pa.sandbox.googleapis.com",
        ];

        let mut saw_auth_error = false;
        let mut last_err = "Cloud Code API 未返回有效数据".to_string();

        for base in base_urls {
            let (project_id, assist_tier) = self.load_code_assist(base, token).await;
            let req_body = match project_id.as_deref() {
                Some(pid) => serde_json::json!({ "project": pid }).to_string(),
                None => "{}".to_string(),
            };

            let endpoints = [
                (format!("{base}/v1internal:fetchAvailableModels"), req_body),
                (
                    format!("{base}/v1internal:retrieveUserQuotaSummary"),
                    "{}".to_string(),
                ),
            ];

            for (url, body_str) in endpoints {
                let resp = self
                    .cloud_client
                    .post(&url)
                    .bearer_auth(token)
                    .header("Content-Type", "application/json")
                    .body(body_str)
                    .send()
                    .await;

                match resp {
                    Ok(r) => {
                        let status = r.status();
                        if status.as_u16() == 401 || status.as_u16() == 403 {
                            saw_auth_error = true;
                            continue;
                        }
                        if status.is_success() {
                            if let Ok(json) = r.json::<Value>().await {
                                if let Some(mut quota) = parse_antigravity_quota(record, &json) {
                                    if let Some(ref tier_raw) = assist_tier {
                                        let (t, lbl) = normalize_agy_plan(
                                            tier_raw,
                                            quota.tier,
                                            &quota.plan_label,
                                        );
                                        quota.tier = t;
                                        quota.plan_label = lbl;
                                    }
                                    if quota.email.is_empty()
                                        || quota.email == "local@antigravity"
                                        || quota.email == "account@local"
                                    {
                                        if let Some(email) =
                                            fetch_google_userinfo_email(&self.cloud_client, token)
                                                .await
                                        {
                                            quota.email = email;
                                        }
                                    }
                                    return Ok(quota);
                                }
                            }
                        } else {
                            last_err = format!("Cloud Code API 返回 HTTP {}", status.as_u16());
                        }
                    }
                    Err(e) => {
                        last_err = format!("请求失败: {e}");
                    }
                }
            }
        }

        if saw_auth_error {
            return Err((
                AccountStatus::AuthRequired,
                "Antigravity 登录态已过期，请确保 Antigravity 客户端已登录并点击刷新/重登".to_string(),
            ));
        }

        Err((AccountStatus::Error, last_err))
    }
}

#[async_trait]
impl QuotaProvider for AntigravityProvider {
    fn platform(&self) -> PlatformId {
        PlatformId::Antigravity
    }

    async fn fetch_quota(
        &self,
        record: &StoredAccountRecord,
        creds: Option<&SecretCredentials>,
        ctx: &ProviderContext,
    ) -> AccountQuota {
        match record.auth_method {
            AuthMethod::OauthToken => {
                let initial_tok = creds
                    .and_then(|c| c.access_token.as_deref())
                    .filter(|s| !s.is_empty());

                let mut tried_tokens = HashSet::new();

                if let Some(tok) = initial_tok {
                    tried_tokens.insert(tok.to_string());
                    match self.fetch_from_oauth(record, tok).await {
                        Ok(q) => return q,
                        Err((AccountStatus::AuthRequired, _)) => {
                            // 1. Try refreshing Google OAuth token if refresh_token + client_id exist
                            if let (Some(c), Some(rt)) = (
                                creds,
                                creds
                                    .and_then(|c| c.refresh_token.as_deref())
                                    .filter(|s| !s.is_empty()),
                            ) {
                                if let Some(cid) =
                                    c.oauth_client_id.as_deref().filter(|s| !s.is_empty())
                                {
                                    if let Ok(new_tok) = refresh_google_oauth_token(
                                        &self.cloud_client,
                                        rt,
                                        cid,
                                        c.oauth_client_secret.as_deref(),
                                    )
                                    .await
                                    {
                                        let mut updated_creds = c.clone();
                                        updated_creds.access_token = Some(new_tok.clone());
                                        if let Some(store) = ctx.store.as_ref() {
                                            let _ = store.update_credentials(
                                                &record.id,
                                                &updated_creds,
                                                None,
                                            );
                                        }
                                        if let Ok(q) = self.fetch_from_oauth(record, &new_tok).await
                                        {
                                            return q;
                                        }
                                    }
                                }
                            }

                            // 2. For the auto-discovered local account, try live server and other offline candidates
                            if record.id.starts_with("agy-local") {
                                if let Ok(q) = self.fetch_from_local_server(record).await {
                                    return q;
                                }
                                if let Ok(Ok(candidates)) =
                                    tokio::task::spawn_blocking(discover_antigravity_offline_candidates)
                                        .await
                                {
                                    for cand in candidates {
                                        let Some(new_tok) = cand.creds.access_token.as_deref() else {
                                            continue;
                                        };
                                        if !tried_tokens.insert(new_tok.to_string()) {
                                            continue;
                                        }
                                        if let Ok(mut q) =
                                            self.fetch_from_oauth(record, new_tok).await
                                        {
                                            if q.email == "local@antigravity" || q.email.is_empty()
                                            {
                                                q.email = cand.email;
                                            }
                                            if let Some(store) = ctx.store.as_ref() {
                                                let _ = store.update_credentials(
                                                    &record.id,
                                                    &cand.creds,
                                                    None,
                                                );
                                            }
                                            return q;
                                        }
                                    }
                                }
                            }

                            return make_error_quota(
                                record,
                                AccountStatus::AuthRequired,
                                "Antigravity OAuth Token 已过期，请点击「重登」或打开 Antigravity 刷新登录态",
                            );
                        }
                        Err((st, msg)) => return make_error_quota(record, st, msg),
                    }
                }
                make_error_quota(
                    record,
                    AccountStatus::AuthRequired,
                    "缺少 Antigravity OAuth Token",
                )
            }
            _ => {
                // 1. Try running language_server first
                match self.fetch_from_local_server(record).await {
                    Ok(q) => q,
                    Err((_st, local_err)) => {
                        // 2. Fallback to offline state.vscdb / ~/.gemini/oauth_creds.json candidates
                        if let Ok(Ok(candidates)) =
                            tokio::task::spawn_blocking(discover_antigravity_offline_candidates)
                                .await
                        {
                            let mut last_oauth_err: Option<(AccountStatus, String)> = None;
                            for cand in candidates {
                                if let Some(tok) = cand.creds.access_token.as_deref() {
                                    match self.fetch_from_oauth(record, tok).await {
                                        Ok(mut q) => {
                                            if q.email == "local@antigravity" || q.email.is_empty()
                                            {
                                                q.email = cand.email;
                                            }
                                            q.status_message =
                                                Some(format!("离线读取：{}", cand.source_desc));
                                            if let Some(store) = ctx.store.as_ref() {
                                                let _ = store.update_credentials(
                                                    &record.id,
                                                    &cand.creds,
                                                    None,
                                                );
                                            }
                                            return q;
                                        }
                                        Err(e) => {
                                            last_oauth_err = Some(e);
                                        }
                                    }
                                }
                            }
                            if let Some((st, oauth_err)) = last_oauth_err {
                                return make_error_quota(record, st, oauth_err);
                            }
                        }
                        make_error_quota(record, AccountStatus::Offline, local_err)
                    }
                }
            }
        }
    }
}

pub async fn refresh_google_oauth_token(
    client: &Client,
    refresh_token: &str,
    client_id: &str,
    client_secret: Option<&str>,
) -> Result<String, String> {
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    if let Some(sec) = client_secret.filter(|s| !s.trim().is_empty()) {
        form.push(("client_secret", sec));
    }

    let resp = client
        .post("https://oauth2.googleapis.com/token")
        .header("Accept", "application/json")
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("刷新 Google Token 失败: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Google Token 刷新返回 HTTP {}", resp.status().as_u16()));
    }
    let json: Value = resp
        .json()
        .await
        .map_err(|e| format!("解析 Google Token 响应失败: {e}"))?;
    json.get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Google Token 刷新响应未包含 access_token".to_string())
}

pub async fn fetch_google_userinfo_email(client: &Client, token: &str) -> Option<String> {
    let resp = client
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .bearer_auth(token)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: Value = resp.json().await.ok()?;
    json.get("email")
        .and_then(|v| v.as_str())
        .filter(|s| s.contains('@'))
        .map(str::to_string)
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WinProcessRow {
    process_id: Option<u32>,
    name: Option<String>,
    command_line: Option<String>,
}

pub fn discover_local_language_servers() -> Result<Vec<LocalServerInfo>, String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let ps_cmd = "Get-CimInstance Win32_Process | Where-Object { $_.Name -like 'language_server*' -or $_.CommandLine -like '*csrf_token*' -or $_.CommandLine -like '*csrf-token*' } | Select-Object ProcessId, Name, CommandLine | ConvertTo-Json -Compress";
        let output = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", ps_cmd])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| format!("查询本地进程失败: {e}"))?;

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() || stdout == "null" {
            return Err("未检测到运行中的 language_server 进程".to_string());
        }

        let rows: Vec<WinProcessRow> = if stdout.starts_with('[') {
            serde_json::from_str(&stdout).unwrap_or_default()
        } else if let Ok(single) = serde_json::from_str::<WinProcessRow>(&stdout) {
            vec![single]
        } else {
            Vec::new()
        };

        let netstat_output = std::process::Command::new("netstat")
            .args(["-ano", "-p", "TCP"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();

        let mut servers = Vec::new();
        for row in rows {
            let Some(pid) = row.process_id else { continue };
            let Some(cmd) = row.command_line else { continue };
            let name = row.name.unwrap_or_else(|| "language_server".to_string());

            let Some(csrf_token) = extract_flag_value(&cmd, &["--csrf_token", "--csrf-token"])
            else {
                continue;
            };

            let mut ports_set = BTreeSet::new();
            for p in parse_netstat_listening_ports(&netstat_output, pid) {
                ports_set.insert(p);
            }
            for p in query_listening_ports_ps(pid) {
                ports_set.insert(p);
            }
            for flag in [
                "--extension_server_port",
                "--extension-server-port",
                "--server_port",
                "--port",
                "--https_port",
                "--http_port",
                "--lsp_port",
            ] {
                if let Some(port) =
                    extract_flag_value(&cmd, &[flag]).and_then(|s| s.parse::<u16>().ok())
                {
                    ports_set.insert(port);
                }
            }

            if ports_set.is_empty() {
                continue;
            }

            let cmd_low = cmd.to_lowercase();
            let mut score = 0;
            if cmd_low.contains("antigravity") || cmd_low.contains("agy") {
                score += 10;
            }
            if cmd_low.contains("jetski") || cmd_low.contains("app_data_dir") {
                score += 5;
            }
            if name.to_lowercase().contains("language_server") {
                score += 2;
            }

            servers.push(LocalServerInfo {
                pid,
                process_name: name,
                csrf_token,
                ports: ports_set.into_iter().collect(),
                priority_score: score,
            });
        }

        servers.sort_by(|a, b| b.priority_score.cmp(&a.priority_score));

        if servers.is_empty() {
            return Err("未在运行进程中找到有效的 CSRF Token 或监听端口".to_string());
        }

        Ok(servers)
    }

    #[cfg(not(windows))]
    {
        Err("仅支持在 Windows 上探测本地 language_server.exe".to_string())
    }
}

#[cfg(windows)]
fn query_listening_ports_ps(pid: u32) -> Vec<u16> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let cmd = format!(
        "Get-NetTCPConnection -OwningProcess {pid} -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty LocalPort | ConvertTo-Json -Compress"
    );
    let Ok(out) = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &cmd])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return Vec::new();
    };

    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() || s == "null" {
        return Vec::new();
    }
    if let Ok(port) = s.parse::<u16>() {
        return vec![port];
    }
    serde_json::from_str::<Vec<u16>>(&s).unwrap_or_default()
}

pub fn parse_netstat_listening_ports(output: &str, target_pid: u32) -> Vec<u16> {
    let mut ports = BTreeSet::new();
    let pid_str = target_pid.to_string();

    for line in output.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 4 {
            continue;
        }
        if !cols[0].eq_ignore_ascii_case("TCP") {
            continue;
        }
        if cols.last().copied() != Some(pid_str.as_str()) {
            continue;
        }
        let state = cols[3];
        if !state.eq_ignore_ascii_case("LISTENING") {
            continue;
        }
        let local_addr = cols[1];
        if let Some((_, port_part)) = local_addr.rsplit_once(':') {
            if let Ok(port) = port_part.parse::<u16>() {
                ports.insert(port);
            }
        }
    }

    ports.into_iter().collect()
}

pub fn extract_flag_value(cmd: &str, flags: &[&str]) -> Option<String> {
    let tokens = split_cmd_tokens(cmd);
    for (idx, tok) in tokens.iter().enumerate() {
        for flag in flags {
            if tok == flag {
                if let Some(next) = tokens.get(idx + 1) {
                    if !next.starts_with('-') {
                        return Some(next.clone());
                    }
                }
            } else if let Some(rest) = tok.strip_prefix(&format!("{flag}=")) {
                return Some(rest.to_string());
            }
        }
    }
    None
}

fn split_cmd_tokens(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut quote_char = '"';

    for ch in cmd.chars() {
        if in_quotes {
            if ch == quote_char {
                in_quotes = false;
            } else {
                current.push(ch);
            }
        } else if ch == '"' || ch == '\'' {
            in_quotes = true;
            quote_char = ch;
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

pub fn parse_antigravity_quota(
    record: &StoredAccountRecord,
    json: &Value,
) -> Option<AccountQuota> {
    let mut gemini_pcts: Vec<(f64, Option<String>)> = Vec::new();
    let mut third_party_pcts: Vec<(f64, Option<String>)> = Vec::new();

    if let Some(groups) = json
        .get("groups")
        .or_else(|| json.pointer("/response/groups"))
        .and_then(|v| v.as_array())
    {
        for grp in groups {
            let group_label = grp
                .get("displayName")
                .or_else(|| grp.get("name"))
                .or_else(|| grp.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();

            if let Some(buckets) = grp.get("buckets").and_then(|v| v.as_array()) {
                for b in buckets {
                    let b_label = b
                        .get("displayName")
                        .or_else(|| b.get("bucketId"))
                        .or_else(|| b.get("id"))
                        .or_else(|| b.get("window"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_lowercase();
                    let rem = b
                        .get("remainingFraction")
                        .or_else(|| b.get("remaining_fraction"))
                        .and_then(|v| v.as_f64())
                        .map(|f| if f <= 1.0 { f * 100.0 } else { f });
                    let reset = b
                        .get("resetTime")
                        .or_else(|| b.get("reset_time"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string);

                    if let Some(pct) = rem {
                        let combined = format!("{group_label} {b_label}");
                        if combined.contains("claude")
                            || combined.contains("gpt")
                            || combined.contains("3p")
                            || combined.contains("anthropic")
                            || combined.contains("openai")
                            || combined.contains("weekly")
                        {
                            third_party_pcts.push((pct, reset));
                        } else {
                            gemini_pcts.push((pct, reset));
                        }
                    }
                }
            }
        }
    }

    let buckets = json
        .get("buckets")
        .or_else(|| json.get("quotaBuckets"))
        .or_else(|| json.pointer("/userQuotaSummary/buckets"))
        .and_then(|v| v.as_array());

    if let Some(arr) = buckets {
        for b in arr {
            let bucket_id = b
                .get("bucketId")
                .or_else(|| b.get("displayName"))
                .or_else(|| b.get("id"))
                .or_else(|| b.get("window"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            let rem = b
                .get("remainingFraction")
                .and_then(|v| v.as_f64())
                .map(|f| if f <= 1.0 { f * 100.0 } else { f });
            let reset = b
                .get("resetTime")
                .and_then(|v| v.as_str())
                .map(str::to_string);

            if let Some(pct) = rem {
                if bucket_id.contains("3p")
                    || bucket_id.contains("claude")
                    || bucket_id.contains("gpt")
                    || bucket_id.contains("weekly")
                {
                    third_party_pcts.push((pct, reset));
                } else {
                    gemini_pcts.push((pct, reset));
                }
            }
        }
    }

    let model_configs = json
        .pointer("/userStatus/cascadeModelConfigData/clientModelConfigs")
        .or_else(|| json.get("clientModelConfigs"))
        .or_else(|| json.get("modelConfigs"))
        .and_then(|v| v.as_array());

    if let Some(arr) = model_configs {
        for m in arr {
            let label = m
                .get("label")
                .or_else(|| m.pointer("/modelOrAlias/model"))
                .or_else(|| m.get("model"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            let q = m.get("quotaInfo");
            let reset = q
                .and_then(|qi| qi.get("resetTime"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let rem = q
                .and_then(|qi| qi.get("remainingFraction"))
                .and_then(|v| v.as_f64())
                .map(|f| if f <= 1.0 { f * 100.0 } else { f })
                .or_else(|| {
                    if reset.as_deref().is_some_and(|s| !s.is_empty()) {
                        Some(0.0)
                    } else {
                        None
                    }
                });

            if let Some(pct) = rem {
                if label.contains("claude")
                    || label.contains("gpt")
                    || label.contains("opus")
                    || label.contains("sonnet")
                {
                    third_party_pcts.push((pct, reset));
                } else {
                    gemini_pcts.push((pct, reset));
                }
            }
        }
    }

    if let Some(map) = json.get("models").and_then(|v| v.as_object()) {
        for (name, m) in map {
            let lower = name.to_lowercase();
            let q = m.get("quotaInfo");
            let reset = q
                .and_then(|qi| qi.get("resetTime"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let rem = q
                .and_then(|qi| qi.get("remainingFraction"))
                .and_then(|v| v.as_f64())
                .map(|f| if f <= 1.0 { f * 100.0 } else { f })
                .or_else(|| {
                    if reset.as_deref().is_some_and(|s| !s.is_empty()) {
                        Some(0.0)
                    } else {
                        None
                    }
                });

            if let Some(pct) = rem {
                if lower.contains("claude")
                    || lower.contains("gpt")
                    || lower.contains("opus")
                    || lower.contains("sonnet")
                {
                    third_party_pcts.push((pct, reset));
                } else {
                    gemini_pcts.push((pct, reset));
                }
            }
        }
    }

    if gemini_pcts.is_empty() && third_party_pcts.is_empty() {
        return None;
    }

    let mut sub_pools = Vec::new();
    let mut primary_pct_val: Option<f64> = None;
    let mut primary_reset: Option<String> = None;

    if !gemini_pcts.is_empty() {
        let min_gem = gemini_pcts
            .iter()
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .cloned()
            .unwrap();
        primary_pct_val = Some(min_gem.0);
        primary_reset = min_gem.1.clone();
        sub_pools.push(SubPool {
            name: "Gemini 模型池 (5h)".to_string(),
            pct: clamp_pct(min_gem.0),
            detail: None,
            reset_in: min_gem.1.as_deref().map(format_reset_countdown),
        });
    }

    if !third_party_pcts.is_empty() {
        let min_3p = third_party_pcts
            .iter()
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .cloned()
            .unwrap();
        if primary_pct_val.is_none() {
            primary_pct_val = Some(min_3p.0);
            primary_reset = min_3p.1.clone();
        }
        sub_pools.push(SubPool {
            name: "Claude / GPT 池 (周)".to_string(),
            pct: clamp_pct(min_3p.0),
            detail: None,
            reset_in: min_3p.1.as_deref().map(format_reset_countdown),
        });
    }

    let detected_email = json
        .pointer("/userStatus/email")
        .or_else(|| json.get("email"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| record.email.clone());

    let tier_str = json
        .pointer("/userStatus/planStatus/planInfo/planName")
        .or_else(|| json.pointer("/userStatus/userTier/name"))
        .or_else(|| json.get("planName"))
        .and_then(|v| v.as_str())
        .unwrap_or(&record.plan_label);

    let (tier, plan_label) = normalize_agy_plan(tier_str, record.tier, &record.plan_label);

    Some(AccountQuota {
        id: record.id.clone(),
        platform: PlatformId::Antigravity,
        alias: record.alias.clone(),
        email: detected_email,
        tier,
        plan_label,
        primary_pct: clamp_pct(primary_pct_val.unwrap_or(100.0)),
        reset_in: primary_reset
            .as_deref()
            .map(format_reset_countdown)
            .unwrap_or_else(|| "运行正常".to_string()),
        sub_pools,
        extra_balance: None,
        status: AccountStatus::Ok,
        status_message: None,
        auth_method: record.auth_method,
        last_synced_at: Some(now_hh_mm()),
        is_demo: false,
    })
}

fn normalize_agy_plan(
    raw: &str,
    fallback_tier: SubscriptionTier,
    fallback_label: &str,
) -> (SubscriptionTier, String) {
    let low = raw.to_lowercase();
    if low.contains("ultra") {
        (SubscriptionTier::Paid, "AI Ultra".to_string())
    } else if low.contains("pro") {
        (SubscriptionTier::Paid, "AI Pro".to_string())
    } else if low.contains("free") {
        (SubscriptionTier::Free, "Free".to_string())
    } else {
        (fallback_tier, fallback_label.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_record() -> StoredAccountRecord {
        StoredAccountRecord {
            id: "agy-test".into(),
            platform: PlatformId::Antigravity,
            alias: "本机反重力".into(),
            email: "dev@example.com".into(),
            tier: SubscriptionTier::Paid,
            plan_label: "AI Pro".into(),
            auth_method: AuthMethod::LocalServer,
            profile_dir: None,
            is_demo: false,
            encrypted_secret: None,
            cached_quota: None,
        }
    }

    #[test]
    fn parses_retrieve_user_quota_summary() {
        let payload = json!({
            "buckets": [
                {
                    "bucketId": "gemini-5h",
                    "remainingFraction": 0.84,
                    "resetTime": "2099-01-01T00:00:00Z"
                },
                {
                    "bucketId": "3p-weekly",
                    "remainingFraction": 0.62,
                    "resetTime": "2099-01-05T00:00:00Z"
                }
            ]
        });

        let quota = parse_antigravity_quota(&sample_record(), &payload).expect("should parse");
        assert_eq!(quota.primary_pct, 84);
        assert_eq!(quota.sub_pools.len(), 2);
        assert_eq!(quota.sub_pools[0].name, "Gemini 模型池 (5h)");
        assert_eq!(quota.sub_pools[0].pct, 84);
        assert_eq!(quota.sub_pools[1].name, "Claude / GPT 池 (周)");
        assert_eq!(quota.sub_pools[1].pct, 62);
    }

    #[test]
    fn parses_retrieve_user_quota_summary_groups() {
        let payload = json!({
            "response": {
                "groups": [
                    {
                        "displayName": "Gemini Models",
                        "buckets": [
                            {
                                "displayName": "Gemini 5h",
                                "window": "5h",
                                "remainingFraction": 0.78,
                                "resetTime": "2099-01-01T05:00:00Z"
                            }
                        ]
                    },
                    {
                        "displayName": "Claude and GPT models",
                        "buckets": [
                            {
                                "displayName": "Claude/GPT weekly",
                                "window": "weekly",
                                "remainingFraction": 0.54,
                                "resetTime": "2099-01-07T00:00:00Z"
                            }
                        ]
                    }
                ]
            }
        });

        let quota =
            parse_antigravity_quota(&sample_record(), &payload).expect("should parse groups");
        assert_eq!(quota.primary_pct, 78);
        assert_eq!(quota.sub_pools.len(), 2);
        assert_eq!(quota.sub_pools[0].pct, 78);
        assert_eq!(quota.sub_pools[1].pct, 54);
    }

    #[test]
    fn parses_get_user_status_models() {
        let payload = json!({
            "userStatus": {
                "email": "user@example.com",
                "userTier": { "name": "Google AI Ultra" },
                "cascadeModelConfigData": {
                    "clientModelConfigs": [
                        {
                            "label": "Gemini 3.0 Pro",
                            "quotaInfo": { "remainingFraction": 0.72 }
                        },
                        {
                            "label": "Claude 4 Sonnet",
                            "quotaInfo": { "remainingFraction": 0.41 }
                        }
                    ]
                }
            }
        });

        let quota = parse_antigravity_quota(&sample_record(), &payload).expect("should parse");
        assert_eq!(quota.email, "user@example.com");
        assert_eq!(quota.plan_label, "AI Ultra");
        assert_eq!(quota.primary_pct, 72);
        assert_eq!(quota.sub_pools[1].pct, 41);
    }

    #[test]
    fn parses_netstat_listening_ports() {
        let sample = r#"
Active Connections

  Proto  Local Address          Foreign Address        State           PID
  TCP    127.0.0.1:51121        0.0.0.0:0              LISTENING       12345
  TCP    [::1]:51122            [::]:0                 LISTENING       12345
  TCP    127.0.0.1:51123        127.0.0.1:9999         ESTABLISHED     12345
  TCP    127.0.0.1:60000        0.0.0.0:0              LISTENING       99999
"#;
        let ports = parse_netstat_listening_ports(sample, 12345);
        assert_eq!(ports, vec![51121, 51122]);
    }

    #[test]
    fn treats_proto3_omitted_fraction_with_reset_time_as_zero() {
        let payload = json!({
            "models": {
                "gemini-2.5-pro": {
                    "quotaInfo": { "resetTime": "2099-01-01T00:00:00Z" }
                },
                "claude-4-sonnet": {
                    "quotaInfo": { "remainingFraction": 0.55, "resetTime": "2099-01-03T00:00:00Z" }
                }
            }
        });

        let quota = parse_antigravity_quota(&sample_record(), &payload).expect("should parse");
        assert_eq!(quota.primary_pct, 0);
        assert_eq!(quota.sub_pools[0].pct, 0);
        assert_eq!(quota.sub_pools[1].pct, 55);
    }

    #[test]
    fn parses_cli_flags_with_quotes_and_equals() {
        let cmd = r#""C:\Program Files\Antigravity\language_server.exe" --csrf_token=tok_123 --extension_server_port 44123"#;
        assert_eq!(
            extract_flag_value(cmd, &["--csrf_token"]),
            Some("tok_123".to_string())
        );
        assert_eq!(
            extract_flag_value(cmd, &["--extension_server_port"]),
            Some("44123".to_string())
        );
    }
}
