// Rotary control in the style of an audio interface: a 270° value arc, a ring
// of LEDs for the live level, the value in the middle. Drag up/down, scroll,
// use the arrow keys; double-click goes back to 100 %.

import { useEffect, useId, useRef, type CSSProperties, type ReactNode } from "react";

const START = -135; // degrees from 12 o'clock
const SWEEP = 270;
const LEDS = 24;
/** Pixels of vertical drag for the full range (four times that with Shift). */
const DRAG_RANGE = 180;

const clamp = (v: number) => Math.min(1, Math.max(0, v));

function point(radius: number, degrees: number): [number, number] {
  const rad = ((degrees - 90) * Math.PI) / 180;
  return [50 + radius * Math.cos(rad), 50 + radius * Math.sin(rad)];
}

function arc(radius: number, from: number, to: number): string {
  const [x1, y1] = point(radius, from);
  const [x2, y2] = point(radius, to);
  return `M ${x1.toFixed(2)} ${y1.toFixed(2)} A ${radius} ${radius} 0 ${to - from > 180 ? 1 : 0} 1 ${x2.toFixed(2)} ${y2.toFixed(2)}`;
}

// ---------- The LED ring ----------
// The level arrives with each poll (every 100 ms). One animation loop for all
// dials makes the ring behave like a VU meter: it jumps up at once, falls
// smoothly, and a peak LED hangs on for a moment. It toggles classes on the
// LEDs directly, so a moving level never re-renders React.

const FALL_PER_S = 1.4; // of the full ring
const PEAK_HOLD_MS = 900;

interface Ring {
  leds: (SVGLineElement | null)[];
  target: number;
  shown: number;
  peak: number;
  peakAt: number;
  lit: number;
  peakLed: number;
}

const rings = new Set<Ring>();
let frame = 0;
let lastFrame = 0;

function animate(now: number) {
  const dt = lastFrame ? Math.min((now - lastFrame) / 1000, 0.1) : 0;
  lastFrame = now;
  for (const r of rings) {
    r.shown = r.target >= r.shown ? r.target : Math.max(r.target, r.shown - dt * FALL_PER_S);
    if (r.shown >= r.peak) {
      r.peak = r.shown;
      r.peakAt = now;
    } else if (now - r.peakAt > PEAK_HOLD_MS) {
      r.peak = Math.max(r.shown, r.peak - dt * FALL_PER_S * 0.6);
    }
    const lit = Math.round(r.shown * LEDS);
    if (lit !== r.lit) {
      for (let i = Math.min(lit, r.lit); i < Math.max(lit, r.lit); i++) r.leds[i]?.classList.toggle("on", i < lit);
      r.lit = lit;
    }
    const top = Math.min(LEDS - 1, Math.ceil(r.peak * LEDS) - 1);
    const peakLed = r.peak > 0.03 && top >= lit ? top : -1;
    if (peakLed !== r.peakLed) {
      if (r.peakLed >= 0) r.leds[r.peakLed]?.classList.remove("peak");
      if (peakLed >= 0) r.leds[peakLed]?.classList.add("peak");
      r.peakLed = peakLed;
    }
  }
  // Rest when every ring has settled (a hidden window keeps its last levels);
  // a new level starts the loop again.
  let moving = false;
  for (const r of rings) moving ||= r.shown !== r.target || r.peak !== r.shown;
  frame = moving ? requestAnimationFrame(animate) : 0;
  if (!frame) lastFrame = 0;
}

function kick() {
  if (!frame && rings.size > 0) frame = requestAnimationFrame(animate);
}

function addRing(ring: Ring) {
  rings.add(ring);
  kick();
}

// A wheel gesture that began on the page keeps scrolling the page when the
// pointer slides over a dial, instead of turning it.
let pageWheelAt = 0;
let watchingWheel = false;
function watchPageWheel() {
  if (watchingWheel) return;
  watchingWheel = true;
  document.addEventListener(
    "wheel",
    (e) => {
      if (!(e.target instanceof Element && e.target.closest(".dial"))) pageWheelAt = performance.now();
    },
    { capture: true, passive: true },
  );
}

