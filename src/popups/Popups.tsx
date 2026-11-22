// Pop-ups: cards in the corner of the screen for what happens between the PCs
// (src-tauri/src/popups.rs). The window holds the stack; it's sized to the
// cards and hidden when the last one goes. Cards without buttons or progress
// fade on their own after a few seconds.

import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  ArrowDownToLine,
  ClipboardCopy,
  Files,
  Image,
  Info,
  Link2,
  Link2Off,
  ShieldAlert,
  Sparkles,
  Volume2,
  X,
  type LucideIcon,
} from "lucide-react";
import { api } from "../api";
import type { PopupCard } from "../types";

const ICONS: Record<string, LucideIcon> = {
  link: Link2,
  unlink: Link2Off,
  image: Image,
  clipboard: ClipboardCopy,
  files: Files,
  update: ArrowDownToLine,
  shield: ShieldAlert,
  volume: Volume2,
  scene: Sparkles,
};

const TONES: Record<PopupCard["tone"], string> = {
  info: "var(--violet-light)",
  success: "var(--success)",
  warning: "var(--warning)",
  error: "var(--danger)",
};

const FADE_AFTER_MS = 5500;
const LEAVE_MS = 220;

export function Popups() {
  const [cards, setCards] = useState<PopupCard[]>([]);
  const [leaving, setLeaving] = useState<Set<number>>(new Set());
  const stack = useRef<HTMLDivElement>(null);
  const timers = useRef(new Map<number, number>());

  /** Slide the card out, then drop it. */
  const leave = (id: number) => {
    setLeaving((prev) => new Set(prev).add(id));
    window.setTimeout(() => {
      setCards((prev) => prev.filter((c) => c.id !== id));
      setLeaving((prev) => {
        const next = new Set(prev);
        next.delete(id);
        return next;
      });
    }, LEAVE_MS);
  };

  const close = (id: number, action = "close") => {
    leave(id);
    api.popupAction(id, action).catch(() => {});
  };

  useEffect(() => {
    api.popups().then(setCards).catch(() => {});
    const subscriptions = [
      listen<PopupCard>("popup", ({ payload }) =>
        setCards((prev) =>
          prev.some((c) => c.id === payload.id) ? prev.map((c) => (c.id === payload.id ? payload : c)) : [...prev, payload],
        ),
      ),
      listen<number>("popup-dismissed", ({ payload }) => leave(payload)),
    ];
    return () => subscriptions.forEach((s) => s.then((unlisten) => unlisten()));
  }, []);

  // Passing cards fade on their own; a card that turns sticky keeps its timer off.
  useEffect(() => {
    for (const card of cards) {
      const timer = timers.current.get(card.id);
      if (!card.sticky && timer === undefined) {
        timers.current.set(
          card.id,
          window.setTimeout(() => close(card.id), FADE_AFTER_MS),
        );
      } else if (card.sticky && timer !== undefined) {
        window.clearTimeout(timer);
        timers.current.delete(card.id);
      }
    }
    for (const [id, timer] of timers.current) {
      if (!cards.some((c) => c.id === id)) {
        window.clearTimeout(timer);
        timers.current.delete(id);
      }
    }
    if (cards.length === 0) api.hidePopups().catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cards]);

  // The window is exactly as tall as the cards.
  useLayoutEffect(() => {
    const el = stack.current;
    if (!el) return;
    const observer = new ResizeObserver(() => {
      const height = Math.ceil(el.getBoundingClientRect().height);
      // Empty: the window is about to hide; shrinking it first would flash a strip.
      if (height > 0) api.resizePopups(height).catch(() => {});
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  return (
    <div className="popups" ref={stack}>
      {cards.map((card) => {
        const Icon = ICONS[card.icon] ?? Info;
        return (
          <div
            key={card.id}
            className={`popup-card${leaving.has(card.id) ? " leaving" : ""}`}
            style={{ "--tone": TONES[card.tone] } as CSSProperties}
            role="status"
            onMouseEnter={() => {
              const timer = timers.current.get(card.id);
              if (timer !== undefined) window.clearTimeout(timer);
              timers.current.delete(card.id);
            }}
            onMouseLeave={() => {
              if (!card.sticky && !timers.current.has(card.id)) {
                timers.current.set(card.id, window.setTimeout(() => close(card.id), 2500));
              }
            }}
          >
            <span className="popup-icon">
              <Icon />
            </span>
            <div className="popup-body">
              <strong>{card.title}</strong>
              {card.text && <span>{card.text}</span>}
              {card.progress !== null && (
                <div className="popup-progress" aria-label="Progress">
                  <div style={{ width: `${Math.round(card.progress * 100)}%` }} />
                </div>
              )}
              {card.actions.length > 0 && (
                <div className="popup-actions">
                  {card.actions.map((action) => (
                    <button
                      key={action.id}
                      type="button"
                      className={action.primary ? "primary" : undefined}
                      // The backend runs the action, then replaces or dismisses the card.
                      onClick={() => api.popupAction(card.id, action.id).catch(() => {})}
                    >
                      {action.label}
                    </button>
                  ))}
                </div>
              )}
            </div>
            <button type="button" className="popup-close" aria-label="Close" onClick={() => close(card.id)}>
              <X />
            </button>
          </div>
        );
      })}
    </div>
  );
}
