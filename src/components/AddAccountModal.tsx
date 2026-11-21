import React, { useEffect, useId, useRef, useState } from 'react';
import { CheckCircle2, Globe, KeyRound, Laptop, Loader2, X } from 'lucide-react';
import {
  addAccount,
  cancelBrowserOauth,
  captureAuthWebview,
  completeClaudeBrowserOauth,
  detectAntigravityLocal,
  detectClaudeCliLocal,
  detectCodexCliLocal,
  detectCommandcodeCliLocal,
  detectOpencodeCliLocal,
  isTauriRuntime,
  openAuthWebview,
  startClaudeBrowserOauth,
  startCodexBrowserOauth,
} from '../api/tauriClient';
import {
  AccountQuota,
  AuthMethod,
  PLATFORM_DETAILS,
  PLATFORM_ORDER,
  PlatformId,
  SubscriptionTier,
} from '../types';

export type AuthModeId =
  | 'default_browser'
  | 'local_auto'
  | 'webview'
  | 'manual_secret';

interface AddAccountModalProps {
  open: boolean;
  initialPlatform?: PlatformId;
  initialMode?: AuthModeId;
  rebindAccount?: AccountQuota | null;
  onClose: () => void;
  onAdded: (account: AccountQuota, message: string) => void;
}

interface AuthModeOption {
  id: AuthModeId;
  label: string;
  wireMethod: AuthMethod;
  secretPlaceholder?: string;
  secretLabel?: string;
}

function getAuthModeOptions(platform: PlatformId): AuthModeOption[] {
  switch (platform) {
    case 'antigravity':
      return [
        { id: 'local_auto', label: '本机自动读取', wireMethod: 'local_server' },
        {
          id: 'manual_secret',
          label: '手动 Token',
          wireMethod: 'oauth_token',
          secretLabel: 'OAuth Access Token',
          secretPlaceholder: '粘贴 Antigravity OAuth Access Token (ya29...)',
        },
      ];
    case 'gemini':
      return [{ id: 'webview', label: '独立多账号窗口登录', wireMethod: 'webview_session' }];
    case 'codex':
      return [
        { id: 'default_browser', label: '默认浏览器登录', wireMethod: 'oauth_token' },
        { id: 'local_auto', label: '本机自动读取', wireMethod: 'local_cli' },
        { id: 'webview', label: '独立窗口登录', wireMethod: 'webview_session' },
      ];
    case 'claude':
      return [
        { id: 'default_browser', label: '默认浏览器登录', wireMethod: 'oauth_token' },
        { id: 'local_auto', label: '本机自动读取', wireMethod: 'local_cli' },
        { id: 'webview', label: '独立窗口登录', wireMethod: 'webview_session' },
        {
          id: 'manual_secret',
          label: '手动 Cookie',
          wireMethod: 'cookie',
          secretLabel: 'sessionKey / Cookie',
          secretPlaceholder: '粘贴 sk-ant-sid01-... 或完整 Cookie',
        },
      ];
    case 'commandcode':
      return [
        { id: 'local_auto', label: '本机自动读取', wireMethod: 'local_cli' },
        { id: 'webview', label: '独立窗口登录', wireMethod: 'webview_session' },
        {
          id: 'manual_secret',
          label: 'API Key',
          wireMethod: 'api_key',
          secretLabel: 'Command Code API Key',
          secretPlaceholder: '粘贴 Command Code API Key',
        },
      ];
    case 'opencode':
      return [
        { id: 'local_auto', label: '本机自动读取', wireMethod: 'local_cli' },
        { id: 'webview', label: '独立窗口登录', wireMethod: 'webview_session' },
        {
          id: 'manual_secret',
          label: 'API Key',
          wireMethod: 'api_key',
          secretLabel: 'OpenCode / Zen API Key',
          secretPlaceholder: '粘贴 OpenCode / Zen API Key',
        },
      ];
  }
}

