import React, { useState } from 'react';
import {
  ChevronDown,
  ChevronUp,
  KeyRound,
  Pin,
  PinOff,
  RefreshCw,
  Trash2,
  Wallet,
} from 'lucide-react';
import { AccountQuota, PLATFORMS } from '../types';

interface AccountCardProps {
  account: AccountQuota;
  variant?: 'hero' | 'compact';
  isPinned?: boolean;
  activeLabel?: string | null;
  expanded: boolean;
  privacy: boolean;
  onToggleExpand: () => void;
  onTogglePin: (id: string) => Promise<void>;
  onRefresh: (id: string) => Promise<void>;
  onReauth: (account: AccountQuota) => void;
  onRemove: (id: string) => Promise<void>;
}

export function getBarColorClass(pct: number): string {
  if (pct <= 25) return 'bg-gradient-to-r from-rose-500 to-orange-400';
  if (pct <= 55) return 'bg-gradient-to-r from-amber-400 to-yellow-300';
  return 'bg-gradient-to-r from-teal-400 via-sky-400 to-indigo-400';
}

export function getPctColorVar(pct: number): string {
  if (pct <= 25) return 'var(--pct-low)';
  if (pct <= 55) return 'var(--pct-mid)';
  return 'var(--pct-high)';
}

function maskEmail(email: string, privacy: boolean): string {
  if (!privacy) return email;
  const parts = email.split('@');
  if (parts.length !== 2) return '••••••';
  const [user, domain] = parts;
  return `${user.slice(0, 2)}•••@${domain}`;
}

function compactPoolLabel(name: string): string {
  if (name.includes('Gemini 模型池 (5h)')) return 'Gem 5h';
  if (name.includes('Gemini 模型池 (周)')) return 'Gem 周';
  if (name.includes('Argon 模型池 (5h)')) return 'Argon 5h';
  if (name.includes('Argon 模型池 (周)')) return 'Argon 周';
  if (name.includes('Argon 模型池')) return 'Argon';
  if (name.includes('Claude / GPT 池 (5h)')) return 'Claude 5h';
  if (name.includes('Claude / GPT 池 (周)')) return 'Claude 周';
  if (name.startsWith('5小时')) return '5h';
  if (name.startsWith('7天周')) return '周';
  if (name.startsWith('月度')) return '月';
  if (name.length > 12) return name.slice(0, 12);
  return name;
}

