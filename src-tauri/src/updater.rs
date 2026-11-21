use reqwest::Client;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub has_update: bool,
    pub release_notes: Option<String>,
    pub download_url: Option<String>,
    pub sha256: Option<String>,
    pub message: String,
    pub configured: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgressEvent {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub pct: u8,
}

pub fn current_app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn is_newer_version(latest: &str, current: &str) -> bool {
    let parse_ver = |s: &str| -> (u64, u64, u64) {
        let clean = s.trim().trim_start_matches(['v', 'V']);
        let core = clean.split(['-', '+']).next().unwrap_or(clean);
        let mut nums = core.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
        (
            nums.next().unwrap_or(0),
            nums.next().unwrap_or(0),
            nums.next().unwrap_or(0),
        )
    };
    parse_ver(latest) > parse_ver(current)
}

pub fn parse_update_manifest(json: &Value, current: &str) -> Result<UpdateCheckResult, String> {
    // Format A: GitHub Releases API (/repos/<owner>/<repo>/releases/latest)
    if let Some(tag) = json.get("tag_name").and_then(|v| v.as_str()) {
        let latest = tag.trim().trim_start_matches(['v', 'V']).to_string();
        let notes = json
            .get("body")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let exe_asset = json
            .get("assets")
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                arr.iter().find(|a| {
                    a.get("name")
                        .and_then(|n| n.as_str())
                        .is_some_and(|n| n.to_lowercase().ends_with(".exe"))
                })
            });
        let download_url = exe_asset
            .and_then(|a| a.get("browser_download_url"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let has_update = is_newer_version(&latest, current) && download_url.is_some();

        return Ok(UpdateCheckResult {
            current_version: current.to_string(),
            latest_version: Some(latest.clone()),
            has_update,
            release_notes: notes,
            download_url,
            sha256: None,
            message: if has_update {
                format!("发现新版本 v{latest}")
            } else {
                format!("当前已是最新版本 v{current}")
            },
            configured: true,
        });
    }

    // Format B: Tauri / AuraQuota latest.json
    if let Some(ver) = json.get("version").and_then(|v| v.as_str()) {
        let latest = ver.trim().trim_start_matches(['v', 'V']).to_string();
        let notes = json
            .get("notes")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let download_url = json
            .pointer("/platforms/windows-x86_64/url")
            .or_else(|| json.get("url"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let sha256 = json
            .pointer("/platforms/windows-x86_64/sha256")
            .or_else(|| json.get("sha256"))
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let has_update = is_newer_version(&latest, current) && download_url.is_some();
        return Ok(UpdateCheckResult {
            current_version: current.to_string(),
            latest_version: Some(latest.clone()),
            has_update,
            release_notes: notes,
            download_url,
            sha256,
            message: if has_update {
                format!("发现新版本 v{latest}")
            } else {
                format!("当前已是最新版本 v{current}")
            },
            configured: true,
        });
    }

    Err("更新清单格式不受支持（需 GitHub Releases API 或 latest.json）".to_string())
}

pub async fn check_for_updates(feed_url: &str) -> UpdateCheckResult {
    let current = current_app_version().to_string();
    let trimmed = feed_url.trim();

    if trimmed.is_empty() {
        return UpdateCheckResult {
            current_version: current,
            latest_version: None,
            has_update: false,
            release_notes: None,
            download_url: None,
            sha256: None,
            message: "尚未配置更新源；请在设置中填写 GitHub Releases API、latest.json URL 或本地目录".to_string(),
            configured: false,
        };
    }

    // Local directory or local latest.json support
    let local_path = Path::new(trimmed);
    if local_path.exists() {
        if local_path.is_file() {
            if let Ok(raw) = fs::read_to_string(local_path) {
                if let Ok(json) = serde_json::from_str::<Value>(&raw) {
                    if let Ok(res) = parse_update_manifest(&json, &current) {
                        return res;
                    }
                }
            }
        } else if local_path.is_dir() {
            let manifest = local_path.join("latest.json");
            if manifest.exists() {
                if let Ok(raw) = fs::read_to_string(&manifest) {
                    if let Ok(json) = serde_json::from_str::<Value>(&raw) {
                        if let Ok(res) = parse_update_manifest(&json, &current) {
                            return res;
                        }
                    }
                }
            }
            if let Some(found) = scan_local_installer_dir(local_path, &current) {
                return found;
            }
        }
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent(format!("AuraQuota/{current}"))
        .build()
        .unwrap_or_else(|_| Client::new());

    let resp = match client.get(trimmed).header("Accept", "application/json").send().await {
        Ok(r) => r,
        Err(e) => {
            return UpdateCheckResult {
                current_version: current,
                latest_version: None,
                has_update: false,
                release_notes: None,
                download_url: None,
                sha256: None,
                message: format!("检查更新失败: {e}"),
                configured: true,
            };
        }
    };

    if !resp.status().is_success() {
        return UpdateCheckResult {
            current_version: current,
            latest_version: None,
            has_update: false,
            release_notes: None,
            download_url: None,
            sha256: None,
            message: format!("更新源返回 HTTP {}", resp.status().as_u16()),
            configured: true,
        };
    }

    match resp.json::<Value>().await {
        Ok(json) => parse_update_manifest(&json, &current).unwrap_or_else(|err| UpdateCheckResult {
            current_version: current,
            latest_version: None,
            has_update: false,
            release_notes: None,
            download_url: None,
            sha256: None,
            message: err,
            configured: true,
        }),
        Err(e) => UpdateCheckResult {
            current_version: current,
            latest_version: None,
            has_update: false,
            release_notes: None,
            download_url: None,
            sha256: None,
            message: format!("解析更新响应失败: {e}"),
            configured: true,
        },
    }
}

fn scan_local_installer_dir(dir: &Path, current: &str) -> Option<UpdateCheckResult> {
    let entries = fs::read_dir(dir).ok()?;
    let mut best: Option<(String, PathBuf)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name()?.to_str()?;
        if name.starts_with("AuraQuota_") && name.ends_with(".exe") {
            let trimmed = name.trim_start_matches("AuraQuota_");
            if let Some(ver) = trimmed.split('_').next() {
                let take = best
                    .as_ref()
                    .is_none_or(|(b_ver, _)| is_newer_version(ver, b_ver));
                if take {
                    best = Some((ver.to_string(), path));
                }
            }
        }
    }

    let (latest, path) = best?;
    let has_update = is_newer_version(&latest, current);
    Some(UpdateCheckResult {
        current_version: current.to_string(),
        latest_version: Some(latest.clone()),
        has_update,
        release_notes: Some(format!("来自本地目录的安装包：{}", path.display())),
        download_url: Some(path.to_string_lossy().to_string()),
        sha256: None,
        message: if has_update {
            format!("在本地更新目录发现新版本 v{latest}")
        } else {
            format!("当前已是最新版本 v{current}")
        },
        configured: true,
    })
}

pub async fn download_and_install_update(
    app: &AppHandle,
    download_url: &str,
    expected_sha256: Option<&str>,
) -> Result<(), String> {
    let local_src = Path::new(download_url);
    let installer_path = if local_src.exists() && local_src.is_file() {
        if let Some(expected) = expected_sha256 {
            let bytes = fs::read(local_src).map_err(|e| format!("读取安装包失败: {e}"))?;
            verify_sha256(&bytes, expected)?;
        }
        let _ = app.emit(
            "update-progress",
            UpdateProgressEvent {
                downloaded: 100,
                total: Some(100),
                pct: 100,
            },
        );
        local_src.to_path_buf()
    } else {
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .user_agent(format!("AuraQuota/{}", current_app_version()))
            .build()
            .map_err(|e| format!("创建下载客户端失败: {e}"))?;

        let mut resp = client
            .get(download_url)
            .send()
            .await
            .map_err(|e| format!("下载更新失败: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("下载返回 HTTP {}", resp.status().as_u16()));
        }

        let total = resp.content_length();
        let target = std::env::temp_dir().join(format!(
            "AuraQuota_update_{}.exe",
            uuid::Uuid::new_v4()
        ));
        let mut file =
            fs::File::create(&target).map_err(|e| format!("创建临时安装包失败: {e}"))?;
        let mut hasher = Sha256::new();
        let mut downloaded: u64 = 0;

        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| format!("接收更新包失败: {e}"))?
        {
            file.write_all(&chunk)
                .map_err(|e| format!("写入更新包失败: {e}"))?;
            hasher.update(&chunk);
            downloaded += chunk.len() as u64;
            let pct = total
                .filter(|t| *t > 0)
                .map(|t| ((downloaded as f64 / t as f64) * 100.0).round().min(100.0) as u8)
                .unwrap_or(50);
            let _ = app.emit(
                "update-progress",
                UpdateProgressEvent {
                    downloaded,
                    total,
                    pct,
                },
            );
        }

        if let Some(expected) = expected_sha256 {
            let digest = format!("{:x}", hasher.finalize());
            if !digest.eq_ignore_ascii_case(expected.trim()) {
                let _ = fs::remove_file(&target);
                return Err("安装包 SHA-256 校验失败，已中止升级".to_string());
            }
        }
        target
    };

    // Launch NSIS silent installer (/S) and exit so files can be replaced cleanly
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        std::process::Command::new(&installer_path)
            .arg("/S")
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("启动安装程序失败: {e}"))?;
    }

    app.exit(0);
    Ok(())
}

