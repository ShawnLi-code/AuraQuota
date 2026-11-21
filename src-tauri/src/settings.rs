use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const DEFAULT_UPDATE_FEED_URL: &str =
    "https://github.com/ShawnLi-code/AuraQuota/releases/latest";

const LEGACY_GITHUB_API_FEED: &str =
    "https://api.github.com/repos/ShawnLi-code/AuraQuota/releases/latest";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    pub theme: String,
    pub auto_check_updates: bool,
    pub update_feed_url: String,
    pub polling_interval_secs: u64,
    pub polling_migrated_v2: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            auto_check_updates: true,
            update_feed_url: DEFAULT_UPDATE_FEED_URL.to_string(),
            polling_interval_secs: 15,
            polling_migrated_v2: false,
        }
    }
}

pub struct SettingsStore {
    path: PathBuf,
    state: Mutex<AppSettings>,
}

impl SettingsStore {
    pub fn from_app_data_dir(app_data_dir: &Path) -> Result<Self, String> {
        let path = app_data_dir.join("settings.json");
        let mut settings = if path.exists() {
            let raw = fs::read_to_string(&path)
                .map_err(|e| format!("读取设置文件失败: {e}"))?;
            serde_json::from_str::<AppSettings>(&raw).unwrap_or_default()
        } else {
            AppSettings::default()
        };

        let mut needs_write = !path.exists();
        if !settings.polling_migrated_v2 {
            if settings.polling_interval_secs == 300 {
                settings.polling_interval_secs = 15;
            }
            settings.polling_migrated_v2 = true;
            needs_write = true;
        }
        let clamped = settings.polling_interval_secs.clamp(5, 3600);
        if clamped != settings.polling_interval_secs {
            settings.polling_interval_secs = clamped;
            needs_write = true;
        }
        if settings.update_feed_url.trim().is_empty()
            || settings.update_feed_url.trim() == LEGACY_GITHUB_API_FEED
        {
            settings.update_feed_url = DEFAULT_UPDATE_FEED_URL.to_string();
            needs_write = true;
        }

        if needs_write {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(&settings) {
                let _ = fs::write(&path, json);
            }
        }

        Ok(Self {
            path,
            state: Mutex::new(settings),
        })
    }

    pub fn get(&self) -> AppSettings {
        self.state
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    pub fn get_for_ui(&self) -> AppSettings {
        self.get()
    }

    fn write_settings(&self, s: &AppSettings) -> Result<(), String> {
        let json = serde_json::to_string_pretty(s)
            .map_err(|e| format!("序列化设置失败: {e}"))?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("创建设置目录失败: {e}"))?;
        }
        fs::write(&self.path, json).map_err(|e| format!("写入设置失败: {e}"))
    }

    pub fn save(&self, next: AppSettings) -> Result<AppSettings, String> {
        let mut guard = self.state.lock().map_err(|_| "设置锁异常".to_string())?;
        let trimmed_feed = next.update_feed_url.trim();
        let sanitized = AppSettings {
            theme: if next.theme == "light" {
                "light".to_string()
            } else {
                "dark".to_string()
            },
            auto_check_updates: next.auto_check_updates,
            update_feed_url: if trimmed_feed.is_empty() {
                DEFAULT_UPDATE_FEED_URL.to_string()
            } else {
                trimmed_feed.to_string()
            },
            polling_interval_secs: next.polling_interval_secs.clamp(5, 3600),
            polling_migrated_v2: true,
        };

        self.write_settings(&sanitized)?;
        *guard = sanitized.clone();
        Ok(sanitized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_legacy_300s_once_and_preserves_explicit_choice() {
        let dir = std::env::temp_dir().join(format!(
            "auraquota_settings_test_{}_{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::create_dir_all(&dir);
        let legacy_json = r#"{"theme":"dark","pollingIntervalSecs":300}"#;
        fs::write(dir.join("settings.json"), legacy_json).expect("write legacy settings");

        let store = SettingsStore::from_app_data_dir(&dir).expect("load settings");
        assert_eq!(store.get().polling_interval_secs, 15);
        assert!(store.get().polling_migrated_v2);

        // User explicitly selects 300s after migration -> stays 300s on reload
        let mut next = store.get();
        next.polling_interval_secs = 300;
        store.save(next).expect("save 300s");

        let reloaded = SettingsStore::from_app_data_dir(&dir).expect("reload settings");
        assert_eq!(reloaded.get().polling_interval_secs, 300);

        let _ = fs::remove_dir_all(&dir);
    }
}
