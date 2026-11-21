use crate::models::{ClaudeOAuthStartResult, SecretCredentials, SubscriptionTier};
use crate::providers::antigravity::{
    fetch_google_userinfo_email, resolve_builtin_antigravity_oauth_clients, AntigravityProvider,
};
use crate::providers::claude::CLAUDE_OAUTH_CLIENT_ID;
use crate::providers::codex::{parse_openai_jwt_claims, CODEX_OAUTH_CLIENT_ID};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use reqwest::Client;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use url::Url;
use uuid::Uuid;

const CLAUDE_REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
const CLAUDE_SCOPE: &str = "org:create_api_key user:profile user:inference";
const CODEX_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const CODEX_SCOPE: &str = "openid profile email offline_access";
const ANTIGRAVITY_SCOPE: &str = "openid https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile https://www.googleapis.com/auth/cclog https://www.googleapis.com/auth/experimentsandconfigs";

#[derive(Debug, Clone)]
pub struct PendingClaudeFlow {
    pub code_verifier: String,
    pub state: String,
}

#[derive(Default)]
pub struct PendingOAuthStore {
    claude_flows: Mutex<HashMap<String, PendingClaudeFlow>>,
    loopback_cancel: Mutex<Option<oneshot::Sender<()>>>,
}

impl PendingOAuthStore {
    pub fn cancel_active_loopback(&self) -> bool {
        if let Ok(mut guard) = self.loopback_cancel.lock() {
            if let Some(tx) = guard.take() {
                let _ = tx.send(());
                return true;
            }
        }
        false
    }

    pub fn begin_loopback_slot(&self) -> (oneshot::Receiver<()>, bool) {
        let mut had_prev = false;
        let (tx, rx) = oneshot::channel();
        if let Ok(mut guard) = self.loopback_cancel.lock() {
            if let Some(prev) = guard.take() {
                let _ = prev.send(());
                had_prev = true;
            }
            *guard = Some(tx);
        }
        (rx, had_prev)
    }

    pub fn insert_claude_flow(&self, flow_id: String, flow: PendingClaudeFlow) {
        if let Ok(mut guard) = self.claude_flows.lock() {
            guard.insert(flow_id, flow);
        }
    }

    pub fn take_claude_flow(&self, flow_id: &str) -> Option<PendingClaudeFlow> {
        self.claude_flows
            .lock()
            .ok()
            .and_then(|mut guard| guard.remove(flow_id))
    }
}

#[derive(Debug, Clone)]
pub struct BrowserOAuthAccountData {
    pub email: Option<String>,
    pub plan_label: Option<String>,
    pub tier: Option<SubscriptionTier>,
    pub creds: SecretCredentials,
}

