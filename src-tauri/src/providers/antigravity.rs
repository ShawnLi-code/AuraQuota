use crate::local_discovery::discover_antigravity_offline_candidates;
use crate::models::{
    AccountQuota, AccountStatus, AuthMethod, PlatformId, SecretCredentials, StoredAccountRecord,
    SubPool, SubscriptionTier,
};
use crate::providers::{
    clamp_pct, format_reset_countdown, make_error_quota, now_hh_mm, ProviderContext, QuotaProvider,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

const ANTIGRAVITY_PERSONAL_CLIENT_ID_SHA256: &str =
    "bf00c418024ba6bf606ccdc37120976e41bc429dd1d46ecf16a729aa532626ea";
const ANTIGRAVITY_PERSONAL_SECRET_SHA256: &str =
    "1d2f041093fd95aa8995a038c711d50a7960da09a505381c09a745d6ad0ecc60";

const ANTIGRAVITY_ENTERPRISE_CLIENT_ID_SHA256: &str =
    "69dee430c4e956f6b672b99a846566d6a86814999fc3b3b050697c0d65ea80c3";
const ANTIGRAVITY_ENTERPRISE_SECRET_SHA256: &str =
    "a2ddedc84850a9c48b7e01d61700848083bf0e10fb8d6c99c496803e8b48ced0";

const GOC_PREFIX: &[u8] = b"GOCSPX-";
const GOC_SECRET_LEN: usize = 35;
const GOOGLE_CLIENT_SUFFIX: &[u8] = b".apps.googleusercontent.com";
const STREAM_OVERLAP_LEN: usize = 128;

#[derive(Debug, Clone)]
pub struct AntigravityOAuthClient {
    pub key: &'static str,
    pub client_id: String,
    pub client_secret: String,
}

static BUILTIN_AGY_OAUTH_CLIENTS: Mutex<Option<Vec<AntigravityOAuthClient>>> = Mutex::new(None);

fn is_google_secret_suffix_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

fn is_google_client_prefix_byte(b: u8) -> bool {
    b.is_ascii_digit() || b.is_ascii_lowercase() || b == b'-'
}

fn sha256_hex_lowercase(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn scan_bytes_for_target_hashes(
    buf: &[u8],
    target_hashes: &[&str],
    out: &mut HashMap<String, String>,
) {
    let min_len = GOC_SECRET_LEN.min(GOOGLE_CLIENT_SUFFIX.len() + 8);
    if buf.len() < min_len {
        return;
    }

    let mut i = 0usize;
    while i < buf.len() {
        if i + GOC_SECRET_LEN <= buf.len()
            && buf[i] == b'G'
            && &buf[i..i + GOC_PREFIX.len()] == GOC_PREFIX
        {
            let cand = &buf[i..i + GOC_SECRET_LEN];
            if cand[GOC_PREFIX.len()..]
                .iter()
                .all(|b| is_google_secret_suffix_byte(*b))
            {
                let digest = sha256_hex_lowercase(cand);
                if target_hashes.contains(&digest.as_str()) {
                    if let Ok(secret) = std::str::from_utf8(cand) {
                        out.insert(digest, secret.to_string());
                        if out.len() >= target_hashes.len() {
                            return;
                        }
                    }
                }
            }
            i += GOC_PREFIX.len();
            continue;
        }

        if i + GOOGLE_CLIENT_SUFFIX.len() <= buf.len()
            && buf[i] == b'.'
            && &buf[i..i + GOOGLE_CLIENT_SUFFIX.len()] == GOOGLE_CLIENT_SUFFIX
        {
            let mut start = i;
            let earliest = i.saturating_sub(64);
            while start > earliest && is_google_client_prefix_byte(buf[start - 1]) {
                start -= 1;
            }
            let cand = &buf[start..i + GOOGLE_CLIENT_SUFFIX.len()];
            if i.saturating_sub(start) >= 12 {
                let digest = sha256_hex_lowercase(cand);
                if target_hashes.contains(&digest.as_str()) {
                    if let Ok(cid) = std::str::from_utf8(cand) {
                        out.insert(digest, cid.to_string());
                        if out.len() >= target_hashes.len() {
                            return;
                        }
                    }
                }
            }
            i += GOOGLE_CLIENT_SUFFIX.len();
            continue;
        }

        i += 1;
    }
}

pub fn scan_reader_for_target_hashes<R: Read>(
    mut reader: R,
    target_hashes: &[&str],
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut buf = vec![0u8; (1024 * 1024) + STREAM_OVERLAP_LEN];
    let mut carry = 0usize;

    loop {
        let read_n = match reader.read(&mut buf[carry..]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        let total = carry + read_n;
        scan_bytes_for_target_hashes(&buf[..total], target_hashes, &mut out);
        if out.len() >= target_hashes.len() {
            break;
        }
        let keep = total.min(STREAM_OVERLAP_LEN);
        buf.copy_within(total - keep..total, 0);
        carry = keep;
    }

    out
}

fn push_unique_file(paths: &mut Vec<PathBuf>, p: PathBuf) {
    if p.is_file() && !paths.contains(&p) {
        paths.push(p);
    }
}

fn candidate_antigravity_binary_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(custom) = std::env::var("ANTIGRAVITY_LANGUAGE_SERVER_PATH") {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            push_unique_file(&mut paths, PathBuf::from(trimmed));
        }
    }

    #[cfg(windows)]
    {
        for env_key in ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)", "USERPROFILE"] {
            if let Ok(base) = std::env::var(env_key) {
                let root = Path::new(&base);
                let base_dirs = if env_key == "LOCALAPPDATA" {
                    vec![root.join("Programs").join("Antigravity"), root.join("Antigravity")]
                } else if env_key == "USERPROFILE" {
                    vec![root.join("AppData").join("Local").join("Programs").join("Antigravity")]
                } else {
                    vec![root.join("Antigravity")]
                };
                for dir in base_dirs {
                    push_unique_file(&mut paths, dir.join("resources").join("bin").join("language_server.exe"));
                    push_unique_file(
                        &mut paths,
                        dir.join("resources")
                            .join("app")
                            .join("extensions")
                            .join("antigravity")
                            .join("bin")
                            .join("language_server_windows_x64.exe"),
                    );
                }
            }
        }

        // Fallback: query running language_server executable path via WMI if installed in a custom drive
        if paths.is_empty() {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            if let Ok(out) = std::process::Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "(Get-CimInstance Win32_Process -Filter \"Name like 'language_server%'\").ExecutablePath",
                ])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
            {
                let text = String::from_utf8_lossy(&out.stdout);
                for line in text.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        push_unique_file(&mut paths, PathBuf::from(trimmed));
                    }
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        push_unique_file(
            &mut paths,
            PathBuf::from("/Applications/Antigravity.app/Contents/Resources/bin/language_server"),
        );
    }

    #[cfg(target_os = "linux")]
    {
        push_unique_file(
            &mut paths,
            PathBuf::from("/usr/share/antigravity/resources/bin/language_server"),
        );
        push_unique_file(
            &mut paths,
            PathBuf::from("/opt/Antigravity/resources/bin/language_server"),
        );
    }

    paths
}

