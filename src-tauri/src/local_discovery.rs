use crate::models::{AuthMethod, PlatformId, SecretCredentials, SubscriptionTier};
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct LocalDiscoveredCredential {
    pub platform: PlatformId,
    pub alias: String,
    pub email: String,
    pub plan_label: String,
    pub tier: SubscriptionTier,
    pub auth_method: AuthMethod,
    pub creds: SecretCredentials,
    pub source_desc: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityExtractedAuth {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub email: Option<String>,
}

fn user_home_dir() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .map(PathBuf::from)
}

fn appdata_roaming_dir() -> Option<PathBuf> {
    std::env::var("APPDATA").ok().map(PathBuf::from)
}

fn appdata_local_dir() -> Option<PathBuf> {
    std::env::var("LOCALAPPDATA").ok().map(PathBuf::from)
}

struct TempFileGuard(PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn read_sqlite_auth_rows(vscdb_path: &Path) -> Result<Vec<(String, String)>, String> {
    if !vscdb_path.exists() {
        return Err(format!("未找到数据库: {}", vscdb_path.display()));
    }

    let tmp_path = std::env::temp_dir().join(format!(
        "auraquota_ro_{}.vscdb",
        uuid::Uuid::new_v4()
    ));
    let _cleanup = TempFileGuard(tmp_path.clone());

    fs::copy(vscdb_path, &tmp_path)
        .map_err(|e| format!("只读复制 state.vscdb 失败: {e}"))?;

    let conn = Connection::open_with_flags(&tmp_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("只读打开 state.vscdb 失败: {e}"))?;

    let mut stmt = conn
        .prepare(
            "SELECT key, CAST(value AS TEXT) FROM ItemTable \
             WHERE key LIKE '%antigravity%' OR key LIKE '%auth%' \
             OR key LIKE '%oauth%' OR key LIKE '%jetski%'",
        )
        .map_err(|e| format!("查询 ItemTable 失败: {e}"))?;

    let rows_iter = stmt
        .query_map([], |row| {
            let k: String = row.get(0)?;
            let v: String = row.get(1).unwrap_or_default();
            Ok((k, v))
        })
        .map_err(|e| format!("读取 ItemTable 记录失败: {e}"))?;

    let mut out = Vec::new();
    for item in rows_iter.flatten() {
        out.push(item);
    }
    Ok(out)
}

fn row_priority(key: &str) -> u8 {
    let low = key.to_lowercase();
    if low.contains("antigravityauthstatus") {
        0
    } else if low.contains("jetskistatesync") {
        1
    } else if low.contains("antigravityunifiedstatesync") {
        2
    } else if low.contains("antigravity") {
        3
    } else {
        4
    }
}

#[derive(Default)]
struct ExtractedFragments {
    access_token: Option<String>,
    refresh_token: Option<String>,
    email: Option<String>,
}

fn collect_fragments_from_value(
    raw: &str,
    depth: u8,
    out: &mut ExtractedFragments,
) {
    if depth > 3 || raw.is_empty() {
        return;
    }

    if out.email.is_none() {
        out.email = find_email_ascii(raw);
    }
    if out.access_token.is_none() {
        out.access_token = extract_prefixed_token(raw, "ya29.", 25);
    }
    if out.refresh_token.is_none() {
        out.refresh_token = extract_prefixed_token(raw, "1//", 24);
    }

    if let Ok(json) = serde_json::from_str::<Value>(raw) {
        collect_fragments_from_json(&json, depth, out);
    } else if depth < 3 {
        let cleaned = raw.trim().trim_matches('"');
        if cleaned.len() >= 24 && !cleaned.contains(' ') {
            if let Some(text) = decode_b64_to_utf8(cleaned) {
                if text != cleaned {
                    collect_fragments_from_value(&text, depth + 1, out);
                }
            }
        }

        // Also scan embedded ASCII base64 runs inside binary protobuf / mixed text
        if out.access_token.is_none() || out.refresh_token.is_none() {
            for run in raw.split(|c: char| {
                !c.is_ascii_alphanumeric() && !matches!(c, '+' | '/' | '=' | '-' | '_')
            }) {
                if run.len() >= 32
                    && run.len() < raw.len()
                    && !run.starts_with("ya29.")
                    && !run.starts_with("1//")
                {
                    if let Some(decoded) = decode_b64_to_utf8(run) {
                        if &decoded != run {
                            collect_fragments_from_value(&decoded, depth + 1, out);
                        }
                    }
                }
            }
        }
    }
}

fn collect_fragments_from_json(json: &Value, depth: u8, out: &mut ExtractedFragments) {
    if out.email.is_none() {
        out.email = json
            .get("email")
            .or_else(|| json.pointer("/user/email"))
            .or_else(|| json.pointer("/account/email"))
            .and_then(|v| v.as_str())
            .filter(|s| s.contains('@'))
            .map(str::to_string);
    }
    if out.access_token.is_none() {
        out.access_token = json
            .get("accessToken")
            .or_else(|| json.get("access_token"))
            .or_else(|| json.get("oauthToken"))
            .or_else(|| json.pointer("/token/accessToken"))
            .or_else(|| json.pointer("/oauthToken/accessToken"))
            .or_else(|| {
                json.get("apiKey")
                    .filter(|v| v.as_str().is_some_and(|s| s.starts_with("ya29.")))
            })
            .and_then(|v| v.as_str())
            .filter(|s| s.len() > 20)
            .map(str::to_string);
    }
    if out.refresh_token.is_none() {
        out.refresh_token = json
            .get("refreshToken")
            .or_else(|| json.get("refresh_token"))
            .or_else(|| json.pointer("/token/refreshToken"))
            .or_else(|| json.pointer("/oauthToken/refreshToken"))
            .and_then(|v| v.as_str())
            .filter(|s| s.len() >= 20)
            .map(str::to_string);
    }

    if depth >= 3 {
        return;
    }

    match json {
        Value::Object(map) => {
            for v in map.values() {
                collect_fragments_from_json(v, depth + 1, out);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                collect_fragments_from_json(v, depth + 1, out);
            }
        }
        Value::String(s) => {
            if let Some(tok) = extract_prefixed_token(s, "ya29.", 25) {
                if out.access_token.is_none() {
                    out.access_token = Some(tok);
                }
            }
            if let Some(rtok) = extract_prefixed_token(s, "1//", 24) {
                if out.refresh_token.is_none() {
                    out.refresh_token = Some(rtok);
                }
            }
            if out.email.is_none() {
                out.email = find_email_ascii(s);
            }
            if (out.access_token.is_none() || out.refresh_token.is_none())
                && s.len() >= 24
                && !s.contains(' ')
            {
                if let Some(decoded) = decode_b64_to_utf8(s) {
                    if &decoded != s {
                        collect_fragments_from_value(&decoded, depth + 1, out);
                    }
                }
            }
        }
        _ => {}
    }
}

fn decode_b64_to_utf8(cleaned: &str) -> Option<String> {
    let decoded = STANDARD
        .decode(cleaned)
        .or_else(|_| STANDARD_NO_PAD.decode(cleaned))
        .or_else(|_| URL_SAFE_NO_PAD.decode(cleaned))
        .or_else(|_| URL_SAFE.decode(cleaned))
        .ok()?;
    Some(String::from_utf8_lossy(&decoded).into_owned())
}

pub fn decode_credential_blob(bytes: &[u8]) -> Option<AntigravityExtractedAuth> {
    if bytes.is_empty() {
        return None;
    }

    let utf8_text = String::from_utf8_lossy(bytes).trim_matches('\0').to_string();
    let mut frag = ExtractedFragments::default();
    collect_fragments_from_value(&utf8_text, 0, &mut frag);

    if (frag.access_token.is_none() || frag.email.is_none()) && bytes.len() >= 4 && bytes.len() % 2 == 0
    {
        let looks_utf16le = bytes
            .iter()
            .skip(1)
            .step_by(2)
            .take(8)
            .filter(|b| **b == 0)
            .count()
            >= 2;
        if looks_utf16le {
            let u16s: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let utf16_text = String::from_utf16_lossy(&u16s).trim_matches('\0').to_string();
            collect_fragments_from_value(&utf16_text, 0, &mut frag);
        }
    }

    frag.access_token.map(|tok| AntigravityExtractedAuth {
        access_token: tok,
        refresh_token: frag.refresh_token,
        email: frag.email,
    })
}

#[cfg(windows)]
fn discover_windows_credential_manager_antigravity() -> Vec<LocalDiscoveredCredential> {
    use windows_sys::Win32::Security::Credentials::{
        CredEnumerateW, CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
    };

    let mut out = Vec::new();
    let to_wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };

    let mut push_cred_ptr = |cred: &CREDENTIALW, label_prefix: &str| {
        if cred.CredentialBlob.is_null() || cred.CredentialBlobSize == 0 {
            return;
        }
        let blob = unsafe {
            std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize)
        };
        if let Some(auth) = decode_credential_blob(blob) {
            let email = auth
                .email
                .unwrap_or_else(|| "local@antigravity".to_string());
            push_unique_agy_candidate(
                &mut out,
                LocalDiscoveredCredential {
                    platform: PlatformId::Antigravity,
                    alias: "本机反重力".to_string(),
                    email,
                    plan_label: "AI Pro".to_string(),
                    tier: SubscriptionTier::Paid,
                    auth_method: AuthMethod::OauthToken,
                    creds: SecretCredentials {
                        access_token: Some(auth.access_token),
                        refresh_token: auth.refresh_token,
                        ..Default::default()
                    },
                    source_desc: label_prefix.to_string(),
                },
            );
        }
    };

    for filter in ["*antigravity*", "*gemini*"] {
        let wide_filter = to_wide(filter);
        let mut count: u32 = 0;
        let mut creds_ptr: *mut *mut CREDENTIALW = std::ptr::null_mut();
        let ok = unsafe {
            CredEnumerateW(wide_filter.as_ptr(), 0, &mut count, &mut creds_ptr)
        };
        if ok != 0 && !creds_ptr.is_null() {
            let slice = unsafe { std::slice::from_raw_parts(creds_ptr, count as usize) };
            for &ptr in slice {
                if !ptr.is_null() {
                    let cred = unsafe { &*ptr };
                    push_cred_ptr(cred, "Windows 凭据管理器");
                }
            }
            unsafe {
                CredFree(creds_ptr as _);
            }
        }
    }

    for target in [
        "gemini:antigravity",
        "LegacyGeneric:target=gemini:antigravity",
        "antigravity",
        "Antigravity",
    ] {
        let wide_target = to_wide(target);
        let mut cred_ptr: *mut CREDENTIALW = std::ptr::null_mut();
        let ok = unsafe {
            CredReadW(wide_target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred_ptr)
        };
        if ok != 0 && !cred_ptr.is_null() {
            let cred = unsafe { &*cred_ptr };
            push_cred_ptr(cred, &format!("Windows 凭据管理器 ({target})"));
            unsafe {
                CredFree(cred_ptr as _);
            }
        }
    }

    out
}

