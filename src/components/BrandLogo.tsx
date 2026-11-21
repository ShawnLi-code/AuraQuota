import React from 'react';

interface BrandLogoProps {
  size?: number;
  className?: string;
}

export const BrandLogo: React.FC<BrandLogoProps> = ({ size = 30, className = '' }) => {
  const uid = React.useId().replace(/:/g, '');
  const small = size < 24;
  const strokeW = small ? 13.5 : 11.5;
  const dotR = small ? 9.5 : 8.5;

  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 128 128"
      aria-hidden="true"
      className={`shrink-0 ${className}`}
    >
      <defs>
        <linearGradient id={`aqBg-${uid}`} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stopColor="#191e2b" />
          <stop offset="100%" stopColor="#0b0e16" />
        </linearGradient>
        <linearGradient id={`aqArc1-${uid}`} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stopColor="#ffffff" />
          <stop offset="100%" stopColor="#cbd5e1" />
        </linearGradient>
        <linearGradient id={`aqArc2-${uid}`} x1="0" y1="1" x2="1" y2="0">
          <stop offset="0%" stopColor="#14b8a6" />
          <stop offset="100%" stopColor="#38bdf8" />
        </linearGradient>
      </defs>
      <rect x="4" y="4" width="120" height="120" rx="28" fill={`url(#aqBg-${uid})`} />
      {!small && (
        <>
          <rect
            x="5"
            y="5"
            width="118"
            height="118"
            rx="28"
            fill="none"
            stroke="rgba(255,255,255,0.16)"
            strokeWidth="2"
          />
          <circle
            cx="64"
            cy="64"
            r="44"
            fill="none"
            stroke="rgba(255,255,255,0.06)"
            strokeWidth="1.5"
          />
        </>
      )}
      <path
        d="M 92 48 A 32 32 0 1 0 92 80"
        fill="none"
        stroke={`url(#aqArc1-${uid})`}
        strokeWidth={strokeW}
        strokeLinecap="round"
      />
      <path
        d="M 64 32 A 32 32 0 0 1 96 64"
        fill="none"
        stroke={`url(#aqArc2-${uid})`}
        strokeWidth={strokeW}
        strokeLinecap="round"
      />
      <path
        d="M 73 73 L 94 94"
        fill="none"
        stroke={`url(#aqArc2-${uid})`}
        strokeWidth={strokeW}
        strokeLinecap="round"
      />
      <circle cx="64" cy="64" r={dotR} fill="#38bdf8" />
    </svg>
  );
};
