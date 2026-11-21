use crate::crypto_dpapi::{protect_string, unprotect_string};
use crate::models::{
    AccountQuota, AccountStatus, AddAccountRequest, AuthMethod, ExtraBalance, PlatformId,
    SecretCredentials, StoredAccountRecord, SubPool, SubscriptionTier,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VaultFile {
    version: u32,
    accounts: Vec<StoredAccountRecord>,
}

pub struct AccountStore {
    vault_path: PathBuf,
    profiles_root: PathBuf,
    state: Mutex<VaultFile>,
}

impl AccountStore {
    pub fn from_app_data_dir(app_data_dir: &Path) -> Result<Self, String> {
        let vault_path = app_data_dir.join("accounts.vault.json");
        let profiles_root = app_data_dir.join("webview_profiles");
        Self::with_paths(vault_path, profiles_root)
    }

    pub fn with_path(vault_path: PathBuf) -> Result<Self, String> {
        let profiles_root = vault_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("webview_profiles");
        Self::with_paths(vault_path, profiles_root)
    }

    fn with_paths(vault_path: PathBuf, profiles_root: PathBuf) -> Result<Self, String> {
        if let Some(parent) = vault_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("创建数据目录失败 {}: {e}", parent.display()))?;
        }
        fs::create_dir_all(&profiles_root)
            .map_err(|e| format!("创建 WebView 配置目录失败 {}: {e}", profiles_root.display()))?;

        let store = Self {
            vault_path,
            profiles_root,
            state: Mutex::new(VaultFile {
                version: 1,
                accounts: Vec::new(),
            }),
        };
        store.load_or_seed()?;
        Ok(store)
    }

    pub fn profile_dir_for(&self, account_id: &str) -> PathBuf {
        let sanitized: String = account_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        self.profiles_root.join(sanitized)
    }

    fn load_or_seed(&self) -> Result<(), String> {
        if self.vault_path.exists() {
            let content = fs::read_to_string(&self.vault_path)
                .map_err(|e| format!("读取凭据库失败: {e}"))?;
            let parsed: VaultFile = serde_json::from_str(&content)
                .map_err(|e| format!("解析凭据库失败: {e}"))?;
            let mut guard = self.state.lock().map_err(|_| "存储锁异常".to_string())?;
            *guard = parsed;
            return Ok(());
        }

        let seeded = VaultFile {
            version: 1,
            accounts: build_demo_records(),
        };
        {
            let mut guard = self.state.lock().map_err(|_| "存储锁异常".to_string())?;
            *guard = seeded;
            self.save_locked(&guard)?;
        }
        Ok(())
    }

    fn save_locked(&self, vault: &VaultFile) -> Result<(), String> {
        let json = serde_json::to_string_pretty(vault)
            .map_err(|e| format!("序列化凭据库失败: {e}"))?;
        let tmp_path = self.vault_path.with_extension("json.tmp");
        fs::write(&tmp_path, json).map_err(|e| format!("写入临时凭据文件失败: {e}"))?;
        fs::rename(&tmp_path, &self.vault_path)
            .map_err(|e| format!("保存凭据库失败: {e}"))?;
        Ok(())
    }

    pub fn list_records(&self) -> Result<Vec<StoredAccountRecord>, String> {
        let guard = self.state.lock().map_err(|_| "存储锁异常".to_string())?;
        Ok(guard.accounts.clone())
    }

    pub fn get_record(&self, id: &str) -> Result<Option<StoredAccountRecord>, String> {
        let guard = self.state.lock().map_err(|_| "存储锁异常".to_string())?;
        Ok(guard.accounts.iter().find(|r| r.id == id).cloned())
    }

    pub fn list_quotas(&self) -> Result<Vec<AccountQuota>, String> {
        let guard = self.state.lock().map_err(|_| "存储锁异常".to_string())?;
        Ok(guard
            .accounts
            .iter()
            .map(|r| {
                r.cached_quota.clone().unwrap_or_else(|| AccountQuota {
                    id: r.id.clone(),
                    platform: r.platform,
                    alias: r.alias.clone(),
                    email: r.email.clone(),
                    tier: r.tier,
                    plan_label: r.plan_label.clone(),
                    primary_pct: 100,
                    reset_in: "待同步".to_string(),
                    sub_pools: Vec::new(),
                    extra_balance: None,
                    status: AccountStatus::Syncing,
                    status_message: None,
                    auth_method: r.auth_method,
                    last_synced_at: None,
                    is_demo: r.is_demo,
                })
            })
            .collect())
    }

    pub fn add_account(
        &self,
        req: &AddAccountRequest,
        creds: Option<&SecretCredentials>,
        explicit_id: Option<String>,
        profile_dir: Option<String>,
        initial_quota: Option<AccountQuota>,
    ) -> Result<StoredAccountRecord, String> {
        let id = explicit_id.unwrap_or_else(|| {
            format!(
                "{:?}-{}",
                req.platform,
                &uuid::Uuid::new_v4().to_string()[..8]
            )
            .to_lowercase()
        });

        let encrypted_secret = match creds {
            Some(c) if has_any_secret(c) => {
                let json = serde_json::to_string(c)
                    .map_err(|e| format!("序列化凭据失败: {e}"))?;
                Some(protect_string(&json)?)
            }
            _ => None,
        };

        let record = StoredAccountRecord {
            id: id.clone(),
            platform: req.platform,
            alias: req.alias.trim().to_string(),
            email: req.email.trim().to_string(),
            tier: req.tier,
            plan_label: req.plan_label.trim().to_string(),
            auth_method: req.auth_method,
            profile_dir,
            is_demo: false,
            encrypted_secret,
            cached_quota: initial_quota,
        };

        let mut guard = self.state.lock().map_err(|_| "存储锁异常".to_string())?;
        guard.accounts.retain(|a| a.id != id);
        guard.accounts.insert(0, record.clone());
        self.save_locked(&guard)?;
        Ok(record)
    }

    pub fn update_credentials(
        &self,
        id: &str,
        creds: &SecretCredentials,
        profile_dir: Option<String>,
    ) -> Result<(), String> {
        let json =
            serde_json::to_string(creds).map_err(|e| format!("序列化凭据失败: {e}"))?;
        let encrypted = protect_string(&json)?;

        let mut guard = self.state.lock().map_err(|_| "存储锁异常".to_string())?;
        if let Some(rec) = guard.accounts.iter_mut().find(|a| a.id == id) {
            rec.encrypted_secret = Some(encrypted);
            if profile_dir.is_some() {
                rec.profile_dir = profile_dir;
            }
            self.save_locked(&guard)?;
            return Ok(());
        }
        Err(format!("未找到账号 {id}"))
    }

    pub fn update_cached_quota(&self, quota: &AccountQuota) -> Result<(), String> {
        let mut guard = self.state.lock().map_err(|_| "存储锁异常".to_string())?;
        if let Some(rec) = guard.accounts.iter_mut().find(|a| a.id == quota.id) {
            rec.plan_label = quota.plan_label.clone();
            rec.tier = quota.tier;
            if is_placeholder_email(&rec.email) && !is_placeholder_email(&quota.email) {
                rec.email = quota.email.clone();
            }
            rec.cached_quota = Some(quota.clone());
            self.save_locked(&guard)?;
        }
        Ok(())
    }

    pub fn remove_account(&self, id: &str) -> Result<bool, String> {
        let mut guard = self.state.lock().map_err(|_| "存储锁异常".to_string())?;
        let before = guard.accounts.len();
        guard.accounts.retain(|a| a.id != id);
        let removed = guard.accounts.len() != before;
        if removed {
            self.save_locked(&guard)?;
        }
        Ok(removed)
    }

    pub fn clear_demo_accounts(&self) -> Result<(), String> {
        let mut guard = self.state.lock().map_err(|_| "存储锁异常".to_string())?;
        guard.accounts.retain(|a| !a.is_demo);
        self.save_locked(&guard)?;
        Ok(())
    }

    pub fn restore_demo_accounts(&self) -> Result<(), String> {
        let mut guard = self.state.lock().map_err(|_| "存储锁异常".to_string())?;
        let demos = build_demo_records();
        for demo in demos {
            if !guard.accounts.iter().any(|a| a.id == demo.id) {
                guard.accounts.push(demo);
            }
        }
        self.save_locked(&guard)?;
        Ok(())
    }

    pub fn decrypt_credentials(
        &self,
        record: &StoredAccountRecord,
    ) -> Result<Option<SecretCredentials>, String> {
        let Some(cipher) = &record.encrypted_secret else {
            return Ok(None);
        };
        let plain = unprotect_string(cipher)?;
        let creds: SecretCredentials =
            serde_json::from_str(&plain).map_err(|e| format!("凭据解析失败: {e}"))?;
        Ok(Some(creds))
    }
}

