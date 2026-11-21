import React from 'react';

interface EdgeDockViewProps {
  avgPct: number;
  hasLowAlert: boolean;
  onExpand: () => void;
}

export const EdgeDockView: React.FC<EdgeDockViewProps> = ({
  avgPct,
  hasLowAlert,
  onExpand,
}) => {
  return (
    <div className="w-[36px] h-[110px] flex items-center justify-end">
      <button
        type="button"
        onClick={onExpand}
        aria-label={`恢复展开 AuraQuota 面板，当前综合额度 ${avgPct}%`}
        className="edge-dock w-full h-full glass-surface rounded-l-2xl py-3 px-2 flex flex-col items-center justify-center gap-1.5 border-r-0 cursor-pointer"
      >
        <span
          aria-hidden="true"
          className={`w-2 h-2 rounded-full ${
            hasLowAlert ? 'bg-amber-400 animate-pulse' : 'bg-emerald-400'
          }`}
        />
        <span className="text-[11px] font-semibold [writing-mode:vertical-rl] tracking-widest leading-none">
          AI 额度
        </span>
        <span className="text-[11px] font-bold text-sky-400 leading-none">{avgPct}%</span>
      </button>
    </div>
  );
};