/// Resolves Antigravity's built-in Desktop OAuth client credentials from environment overrides
/// or the locally installed Antigravity `language_server` binary (verified strictly by SHA-256).
pub fn resolve_builtin_antigravity_oauth_clients() -> Vec<AntigravityOAuthClient> {
    if let Ok(guard) = BUILTIN_AGY_OAUTH_CLIENTS.lock() {
        if let Some(cached) = guard.as_ref() {
            if !cached.is_empty() {
                return cached.clone();
            }
        }
    }

    let mut clients = Vec::new();

    if let (Ok(cid), Ok(sec)) = (
        std::env::var("ANTIGRAVITY_OAUTH_CLIENT_ID"),
        std::env::var("ANTIGRAVITY_OAUTH_CLIENT_SECRET"),
    ) {
        let cid_trim = cid.trim().to_string();
        let sec_trim = sec.trim().to_string();
        if !cid_trim.is_empty() && !sec_trim.is_empty() {
            clients.push(AntigravityOAuthClient {
                key: "custom_env",
                client_id: cid_trim,
                client_secret: sec_trim,
            });
        }
    }

    let targets = [
        ANTIGRAVITY_PERSONAL_CLIENT_ID_SHA256,
        ANTIGRAVITY_PERSONAL_SECRET_SHA256,
        ANTIGRAVITY_ENTERPRISE_CLIENT_ID_SHA256,
        ANTIGRAVITY_ENTERPRISE_SECRET_SHA256,
    ];
    let mut resolved_hashes: HashMap<String, String> = HashMap::new();
    for path in candidate_antigravity_binary_paths() {
        if let Ok(file) = std::fs::File::open(&path) {
            let reader = std::io::BufReader::with_capacity(1024 * 1024, file);
            for (k, v) in scan_reader_for_target_hashes(reader, &targets) {
                resolved_hashes.entry(k).or_insert(v);
            }
            if resolved_hashes.len() >= targets.len() {
                break;
            }
        }
    }

    if let (Some(cid), Some(secret)) = (
        resolved_hashes.get(ANTIGRAVITY_PERSONAL_CLIENT_ID_SHA256),
        resolved_hashes.get(ANTIGRAVITY_PERSONAL_SECRET_SHA256),
    ) {
        clients.push(AntigravityOAuthClient {
            key: "antigravity_personal",
            client_id: cid.clone(),
            client_secret: secret.clone(),
        });
    }
    if let (Some(cid), Some(secret)) = (
        resolved_hashes.get(ANTIGRAVITY_ENTERPRISE_CLIENT_ID_SHA256),
        resolved_hashes.get(ANTIGRAVITY_ENTERPRISE_SECRET_SHA256),
    ) {
        clients.push(AntigravityOAuthClient {
            key: "antigravity_enterprise",
            client_id: cid.clone(),
            client_secret: secret.clone(),
        });
    }

    if !clients.is_empty() {
        if let Ok(mut guard) = BUILTIN_AGY_OAUTH_CLIENTS.lock() {
            *guard = Some(clients.clone());
        }
    }

    clients
}

pub fn lookup_secret_for_antigravity_client_id(client_id: &str) -> Option<String> {
    let trimmed = client_id.trim();
    if trimmed.is_empty() {
        return None;
    }
    resolve_builtin_antigravity_oauth_clients()
        .into_iter()
        .find(|c| c.client_id == trimmed)
        .map(|c| c.client_secret)
}

