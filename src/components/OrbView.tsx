import React, { useRef } from 'react';
import { snapOrbToEdge, startWindowDragging } from '../api/tauriClient';

interface OrbViewProps {
  avgPct: number;
  lowestLabel: string;
  onExpand: () => void;
}

export const OrbView: React.FC<OrbViewProps> = ({ avgPct, lowestLabel, onExpand }) => {
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

  const circumference = 150.8;
  const dashOffset = (circumference * (1 - avgPct / 100)).toFixed(1);

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
        // After Windows native drag concludes, snap cleanly to screen edge
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

  return (
    <div className="w-[72px] h-[72px] flex items-center justify-center relative">
      <div className="relative">
        <div
          aria-hidden="true"
          className="orb-halo absolute inset-0 rounded-full bg-gradient-to-tr from-sky-400/45 via-indigo-400/45 to-fuchsia-400/45 blur-sm pointer-events-none"
        />
        <button
          type="button"
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={handlePointerUp}
          onClick={handleClick}
          title={`综合可用额度 ${avgPct}% · ${lowestLabel}（点击展开，拖动吸附边缘）`}
          aria-label={`展开额度面板，当前综合可用额度 ${avgPct}%`}
          className="orb-btn glass-surface flex items-center justify-center"
        >
          <svg className="w-14 h-14 -rotate-90" viewBox="0 0 60 60" aria-hidden="true">
            <defs>
              <linearGradient id="orbGrad" x1="0%" y1="0%" x2="100%" y2="100%">
                <stop offset="0%" stopColor="#38bdf8" />
                <stop offset="55%" stopColor="#818cf8" />
                <stop offset="100%" stopColor="#c084fc" />
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
              stroke="url(#orbGrad)"
              strokeWidth="4"
              strokeLinecap="round"
              strokeDasharray={circumference}
              strokeDashoffset={dashOffset}
              style={{ transition: 'stroke-dashoffset 0.6s cubic-bezier(0.22,1,0.36,1)' }}
            />
          </svg>
          <div className="absolute inset-0 flex flex-col items-center justify-center pointer-events-none">
            <span className="text-sm font-bold leading-none">{avgPct}%</span>
            <span className="text-[9px] mt-0.5 opacity-75">AI额度</span>
          </div>
        </button>
      </div>
    </div>
  );
};
