use reqwest::redirect::Policy;
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use url::Url;

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

pub fn extract_github_owner_repo(raw_url: &str) -> Option<(String, String)> {
    let trimmed = raw_url.trim();
    if trimmed.to_lowercase().ends_with(".json") {
        return None;
    }
    let parsed = Url::parse(trimmed).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let segs: Vec<&str> = parsed.path_segments()?.filter(|s| !s.is_empty()).collect();

    if host == "api.github.com" {
        if segs.len() >= 3 && segs[0].eq_ignore_ascii_case("repos") {
            return Some((segs[1].to_string(), segs[2].to_string()));
        }
        return None;
    }

    if host == "github.com" || host == "www.github.com" {
        if segs.len() == 2
            || (segs.len() >= 3 && segs[2].eq_ignore_ascii_case("releases"))
        {
            return Some((segs[0].to_string(), segs[1].to_string()));
        }
    }
    None
}

pub fn extract_tag_from_github_location(location: &str) -> Option<(String, String)> {
    let marker = "/releases/tag/";
    let idx = location.find(marker)?;
    let tail = &location[idx + marker.len()..];
    let tag: String = tail
        .split(['?', '#', '/'])
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if tag.is_empty() {
        return None;
    }
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    Some((tag, version))
}

pub fn extract_exe_asset_from_expanded_assets_html(
    html: &str,
    owner: &str,
    repo: &str,
) -> Option<String> {
    let prefix = format!("/{owner}/{repo}/releases/download/");
    for part in html.split("href=\"") {
        if let Some(end_idx) = part.find('"') {
            let href = &part[..end_idx];
            if href.contains(&prefix) && href.to_ascii_lowercase().ends_with(".exe") {
                if href.starts_with("http://") || href.starts_with("https://") {
                    return Some(href.to_string());
                }
                return Some(format!("https://github.com{href}"));
            }
        }
    }
    None
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

    Err("更新清单格式不受支持（需 GitHub Releases 或 latest.json）".to_string())
}