#[derive(Debug, Clone)]
pub struct LocalServerInfo {
    pub pid: u32,
    pub process_name: String,
    pub csrf_token: String,
    pub extra_csrf_tokens: Vec<String>,
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
            .user_agent("antigravity/hub/2.9.1 windows/amd64")
            .timeout(Duration::from_secs(12))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            loopback_client,
            cloud_client,
        }
    }

    pub async fn fetch_from_local_server(
        &self,
        record: &StoredAccountRecord,
    ) -> Result<AccountQuota, (AccountStatus, String)> {
        let servers = tokio::task::spawn_blocking(discover_local_language_servers)
            .await
            .map_err(|e| (AccountStatus::Offline, format!("本地检测任务异常: {e}")))?
            .map_err(|e| (AccountStatus::Offline, e))?;

        let rpc_summary = "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
        let rpc_status = "/exa.language_server_pb.LanguageServerService/GetUserStatus";
        let rpc_configs = "/exa.language_server_pb.LanguageServerService/GetCommandModelConfigs";

        let summary_bodies = [
            r#"{"forceRefresh":true}"#,
            "{}",
            r#"{"metadata":{"ideName":"antigravity","extensionName":"antigravity","locale":"en"}}"#,
        ];
        let meta_bodies = [
            r#"{"metadata":{"ideName":"antigravity","extensionName":"antigravity","ideVersion":"unknown","locale":"en"}}"#,
            "{}",
        ];

        let mut total_ports = 0usize;
        let mut last_http_status: Option<u16> = None;
        let mut best_matching_fallback: Option<AccountQuota> = None;
        let mut best_mismatch: Option<(bool, AccountQuota)> = None;

        for info in &servers {
            total_ports += info.ports.len();
            let mut csrf_list = vec![info.csrf_token.as_str()];
            for tok in &info.extra_csrf_tokens {
                if tok != &info.csrf_token {
                    csrf_list.push(tok.as_str());
                }
            }

            for port in &info.ports {
                for host in ["127.0.0.1", "[::1]"] {
                    for scheme in ["https", "http"] {
                        let base = format!("{scheme}://{host}:{port}");
                        let mut summary_json: Option<Value> = None;
                        let mut status_json: Option<Value> = None;
                        let mut connection_dead = false;

                        for path in [rpc_summary, rpc_status, rpc_configs] {
                            if connection_dead {
                                break;
                            }
                            if path == rpc_configs
                                && (summary_json.is_some() || status_json.is_some())
                            {
                                break;
                            }
                            let bodies: &[&str] = if path == rpc_summary {
                                &summary_bodies
                            } else {
                                &meta_bodies
                            };
                            'body_loop: for body in bodies {
                                for csrf in &csrf_list {
                                    let url = format!("{base}{path}");
                                    let resp = self
                                        .loopback_client
                                        .post(&url)
                                        .header("X-Codeium-Csrf-Token", *csrf)
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
                                                    if path == rpc_summary {
                                                        if has_quota_data(&json) {
                                                            summary_json = Some(json);
                                                            break 'body_loop;
                                                        }
                                                    } else if path == rpc_status {
                                                        if has_quota_data(&json)
                                                            || has_user_meta(&json)
                                                        {
                                                            status_json = Some(json);
                                                            break 'body_loop;
                                                        }
                                                    } else if has_quota_data(&json) {
                                                        status_json = Some(json);
                                                        break 'body_loop;
                                                    }
                                                }
                                                break;
                                            } else {
                                                last_http_status = Some(status.as_u16());
                                                if status.as_u16() != 401 && status.as_u16() != 403
                                                {
                                                    break;
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            if e.is_connect() || e.is_timeout() {
                                                connection_dead = true;
                                                break 'body_loop;
                                            }
                                            break;
                                        }
                                    }
                                }
                            }
                        }

                        if summary_json.is_some() || status_json.is_some() {
                            let has_summary = summary_json.is_some();
                            if let Some(quota) = merge_antigravity_quotas(
                                record,
                                summary_json.as_ref(),
                                status_json.as_ref(),
                            ) {
                                if email_matches_strict(&record.email, &quota.email) {
                                    if has_summary {
                                        return Ok(quota);
                                    }
                                    if best_matching_fallback.is_none() {
                                        best_matching_fallback = Some(quota);
                                    }
                                } else if best_mismatch
                                    .as_ref()
                                    .is_none_or(|(prev_sum, _)| !*prev_sum && has_summary)
                                {
                                    best_mismatch = Some((has_summary, quota));
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(quota) = best_matching_fallback {
            return Ok(quota);
        }
        if let Some((_, quota)) = best_mismatch {
            return Ok(quota);
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

    pub async fn probe_account_tier(&self, token: &str) -> Option<(SubscriptionTier, String)> {
        let (_, tier_raw) = self
            .load_code_assist("https://daily-cloudcode-pa.googleapis.com", token)
            .await;
        tier_raw.map(|raw| normalize_agy_plan(&raw, SubscriptionTier::Free, "Free"))
    }

    async fn fetch_from_oauth(
        &self,
        record: &StoredAccountRecord,
        token: &str,
    ) -> Result<AccountQuota, (AccountStatus, String)> {
        // Daily -> Sandbox first (matches official language_server; Prod can report mismatched Gemini pool values)
        let base_urls = [
            "https://daily-cloudcode-pa.googleapis.com",
            "https://daily-cloudcode-pa.sandbox.googleapis.com",
            "https://cloudcode-pa.googleapis.com",
        ];

        let mut saw_auth_error = false;
        let mut last_err = "Cloud Code API 未返回有效数据".to_string();
        let mut fallback_quota: Option<AccountQuota> = None;

        for base in base_urls {
            let (project_id, assist_tier) = self.load_code_assist(base, token).await;
            let req_body = match project_id.as_deref() {
                Some(pid) => serde_json::json!({ "project": pid }).to_string(),
                None => "{}".to_string(),
            };

            let mut summary_json: Option<Value> = None;
            let mut models_json: Option<Value> = None;

            for url_body in [
                (
                    format!("{base}/v1internal:retrieveUserQuotaSummary"),
                    req_body.clone(),
                ),
                (
                    format!("{base}/v1internal:retrieveUserQuotaSummary"),
                    "{}".to_string(),
                ),
            ] {
                if summary_json.is_some() {
                    break;
                }
                if let Ok(r) = self
                    .cloud_client
                    .post(&url_body.0)
                    .bearer_auth(token)
                    .header("Content-Type", "application/json")
                    .body(url_body.1)
                    .send()
                    .await
                {
                    if r.status().as_u16() == 401 || r.status().as_u16() == 403 {
                        saw_auth_error = true;
                    } else if r.status().is_success() {
                        if let Ok(j) = r.json::<Value>().await {
                            if has_quota_data(&j) {
                                summary_json = Some(j);
                            }
                        }
                    }
                }
            }

            let models_url = format!("{base}/v1internal:fetchAvailableModels");
            match self
                .cloud_client
                .post(&models_url)
                .bearer_auth(token)
                .header("Content-Type", "application/json")
                .body(req_body.clone())
                .send()
                .await
            {
                Ok(r) => {
                    let status = r.status();
                    if status.as_u16() == 401 || status.as_u16() == 403 {
                        saw_auth_error = true;
                    } else if status.is_success() {
                        if let Ok(j) = r.json::<Value>().await {
                            if has_quota_data(&j) {
                                models_json = Some(j);
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

            // 当 RetrieveUserQuotaSummary 不可用，且 fetchAvailableModels 全返回 100% 占位或被拒时，调用 retrieveUserQuota 获取真实逐模型桶额度
            let models_all_full = models_json
                .as_ref()
                .is_some_and(all_models_look_full_placeholder);
            if summary_json.is_none() && (models_json.is_none() || models_all_full) {
                for quota_base in [base, "https://cloudcode-pa.googleapis.com"] {
                    if summary_json.is_some() {
                        break;
                    }
                    let q_url = format!("{quota_base}/v1internal:retrieveUserQuota");
                    for q_body in [req_body.clone(), "{}".to_string()] {
                        if let Ok(r) = self
                            .cloud_client
                            .post(&q_url)
                            .bearer_auth(token)
                            .header("Content-Type", "application/json")
                            .body(q_body)
                            .send()
                            .await
                        {
                            if r.status().as_u16() == 401 || r.status().as_u16() == 403 {
                                saw_auth_error = true;
                            } else if r.status().is_success() {
                                if let Ok(j) = r.json::<Value>().await {
                                    if has_quota_data(&j) {
                                        summary_json = Some(j);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            let has_structured = summary_json.is_some();
            if let Some(mut quota) =
                merge_antigravity_quotas(record, summary_json.as_ref(), models_json.as_ref())
            {
                if let Some(ref tier_raw) = assist_tier {
                    let (t, lbl) =
                        normalize_agy_plan(tier_raw, quota.tier, &quota.plan_label);
                    quota.tier = t;
                    quota.plan_label = lbl;
                }
                if quota.email.is_empty()
                    || quota.email == "local@antigravity"
                    || quota.email == "account@local"
                {
                    if let Some(email) =
                        fetch_google_userinfo_email(&self.cloud_client, token).await
                    {
                        quota.email = email;
                    }
                }
                if has_structured || !models_all_full {
                    return Ok(quota);
                }
                if fallback_quota.is_none() {
                    fallback_quota = Some(quota);
                }
            }
        }

        if let Some(quota) = fallback_quota {
            return Ok(quota);
        }

        if saw_auth_error {
            return Err((
                AccountStatus::AuthRequired,
                "Antigravity 登录态已过期，请确保 Antigravity 客户端已登录并点击刷新/重登".to_string(),
            ));
        }

        Err((AccountStatus::Error, last_err))
    }

    async fn try_fetch_oauth_or_refresh(
        &self,
        record: &StoredAccountRecord,
        creds: Option<&SecretCredentials>,
        ctx: &ProviderContext,
        tried_tokens: &mut HashSet<String>,
    ) -> Result<(AccountQuota, bool), (AccountStatus, String)> {
        let initial_tok = creds
            .and_then(|c| c.access_token.as_deref())
            .filter(|s| !s.is_empty());

        let mut last_err: Option<(AccountStatus, String)> = None;
        if let Some(tok) = initial_tok {
            if tried_tokens.insert(tok.to_string()) {
                match self.fetch_from_oauth(record, tok).await {
                    Ok(q) => return Ok((q, false)),
                    Err((AccountStatus::AuthRequired, msg)) => {
                        last_err = Some((AccountStatus::AuthRequired, msg));
                    }
                    Err(other) => return Err(other),
                }
            }
        }

        if let (Some(c), Some(raw_rt)) = (
            creds,
            creds
                .and_then(|c| c.refresh_token.as_deref())
                .filter(|s| !s.is_empty()),
        ) {
            let rt = raw_rt.split('|').next().unwrap_or(raw_rt).trim();
            if !rt.is_empty() {
                let mut client_candidates: Vec<(String, Option<String>)> = Vec::new();
                fn push_oauth_candidate(
                    list: &mut Vec<(String, Option<String>)>,
                    cid: String,
                    sec: Option<String>,
                ) {
                    let clean_id = cid.trim().to_string();
                    if clean_id.is_empty() {
                        return;
                    }
                    let clean_sec = sec.and_then(|s| {
                        let t = s.trim().to_string();
                        if t.is_empty() { None } else { Some(t) }
                    });
                    if !list
                        .iter()
                        .any(|(id, sc)| id == &clean_id && sc == &clean_sec)
                    {
                        list.push((clean_id, clean_sec));
                    }
                }

                if let Some(cid) = c.oauth_client_id.as_deref().filter(|s| !s.trim().is_empty()) {
                    let sec = c
                        .oauth_client_secret
                        .clone()
                        .filter(|s| !s.trim().is_empty())
                        .or_else(|| lookup_secret_for_antigravity_client_id(cid));
                    push_oauth_candidate(&mut client_candidates, cid.to_string(), sec);
                }

                for builtin in resolve_builtin_antigravity_oauth_clients() {
                    push_oauth_candidate(
                        &mut client_candidates,
                        builtin.client_id,
                        Some(builtin.client_secret),
                    );
                }

                for (cid, sec) in client_candidates {
                    if let Ok(refreshed) = refresh_google_oauth_token_full(
                        &self.cloud_client,
                        rt,
                        &cid,
                        sec.as_deref(),
                    )
                    .await
                    {
                        if tried_tokens.insert(refreshed.access_token.clone()) {
                            let mut updated_creds = c.clone();
                            updated_creds.access_token = Some(refreshed.access_token.clone());
                            if let Some(new_rt) = refreshed.refresh_token {
                                updated_creds.refresh_token = Some(new_rt);
                            }
                            if updated_creds
                                .oauth_client_id
                                .as_deref()
                                .map(str::trim)
                                .is_none_or(str::is_empty)
                                || updated_creds.oauth_client_secret.is_none()
                            {
                                updated_creds.oauth_client_id = Some(cid.clone());
                            }
                            if let Some(store) = ctx.store.as_ref() {
                                let _ = store.update_credentials(&record.id, &updated_creds, None);
                            }
                            match self
                                .fetch_from_oauth(record, &refreshed.access_token)
                                .await
                            {
                                Ok(q) => return Ok((q, true)),
                                Err(e) => last_err = Some(e),
                            }
                        }
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            (
                AccountStatus::AuthRequired,
                "缺少 Antigravity OAuth Token".to_string(),
            )
        }))
    }

    async fn try_recover_from_offline_candidates(
        &self,
        record: &StoredAccountRecord,
        ctx: &ProviderContext,
        tried_tokens: &mut HashSet<String>,
        attach_offline_note: bool,
    ) -> Result<AccountQuota, (AccountStatus, String)> {
        let candidates = tokio::task::spawn_blocking(discover_antigravity_offline_candidates)
            .await
            .map_err(|e| (AccountStatus::Error, format!("读取离线缓存失败: {e}")))?
            .map_err(|e| (AccountStatus::Error, e))?;

        let mut last_oauth_err: Option<(AccountStatus, String)> = None;
        for cand in candidates {
            if !email_matches_strict(&record.email, &cand.email) {
                continue;
            }
            match self
                .try_fetch_oauth_or_refresh(record, Some(&cand.creds), ctx, tried_tokens)
                .await
            {
                Ok((mut q, refreshed)) => {
                    if is_placeholder_agy_email(&q.email) {
                        q.email = cand.email;
                    }
                    if attach_offline_note {
                        q.status_message = Some(format!("离线读取：{}", cand.source_desc));
                    }
                    if !refreshed {
                        if let Some(store) = ctx.store.as_ref() {
                            let _ = store.update_credentials(&record.id, &cand.creds, None);
                        }
                    }
                    return Ok(q);
                }
                Err(e) => last_oauth_err = Some(e),
            }
        }

        Err(last_oauth_err.unwrap_or_else(|| {
            (
                AccountStatus::AuthRequired,
                "未找到匹配的离线授权凭据".to_string(),
            )
        }))
    }
}

fn is_placeholder_agy_email(email: &str) -> bool {
    let t = email.trim();
    t.is_empty() || t == "local@antigravity" || t == "account@local"
}

fn email_matches_strict(record_email: &str, found_email: &str) -> bool {
    if is_placeholder_agy_email(record_email) {
        return true;
    }
    !is_placeholder_agy_email(found_email)
        && record_email.trim().eq_ignore_ascii_case(found_email.trim())
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
        let mut tried_tokens = HashSet::new();
        match record.auth_method {
            AuthMethod::OauthToken => {
                match self
                    .try_fetch_oauth_or_refresh(record, creds, ctx, &mut tried_tokens)
                    .await
                {
                    Ok((q, _)) => q,
                    Err((AccountStatus::AuthRequired, _)) => {
                        // For auto-discovered local accounts, only renew from local server / offline cache if email matches
                        if record.id.starts_with("agy-local") {
                            if let Ok(q) = self.fetch_from_local_server(record).await {
                                if email_matches_strict(&record.email, &q.email) {
                                    return q;
                                }
                            }
                            if let Ok(q) = self
                                .try_recover_from_offline_candidates(
                                    record,
                                    ctx,
                                    &mut tried_tokens,
                                    false,
                                )
                                .await
                            {
                                return q;
                            }
                        }

                        let msg = if is_placeholder_agy_email(&record.email) {
                            "Antigravity 登录态已过期，请点击「重登」改用默认浏览器持久授权".to_string()
                        } else {
                            format!(
                                "{} 的授权已过期，请点击「重登」使用默认浏览器获取持久 Refresh Token",
                                record.email
                            )
                        };
                        make_error_quota(record, AccountStatus::AuthRequired, msg)
                    }
                    Err((st, msg)) => make_error_quota(record, st, msg),
                }
            }
            _ => {
                // 1. Try running language_server first (only accept if email matches when record already has an email)
                match self.fetch_from_local_server(record).await {
                    Ok(q) if email_matches_strict(&record.email, &q.email) => q,
                    other_res => {
                        let mismatch_email = match &other_res {
                            Ok(q) => Some(q.email.clone()),
                            Err(_) => None,
                        };
                        let local_err = match other_res {
                            Ok(q) => format!(
                                "本机当前已切至 {}；{} 会话已登出或不可用，请在 Antigravity 切回后刷新",
                                q.email, record.email
                            ),
                            Err((_, e)) => e,
                        };

                        // 2. Fallback to stored credentials (with refresh_token attempt), then matching offline candidates
                        if let Ok((q, _)) = self
                            .try_fetch_oauth_or_refresh(record, creds, ctx, &mut tried_tokens)
                            .await
                        {
                            return q;
                        }

                        if let Ok(q) = self
                            .try_recover_from_offline_candidates(
                                record,
                                ctx,
                                &mut tried_tokens,
                                true,
                            )
                            .await
                        {
                            return q;
                        }

                        let status = if mismatch_email.is_some() {
                            AccountStatus::AuthRequired
                        } else {
                            AccountStatus::Offline
                        };
                        make_error_quota(record, status, local_err)
                    }
                }
            }
        }
    }
}

pub struct RefreshedGoogleToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

pub async fn refresh_google_oauth_token_full(
    client: &Client,
    refresh_token: &str,
    client_id: &str,
    client_secret: Option<&str>,
) -> Result<RefreshedGoogleToken, String> {
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
    let access_token = json
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Google Token 刷新响应未包含 access_token".to_string())?;
    let next_refresh = json
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    Ok(RefreshedGoogleToken {
        access_token,
        refresh_token: next_refresh,
    })
}

pub async fn refresh_google_oauth_token(
    client: &Client,
    refresh_token: &str,
    client_id: &str,
    client_secret: Option<&str>,
) -> Result<String, String> {
    refresh_google_oauth_token_full(client, refresh_token, client_id, client_secret)
        .await
        .map(|t| t.access_token)
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

            let mut extra_csrf_tokens = Vec::new();
            for flag in [
                "--extension_server_csrf_token",
                "--extension-server-csrf-token",
            ] {
                if let Some(tok) = extract_flag_value(&cmd, &[flag]) {
                    if tok != csrf_token && !extra_csrf_tokens.contains(&tok) {
                        extra_csrf_tokens.push(tok);
                    }
                }
            }

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
                extra_csrf_tokens,
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum AgyCategory {
    Gemini,
    Argon,
    ClaudeGpt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AgyWindow {
    FiveHour,
    Weekly,
    Unspecified,
}

#[derive(Debug, Clone)]
struct AgyQuotaEntry {
    category: AgyCategory,
    window: AgyWindow,
    pct: f64,
    reset_time: Option<String>,
}

fn classify_agy_category(raw: &str) -> AgyCategory {
    let lower = raw.to_lowercase();
    // 必须优先匹配 Argon / Agron，因为模型名可能形如 gemini-4-argon
    if lower.contains("argon") || lower.contains("agron") {
        AgyCategory::Argon
    } else if lower.contains("claude")
        || lower.contains("gpt")
        || lower.contains("3p")
        || lower.contains("anthropic")
        || lower.contains("openai")
        || lower.contains("opus")
        || lower.contains("sonnet")
        || lower.contains("haiku")
    {
        AgyCategory::ClaudeGpt
    } else {
        AgyCategory::Gemini
    }
}

fn classify_agy_window(
    label: &str,
    reset_iso: Option<&str>,
    category: &AgyCategory,
) -> AgyWindow {
    let lower = label.to_lowercase();
    if lower.contains("weekly")
        || lower.contains("week")
        || lower.contains("7d")
        || lower.contains("7_day")
        || lower.contains("7-day")
        || lower.contains("7 day")
        || lower.contains("168h")
        || lower.contains("周")
    {
        return AgyWindow::Weekly;
    }
    if lower.contains("5h")
        || lower.contains("5_h")
        || lower.contains("5-h")
        || lower.contains("5_hour")
        || lower.contains("5-hour")
        || lower.contains("5 hour")
        || lower.contains("five_hour")
        || lower.contains("five-hour")
        || lower.contains("five hour")
        || lower.contains("fivehour")
        || lower.contains("short")
    {
        return AgyWindow::FiveHour;
    }

    if let Some(iso) = reset_iso.filter(|s| !s.is_empty()) {
        if let Ok(dt) = DateTime::parse_from_rfc3339(iso) {
            let secs = (dt.with_timezone(&Utc) - Utc::now()).num_seconds();
            if secs > 0 {
                let hours = secs as f64 / 3600.0;
                if hours > 6.0 && hours <= 192.0 {
                    return AgyWindow::Weekly;
                }
                if hours <= 6.0 {
                    return AgyWindow::FiveHour;
                }
            }
        }
    }

    match category {
        AgyCategory::Gemini => AgyWindow::FiveHour,
        AgyCategory::ClaudeGpt => AgyWindow::Weekly,
        AgyCategory::Argon => AgyWindow::Unspecified,
    }
}

fn format_agy_pool_name(category: &AgyCategory, window: AgyWindow) -> String {
    match (category, window) {
        (AgyCategory::Gemini, AgyWindow::FiveHour) => "Gemini 模型池 (5h)".to_string(),
        (AgyCategory::Gemini, AgyWindow::Weekly) => "Gemini 模型池 (周)".to_string(),
        (AgyCategory::Gemini, AgyWindow::Unspecified) => "Gemini 模型池".to_string(),
        (AgyCategory::Argon, AgyWindow::FiveHour) => "Argon 模型池 (5h)".to_string(),
        (AgyCategory::Argon, AgyWindow::Weekly) => "Argon 模型池 (周)".to_string(),
        (AgyCategory::Argon, AgyWindow::Unspecified) => "Argon 模型池".to_string(),
        (AgyCategory::ClaudeGpt, AgyWindow::FiveHour) => "Claude / GPT 池 (5h)".to_string(),
        (AgyCategory::ClaudeGpt, AgyWindow::Weekly) => "Claude / GPT 池 (周)".to_string(),
        (AgyCategory::ClaudeGpt, AgyWindow::Unspecified) => "Claude / GPT 池".to_string(),
    }
}

fn extract_object_label_tokens(obj: &Value) -> String {
    let mut parts = Vec::new();
    for key in [
        "displayName",
        "display_name",
        "title",
        "label",
        "name",
        "groupId",
        "group_id",
        "bucketId",
        "bucket_id",
        "modelId",
        "model_id",
        "id",
        "window",
        "period",
        "duration",
        "model",
    ] {
        if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                parts.push(s.to_string());
            }
        }
    }
    if let Some(m) = obj.pointer("/modelOrAlias/model").and_then(|v| v.as_str()) {
        if !m.is_empty() {
            parts.push(m.to_string());
        }
    }
    for arr_key in ["models", "modelIds", "model_ids"] {
        if let Some(arr) = obj.get(arr_key).and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(s) = item.as_str() {
                    parts.push(s.to_string());
                } else if let Some(s) = item
                    .get("model")
                    .or_else(|| item.get("id"))
                    .and_then(|v| v.as_str())
                {
                    parts.push(s.to_string());
                }
            }
        }
    }
    parts.join(" ")
}

fn extract_bucket_pct_and_reset(
    obj: &Value,
    allow_omitted_zero_with_reset: bool,
) -> Option<(f64, Option<String>)> {
    if obj.get("disabled").and_then(|v| v.as_bool()) == Some(true) {
        return None;
    }
    let reset = obj
        .get("resetTime")
        .or_else(|| obj.get("reset_time"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let oneof_value = if obj
        .pointer("/remaining/case")
        .and_then(|v| v.as_str())
        .is_some_and(|c| c == "remainingFraction" || c == "remaining_fraction")
    {
        obj.pointer("/remaining/value").and_then(|v| v.as_f64())
    } else {
        None
    };

    let rem = obj
        .get("remainingFraction")
        .or_else(|| obj.get("remaining_fraction"))
        .and_then(|v| v.as_f64())
        .or_else(|| {
            obj.pointer("/remaining/remainingFraction")
                .or_else(|| obj.pointer("/remaining/remaining_fraction"))
                .and_then(|v| v.as_f64())
        })
        .or(oneof_value)
        .map(|f| if f <= 1.0 { f * 100.0 } else { f })
        .or_else(|| {
            let has_remaining_obj = obj.get("remaining").is_some_and(|r| r.is_object());
            if (allow_omitted_zero_with_reset || has_remaining_obj) && reset.is_some() {
                Some(0.0)
            } else {
                None
            }
        });
    rem.map(|pct| (pct, reset))
}

fn collect_structured_entries(json: &Value) -> Vec<AgyQuotaEntry> {
    let mut entries = Vec::new();

    if let Some(groups) = json
        .get("groups")
        .or_else(|| json.get("quotaGroups"))
        .or_else(|| json.pointer("/response/groups"))
        .or_else(|| json.pointer("/summary/groups"))
        .or_else(|| json.pointer("/quotaSummary/groups"))
        .or_else(|| json.pointer("/userQuotaSummary/groups"))
        .or_else(|| json.pointer("/userQuotaSummary/quotaGroups"))
        .and_then(|v| v.as_array())
    {
        for grp in groups {
            let group_label = extract_object_label_tokens(grp);
            let buckets = grp
                .get("buckets")
                .or_else(|| grp.get("quotaBuckets"))
                .and_then(|v| v.as_array());
            if let Some(buckets_arr) = buckets {
                for b in buckets_arr {
                    if let Some((pct, reset)) = extract_bucket_pct_and_reset(b, false) {
                        let b_label = extract_object_label_tokens(b);
                        let combined = format!("{group_label} {b_label}");
                        let category = classify_agy_category(&combined);
                        let window =
                            classify_agy_window(&combined, reset.as_deref(), &category);
                        entries.push(AgyQuotaEntry {
                            category,
                            window,
                            pct,
                            reset_time: reset,
                        });
                    }
                }
            } else if let Some((pct, reset)) = extract_bucket_pct_and_reset(grp, false) {
                let category = classify_agy_category(&group_label);
                let window = classify_agy_window(&group_label, reset.as_deref(), &category);
                entries.push(AgyQuotaEntry {
                    category,
                    window,
                    pct,
                    reset_time: reset,
                });
            }
        }
    }

    if let Some(arr) = json
        .get("buckets")
        .or_else(|| json.get("quotaBuckets"))
        .or_else(|| json.pointer("/response/buckets"))
        .or_else(|| json.pointer("/summary/buckets"))
        .or_else(|| json.pointer("/quotaSummary/buckets"))
        .or_else(|| json.pointer("/userQuotaSummary/buckets"))
        .or_else(|| json.pointer("/userQuotaSummary/quotaBuckets"))
        .and_then(|v| v.as_array())
    {
        for b in arr {
            if let Some((pct, reset)) = extract_bucket_pct_and_reset(b, true) {
                let b_label = extract_object_label_tokens(b);
                let category = classify_agy_category(&b_label);
                let window = classify_agy_window(&b_label, reset.as_deref(), &category);
                entries.push(AgyQuotaEntry {
                    category,
                    window,
                    pct,
                    reset_time: reset,
                });
            }
        }
    }

    entries
}

fn collect_model_entries(json: &Value) -> Vec<AgyQuotaEntry> {
    let mut entries = Vec::new();

    if let Some(arr) = json
        .pointer("/userStatus/cascadeModelConfigData/clientModelConfigs")
        .or_else(|| json.get("clientModelConfigs"))
        .or_else(|| json.get("modelConfigs"))
        .and_then(|v| v.as_array())
    {
        for m in arr {
            let label = extract_object_label_tokens(m);
            let Some(q) = m.get("quotaInfo") else {
                continue;
            };
            if let Some((pct, reset)) = extract_bucket_pct_and_reset(q, true) {
                let category = classify_agy_category(&label);
                let window = classify_agy_window(&label, reset.as_deref(), &category);
                entries.push(AgyQuotaEntry {
                    category,
                    window,
                    pct,
                    reset_time: reset,
                });
            }
        }
    }

    if let Some(map) = json.get("models").and_then(|v| v.as_object()) {
        for (name, m) in map {
            let extra = extract_object_label_tokens(m);
            let combined = format!("{name} {extra}");
            let Some(q) = m.get("quotaInfo") else {
                continue;
            };
            if let Some((pct, reset)) = extract_bucket_pct_and_reset(q, true) {
                let category = classify_agy_category(&combined);
                let window = classify_agy_window(&combined, reset.as_deref(), &category);
                entries.push(AgyQuotaEntry {
                    category,
                    window,
                    pct,
                    reset_time: reset,
                });
            }
        }
    }

    entries
}

fn all_models_look_full_placeholder(json: &Value) -> bool {
    let entries = collect_model_entries(json);
    !entries.is_empty() && entries.iter().all(|e| e.pct >= 99.9)
}

pub fn has_quota_data(json: &Value) -> bool {
    !collect_structured_entries(json).is_empty() || !collect_model_entries(json).is_empty()
}

pub(crate) fn has_user_meta(json: &Value) -> bool {
    extract_email_from_json(json).is_some() || extract_tier_from_json(json).is_some()
}

fn extract_email_from_json(json: &Value) -> Option<String> {
    json.pointer("/userStatus/email")
        .or_else(|| json.pointer("/userStatus/user/email"))
        .or_else(|| json.pointer("/user/email"))
        .or_else(|| json.pointer("/account/email"))
        .or_else(|| json.get("email"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "local@antigravity")
        .map(str::to_string)
}

fn extract_tier_from_json(json: &Value) -> Option<String> {
    json.pointer("/userStatus/planStatus/planInfo/planName")
        .or_else(|| json.pointer("/userStatus/planStatus/planInfo/planTier"))
        .or_else(|| json.pointer("/userStatus/userTier/name"))
        .or_else(|| json.pointer("/userStatus/userTier/id"))
        .or_else(|| json.pointer("/userTier/name"))
        .or_else(|| json.get("planName"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub fn merge_antigravity_quotas(
    record: &StoredAccountRecord,
    summary_json: Option<&Value>,
    models_json: Option<&Value>,
) -> Option<AccountQuota> {
    let mut structured = Vec::new();
    let mut model_only = Vec::new();

    if let Some(j) = summary_json {
        structured.extend(collect_structured_entries(j));
        model_only.extend(collect_model_entries(j));
    }
    if let Some(j) = models_json {
        let same_ptr = summary_json.is_some_and(|s| std::ptr::eq(s, j));
        if !same_ptr {
            structured.extend(collect_structured_entries(j));
            model_only.extend(collect_model_entries(j));
        }
    }

    let covered_structured_categories: Vec<AgyCategory> = structured
        .iter()
        .map(|e| e.category.clone())
        .collect();

    let mut pool_map: BTreeMap<(AgyCategory, AgyWindow), (f64, Option<String>)> = BTreeMap::new();

    for entry in structured {
        pool_map
            .entry((entry.category, entry.window))
            .and_modify(|cur| {
                if entry.pct < cur.0 {
                    *cur = (entry.pct, entry.reset_time.clone());
                }
            })
            .or_insert((entry.pct, entry.reset_time));
    }

    for entry in model_only {
        // 若 RetrieveUserQuotaSummary 已按组/时间窗返回某大类额度，不让粗粒度 modelConfigs 覆盖或污染它；
        // 但若 summary 缺少某大类（例如单独的 Argon 模型池），仍补充进来。
        if covered_structured_categories.contains(&entry.category) {
            continue;
        }
        pool_map
            .entry((entry.category, entry.window))
            .and_modify(|cur| {
                if entry.pct < cur.0 {
                    *cur = (entry.pct, entry.reset_time.clone());
                }
            })
            .or_insert((entry.pct, entry.reset_time));
    }

    if pool_map.is_empty() {
        return None;
    }

    let mut sub_pools = Vec::with_capacity(pool_map.len());
    let mut gemini_primary: Option<(f64, Option<String>)> = None;
    let mut fallback_primary: Option<(f64, Option<String>)> = None;

    for ((category, window), (pct, reset)) in &pool_map {
        sub_pools.push(SubPool {
            name: format_agy_pool_name(category, *window),
            pct: clamp_pct(*pct),
            detail: None,
            reset_in: reset.as_deref().map(format_reset_countdown),
        });

        if matches!(category, AgyCategory::Gemini) {
            let replace = gemini_primary
                .as_ref()
                .is_none_or(|(cur_pct, _)| *pct < *cur_pct);
            if replace {
                gemini_primary = Some((*pct, reset.clone()));
            }
        } else {
            let replace = fallback_primary
                .as_ref()
                .is_none_or(|(cur_pct, _)| *pct < *cur_pct);
            if replace {
                fallback_primary = Some((*pct, reset.clone()));
            }
        }
    }

    let (primary_pct_val, primary_reset) = gemini_primary
        .or(fallback_primary)
        .unwrap_or((100.0, None));

    let detected_email = models_json
        .and_then(extract_email_from_json)
        .or_else(|| summary_json.and_then(extract_email_from_json))
        .unwrap_or_else(|| record.email.clone());

    let tier_str = models_json
        .and_then(extract_tier_from_json)
        .or_else(|| summary_json.and_then(extract_tier_from_json))
        .unwrap_or_else(|| record.plan_label.clone());

    let (tier, plan_label) = normalize_agy_plan(&tier_str, record.tier, &record.plan_label);

    Some(AccountQuota {
        id: record.id.clone(),
        platform: PlatformId::Antigravity,
        alias: record.alias.clone(),
        email: detected_email,
        tier,
        plan_label,
        primary_pct: clamp_pct(primary_pct_val),
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

pub fn parse_antigravity_quota(
    record: &StoredAccountRecord,
    json: &Value,
) -> Option<AccountQuota> {
    merge_antigravity_quotas(record, Some(json), None)
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

    #[test]
    fn parses_official_dual_window_gemini_and_claude_pools() {
        let now = Utc::now();
        let h4 = (now + chrono::Duration::hours(4)).to_rfc3339();
        let d6 = (now + chrono::Duration::days(6)).to_rfc3339();
        let d5 = (now + chrono::Duration::days(5)).to_rfc3339();
        let payload = serde_json::json!({
            "groups": [
                {
                    "displayName": "Gemini Models",
                    "buckets": [
                        { "bucketId": "five_hour", "displayName": "Five Hour Limit", "remainingFraction": 0.64, "resetTime": h4 },
                        { "bucketId": "weekly", "displayName": "Weekly Limit", "remainingFraction": 0.90, "resetTime": d6 }
                    ]
                },
                {
                    "displayName": "Claude and GPT models",
                    "buckets": [
                        { "bucketId": "five_hour", "displayName": "Five Hour Limit", "remainingFraction": 1.0 },
                        { "bucketId": "weekly", "displayName": "Weekly Limit", "remainingFraction": 0.99, "resetTime": d5 }
                    ]
                }
            ]
        });

        let quota = parse_antigravity_quota(&sample_record(), &payload).expect("should parse dual windows");
        assert_eq!(quota.primary_pct, 64);
        assert_eq!(quota.sub_pools.len(), 4);
        assert_eq!(quota.sub_pools[0].name, "Gemini 模型池 (5h)");
        assert_eq!(quota.sub_pools[0].pct, 64);
        assert_eq!(quota.sub_pools[1].name, "Gemini 模型池 (周)");
        assert_eq!(quota.sub_pools[1].pct, 90);
        assert_eq!(quota.sub_pools[2].name, "Claude / GPT 池 (5h)");
        assert_eq!(quota.sub_pools[2].pct, 100);
        assert_eq!(quota.sub_pools[3].name, "Claude / GPT 池 (周)");
        assert_eq!(quota.sub_pools[3].pct, 99);
    }

    #[test]
    fn parses_separate_argon_and_agron_pools() {
        let now = Utc::now();
        let h3 = (now + chrono::Duration::hours(3)).to_rfc3339();
        let payload = serde_json::json!({
            "groups": [
                {
                    "displayName": "Gemini Models",
                    "buckets": [
                        { "bucketId": "five_hour", "remainingFraction": 0.80 },
                        { "bucketId": "weekly", "remainingFraction": 0.95 }
                    ]
                },
                {
                    "displayName": "Gemini 4 Argon",
                    "buckets": [
                        { "bucketId": "five_hour", "remainingFraction": 0.42 },
                        { "bucketId": "weekly", "remainingFraction": 0.77 }
                    ]
                },
                {
                    "displayName": "Claude and GPT models",
                    "buckets": [
                        { "bucketId": "weekly", "remainingFraction": 0.88 }
                    ]
                }
            ]
        });

        let quota = parse_antigravity_quota(&sample_record(), &payload).expect("should parse argon");
        assert_eq!(quota.primary_pct, 80);
        let names: Vec<(&str, u8)> = quota
            .sub_pools
            .iter()
            .map(|p| (p.name.as_str(), p.pct))
            .collect();
        assert_eq!(
            names,
            vec![
                ("Gemini 模型池 (5h)", 80),
                ("Gemini 模型池 (周)", 95),
                ("Argon 模型池 (5h)", 42),
                ("Argon 模型池 (周)", 77),
                ("Claude / GPT 池 (周)", 88),
            ]
        );

        // 也兼容仅含 models 且拼写或模型名为 agron 的情况
        let agron_payload = serde_json::json!({
            "models": {
                "gemini-4-agron": { "quotaInfo": { "remainingFraction": 0.35, "resetTime": h3 } }
            }
        });
        let agron_quota = parse_antigravity_quota(&sample_record(), &agron_payload).expect("agron");
        assert_eq!(agron_quota.sub_pools[0].name, "Argon 模型池 (5h)");
        assert_eq!(agron_quota.sub_pools[0].pct, 35);
    }

    #[test]
    fn merges_summary_quotas_with_user_status_meta_and_extra_argon_model() {
        let now = Utc::now();
        let h3 = (now + chrono::Duration::hours(3)).to_rfc3339();
        let summary = serde_json::json!({
            "groups": [
                {
                    "displayName": "Gemini Models",
                    "buckets": [
                        { "bucketId": "five_hour", "remainingFraction": 0.64 },
                        { "bucketId": "weekly", "remainingFraction": 0.90 }
                    ]
                },
                {
                    "displayName": "Claude and GPT models",
                    "buckets": [
                        { "bucketId": "five_hour", "remainingFraction": 1.0 },
                        { "bucketId": "weekly", "remainingFraction": 0.99 }
                    ]
                }
            ]
        });
        let models = serde_json::json!({
            "userStatus": {
                "email": "pilot@example.com",
                "planStatus": { "planInfo": { "planName": "Google AI Ultra" } },
                "cascadeModelConfigData": {
                    "clientModelConfigs": [
                        {
                            "label": "Gemini 4 Neon",
                            "quotaInfo": { "remainingFraction": 1.0 }
                        },
                        {
                            "label": "Claude Opus 4.6",
                            "quotaInfo": { "remainingFraction": 1.0 }
                        },
                        {
                            "label": "Gemini 4 Argon",
                            "modelOrAlias": { "model": "gemini-4-argon" },
                            "quotaInfo": { "remainingFraction": 0.52, "resetTime": h3 }
                        }
                    ]
                }
            }
        });

        let quota = merge_antigravity_quotas(&sample_record(), Some(&summary), Some(&models))
            .expect("should merge");
        assert_eq!(quota.email, "pilot@example.com");
        assert_eq!(quota.plan_label, "AI Ultra");
        assert_eq!(quota.primary_pct, 64);
        let names: Vec<(&str, u8)> = quota
            .sub_pools
            .iter()
            .map(|p| (p.name.as_str(), p.pct))
            .collect();
        assert_eq!(
            names,
            vec![
                ("Gemini 模型池 (5h)", 64),
                ("Gemini 模型池 (周)", 90),
                ("Argon 模型池 (5h)", 52),
                ("Claude / GPT 池 (5h)", 100),
                ("Claude / GPT 池 (周)", 99),
            ]
        );
    }

    #[test]
    fn parses_codexbar_nested_remaining_response_wrapper() {
        let now = Utc::now();
        let h4 = (now + chrono::Duration::hours(4)).to_rfc3339();
        let d6 = (now + chrono::Duration::days(6)).to_rfc3339();
        let d5 = (now + chrono::Duration::days(5)).to_rfc3339();
        let payload = serde_json::json!({
            "response": {
                "groups": [
                    {
                        "displayName": "Gemini Models",
                        "buckets": [
                            {
                                "bucketId": "gemini-weekly",
                                "displayName": "Weekly Limit",
                                "remaining": { "remainingFraction": 0.90 },
                                "resetTime": d6
                            },
                            {
                                "bucketId": "gemini-5h",
                                "displayName": "Five Hour Limit",
                                "remaining": { "case": "remainingFraction", "value": 0.64 },
                                "resetTime": h4
                            },
                            {
                                "bucketId": "gemini-disabled",
                                "displayName": "Five Hour Limit",
                                "disabled": true,
                                "remaining": { "remainingFraction": 0.01 }
                            }
                        ]
                    },
                    {
                        "displayName": "Claude and GPT models",
                        "buckets": [
                            {
                                "bucketId": "3p-weekly",
                                "displayName": "Weekly Limit",
                                "remaining": { "remainingFraction": 0.99 },
                                "resetTime": d5
                            },
                            {
                                "bucketId": "3p-5h",
                                "displayName": "Five Hour Limit",
                                "remaining": { "remainingFraction": 1.0 }
                            }
                        ]
                    }
                ]
            }
        });

        let quota = parse_antigravity_quota(&sample_record(), &payload).expect("should parse nested remaining");
        assert_eq!(quota.primary_pct, 64);
        let names: Vec<(&str, u8)> = quota
            .sub_pools
            .iter()
            .map(|p| (p.name.as_str(), p.pct))
            .collect();
        assert_eq!(
            names,
            vec![
                ("Gemini 模型池 (5h)", 64),
                ("Gemini 模型池 (周)", 90),
                ("Claude / GPT 池 (5h)", 100),
                ("Claude / GPT 池 (周)", 99),
            ]
        );
    }

    #[test]
    fn parses_retrieve_user_quota_model_id_buckets() {
        let payload = serde_json::json!({
            "buckets": [
                { "modelId": "gemini-4-neon", "remainingFraction": 0.64 },
                { "modelId": "gemini-4-argon", "remainingFraction": 0.42 },
                { "modelId": "claude-sonnet-4-6", "remainingFraction": 0.99 }
            ]
        });
        let quota = parse_antigravity_quota(&sample_record(), &payload).expect("should parse modelId buckets");
        assert_eq!(quota.primary_pct, 64);
        let names: Vec<(&str, u8)> = quota
            .sub_pools
            .iter()
            .map(|p| (p.name.as_str(), p.pct))
            .collect();
        assert_eq!(
            names,
            vec![
                ("Gemini 模型池 (5h)", 64),
                ("Argon 模型池", 42),
                ("Claude / GPT 池 (周)", 99),
            ]
        );
    }

    #[test]
    fn extracts_matching_secret_across_stream_chunk_boundary() {
        let mock_secret = [b"GOC".as_slice(), b"SPX-mock_boundary_secret_1234567".as_slice()].concat();
        assert_eq!(mock_secret.len(), GOC_SECRET_LEN);
        let secret_hash = sha256_hex_lowercase(&mock_secret);

        let mock_cid = [
            b"123456789012-mockclientid0123456789".as_slice(),
            GOOGLE_CLIENT_SUFFIX,
        ]
        .concat();
        let cid_hash = sha256_hex_lowercase(&mock_cid);

        let chunk_size = 1024 * 1024;
        let mut synthetic_bin = vec![0x90u8; chunk_size + 512];
        let boundary_offset = chunk_size - 11;
        synthetic_bin[boundary_offset..boundary_offset + mock_secret.len()]
            .copy_from_slice(&mock_secret);
        let cid_offset = chunk_size + 128;
        synthetic_bin[cid_offset..cid_offset + mock_cid.len()].copy_from_slice(&mock_cid);

        let cursor = std::io::Cursor::new(synthetic_bin);
        let found = scan_reader_for_target_hashes(
            cursor,
            &[secret_hash.as_str(), cid_hash.as_str()],
        );
        assert_eq!(
            found.get(&secret_hash).map(|s| s.as_bytes()),
            Some(mock_secret.as_slice())
        );
        assert_eq!(
            found.get(&cid_hash).map(|s| s.as_bytes()),
            Some(mock_cid.as_slice())
        );
    }
}
