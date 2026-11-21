import React, { useEffect, useId, useRef, useState } from 'react';
import {
  CheckCircle2,
  Download,
  Laptop,
  Moon,
  RefreshCw,
  Sun,
  X,
  XCircle,
} from 'lucide-react';
import {
  checkForUpdates,
  getAppSettings,
  getAutostartEnabled,
  installUpdate,
  onUpdateProgress,
  saveAppSettings,
  scanLocalAccounts,
  setAutostartEnabled,
} from '../api/tauriClient';
import {
  AccountQuota,
  AppSettings,
  LocalScanItem,
  PLATFORMS,
  UpdateCheckResult,
  UpdateProgressEvent,
} from '../types';

interface SettingsModalProps {
  open: boolean;
  theme: 'dark' | 'light';
  initialUpdateResult?: UpdateCheckResult | null;
  onThemeChange: (theme: 'dark' | 'light') => void;
  onScanImported: (accounts: AccountQuota[], count: number) => void;
  onClose: () => void;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({
  open,
  theme,
  initialUpdateResult,
  onThemeChange,
  onScanImported,
  onClose,
}) => {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const feedId = useId();
  const pollId = useId();

  const [settings, setSettings] = useState<AppSettings>({
    theme,
    autoCheckUpdates: true,
    updateFeedUrl: '',
    pollingIntervalSecs: 300,
  });
  const [autostart, setAutostart] = useState(false);

  const [scanning, setScanning] = useState(false);
  const [scanItems, setScanItems] = useState<LocalScanItem[] | null>(null);

  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [showCustomFeed, setShowCustomFeed] = useState(false);
  const [updateResult, setUpdateResult] = useState<UpdateCheckResult | null>(
    initialUpdateResult ?? null
  );
  const [installing, setInstalling] = useState(false);
  const [progress, setProgress] = useState<UpdateProgressEvent | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  useEffect(() => {
    if (initialUpdateResult) {
      setUpdateResult(initialUpdateResult);
    }
  }, [initialUpdateResult]);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;

    if (open && !dialog.open) {
      setErrorMsg(null);
      void getAppSettings().then((s) => setSettings({ ...s, theme }));
      void getAutostartEnabled().then(setAutostart);
      dialog.showModal();
    } else if (!open && dialog.open) {
      dialog.close();
    }
  }, [open, theme]);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;

    const handleClose = () => {
      dialog.returnValue = '';
      if (open) onClose();
    };
    dialog.addEventListener('close', handleClose);

    if (!('closedBy' in HTMLDialogElement.prototype)) {
      let pressedOutside = false;
      const isOutside = (e: PointerEvent) => {
        const r = dialog.getBoundingClientRect();
        return (
          e.clientX < r.left ||
          e.clientX > r.right ||
          e.clientY < r.top ||
          e.clientY > r.bottom
        );
      };
      const onDown = (e: PointerEvent) => {
        pressedOutside = isOutside(e);
      };
      const onUp = (e: PointerEvent) => {
        if (pressedOutside && isOutside(e)) dialog.close();
      };
      dialog.addEventListener('pointerdown', onDown);
      dialog.addEventListener('pointerup', onUp);
      return () => {
        dialog.removeEventListener('close', handleClose);
        dialog.removeEventListener('pointerdown', onDown);
        dialog.removeEventListener('pointerup', onUp);
      };
    }

    return () => {
      dialog.removeEventListener('close', handleClose);
    };
  }, [open, onClose]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void onUpdateProgress((ev) => setProgress(ev)).then((fn) => {
      unlisten = fn;
    });
    return () => {
      unlisten?.();
    };
  }, []);

  const handleSaveSettings = async (patch: Partial<AppSettings>) => {
    const next = { ...settings, ...patch };
    setSettings(next);
    if (patch.theme) onThemeChange(patch.theme);
    try {
      const saved = await saveAppSettings(next);
      setSettings(saved);
    } catch (e) {
      setErrorMsg(e instanceof Error ? e.message : String(e));
    }
  };

  const handleToggleAutostart = async () => {
    const target = !autostart;
    try {
      const res = await setAutostartEnabled(target);
      setAutostart(res);
    } catch (e) {
      setErrorMsg(e instanceof Error ? e.message : String(e));
    }
  };

  const handleScanAll = async () => {
    setScanning(true);
    setErrorMsg(null);
    try {
      const summary = await scanLocalAccounts();
      setScanItems(summary.items);
      const imported = summary.items
        .filter((i) => i.found && i.account)
        .map((i) => i.account!);
      onScanImported(imported, summary.foundCount);
    } catch (e) {
      setErrorMsg(e instanceof Error ? e.message : String(e));
    } finally {
      setScanning(false);
    }
  };

  const handleCheckUpdate = async () => {
    setCheckingUpdate(true);
    setErrorMsg(null);
    try {
      await saveAppSettings(settings);
      const res = await checkForUpdates(settings.updateFeedUrl);
      setUpdateResult(res);
    } catch (e) {
      setErrorMsg(e instanceof Error ? e.message : String(e));
    } finally {
      setCheckingUpdate(false);
    }
  };

  const handleInstallUpdate = async () => {
    if (!updateResult?.downloadUrl) return;
    setInstalling(true);
    setErrorMsg(null);
    try {
      await installUpdate(updateResult.downloadUrl, updateResult.sha256);
    } catch (e) {
      setErrorMsg(e instanceof Error ? e.message : String(e));
      setInstalling(false);
    }
  };

  return (
    <dialog
      ref={dialogRef}
      closedby="any"
      aria-labelledby={titleId}
      className="aura-dialog w-[364px]"
    >
      <div className="glass-surface-elevated w-full rounded-2xl flex flex-col max-h-[calc(100vh-2rem)] overflow-hidden text-xs">
        <div
          className="px-4 py-3 flex items-center justify-between shrink-0"
          style={{ borderBottom: '1px solid var(--divider)' }}
        >
          <h3 id={titleId} className="text-sm font-semibold">
            设置 · 本机扫描与更新
          </h3>
          <button
            type="button"
            onClick={() => dialogRef.current?.close()}
            aria-label="关闭设置对话框"
            className="w-7 h-7 rounded-lg glass-icon-btn flex items-center justify-center"
          >
            <X className="w-4 h-4" aria-hidden="true" />
          </button>
        </div>

        <div className="flex-1 min-h-0 overflow-y-auto custom-scroll px-4 py-3 space-y-3.5">
        {/* 1. Offline Local Discovery */}
        <section
          className="rounded-xl p-3 space-y-2.5"
          style={{
            background: 'var(--surface-card)',
            border: '1px solid var(--border-subtle)',
          }}
        >
          <div className="flex items-center justify-between gap-2">
            <div>
              <div className="font-semibold flex items-center gap-1.5">
                <Laptop className="w-3.5 h-3.5 text-sky-400" aria-hidden="true" />
                <span>本机账号自动检索</span>
              </div>
              <p className="text-[10.5px] mt-0.5" style={{ color: 'var(--text-sub)' }}>
                一键扫描本机已登录的 5 个客户端
              </p>
            </div>
            <button
              type="button"
              disabled={scanning}
              onClick={() => void handleScanAll()}
              className="px-2.5 py-1.5 rounded-xl font-medium shrink-0 inline-flex items-center gap-1"
              style={{
                background: 'var(--pill-active-bg)',
                border: '1px solid var(--pill-active-border)',
                color: 'var(--pct-high)',
              }}
            >
              <RefreshCw
                className={`w-3 h-3 ${scanning ? 'spinning' : ''}`}
                aria-hidden="true"
              />
              <span>{scanning ? '扫描中...' : '一键扫描'}</span>
            </button>
          </div>

          {scanItems && (
            <ul className="space-y-1.5 m-0 p-0">
              {scanItems.map((item) => {
                const isSynced = item.found && item.account?.status === 'ok';
                return (
                  <li
                    key={item.platform}
                    className="list-none flex items-start gap-2 text-[11px] py-1"
                    style={{ borderTop: '1px solid var(--divider)' }}
                  >
                    {isSynced ? (
                      <CheckCircle2 className="w-3.5 h-3.5 text-emerald-500 shrink-0 mt-0.5" aria-hidden="true" />
                    ) : item.found ? (
                      <CheckCircle2 className="w-3.5 h-3.5 text-amber-500 shrink-0 mt-0.5" aria-hidden="true" />
                    ) : (
                      <XCircle className="w-3.5 h-3.5 opacity-45 shrink-0 mt-0.5" aria-hidden="true" />
                    )}
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center justify-between gap-1">
                        <span className="font-medium">{PLATFORMS[item.platform].name}</span>
                        <span
                          className={`text-[10px] font-medium shrink-0 ${
                            isSynced
                              ? 'text-emerald-500'
                              : item.found
                                ? 'text-amber-500'
                                : 'opacity-60'
                          }`}
                        >
                          {isSynced ? '已同步' : item.found ? '需重登' : '未检测到'}
                        </span>
                      </div>
                      <p className="text-[10.5px] leading-snug" style={{ color: 'var(--text-sub)' }}>
                        {item.message}
                      </p>
                    </div>
                  </li>
                );
              })}
            </ul>
          )}
        </section>

        {/* 2. Appearance & System */}
        <section
          className="rounded-xl p-3 space-y-2.5"
          style={{
            background: 'var(--surface-card)',
            border: '1px solid var(--border-subtle)',
          }}
        >
          <div className="font-semibold">外观与常规</div>

          <div className="flex items-center justify-between">
            <span style={{ color: 'var(--text-sub)' }}>界面主题</span>
            <div className="flex gap-1.5">
              <button
                type="button"
                aria-pressed={theme === 'dark'}
                onClick={() => void handleSaveSettings({ theme: 'dark' })}
                className="px-2.5 py-1 rounded-lg inline-flex items-center gap-1 text-[11px] font-medium"
                style={
                  theme === 'dark'
                    ? {
                        background: 'var(--pill-active-bg)',
                        border: '1px solid var(--pill-active-border)',
                        color: 'var(--pct-high)',
                      }
                    : {
                        background: 'var(--pill-bg)',
                        border: '1px solid var(--border-subtle)',
                        color: 'var(--text-sub)',
                      }
                }
              >
                <Moon className="w-3 h-3" aria-hidden="true" />
                <span>深空</span>
              </button>
              <button
                type="button"
                aria-pressed={theme === 'light'}
                onClick={() => void handleSaveSettings({ theme: 'light' })}
                className="px-2.5 py-1 rounded-lg inline-flex items-center gap-1 text-[11px] font-medium"
                style={
                  theme === 'light'
                    ? {
                        background: 'var(--pill-active-bg)',
                        border: '1px solid var(--pill-active-border)',
                        color: 'var(--pct-high)',
                      }
                    : {
                        background: 'var(--pill-bg)',
                        border: '1px solid var(--border-subtle)',
                        color: 'var(--text-sub)',
                      }
                }
              >
                <Sun className="w-3 h-3" aria-hidden="true" />
                <span>浅瓷</span>
              </button>
            </div>
          </div>

          <div className="flex items-center justify-between">
            <label htmlFor={pollId} style={{ color: 'var(--text-sub)' }}>
              自动轮询频率
            </label>
            <select
              id={pollId}
              value={settings.pollingIntervalSecs}
              onChange={(e) =>
                void handleSaveSettings({ pollingIntervalSecs: Number(e.target.value) })
              }
              className="aura-select px-2 py-1 rounded-lg text-[11px]"
            >
              <option value={180}>每 3 分钟</option>
              <option value={300}>每 5 分钟</option>
              <option value={600}>每 10 分钟</option>
            </select>
          </div>

          <div className="flex items-center justify-between">
            <span style={{ color: 'var(--text-sub)' }}>开机自动启动</span>
            <button
              type="button"
              role="switch"
              aria-checked={autostart}
              onClick={() => void handleToggleAutostart()}
              className="px-2.5 py-1 rounded-lg text-[11px] font-medium"
              style={
                autostart
                  ? {
                      background: 'var(--pill-active-bg)',
                      border: '1px solid var(--pill-active-border)',
                      color: 'var(--pct-high)',
                    }
                  : {
                      background: 'var(--pill-bg)',
                      border: '1px solid var(--border-subtle)',
                      color: 'var(--text-sub)',
                    }
              }
            >
              {autostart ? '已开启' : '未开启'}
            </button>
          </div>
        </section>

        {/* 3. Auto-Updater */}
        <section
          className="rounded-xl p-3 space-y-2.5"
          style={{
            background: 'var(--surface-card)',
            border: '1px solid var(--border-subtle)',
          }}
        >
          <div className="flex items-center justify-between">
            <span className="font-semibold">
              应用更新 <span className="font-normal opacity-75">v{updateResult?.currentVersion ?? '0.3.0'}</span>
            </span>
            <button
              type="button"
              disabled={checkingUpdate}
              onClick={() => void handleCheckUpdate()}
              className="px-2.5 py-1 rounded-lg glass-icon-btn text-[11px] font-medium inline-flex items-center gap-1"
            >
              <RefreshCw
                className={`w-3 h-3 ${checkingUpdate ? 'spinning' : ''}`}
                aria-hidden="true"
              />
              <span>{checkingUpdate ? '检查中...' : '检查更新'}</span>
            </button>
          </div>

          <div className="flex items-center justify-between text-[10.5px]">
            <span style={{ color: 'var(--text-sub)' }}>
              更新源：GitHub 官方发布 (AuraQuota)
            </span>
            <button
              type="button"
              onClick={() => setShowCustomFeed((v) => !v)}
              className="hover:underline shrink-0"
              style={{ color: 'var(--pct-high)' }}
            >
              {showCustomFeed ? '收起' : '自定义'}
            </button>
          </div>

          {showCustomFeed && (
            <div>
              <label
                htmlFor={feedId}
                className="block mb-1 text-[10.5px]"
                style={{ color: 'var(--text-sub)' }}
              >
                自定义更新源（留空则自动恢复 GitHub 官方源）
              </label>
              <input
                id={feedId}
                type="text"
                value={settings.updateFeedUrl}
                onChange={(e) => setSettings((s) => ({ ...s, updateFeedUrl: e.target.value }))}
                onBlur={() => void handleSaveSettings({ updateFeedUrl: settings.updateFeedUrl })}
                placeholder="https://api.github.com/repos/ShawnLi-code/AuraQuota/releases/latest"
                className="aura-input w-full px-2.5 py-1.5 rounded-xl text-[11px] focus:outline-none focus:border-sky-400"
              />
            </div>
          )}

          <div className="flex items-center justify-between">
            <span className="text-[11px]" style={{ color: 'var(--text-sub)' }}>
              启动时静默检查更新
            </span>
            <button
              type="button"
              role="switch"
              aria-checked={settings.autoCheckUpdates}
              onClick={() =>
                void handleSaveSettings({ autoCheckUpdates: !settings.autoCheckUpdates })
              }
              className="px-2.5 py-1 rounded-lg text-[11px] font-medium"
              style={
                settings.autoCheckUpdates
                  ? {
                      background: 'var(--pill-active-bg)',
                      border: '1px solid var(--pill-active-border)',
                      color: 'var(--pct-high)',
                    }
                  : {
                      background: 'var(--pill-bg)',
                      border: '1px solid var(--border-subtle)',
                      color: 'var(--text-sub)',
                    }
              }
            >
              {settings.autoCheckUpdates ? '已开启' : '已关闭'}
            </button>
          </div>

          {updateResult && (
            <div
              className="rounded-xl p-2.5 space-y-2 text-[11px]"
              style={{
                background: 'var(--pill-bg)',
                border: '1px solid var(--border-subtle)',
              }}
            >
              <div className="font-medium">{updateResult.message}</div>
              {updateResult.releaseNotes && (
                <p
                  className="text-[10.5px] whitespace-pre-wrap max-h-20 overflow-y-auto custom-scroll"
                  style={{ color: 'var(--text-sub)' }}
                >
                  {updateResult.releaseNotes}
                </p>
              )}
              {updateResult.hasUpdate && updateResult.downloadUrl && (
                <div className="space-y-1.5">
                  {progress && (
                    <div
                      className="h-1.5 rounded-full overflow-hidden"
                      style={{ background: 'var(--track-bg)' }}
                    >
                      <div
                        className="bar-fill h-full rounded-full bg-gradient-to-r from-sky-400 to-indigo-400"
                        style={{ width: `${progress.pct}%` }}
                      />
                    </div>
                  )}
                  <button
                    type="button"
                    disabled={installing}
                    onClick={() => void handleInstallUpdate()}
                    className="w-full py-1.5 rounded-xl bg-gradient-to-r from-sky-500 to-indigo-500 text-white font-medium inline-flex items-center justify-center gap-1.5 hover:brightness-110 transition disabled:opacity-60"
                  >
                    <Download className="w-3.5 h-3.5" aria-hidden="true" />
                    <span>
                      {installing
                        ? `正在更新 ${progress ? `${progress.pct}%` : '...'}`
                        : `立即下载并安装 v${updateResult.latestVersion}`}
                    </span>
                  </button>
                </div>
              )}
            </div>
          )}
        </section>

        {errorMsg && (
          <div
            role="alert"
            className="p-2 rounded-xl bg-rose-500/15 border border-rose-500/35 text-rose-500 text-[11px]"
          >
            {errorMsg}
          </div>
        )}
        </div>
      </div>
    </dialog>
  );
};