#[cfg(not(windows))]
fn discover_windows_credential_manager_antigravity() -> Vec<LocalDiscoveredCredential> {
    Vec::new()
}

pub fn extract_antigravity_candidates_from_rows(
    rows: &[(String, String)],
) -> Vec<AntigravityExtractedAuth> {
    let mut ordered: Vec<&(String, String)> = rows.iter().collect();
    ordered.sort_by_key(|(k, _)| row_priority(k));

    let mut fallback_email: Option<String> = None;
    let mut fallback_refresh: Option<String> = None;
    let mut candidates: Vec<AntigravityExtractedAuth> = Vec::new();

    for (_key, val) in ordered {
        let mut frag = ExtractedFragments::default();
        collect_fragments_from_value(val, 0, &mut frag);

        if fallback_email.is_none() {
            fallback_email = frag.email.clone();
        }
        if fallback_refresh.is_none() {
            fallback_refresh = frag.refresh_token.clone();
        }

        if let Some(tok) = frag.access_token {
            if let Some(existing) = candidates.iter_mut().find(|c| c.access_token == tok) {
                if existing.refresh_token.is_none() {
                    existing.refresh_token = frag.refresh_token;
                }
                if existing.email.is_none() {
                    existing.email = frag.email;
                }
            } else {
                candidates.push(AntigravityExtractedAuth {
                    access_token: tok,
                    refresh_token: frag.refresh_token,
                    email: frag.email,
                });
            }
        }
    }

    for c in &mut candidates {
        if c.email.is_none() {
            c.email = fallback_email.clone();
        }
        if c.refresh_token.is_none() {
            c.refresh_token = fallback_refresh.clone();
        }
    }

    candidates
}

