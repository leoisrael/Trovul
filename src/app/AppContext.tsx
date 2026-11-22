// App-wide state: the link, audio direction, settings, nearby PCs, the pairing
// prompt, toasts and the activity log. The backend pushes changes as events;
// pages read everything through `useApp()`.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, errorMessage, type SettingsPatch } from "../api";
import { usePolling, useWindowVisible } from "../hooks";
import type {
  AppInfo,
  AudioModeEvent,
  ConnectionEvent,
  ConnectionInfo,
  ConnectionRequest,
  Direction,
  Peer,
  Settings,
  UpdateState,
} from "../types";
import { useRoute, type Route } from "./router";

export type Tone = "info" | "success" | "warning" | "error";

export interface LogEntry {
  id: number;
  time: number;
  text: string;
  tone: Tone;
}

export interface ToastItem {
  id: number;
  tone: Exclude<Tone, "warning">;
  message: string;
}

/** The link as the UI shows it. */
export type LinkState = "linked" | "reconnecting" | "unlinked";

const DISCONNECTED: ConnectionInfo = {
  connected: false,
  is_initiator: false,
  peer_hostname: null,
  peer_ip: null,
  peer_device_id: null,
};
const MAX_LOG_ENTRIES = 60;

// Reasons sent back by the other PC's listener (src-tauri/src/connection.rs).
const REFUSALS: Record<string, string> = {
  declined: "declined the connection",
  "no answer": "did not answer the connection request",
  "not paired": "needs to approve this PC first",
  "busy with another connection": "is already connected to another PC",
  "busy with another request": "is answering another connection request",
};

const DIRECTION_LOG: Record<Direction, string> = {
  off: "Audio turned off",
  send: "Sending this PC's audio",
  receive: "Playing the other PC's audio here",
};

// Desktop notifications are the backend's pop-ups now (src-tauri/src/popups.rs).

interface AppValue {
  appInfo: AppInfo | null;
  connection: ConnectionInfo;
  link: LinkState;
  /** Connected PC, or the one being reconnected to. */
  peerName: string | null;
  direction: Direction;
  changingDirection: boolean;
  settings: Settings | null;
  peers: Peer[];
  connectingIp: string | null;
  incomingRequest: ConnectionRequest | null;
  /** Polling pauses while the window is hidden in the tray. */
  visible: boolean;
  logs: LogEntry[];
  toasts: ToastItem[];
  route: Route;
  /** Updates the linked PC can hand over. */
  update: UpdateState | null;

  navigate: (route: Route) => void;
  connect: (ip: string, deviceId?: string, name?: string) => Promise<void>;
  disconnect: () => void;
  setDirection: (direction: Direction) => Promise<void>;
  updateSettings: (patch: SettingsPatch) => void;
  setAutostart: (enabled: boolean) => void;
  testSound: () => void;
  answerRequest: (accept: boolean) => void;
  /** Error: toast + activity log. */
  report: (message: string) => void;
  notify: (message: string, tone?: ToastItem["tone"]) => void;
  dismissToast: (id: number) => void;
}

const AppContext = createContext<AppValue | null>(null);

export function useApp(): AppValue {
  const value = useContext(AppContext);
  if (!value) throw new Error("useApp outside AppProvider");
  return value;
}

