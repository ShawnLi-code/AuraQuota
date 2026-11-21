use crate::models::PlatformId;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::oneshot;
use url::Url;

pub const BRIDGE_PREFIX: &str = "__AURAQUOTA_BRIDGE__:";

pub struct WebviewBridgeStore {
    waiters: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    scrape_lock: tokio::sync::Mutex<()>,
}

impl Default for WebviewBridgeStore {
    fn default() -> Self {
        Self {
            waiters: Mutex::new(HashMap::new()),
            scrape_lock: tokio::sync::Mutex::new(()),
        }
    }
}

impl WebviewBridgeStore {
    pub fn register_waiter(&self, label: &str) -> oneshot::Receiver<Value> {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut guard) = self.waiters.lock() {
            guard.insert(label.to_string(), tx);
        }
        rx
    }

    pub fn cancel_waiter(&self, label: &str) {
        if let Ok(mut guard) = self.waiters.lock() {
            guard.remove(label);
        }
    }

    pub fn deliver_from_title(&self, label: &str, title: &str) -> bool {
        let Some(val) = decode_bridge_title(title) else {
            return false;
        };
        if let Ok(mut guard) = self.waiters.lock() {
            if let Some(tx) = guard.remove(label) {
                let _ = tx.send(val);
                return true;
            }
        }
        false
    }
}

pub fn decode_bridge_title(title: &str) -> Option<Value> {
    let encoded = title.strip_prefix(BRIDGE_PREFIX)?;
    let bytes = STANDARD.decode(encoded.trim()).ok()?;
    serde_json::from_slice::<Value>(&bytes).ok()
}

pub fn usage_url_for(platform: PlatformId) -> &'static str {
    match platform {
        PlatformId::Gemini => "https://gemini.google.com/usage",
        PlatformId::Antigravity => "https://antigravity.google",
        PlatformId::Claude => "https://claude.ai/settings/usage",
        PlatformId::Codex => "https://chatgpt.com/codex/settings/usage",
        PlatformId::Commandcode => "https://commandcode.ai",
        PlatformId::Opencode => "https://opencode.ai/zen",
    }
}

