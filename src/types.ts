export type PlatformId =
  | 'antigravity'
  | 'gemini'
  | 'codex'
  | 'claude'
  | 'commandcode'
  | 'opencode';

export type SubscriptionTier = 'paid' | 'free';

export type WindowMode = 'panel' | 'orb' | 'edge';

export type AuthMethod =
  | 'local_server'
  | 'local_cli'
  | 'oauth_token'
  | 'webview_session'
  | 'cookie'
  | 'api_key'
  | 'demo';

export type AccountStatus =
  | 'ok'
  | 'syncing'
  | 'auth_required'
  | 'offline'
  | 'error'
  | 'demo';

export interface SubPool {
  name: string;
  pct: number;
  detail?: string | null;
  resetIn?: string | null;
}

export interface ExtraBalance {
  label: string;
  amount: string;
  note?: string | null;
}

export interface AccountQuota {
  id: string;
  platform: PlatformId;
  alias: string;
  email: string;
  tier: SubscriptionTier;
  planLabel: string;
  primaryPct: number;
  resetIn: string;
  subPools: SubPool[];
  extraBalance?: ExtraBalance | null;
  status: AccountStatus;
  statusMessage?: string | null;
  authMethod: AuthMethod;
  lastSyncedAt?: string | null;
  isDemo: boolean;
}

export interface QuotaSnapshot {
  accounts: AccountQuota[];
  lastUpdatedAt: string;
  pollingIntervalSecs: number;
  activeAccountId?: string | null;
  pinnedAccountId?: string | null;
  activeReason?: string | null;
  lastActiveMap?: Record<string, number>;
}

export interface AddAccountRequest {
  platform: PlatformId;
  alias: string;
  email: string;
  tier: SubscriptionTier;
  planLabel: string;
  authMethod: AuthMethod;
  secretInput?: string | null;
  refreshToken?: string | null;
  profileId?: string | null;
}

export interface CapturedAuthResult {
  sessionId: string;
  captured: boolean;
  cookieCount: number;
  detectedTierHint?: string | null;
  detectedEmail?: string | null;
  detectedPlanLabel?: string | null;
  detectedPrimaryPct?: number | null;
}

export interface DetectLocalResult {
  found: boolean;
  message: string;
  account?: AccountQuota | null;
}

export interface LocalScanItem {
  platform: PlatformId;
  found: boolean;
  source: string;
  message: string;
  account?: AccountQuota | null;
}

export interface ScanLocalSummary {
  items: LocalScanItem[];
  foundCount: number;
}

export interface ClaudeOAuthStartResult {
  flowId: string;
  authorizeUrl: string;
}

export interface AppSettings {
  theme: 'dark' | 'light';
  autoCheckUpdates: boolean;
  updateFeedUrl: string;
  pollingIntervalSecs: number;
}

export interface UpdateCheckResult {
  currentVersion: string;
  latestVersion?: string | null;
  hasUpdate: boolean;
  releaseNotes?: string | null;
  downloadUrl?: string | null;
  sha256?: string | null;
  message: string;
  configured: boolean;
}

export interface UpdateProgressEvent {
  downloaded: number;
  total?: number | null;
  pct: number;
}

export interface PlatformMeta {
  name: string;
  shortLabel: string;
  color: string;
  hint: string;
  plans: Array<{ value: string; label: string; tier: SubscriptionTier; planLabel: string }>;
}

export const PLATFORMS: Record<'all' | PlatformId, { name: string; color: string }> = {
  all: { name: '全部', color: '#38bdf8' },
  antigravity: { name: '反重力', color: '#60a5fa' },
  gemini: { name: 'Gemini', color: '#a78bfa' },
  codex: { name: 'Codex', color: '#34d399' },
  claude: { name: 'Claude', color: '#fb923c' },
  commandcode: { name: 'Command Code', color: '#f472b6' },
  opencode: { name: 'OpenCode Go', color: '#2dd4bf' },
};

