import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type {
  AccountQuota,
  AddAccountRequest,
  AppSettings,
  CapturedAuthResult,
  ClaudeOAuthStartResult,
  DetectLocalResult,
  PlatformId,
  QuotaSnapshot,
  ScanLocalSummary,
  SubscriptionTier,
  UpdateCheckResult,
  UpdateProgressEvent,
  WindowMode,
} from '../types';

export function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

const BROWSER_FALLBACK_ACCOUNTS: AccountQuota[] = [
  {
    id: 'demo-agy-1',
    platform: 'antigravity',
    alias: '开发主号',
    email: 'dev.main@example.com',
    tier: 'paid',
    planLabel: 'AI Pro',
    primaryPct: 84,
    resetIn: '3小时18分后重置',
    subPools: [
      { name: 'Gemini 4 / Flash (5h)', pct: 84 },
      { name: 'Claude / GPT 池 (周)', pct: 62 },
    ],
    status: 'demo',
    authMethod: 'demo',
    isDemo: true,
  },
  {
    id: 'demo-gem-1',
    platform: 'gemini',
    alias: '日常主号',
    email: 'daily.gem@example.com',
    tier: 'paid',
    planLabel: 'AI Pro',
    primaryPct: 67,
    resetIn: '3:30 PM 重置',
    subPools: [
      { name: '5小时当前窗口 (Current usage)', pct: 67, detail: '已使用 33%', resetIn: '3:30 PM 重置' },
      { name: '7天周限额 (Weekly limit)', pct: 98, detail: '已使用 2%', resetIn: 'Nov 25 12:30 AM 重置' },
    ],
    status: 'demo',
    authMethod: 'demo',
    isDemo: true,
  },
  {
    id: 'demo-claude-1',
    platform: 'claude',
    alias: '主力号',
    email: 'studio@example.com',
    tier: 'paid',
    planLabel: 'Pro',
    primaryPct: 22,
    resetIn: '1小时12分后重置',
    subPools: [
      { name: '5小时会话限额', pct: 22 },
      { name: '周总额度', pct: 54 },
    ],
    status: 'demo',
    authMethod: 'demo',
    isDemo: true,
  },
  {
    id: 'demo-cc-1',
    platform: 'commandcode',
    alias: 'GOAT 主力号',
    email: 'goat.dev@example.com',
    tier: 'paid',
    planLabel: 'GOAT',
    primaryPct: 74,
    resetIn: '3小时50分后重置',
    subPools: [
      { name: '5小时窗口（月额 20%）', pct: 74 },
      { name: '7天周窗口（月额 50%）', pct: 66 },
      { name: '月度包含额度', pct: 82, detail: '仅计算订阅内月度配额' },
    ],
    extraBalance: {
      label: '充值余额 (Purchased Credits)',
      amount: '$18.50',
      note: '可结转 · 独立于月度配额',
    },
    status: 'demo',
    authMethod: 'demo',
    isDemo: true,
  },
];

const DEFAULT_GITHUB_FEED =
  'https://api.github.com/repos/ShawnLi-code/AuraQuota/releases/latest';

let browserDemoState = [...BROWSER_FALLBACK_ACCOUNTS];
let browserSettings: AppSettings = {
  theme: 'dark',
  autoCheckUpdates: true,
  updateFeedUrl: DEFAULT_GITHUB_FEED,
  pollingIntervalSecs: 300,
};

export async function getQuotaSnapshot(): Promise<QuotaSnapshot> {
  if (!isTauriRuntime()) {
    return {
      accounts: browserDemoState,
      lastUpdatedAt: '预览',
      pollingIntervalSecs: 300,
    };
  }
  return invoke<QuotaSnapshot>('get_quota_snapshot');
}

export async function refreshAllQuotas(): Promise<QuotaSnapshot> {
  if (!isTauriRuntime()) {
    return {
      accounts: browserDemoState,
      lastUpdatedAt: '刚刚',
      pollingIntervalSecs: 300,
    };
  }
  return invoke<QuotaSnapshot>('refresh_all_quotas');
}

export async function refreshSingleAccount(accountId: string): Promise<AccountQuota> {
  if (!isTauriRuntime()) {
    const found = browserDemoState.find((a) => a.id === accountId);
    if (!found) throw new Error('账号不存在');
    return found;
  }
  return invoke<AccountQuota>('refresh_single_account', { accountId });
}

