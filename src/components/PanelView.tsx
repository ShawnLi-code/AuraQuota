import React, { useRef } from 'react';
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
  Sparkles,
  Sun,
} from 'lucide-react';
import { isTauriRuntime } from '../api/tauriClient';
import {
  AccountQuota,
  PLATFORMS,
  PlatformId,
  SubscriptionTier,
  WindowMode,
} from '../types';
import { AccountCard } from './AccountCard';

interface PanelViewProps {
  accounts: AccountQuota[];
  lastUpdatedAt: string;
  filterPlatform: 'all' | PlatformId;
  filterTier: 'all' | SubscriptionTier;
  privacy: boolean;
  theme: 'dark' | 'light';
  expandedId: string | null;
  refreshingAll: boolean;
  hasUpdateBadge: boolean;
  onFilterPlatform: (p: 'all' | PlatformId) => void;
  onFilterTier: (t: 'all' | SubscriptionTier) => void;
  onTogglePrivacy: () => void;
  onToggleTheme: () => void;
  onToggleExpand: (id: string) => void;
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
  lastUpdatedAt,
  filterPlatform,
  filterTier,
  privacy,
  theme,
  expandedId,
  refreshingAll,
  hasUpdateBadge,
  onFilterPlatform,
  onFilterTier,
  onTogglePrivacy,
  onToggleTheme,
  onToggleExpand,
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
  const pillsRef = useRef<HTMLDivElement>(null);

  const filtered = accounts.filter((acc) => {
    const matchPlat = filterPlatform === 'all' || acc.platform === filterPlatform;
    const matchTier = filterTier === 'all' || acc.tier === filterTier;
    return matchPlat && matchTier;
  });

  const lowCount = accounts.filter((a) => a.primaryPct <= 25).length;
  const hasDemo = accounts.some((a) => a.isDemo);
  const frameRadius = isTauriRuntime() ? 'rounded-[8px]' : 'rounded-3xl';

  const handlePillsWheel = (e: React.WheelEvent<HTMLDivElement>) => {
    if (pillsRef.current && Math.abs(e.deltaY) > 0) {
      pillsRef.current.scrollLeft += e.deltaY;
    }
  };