pub fn generate_pkce_pair() -> (String, String) {
    let mut raw = [0u8; 48];
    raw[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    raw[16..32].copy_from_slice(Uuid::new_v4().as_bytes());
    raw[32..48].copy_from_slice(Uuid::new_v4().as_bytes());
    let verifier = URL_SAFE_NO_PAD.encode(raw);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

pub fn generate_oauth_state() -> String {
    URL_SAFE_NO_PAD.encode(Uuid::new_v4().as_bytes())
}

#[derive(Debug, PartialEq, Eq)]
pub enum CallbackParseResult {
    Ignore,
    Code {
        code: String,
        state: Option<String>,
    },
    OAuthError(String),
}

pub fn parse_callback_query(raw_http_request: &str, expected_path: &str) -> CallbackParseResult {
    let first_line = raw_http_request.lines().next().unwrap_or("").trim();
    let mut parts = first_line.split_whitespace();
    let _method = parts.next();
    let Some(target) = parts.next() else {
        return CallbackParseResult::Ignore;
    };

    let Ok(parsed_url) = Url::parse(&format!("http://127.0.0.1{target}")) else {
        return CallbackParseResult::Ignore;
    };

    if parsed_url.path() != expected_path {
        return CallbackParseResult::Ignore;
    }

    let mut code: Option<String> = None;
    let mut state: Option<String> = None;
    let mut err: Option<String> = None;
    let mut err_desc: Option<String> = None;

    for (k, v) in parsed_url.query_pairs() {
        match k.as_ref() {
            "code" if !v.is_empty() => code = Some(v.into_owned()),
            "state" if !v.is_empty() => state = Some(v.into_owned()),
            "error" if !v.is_empty() => err = Some(v.into_owned()),
            "error_description" if !v.is_empty() => err_desc = Some(v.into_owned()),
            _ => {}
        }
    }

    if let Some(e) = err {
        let msg = match err_desc {
            Some(d) => format!("浏览器授权被拒绝 ({e}): {d}"),
            None => format!("浏览器授权未完成: {e}"),
        };
        return CallbackParseResult::OAuthError(msg);
    }

    match code {
        Some(c) => CallbackParseResult::Code { code: c, state },
        None => CallbackParseResult::OAuthError("回调地址未携带授权码 (code)".to_string()),
    }
}

pub fn split_claude_code_state(raw_input: &str) -> Result<(String, Option<String>), String> {
    let cleaned = raw_input.trim();
    if cleaned.is_empty() {
        return Err("请粘贴浏览器页面显示的授权码".to_string());
    }
    if let Some((code, state)) = cleaned.split_once('#') {
        let c = code.trim().to_string();
        let s = state.trim().to_string();
        if c.is_empty() {
            return Err("授权码格式无效，缺少 code 部分".to_string());
        }
        return Ok((c, if s.is_empty() { None } else { Some(s) }));
    }
    Ok((cleaned.to_string(), None))
}

fn render_callback_html(ok: bool, title: &str, detail: &str) -> String {
    let badge = if ok { "#10b981" } else { "#f43f5e" };
    let symbol = if ok { "✓" } else { "!" };
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8" />
<title>AuraQuota · {title}</title>
<style>
  body {{ margin: 0; font-family: "Microsoft YaHei", system-ui, sans-serif; background: #0b1020; color: #f1f5f9; display: grid; place-items: center; min-height: 100vh; }}
  .card {{ max-width: 420px; padding: 28px 26px; border-radius: 18px; background: rgba(17, 24, 39, 0.92); border: 1px solid rgba(255,255,255,0.12); text-align: center; box-shadow: 0 20px 44px rgba(0,0,0,0.45); }}
  .icon {{ width: 46px; height: 46px; border-radius: 999px; margin: 0 auto 14px; display: grid; place-items: center; font-size: 22px; font-weight: bold; color: #fff; background: {badge}; }}
  h1 {{ font-size: 18px; margin: 0 0 8px; }}
  p {{ font-size: 13px; line-height: 1.6; color: #94a3b8; margin: 0; }}
</style>
</head>
<body>
  <div class="card">
    <div class="icon">{symbol}</div>
    <h1>{title}</h1>
    <p>{detail}</p>
  </div>
</body>
</html>"#
    )
}

async fn write_http_html(
    stream: &mut tokio::net::TcpStream,
    status_line: &str,
    html: &str,
) {
    let resp = format!(
        "HTTP/1.1 {status_line}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

async fn wait_for_oauth_callback(
    listener: TcpListener,
    expected_path: &str,
    expected_state: &str,
    mut cancel_rx: oneshot::Receiver<()>,
) -> Result<String, String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);

    loop {
        let sleep = tokio::time::sleep_until(deadline);
        tokio::pin!(sleep);

        let (mut stream, _) = tokio::select! {
            _ = &mut cancel_rx => {
                return Err("已取消默认浏览器登录".to_string());
            }
            _ = &mut sleep => {
                return Err("等待默认浏览器授权超时（180 秒），请重试".to_string());
            }
            accept_res = listener.accept() => {
                match accept_res {
                    Ok(conn) => conn,
                    Err(e) => return Err(format!("本地回调监听异常: {e}")),
                }
            }
        };

        let mut buf = [0u8; 4096];
        let n = match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await {
            Ok(Ok(n)) if n > 0 => n,
            _ => continue,
        };

        let req_str = String::from_utf8_lossy(&buf[..n]);
        match parse_callback_query(&req_str, expected_path) {
            CallbackParseResult::Ignore => {
                let _ = stream
                    .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
                continue;
            }
            CallbackParseResult::OAuthError(msg) => {
                let html = render_callback_html(false, "授权未完成", &msg);
                write_http_html(&mut stream, "400 Bad Request", &html).await;
                return Err(msg);
            }
            CallbackParseResult::Code { code, state } => {
                if state.as_deref() != Some(expected_state) {
                    let msg = "OAuth state 校验失败，请回到 AuraQuota 重新发起登录";
                    let html = render_callback_html(false, "安全校验失败", msg);
                    write_http_html(&mut stream, "400 Bad Request", &html).await;
                    return Err(msg.to_string());
                }
                let html = render_callback_html(
                    true,
                    "授权成功",
                    "已安全获取您的 OAuth 凭据并回传至 AuraQuota，您可以关闭此网页。",
                );
                write_http_html(&mut stream, "200 OK", &html).await;
                return Ok(code);
            }
        }
    }
}

fn oauth_http_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("AuraQuota/0.3.9")
        .build()
        .unwrap_or_else(|_| Client::new())
}

pub async fn run_codex_browser_oauth(
    app: &AppHandle,
    pending: &PendingOAuthStore,
) -> Result<BrowserOAuthAccountData, String> {
    let (cancel_rx, had_prev) = pending.begin_loopback_slot();
    if had_prev {
        tokio::time::sleep(Duration::from_millis(60)).await;
    }

    let listener = TcpListener::bind("127.0.0.1:1455").await.map_err(|e| {
        format!(
            "无法监听本地回调端口 1455（可能正在运行其他 Codex 登录进程）: {e}"
        )
    })?;

    let (verifier, challenge) = generate_pkce_pair();
    let state = generate_oauth_state();

    let auth_url = Url::parse_with_params(
        "https://auth.openai.com/oauth/authorize",
        &[
            ("response_type", "code"),
            ("client_id", CODEX_OAUTH_CLIENT_ID),
            ("redirect_uri", CODEX_REDIRECT_URI),
            ("scope", CODEX_SCOPE),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("state", state.as_str()),
        ],
    )
    .map_err(|e| format!("构造 Codex 授权链接失败: {e}"))?;

    app.opener()
        .open_url(auth_url.as_str(), None::<&str>)
        .map_err(|e| format!("打开系统默认浏览器失败: {e}"))?;

    let code = wait_for_oauth_callback(listener, "/auth/callback", &state, cancel_rx).await?;

    let client = oauth_http_client();
    let resp = client
        .post("https://auth.openai.com/oauth/token")
        .header("Accept", "application/json")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CODEX_OAUTH_CLIENT_ID),
            ("code", code.as_str()),
            ("code_verifier", verifier.as_str()),
            ("redirect_uri", CODEX_REDIRECT_URI),
        ])
        .send()
        .await
        .map_err(|e| format!("Codex 换取 Token 失败: {e}"))?;

    if !resp.status().is_success() {
        let code_num = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Codex 换取 Token 返回 HTTP {code_num}: {body}"));
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
        .ok_or_else(|| "Codex 授权响应中缺少 access_token".to_string())?;
    let refresh_token = json
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let id_token = json
        .get("id_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());

    let (email, account_id, plan_raw) = id_token
        .map(parse_openai_jwt_claims)
        .unwrap_or_else(|| parse_openai_jwt_claims(&access_token));

    let (tier, plan_label) = match plan_raw.as_deref().map(str::to_lowercase).as_deref() {
        Some("pro") => (Some(SubscriptionTier::Paid), Some("Pro".to_string())),
        Some("plus") => (Some(SubscriptionTier::Paid), Some("Plus".to_string())),
        Some("team") | Some("business") => (Some(SubscriptionTier::Paid), Some("Team".to_string())),
        Some("free") => (Some(SubscriptionTier::Free), Some("Free".to_string())),
        _ => (None, None),
    };

    Ok(BrowserOAuthAccountData {
        email,
        plan_label,
        tier,
        creds: SecretCredentials {
            access_token: Some(access_token),
            refresh_token,
            account_id,
            ..Default::default()
        },
    })
}

pub fn start_claude_browser_oauth(
    app: &AppHandle,
    pending: &PendingOAuthStore,
) -> Result<ClaudeOAuthStartResult, String> {
    let (verifier, challenge) = generate_pkce_pair();
    let state = generate_oauth_state();
    let flow_id = format!("claude-flow-{}", &Uuid::new_v4().to_string()[..8]);

    let auth_url = Url::parse_with_params(
        "https://claude.ai/oauth/authorize",
        &[
            ("code", "true"),
            ("client_id", CLAUDE_OAUTH_CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", CLAUDE_REDIRECT_URI),
            ("scope", CLAUDE_SCOPE),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", state.as_str()),
        ],
    )
    .map_err(|e| format!("构造 Claude 授权链接失败: {e}"))?
    .to_string();

    pending.insert_claude_flow(
        flow_id.clone(),
        PendingClaudeFlow {
            code_verifier: verifier,
            state,
        },
    );

    app.opener()
        .open_url(&auth_url, None::<&str>)
        .map_err(|e| format!("打开系统默认浏览器失败: {e}"))?;

    Ok(ClaudeOAuthStartResult {
        flow_id,
        authorize_url: auth_url,
    })
}

pub async fn complete_claude_browser_oauth(
    pending: &PendingOAuthStore,
    flow_id: &str,
    raw_code_input: &str,
) -> Result<BrowserOAuthAccountData, String> {
    let (code, pasted_state) = split_claude_code_state(raw_code_input)?;
    let flow = pending
        .take_claude_flow(flow_id)
        .ok_or_else(|| "当前 Claude 授权会话已失效，请重新点击「打开默认浏览器授权」".to_string())?;

    if let Some(ref st) = pasted_state {
        if st != &flow.state {
            return Err("粘贴的授权码 state 与当前会话不匹配，请重新发起浏览器授权".to_string());
        }
    }

    let payload = serde_json::json!({
        "grant_type": "authorization_code",
        "code": code,
        "state": pasted_state.unwrap_or_else(|| flow.state.clone()),
        "client_id": CLAUDE_OAUTH_CLIENT_ID,
        "redirect_uri": CLAUDE_REDIRECT_URI,
        "code_verifier": flow.code_verifier,
    });

    let client = oauth_http_client();
    let resp = client
        .post("https://console.anthropic.com/v1/oauth/token")
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Claude 换取 Token 失败: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Claude 换取 Token 返回 HTTP {status}: {body}"));
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
        .ok_or_else(|| "Claude 响应未包含 access_token".to_string())?;
    let refresh_token = json
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let email = json
        .pointer("/account/email_address")
        .or_else(|| json.get("email"))
        .and_then(|v| v.as_str())
        .filter(|s| s.contains('@'))
        .map(str::to_string);

    Ok(BrowserOAuthAccountData {
        email,
        plan_label: None,
        tier: None,
        creds: SecretCredentials {
            access_token: Some(access_token),
            refresh_token,
            ..Default::default()
        },
    })
}

pub fn build_antigravity_auth_url(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
) -> Result<String, String> {
    Url::parse_with_params(
        "https://accounts.google.com/o/oauth2/v2/auth",
        &[
            ("client_id", client_id),
            ("redirect_uri", redirect_uri),
            ("response_type", "code"),
            ("scope", ANTIGRAVITY_SCOPE),
            ("access_type", "offline"),
            ("prompt", "consent select_account"),
            ("include_granted_scopes", "true"),
            ("code_challenge", code_challenge),
            ("code_challenge_method", "S256"),
            ("state", state),
        ],
    )
    .map(|u| u.to_string())
    .map_err(|e| format!("构造 Google 授权链接失败: {e}"))
}

pub async fn run_antigravity_browser_oauth(
    app: &AppHandle,
    pending: &PendingOAuthStore,
) -> Result<BrowserOAuthAccountData, String> {
    let clients = resolve_builtin_antigravity_oauth_clients();
    let primary = clients.into_iter().next().ok_or_else(|| {
        "未在本机检测到 Antigravity 客户端（language_server.exe），无法自动读取桌面端 OAuth 授权配置；请先安装 Antigravity 或改用手动 Token".to_string()
    })?;

    let (cancel_rx, had_prev) = pending.begin_loopback_slot();
    if had_prev {
        tokio::time::sleep(Duration::from_millis(60)).await;
    }

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("无法绑定本地 Google OAuth 回调端口: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("获取本地回调端口失败: {e}"))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/oauth-callback");

    let (verifier, challenge) = generate_pkce_pair();
    let state = generate_oauth_state();
    let auth_url =
        build_antigravity_auth_url(&primary.client_id, &redirect_uri, &state, &challenge)?;

    app.opener()
        .open_url(&auth_url, None::<&str>)
        .map_err(|e| format!("打开系统默认浏览器失败: {e}"))?;

    let code = wait_for_oauth_callback(listener, "/oauth-callback", &state, cancel_rx).await?;

    let client = oauth_http_client();
    let resp = client
        .post("https://oauth2.googleapis.com/token")
        .header("Accept", "application/json")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", primary.client_id.as_str()),
            ("client_secret", primary.client_secret.as_str()),
            ("code", code.as_str()),
            ("code_verifier", verifier.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
        ])
        .send()
        .await
        .map_err(|e| {
            format!("Google OAuth 换取 Token 失败（请检查本机网络/代理能否访问 googleapis.com）: {e}")
        })?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Google OAuth 换取 Token 返回 HTTP {status}: {body}"));
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
        .ok_or_else(|| "Google 授权响应未包含 access_token".to_string())?;
    let refresh_token = json
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    if refresh_token.is_none() {
        return Err("Google 未返回永久 Refresh Token，请在授权确认页勾选全部权限后再试一次".to_string());
    }

    let email = fetch_google_userinfo_email(&client, &access_token).await;
    let tier_meta = AntigravityProvider::new()
        .probe_account_tier(&access_token)
        .await;
    let (tier, plan_label) = match tier_meta {
        Some((t, lbl)) => (Some(t), Some(lbl)),
        None => (None, None),
    };

    Ok(BrowserOAuthAccountData {
        email,
        plan_label,
        tier,
        creds: SecretCredentials {
            access_token: Some(access_token),
            refresh_token,
            oauth_client_id: Some(primary.client_id),
            ..Default::default()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_valid_pkce_s256_pair() {
        let (verifier, challenge) = generate_pkce_pair();
        assert!(verifier.len() >= 43 && verifier.len() <= 128);
        assert!(!verifier.contains('='));
        assert!(!challenge.contains('='));
        let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, expected);
    }

    #[test]
    fn parses_loopback_callback_requests() {
        let fav = "GET /favicon.ico HTTP/1.1\r\nHost: 127.0.0.1:1455\r\n\r\n";
        assert_eq!(
            parse_callback_query(fav, "/auth/callback"),
            CallbackParseResult::Ignore
        );

        let ok = "GET /auth/callback?code=auth_code_123&state=st_abc HTTP/1.1\r\nHost: localhost:1455\r\n\r\n";
        assert_eq!(
            parse_callback_query(ok, "/auth/callback"),
            CallbackParseResult::Code {
                code: "auth_code_123".to_string(),
                state: Some("st_abc".to_string()),
            }
        );

        let denied = "GET /oauth-callback?error=access_denied&error_description=User%20cancelled HTTP/1.1\r\n\r\n";
        assert!(matches!(
            parse_callback_query(denied, "/oauth-callback"),
            CallbackParseResult::OAuthError(_)
        ));
    }

    #[test]
    fn splits_claude_code_and_state() {
        let (c1, s1) = split_claude_code_state("  abcCode123#xyzState789 \n").expect("split");
        assert_eq!(c1, "abcCode123");
        assert_eq!(s1.as_deref(), Some("xyzState789"));

        let (c2, s2) = split_claude_code_state("onlyCode999").expect("plain code");
        assert_eq!(c2, "onlyCode999");
        assert_eq!(s2, None);

        assert!(split_claude_code_state("   ").is_err());
    }

    #[test]
    fn builds_antigravity_offline_auth_url() {
        let url = build_antigravity_auth_url(
            "1071006060591-mock.apps.googleusercontent.com",
            "http://127.0.0.1:45678/oauth-callback",
            "state123",
            "challengeXYZ",
        )
        .expect("build agy auth url");
        let parsed = Url::parse(&url).expect("parse generated auth url");
        let params: HashMap<String, String> = parsed.query_pairs().into_owned().collect();
        assert_eq!(params.get("access_type").map(String::as_str), Some("offline"));
        assert_eq!(
            params.get("prompt").map(String::as_str),
            Some("consent select_account")
        );
        assert_eq!(
            params.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(
            params.get("redirect_uri").map(String::as_str),
            Some("http://127.0.0.1:45678/oauth-callback")
        );
    }
}