async fn check_github_without_api(
    client: &Client,
    owner: &str,
    repo: &str,
    current: &str,
) -> Result<UpdateCheckResult, String> {
    // 1. Try direct CDN latest.json asset (no API rate limit!)
    let latest_json_url =
        format!("https://github.com/{owner}/{repo}/releases/latest/download/latest.json");
    if let Ok(resp) = client
        .get(&latest_json_url)
        .header("Accept", "application/json")
        .send()
        .await
    {
        if resp.status().is_success() {
            if let Ok(json) = resp.json::<Value>().await {
                if let Ok(res) = parse_update_manifest(&json, current) {
                    return Ok(res);
                }
            }
        }
    }

    // 2. Try GitHub web /releases/latest 302 redirect (no API rate limit!)
    let no_redirect = Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(12))
        .user_agent(format!("AuraQuota/{current}"))
        .build()
        .unwrap_or_else(|_| Client::new());

    let web_latest = format!("https://github.com/{owner}/{repo}/releases/latest");
    let last_err;

    match no_redirect.get(&web_latest).send().await {
        Ok(r) => {
            if let Some(loc) = r.headers().get("location").and_then(|v| v.to_str().ok()) {
                if let Some((tag, latest)) = extract_tag_from_github_location(loc) {
                    let expanded_url =
                        format!("https://github.com/{owner}/{repo}/releases/expanded_assets/{tag}");
                    let exe_url = match client.get(&expanded_url).send().await {
                        Ok(exp) if exp.status().is_success() => exp
                            .text()
                            .await
                            .ok()
                            .and_then(|html| {
                                extract_exe_asset_from_expanded_assets_html(&html, owner, repo)
                            }),
                        _ => None,
                    }
                    .unwrap_or_else(|| {
                        format!(
                            "https://github.com/{owner}/{repo}/releases/download/{tag}/AuraQuota_{latest}_x64-setup.exe"
                        )
                    });

                    let has_update = is_newer_version(&latest, current);
                    return Ok(UpdateCheckResult {
                        current_version: current.to_string(),
                        latest_version: Some(latest.clone()),
                        has_update,
                        release_notes: None,
                        download_url: Some(exe_url),
                        sha256: None,
                        message: if has_update {
                            format!("发现新版本 v{latest}")
                        } else {
                            format!("当前已是最新版本 v{current}")
                        },
                        configured: true,
                    });
                }
            }
            last_err = format!("GitHub Releases 页面返回 HTTP {}", r.status().as_u16());
        }
        Err(e) => {
            last_err = format!("连接 GitHub Releases 失败: {e}");
        }
    }

    // 3. Final fallback to GitHub REST API if web redirect was blocked
    let api_url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
    if let Ok(resp) = client
        .get(&api_url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
    {
        if resp.status().is_success() {
            if let Ok(json) = resp.json::<Value>().await {
                if let Ok(res) = parse_update_manifest(&json, current) {
                    return Ok(res);
                }
            }
        }
    }

    Err(last_err)
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
            message: "尚未配置更新源；请在设置中填写 GitHub Releases URL、latest.json 或本地目录"
                .to_string(),
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

    if let Some((owner, repo)) = extract_github_owner_repo(trimmed) {
        return match check_github_without_api(&client, &owner, &repo, &current).await {
            Ok(res) => res,
            Err(err) => UpdateCheckResult {
                current_version: current,
                latest_version: None,
                has_update: false,
                release_notes: None,
                download_url: None,
                sha256: None,
                message: err,
                configured: true,
            },
        };
    }

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

fn escape_ps_single_quoted(raw: &str) -> String {
    raw.replace('\'', "''")
}

pub fn build_windows_update_script(installer_path: &Path, relaunch_exe: Option<&Path>) -> String {
    let inst = escape_ps_single_quoted(&installer_path.to_string_lossy());
    let relaunch = relaunch_exe
        .map(|p| escape_ps_single_quoted(&p.to_string_lossy()))
        .unwrap_or_default();

    format!(
        "Start-Sleep -Milliseconds 1200; \
         $proc = Start-Process -FilePath '{inst}' -ArgumentList '/S' -PassThru; \
         if ($proc) {{ $proc.WaitForExit() }}; \
         Start-Sleep -Milliseconds 400; \
         $target = '{relaunch}'; \
         if (-not $target -or -not (Test-Path $target)) {{ $target = Join-Path $env:LOCALAPPDATA 'AuraQuota\\auraquota.exe' }}; \
         if (Test-Path $target) {{ Start-Process -FilePath $target }}"
    )
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

    // Wait briefly for process exit, install silently with NSIS (/S), then relaunch upgraded app
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let current_exe = std::env::current_exe().ok();
        let script = build_windows_update_script(&installer_path, current_exe.as_deref());
        std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
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
    fn parses_github_owner_repo_and_redirect_tag() {
        assert_eq!(
            extract_github_owner_repo("https://api.github.com/repos/ShawnLi-code/AuraQuota/releases/latest"),
            Some(("ShawnLi-code".into(), "AuraQuota".into()))
        );
        assert_eq!(
            extract_github_owner_repo("https://github.com/ShawnLi-code/AuraQuota/releases/latest"),
            Some(("ShawnLi-code".into(), "AuraQuota".into()))
        );
        assert_eq!(
            extract_github_owner_repo("https://github.com/ShawnLi-code/AuraQuota/releases/latest/download/latest.json"),
            None
        );

        assert_eq!(
            extract_tag_from_github_location("https://github.com/ShawnLi-code/AuraQuota/releases/tag/v0.3.2"),
            Some(("v0.3.2".into(), "0.3.2".into()))
        );
    }

    #[test]
    fn parses_expanded_assets_exe_link() {
        let html = r#"<a href="/ShawnLi-code/AuraQuota/releases/download/v0.3.2/AuraQuota_0.3.2_x64-setup.exe">Setup</a>"#;
        let url = extract_exe_asset_from_expanded_assets_html(html, "ShawnLi-code", "AuraQuota");
        assert_eq!(
            url.as_deref(),
            Some("https://github.com/ShawnLi-code/AuraQuota/releases/download/v0.3.2/AuraQuota_0.3.2_x64-setup.exe")
        );
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
