import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import type { AudioStats } from "./types";

/**
 * Runs `task` every `intervalMs` while `enabled`. The next run is scheduled
 * only after the previous one finished, so slow calls never pile up.
 */
export function usePolling(task: () => Promise<void>, intervalMs: number, enabled: boolean) {
  const taskRef = useRef(task);
  taskRef.current = task;

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    let timer: number | undefined;
    const tick = async () => {
      try {
        await taskRef.current();
      } catch {
        // Tasks report their own errors.
      }
      if (!cancelled) timer = window.setTimeout(tick, intervalMs);
    };
    tick();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [intervalMs, enabled]);
}

/** False while the window is hidden to the tray or minimised — polling pauses. */
export function useWindowVisible(): boolean {
  const [documentVisible, setDocumentVisible] = useState(!document.hidden);
  const [shown, setShown] = useState(true);

  useEffect(() => {
    const onVisibilityChange = () => setDocumentVisible(!document.hidden);
    document.addEventListener("visibilitychange", onVisibilityChange);
    // The app may have started hidden in the tray (Windows sign-in).
    invoke<boolean>("get_window_visible").then(setShown).catch(() => {});
    const unlisten = listen<boolean>("window-visibility", (e) => setShown(e.payload));
    return () => {
      document.removeEventListener("visibilitychange", onVisibilityChange);
      unlisten.then((f) => f());
    };
  }, []);

  return documentVisible && shown;
}

/** Leading + trailing throttle: the last value always gets through. */
export function useThrottle<T>(fn: (value: T) => void, ms: number): (value: T) => void {
  const fnRef = useRef(fn);
  fnRef.current = fn;
  const state = useRef<{ last: number; timer?: number; pending?: { value: T } }>({ last: 0 });

  useEffect(() => () => window.clearTimeout(state.current.timer), []);

  return useCallback(
    (value: T) => {
      const s = state.current;
      s.pending = { value };
      if (s.timer !== undefined) return;
      const run = () => {
        s.timer = undefined;
        s.last = Date.now();
        const pending = s.pending;
        s.pending = undefined;
        if (pending) fnRef.current(pending.value);
      };
      const wait = ms - (Date.now() - s.last);
      if (wait <= 0) run();
      else s.timer = window.setTimeout(run, wait);
    },
    [ms],
  );
}

/**
 * A slider value that follows the backend, except while the user is moving it:
 * their value is shown (and sent, throttled) until they have let go for a moment,
 * so polled updates don't make the handle jump back.
 */
export function useLiveValue(value: number, commit: (value: number) => void): [number, (value: number) => void] {
  const [held, setHeld] = useState<number | null>(null);
  const timer = useRef<number | undefined>(undefined);
  const send = useThrottle(commit, 50);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  const change = useCallback(
    (next: number) => {
      setHeld(next);
      send(next);
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setHeld(null), 600);
    },
    [send],
  );

  return [held ?? value, change];
}

/** Audio engine stats: fast while streaming (smooth meters), lazy otherwise. */
export function useAudioStats(active: boolean, streaming: boolean): AudioStats | null {
  const [stats, setStats] = useState<AudioStats | null>(null);
  usePolling(async () => {
    setStats(await api.audioStats());
  }, streaming ? 120 : 1000, active);
  return stats;
}

/**
 * Icons are fetched once per id instead of riding along with every poll.
 * `scope` resets the cache (e.g. when the remote PC changes).
 */
export function useIconCache(fetchIcons: (ids: string[]) => Promise<Record<string, string>>, scope?: unknown) {
  const [icons, setIcons] = useState<Record<string, string>>({});
  const requested = useRef(new Set<string>());

  useEffect(() => {
    setIcons({});
    requested.current = new Set();
  }, [scope]);

  const ensure = useCallback(
    (ids: (string | null)[]) => {
      const missing = ids.filter((id): id is string => !!id && !requested.current.has(id));
      if (missing.length === 0) return;
      missing.forEach((id) => requested.current.add(id));
      fetchIcons(missing)
        .then((found) => setIcons((prev) => ({ ...prev, ...found })))
        .catch(() => missing.forEach((id) => requested.current.delete(id)));
    },
    [fetchIcons],
  );

  return { icons, ensure };
}