function resolveModeForPlatform(
  platform: PlatformId,
  preferred?: AuthModeId
): AuthModeOption {
  const options = getAuthModeOptions(platform);
  if (preferred) {
    const matched = options.find((o) => o.id === preferred);
    if (matched) return matched;
  }
  return options[0];
}

export const AddAccountModal: React.FC<AddAccountModalProps> = ({
  open,
  initialPlatform = 'antigravity',
  initialMode,
  rebindAccount = null,
  onClose,
  onAdded,
}) => {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const aliasId = useId();
  const planId = useId();
  const emailId = useId();
  const secretId = useId();
  const claudeCodeId = useId();

  const [platform, setPlatform] = useState<PlatformId>(initialPlatform);
  const [authMode, setAuthMode] = useState<AuthModeId>('local_auto');
  const [alias, setAlias] = useState('');
  const [email, setEmail] = useState('');
  const [planValue, setPlanValue] = useState(PLATFORM_DETAILS.antigravity.plans[0].value);
  const [secretInput, setSecretInput] = useState('');

  // WebView2 multi-account isolation state
  const [loginSessionId, setLoginSessionId] = useState<string | null>(null);
  const [capturedInfo, setCapturedInfo] = useState<{
    sessionId: string;
    count: number;
    pct?: number | null;
    plan?: string | null;
  } | null>(null);

  // Default browser OAuth state (Codex & Claude - zero config)
  const [oauthWaiting, setOauthWaiting] = useState(false);
  const [claudeFlowId, setClaudeFlowId] = useState<string | null>(null);
  const [claudeAuthCode, setClaudeAuthCode] = useState('');

  const [busy, setBusy] = useState(false);
  const [statusError, setStatusError] = useState<string | null>(null);

  const modeOptions = getAuthModeOptions(platform);
  const activeMode = modeOptions.find((o) => o.id === authMode) ?? modeOptions[0];
  const meta = PLATFORM_DETAILS[platform];

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;

    if (open && !dialog.open) {
      const targetPlatform = rebindAccount?.platform ?? initialPlatform;
      const firstOpt = resolveModeForPlatform(targetPlatform, initialMode);
      const plans = PLATFORM_DETAILS[targetPlatform].plans;
      const matchedPlan = rebindAccount
        ? plans.find(
            (p) => p.planLabel.toLowerCase() === rebindAccount.planLabel.toLowerCase()
          ) ?? plans[0]
        : plans[0];

      setPlatform(targetPlatform);
      setAuthMode(firstOpt.id);
      setPlanValue(matchedPlan.value);
      setAlias(rebindAccount?.alias ?? '');
      setEmail(rebindAccount?.email ?? '');
      setSecretInput('');
      setLoginSessionId(null);
      setCapturedInfo(null);
      setClaudeFlowId(null);
      setClaudeAuthCode('');
      setOauthWaiting(false);
      setBusy(false);
      setStatusError(null);

      dialog.showModal();
    } else if (!open && dialog.open) {
      dialog.close();
    }
  }, [open, initialPlatform, initialMode, rebindAccount]);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;

    const handleClose = () => {
      dialog.returnValue = '';
      void cancelBrowserOauth();
      setOauthWaiting(false);
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

  const handleSelectPlatform = (next: PlatformId) => {
    if (rebindAccount) return;
    void cancelBrowserOauth();
    setOauthWaiting(false);
    setPlatform(next);
    const firstOpt = getAuthModeOptions(next)[0];
    setAuthMode(firstOpt.id);
    setPlanValue(PLATFORM_DETAILS[next].plans[0].value);
    setSecretInput('');
    setLoginSessionId(null);
    setCapturedInfo(null);
    setClaudeFlowId(null);
    setClaudeAuthCode('');
    setStatusError(null);
  };

  const handleSelectAuthMode = (nextMode: AuthModeId) => {
    void cancelBrowserOauth();
    setOauthWaiting(false);
    setAuthMode(nextMode);
    setStatusError(null);
  };

  const handleCancelOauthWait = async () => {
    await cancelBrowserOauth();
    setOauthWaiting(false);
    setBusy(false);
  };

  const handleStartClaudeOauth = async () => {
    setBusy(true);
    setStatusError(null);
    try {
      const started = await startClaudeBrowserOauth();
      setClaudeFlowId(started.flowId);
    } catch (e) {
      setStatusError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleOpenWebview = async () => {
    setBusy(true);
    setStatusError(null);
    try {
      const sid = await openAuthWebview(platform, rebindAccount?.id);
      setLoginSessionId(sid);
      setCapturedInfo(null);
    } catch (e) {
      setStatusError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleCaptureWebview = async () => {
    if (!loginSessionId) return;
    setBusy(true);
    setStatusError(null);
    try {
      const res = await captureAuthWebview(platform, loginSessionId);
      setCapturedInfo({
        sessionId: res.sessionId,
        count: res.cookieCount,
        pct: res.detectedPrimaryPct,
        plan: res.detectedPlanLabel,
      });
      if (res.detectedEmail && !email.trim()) {
        setEmail(res.detectedEmail);
      }
      if (res.detectedPlanLabel) {
        const matched = meta.plans.find(
          (p) => p.planLabel.toLowerCase() === res.detectedPlanLabel?.toLowerCase()
        );
        if (matched) setPlanValue(matched.value);
      }
    } catch (e) {
      setStatusError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleQuickLocalAction = async () => {
    setBusy(true);
    setStatusError(null);
    const customAlias = alias.trim() || undefined;
    try {
      const res = await (async () => {
        switch (platform) {
          case 'antigravity':
            return detectAntigravityLocal(customAlias);
          case 'codex':
            return detectCodexCliLocal(customAlias);
          case 'claude':
            return detectClaudeCliLocal(customAlias);
          case 'commandcode':
            return detectCommandcodeCliLocal(customAlias);
          case 'opencode':
            return detectOpencodeCliLocal(customAlias);
          default:
            return { found: false, message: '该平台不支持本机自动读取，请切换其他方式登录' };
        }
      })();

      if (res.found && res.account) {
        onAdded(res.account, res.message);
      } else {
        setStatusError(res.message);
      }
    } catch (e) {
      setStatusError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleDefaultBrowserOauth = async () => {
    setStatusError(null);
    const [tierRaw, planLabel] = planValue.split('|');
    const customAlias = alias.trim() || undefined;
    const targetTier = (tierRaw as SubscriptionTier) || 'paid';
    const rebindId = rebindAccount?.id;

    if (platform === 'codex') {
      setBusy(true);
      setOauthWaiting(true);
      try {
        const account = await startCodexBrowserOauth({
          alias: customAlias,
          planLabel,
          tier: targetTier,
          rebindAccountId: rebindId,
        });
        onAdded(
          account,
          rebindId ? `已通过默认浏览器更新 ${account.alias} 授权` : `已授权并导入 Codex · ${account.alias}`
        );
      } catch (err) {
        setStatusError(err instanceof Error ? err.message : String(err));
      } finally {
        setOauthWaiting(false);
        setBusy(false);
      }
      return;
    }

    if (platform === 'claude') {
      if (!claudeFlowId) {
        await handleStartClaudeOauth();
        return;
      }
      if (!claudeAuthCode.trim()) {
        setStatusError('请先粘贴浏览器页面返回的 Claude 授权码');
        return;
      }
      setBusy(true);
      try {
        const account = await completeClaudeBrowserOauth({
          flowId: claudeFlowId,
          authCode: claudeAuthCode.trim(),
          alias: customAlias,
          planLabel,
          tier: targetTier,
          rebindAccountId: rebindId,
        });
        onAdded(
          account,
          rebindId ? `已更新 ${account.alias} 授权` : `已授权并导入 Claude · ${account.alias}`
        );
      } catch (err) {
        setStatusError(err instanceof Error ? err.message : String(err));
      } finally {
        setBusy(false);
      }
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setStatusError(null);

    if (activeMode.id === 'local_auto') {
      await handleQuickLocalAction();
      return;
    }

    if (activeMode.id === 'default_browser') {
      await handleDefaultBrowserOauth();
      return;
    }

    if (activeMode.id === 'webview' && isTauriRuntime() && !capturedInfo) {
      setStatusError('请先点击「1. 打开独立登录窗」，登录并看到额度页后再点击「2. 提取会话」');
      return;
    }

    if (activeMode.id === 'manual_secret' && isTauriRuntime() && !secretInput.trim()) {
      setStatusError('请输入有效的凭据内容');
      return;
    }

    setBusy(true);
    try {
      const [tierRaw, planLabel] = planValue.split('|');
      const created = await addAccount({
        platform,
        alias: alias.trim() || `${meta.name} 账号`,
        email: email.trim() || 'account@local',
        tier: (tierRaw as SubscriptionTier) || 'paid',
        planLabel: planLabel || 'Pro',
        authMethod: activeMode.wireMethod,
        secretInput: secretInput.trim() || undefined,
        profileId: capturedInfo?.sessionId,
      });

      onAdded(created, `已添加 ${meta.name} · ${created.alias}`);
    } catch (err) {
      setStatusError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const submitLabel = (() => {
    if (oauthWaiting) return '等待浏览器授权...';
    if (busy) return '处理中...';
    if (activeMode.id === 'local_auto') return '立即读取并添加';
    if (activeMode.id === 'default_browser') {
      if (platform === 'claude') {
        return claudeFlowId ? '完成登录并同步' : '打开默认浏览器授权';
      }
      return '打开默认浏览器登录';
    }
    return '保存并同步额度';
  })();

  const showEmailInput = activeMode.id === 'webview' || activeMode.id === 'manual_secret';

  return (
    <dialog
      ref={dialogRef}
      closedby="any"
      aria-labelledby={titleId}
      className="aura-dialog w-[364px]"
    >
      <div className="glass-surface-elevated w-full rounded-2xl flex flex-col max-h-[calc(100vh-2rem)] overflow-hidden">
        {/* Pinned Header */}
        <div
          className="px-4 py-3 flex items-center justify-between shrink-0"
          style={{ borderBottom: '1px solid var(--divider)' }}
        >
          <div className="min-w-0">
            <h3 id={titleId} className="text-sm font-semibold truncate">
              {rebindAccount ? `更新授权 · ${rebindAccount.alias}` : '添加监控账号'}
            </h3>
          </div>
          <button
            type="button"
            onClick={() => dialogRef.current?.close()}
            aria-label="关闭添加账号对话框"
            className="w-7 h-7 rounded-lg glass-icon-btn flex items-center justify-center shrink-0"
          >
            <X className="w-4 h-4" aria-hidden="true" />
          </button>
        </div>

        {/* Scrollable Middle Form + Pinned Footer */}
        <form onSubmit={handleSubmit} className="flex flex-col flex-1 min-h-0 text-xs">
          <div className="flex-1 min-h-0 overflow-y-auto custom-scroll px-4 py-3 space-y-3">
            <div>
              <span className="block mb-1.5 font-medium" style={{ color: 'var(--text-sub)' }}>
                选择平台
              </span>
              <div className="grid grid-cols-2 gap-1.5">
                {PLATFORM_ORDER.map((pid) => {
                  const active = platform === pid;
                  const locked = Boolean(rebindAccount && rebindAccount.platform !== pid);
                  return (
                    <button
                      key={pid}
                      type="button"
                      disabled={locked}
                      onClick={() => handleSelectPlatform(pid)}
                      className="px-2.5 py-2 rounded-xl text-left font-medium transition flex items-center gap-2 disabled:opacity-40"
                      style={
                        active
                          ? {
                              background: 'var(--pill-active-bg)',
                              border: '1px solid var(--pill-active-border)',
                              color: 'var(--text-main)',
                            }
                          : {
                              background: 'var(--surface-card)',
                              border: '1px solid var(--border-subtle)',
                              color: 'var(--text-sub)',
                            }
                      }
                    >
                      <span
                        aria-hidden="true"
                        className="w-2 h-2 rounded-full shrink-0"
                        style={{ background: PLATFORM_DETAILS[pid].color }}
                      />
                      <span className="truncate">{PLATFORM_DETAILS[pid].shortLabel}</span>
                    </button>
                  );
                })}
              </div>
            </div>

            <div>
              <span className="block mb-1.5 font-medium" style={{ color: 'var(--text-sub)' }}>
                接入方式（DPAPI 加密保存）
              </span>
              <div className="flex flex-wrap gap-1.5">
                {modeOptions.map((opt) => {
                  const active = activeMode.id === opt.id;
                  return (
                    <button
                      key={opt.id}
                      type="button"
                      onClick={() => handleSelectAuthMode(opt.id)}
                      className="px-2.5 py-1.5 rounded-xl text-[11px] font-medium transition"
                      style={
                        active
                          ? {
                              background: 'var(--badge-paid-bg)',
                              border: '1px solid var(--badge-paid-border)',
                              color: 'var(--badge-paid-text)',
                            }
                          : {
                              background: 'var(--pill-bg)',
                              border: '1px solid var(--border-subtle)',
                              color: 'var(--text-sub)',
                            }
                      }
                    >
                      {opt.label}
                    </button>
                  );
                })}
              </div>
            </div>

            <div className="grid grid-cols-2 gap-2">
              <div>
                <label
                  htmlFor={aliasId}
                  className="block mb-1 font-medium"
                  style={{ color: 'var(--text-sub)' }}
                >
                  账号备注（可选）
                </label>
                <input
                  id={aliasId}
                  type="text"
                  value={alias}
                  onChange={(e) => setAlias(e.target.value)}
                  placeholder="如：主力号 / 备用号"
                  className="aura-input w-full px-2.5 py-2 rounded-xl focus:outline-none focus:border-sky-400 text-xs"
                />
              </div>
              <div>
                <label
                  htmlFor={planId}
                  className="block mb-1 font-medium"
                  style={{ color: 'var(--text-sub)' }}
                >
                  订阅类型
                </label>
                <select
                  id={planId}
                  value={planValue}
                  onChange={(e) => setPlanValue(e.target.value)}
                  className="aura-select w-full px-2.5 py-2 rounded-xl focus:outline-none focus:border-sky-400 text-xs"
                >
                  {meta.plans.map((p) => (
                    <option key={p.value} value={p.value}>
                      {p.label}
                    </option>
                  ))}
                </select>
              </div>
            </div>

            {showEmailInput && (
              <div>
                <label
                  htmlFor={emailId}
                  className="block mb-1 font-medium"
                  style={{ color: 'var(--text-sub)' }}
                >
                  账号邮箱（可选，登录后自动识别）
                </label>
                <input
                  id={emailId}
                  type="email"
                  autoComplete="email"
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  placeholder="name@example.com"
                  className="aura-input w-full px-2.5 py-2 rounded-xl focus:outline-none focus:border-sky-400 text-xs"
                />
              </div>
            )}

            {activeMode.id === 'local_auto' && (
              <div
                className="p-2.5 rounded-xl space-y-1.5"
                style={{
                  background: 'var(--pill-active-bg)',
                  border: '1px solid var(--pill-active-border)',
                }}
              >
                <div className="flex items-center gap-1.5 font-medium" style={{ color: 'var(--pct-high)' }}>
                  <Laptop className="w-3.5 h-3.5" aria-hidden="true" />
                  <span>
                    {platform === 'antigravity'
                      ? '本机官方态自动读取（按邮箱多号不覆盖）'
                      : '自动读取本机已登录账号'}
                  </span>
                </div>
                {platform === 'antigravity' ? (
                  <div className="text-[11px] leading-relaxed space-y-1" style={{ color: 'var(--text-sub)' }}>
                    <p>
                      <strong className="font-medium" style={{ color: 'var(--text-main)' }}>官方态怎么登录：</strong>
                      打开 Antigravity 桌面客户端 / IDE 在账号头像处登录 Google，或在终端运行 <code className="px-1 rounded bg-black/15">agy</code> 完成登录即可。
                    </p>
                    <p>
                      切换不同 Google 账号后再点读取会按邮箱新建独立卡片（互不覆盖）；如读取不到或已过期，可先打开客户端刷新或切换「手动 Token」。
                    </p>
                  </div>
                ) : (
                  <p className="text-[11px] leading-relaxed" style={{ color: 'var(--text-sub)' }}>
                    {meta.hint}
                  </p>
                )}
              </div>
            )}

            {activeMode.id === 'default_browser' && (
              <div
                className="p-2.5 rounded-xl space-y-2.5"
                style={{
                  background: 'var(--pill-active-bg)',
                  border: '1px solid var(--pill-active-border)',
                }}
              >
                <div className="flex items-center gap-1.5 font-medium" style={{ color: 'var(--pct-high)' }}>
                  <Globe className="w-3.5 h-3.5" aria-hidden="true" />
                  <span>通过系统默认浏览器直接授权登录</span>
                </div>

                {platform === 'codex' && (
                  <p className="text-[11px] leading-relaxed" style={{ color: 'var(--text-sub)' }}>
                    点击下方按钮将在系统默认浏览器打开 OpenAI 授权页面，无需任何配置，确认登录后自动回传 OAuth Token 并同步 5h / 7d 额度。
                  </p>
                )}

                {platform === 'claude' && (
                  <div className="space-y-2">
                    <p className="text-[11px] leading-relaxed" style={{ color: 'var(--text-sub)' }}>
                      在系统默认浏览器完成 Anthropic 授权后，复制网页上显示的一次性授权码粘贴到下方即可自动获取长期 OAuth Token。
                    </p>
                    <div className="flex items-center gap-2">
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => void handleStartClaudeOauth()}
                        className="px-3 py-1.5 rounded-xl glass-icon-btn font-medium"
                      >
                        1. 打开默认浏览器授权
                      </button>
                      {claudeFlowId && (
                        <span className="text-[10.5px] text-emerald-500 font-medium">
                          已拉起浏览器，请复制授权码
                        </span>
                      )}
                    </div>
                    {claudeFlowId && (
                      <div>
                        <label
                          htmlFor={claudeCodeId}
                          className="block mb-1 text-[11px] font-medium"
                          style={{ color: 'var(--text-sub)' }}
                        >
                          2. 粘贴浏览器页面显示的授权码
                        </label>
                        <input
                          id={claudeCodeId}
                          type="text"
                          value={claudeAuthCode}
                          onChange={(e) => setClaudeAuthCode(e.target.value)}
                          placeholder="粘贴授权码，例如 aBcDeF#state..."
                          className="aura-input w-full px-2.5 py-2 rounded-xl focus:outline-none focus:border-sky-400 text-xs"
                        />
                      </div>
                    )}
                  </div>
                )}

                {oauthWaiting && (
                  <div className="flex items-center justify-between gap-2 pt-1 text-[11px]">
                    <div className="flex items-center gap-1.5" style={{ color: 'var(--pct-high)' }}>
                      <Loader2 className="w-3.5 h-3.5 animate-spin shrink-0" aria-hidden="true" />
                      <span>请在默认浏览器完成授权，完成后自动导入...</span>
                    </div>
                    <button
                      type="button"
                      onClick={() => void handleCancelOauthWait()}
                      className="text-[10.5px] hover:underline shrink-0"
                      style={{ color: 'var(--text-sub)' }}
                    >
                      取消等待
                    </button>
                  </div>
                )}
              </div>
            )}

            {activeMode.id === 'webview' && (
              <div
                className="p-2.5 rounded-xl space-y-2"
                style={{
                  background: 'var(--badge-paid-bg)',
                  border: '1px solid var(--badge-paid-border)',
                }}
              >
                <div className="flex items-center gap-1.5 font-medium" style={{ color: 'var(--badge-paid-text)' }}>
                  <Globe className="w-3.5 h-3.5" aria-hidden="true" />
                  <span>独立目录隔离窗口（支持后台定时刷新）</span>
                </div>
                <p className="text-[10.5px] leading-relaxed" style={{ color: 'var(--text-sub)' }}>
                  每个账号使用独立的隔离会话目录登录官方页面，支持多个账号同时在线不串号。
                </p>
                <div className="flex flex-wrap gap-2">
                  <button
                    type="button"
                    disabled={busy}
                    onClick={handleOpenWebview}
                    className="px-3 py-1.5 rounded-xl glass-icon-btn font-medium"
                  >
                    1. 打开独立登录窗
                  </button>
                  {loginSessionId && (
                    <button
                      type="button"
                      disabled={busy}
                      onClick={handleCaptureWebview}
                      className="px-3 py-1.5 rounded-xl font-medium"
                      style={{
                        background: 'var(--pill-active-bg)',
                        border: '1px solid var(--pill-active-border)',
                        color: 'var(--pct-high)',
                      }}
                    >
                      2. 提取会话并读取额度
                    </button>
                  )}
                </div>
                {capturedInfo && (
                  <div className="flex items-center gap-1.5 text-emerald-500 text-[11px]">
                    <CheckCircle2 className="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
                    <span>
                      已捕获会话（{capturedInfo.count} 项 Cookie）
                      {typeof capturedInfo.pct === 'number'
                        ? ` · 实时主额度 ${capturedInfo.pct}%`
                        : ''}
                      {capturedInfo.plan ? ` · ${capturedInfo.plan}` : ''}
                    </span>
                  </div>
                )}
              </div>
            )}

            {activeMode.id === 'manual_secret' && (
              <div>
                <label
                  htmlFor={secretId}
                  className="flex items-center gap-1 mb-1 font-medium"
                  style={{ color: 'var(--text-sub)' }}
                >
                  <KeyRound className="w-3 h-3" aria-hidden="true" />
                  <span>{activeMode.secretLabel ?? '凭据内容'}（DPAPI 加密保存）</span>
                </label>
                <textarea
                  id={secretId}
                  rows={2}
                  value={secretInput}
                  onChange={(e) => setSecretInput(e.target.value)}
                  placeholder={activeMode.secretPlaceholder}
                  className="aura-input w-full px-2.5 py-2 rounded-xl focus:outline-none focus:border-sky-400 text-xs resize-none"
                />
              </div>
            )}

            {statusError && (
              <div
                role="alert"
                className="p-2 rounded-xl bg-rose-500/15 border border-rose-500/35 text-rose-500 text-[11px] space-y-1.5"
              >
                <p>{statusError}</p>
                {platform === 'antigravity' && activeMode.id === 'local_auto' && (
                  <div className="flex items-center gap-2">
                    <button
                      type="button"
                      onClick={() => handleSelectAuthMode('manual_secret')}
                      className="px-2 py-0.5 rounded-lg bg-rose-500/15 border border-rose-500/35 font-medium hover:bg-rose-500/25 transition"
                    >
                      改用手动 Token
                    </button>
                  </div>
                )}
              </div>
            )}
          </div>

          {/* Pinned Footer */}
          <div
            className="px-4 py-3 shrink-0 flex justify-end gap-2"
            style={{ borderTop: '1px solid var(--divider)' }}
          >
            <button
              type="button"
              onClick={() => dialogRef.current?.close()}
              className="px-3.5 py-2 rounded-xl glass-icon-btn font-medium"
            >
              取消
            </button>
            <button
              type="submit"
              disabled={busy}
              className="px-4 py-2 rounded-xl bg-gradient-to-r from-sky-500 to-indigo-500 text-white font-medium shadow-md hover:brightness-110 transition disabled:opacity-60"
            >
              {submitLabel}
            </button>
          </div>
        </form>
      </div>
    </dialog>
  );
};
