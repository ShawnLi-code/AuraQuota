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
  togglePinAccount,
} from './api/tauriClient';
import { AddAccountModal, AuthModeId } from './components/AddAccountModal';
import { EdgeDockView } from './components/EdgeDockView';
import { OrbView } from './components/OrbView';
import { PanelView } from './components/PanelView';
import { SettingsModal } from './components/SettingsModal';
import {
  AccountQuota,
  PlatformId,
  QuotaSnapshot,
  UpdateCheckResult,
  WindowMode,
} from './types';

export const App: React.FC = () => {
  const [mode, setMode] = useState<WindowMode>('panel');
  const [theme, setTheme] = useState<'dark' | 'light'>('dark');
  const [accounts, setAccounts] = useState<AccountQuota[]>([]);
  const [activeAccountId, setActiveAccountId] = useState<string | null>(null);
  const [pinnedAccountId, setPinnedAccountId] = useState<string | null>(null);
  const [activeReason, setActiveReason] = useState<string | null>(null);
  const [lastActiveMap, setLastActiveMap] = useState<Record<string, number>>({});
  const [lastUpdatedAt, setLastUpdatedAt] = useState('刚刚');
  const [pollingIntervalSecs, setPollingIntervalSecs] = useState(15);
  const [privacy, setPrivacy] = useState(false);
  const [expandedId, setExpandedId] = useState<string | null>(null);
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

  const applySnapshot = useCallback((snap: QuotaSnapshot) => {
    setAccounts(snap.accounts);
    setLastUpdatedAt(snap.lastUpdatedAt);
    setActiveAccountId(snap.activeAccountId ?? null);
    setPinnedAccountId(snap.pinnedAccountId ?? null);
    setActiveReason(snap.activeReason ?? null);
    if (snap.lastActiveMap) {
      setLastActiveMap(snap.lastActiveMap);
    }
    if (typeof snap.pollingIntervalSecs === 'number' && snap.pollingIntervalSecs > 0) {
      setPollingIntervalSecs(snap.pollingIntervalSecs);
    }
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
      if (typeof s.pollingIntervalSecs === 'number' && s.pollingIntervalSecs > 0) {
        setPollingIntervalSecs(s.pollingIntervalSecs);
      }
    });

    void getQuotaSnapshot().then((snap) => {
      applySnapshot(snap);
    });

    let unlistenQuota: (() => void) | undefined;
    let unlistenMode: (() => void) | undefined;
    let unlistenOpenSettings: (() => void) | undefined;
    let unlistenUpdateAvail: (() => void) | undefined;

    void onQuotaUpdated((snap) => {
      applySnapshot(snap);
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
  }, [applySnapshot, showToast]);

  const { spotlightAccount, hasLowAlert } = useMemo(() => {
    if (accounts.length === 0) {
      return { spotlightAccount: null as AccountQuota | null, hasLowAlert: false };
    }
    const spot =
      accounts.find((a) => a.id === pinnedAccountId) ??
      accounts.find((a) => a.id === activeAccountId) ??
      accounts[0];
    const low = accounts.some((a) => a.primaryPct <= 25);
    return {
      spotlightAccount: spot,
      hasLowAlert: low,
    };
  }, [accounts, pinnedAccountId, activeAccountId]);

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
      applySnapshot(snap);
      showToast('已刷新全部已有账号额度');
    } catch (e) {
      showToast(`刷新失败：${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setRefreshingAll(false);
    }
  };

  const handleRefreshSingle = async (id: string) => {
    try {
      const updated = await refreshSingleAccount(id);
      const snap = await getQuotaSnapshot();
      applySnapshot(snap);
      showToast(`已同步 ${updated.alias}`);
    } catch (e) {
      showToast(`同步失败：${e instanceof Error ? e.message : String(e)}`);
    }
  };

  const handleTogglePin = async (id: string) => {
    try {
      const wasPinned = pinnedAccountId === id;
      const snap = await togglePinAccount(id);
      applySnapshot(snap);
      showToast(wasPinned ? '已取消固定，恢复自动跟随当前使用应用' : '已置顶为主焦点');
    } catch (e) {
      showToast(`置顶切换失败：${e instanceof Error ? e.message : String(e)}`);
    }
  };

  const handleRemove = async (id: string) => {
    const ok = await removeAccount(id);
    if (ok) {
      const snap = await getQuotaSnapshot();
      applySnapshot(snap);
      showToast('已移除该监控账号');
    }
  };

  const formatScanToast = (foundAccounts: AccountQuota[]) => {
    const okCount = foundAccounts.filter((a) => a.status === 'ok').length;
    const authCount = foundAccounts.length - okCount;
    if (authCount > 0) {
      return `本机扫描完成：发现 ${foundAccounts.length} 个账号（${okCount} 个已同步，${authCount} 个需重登）`;
    }
    return `本机扫描完成：已同步 ${foundAccounts.length} 个有效账号`;
  };

  const handleScanLocalAll = async () => {
    try {
      const summary = await scanLocalAccounts();
      const foundAccounts = summary.items
        .filter((item) => item.found && item.account)
        .map((item) => item.account!);
      if (foundAccounts.length > 0) {
        const snap = await getQuotaSnapshot();
        applySnapshot(snap);
        showToast(formatScanToast(foundAccounts));
      } else {
        setSettingsOpen(true);
        showToast('未检测到可用的本地客户端凭据，已忽略无效残留');
      }
    } catch (e) {
      showToast(e instanceof Error ? e.message : String(e));
    }
  };

  const handleClearDemo = async () => {
    const snap = await clearDemoAccounts();
    applySnapshot(snap);
    showToast('已清空示例账号');
  };

  const handleRestoreDemo = async () => {
    const snap = await restoreDemoAccounts();
    applySnapshot(snap);
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
        applySnapshot(snap);
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
      applySnapshot(snap);
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
          activeAccount={spotlightAccount}
          onExpand={() => void handleSwitchMode('panel')}
        />
      );
    }
    if (mode === 'edge') {
      return (
        <EdgeDockView
          activeAccount={spotlightAccount}
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
          activeAccountId={activeAccountId}
          pinnedAccountId={pinnedAccountId}
          activeReason={activeReason}
          lastActiveMap={lastActiveMap}
          lastUpdatedAt={lastUpdatedAt}
          pollingIntervalSecs={pollingIntervalSecs}
          privacy={privacy}
          theme={theme}
          expandedId={expandedId}
          refreshingAll={refreshingAll}
          hasUpdateBadge={Boolean(pendingUpdate?.hasUpdate)}
          onTogglePrivacy={() => setPrivacy((p) => !p)}
          onToggleTheme={() => void handleToggleTheme()}
          onToggleExpand={(id) => setExpandedId((cur) => (cur === id ? null : id))}
          onTogglePin={handleTogglePin}
          onRefreshAll={handleRefreshAll}
          onRefreshSingle={handleRefreshSingle}
          onReauth={handleReauth}
          onRemoveAccount={handleRemove}
          onOpenAddModal={() => {
            setAddDefaultPlatform(spotlightAccount?.platform ?? 'antigravity');
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
        onAdded={(_acc, msg) => {
          void getQuotaSnapshot().then((snap) => {
            applySnapshot(snap);
          });
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
              applySnapshot(snap);
            });
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