pub fn extract_antigravity_auth_from_rows(
    rows: &[(String, String)],
) -> Option<(String, Option<String>)> {
    extract_antigravity_candidates_from_rows(rows)
        .into_iter()
        .next()
        .map(|c| (c.access_token, c.email))
}

fn extract_prefixed_token(text: &str, prefix: &str, min_len: usize) -> Option<String> {
    let mut offset = 0usize;
    while let Some(rel_idx) = text[offset..].find(prefix) {
        let idx = offset + rel_idx;
        let tail = &text[idx..];
        let tok: String = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/'))
            .collect();
        if tok.len() >= min_len {
            return Some(tok);
        }
        offset = idx + prefix.len();
    }
    None
}

fn find_email_ascii(text: &str) -> Option<String> {
    for part in text.split(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '@' | '.' | '_' | '-' | '+')) {
        if part.contains('@') && part.contains('.') && part.len() >= 5 && part.len() <= 80 {
            let mut segs = part.split('@');
            let u = segs.next()?;
            let d = segs.next()?;
            if segs.next().is_none() && !u.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.') {
                return Some(part.to_string());
            }
        }
    }
    None
}

fn push_unique_agy_candidate(
    list: &mut Vec<LocalDiscoveredCredential>,
    cand: LocalDiscoveredCredential,
) {
    let tok = cand.creds.access_token.as_deref().unwrap_or("");
    let rtok = cand.creds.refresh_token.as_deref().unwrap_or("");
    if let Some(existing) = list.iter_mut().find(|item| {
        let same_tok = !tok.is_empty() && item.creds.access_token.as_deref() == Some(tok);
        let same_rtok = !rtok.is_empty() && item.creds.refresh_token.as_deref() == Some(rtok);
        same_tok || same_rtok
    }) {
        if existing.creds.refresh_token.is_none() && cand.creds.refresh_token.is_some() {
            existing.creds.refresh_token = cand.creds.refresh_token;
        }
        if (existing.email.is_empty() || existing.email == "local@antigravity")
            && cand.email != "local@antigravity"
        {
            existing.email = cand.email;
        }
        return;
    }
    list.push(cand);
}

