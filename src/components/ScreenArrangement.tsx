// Drag-and-drop editor for the shared screen arrangement. Screens are drawn to
// scale; on release a screen snaps against the nearest edge of another one
// (the pointer only crosses where edges touch) and never overlaps.

import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type PointerEvent } from "react";
import { Star } from "lucide-react";
import type { Placement, ScreenView } from "../types";

interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

const PADDING = 28;
const MAX_HEIGHT = 360;
/** World pixels of shared edge needed for the pointer to cross. */
const MIN_SHARED_EDGE = 120;

const key = (s: ScreenView) => `${s.pc}|${s.id}`;

function bounds(boxes: Box[]): Box {
  const x = Math.min(...boxes.map((b) => b.x));
  const y = Math.min(...boxes.map((b) => b.y));
  return {
    x,
    y,
    width: Math.max(...boxes.map((b) => b.x + b.width)) - x,
    height: Math.max(...boxes.map((b) => b.y + b.height)) - y,
  };
}

const overlaps = (a: Box, b: Box) =>
  a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

const clamp = (v: number, lo: number, hi: number) => Math.min(Math.max(v, lo), hi);

/** Edges meet with some overlap: the pointer can cross (same rule as layout.rs). */
function touching(a: Box, b: Box): boolean {
  const vertical = a.y < b.y + b.height && b.y < a.y + a.height;
  const horizontal = a.x < b.x + b.width && b.x < a.x + a.width;
  return ((a.x + a.width === b.x || b.x + b.width === a.x) && vertical) || ((a.y + a.height === b.y || b.y + b.height === a.y) && horizontal);
}

/** Screens that touch no other screen: the pointer can neither reach nor leave them. */
export function isolatedScreens<T extends Box>(screens: T[]): T[] {
  if (screens.length < 2) return [];
  return screens.filter((s) => !screens.some((o) => o !== s && touching(s, o)));
}

/** Pull `v` onto the closest of `targets` when within `reach`. */
function magnet(v: number, targets: number[], reach: number): number {
  let best = v;
  let distance = reach;
  for (const t of targets) {
    if (Math.abs(t - v) <= distance) {
      best = t;
      distance = Math.abs(t - v);
    }
  }
  return best;
}

/** Nearest spot touching another screen without overlapping any. */
export function snap(moving: Box, others: Box[]): { x: number; y: number } | null {
  if (others.length === 0) return { x: moving.x, y: moving.y };
  const reachY = moving.height * 0.08;
  const reachX = moving.width * 0.08;
  const candidates: { x: number; y: number }[] = [];
  for (const o of others) {
    const minY = o.y - moving.height + MIN_SHARED_EDGE;
    const maxY = o.y + o.height - MIN_SHARED_EDGE;
    const y = magnet(clamp(moving.y, minY, maxY), [o.y, o.y + o.height - moving.height, o.y + (o.height - moving.height) / 2], reachY);
    candidates.push({ x: o.x + o.width, y }, { x: o.x - moving.width, y });
    const minX = o.x - moving.width + MIN_SHARED_EDGE;
    const maxX = o.x + o.width - MIN_SHARED_EDGE;
    const x = magnet(clamp(moving.x, minX, maxX), [o.x, o.x + o.width - moving.width, o.x + (o.width - moving.width) / 2], reachX);
    candidates.push({ x, y: o.y + o.height }, { x, y: o.y - moving.height });
  }
  let best: { x: number; y: number } | null = null;
  let bestDistance = Infinity;
  for (const c of candidates) {
    const placed = { ...moving, x: Math.round(c.x), y: Math.round(c.y) };
    if (others.some((o) => overlaps(placed, o))) continue;
    const distance = Math.hypot(placed.x - moving.x, placed.y - moving.y);
    if (distance < bestDistance) {
      best = { x: placed.x, y: placed.y };
      bestDistance = distance;
    }
  }
  return best;
}

interface Drag {
  key: string;
  pointerId: number;
  startX: number;
  startY: number;
  origin: { x: number; y: number };
  frame: Box;
  scale: number;
}

