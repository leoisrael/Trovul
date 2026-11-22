// Hash routing (#/settings, #/audio for the mixer): no library, survives webview reloads, and lets
// the UI preview open any screen by URL.

import { useCallback, useEffect, useState } from "react";

export type Page = "home" | "audio" | "input" | "screen" | "settings";

export interface Route {
  page: Page;
  /** Sub-view, e.g. the Audio page's tab. */
  tab?: string;
}

const PAGES: Page[] = ["home", "audio", "input", "screen", "settings"];
/** Routes from earlier versions of the UI. */
const RENAMED: Record<string, Page> = { overview: "home", pcs: "settings" };

export function parseRoute(hash: string): Route {
  const [raw, tab] = hash.replace(/^#\/?/, "").split("/");
  const page = RENAMED[raw] ?? raw;
  return PAGES.includes(page as Page) ? { page: page as Page, tab: tab || undefined } : { page: "home" };
}

export function routeHash(route: Route): string {
  return `#/${route.page}${route.tab ? `/${route.tab}` : ""}`;
}

export function useRoute(): [Route, (route: Route) => void] {
  const [route, setRoute] = useState(() => parseRoute(window.location.hash));

  useEffect(() => {
    const onHashChange = () => setRoute(parseRoute(window.location.hash));
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  const navigate = useCallback((next: Route) => {
    const hash = routeHash(next);
    if (window.location.hash !== hash) window.location.hash = hash;
    else setRoute(next);
  }, []);

  return [route, navigate];
}
