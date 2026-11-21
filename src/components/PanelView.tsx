import React from 'react';
import {
  Eye,
  EyeOff,
  Laptop,
  Minimize2,
  Moon,
  PanelRightClose,
  Plus,
  RefreshCw,
  Settings,
  Sun,
} from 'lucide-react';
import { isTauriRuntime } from '../api/tauriClient';
import { AccountQuota, WindowMode } from '../types';
import { AccountCard } from './AccountCard';
import { BrandLogo } from './BrandLogo';

interface PanelViewProps {
  accounts: AccountQuota[];
  activeAccountId: string | null;
  pinnedAccountId: string | null;
  activeReason: string | null;
  lastActiveMap: Record<string, number>;
  lastUpdatedAt: string;
  privacy: boolean;
  theme: 'dark' | 'light';
  expandedId: string | null;
  refreshingAll: boolean;
  hasUpdateBadge: boolean;
  onTogglePrivacy: () => void;
  onToggleTheme: () => void;
  onToggleExpand: (id: string) => void;
  onTogglePin: (id: string) => Promise<void>;
  onRefreshAll: () => Promise<void>;
  onRefreshSingle: (id: string) => Promise<void>;
  onReauth: (acc: AccountQuota) => void;
  onRemoveAccount: (id: string) => Promise<void>;
  onOpenAddModal: () => void;
  onOpenSettingsModal: () => void;
  onSwitchMode: (mode: WindowMode) => void;
  onScanLocalAll: () => Promise<void>;
  onClearDemo: () => Promise<void>;
  onRestoreDemo: () => Promise<void>;
}