export async function addAccount(req: AddAccountRequest): Promise<AccountQuota> {
  if (!isTauriRuntime()) {
    const created: AccountQuota = {
      id: `${req.platform}-${Date.now()}`,
      platform: req.platform,
      alias: req.alias,
      email: req.email,
      tier: req.tier,
      planLabel: req.planLabel,
      primaryPct: 88,
      resetIn: '4小时59分后重置',
      subPools: [
        { name: '当前主额度窗口', pct: 88 },
        { name: '周期总额度', pct: 94 },
      ],
      status: 'demo',
      authMethod: req.authMethod,
      isDemo: true,
    };
    browserDemoState = [created, ...browserDemoState];
    return created;
  }
  return invoke<AccountQuota>('add_account', { req });
}

export async function removeAccount(accountId: string): Promise<boolean> {
  if (!isTauriRuntime()) {
    browserDemoState = browserDemoState.filter((a) => a.id !== accountId);
    return true;
  }
  return invoke<boolean>('remove_account', { accountId });
}

export async function clearDemoAccounts(): Promise<QuotaSnapshot> {
  if (!isTauriRuntime()) {
    browserDemoState = browserDemoState.filter((a) => !a.isDemo);
    return {
      accounts: browserDemoState,
      lastUpdatedAt: '刚刚',
      pollingIntervalSecs: 300,
    };
  }
  return invoke<QuotaSnapshot>('clear_demo_accounts');
}

export async function restoreDemoAccounts(): Promise<QuotaSnapshot> {
  if (!isTauriRuntime()) {
    browserDemoState = [...BROWSER_FALLBACK_ACCOUNTS];
    return {
      accounts: browserDemoState,
      lastUpdatedAt: '刚刚',
      pollingIntervalSecs: 300,
    };
  }
  return invoke<QuotaSnapshot>('restore_demo_accounts');
}

export async function detectAntigravityLocal(alias?: string): Promise<DetectLocalResult> {
  if (!isTauriRuntime()) {
    return { found: false, message: '请在 Tauri 桌面端中检测本机反重力' };
  }
  return invoke<DetectLocalResult>('detect_antigravity_local', { alias });
}

export async function detectCodexCliLocal(alias?: string): Promise<DetectLocalResult> {
  if (!isTauriRuntime()) {
    return { found: false, message: '请在 Tauri 桌面端中读取本机 Codex CLI' };
  }
  return invoke<DetectLocalResult>('detect_codex_cli_local', { alias });
}

export async function detectClaudeCliLocal(alias?: string): Promise<DetectLocalResult> {
  if (!isTauriRuntime()) {
    return { found: false, message: '请在 Tauri 桌面端中读取本机 Claude Code' };
  }
  return invoke<DetectLocalResult>('detect_claude_cli_local', { alias });
}

export async function detectCommandcodeCliLocal(alias?: string): Promise<DetectLocalResult> {
  if (!isTauriRuntime()) {
    return { found: false, message: '请在 Tauri 桌面端中读取本机 Command Code CLI' };
  }
  return invoke<DetectLocalResult>('detect_commandcode_cli_local', { alias });
}

export async function detectOpencodeCliLocal(alias?: string): Promise<DetectLocalResult> {
  if (!isTauriRuntime()) {
    return { found: false, message: '请在 Tauri 桌面端中读取本机 OpenCode CLI' };
  }
  return invoke<DetectLocalResult>('detect_opencode_cli_local', { alias });
}

export async function scanLocalAccounts(): Promise<ScanLocalSummary> {
  if (!isTauriRuntime()) {
    return { items: [], foundCount: 0 };
  }
  return invoke<ScanLocalSummary>('scan_local_accounts');
}

export async function openInDefaultBrowser(platform: PlatformId): Promise<void> {
  if (!isTauriRuntime()) {
    window.open('https://gemini.google.com/usage', '_blank', 'noopener');
    return;
  }
  await invoke('open_in_default_browser', { platform });
}

export async function openAuthWebview(
  platform: PlatformId,
  accountId?: string
): Promise<string> {
  if (!isTauriRuntime()) {
    throw new Error('独立多账号窗口仅在 Tauri 桌面端可用');
  }
  return invoke<string>('open_auth_webview', { platform, accountId });
}

export async function captureAuthWebview(
  platform: PlatformId,
  sessionId: string,
  rebindAccountId?: string
): Promise<CapturedAuthResult> {
  if (!isTauriRuntime()) {
    throw new Error('独立多账号窗口仅在 Tauri 桌面端可用');
  }
  return invoke<CapturedAuthResult>('capture_auth_webview', {
    platform,
    sessionId,
    rebindAccountId,
  });
}