  return (
    <aside
      aria-label="AuraQuota 额度监控面板"
      className={`w-full h-full glass-surface ${frameRadius} flex flex-col overflow-hidden`}
    >
      {/* Header (clean 5-button toolbar + draggable title cluster) */}
      <header
        className="px-4 pt-3.5 pb-2.5 flex items-center justify-between gap-2"
        style={{ borderBottom: '1px solid var(--divider)' }}
      >
        <div className="flex items-center gap-2.5 min-w-0 flex-1" data-tauri-drag-region>
          <div
            data-tauri-drag-region
            className="w-8 h-8 rounded-xl bg-gradient-to-br from-sky-400 via-indigo-500 to-purple-500 flex items-center justify-center shadow-md shrink-0"
          >
            <Sparkles className="w-4 h-4 text-white pointer-events-none" aria-hidden="true" />
          </div>
          <div className="min-w-0" data-tauri-drag-region>
            <div className="flex items-center gap-1.5" data-tauri-drag-region>
              <h1 className="text-sm font-semibold tracking-tight" data-tauri-drag-region>
                AuraQuota
              </h1>
              <span
                data-tauri-drag-region
                className="text-[10px] px-1.5 py-0.5 rounded-full font-medium tabular-nums"
                style={{
                  background: 'var(--pill-bg)',
                  color: 'var(--text-sub)',
                  border: '1px solid var(--border-subtle)',
                }}
              >
                {accounts.length} 账号
              </span>
            </div>
            <p
              className="text-[11px] truncate tabular-nums"
              style={{ color: 'var(--text-sub)' }}
              data-tauri-drag-region
            >
              {lastUpdatedAt ? `${lastUpdatedAt} 已同步` : '刚刚同步'} · 6 平台
            </p>
          </div>
        </div>

        <div className="flex items-center gap-1 shrink-0">
          <button
            type="button"
            onClick={() => void onRefreshAll()}
            disabled={refreshingAll}
            title="刷新全部额度"
            aria-label="刷新全部额度"
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

      {/* Filters */}
      <div className="px-4 pt-2.5 pb-1.5 space-y-2">
        {/* Single-row smooth horizontal pill bar */}
        <div
          ref={pillsRef}
          onWheel={handlePillsWheel}
          className="flex items-center gap-1.5 overflow-x-auto no-scrollbar py-0.5"
          role="group"
          aria-label="按平台筛选"
        >
          {(
            Object.entries(PLATFORMS) as Array<['all' | PlatformId, { name: string; color: string }]>
          ).map(([key, info]) => {
            const active = filterPlatform === key;
            const count =
              key === 'all'
                ? accounts.length
                : accounts.filter((a) => a.platform === key).length;
            return (
              <button
                key={key}
                type="button"
                aria-pressed={active}
                onClick={() => onFilterPlatform(key)}
                className="shrink-0 px-2.5 py-1 rounded-xl text-[11px] font-medium transition flex items-center gap-1.5"
                style={
                  active
                    ? {
                        background: 'var(--pill-active-bg)',
                        border: '1px solid var(--pill-active-border)',
                        color: 'var(--text-main)',
                      }
                    : {
                        background: 'var(--pill-bg)',
                        border: '1px solid var(--border-subtle)',
                        color: 'var(--text-sub)',
                      }
                }
              >
                <span
                  aria-hidden="true"
                  className="w-1.5 h-1.5 rounded-full"
                  style={{ background: info.color }}
                />
                <span>{info.name}</span>
                <span className="text-[10px] opacity-65 tabular-nums">{count}</span>
              </button>
            );
          })}
        </div>

        <div className="flex items-center justify-between text-[11px]">
          <div className="flex items-center gap-1" role="group" aria-label="按订阅方案筛选">
            {(
              [
                ['all', '全部'],
                ['paid', '付费'],
                ['free', '免费'],
              ] as const
            ).map(([val, label]) => {
              const active = filterTier === val;
              return (
                <button
                  key={val}
                  type="button"
                  aria-pressed={active}
                  onClick={() => onFilterTier(val)}
                  className="px-2.5 py-1 rounded-lg font-medium transition"
                  style={
                    active
                      ? {
                          background: 'var(--surface-card-hover)',
                          color: 'var(--text-main)',
                          border: '1px solid var(--border-subtle)',
                        }
                      : {
                          color: 'var(--text-sub)',
                          border: '1px solid transparent',
                        }
                  }
                >
                  {label}
                </button>
              );
            })}
          </div>

          <div className="flex items-center gap-2" style={{ color: 'var(--text-sub)' }}>
            <button
              type="button"
              onClick={() => void onScanLocalAll()}
              title="一键检索本机离线/在线客户端凭据"
              className="inline-flex items-center gap-1 hover:text-sky-400 transition"
            >
              <Laptop className="w-3.5 h-3.5" aria-hidden="true" />
              <span>本机扫描</span>
            </button>
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
        </div>

        {hasDemo && (
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
        )}
      </div>

      {/* Scrollable Account Quota Cards */}
      <main className="flex-1 overflow-y-auto custom-scroll px-4 py-1.5">
        {filtered.length === 0 ? (
          <div
            className="glass-card rounded-2xl p-6 text-center text-xs space-y-2.5"
            style={{ color: 'var(--text-sub)' }}
          >
            <p>当前筛选条件下暂无账号，可扫描本机已登录客户端或点击「+」添加。</p>
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
          <ul className="space-y-2.5 p-0 m-0">
            {filtered.map((acc) => (
              <AccountCard
                key={acc.id}
                account={acc}
                expanded={expandedId === acc.id}
                privacy={privacy}
                onToggleExpand={() => onToggleExpand(acc.id)}
                onRefresh={onRefreshSingle}
                onReauth={onReauth}
                onRemove={onRemoveAccount}
              />
            ))}
          </ul>
        )}
      </main>

      {/* Footer */}
      <footer
        className="px-4 py-2.5 flex items-center justify-between text-[11px]"
        style={{
          color: 'var(--text-sub)',
          borderTop: '1px solid var(--divider)',
        }}
      >
        <div className="flex items-center gap-1.5">
          <span aria-hidden="true" className="w-1.5 h-1.5 rounded-full bg-emerald-500" />
          <span>自动轮询中</span>
        </div>
        {lowCount > 0 ? (
          <span className="font-medium" style={{ color: 'var(--pct-mid)' }}>
            {lowCount} 个账号额度偏低
          </span>
        ) : (
          <span className="font-medium text-emerald-500">全部账号状态健康</span>
        )}
      </footer>
    </aside>
  );
};