fn is_placeholder_email(email: &str) -> bool {
    let e = email.trim();
    e.is_empty()
        || matches!(
            e,
            "account@local"
                | "local@antigravity"
                | "cli@codex.local"
                | "cli@claude.local"
                | "cli@commandcode.local"
                | "cli@opencode.local"
        )
}

fn has_any_secret(c: &SecretCredentials) -> bool {
    c.access_token.as_deref().is_some_and(|s| !s.is_empty())
        || c.refresh_token.as_deref().is_some_and(|s| !s.is_empty())
        || c.session_key.as_deref().is_some_and(|s| !s.is_empty())
        || c.cookie_header.as_deref().is_some_and(|s| !s.is_empty())
        || c.api_key.as_deref().is_some_and(|s| !s.is_empty())
}

pub fn build_demo_records() -> Vec<StoredAccountRecord> {
    let specs = vec![
        AccountQuota {
            id: "demo-agy-1".into(),
            platform: PlatformId::Antigravity,
            alias: "开发主号".into(),
            email: "dev.main@example.com".into(),
            tier: SubscriptionTier::Paid,
            plan_label: "AI Pro".into(),
            primary_pct: 84,
            reset_in: "3小时18分后重置".into(),
            sub_pools: vec![
                SubPool {
                    name: "Gemini 4 / Flash (5h)".into(),
                    pct: 84,
                    detail: Some("模型池正常".into()),
                    reset_in: Some("3小时18分后重置".into()),
                },
                SubPool {
                    name: "Claude / GPT 池 (周)".into(),
                    pct: 62,
                    detail: Some("第三方模型周额度".into()),
                    reset_in: Some("4天12小时后重置".into()),
                },
            ],
            extra_balance: None,
            status: AccountStatus::Demo,
            status_message: Some("示例数据 · 点击上方可检测本机反重力".into()),
            auth_method: AuthMethod::Demo,
            last_synced_at: None,
            is_demo: true,
        },
        AccountQuota {
            id: "demo-agy-2".into(),
            platform: PlatformId::Antigravity,
            alias: "免费备用号".into(),
            email: "backup.agy@example.com".into(),
            tier: SubscriptionTier::Free,
            plan_label: "Free".into(),
            primary_pct: 45,
            reset_in: "4小时05分后重置".into(),
            sub_pools: vec![
                SubPool {
                    name: "Gemini Flash (5h)".into(),
                    pct: 45,
                    detail: None,
                    reset_in: None,
                },
                SubPool {
                    name: "周总额度".into(),
                    pct: 70,
                    detail: None,
                    reset_in: None,
                },
            ],
            extra_balance: None,
            status: AccountStatus::Demo,
            status_message: None,
            auth_method: AuthMethod::Demo,
            last_synced_at: None,
            is_demo: true,
        },
        AccountQuota {
            id: "demo-gem-1".into(),
            platform: PlatformId::Gemini,
            alias: "日常主号".into(),
            email: "daily.gem@example.com".into(),
            tier: SubscriptionTier::Paid,
            plan_label: "AI Pro".into(),
            primary_pct: 67,
            reset_in: "3:30 PM 重置".into(),
            sub_pools: vec![
                SubPool {
                    name: "5小时当前窗口 (Current usage)".into(),
                    pct: 67,
                    detail: Some("已使用 33%".into()),
                    reset_in: Some("3:30 PM 重置".into()),
                },
                SubPool {
                    name: "7天周限额 (Weekly limit)".into(),
                    pct: 98,
                    detail: Some("已使用 2%".into()),
                    reset_in: Some("Nov 25 12:30 AM 重置".into()),
                },
            ],
            extra_balance: None,
            status: AccountStatus::Demo,
            status_message: None,
            auth_method: AuthMethod::Demo,
            last_synced_at: None,
            is_demo: true,
        },
        AccountQuota {
            id: "demo-codex-1".into(),
            platform: PlatformId::Codex,
            alias: "主力工作号".into(),
            email: "work.codex@example.com".into(),
            tier: SubscriptionTier::Paid,
            plan_label: "Plus".into(),
            primary_pct: 68,
            reset_in: "2小时42分后重置".into(),
            sub_pools: vec![
                SubPool {
                    name: "5小时滑动窗口".into(),
                    pct: 68,
                    detail: None,
                    reset_in: None,
                },
                SubPool {
                    name: "7天周额度".into(),
                    pct: 81,
                    detail: None,
                    reset_in: None,
                },
            ],
            extra_balance: None,
            status: AccountStatus::Demo,
            status_message: None,
            auth_method: AuthMethod::Demo,
            last_synced_at: None,
            is_demo: true,
        },
        AccountQuota {
            id: "demo-claude-1".into(),
            platform: PlatformId::Claude,
            alias: "主力号".into(),
            email: "studio.claude@example.com".into(),
            tier: SubscriptionTier::Paid,
            plan_label: "Pro".into(),
            primary_pct: 22,
            reset_in: "1小时12分后重置".into(),
            sub_pools: vec![
                SubPool {
                    name: "5小时会话限额".into(),
                    pct: 22,
                    detail: None,
                    reset_in: None,
                },
                SubPool {
                    name: "周总额度".into(),
                    pct: 54,
                    detail: None,
                    reset_in: None,
                },
            ],
            extra_balance: None,
            status: AccountStatus::Demo,
            status_message: None,
            auth_method: AuthMethod::Demo,
            last_synced_at: None,
            is_demo: true,
        },
        AccountQuota {
            id: "demo-claude-2".into(),
            platform: PlatformId::Claude,
            alias: "轻度免费号".into(),
            email: "free.note@example.com".into(),
            tier: SubscriptionTier::Free,
            plan_label: "Free".into(),
            primary_pct: 95,
            reset_in: "4小时50分后重置".into(),
            sub_pools: vec![SubPool {
                name: "当前窗口剩余".into(),
                pct: 95,
                detail: None,
                reset_in: None,
            }],
            extra_balance: None,
            status: AccountStatus::Demo,
            status_message: None,
            auth_method: AuthMethod::Demo,
            last_synced_at: None,
            is_demo: true,
        },
        AccountQuota {
            id: "demo-cc-1".into(),
            platform: PlatformId::Commandcode,
            alias: "GOAT 主力号".into(),
            email: "goat.dev@example.com".into(),
            tier: SubscriptionTier::Paid,
            plan_label: "GOAT".into(),
            primary_pct: 74,
            reset_in: "3小时50分后重置".into(),
            sub_pools: vec![
                SubPool {
                    name: "5小时窗口（月额 20%）".into(),
                    pct: 74,
                    detail: None,
                    reset_in: None,
                },
                SubPool {
                    name: "7天周窗口（月额 50%）".into(),
                    pct: 66,
                    detail: None,
                    reset_in: None,
                },
                SubPool {
                    name: "月度包含额度".into(),
                    pct: 82,
                    detail: Some("仅计算订阅内月度配额".into()),
                    reset_in: None,
                },
            ],
            extra_balance: Some(ExtraBalance {
                label: "充值余额 (Purchased Credits)".into(),
                amount: "$18.50".into(),
                note: Some("可结转 · 独立于月度配额".into()),
            }),
            status: AccountStatus::Demo,
            status_message: None,
            auth_method: AuthMethod::Demo,
            last_synced_at: None,
            is_demo: true,
        },
        AccountQuota {
            id: "demo-oc-1".into(),
            platform: PlatformId::Opencode,
            alias: "Go 常用号".into(),
            email: "opencode.go@example.com".into(),
            tier: SubscriptionTier::Paid,
            plan_label: "Go".into(),
            primary_pct: 58,
            reset_in: "2小时15分后重置".into(),
            sub_pools: vec![
                SubPool {
                    name: "5小时窗口（月额 20%）".into(),
                    pct: 58,
                    detail: None,
                    reset_in: None,
                },
                SubPool {
                    name: "7天周窗口（月额 50%）".into(),
                    pct: 71,
                    detail: None,
                    reset_in: None,
                },
                SubPool {
                    name: "月度额度（100%）".into(),
                    pct: 85,
                    detail: None,
                    reset_in: None,
                },
            ],
            extra_balance: Some(ExtraBalance {
                label: "Console / Zen 余额".into(),
                amount: "$12.00".into(),
                note: Some("按量计费余额".into()),
            }),
            status: AccountStatus::Demo,
            status_message: None,
            auth_method: AuthMethod::Demo,
            last_synced_at: None,
            is_demo: true,
        },
    ];

    specs.into_iter()
        .map(|q| StoredAccountRecord {
            id: q.id.clone(),
            platform: q.platform,
            alias: q.alias.clone(),
            email: q.email.clone(),
            tier: q.tier,
            plan_label: q.plan_label.clone(),
            auth_method: AuthMethod::Demo,
            profile_dir: None,
            is_demo: true,
            encrypted_secret: None,
            cached_quota: Some(q),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_persists_encrypted_secrets_without_plaintext() {
        let dir = std::env::temp_dir().join(format!("auraquota_test_{}", uuid::Uuid::new_v4()));
        let vault_path = dir.join("accounts.vault.json");
        let store = AccountStore::with_path(vault_path.clone()).expect("create store");

        let req = AddAccountRequest {
            platform: PlatformId::Claude,
            alias: "测试号".into(),
            email: "test@example.com".into(),
            tier: SubscriptionTier::Paid,
            plan_label: "Pro".into(),
            auth_method: AuthMethod::Cookie,
            secret_input: None,
            refresh_token: None,
            profile_id: None,
        };
        let secret_marker = "mock_super_secret_session_key_xyz987";
        let creds = SecretCredentials {
            session_key: Some(secret_marker.into()),
            ..Default::default()
        };

        let rec = store
            .add_account(&req, Some(&creds), None, None, None)
            .expect("add account");
        let raw_file = fs::read_to_string(&vault_path).expect("read vault file");
        assert!(
            !raw_file.contains(secret_marker),
            "Vault JSON on disk must never contain plaintext credentials!"
        );

        let decrypted = store
            .decrypt_credentials(&rec)
            .expect("decrypt")
            .expect("some creds");
        assert_eq!(decrypted.session_key.as_deref(), Some(secret_marker));

        let _ = fs::remove_dir_all(&dir);
    }
}
