use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlatformId {
    Antigravity,
    Gemini,
    Codex,
    Claude,
    Commandcode,
    Opencode,
}

impl PlatformId {
    pub fn display_name(self) -> &'static str {
        match self {
            PlatformId::Antigravity => "反重力",
            PlatformId::Gemini => "Gemini",
            PlatformId::Codex => "Codex",
            PlatformId::Claude => "Claude",
            PlatformId::Commandcode => "Command Code",
            PlatformId::Opencode => "OpenCode Go",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubscriptionTier {
    Paid,
    Free,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowMode {
    Panel,
    Orb,
    Edge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    LocalServer,
    LocalCli,
    OauthToken,
    WebviewSession,
    Cookie,
    ApiKey,
    Demo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    Ok,
    Syncing,
    AuthRequired,
    Offline,
    Error,
    Demo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubPool {
    pub name: String,
    pub pct: u8,
    pub detail: Option<String>,
    pub reset_in: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtraBalance {
    pub label: String,
    pub amount: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountQuota {
    pub id: String,
    pub platform: PlatformId,
    pub alias: String,
    pub email: String,
    pub tier: SubscriptionTier,
    pub plan_label: String,
    pub primary_pct: u8,
    pub reset_in: String,
    pub sub_pools: Vec<SubPool>,
    pub extra_balance: Option<ExtraBalance>,
    pub status: AccountStatus,
    pub status_message: Option<String>,
    pub auth_method: AuthMethod,
    pub last_synced_at: Option<String>,
    pub is_demo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    pub accounts: Vec<AccountQuota>,
    pub last_updated_at: String,
    pub polling_interval_secs: u64,
    #[serde(default)]
    pub active_account_id: Option<String>,
    #[serde(default)]
    pub pinned_account_id: Option<String>,
    #[serde(default)]
    pub active_reason: Option<String>,
    #[serde(default)]
    pub last_active_map: HashMap<String, i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SecretCredentials {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub session_key: Option<String>,
    pub cookie_header: Option<String>,
    pub api_key: Option<String>,
    pub org_id: Option<String>,
    pub account_id: Option<String>,
    pub extra_endpoint: Option<String>,
    pub oauth_client_id: Option<String>,
    pub oauth_client_secret: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredAccountRecord {
    pub id: String,
    pub platform: PlatformId,
    pub alias: String,
    pub email: String,
    pub tier: SubscriptionTier,
    pub plan_label: String,
    pub auth_method: AuthMethod,
    pub profile_dir: Option<String>,
    pub is_demo: bool,
    pub encrypted_secret: Option<String>,
    pub cached_quota: Option<AccountQuota>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddAccountRequest {
    pub platform: PlatformId,
    pub alias: String,
    pub email: String,
    pub tier: SubscriptionTier,
    pub plan_label: String,
    pub auth_method: AuthMethod,
    pub secret_input: Option<String>,
    pub refresh_token: Option<String>,
    pub profile_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedAuthResult {
    pub session_id: String,
    pub captured: bool,
    pub cookie_count: usize,
    pub detected_tier_hint: Option<String>,
    pub detected_email: Option<String>,
    pub detected_plan_label: Option<String>,
    pub detected_primary_pct: Option<u8>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectLocalResult {
    pub found: bool,
    pub message: String,
    pub account: Option<AccountQuota>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalScanItem {
    pub platform: PlatformId,
    pub found: bool,
    pub source: String,
    pub message: String,
    pub account: Option<AccountQuota>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanLocalSummary {
    pub items: Vec<LocalScanItem>,
    pub found_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeOAuthStartResult {
    pub flow_id: String,
    pub authorize_url: String,
}