pub fn scraper_script_for(platform: PlatformId, max_wait_ms: u64) -> String {
    let body_logic = match platform {
        PlatformId::Gemini => r#"
      if (location.hostname.includes('accounts.google.com')) {
        return send({ ok: false, authRequired: true, url: location.href });
      }
      const deadline = Date.now() + MAX_WAIT;
      const tryRead = () => {
        if (location.hostname.includes('accounts.google.com')) {
          return send({ ok: false, authRequired: true, url: location.href });
        }
        const text = (document.body && document.body.innerText) ? document.body.innerText : '';
        const hasUsageKeywords = /(Current usage|Weekly limit|Usage limits|当前用量|当前使用|每周限额|周限额)/i.test(text);
        const hasPct = /\d+(?:\.\d+)?\s*%/.test(text);
        const profileNode = document.querySelector('[aria-label*="@"], [data-email], button[aria-label*="Google Account"], a[aria-label*="@"]');
        const profileHint = profileNode ? (profileNode.getAttribute('aria-label') || profileNode.getAttribute('data-email') || profileNode.innerText || '') : '';
        if (hasUsageKeywords && hasPct) {
          return send({
            ok: true,
            authRequired: false,
            url: location.href,
            bodyText: text.slice(0, 4500),
            profileHint: profileHint.slice(0, 300)
          });
        }
        if (Date.now() >= deadline) {
          return send({
            ok: false,
            authRequired: /Sign in|登录/i.test(text) && !hasUsageKeywords,
            url: location.href,
            bodyText: text.slice(0, 4500),
            profileHint: profileHint.slice(0, 300)
          });
        }
        setTimeout(tryRead, 400);
      };
      tryRead();
"#,
        PlatformId::Claude => r#"
      if (location.pathname.includes('/login')) {
        return send({ ok: false, authRequired: true, url: location.href });
      }
      const deadline = Date.now() + MAX_WAIT;
      const run = async () => {
        try {
          const orgRes = await fetch('/api/organizations', { credentials: 'include' });
          if (orgRes.status === 401 || orgRes.status === 403) {
            return send({ ok: false, authRequired: true, url: location.href });
          }
          if (orgRes.ok) {
            const orgs = await orgRes.json();
            const org = Array.isArray(orgs) && orgs.length ? orgs[0] : null;
            if (org && org.uuid) {
              const uRes = await fetch('/api/organizations/' + org.uuid + '/usage?cedar_ember=1', { credentials: 'include' });
              if (uRes.ok) {
                const apiUsage = await uRes.json();
                const text = (document.body && document.body.innerText) ? document.body.innerText.slice(0, 3000) : '';
                return send({ ok: true, authRequired: false, url: location.href, org, apiUsage, bodyText: text });
              }
            }
          }
        } catch (_e) {}
        const pollDom = () => {
          const text = (document.body && document.body.innerText) ? document.body.innerText : '';
          if (/\d+%/.test(text) && /(session|weekly|限额|用量|usage)/i.test(text)) {
            return send({ ok: true, authRequired: false, url: location.href, bodyText: text.slice(0, 4500) });
          }
          if (Date.now() >= deadline) {
            return send({ ok: false, authRequired: location.pathname.includes('/login'), url: location.href, bodyText: text.slice(0, 4500) });
          }
          setTimeout(pollDom, 450);
        };
        pollDom();
      };
      run();
"#,
        PlatformId::Codex => r#"
      if (location.pathname.includes('/auth/login')) {
        return send({ ok: false, authRequired: true, url: location.href });
      }
      const deadline = Date.now() + MAX_WAIT;
      const run = async () => {
        let email = null;
        try {
          const sRes = await fetch('/api/auth/session', { credentials: 'include' });
          if (sRes.ok) {
            const sess = await sRes.json();
            email = (sess && sess.user && sess.user.email) ? sess.user.email : null;
            const headers = { Accept: 'application/json' };
            if (sess && sess.accessToken) headers.Authorization = 'Bearer ' + sess.accessToken;
            const uRes = await fetch('/backend-api/wham/usage', { headers, credentials: 'include' });
            if (uRes.status === 401 || uRes.status === 403) {
              return send({ ok: false, authRequired: true, url: location.href });
            }
            if (uRes.ok) {
              const apiUsage = await uRes.json();
              return send({ ok: true, authRequired: false, url: location.href, email, apiUsage });
            }
          }
        } catch (_e) {}
        const pollDom = () => {
          const text = (document.body && document.body.innerText) ? document.body.innerText : '';
          if (/\d+%/.test(text) && /(5-hour|5 hour|weekly|usage|remaining|left|剩余|限额)/i.test(text)) {
            return send({ ok: true, authRequired: false, url: location.href, email, bodyText: text.slice(0, 4500) });
          }
          if (Date.now() >= deadline) {
            return send({ ok: false, authRequired: /Log in|Sign up/i.test(text), url: location.href, email, bodyText: text.slice(0, 4500) });
          }
          setTimeout(pollDom, 450);
        };
        pollDom();
      };
      run();
"#,
        PlatformId::Commandcode => r#"
      const run = async () => {
        try {
          const cRes = await fetch('https://api.commandcode.ai/internal/billing/credits', { credentials: 'include' });
          if (cRes.status === 401 || cRes.status === 403) {
            return send({ ok: false, authRequired: true, url: location.href });
          }
          if (cRes.ok) {
            const creditsJson = await cRes.json();
            let subJson = null;
            try {
              const bRes = await fetch('https://api.commandcode.ai/internal/billing/subscriptions', { credentials: 'include' });
              if (bRes.ok) subJson = await bRes.json();
            } catch (_e1) {}
            let summaryJson = null;
            try {
              const sRes = await fetch('https://api.commandcode.ai/internal/usage/summary', { credentials: 'include' });
              if (sRes.ok) summaryJson = await sRes.json();
            } catch (_e2) {}
            return send({ ok: true, authRequired: false, url: location.href, creditsJson, subJson, summaryJson });
          }
        } catch (_e) {}
        const text = (document.body && document.body.innerText) ? document.body.innerText.slice(0, 4500) : '';
        send({ ok: false, authRequired: /Sign in|Log in/i.test(text), url: location.href, bodyText: text });
      };
      run();
"#,
        PlatformId::Opencode | PlatformId::Antigravity => r#"
      const run = async () => {
        try {
          const uRes = await fetch('/zen/go/v1/usage', { credentials: 'include' });
          let billingJson = null;
          try {
            const bRes = await fetch('/console/api/billing/status', { credentials: 'include' });
            if (bRes.ok) billingJson = await bRes.json();
          } catch (_e2) {}
          if (uRes.ok) {
            const apiUsage = await uRes.json();
            return send({ ok: true, authRequired: false, url: location.href, apiUsage, billingJson });
          }
          if (uRes.status === 401 || uRes.status === 403) {
            return send({ ok: false, authRequired: true, url: location.href });
          }
        } catch (_e) {}
        const text = (document.body && document.body.innerText) ? document.body.innerText.slice(0, 4500) : '';
        send({ ok: false, authRequired: /Sign in|Log in/i.test(text), url: location.href, bodyText: text });
      };
      run();
"#,
    };

    format!(
        r#"(() => {{
  const MAX_WAIT = {max_wait_ms};
  const PREFIX = "{BRIDGE_PREFIX}";
  const send = (payload) => {{
    try {{
      const prev = document.title;
      const bytes = new TextEncoder().encode(JSON.stringify(payload));
      let bin = '';
      for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
      document.title = PREFIX + btoa(bin);
      setTimeout(() => {{ try {{ document.title = prev; }} catch (_e) {{}} }}, 60);
    }} catch (_err) {{}}
  }};
{body_logic}
}})();"#
    )
}