export function Dial({
  value,
  level = 0,
  muted,
  disabled,
  label,
  center,
  onChange,
}: {
  /** 0–1. */
  value: number;
  /** Live signal 0–1, lit on the LED ring. */
  level?: number;
  muted?: boolean;
  disabled?: boolean;
  label: string;
  /** Replaces the value in the middle (e.g. "Off", an icon). */
  center?: ReactNode;
  /** Without it the dial is a meter only. */
  onChange?: (value: number) => void;
}) {
  const gradient = `dial${useId().replace(/[^\w-]/g, "")}`;
  const ref = useRef<HTMLDivElement>(null);
  const latest = useRef({ value, onChange });
  const drag = useRef<{ y: number; from: number } | null>(null);
  const leds = useRef<(SVGLineElement | null)[]>([]);
  const ring = useRef<Ring | null>(null);
  const interactive = !!onChange && !disabled;
  const target = muted || disabled ? 0 : Math.min(Math.sqrt(Math.max(level, 0)), 1);

  useEffect(() => {
    const r: Ring = { leds: leds.current, target: 0, shown: 0, peak: 0, peakAt: 0, lit: 0, peakLed: -1 };
    ring.current = r;
    addRing(r);
    return () => {
      rings.delete(r);
    };
  }, []);

  useEffect(() => {
    if (ring.current) {
      ring.current.target = target;
      kick();
    }
  }, [target]);

  useEffect(() => {
    latest.current = { value, onChange };
  });

  useEffect(() => {
    const el = ref.current;
    if (!el || !interactive) return;
    watchPageWheel();
    const onWheel = (e: WheelEvent) => {
      const now = performance.now();
      if (now - pageWheelAt < 350) {
        pageWheelAt = now;
        return;
      }
      e.preventDefault();
      const { value: current, onChange: change } = latest.current;
      change?.(clamp(Math.round((current + (e.deltaY < 0 ? 0.02 : -0.02)) * 100) / 100));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [interactive]);

  const set = (next: number) => onChange?.(clamp(Math.round(next * 100) / 100));

  const onKeyDown = (e: React.KeyboardEvent) => {
    const step = { ArrowUp: 0.02, ArrowRight: 0.02, ArrowDown: -0.02, ArrowLeft: -0.02, PageUp: 0.1, PageDown: -0.1 }[e.key];
    if (step !== undefined) set(value + step);
    else if (e.key === "Home") set(0);
    else if (e.key === "End") set(1);
    else return;
    e.preventDefault();
  };

  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { y: e.clientY, from: value };
  };
  const onPointerMove = (e: React.PointerEvent) => {
    if (!drag.current) return;
    const range = e.shiftKey ? DRAG_RANGE * 4 : DRAG_RANGE;
    set(drag.current.from + (drag.current.y - e.clientY) / range);
  };
  const endDrag = () => {
    drag.current = null;
  };

  const shown = clamp(value);
  const angle = START + SWEEP * shown;
  const percent = Math.round(shown * 100);
  // A meter-only dial reports its level.
  const reported = onChange ? percent : Math.round(target * 100);

  return (
    <div
      ref={ref}
      className={`dial${muted ? " muted" : ""}${disabled ? " disabled" : ""}${interactive ? " interactive" : ""}`}
      role={onChange ? "slider" : "meter"}
      tabIndex={interactive ? 0 : undefined}
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={reported}
      aria-valuetext={muted ? `${reported}%, muted` : `${reported}%`}
      aria-disabled={disabled || undefined}
      onKeyDown={interactive ? onKeyDown : undefined}
      onPointerDown={interactive ? onPointerDown : undefined}
      onPointerMove={interactive ? onPointerMove : undefined}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onDoubleClick={interactive ? () => set(1) : undefined}
    >
      <svg viewBox="0 0 100 100" aria-hidden="true">
        <defs>
          <linearGradient id={gradient} x1="0" y1="1" x2="1" y2="0">
            <stop offset="0%" style={{ stopColor: "var(--dial-from, var(--violet))" }} />
            <stop offset="100%" style={{ stopColor: "var(--dial-to, var(--cyan))" }} />
          </linearGradient>
        </defs>
        {Array.from({ length: LEDS }, (_, i) => {
          const a = START + (SWEEP * (i + 0.5)) / LEDS;
          const [x1, y1] = point(43.5, a);
          const [x2, y2] = point(48, a);
          const zone = i / LEDS < 0.66 ? "low" : i / LEDS < 0.86 ? "mid" : "high";
          return (
            <line
              key={i}
              ref={(el) => {
                leds.current[i] = el;
              }}
              className={`dial-led ${zone}`}
              x1={x1}
              y1={y1}
              x2={x2}
              y2={y2}
            />
          );
        })}
        <path className="dial-track" d={arc(36, START, START + SWEEP)} />
        {shown > 0.004 && !center && <path className="dial-arc" d={arc(36, START, angle)} stroke={`url(#${gradient})`} />}
      </svg>
      <div className="dial-knob">
        {!center && <span className="dial-pointer" style={{ "--angle": `${angle}deg` } as CSSProperties} />}
        {center !== undefined ? (
          <span className="dial-note">{center}</span>
        ) : (
          <span className="dial-value">
            {percent}
            <small>%</small>
          </span>
        )}
      </div>
    </div>
  );
}

/** Colours for a dial and its strip, e.g. `dialColors("var(--violet)", "var(--cyan)")`. */
export const dialColors = (from: string, to: string) => ({ "--dial-from": from, "--dial-to": to }) as CSSProperties;