export function ScreenArrangement({
  screens,
  thisPcName,
  peerName,
  editable,
  onCommit,
}: {
  screens: ScreenView[];
  thisPcName: string;
  peerName: string | null;
  editable: boolean;
  onCommit: (placements: Placement[]) => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(640);
  const [positions, setPositions] = useState<Record<string, { x: number; y: number }>>({});
  const [drag, setDrag] = useState<Drag | null>(null);

  // Follow the backend unless a drag is in progress.
  useEffect(() => {
    if (drag) return;
    setPositions(Object.fromEntries(screens.map((s) => [key(s), { x: s.x, y: s.y }])));
  }, [screens, drag]);

  useLayoutEffect(() => {
    const element = containerRef.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const boxes = screens.map((s) => ({ ...s, ...(positions[key(s)] ?? { x: s.x, y: s.y }) }));
  const isolated = new Set(drag ? [] : isolatedScreens(boxes).map(key));
  // Keep the frame still while dragging, so the scale doesn't jump.
  const frame = drag?.frame ?? (boxes.length ? bounds(boxes) : { x: 0, y: 0, width: 1920, height: 1080 });
  const scale =
    drag?.scale ??
    Math.min((width - PADDING * 2) / Math.max(frame.width, 1), (MAX_HEIGHT - PADDING * 2) / Math.max(frame.height, 1));
  const height = Math.round(frame.height * scale + PADDING * 2);
  const offsetX = (width - frame.width * scale) / 2;

  const onPointerDown = (e: PointerEvent<HTMLDivElement>, screen: ScreenView) => {
    if (!editable || e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    setDrag({
      key: key(screen),
      pointerId: e.pointerId,
      startX: e.clientX,
      startY: e.clientY,
      origin: positions[key(screen)] ?? { x: screen.x, y: screen.y },
      frame,
      scale,
    });
  };

  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    if (!drag || e.pointerId !== drag.pointerId) return;
    const x = Math.round(drag.origin.x + (e.clientX - drag.startX) / drag.scale);
    const y = Math.round(drag.origin.y + (e.clientY - drag.startY) / drag.scale);
    setPositions((prev) => ({ ...prev, [drag.key]: { x, y } }));
  };

  const onPointerUp = (e: PointerEvent<HTMLDivElement>) => {
    if (!drag || e.pointerId !== drag.pointerId) return;
    const moving = boxes.find((b) => key(b) === drag.key);
    const others = boxes.filter((b) => key(b) !== drag.key);
    const snapped = moving ? snap(moving, others) : null;
    const final = { ...positions, [drag.key]: snapped ?? drag.origin };
    setPositions(final);
    setDrag(null);
    const moved = snapped && (snapped.x !== drag.origin.x || snapped.y !== drag.origin.y);
    if (moved) {
      onCommit(screens.map((s) => ({ pc: s.pc, monitor: s.id, ...final[key(s)] })));
    }
  };

  return (
    <div className="arrangement">
      <div ref={containerRef} className={`arrangement-canvas${editable ? " editable" : ""}`} style={{ height }}>
        {boxes.map((s) => {
          const dragging = drag?.key === key(s);
          const style = {
            left: offsetX + (s.x - frame.x) * scale,
            top: PADDING + (s.y - frame.y) * scale,
            width: s.width * scale,
            height: s.height * scale,
          } as CSSProperties;
          return (
            <div
              key={key(s)}
              className={`arr-screen ${s.local ? "local" : "remote"}${dragging ? " dragging" : ""}${isolated.has(key(s)) ? " isolated" : ""}`}
              style={style}
              onPointerDown={(e) => onPointerDown(e, s)}
              onPointerMove={onPointerMove}
              onPointerUp={onPointerUp}
              onPointerCancel={onPointerUp}
              title={`${s.name} · ${s.width}×${s.height}`}
            >
              <span className="arr-number mono">{s.number}</span>
              <span className="arr-name">{s.name}</span>
              <span className="arr-meta mono">
                {s.width}×{s.height}
              </span>
              <span className="arr-pc">
                {s.primary && <Star aria-label="Main display" />}
                {s.local ? thisPcName : (peerName ?? "Other PC")}
              </span>
            </div>
          );
        })}
      </div>
      <div className="arrangement-legend">
        <span className="legend-item local">{thisPcName} (this PC)</span>
        {peerName && <span className="legend-item remote">{peerName}</span>}
      </div>
    </div>
  );
}