export function AppProvider({ children }: { children: ReactNode }) {
  const [route, navigate] = useRoute();
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null);
  const [connection, setConnection] = useState<ConnectionInfo>(DISCONNECTED);
  const [peers, setPeers] = useState<Peer[]>([]);
  const [connectingIp, setConnectingIp] = useState<string | null>(null);
  const [direction, setDirectionState] = useState<Direction>("off");
  const [changingDirection, setChangingDirection] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [incomingRequest, setIncomingRequest] = useState<ConnectionRequest | null>(null);
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const [update, setUpdate] = useState<UpdateState | null>(null);
  const nextId = useRef(1);
  const visible = useWindowVisible();

  const addLog = useCallback((text: string, tone: Tone = "info") => {
    const entry = { id: nextId.current++, time: Date.now(), text, tone };
    setLogs((prev) => [entry, ...prev].slice(0, MAX_LOG_ENTRIES));
  }, []);

  const dismissToast = useCallback((id: number) => setToasts((prev) => prev.filter((t) => t.id !== id)), []);

  const notify = useCallback(
    (message: string, tone: ToastItem["tone"] = "info") => {
      const id = nextId.current++;
      setToasts((prev) => [...prev.slice(-3), { id, tone, message }]);
      window.setTimeout(() => dismissToast(id), tone === "error" ? 7000 : 4500);
    },
    [dismissToast],
  );

  const report = useCallback(
    (message: string) => {
      addLog(message, "error");
      notify(message, "error");
    },
    [addLog, notify],
  );

  // Initial state; afterwards the backend pushes changes as events.
  useEffect(() => {
    api.connectionInfo().then(setConnection).catch(() => {});
    api.direction().then(setDirectionState).catch(() => {});
    api.settings().then(setSettings).catch(() => {});
    api.updateState().then(setUpdate).catch(() => {});
  }, []);

  // Say it once when the linked PC turns out to have a newer Trovul.
  const offered = update?.available?.version;
  useEffect(() => {
    if (!offered || !update?.available) return;
    addLog(`${update.available.from} has Trovul ${offered} — update this PC from the banner`, "success");
  }, [offered]);

  // The name shown for this PC follows the "Name of this PC" setting.
  const pcName = settings?.pc_name;
  useEffect(() => {
    api.appInfo().then(setAppInfo).catch((e) => console.error("get_app_info failed:", e));
  }, [pcName]);

  useEffect(() => {
    const subscriptions = [
      listen<ConnectionEvent>("connection-changed", ({ payload }) => {
        setConnection(payload.info);
        const peer = payload.peer_hostname;
        if (payload.kind === "established") {
          addLog(`Linked with ${peer}`, "success");
        } else {
          const reason = payload.reason ? ` — ${payload.reason}` : "";
          addLog(`Disconnected from ${peer}${reason}`, payload.intentional ? "info" : "warning");
          if (!payload.intentional) addLog("Will reconnect automatically when it's back");
        }
      }),
      listen<ConnectionRequest>("connection-request", ({ payload }) => {
        setIncomingRequest(payload);
        addLog(`${payload.hostname} (${payload.ip}) asked to connect`);
      }),
      listen("connection-request-cancelled", () => {
        setIncomingRequest((current) => {
          if (current) notify(`The request from ${current.hostname} expired`);
          return null;
        });
      }),
      listen<AudioModeEvent>("audio-mode-changed", ({ payload }) => {
        setDirectionState(payload.direction);
        // A reason means something other than this PC's user changed it.
        if (payload.reason) {
          addLog(payload.reason, "warning");
          notify(payload.reason);
        }
      }),
      listen<Settings>("settings-changed", ({ payload }) => setSettings(payload)),
      listen<UpdateState>("update-changed", ({ payload }) => setUpdate(payload)),
      listen<string>("app-error", ({ payload }) => report(payload)),
      listen<string>("input-notice", ({ payload }) => {
        addLog(payload, "info");
        notify(payload);
      }),
    ];
    return () => subscriptions.forEach((s) => s.then((unlisten) => unlisten()));
  }, [addLog, notify, report]);

  // Safety net in case an event was missed while the webview was reloading.
  usePolling(async () => {
    setConnection(await api.connectionInfo());
  }, 5000, visible);

  const wantsPeers = route.page === "home" && !connection.connected;
  usePolling(async () => {
    setPeers(await api.discoveredPeers());
  }, 2000, visible && wantsPeers);

  const connect = useCallback(
    async (ip: string, deviceId?: string, name?: string) => {
      const label = name ?? ip;
      setConnectingIp(ip);
      addLog(`Connecting to ${label}…`);
      try {
        const result = await api.connect(ip, deviceId);
        if (!result.accepted) {
          const reason = result.reason ?? "";
          const message = `${label} ${REFUSALS[reason] ?? `refused the connection (${reason || "no reason given"})`}`;
          addLog(message, "warning");
          notify(message);
        }
      } catch (e) {
        report(`Could not connect to ${label}: ${errorMessage(e)}`);
      } finally {
        setConnectingIp(null);
      }
    },
    [addLog, notify, report],
  );

  const disconnect = useCallback(() => {
    api.disconnect().catch((e) => report(`Disconnect: ${errorMessage(e)}`));
  }, [report]);

  const setDirection = useCallback(
    async (next: Direction) => {
      setChangingDirection(true);
      try {
        setDirectionState(await api.setDirection(next));
        addLog(DIRECTION_LOG[next]);
      } catch (e) {
        report(errorMessage(e));
        api.direction().then(setDirectionState).catch(() => {});
      } finally {
        setChangingDirection(false);
      }
    },
    [addLog, report],
  );

  const updateSettings = useCallback(
    (patch: SettingsPatch) => {
      setSettings((prev) => (prev ? { ...prev, ...patch } : prev));
      api
        .updateSettings(patch)
        .then(setSettings)
        .catch((e) => report(`Settings: ${errorMessage(e)}`));
    },
    [report],
  );

  const setAutostart = useCallback(
    (enabled: boolean) => {
      api
        .setAutostart(enabled)
        .then((on) => setSettings((prev) => (prev ? { ...prev, autostart: on } : prev)))
        .catch((e) => report(errorMessage(e)));
    },
    [report],
  );

  const peerName = connection.peer_hostname ?? settings?.last_peer?.hostname ?? null;

  const testSound = useCallback(() => {
    const peer = connection.peer_hostname ?? "the other PC";
    api
      .playTestSound()
      .then(() =>
        notify(
          direction === "receive"
            ? `${peer} is playing a test chime — you should hear it here in a moment.`
            : `Playing a test chime — it should come out on ${peer}.`,
        ),
      )
      .catch((e) => report(`Test sound: ${errorMessage(e)}`));
  }, [connection.peer_hostname, direction, notify, report]);

  const answerRequest = useCallback(
    (accept: boolean) => {
      setIncomingRequest(null);
      (accept ? api.acceptConnection() : api.rejectConnection()).catch((e) => report(errorMessage(e)));
    },
    [report],
  );

  const last = settings?.last_peer;
  const link: LinkState = connection.connected
    ? "linked"
    : last?.resume && settings?.auto_reconnect
      ? "reconnecting"
      : "unlinked";

  const value = useMemo<AppValue>(
    () => ({
      appInfo,
      connection,
      link,
      peerName,
      direction,
      changingDirection,
      settings,
      peers,
      connectingIp,
      incomingRequest,
      visible,
      logs,
      toasts,
      route,
      update,
      navigate,
      connect,
      disconnect,
      setDirection,
      updateSettings,
      setAutostart,
      testSound,
      answerRequest,
      report,
      notify,
      dismissToast,
    }),
    [
      appInfo, connection, link, peerName, direction, changingDirection, settings, peers, connectingIp,
      incomingRequest, visible, logs, toasts, route, update, navigate, connect, disconnect, setDirection, updateSettings,
      setAutostart, testSound, answerRequest, report, notify, dismissToast,
    ],
  );

  return <AppContext.Provider value={value}>{children}</AppContext.Provider>;
}