export const PanelView: React.FC<PanelViewProps> = ({
  accounts,
  activeAccountId,
  pinnedAccountId,
  activeReason,
  lastActiveMap,
  lastUpdatedAt,
  privacy,
  theme,
  expandedId,
  refreshingAll,
  hasUpdateBadge,
  onTogglePrivacy,
  onToggleTheme,
  onToggleExpand,
  onTogglePin,
  onRefreshAll,
  onRefreshSingle,
  onReauth,
  onRemoveAccount,
  onOpenAddModal,
  onOpenSettingsModal,
  onSwitchMode,
  onScanLocalAll,
  onClearDemo,
  onRestoreDemo,
}) => {
  const spotlightAccount =
    accounts.find((a) => a.id === pinnedAccountId) ??
    accounts.find((a) => a.id === activeAccountId) ??
    accounts[0] ??
    null;

  const otherAccounts = spotlightAccount
    ? accounts
        .filter((a) => a.id !== spotlightAccount.id)
        .sort((a, b) => (lastActiveMap[b.id] ?? 0) - (lastActiveMap[a.id] ?? 0))
    : [];

  const isManuallyPinned =
    spotlightAccount !== null && spotlightAccount.id === pinnedAccountId;
  const lowCount = accounts.filter((a) => a.primaryPct <= 25).length;
  const hasDemo = accounts.some((a) => a.isDemo);
  const realCount = accounts.filter((a) => !a.isDemo).length;
  const frameRadius = isTauriRuntime() ? 'rounded-[8px]' : 'rounded-3xl';

  return (
    <aside
      aria-label="AuraQuota 额度监控面板"
      className={`w-full h-full glass-surface ${frameRadius} flex flex-col overflow-hidden`}
    >
      {/* Unified Minimal Header */}
      <header
        className="px-4 pt-3.5 pb-2.5 flex items-center justify-between gap-2"
        style={{ borderBottom: '1px solid var(--divider)' }}
      >
        <div className="flex items-center gap-2.5 min-w-0 flex-1" data-tauri-drag-region>
          <div data-tauri-drag-region className="pointer-events-none flex items-center">
            <BrandLogo size={30} />
          </div>
          <div className="min-w-0" data-tauri-drag-region>
            <div className="flex items-center gap-1.5" data-tauri-drag-region>
              <h1 className="text-sm font-semibold tracking-tight" data-tauri-drag-region>
                AuraQuota
              </h1>
            </div>
            <p
              className="text-[11px] truncate tabular-nums flex items-center gap-1.5"
              style={{ color: 'var(--text-sub)' }}
              data-tauri-drag-region
            >
              <span
                aria-hidden="true"
                className="w-1.5 h-1.5 rounded-full bg-emerald-400 shrink-0"
              />
              <span data-tauri-drag-region>
                {lastUpdatedAt ? `${lastUpdatedAt} 已同步` : '刚刚同步'} ·{' '}
                {accounts.length > 0 ? `已接入 ${accounts.length} 号` : '等待添加账号'}
              </span>
            </p>
          </div>
        </div>

        <div className="flex items-center gap-1 shrink-0">
          <button
            type="button"
            onClick={() => void onRefreshAll()}
            disabled={refreshingAll}
            title="刷新全部已有账号"
            aria-label="刷新全部已有账号"
            className="w-7 h-7 rounded-xl glass-icon-btn flex items-center justify-center"
          >
            <RefreshCw
              className={`w-3.5 h-3.5 ${refreshingAll ? 'spinning' : ''}`}
              aria-hidden="true"
            />
          </button>
          <button
            type="button"
            onClick={onOpenAddModal}
            title="添加账号"
            aria-label="添加账号"
            className="w-7 h-7 rounded-xl glass-icon-btn flex items-center justify-center"
          >
            <Plus className="w-4 h-4" aria-hidden="true" />
          </button>
          <button
            type="button"
            onClick={onOpenSettingsModal}
            title="设置与检查更新"
            aria-label="设置与检查更新"
            className="w-7 h-7 rounded-xl glass-icon-btn flex items-center justify-center relative"
          >
            <Settings className="w-3.5 h-3.5" aria-hidden="true" />
            {hasUpdateBadge && (
              <span
                aria-label="有可用更新"
                className="absolute top-1 right-1 w-1.5 h-1.5 rounded-full bg-rose-500"
              />
            )}
          </button>
          <button
            type="button"
            onClick={() => onSwitchMode('edge')}
            title="贴边收起"
            aria-label="贴边收起"
            className="w-7 h-7 rounded-xl glass-icon-btn flex items-center justify-center"
          >
            <PanelRightClose className="w-3.5 h-3.5" aria-hidden="true" />
          </button>
          <button
            type="button"
            onClick={() => onSwitchMode('orb')}
            title="缩成悬浮圆球"
            aria-label="缩成悬浮圆球"
            className="w-7 h-7 rounded-xl glass-icon-btn flex items-center justify-center"
          >
            <Minimize2 className="w-3.5 h-3.5" aria-hidden="true" />
          </button>
        </div>
      </header>

      {hasDemo && (
        <div className="px-4 pt-2.5">
          <div
            className="px-2.5 py-1.5 rounded-xl flex items-center justify-between text-[11px]"
            style={{
              background: 'var(--pill-active-bg)',
              border: '1px solid var(--pill-active-border)',
            }}
          >
            <span style={{ color: 'var(--text-sub)' }}>当前包含示例预览账号</span>
            <div className="flex items-center gap-2.5">
              <button
                type="button"
                onClick={() => void onScanLocalAll()}
                className="font-medium hover:underline"
                style={{ color: 'var(--pct-high)' }}
              >
                扫描本机账号
              </button>
              <button
                type="button"
                onClick={() => void onClearDemo()}
                style={{ color: 'var(--text-sub)' }}
                className="hover:underline"
              >
                清空示例
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Zero-Tab Focus Spotlight + Compact Account List */}
      <main className="flex-1 overflow-y-auto custom-scroll px-4 py-2.5 space-y-3">
        {accounts.length === 0 ? (
          <div
            className="glass-card rounded-2xl p-6 text-center text-xs space-y-3 mt-2"
            style={{ color: 'var(--text-sub)' }}
          >
            <p>暂无已保存的监控账号，可一键扫描本机已登录客户端或点击右上角「+」添加。</p>
            <div className="flex justify-center gap-2 flex-wrap">
              <button
                type="button"
                onClick={() => void onScanLocalAll()}
                className="px-3 py-1.5 rounded-xl font-medium"
                style={{
                  background: 'var(--pill-active-bg)',
                  border: '1px solid var(--pill-active-border)',
                  color: 'var(--pct-high)',
                }}
              >
                扫描本机客户端
              </button>
              <button
                type="button"
                onClick={onOpenAddModal}
                className="px-3 py-1.5 rounded-xl glass-icon-btn font-medium"
              >
                手动添加
              </button>
              <button
                type="button"
                onClick={() => void onRestoreDemo()}
                className="px-3 py-1.5 rounded-xl glass-icon-btn font-medium"
              >
                恢复示例
              </button>
            </div>
          </div>
        ) : (
          <>
            {spotlightAccount && (
              <section aria-label="当前焦点账号" className="space-y-1.5">
                <div
                  className="flex items-center justify-between px-0.5 text-[10.5px] font-medium"
                  style={{ color: 'var(--text-muted)' }}
                >
                  <span>
                    {isManuallyPinned ? '主焦点（已手动置顶）' : '当前正在使用（自动置顶）'}
                  </span>
                  <span className="truncate max-w-[160px] opacity-85">
                    {activeReason ?? ''}
                  </span>
                </div>
                <ul className="p-0 m-0">
                  <AccountCard
                    account={spotlightAccount}
                    variant="hero"
                    isPinned={isManuallyPinned}
                    activeLabel={activeReason}
                    expanded={true}
                    privacy={privacy}
                    onToggleExpand={() => onToggleExpand(spotlightAccount.id)}
                    onTogglePin={onTogglePin}
                    onRefresh={onRefreshSingle}
                    onReauth={onReauth}
                    onRemove={onRemoveAccount}
                  />
                </ul>
              </section>
            )}

            {otherAccounts.length > 0 && (
              <section aria-label="其余已保存账号" className="space-y-1.5">
                <div
                  className="flex items-center justify-between px-0.5 text-[10.5px] font-medium"
                  style={{ color: 'var(--text-muted)' }}
                >
                  <span>其余已保存账号（{otherAccounts.length}）</span>
                  <button
                    type="button"
                    onClick={() => void onScanLocalAll()}
                    className="inline-flex items-center gap-1 hover:text-sky-400 transition"
                    title="检索本机运行或已登录的客户端"
                  >
                    <Laptop className="w-3 h-3" aria-hidden="true" />
                    <span>本机扫描</span>
                  </button>
                </div>
                <ul className="space-y-2 p-0 m-0">
                  {otherAccounts.map((acc) => (
                    <AccountCard
                      key={acc.id}
                      account={acc}
                      variant="compact"
                      isPinned={false}
                      expanded={expandedId === acc.id}
                      privacy={privacy}
                      onToggleExpand={() => onToggleExpand(acc.id)}
                      onTogglePin={onTogglePin}
                      onRefresh={onRefreshSingle}
                      onReauth={onReauth}
                      onRemove={onRemoveAccount}
                    />
                  ))}
                </ul>
              </section>
            )}
          </>
        )}
      </main>

      {/* Whisper-quiet Footer */}
      <footer
        className="px-4 py-2 flex items-center justify-between text-[10.5px]"
        style={{
          color: 'var(--text-sub)',
          borderTop: '1px solid var(--divider)',
        }}
      >
        <div className="flex items-center gap-2">
          <span>{realCount > 0 ? `仅刷新已有 ${realCount} 号` : '示例预览中'}</span>
          {lowCount > 0 ? (
            <span className="font-medium" style={{ color: 'var(--pct-mid)' }}>
              · {lowCount} 项偏低
            </span>
          ) : (
            accounts.length > 0 && <span className="text-emerald-500">· 状态正常</span>
          )}
        </div>

        <div className="flex items-center gap-1.5">
          {otherAccounts.length === 0 && accounts.length > 0 && (
            <button
              type="button"
              onClick={() => void onScanLocalAll()}
              title="扫描本机客户端"
              className="p-1 rounded-lg hover:text-sky-400 transition inline-flex items-center gap-1"
            >
              <Laptop className="w-3.5 h-3.5" aria-hidden="true" />
            </button>
          )}
          <button
            type="button"
            onClick={onToggleTheme}
            title={theme === 'dark' ? '切换浅瓷模式' : '切换深空模式'}
            aria-label={theme === 'dark' ? '切换浅瓷模式' : '切换深空模式'}
            className="p-1 rounded-lg hover:text-sky-400 transition"
          >
            {theme === 'dark' ? (
              <Sun className="w-3.5 h-3.5" aria-hidden="true" />
            ) : (
              <Moon className="w-3.5 h-3.5" aria-hidden="true" />
            )}
          </button>
          <button
            type="button"
            aria-pressed={privacy}
            onClick={onTogglePrivacy}
            title={privacy ? '显示邮箱' : '隐藏邮箱'}
            aria-label={privacy ? '显示邮箱' : '隐藏邮箱'}
            className="p-1 rounded-lg hover:text-sky-400 transition"
          >
            {privacy ? (
              <EyeOff className="w-3.5 h-3.5" aria-hidden="true" />
            ) : (
              <Eye className="w-3.5 h-3.5" aria-hidden="true" />
            )}
          </button>
        </div>
      </footer>
    </aside>
  );
};
