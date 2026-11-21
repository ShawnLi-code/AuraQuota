import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  captureAuthWebview,
  clearDemoAccounts,
  detectAntigravityLocal,
  getAppSettings,
  getQuotaSnapshot,
  isTauriRuntime,
  onOpenSettingsUpdates,
  onQuotaUpdated,
  onUpdateAvailable,
  onWindowModeChanged,
  openAuthWebview,
  refreshAllQuotas,
  refreshSingleAccount,
  removeAccount,
  restoreDemoAccounts,
  saveAppSettings,
  scanLocalAccounts,
  setThemeVibrancy,
  setWindowMode,
} from './api/tauriClient';
import { AddAccountModal, AuthModeId } from './components/AddAccountModal';
import { EdgeDockView } from './components/EdgeDockView';
import { OrbView } from './components/OrbView';
import { PanelView } from './components/PanelView';
import { SettingsModal } from './components/SettingsModal';
import {
  AccountQuota,
  PlatformId,
  SubscriptionTier,
  UpdateCheckResult,
  WindowMode,
} from './types';

export const App: React.FC = () => {
  const [mode, setMode] = useState<WindowMode>('panel');
  const [theme, setTheme] = useState<'dark' | 'light'>('dark');
  const [accounts, setAccounts] = useState<AccountQuota[]>([]);
  const [lastUpdatedAt, setLastUpdatedAt] = useState('刚刚');
  const [filterPlatform, setFilterPlatform] = useState<'all' | PlatformId>('all');
  const [filterTier, setFilterTier] = useState<'all' | SubscriptionTier>('all');
  const [privacy, setPrivacy] = useState(false);
  const [expandedId, setExpandedId] = useState<string | null>('demo-agy-1');
  const [refreshingAll, setRefreshingAll] = useState(false);

  const [addOpen, setAddOpen] = useState(false);
  const [addDefaultPlatform, setAddDefaultPlatform] = useState<PlatformId>('antigravity');
  const [addInitialMode, setAddInitialMode] = useState<AuthModeId | undefined>(undefined);
  const [rebindOAuthAccount, setRebindOAuthAccount] = useState<AccountQuota | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [pendingUpdate, setPendingUpdate] = useState<UpdateCheckResult | null>(null);
  const [rebindTarget, setRebindTarget] = useState<{
    account: AccountQuota;
    sessionId: string;
  } | null>(null);

  const [toastMsg, setToastMsg] = useState<string | null>(null);
  const toastTimerRef = useRef<number | undefined>(undefined);

  const showToast = useCallback((msg: string) => {
    setToastMsg(msg);
    if (toastTimerRef.current !== undefined) {
      window.clearTimeout(toastTimerRef.current);
    }
    toastTimerRef.current = window.setTimeout(() => {
      setToastMsg(null);
    }, 2400);
  }, []);

  useEffect(() => {
    if (!isTauriRuntime()) {
      document.body.classList.add('browser-preview');
    }
    return () => {
      document.body.classList.remove('browser-preview');
    };
  }, []);

  useEffect(() => {
    document.documentElement.classList.toggle('theme-light', theme === 'light');
    void setThemeVibrancy(theme);
  }, [theme]);

  useEffect(() => {
    void getAppSettings().then((s) => {
      if (s.theme === 'light' || s.theme === 'dark') {
        setTheme(s.theme);
      }
    });

    void getQuotaSnapshot().then((snap) => {
      setAccounts(snap.accounts);
      setLastUpdatedAt(snap.lastUpdatedAt);
      if (snap.accounts.length > 0) {
        setExpandedId(snap.accounts[0].id);
      }
    });

    let unlistenQuota: (() => void) | undefined;
    let unlistenMode: (() => void) | undefined;
    let unlistenOpenSettings: (() => void) | undefined;
    let unlistenUpdateAvail: (() => void) | undefined;

    void onQuotaUpdated((snap) => {
      setAccounts(snap.accounts);
      setLastUpdatedAt(snap.lastUpdatedAt);
    }).then((fn) => {
      unlistenQuota = fn;
    });

    void onWindowModeChanged((nextMode) => {
      setMode(nextMode);
    }).then((fn) => {
      unlistenMode = fn;
    });

    void onOpenSettingsUpdates(() => {
      setSettingsOpen(true);
    }).then((fn) => {
      unlistenOpenSettings = fn;
    });

    void onUpdateAvailable((info) => {
      setPendingUpdate(info);
      showToast(`发现新版本 v${info.latestVersion}，可在设置中一键更新`);
    }).then((fn) => {
      unlistenUpdateAvail = fn;
    });

    return () => {
      unlistenQuota?.();
      unlistenMode?.();
      unlistenOpenSettings?.();
      unlistenUpdateAvail?.();
    };
  }, [showToast]);

  const { avgPct, lowestLabel, hasLowAlert } = useMemo(() => {
    if (accounts.length === 0) {
      return { avgPct: 0, lowestLabel: '暂无账号', hasLowAlert: false };
    }
    const avg = Math.round(accounts.reduce((sum, a) => sum + a.primaryPct, 0) / accounts.length);
    const lowest = accounts.reduce(
      (min, a) => (a.primaryPct < min.primaryPct ? a : min),
      accounts[0]
    );
    const low = accounts.some((a) => a.primaryPct <= 25);
    return {
      avgPct: avg,
      lowestLabel: `最低项：${lowest.alias} ${lowest.primaryPct}%`,
      hasLowAlert: low,
    };
  }, [accounts]);

  const handleSwitchMode = async (next: WindowMode) => {
    setMode(next);
    await setWindowMode(next);
  };

  const handleToggleTheme = async () => {
    const nextTheme = theme === 'dark' ? 'light' : 'dark';
    setTheme(nextTheme);
    try {
      const current = await getAppSettings();
      await saveAppSettings({ ...current, theme: nextTheme });
    } catch {
      // ignore non-critical preference save errors
    }
  };

  const handleRefreshAll = async () => {
    setRefreshingAll(true);
    try {
      const snap = await refreshAllQuotas();
      setAccounts(snap.accounts);
      setLastUpdatedAt(snap.lastUpdatedAt);
      showToast('已刷新全部账号额度');
    } catch (e) {
      showToast(`刷新失败：${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setRefreshingAll(false);
    }
  };

  const handleRefreshSingle = async (id: string) => {
    try {
      const updated = await refreshSingleAccount(id);
      setAccounts((prev) => prev.map((a) => (a.id === id ? updated : a)));
      showToast(`已同步 ${updated.alias}`);
    } catch (e) {
      showToast(`同步失败：${e instanceof Error ? e.message : String(e)}`);
    }
  };

  const handleRemove = async (id: string) => {
    const ok = await removeAccount(id);
    if (ok) {
      setAccounts((prev) => prev.filter((a) => a.id !== id));
      showToast('已移除该监控账号');
    }
  };

  const formatScanToast = (foundAccounts: AccountQuota[]) => {
    const okCount = foundAccounts.filter((a) => a.status === 'ok').length;
    const authCount = foundAccounts.length - okCount;
    if (authCount > 0) {
      return `本机扫描完成：发现 ${foundAccounts.length} 个账号（${okCount} 个已同步，${authCount} 个需重登）`;
    }
    return `本机扫描完成：已同步 ${foundAccounts.length} 个本机账号`;
  };

  const handleScanLocalAll = async () => {
    try {
      const summary = await scanLocalAccounts();
      const foundAccounts = summary.items
        .filter((item) => item.found && item.account)
        .map((item) => item.account!);
      if (foundAccounts.length > 0) {
        const snap = await getQuotaSnapshot();
        setAccounts(snap.accounts);
        setLastUpdatedAt(snap.lastUpdatedAt);
        setFilterPlatform('all');
        setFilterTier('all');
        setExpandedId(foundAccounts[0].id);
        showToast(formatScanToast(foundAccounts));
      } else {
        setSettingsOpen(true);
        showToast('未检测到本地客户端凭据，可在设置查看各平台详情');
      }
    } catch (e) {
      showToast(e instanceof Error ? e.message : String(e));
    }
  };

  const handleClearDemo = async () => {
    const snap = await clearDemoAccounts();
    setAccounts(snap.accounts);
    showToast('已清空示例账号');
  };

  const handleRestoreDemo = async () => {
    const snap = await restoreDemoAccounts();
    setAccounts(snap.accounts);
    if (snap.accounts[0]) setExpandedId(snap.accounts[0].id);
    showToast('已恢复示例预览数据');
  };

  const handleReauth = async (acc: AccountQuota) => {
    if (acc.authMethod === 'webview_session' || acc.authMethod === 'cookie') {
      try {
        const sid = await openAuthWebview(acc.platform, acc.id);
        setRebindTarget({ account: acc, sessionId: sid });
      } catch (e) {
        showToast(e instanceof Error ? e.message : String(e));
      }
      return;
    }

    if (
      acc.platform === 'antigravity' &&
      (acc.id.startsWith('agy-local') || acc.authMethod === 'local_server')
    ) {
      showToast('正在重新读取本机反重力登录态...');
      try {
        const res = await detectAntigravityLocal(acc.alias);
        const snap = await getQuotaSnapshot();
        setAccounts(snap.accounts);
        setLastUpdatedAt(snap.lastUpdatedAt);
        showToast(res.message);
      } catch (e) {
        showToast(e instanceof Error ? e.message : String(e));
      }
      return;
    }

    setAddDefaultPlatform(acc.platform);
    setAddInitialMode(
      acc.platform === 'codex' || acc.platform === 'claude' ? 'default_browser' : 'local_auto'
    );
    setRebindOAuthAccount(acc);
    setAddOpen(true);
  };

  const handleCompleteRebind = async () => {
    if (!rebindTarget) return;
    try {
      await captureAuthWebview(
        rebindTarget.account.platform,
        rebindTarget.sessionId,
        rebindTarget.account.id
      );
      const snap = await refreshAllQuotas();
      setAccounts(snap.accounts);
      setRebindTarget(null);
      showToast(`已更新 ${rebindTarget.account.alias} 会话`);
    } catch (e) {
      showToast(e instanceof Error ? e.message : String(e));
    }
  };

  const content = (() => {
    if (mode === 'orb') {
      return (
        <OrbView
          avgPct={avgPct}
          lowestLabel={lowestLabel}
          onExpand={() => void handleSwitchMode('panel')}
        />
      );
    }
    if (mode === 'edge') {
      return (
        <EdgeDockView
          avgPct={avgPct}
          hasLowAlert={hasLowAlert}
          onExpand={() => void handleSwitchMode('panel')}
        />
      );
    }
    return (
      <div
        className={
          isTauriRuntime() ? 'w-[396px] h-[576px]' : 'w-[396px] h-[576px] mx-auto my-6'
        }
      >
        <PanelView
          accounts={accounts}
          lastUpdatedAt={lastUpdatedAt}
          filterPlatform={filterPlatform}
          filterTier={filterTier}
          privacy={privacy}
          theme={theme}
          expandedId={expandedId}
          refreshingAll={refreshingAll}
          hasUpdateBadge={Boolean(pendingUpdate?.hasUpdate)}
          onFilterPlatform={setFilterPlatform}
          onFilterTier={setFilterTier}
          onTogglePrivacy={() => setPrivacy((p) => !p)}
          onToggleTheme={() => void handleToggleTheme()}
          onToggleExpand={(id) => setExpandedId((cur) => (cur === id ? null : id))}
          onRefreshAll={handleRefreshAll}
          onRefreshSingle={handleRefreshSingle}
          onReauth={handleReauth}
          onRemoveAccount={handleRemove}
          onOpenAddModal={() => {
            setAddDefaultPlatform(
              filterPlatform === 'all' ? 'antigravity' : filterPlatform
            );
            setAddInitialMode(undefined);
            setRebindOAuthAccount(null);
            setAddOpen(true);
          }}
          onOpenSettingsModal={() => setSettingsOpen(true)}
          onSwitchMode={(m) => void handleSwitchMode(m)}
          onScanLocalAll={handleScanLocalAll}
          onClearDemo={handleClearDemo}
          onRestoreDemo={handleRestoreDemo}
        />
      </div>
    );
  })();

  return (
    <div className="w-full h-full relative flex items-center justify-center">
      {content}

      <AddAccountModal
        open={addOpen}
        initialPlatform={addDefaultPlatform}
        initialMode={addInitialMode}
        rebindAccount={rebindOAuthAccount}
        onClose={() => {
          setAddOpen(false);
          setRebindOAuthAccount(null);
          setAddInitialMode(undefined);
        }}
        onAdded={(acc, msg) => {
          void getQuotaSnapshot().then((snap) => {
            setAccounts(snap.accounts);
            setLastUpdatedAt(snap.lastUpdatedAt);
          });
          setAccounts((prev) => [acc, ...prev.filter((a) => a.id !== acc.id)]);
          if (filterPlatform !== 'all' && filterPlatform !== acc.platform) {
            setFilterPlatform('all');
          }
          if (filterTier !== 'all' && filterTier !== acc.tier) {
            setFilterTier('all');
          }
          setExpandedId(acc.id);
          setAddOpen(false);
          setRebindOAuthAccount(null);
          setAddInitialMode(undefined);
          showToast(msg);
        }}
      />

      <SettingsModal
        open={settingsOpen}
        theme={theme}
        initialUpdateResult={pendingUpdate}
        onThemeChange={setTheme}
        onScanImported={(foundAccounts, count) => {
          if (foundAccounts.length > 0) {
            void getQuotaSnapshot().then((snap) => {
              setAccounts(snap.accounts);
              setLastUpdatedAt(snap.lastUpdatedAt);
            });
            setAccounts((prev) => {
              const ids = new Set(foundAccounts.map((a) => a.id));
              return [...foundAccounts, ...prev.filter((a) => !ids.has(a.id))];
            });
            setFilterPlatform('all');
            setFilterTier('all');
            setExpandedId(foundAccounts[0].id);
            showToast(formatScanToast(foundAccounts));
          } else {
            showToast(`本机扫描完成：发现 ${count} 个客户端`);
          }
        }}
        onClose={() => setSettingsOpen(false)}
      />

      {rebindTarget && mode === 'panel' && (
        <div className="fixed bottom-12 left-4 right-4 z-40 glass-surface-elevated rounded-2xl p-3 flex items-center justify-between gap-2 text-xs">
          <span>已在弹窗登录 {rebindTarget.account.alias}？</span>
          <div className="flex gap-1.5">
            <button
              type="button"
              onClick={() => setRebindTarget(null)}
              className="px-2.5 py-1 rounded-lg glass-icon-btn"
            >
              取消
            </button>
            <button
              type="button"
              onClick={() => void handleCompleteRebind()}
              className="px-2.5 py-1 rounded-lg font-medium"
              style={{
                background: 'var(--pill-active-bg)',
                border: '1px solid var(--pill-active-border)',
                color: 'var(--pct-high)',
              }}
            >
              提取新会话
            </button>
          </div>
        </div>
      )}

      {/* Centralized Polite Live Region Toast */}
      <div
        role="status"
        aria-live="polite"
        className={`fixed bottom-4 left-1/2 -translate-x-1/2 z-50 pointer-events-none glass-surface-elevated px-3.5 py-1.5 rounded-full text-[11px] font-medium transition-all duration-200 ${
          toastMsg ? 'opacity-100 translate-y-0' : 'opacity-0 translate-y-2'
        }`}
      >
        {toastMsg ?? ''}
      </div>
    </div>
  );
};