pub fn discover_antigravity_offline_candidates() -> Result<Vec<LocalDiscoveredCredential>, String> {
    let mut results = Vec::new();
    let mut db_candidates = Vec::new();
    if let Some(roaming) = appdata_roaming_dir() {
        db_candidates.push(roaming.join("Antigravity").join("User").join("globalStorage").join("state.vscdb"));
        db_candidates.push(roaming.join("Antigravity IDE").join("User").join("globalStorage").join("state.vscdb"));
    }
    if let Some(local) = appdata_local_dir() {
        db_candidates.push(local.join("Antigravity").join("User").join("globalStorage").join("state.vscdb"));
    }

    for db_path in &db_candidates {
        if db_path.exists() {
            if let Ok(rows) = read_sqlite_auth_rows(db_path) {
                for extracted in extract_antigravity_candidates_from_rows(&rows) {
                    push_unique_agy_candidate(
                        &mut results,
                        LocalDiscoveredCredential {
                            platform: PlatformId::Antigravity,
                            alias: "本机反重力".to_string(),
                            email: extracted
                                .email
                                .unwrap_or_else(|| "local@antigravity".to_string()),
                            plan_label: "AI Pro".to_string(),
                            tier: SubscriptionTier::Paid,
                            auth_method: AuthMethod::OauthToken,
                            creds: SecretCredentials {
                                access_token: Some(extracted.access_token),
                                refresh_token: extracted.refresh_token,
                                ..Default::default()
                            },
                            source_desc: format!(
                                "离线数据库 {}",
                                db_path.file_name().unwrap_or_default().to_string_lossy()
                            ),
                        },
                    );
                }
            }
        }
    }

    if let Some(home) = user_home_dir() {
        let json_paths = [
            home.join(".gemini").join("antigravity-cli").join("oauth_creds.json"),
            home.join(".gemini").join("antigravity-cli").join("credentials.json"),
            home.join(".gemini").join("antigravity").join("oauth_creds.json"),
            home.join(".gemini").join("oauth_creds.json"),
        ];
        for path in json_paths {
            if let Ok(raw) = fs::read_to_string(&path) {
                if let Ok(json) = serde_json::from_str::<Value>(&raw) {
                    let tok = json
                        .get("access_token")
                        .or_else(|| json.get("accessToken"))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);
                    let refresh = json
                        .get("refresh_token")
                        .or_else(|| json.get("refreshToken"))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);

                    if tok.is_some() || refresh.is_some() {
                        let email = json
                            .get("email")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                            .or_else(|| {
                                json.get("id_token")
                                    .and_then(|v| v.as_str())
                                    .and_then(decode_jwt_email)
                            })
                            .unwrap_or_else(|| "local@antigravity".to_string());

                        push_unique_agy_candidate(
                            &mut results,
                            LocalDiscoveredCredential {
                                platform: PlatformId::Antigravity,
                                alias: "本机反重力".to_string(),
                                email,
                                plan_label: "AI Pro".to_string(),
                                tier: SubscriptionTier::Paid,
                                auth_method: AuthMethod::OauthToken,
                                creds: SecretCredentials {
                                    access_token: tok,
                                    refresh_token: refresh,
                                    ..Default::default()
                                },
                                source_desc: format!("本地凭证 {}", path.display()),
                            },
                        );
                    }
                }
            }
        }
    }

    for cred in discover_windows_credential_manager_antigravity() {
        push_unique_agy_candidate(&mut results, cred);
    }

    if results.is_empty() {
        Err("未在本机检测到 Antigravity 客户端登录态（请先打开 Antigravity 桌面客户端 / IDE 或运行 agy 登录 Google 账号）".to_string())
    } else {
        Ok(results)
    }
}