export async function startCodexBrowserOauth(params?: {
  alias?: string;
  planLabel?: string;
  tier?: SubscriptionTier;
  rebindAccountId?: string;
}): Promise<AccountQuota> {
  if (!isTauriRuntime()) {
    throw new Error('默认浏览器 OAuth 登录仅在 Tauri 桌面端可用');
  }
  return invoke<AccountQuota>('start_codex_browser_oauth', {
    alias: params?.alias ?? null,
    planLabel: params?.planLabel ?? null,
    tier: params?.tier ?? null,
    rebindAccountId: params?.rebindAccountId ?? null,
  });
}

export async function startClaudeBrowserOauth(): Promise<ClaudeOAuthStartResult> {
  if (!isTauriRuntime()) {
    throw new Error('默认浏览器 OAuth 登录仅在 Tauri 桌面端可用');
  }
  return invoke<ClaudeOAuthStartResult>('start_claude_browser_oauth');
}

export async function completeClaudeBrowserOauth(params: {
  flowId: string;
  authCode: string;
  alias?: string;
  planLabel?: string;
  tier?: SubscriptionTier;
  rebindAccountId?: string;
}): Promise<AccountQuota> {
  if (!isTauriRuntime()) {
    throw new Error('默认浏览器 OAuth 登录仅在 Tauri 桌面端可用');
  }
  return invoke<AccountQuota>('complete_claude_browser_oauth', {
    flowId: params.flowId,
    authCode: params.authCode,
    alias: params.alias ?? null,
    planLabel: params.planLabel ?? null,
    tier: params.tier ?? null,
    rebindAccountId: params.rebindAccountId ?? null,
  });
}

export async function cancelBrowserOauth(): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke('cancel_browser_oauth');
}

export async function setWindowMode(mode: WindowMode): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke('set_window_mode', { mode });
}

export async function snapOrbToEdge(): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke('snap_orb_to_edge');
}

export async function startWindowDragging(): Promise<void> {
  if (!isTauriRuntime()) return;
  await getCurrentWindow().startDragging();
}

export async function setThemeVibrancy(theme: 'dark' | 'light'): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke('set_theme_vibrancy', { theme });
}

export async function getAutostartEnabled(): Promise<boolean> {
  if (!isTauriRuntime()) return false;
  return invoke<boolean>('get_autostart_enabled');
}

export async function setAutostartEnabled(enabled: boolean): Promise<boolean> {
  if (!isTauriRuntime()) return enabled;
  return invoke<boolean>('set_autostart_enabled', { enabled });
}

export async function getAppSettings(): Promise<AppSettings> {
  if (!isTauriRuntime()) return browserSettings;
  return invoke<AppSettings>('get_app_settings');
}

export async function saveAppSettings(next: AppSettings): Promise<AppSettings> {
  if (!isTauriRuntime()) {
    browserSettings = next;
    return next;
  }
  return invoke<AppSettings>('save_app_settings', { next });
}

export async function checkForUpdates(feedUrl?: string): Promise<UpdateCheckResult> {
  if (!isTauriRuntime()) {
    return {
      currentVersion: '0.3.1',
      hasUpdate: false,
      message: '浏览器预览模式不检查更新',
      configured: false,
    };
  }
  return invoke<UpdateCheckResult>('check_for_updates', { feedUrl });
}

export async function installUpdate(
  downloadUrl: string,
  sha256?: string | null
): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke('install_update', { downloadUrl, sha256: sha256 ?? null });
}

export async function onQuotaUpdated(
  cb: (snapshot: QuotaSnapshot) => void
): Promise<UnlistenFn> {
  if (!isTauriRuntime()) return () => {};
  return listen<QuotaSnapshot>('quota-updated', (e) => cb(e.payload));
}

export async function onWindowModeChanged(
  cb: (mode: WindowMode) => void
): Promise<UnlistenFn> {
  if (!isTauriRuntime()) return () => {};
  return listen<WindowMode>('window-mode-changed', (e) => cb(e.payload));
}

export async function onOpenSettingsUpdates(cb: () => void): Promise<UnlistenFn> {
  if (!isTauriRuntime()) return () => {};
  return listen('open-settings-updates', () => cb());
}

export async function onUpdateAvailable(
  cb: (info: UpdateCheckResult) => void
): Promise<UnlistenFn> {
  if (!isTauriRuntime()) return () => {};
  return listen<UpdateCheckResult>('update-available', (e) => cb(e.payload));
}

export async function onUpdateProgress(
  cb: (progress: UpdateProgressEvent) => void
): Promise<UnlistenFn> {
  if (!isTauriRuntime()) return () => {};
  return listen<UpdateProgressEvent>('update-progress', (e) => cb(e.payload));
}
