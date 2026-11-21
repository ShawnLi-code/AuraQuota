import React, { useState } from 'react';
import { ChevronDown, ChevronUp, KeyRound, RefreshCw, Trash2, Wallet } from 'lucide-react';
import { AccountQuota, PLATFORMS } from '../types';

interface AccountCardProps {
  account: AccountQuota;
  expanded: boolean;
  privacy: boolean;
  onToggleExpand: () => void;
  onRefresh: (id: string) => Promise<void>;
  onReauth: (account: AccountQuota) => void;
  onRemove: (id: string) => Promise<void>;
}

export function getBarColorClass(pct: number): string {
  if (pct <= 25) return 'bg-gradient-to-r from-rose-500 to-orange-400';
  if (pct <= 55) return 'bg-gradient-to-r from-amber-400 to-yellow-300';
  return 'bg-gradient-to-r from-sky-400 to-indigo-400';
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

export const AccountCard: React.FC<AccountCardProps> = ({
  account,
  expanded,
  privacy,
  onToggleExpand,
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

  const handleRefreshClick = async () => {
    setRefreshing(true);
    try {
      await onRefresh(account.id);
    } finally {
      setRefreshing(false);
    }
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
    return null;
  })();

  return (
    <li className="glass-card rounded-2xl p-3.5 list-none">
      <button
        type="button"
        aria-expanded={expanded}
        onClick={onToggleExpand}
        className="w-full text-left rounded-xl"
      >
        <div className="flex items-center justify-between gap-2">
          <div className="flex items-center gap-2 min-w-0">
            <span
              aria-hidden="true"
              className="w-2.5 h-2.5 rounded-full shrink-0"
              style={{
                background: pInfo.color,
                boxShadow: `0 0 8px ${pInfo.color}88`,
              }}
            />
            <span className="text-[12.5px] font-semibold truncate">
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
          </div>

          <div className="flex items-center gap-1.5 shrink-0">
            <span
              className="text-sm font-bold tabular-nums"
              style={{ color: getPctColorVar(account.primaryPct) }}
            >
              {account.primaryPct}%
            </span>
            {expanded ? (
              <ChevronUp className="w-3.5 h-3.5 opacity-60" aria-hidden="true" />
            ) : (
              <ChevronDown className="w-3.5 h-3.5 opacity-60" aria-hidden="true" />
            )}
          </div>
        </div>

        <div
          className="mt-2.5 h-1.5 rounded-full overflow-hidden"
          style={{ background: 'var(--track-bg)' }}
        >
          <div
            className={`bar-fill h-full rounded-full ${getBarColorClass(account.primaryPct)}`}
            style={{ width: `${account.primaryPct}%` }}
          />
        </div>

        <div
          className="mt-1.5 flex items-center justify-between text-[11px]"
          style={{ color: 'var(--text-sub)' }}
        >
          <span>剩余主额度</span>
          <span className="tabular-nums">{account.resetIn}</span>
        </div>
      </button>

      {expanded && (
        <div
          className="mt-3 pt-2.5 space-y-2.5"
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
                  onClick={handleRefreshClick}
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
