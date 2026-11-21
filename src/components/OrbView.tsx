import React, { useRef } from 'react';
import { snapOrbToEdge, startWindowDragging } from '../api/tauriClient';
import { AccountQuota, PLATFORMS, PlatformId } from '../types';

interface OrbViewProps {
  activeAccount: AccountQuota | null;
  onExpand: () => void;
}

function shortOrbLabel(platform?: PlatformId): string {
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
      return 'AI额度';
  }
}

function orbStrokeColors(pct: number): [string, string] {
  if (pct <= 25) return ['#fb7185', '#f97316'];
  if (pct <= 55) return ['#fbbf24', '#f59e0b'];
  return ['#2dd4bf', '#38bdf8'];
}

export const OrbView: React.FC<OrbViewProps> = ({ activeAccount, onExpand }) => {
  const dragRef = useRef<{
    down: boolean;
    startedNativeDrag: boolean;
    startX: number;
    startY: number;
  }>({
    down: false,
    startedNativeDrag: false,
    startX: 0,
    startY: 0,
  });

  const pct = activeAccount ? activeAccount.primaryPct : 0;
  const label = shortOrbLabel(activeAccount?.platform);
  const pInfo = activeAccount ? PLATFORMS[activeAccount.platform] : null;
  const [c1, c2] = orbStrokeColors(pct);

  const circumference = 150.8;
  const dashOffset = (circumference * (1 - pct / 100)).toFixed(1);

  const handlePointerDown = (e: React.PointerEvent<HTMLButtonElement>) => {
    if (e.button !== 0) return;
    dragRef.current = {
      down: true,
      startedNativeDrag: false,
      startX: e.screenX,
      startY: e.screenY,
    };
  };

  const handlePointerMove = async (e: React.PointerEvent<HTMLButtonElement>) => {
    const state = dragRef.current;
    if (!state.down || state.startedNativeDrag) return;
    const dist = Math.hypot(e.screenX - state.startX, e.screenY - state.startY);
    if (dist > 4) {
      state.startedNativeDrag = true;
      state.down = false;
      try {
        await startWindowDragging();
      } finally {
        setTimeout(() => {
          void snapOrbToEdge();
        }, 60);
      }
    }
  };

  const handlePointerUp = () => {
    const wasDragging = dragRef.current.startedNativeDrag;
    dragRef.current.down = false;
    dragRef.current.startedNativeDrag = false;
    if (wasDragging) {
      void snapOrbToEdge();
    }
  };

  const handleClick = () => {
    if (!dragRef.current.startedNativeDrag) {
      onExpand();
    }
  };

  const tip = activeAccount
    ? `当前活跃：${pInfo?.name ?? ''} · ${activeAccount.alias} (${pct}% · ${activeAccount.resetIn}) — 点击展开，拖动吸附边缘`
    : 'AuraQuota — 点击展开添加监控账号';

  return (
    <div className="w-[72px] h-[72px] flex items-center justify-center relative">
      <div className="relative">
        <div
          aria-hidden="true"
          className="orb-halo absolute inset-0 rounded-full blur-sm pointer-events-none"
          style={{
            background: `linear-gradient(135deg, ${c1}55, ${c2}55)`,
          }}
        />
        <button
          type="button"
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={handlePointerUp}
          onClick={handleClick}
          title={tip}
          aria-label={tip}
          className="orb-btn glass-surface flex items-center justify-center"
        >
          <svg className="w-14 h-14 -rotate-90" viewBox="0 0 60 60" aria-hidden="true">
            <defs>
              <linearGradient id="orbGradActive" x1="0%" y1="0%" x2="100%" y2="100%">
                <stop offset="0%" stopColor={c1} />
                <stop offset="100%" stopColor={c2} />
              </linearGradient>
            </defs>
            <circle
              cx="30"
              cy="30"
              r="24"
              fill="none"
              stroke="rgba(148,163,184,0.22)"
              strokeWidth="4"
            />
            <circle
              cx="30"
              cy="30"
              r="24"
              fill="none"
              stroke="url(#orbGradActive)"
              strokeWidth="4"
              strokeLinecap="round"
              strokeDasharray={circumference}
              strokeDashoffset={dashOffset}
              style={{ transition: 'stroke-dashoffset 0.6s cubic-bezier(0.22,1,0.36,1)' }}
            />
          </svg>
          <div className="absolute inset-0 flex flex-col items-center justify-center pointer-events-none">
            <span className="text-sm font-bold leading-none tabular-nums">
              {activeAccount ? `${pct}%` : '--'}
            </span>
            <span
              className="text-[9px] mt-0.5 font-medium truncate max-w-[44px]"
              style={{ color: activeAccount ? c2 : 'var(--text-sub)' }}
            >
              {label}
            </span>
          </div>
        </button>
      </div>
    </div>
  );
};
