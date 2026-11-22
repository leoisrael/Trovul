// Inputs: switch, segmented control, slider (with optional live level), LED meter.

import type { CSSProperties } from "react";
import type { LucideIcon } from "lucide-react";

export function Toggle({
  checked,
  disabled,
  label,
  onChange,
}: {
  checked: boolean;
  disabled?: boolean;
  label: string;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className="toggle" title={label}>
      <input
        type="checkbox"
        role="switch"
        checked={checked}
        disabled={disabled}
        aria-label={label}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className="toggle-track" />
    </label>
  );
}

export interface SegmentOption<T extends string> {
  value: T;
  label: string;
  icon?: LucideIcon;
  title?: string;
}

export function Segmented<T extends string>({
  value,
  options,
  label,
  disabled,
  full,
  onChange,
}: {
  value: T;
  options: SegmentOption<T>[];
  label: string;
  disabled?: boolean;
  full?: boolean;
  onChange: (value: T) => void;
}) {
  return (
    <div className={`segmented${full ? " full" : ""}`} role="radiogroup" aria-label={label}>
      {options.map(({ value: option, label: text, icon: Icon, title }) => (
        <button
          key={option}
          type="button"
          role="radio"
          aria-checked={value === option}
          disabled={disabled}
          title={title}
          onClick={() => option !== value && onChange(option)}
        >
          {Icon && <Icon aria-hidden="true" />}
          {text}
        </button>
      ))}
    </div>
  );
}

/** Linear level 0–1 on a perceptual (square-root) scale, as a CSS percentage. */
export function levelPercent(level: number): string {
  return `${Math.min(Math.sqrt(Math.max(level, 0)), 1) * 100}%`;
}

/** Volume slider (0–1). With `level`, the live signal glows inside the track. */
export function Slider({
  value,
  level,
  muted,
  label,
  showValue = true,
  onChange,
}: {
  value: number;
  level?: number;
  muted?: boolean;
  label: string;
  showValue?: boolean;
  onChange: (value: number) => void;
}) {
  const percent = Math.round(value * 100);
  const style = {
    "--value": `${percent}%`,
    "--level": level === undefined ? "0%" : levelPercent(level),
  } as CSSProperties;
  return (
    <div className="field-row">
      <div className={`slider${muted ? " muted" : ""}`} style={style}>
        <div className="slider-track">
          <div className="slider-fill" />
          {level !== undefined && <div className="slider-level" />}
        </div>
        <input
          type="range"
          min={0}
          max={100}
          value={percent}
          aria-label={label}
          onChange={(e) => onChange(Number(e.target.value) / 100)}
        />
      </div>
      {showValue && <span className="slider-value">{percent}%</span>}
    </div>
  );
}

/** Segmented LED meter for a linear 0–1 level. */
export function Meter({ level, muted }: { level: number; muted?: boolean }) {
  return (
    <div className={`meter${muted ? " muted" : ""}`} style={{ "--level": levelPercent(level) } as CSSProperties} aria-hidden="true">
      <div className="meter-fill" />
    </div>
  );
}