export const AccountCard: React.FC<AccountCardProps> = ({
  account,
  variant = 'compact',
  isPinned = false,
  activeLabel,
  expanded,
  privacy,
  onToggleExpand,
  onTogglePin,
  onRefresh,
  onReauth,
  onRemove,
}) => {
  const [refreshing, setRefreshing] = useState(false);
  const pInfo = PLATFORMS[account.platform];
  const dupPrefix = `${pInfo.name} · `;
  const displayAlias = account.alias.startsWith(dupPrefix)
    ? account.alias.slice(dupPrefix.length)
    : account.alias;

  const isHero = variant === 'hero';
  const showDetails = isHero || expanded;

  const handleRefreshClick = async (e: React.MouseEvent) => {
    e.stopPropagation();
    setRefreshing(true);
    try {
      await onRefresh(account.id);
    } finally {
      setRefreshing(false);
    }
  };

  const handlePinClick = async (e: React.MouseEvent) => {
    e.stopPropagation();
    await onTogglePin(account.id);
  };

  const statusTag = (() => {
    if (account.isDemo) {
      return (
        <span
          className="px-1.5 py-0.5 rounded-md text-[10px] font-medium shrink-0"
          style={{
            background: 'var(--pill-active-bg)',
            color: 'var(--pct-high)',
            border: '1px solid var(--pill-active-border)',
          }}
        >
          示例
        </span>
      );
    }
    if (account.status === 'auth_required') {
      return (
        <span className="px-1.5 py-0.5 rounded-md text-[10px] font-medium bg-rose-500/15 text-rose-500 border border-rose-500/30 shrink-0">
          需重登
        </span>
      );
    }
    if (account.status === 'offline' || account.status === 'error') {
      return (
        <span className="px-1.5 py-0.5 rounded-md text-[10px] font-medium bg-amber-500/15 text-amber-500 border border-amber-500/30 shrink-0">
          离线
        </span>
      );
    }
    if (isHero) {
      return (
        <span className="px-1.5 py-0.5 rounded-full text-[10px] font-medium bg-emerald-500/15 text-emerald-400 border border-emerald-500/30 inline-flex items-center gap-1 shrink-0">
          <span className="w-1.5 h-1.5 rounded-full bg-emerald-400 animate-pulse" />
          {isPinned ? '已固定' : activeLabel || '使用中'}
        </span>
      );
    }
    return null;
  })();

  const compactSummaryText = (() => {
    if (account.statusMessage && account.status !== 'ok' && account.status !== 'demo') {
      return account.statusMessage;
    }
    const parts = account.subPools.map(
      (sp) => `${compactPoolLabel(sp.name)} ${sp.pct}%`
    );
    if (account.extraBalance) {
      parts.push(`余额 ${account.extraBalance.amount}`);
    }
    return parts.length > 0 ? parts.join(' · ') : '剩余主额度';
  })();

  return (
    <li
      className={`${
        isHero ? 'glass-hero-card rounded-2xl p-3.5' : 'glass-card rounded-[14px] px-3 py-2.5'
      } list-none`}
    >
      <div className="flex items-center justify-between gap-2">
        <button
          type="button"
          aria-expanded={showDetails}
          onClick={onToggleExpand}
          className="flex items-center gap-2 min-w-0 flex-1 text-left"
        >
          <span
            aria-hidden="true"
            className="w-2 h-2 rounded-full shrink-0"
            style={{
              background: pInfo.color,
              boxShadow: `0 0 8px ${pInfo.color}99`,
            }}
          />
          <span className={`${isHero ? 'text-[13px]' : 'text-[12px]'} font-semibold truncate`}>
            {pInfo.name} · {displayAlias}
          </span>
          {account.tier === 'paid' ? (
            <span
              className="px-1.5 py-0.5 rounded-md text-[10px] font-semibold shrink-0"
              style={{
                background: 'var(--badge-paid-bg)',
                color: 'var(--badge-paid-text)',
                border: '1px solid var(--badge-paid-border)',
              }}
            >
              {account.planLabel}
            </span>
          ) : (
            <span
              className="px-1.5 py-0.5 rounded-md text-[10px] font-medium shrink-0"
              style={{
                background: 'var(--pill-bg)',
                color: 'var(--text-sub)',
                border: '1px solid var(--border-subtle)',
              }}
            >
              {account.planLabel}
            </span>
          )}
          {statusTag}
        </button>

        <div className="flex items-center gap-1.5 shrink-0">
          <button
            type="button"
            onClick={(e) => void handlePinClick(e)}
            title={isPinned ? '取消固定，恢复自动跟随正在使用的应用' : '手动置顶为当前主焦点'}
            aria-label={isPinned ? '取消置顶' : '置顶此账号'}
            className={`p-1 rounded-lg transition ${
              isPinned
                ? 'text-sky-400 bg-sky-400/15'
                : 'opacity-55 hover:opacity-100 hover:text-sky-400'
            }`}
          >
            {isPinned ? (
              <PinOff className="w-3 h-3" aria-hidden="true" />
            ) : (
              <Pin className="w-3 h-3" aria-hidden="true" />
            )}
          </button>

          <button
            type="button"
            onClick={onToggleExpand}
            className="flex items-center gap-1"
            aria-label={expanded ? '收起详情' : '展开详情'}
          >
            <span
              className={`${isHero ? 'text-base' : 'text-[13px]'} font-bold tabular-nums`}
              style={{ color: getPctColorVar(account.primaryPct) }}
            >
              {account.primaryPct}%
            </span>
            {!isHero &&
              (expanded ? (
                <ChevronUp className="w-3.5 h-3.5 opacity-55" aria-hidden="true" />
              ) : (
                <ChevronDown className="w-3.5 h-3.5 opacity-55" aria-hidden="true" />
              ))}
          </button>
        </div>
      </div>

      {!isHero && (
        <button type="button" onClick={onToggleExpand} className="w-full text-left mt-2">
          <div
            className="h-1 rounded-full overflow-hidden"
            style={{ background: 'var(--track-bg)' }}
          >
            <div
              className={`bar-fill h-full rounded-full ${getBarColorClass(account.primaryPct)}`}
              style={{ width: `${account.primaryPct}%` }}
            />
          </div>
          <div
            className="mt-1.5 flex items-start justify-between gap-2 text-[10.5px]"
            style={{ color: 'var(--text-sub)' }}
          >
            <span className="leading-snug">{compactSummaryText}</span>
            <span className="shrink-0 tabular-nums opacity-80">{account.resetIn}</span>
          </div>
        </button>
      )}

      {isHero && account.subPools.length === 0 && (
        <div className="mt-2.5">
          <div
            className="h-1.5 rounded-full overflow-hidden"
            style={{ background: 'var(--track-bg)' }}
          >
            <div
              className={`bar-fill h-full rounded-full ${getBarColorClass(account.primaryPct)}`}
              style={{ width: `${account.primaryPct}%` }}
            />
          </div>
        </div>
      )}

      {showDetails && (
        <div
          className={`${isHero ? 'mt-2.5 pt-2' : 'mt-2.5 pt-2.5'} space-y-2`}
          style={{ borderTop: '1px solid var(--divider)' }}
        >
          {account.subPools.map((sp) => (
            <div key={sp.name}>
              <div className="flex justify-between text-[11px] mb-1 gap-2">
                <span className="truncate" style={{ color: 'var(--text-sub)' }}>
                  {sp.name}
                </span>
                <span
                  className="font-semibold shrink-0 tabular-nums"
                  style={{ color: getPctColorVar(sp.pct) }}
                >
                  {sp.pct}%
                </span>
              </div>
              <div
                className="h-1.5 rounded-full overflow-hidden"
                style={{ background: 'var(--track-bg)' }}
              >
                <div
                  className={`bar-fill h-full rounded-full ${getBarColorClass(sp.pct)}`}
                  style={{ width: `${sp.pct}%` }}
                />
              </div>
              {(sp.detail || sp.resetIn) && (
                <div
                  className="mt-0.5 flex justify-between text-[10px]"
                  style={{ color: 'var(--text-muted)' }}
                >
                  <span>{sp.detail ?? ''}</span>
                  <span className="tabular-nums">{sp.resetIn ?? ''}</span>
                </div>
              )}
            </div>
          ))}

          {account.extraBalance && (
            <div className="rounded-xl px-2.5 py-2 bg-emerald-500/10 border border-emerald-500/25 flex items-center justify-between gap-2">
              <div className="min-w-0">
                <div className="flex items-center gap-1.5 text-[11px] font-medium text-emerald-500">
                  <Wallet className="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
                  <span className="truncate">{account.extraBalance.label}</span>
                </div>
                {account.extraBalance.note && (
                  <p className="text-[10px] mt-0.5" style={{ color: 'var(--text-sub)' }}>
                    {account.extraBalance.note}
                  </p>
                )}
              </div>
              <span className="text-xs font-bold text-emerald-500 shrink-0 tabular-nums">
                {account.extraBalance.amount}
              </span>
            </div>
          )}

          {account.statusMessage && (
            <p
              className="text-[10.5px] leading-relaxed px-2.5 py-1.5 rounded-lg"
              style={{
                background: 'var(--pill-bg)',
                color: 'var(--text-sub)',
                border: '1px solid var(--border-subtle)',
              }}
            >
              {account.statusMessage}
            </p>
          )}

          <div
            className="flex items-center justify-between pt-1 text-[11px] gap-2"
            style={{ color: 'var(--text-sub)' }}
          >
            <span className="truncate">{maskEmail(account.email, privacy)}</span>
            <div className="flex items-center gap-2.5 shrink-0">
              {!account.isDemo && (
                <button
                  type="button"
                  onClick={(e) => void handleRefreshClick(e)}
                  disabled={refreshing}
                  className="inline-flex items-center gap-1 hover:text-sky-500 transition"
                >
                  <RefreshCw
                    className={`w-3 h-3 ${refreshing ? 'spinning' : ''}`}
                    aria-hidden="true"
                  />
                  <span>刷新</span>
                </button>
              )}
              {account.status === 'auth_required' && (
                <button
                  type="button"
                  onClick={() => onReauth(account)}
                  className="inline-flex items-center gap-1 text-amber-500 hover:text-amber-400 transition"
                >
                  <KeyRound className="w-3 h-3" aria-hidden="true" />
                  <span>重登</span>
                </button>
              )}
              <button
                type="button"
                onClick={() => void onRemove(account.id)}
                className="inline-flex items-center gap-1 text-rose-500 hover:text-rose-400 transition"
              >
                <Trash2 className="w-3 h-3" aria-hidden="true" />
                <span>移除</span>
              </button>
            </div>
          </div>
        </div>
      )}
    </li>
  );
};