fn verify_sha256(bytes: &[u8], expected: &str) -> Result<(), String> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = format!("{:x}", hasher.finalize());
    if !digest.eq_ignore_ascii_case(expected.trim()) {
        return Err("安装包 SHA-256 校验失败".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compares_semver_versions() {
        assert!(is_newer_version("0.2.1", "0.2.0"));
        assert!(is_newer_version("v1.0.0", "0.9.9"));
        assert!(!is_newer_version("0.2.0", "0.2.0"));
        assert!(!is_newer_version("0.1.9", "0.2.0"));
    }

    #[test]
    fn parses_github_release_json() {
        let gh = json!({
            "tag_name": "v0.3.0",
            "body": "- 新增离线扫描\n- 优化深色模式",
            "assets": [
                {
                    "name": "AuraQuota_0.3.0_x64-setup.exe",
                    "browser_download_url": "https://example.com/AuraQuota_0.3.0_x64-setup.exe"
                }
            ]
        });
        let res = parse_update_manifest(&gh, "0.2.0").expect("parse gh");
        assert!(res.has_update);
        assert_eq!(res.latest_version.as_deref(), Some("0.3.0"));
        assert_eq!(
            res.download_url.as_deref(),
            Some("https://example.com/AuraQuota_0.3.0_x64-setup.exe")
        );
    }

    #[test]
    fn parses_latest_json() {
        let manifest = json!({
            "version": "0.2.5",
            "notes": "Bug fixes",
            "platforms": {
                "windows-x86_64": {
                    "url": "https://example.com/setup.exe",
                    "sha256": "abc123"
                }
            }
        });
        let res = parse_update_manifest(&manifest, "0.2.0").expect("parse latest.json");
        assert!(res.has_update);
        assert_eq!(res.sha256.as_deref(), Some("abc123"));
    }
}