pub fn discover_antigravity_offline() -> Result<LocalDiscoveredCredential, String> {
    discover_antigravity_offline_candidates()
        .and_then(|mut list| {
            if list.is_empty() {
                Err("未在本机检测到 Antigravity 登录凭证".to_string())
            } else {
                Ok(list.remove(0))
            }
        })
}

pub fn parse_claude_credentials_json(raw: &str) -> Result<LocalDiscoveredCredential, String> {
    let json: Value =
        serde_json::from_str(raw).map_err(|e| format!("解析 Claude 凭证失败: {e}"))?;
    let oauth = json.get("claudeAiOauth").unwrap_or(&json);
    let token = oauth
        .get("accessToken")
        .or_else(|| oauth.get("access_token"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "未在凭证文件中找到 claudeAiOauth.accessToken".to_string())?;

    let refresh = oauth
        .get("refreshToken")
        .or_else(|| oauth.get("refresh_token"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let email = json
        .get("email")
        .or_else(|| oauth.get("email"))
        .or_else(|| json.get("accountEmail"))
        .or_else(|| json.pointer("/oauthAccount/emailAddress"))
        .and_then(|v| v.as_str())
        .filter(|s| s.contains('@'))
        .unwrap_or("cli@claude.local")
        .to_string();

    let sub_type = oauth
        .get("subscriptionType")
        .or_else(|| oauth.get("rateLimitTier"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();

    let (tier, plan_label) = if sub_type.contains("max") {
        (SubscriptionTier::Paid, "Max".to_string())
    } else if sub_type.contains("pro") {
        (SubscriptionTier::Paid, "Pro".to_string())
    } else if sub_type.contains("free") {
        (SubscriptionTier::Free, "Free".to_string())
    } else {
        (SubscriptionTier::Paid, "Pro".to_string())
    };

    Ok(LocalDiscoveredCredential {
        platform: PlatformId::Claude,
        alias: "Claude Code 本机".to_string(),
        email,
        plan_label,
        tier,
        auth_method: AuthMethod::LocalCli,
        creds: SecretCredentials {
            access_token: Some(token.to_string()),
            refresh_token: refresh,
            ..Default::default()
        },
        source_desc: "~/.claude/.credentials.json".to_string(),
    })
}

pub fn discover_claude_code_local() -> Result<LocalDiscoveredCredential, String> {
    let home = user_home_dir().ok_or_else(|| "无法定位用户主目录".to_string())?;
    let path = home.join(".claude").join(".credentials.json");
    if !path.exists() {
        return Err(format!("未找到 Claude Code 凭证文件: {}", path.display()));
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("读取失败: {e}"))?;
    let mut cred = parse_claude_credentials_json(&raw)?;
    if cred.email == "cli@claude.local" {
        let state_path = home.join(".claude.json");
        if let Ok(state_raw) = fs::read_to_string(&state_path) {
            if let Ok(state_json) = serde_json::from_str::<Value>(&state_raw) {
                if let Some(em) = state_json
                    .pointer("/oauthAccount/emailAddress")
                    .and_then(|v| v.as_str())
                    .filter(|s| s.contains('@'))
                {
                    cred.email = em.to_string();
                }
            }
        }
    }
    Ok(cred)
}

pub fn parse_codex_auth_json(
    raw: &str,
    source_desc: &str,
) -> Result<LocalDiscoveredCredential, String> {
    let json: Value =
        serde_json::from_str(raw).map_err(|e| format!("解析 Codex auth.json 失败: {e}"))?;

    let token = json
        .pointer("/tokens/access_token")
        .or_else(|| json.get("access_token"))
        .or_else(|| json.get("OPENAI_API_KEY"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "auth.json 中未包含 access_token".to_string())?;

    let refresh_token = json
        .pointer("/tokens/refresh_token")
        .or_else(|| json.get("refresh_token"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let id_token = json
        .pointer("/tokens/id_token")
        .or_else(|| json.get("id_token"))
        .and_then(|v| v.as_str());

    let account_id = json
        .pointer("/tokens/account_id")
        .or_else(|| json.get("account_id"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| decode_jwt_account_id(token))
        .or_else(|| id_token.and_then(decode_jwt_account_id));

    let email = id_token
        .and_then(decode_jwt_email)
        .or_else(|| decode_jwt_email(token))
        .or_else(|| json.get("email").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| "cli@codex.local".to_string());

    Ok(LocalDiscoveredCredential {
        platform: PlatformId::Codex,
        alias: "Codex CLI".to_string(),
        email,
        plan_label: "Plus".to_string(),
        tier: SubscriptionTier::Paid,
        auth_method: AuthMethod::LocalCli,
        creds: SecretCredentials {
            access_token: Some(token.to_string()),
            refresh_token,
            account_id,
            ..Default::default()
        },
        source_desc: source_desc.to_string(),
    })
}

pub fn discover_codex_cli_local() -> Result<LocalDiscoveredCredential, String> {
    let mut paths = Vec::new();
    if let Ok(ch) = std::env::var("CODEX_HOME") {
        paths.push(PathBuf::from(ch).join("auth.json"));
    }
    if let Some(home) = user_home_dir() {
        paths.push(home.join(".codex").join("auth.json"));
    }
    if let Some(roaming) = appdata_roaming_dir() {
        paths.push(roaming.join("codex").join("auth.json"));
    }

    for path in paths {
        if !path.exists() {
            continue;
        }
        let raw = fs::read_to_string(&path).map_err(|e| format!("读取 Codex auth.json 失败: {e}"))?;
        if let Ok(cred) = parse_codex_auth_json(&raw, &path.display().to_string()) {
            return Ok(cred);
        }
    }

    Err("未在本机找到 Codex CLI 登录凭证 (~/.codex/auth.json)".to_string())
}

pub fn parse_commandcode_auth_json(raw: &str) -> Result<LocalDiscoveredCredential, String> {
    let json: Value =
        serde_json::from_str(raw).map_err(|e| format!("解析 Command Code 凭证失败: {e}"))?;

    let api_key = json
        .get("apiKey")
        .or_else(|| json.get("api_key"))
        .or_else(|| json.get("key"))
        .or_else(|| json.get("accessToken"))
        .or_else(|| json.get("token"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let session_cookie = json
        .get("sessionToken")
        .or_else(|| json.get("cookie"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    if api_key.is_none() && session_cookie.is_none() {
        return Err("Command Code 本地文件中未包含有效 API Key 或会话令牌".to_string());
    }

    let email = json
        .get("email")
        .or_else(|| json.pointer("/user/email"))
        .and_then(|v| v.as_str())
        .unwrap_or("cli@commandcode.local")
        .to_string();

    let auth_method = if api_key.is_some() {
        AuthMethod::ApiKey
    } else {
        AuthMethod::Cookie
    };

    Ok(LocalDiscoveredCredential {
        platform: PlatformId::Commandcode,
        alias: "Command Code 本机".to_string(),
        email,
        plan_label: "GOAT".to_string(),
        tier: SubscriptionTier::Paid,
        auth_method,
        creds: SecretCredentials {
            api_key,
            cookie_header: session_cookie,
            ..Default::default()
        },
        source_desc: "~/.commandcode/auth.json".to_string(),
    })
}

pub fn discover_commandcode_cli_local() -> Result<LocalDiscoveredCredential, String> {
    let home = user_home_dir().ok_or_else(|| "无法定位用户主目录".to_string())?;
    let candidates = [
        home.join(".commandcode").join("auth.json"),
        home.join(".commandcode").join("config.json"),
    ];
    for path in candidates {
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(cred) = parse_commandcode_auth_json(&raw) {
                return Ok(cred);
            }
        }
    }
    Err("未在本机找到 Command Code CLI 凭证 (~/.commandcode/auth.json)".to_string())
}

pub fn parse_opencode_auth_json(raw: &str) -> Result<LocalDiscoveredCredential, String> {
    let json: Value =
        serde_json::from_str(raw).map_err(|e| format!("解析 OpenCode 凭证失败: {e}"))?;

    let key = json
        .pointer("/opencode/key")
        .or_else(|| json.pointer("/opencode/apiKey"))
        .or_else(|| json.pointer("/opencode-go/key"))
        .or_else(|| json.pointer("/zen/key"))
        .or_else(|| json.get("apiKey"))
        .or_else(|| json.get("key"))
        .or_else(|| json.get("token"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "OpenCode auth.json 中未找到 opencode/zen API Key".to_string())?;

    Ok(LocalDiscoveredCredential {
        platform: PlatformId::Opencode,
        alias: "OpenCode 本机".to_string(),
        email: "cli@opencode.local".to_string(),
        plan_label: "Go".to_string(),
        tier: SubscriptionTier::Paid,
        auth_method: AuthMethod::ApiKey,
        creds: SecretCredentials {
            api_key: Some(key.to_string()),
            ..Default::default()
        },
        source_desc: "~/.local/share/opencode/auth.json".to_string(),
    })
}

pub fn discover_opencode_cli_local() -> Result<LocalDiscoveredCredential, String> {
    let mut candidates = Vec::new();
    if let Some(home) = user_home_dir() {
        candidates.push(home.join(".local").join("share").join("opencode").join("auth.json"));
    }
    if let Some(local) = appdata_local_dir() {
        candidates.push(local.join("opencode").join("auth.json"));
    }
    if let Some(roaming) = appdata_roaming_dir() {
        candidates.push(roaming.join("opencode").join("auth.json"));
    }

    for path in candidates {
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(cred) = parse_opencode_auth_json(&raw) {
                return Ok(cred);
            }
        }
    }
    Err("未在本机找到 OpenCode CLI 凭证 (~/.local/share/opencode/auth.json)".to_string())
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

fn decode_jwt_account_id(jwt: &str) -> Option<String> {
    let mut parts = jwt.split('.');
    let _header = parts.next()?;
    let payload_b64 = parts.next()?;
    let decoded = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let payload: Value = serde_json::from_slice(&decoded).ok()?;
    payload
        .pointer("/https://api.openai.com/auth/chatgpt_account_id")
        .or_else(|| payload.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_token_from_antigravity_vscdb_sqlite() {
        let dir = std::env::temp_dir().join(format!("agy_vscdb_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("state.vscdb");

        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)",
                [],
            )
            .unwrap();
            let blob = STANDARD.encode(br#"{"email":"agy.user@example.com","accessToken":"ya29.mock_access_token_value_1234567890"}"#);
            conn.execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                ["antigravityUnifiedStateSync.oauthToken", &blob],
            )
            .unwrap();
        }

        let rows = read_sqlite_auth_rows(&db_path).expect("should read rows");
        let (tok, email) = extract_antigravity_auth_from_rows(&rows).expect("should extract auth");
        assert_eq!(tok, "ya29.mock_access_token_value_1234567890");
        assert_eq!(email.as_deref(), Some("agy.user@example.com"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extracts_antigravity_api_key_and_refresh_from_nested_rows() {
        let inner_b64 = STANDARD.encode(br#"1//mock_refresh_token_abcdefghijklmnopqrstuvwxyz"#);
        let auth_status_json = serde_json::json!({
            "email": "agy.nested@example.com",
            "apiKey": "ya29.mock_nested_api_key_token_1234567890",
            "nestedBlob": inner_b64
        })
        .to_string();

        let rows = vec![
            ("antigravityAuthStatus".to_string(), auth_status_json),
        ];
        let cands = extract_antigravity_candidates_from_rows(&rows);
        assert_eq!(cands.len(), 1);
        assert_eq!(
            cands[0].access_token,
            "ya29.mock_nested_api_key_token_1234567890"
        );
        assert_eq!(
            cands[0].refresh_token.as_deref(),
            Some("1//mock_refresh_token_abcdefghijklmnopqrstuvwxyz")
        );
        assert_eq!(cands[0].email.as_deref(), Some("agy.nested@example.com"));
    }

    #[test]
    fn decodes_windows_credential_manager_utf8_and_utf16_blobs() {
        let json_utf8 = br#"{"email":"cred.user@example.com","access_token":"ya29.mock_credmgr_token_abcdefghij12345","refresh_token":"1//mock_credmgr_refresh_abcdefghij"}"#;
        let parsed = decode_credential_blob(json_utf8).expect("utf8 cred blob");
        assert_eq!(parsed.access_token, "ya29.mock_credmgr_token_abcdefghij12345");
        assert_eq!(parsed.email.as_deref(), Some("cred.user@example.com"));

        let text = "ya29.mock_utf16_credmgr_token_99887766";
        let utf16_bytes: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let parsed_16 = decode_credential_blob(&utf16_bytes).expect("utf16 cred blob");
        assert_eq!(parsed_16.access_token, text);
    }

    #[test]
    fn extracts_unpadded_base64_and_embedded_protobuf_chunks() {
        let inner_unpadded = STANDARD_NO_PAD.encode(br#"ya29.mock_unpadded_proto_token_1234567890"#);
        let pseudo_proto = format!("\u{0008}\u{0012}*{inner_unpadded}\u{001a}\u{0010}proto@example.com");
        let rows = vec![("jetskiStateSync.agentManagerInitState".to_string(), pseudo_proto)];
        let cands = extract_antigravity_candidates_from_rows(&rows);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].access_token, "ya29.mock_unpadded_proto_token_1234567890");
        assert_eq!(cands[0].email.as_deref(), Some("proto@example.com"));
    }

    #[test]
    fn parses_codex_auth_json_with_refresh_token() {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(br#"{"email":"codex.user@example.com","https://api.openai.com/auth":{"chatgpt_account_id":"acc_mock_123"}}"#);
        let id_tok = format!("{header}.{payload}.sig");

        let raw = serde_json::json!({
            "tokens": {
                "access_token": "mock_codex_access_token",
                "refresh_token": "mock_codex_refresh_token",
                "id_token": id_tok,
                "account_id": "acc_mock_123"
            }
        })
        .to_string();

        let cred = parse_codex_auth_json(&raw, "~/.codex/auth.json").expect("parse codex");
        assert_eq!(cred.email, "codex.user@example.com");
        assert_eq!(cred.creds.access_token.as_deref(), Some("mock_codex_access_token"));
        assert_eq!(cred.creds.refresh_token.as_deref(), Some("mock_codex_refresh_token"));
        assert_eq!(cred.creds.account_id.as_deref(), Some("acc_mock_123"));
    }

    #[test]
    fn parses_claude_code_credentials_json() {
        let raw = r#"{
          "claudeAiOauth": {
            "accessToken": "sk-ant-oat01-mock-token",
            "refreshToken": "sk-ant-ort01-mock-refresh",
            "subscriptionType": "max"
          }
        }"#;
        let cred = parse_claude_credentials_json(raw).expect("parse claude creds");
        assert_eq!(cred.plan_label, "Max");
        assert_eq!(cred.auth_method, AuthMethod::LocalCli);
        assert_eq!(
            cred.creds.access_token.as_deref(),
            Some("sk-ant-oat01-mock-token")
        );
        assert_eq!(
            cred.creds.refresh_token.as_deref(),
            Some("sk-ant-ort01-mock-refresh")
        );
    }

    #[test]
    fn parses_commandcode_and_opencode_auth_json() {
        let cc = r#"{"apiKey":"cc_mock_key_999","email":"cc@example.com"}"#;
        let cc_cred = parse_commandcode_auth_json(cc).expect("parse cc");
        assert_eq!(cc_cred.creds.api_key.as_deref(), Some("cc_mock_key_999"));
        assert_eq!(cc_cred.email, "cc@example.com");

        let oc = r#"{"opencode":{"type":"api","key":"oc_mock_key_123"}}"#;
        let oc_cred = parse_opencode_auth_json(oc).expect("parse oc");
        assert_eq!(oc_cred.creds.api_key.as_deref(), Some("oc_mock_key_123"));
    }
}