export const PLATFORM_DETAILS: Record<PlatformId, PlatformMeta> = {
  antigravity: {
    name: '反重力',
    shortLabel: '反重力 (AGY)',
    color: '#60a5fa',
    hint: '自动读取本机 Antigravity 客户端 / CLI 登录态，与 Gemini App 完全独立',
    plans: [
      { value: 'paid|AI Pro', label: '付费 · AI Pro', tier: 'paid', planLabel: 'AI Pro' },
      { value: 'paid|AI Ultra', label: '付费 · AI Ultra', tier: 'paid', planLabel: 'AI Ultra' },
      { value: 'free|Free', label: '免费 · Free', tier: 'free', planLabel: 'Free' },
    ],
  },
  gemini: {
    name: 'Gemini',
    shortLabel: 'Gemini App',
    color: '#a78bfa',
    hint: '每个 Google 账号使用独立目录隔离登录 gemini.google.com/usage，实时读取 5 小时与 7 天周额度',
    plans: [
      { value: 'paid|AI Pro', label: '付费 · AI Pro', tier: 'paid', planLabel: 'AI Pro' },
      { value: 'paid|AI Ultra', label: '付费 · AI Ultra', tier: 'paid', planLabel: 'AI Ultra' },
      { value: 'free|Free', label: '免费 · Free', tier: 'free', planLabel: 'Free' },
    ],
  },
  codex: {
    name: 'Codex',
    shortLabel: 'Codex',
    color: '#34d399',
    hint: '支持默认浏览器一键授权，或本机自动读取 ~/.codex/auth.json 监控 5h / 7d 额度',
    plans: [
      { value: 'paid|Plus', label: '付费 · Plus', tier: 'paid', planLabel: 'Plus' },
      { value: 'paid|Pro', label: '付费 · Pro', tier: 'paid', planLabel: 'Pro' },
      { value: 'free|Free', label: '免费 · Free', tier: 'free', planLabel: 'Free' },
    ],
  },
  claude: {
    name: 'Claude',
    shortLabel: 'Claude',
    color: '#fb923c',
    hint: '支持默认浏览器 OAuth 授权，或本机自动读取 Claude Code 凭据监控 5h / 7d 额度',
    plans: [
      { value: 'paid|Pro', label: '付费 · Pro', tier: 'paid', planLabel: 'Pro' },
      { value: 'paid|Max', label: '付费 · Max', tier: 'paid', planLabel: 'Max' },
      { value: 'free|Free', label: '免费 · Free', tier: 'free', planLabel: 'Free' },
    ],
  },
  commandcode: {
    name: 'Command Code',
    shortLabel: 'Command Code',
    color: '#f472b6',
    hint: '支持本机自动读取 ~/.commandcode/auth.json 或独立登录窗，充值余额单独展示',
    plans: [
      { value: 'paid|GOAT', label: '付费 · GOAT', tier: 'paid', planLabel: 'GOAT' },
      { value: 'paid|Go', label: '付费 · Go', tier: 'paid', planLabel: 'Go' },
      { value: 'paid|Pro', label: '付费 · Pro', tier: 'paid', planLabel: 'Pro' },
      { value: 'paid|Max', label: '付费 · Max', tier: 'paid', planLabel: 'Max' },
    ],
  },
  opencode: {
    name: 'OpenCode Go',
    shortLabel: 'OpenCode Go',
    color: '#2dd4bf',
    hint: '支持本机自动读取 OpenCode CLI 凭据、API Key 或独立登录窗监控额度及余额',
    plans: [
      { value: 'paid|Go', label: '付费 · Go', tier: 'paid', planLabel: 'Go' },
      { value: 'paid|Go Plus', label: '付费 · Go Plus', tier: 'paid', planLabel: 'Go Plus' },
      { value: 'paid|Go Teams', label: '付费 · Go Teams', tier: 'paid', planLabel: 'Go Teams' },
      { value: 'free|Free', label: '免费 · Free 模型', tier: 'free', planLabel: 'Free' },
    ],
  },
};

export const PLATFORM_ORDER: PlatformId[] = [
  'antigravity',
  'gemini',
  'codex',
  'claude',
  'commandcode',
  'opencode',
];
