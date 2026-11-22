// Mixer scenes: capture the mix of both PCs, and put it back with one click.
// Capturing, applying and storing are the backend's (`scenes.rs`), so a
// Stream Deck applies them too; the main window and the tray mixer share the
// list through the "scenes-changed" event.

import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  Briefcase,
  Clapperboard,
  Gamepad2,
  Headphones,
  Moon,
  Music,
  Radio,
  Sparkles,
  type LucideIcon,
} from "lucide-react";
import { api } from "../api";
import type { MixScene } from "../types";

export function useScenes(): [MixScene[] | null, (scenes: MixScene[]) => Promise<void>] {
  const [scenes, setScenes] = useState<MixScene[] | null>(null);
  useEffect(() => {
    api.mixScenes().then(setScenes).catch(() => setScenes([]));
    const unlisten = listen<MixScene[]>("scenes-changed", ({ payload }) => setScenes(payload));
    return () => {
      unlisten.then((f) => f());
    };
  }, []);
  const save = async (next: MixScene[]) => setScenes(await api.setMixScenes(next));
  return [scenes, save];
}

/** An icon that fits the scene's name. */
export function sceneIcon(name: string): LucideIcon {
  const n = name.toLowerCase();
  if (/game|gaming|jog|play|fps|rank/.test(n)) return Gamepad2;
  if (/stream|live|twitch|obs|youtube/.test(n)) return Radio;
  if (/work|call|meet|reuni|trabalh|office|zoom|teams/.test(n)) return Briefcase;
  if (/music|m[uú]sica|spotify|chill/.test(n)) return Music;
  if (/movie|film|video|v[ií]deo|netflix|cinema/.test(n)) return Clapperboard;
  if (/night|noite|sleep|quiet|sil[eê]n/.test(n)) return Moon;
  if (/focus|foco|headphone|fone/.test(n)) return Headphones;
  return Sparkles;
}

/** The mix both PCs have right now (the backend captures it). */
export function captureScene(name: string, _linked: boolean, id?: string): Promise<MixScene> {
  return api.captureScene(name, id ?? null);
}

/** Puts a scene back on both PCs (backend: also works from a Stream Deck); returns a summary. */
export function applyScene(scene: MixScene, _linked: boolean): Promise<string> {
  return api.applyScene(scene.id);
}
