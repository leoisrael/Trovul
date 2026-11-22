import { useId } from "react";

/** The Trovul mark: two arcs (the PCs) and two nodes (the link), in the brand gradient. */
export function Logo({ size = 28, animated }: { size?: number; animated?: boolean }) {
  const id = useId();
  return (
    <svg
      viewBox="0 0 100 100"
      width={size}
      height={size}
      className={`logo${animated ? " logo-animated" : ""}`}
      aria-hidden="true"
    >
      <defs>
        <linearGradient id={`${id}-g`} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stopColor="#a78bfa" />
          <stop offset="55%" stopColor="#22d3ee" />
          <stop offset="100%" stopColor="#34d399" />
        </linearGradient>
      </defs>
      <path className="logo-arc-a" d="M75 20 A35 35 0 0 1 75 80" fill="none" stroke={`url(#${id}-g)`} strokeWidth="10" strokeLinecap="round" />
      <path className="logo-arc-b" d="M25 80 A35 35 0 0 1 25 20" fill="none" stroke={`url(#${id}-g)`} strokeWidth="10" strokeLinecap="round" opacity="0.55" />
      <circle className="logo-node-a" cx="40" cy="36" r="8" fill="#a78bfa" opacity="0.75" />
      <circle className="logo-node-b" cx="60" cy="64" r="8" fill="#22d3ee" />
    </svg>
  );
}
