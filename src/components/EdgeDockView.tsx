import React from 'react';
import { AccountQuota, PlatformId } from '../types';

interface EdgeDockViewProps {
  activeAccount: AccountQuota | null;
  hasLowAlert: boolean;
  onExpand: () => void;
}

function edgeShortLabel(platform?: PlatformId): string {
  switch (platform) {
    case 'antigravity':
      return '反重力';
    case 'gemini':
      return 'Gemini';
    case 'codex':
      return 'Codex';
    case 'claude':
      return 'Claude';
    case 'commandcode':
      return 'CC';
    case 'opencode':
      return 'OC Go';
    default:
      return 'AI 额度';
  }
}

export const EdgeDockView: React.FC<EdgeDockViewProps> = ({
  activeAccount,
  hasLowAlert,
  onExpand,
}) => {
  const pct = activeAccount ? activeAccount.primaryPct : 0;
  const label = edgeShortLabel(activeAccount?.platform);

  return (
    <div className="w-[36px] h-[110px] flex items-center justify-end">
      <button
        type="button"
        onClick={onExpand}
        aria-label={`恢复展开 AuraQuota 面板，最近活跃 ${label} ${pct}%`}
        className="edge-dock w-full h-full glass-surface rounded-l-2xl py-3 px-2 flex flex-col items-center justify-center gap-1.5 border-r-0 cursor-pointer"
      >
        <span
          aria-hidden="true"
          className={`w-2 h-2 rounded-full ${
            hasLowAlert || (activeAccount && pct <= 25)
              ? 'bg-amber-400 animate-pulse'
              : 'bg-emerald-400'
          }`}
        />
        <span className="text-[11px] font-semibold [writing-mode:vertical-rl] tracking-widest leading-none">
          {label}
        </span>
        <span className="text-[11px] font-bold text-sky-400 leading-none tabular-nums">
          {activeAccount ? `${pct}%` : '--'}
        </span>
      </button>
    </div>
  );
};