pub async fn scrape_existing_webview(
    win: &WebviewWindow,
    bridge: &WebviewBridgeStore,
    platform: PlatformId,
    timeout_ms: u64,
) -> Option<Value> {
    let label = win.label().to_string();
    let rx = bridge.register_waiter(&label);
    let script = scraper_script_for(platform, timeout_ms.saturating_sub(500).max(1500));

    if win.eval(&script).is_err() {
        bridge.cancel_waiter(&label);
        return None;
    }

    match tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await {
        Ok(Ok(val)) => Some(val),
        _ => {
            bridge.cancel_waiter(&label);
            None
        }
    }
}

pub async fn scrape_hidden_profile(
    app: &AppHandle,
    bridge: &Arc<WebviewBridgeStore>,
    platform: PlatformId,
    account_id: &str,
    profile_dir: PathBuf,
    timeout_ms: u64,
) -> Result<Value, String> {
    let _guard = bridge.scrape_lock.lock().await;

    // If user currently has the visible auth window open for this account, reuse it directly
    let auth_label = format!("auth-{account_id}");
    if let Some(existing_auth) = app.get_webview_window(&auth_label) {
        if let Some(val) = scrape_existing_webview(&existing_auth, bridge, platform, 5_000).await
        {
            return Ok(val);
        }
    }

    let scrape_label = format!("scrape-{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let target_url = Url::parse(usage_url_for(platform))
        .map_err(|e| format!("无效目标 URL: {e}"))?;

    let bridge_clone = Arc::clone(bridge);
    let win = WebviewWindowBuilder::new(app, &scrape_label, WebviewUrl::External(target_url))
        .title("AuraQuota Sync")
        .visible(false)
        .skip_taskbar(true)
        .focused(false)
        .inner_size(1100.0, 820.0)
        .data_directory(profile_dir)
        .on_document_title_changed(move |window, title| {
            bridge_clone.deliver_from_title(window.label(), &title);
        })
        .build()
        .map_err(|e| format!("创建后台同步 WebView2 失败: {e}"))?;

    // Allow initial navigation & DOM hydration to begin
    tokio::time::sleep(Duration::from_millis(1800)).await;

    let rx = bridge.register_waiter(&scrape_label);
    let script = scraper_script_for(platform, timeout_ms.saturating_sub(2500).max(4000));

    let eval_res = win.eval(&script);
    let res = if let Err(e) = eval_res {
        bridge.cancel_waiter(&scrape_label);
        Err(format!("注入后台读取脚本失败: {e}"))
    } else {
        match tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await {
            Ok(Ok(val)) => Ok(val),
            Ok(Err(_)) => Err("后台 WebView2 通道意外关闭".to_string()),
            Err(_) => {
                bridge.cancel_waiter(&scrape_label);
                Err("后台读取页面超时，请检查网络或重新登录".to_string())
            }
        }
    };

    let _ = win.close();
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_bridge_base64_payload() {
        let json_str = r#"{"ok":true,"bodyText":"Current usage 33% used"}"#;
        let encoded = STANDARD.encode(json_str.as_bytes());
        let title = format!("{BRIDGE_PREFIX}{encoded}");
        let parsed = decode_bridge_title(&title).expect("should decode bridge title");
        assert_eq!(parsed.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            parsed.get("bodyText").and_then(|v| v.as_str()),
            Some("Current usage 33% used")
        );
        assert!(decode_bridge_title("Regular Page Title").is_none());
    }
}
