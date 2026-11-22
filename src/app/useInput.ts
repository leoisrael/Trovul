// Keyboard & mouse sharing state for any page that shows it.

import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import type { ClipboardView, InputStatus, InputView } from "../types";
import { useApp } from "./AppContext";

/** One short line for the sharing switch. */
export function inputStatusText(status: InputStatus, peer: string): { text: string; ok: boolean } {
  switch (status) {
    case "off":
      return { text: "Off", ok: false };
    case "waiting":
      return { text: "Connect a PC first", ok: false };
    case "unsupported":
      return { text: `Update Trovul on ${peer}`, ok: false };
    case "peer_off":
      return { text: `Turn it on on ${peer} too`, ok: false };
    case "ready":
      return { text: "On — move the pointer to the other PC's screens", ok: true };
    case "controlling":
      return { text: `You're using ${peer}`, ok: true };
    case "controlled":
      return { text: `${peer} is using this PC`, ok: true };
  }
}

/** Keeps the keyboard & mouse view up to date (events + one fetch). */
export function useInputView(): [InputView | null, (view: InputView) => void] {
  const { link } = useApp();
  const [view, setView] = useState<InputView | null>(null);

  useEffect(() => {
    const unlisten = listen<InputView>("input-changed", ({ payload }) => setView(payload));
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  useEffect(() => {
    (link === "linked" ? api.refreshInput() : api.inputView()).then(setView).catch(() => {});
  }, [link]);

  return [view, setView];
}

/** The shared-clipboard switch and its last activity (events + one fetch). */
export function useClipboardView(): [ClipboardView | null, (view: ClipboardView) => void] {
  const [view, setView] = useState<ClipboardView | null>(null);
  useEffect(() => {
    api.clipboardView().then(setView).catch(() => {});
    const unlisten = listen<ClipboardView>("clipboard-changed", ({ payload }) => setView(payload));
    return () => {
      unlisten.then((f) => f());
    };
  }, []);
  return [view, setView];
}
