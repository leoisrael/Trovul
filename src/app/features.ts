// What the link between two PCs can do. The sidebar is built from this list,
// so a new capability (screen…) starts here: add an entry, a page in
// src/pages, and a section on Home.

import { MousePointer2, ScreenShare, SlidersVertical, type LucideIcon } from "lucide-react";
import type { Page } from "./router";

export interface Feature {
  id: Extract<Page, "audio" | "input" | "screen">;
  name: string;
  tagline: string;
  icon: LucideIcon;
  /** CSS colour; each feature has its own hue from the brand gradient. */
  color: string;
  status: "live" | "soon";
  /** Shown on the "coming soon" page. */
  highlights: string[];
}

export const FEATURES: Feature[] = [
  {
    id: "audio",
    name: "Mixer",
    tagline: "Hear both PCs in one headset, and mix every app on both",
    icon: SlidersVertical,
    color: "var(--feature-audio)",
    status: "live",
    highlights: [],
  },
  {
    id: "input",
    name: "Keyboard & Mouse",
    tagline: "One keyboard and mouse for both PCs",
    icon: MousePointer2,
    color: "var(--feature-input)",
    status: "live",
    highlights: [
      "Push the pointer past the edge of your screen to control the other PC",
      "Your keyboard follows the pointer, with no switch to press",
      "Copy on one PC, paste on the other",
    ],
  },
  {
    id: "screen",
    name: "Screen",
    tagline: "See and control the other PC's screen",
    icon: ScreenShare,
    color: "var(--feature-screen)",
    status: "soon",
    highlights: [
      "Watch the other PC's screen in a window, at LAN speed",
      "Take control with your own keyboard and mouse when you need to",
      "Pick one monitor or the whole desktop",
    ],
  },
];

/** Upcoming features appear in the UI (marked "Soon"). False hides them. */
export const SHOW_UPCOMING = false;

export const visibleFeatures = () => FEATURES.filter((f) => f.status === "live" || SHOW_UPCOMING);

export const feature = (id: Feature["id"]) => FEATURES.find((f) => f.id === id)!;
