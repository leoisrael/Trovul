// Showing people what they get: the feature tiles on a first run, and the
// "What's new" dialog once after an update (updates arrive over the link, so
// nobody reads release notes).

import { useEffect, useState, type CSSProperties } from "react";
import {
  ClipboardCopy,
  Crosshair,
  Files,
  Headphones,
  LayoutGrid,
  MousePointer2,
  Send,
  ShieldCheck,
  SlidersVertical,
  type LucideIcon,
} from "lucide-react";
import { useApp } from "../app/AppContext";
import type { Page } from "../app/router";
import { Button } from "../ui";

interface Highlight {
  icon: LucideIcon;
  color: string;
  title: string;
  text: string;
  page?: Page;
}

const FEATURES: Highlight[] = [
  {
    icon: Headphones,
    color: "var(--violet-light)",
    title: "Hear both PCs",
    text: "The other PC's sound in your headset, live, with no cables.",
  },
  {
    icon: MousePointer2,
    color: "var(--cyan)",
    title: "One keyboard and mouse",
    text: "Push the pointer past the edge and keep working on the other PC.",
  },
  {
    icon: SlidersVertical,
    color: "#c084fc",
    title: "A mixer for both",
    text: "Every app on both PCs: volume, mute and its own output.",
  },
  {
    icon: ClipboardCopy,
    color: "var(--emerald)",
    title: "Shared clipboard",
    text: "Copy text or a screenshot on one PC, paste it on the other.",
  },
];

/** What linking a second PC brings, shown on Home until the first link. */
export function FeatureTiles() {
  return (
    <div className="feature-tiles">
      {FEATURES.map(({ icon: Icon, color, title, text }, i) => (
        <div key={title} className="feature-tile" style={{ "--tile": color, animationDelay: `${i * 70}ms` } as CSSProperties}>
          <span className="feature-tile-icon">
            <Icon />
          </span>
          <strong>{title}</strong>
          <span>{text}</span>
        </div>
      ))}
    </div>
  );
}

// ---------- What's new ----------

/** Shown once to people coming from an earlier version. Bump with each release that has news. */
export const NEWS_VERSION = "0.7.9";
const NEWS: Highlight[] = [
  {
    icon: LayoutGrid,
    color: "#f472b6",
    title: "Stream Deck",
    text: "A plugin with keys for scenes, where the sound goes and mute, and volume dials on Stream Deck +. Turn on Settings → Stream Deck & controllers.",
    page: "settings",
  },
  {
    icon: Send,
    color: "var(--emerald)",
    title: "Send only the apps you choose",
    text: "On Windows 11, pick which apps go to the other PC, each at its own level: a stream mix apart from your headset.",
    page: "audio",
  },
  {
    icon: Crosshair,
    color: "var(--cyan)",
    title: "Follow the pointer",
    text: "In the headset, the PC you're using is louder. The other one comes back as soon as the pointer does.",
    page: "audio",
  },
  {
    icon: Files,
    color: "#c084fc",
    title: "Files between the PCs",
    text: "Copy files or folders in Explorer on one PC, paste them on the other. Big ones ask first.",
    page: "input",
  },
  {
    icon: ShieldCheck,
    color: "var(--violet-light)",
    title: "Admin windows, UAC and lock screen",
    text: "An option lets the other PC's keyboard and mouse work there too, installers included.",
    page: "input",
  },
];

export const SEEN_KEY = "trovul.news-seen";

function readSeen(): string | null {
  try {
    return localStorage.getItem(SEEN_KEY);
  } catch {
    return null;
  }
}

function markSeen() {
  try {
    localStorage.setItem(SEEN_KEY, NEWS_VERSION);
  } catch {
    // Private mode or blocked storage: the dialog may show again, harmlessly.
  }
}

export function WhatsNew() {
  const { settings, navigate } = useApp();
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!settings) return;
    const seen = readSeen();
    if (seen === NEWS_VERSION) return;
    // A fresh install has nothing "new": only people who used Trovul before
    // (they linked a PC at some point) see it.
    if (seen === null && !settings.last_peer) {
      markSeen();
      return;
    }
    const timer = window.setTimeout(() => setOpen(true), 1400); // after the splash
    return () => window.clearTimeout(timer);
  }, [settings]);

  if (!open) return null;
  const close = () => {
    markSeen();
    setOpen(false);
  };

  return (
    <div className="dialog-backdrop" onClick={(e) => e.target === e.currentTarget && close()}>
      <div className="dialog dialog-news" role="dialog" aria-modal="true" aria-labelledby="news-title">
        <p className="news-kicker">Trovul {NEWS_VERSION}</p>
        <h2 id="news-title" className="dialog-title gradient-text">
          What's new
        </h2>
        <ul className="news-list">
          {NEWS.map(({ icon: Icon, color, title, text, page }, i) => (
            <li key={title} style={{ "--tile": color, animationDelay: `${120 + i * 80}ms` } as CSSProperties}>
              <span className="feature-tile-icon">
                <Icon />
              </span>
              <div>
                <strong>{title}</strong>
                <span>{text}</span>
              </div>
              {page && (
                <button
                  className="link-action"
                  onClick={() => {
                    close();
                    navigate({ page });
                  }}
                >
                  Show me
                </button>
              )}
            </li>
          ))}
        </ul>
        <div className="dialog-actions">
          <Button variant="primary" onClick={close} autoFocus>
            Got it
          </Button>
        </div>
      </div>
    </div>
  );
}
