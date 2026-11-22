// The tray mixer: opens above the tray icon on a left click (see
// src-tauri/src/flyout.rs). Where the sound goes, this PC's master and the
// other PC's sound as dials, then every app as a slider. It has its own small
// state instead of AppContext, which belongs to the main window.

import { useEffect, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { CirclePause, Headphones, Maximize2, Send, Volume2, VolumeX } from "lucide-react";
import { api, errorMessage } from "../api";
import { Logo } from "../components/Logo";
import { useIconCache, useLiveValue, usePolling } from "../hooks";
import { AppIcon, STRIP_COLORS } from "../pages/mixer/Deck";
import { SceneChips } from "../pages/mixer/Scenes";
import type { AppInfo, AudioModeEvent, ConnectionInfo, Direction, MixerSnapshot } from "../types";
import { Dial, Dot, Segmented, Slider, type SegmentOption } from "../ui";

export function Flyout() {
  const [shown, setShown] = useState(true);
  const [mixer, setMixer] = useState<MixerSnapshot | null>(null);
  const [connection, setConnection] = useState<ConnectionInfo | null>(null);
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null);
  const [direction, setDirection] = useState<Direction>("off");
  const [changing, setChanging] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const icons = useIconCache(api.localIcons);

  useEffect(() => {
    const refresh = () => {
      api.connectionInfo().then(setConnection).catch(() => {});
      api.direction().then(setDirection).catch(() => {});
      api.appInfo().then(setAppInfo).catch(() => {});
    };
    refresh();
    const subscriptions = [
      listen<boolean>("flyout-visibility", ({ payload }) => {
        setShown(payload);
        if (payload) {
          setError(null);
          refresh();
        }
      }),
      listen("connection-changed", refresh),
      listen<AudioModeEvent>("audio-mode-changed", ({ payload }) => setDirection(payload.direction)),
    ];
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && api.hideFlyout();
    window.addEventListener("keydown", onKey);
    return () => {
      subscriptions.forEach((s) => s.then((unlisten) => unlisten()));
      window.removeEventListener("keydown", onKey);
    };
  }, []);

  usePolling(async () => {
    const next = await api.localMixer();
    setMixer(next);
    icons.ensure(next.sessions.map((s) => s.icon_id));
  }, 100, shown);

  const linked = !!connection?.connected;
  const peer = connection?.peer_hostname ?? "the other PC";
  const fail = (what: string) => (e: unknown) => setError(`${what}: ${errorMessage(e)}`);

  const changeDirection = async (next: Direction) => {
    setChanging(true);
    try {
      setDirection(await api.setDirection(next));
    } catch (e) {
      fail("Audio")(e);
    } finally {
      setChanging(false);
    }
  };

  const options: SegmentOption<Direction>[] = [
    { value: "receive", label: "Hear", icon: Headphones, title: `Play ${peer}'s sound on this PC` },
    { value: "send", label: "Send", icon: Send, title: `Play this PC's sound on ${peer}` },
    { value: "off", label: "Off", icon: CirclePause, title: "No sound between the PCs" },
  ];
  const stream = mixer?.stream;
  const update = (change: (prev: MixerSnapshot) => MixerSnapshot) => setMixer((prev) => prev && change(prev));

  return (
    <div className="flyout">
      <header className="flyout-head">
        <Logo size={22} />
        <div className="flyout-title">
          <strong>{appInfo?.hostname ?? "Trovul"}</strong>
          <span>
            <Dot tone={linked ? "success" : "neutral"} pulse={linked} />
            {linked ? `Linked to ${peer}` : "No PC connected"}
          </span>
        </div>
        <button className="flyout-open" title="Open Trovul" aria-label="Open Trovul" onClick={() => api.openMainWindow()}>
          <Maximize2 />
        </button>
      </header>

      {linked && (
        <Segmented full label="Where the sound plays" value={direction} disabled={changing} options={options} onChange={changeDirection} />
      )}

      <SceneChips linked={linked} onMessage={() => setError(null)} onError={setError} />

      <div className="flyout-buses">
        {mixer?.master && (
          <Bus
            label="Master"
            sub={mixer.master.name}
            colors={STRIP_COLORS.master}
            value={mixer.master.volume}
            level={mixer.master.peak}
            muted={mixer.master.muted}
            onVolume={(v) => api.setMasterVolume(v).catch(fail("Master"))}
            onMute={() => {
              const muted = !mixer.master!.muted;
              api.setMasterMute(muted).catch(fail("Master"));
              update((prev) => ({ ...prev, master: prev.master && { ...prev.master, muted } }));
            }}
          />
        )}
        {linked && stream?.receiving && (
          <Bus
            label="Return"
            sub={stream.ducked && !stream.muted ? "Lowered for a call" : `from ${peer}`}
            warn={stream.ducked && !stream.muted}
            colors={STRIP_COLORS.stream}
            value={stream.volume}
            level={stream.level}
            muted={stream.muted}
            onVolume={(v) => api.updateSettings({ remote_volume: v }).catch(fail(peer))}
            onMute={() => {
              const muted = !stream.muted;
              api.updateSettings({ remote_muted: muted }).catch(fail(peer));
              update((prev) => ({ ...prev, stream: prev.stream && { ...prev.stream, muted } }));
            }}
          />
        )}
        {linked && stream && !stream.receiving && (
          <Bus label="Send" sub={`to ${peer}`} colors={STRIP_COLORS.send} value={1} level={stream.level} center={<Send />} />
        )}
        {linked && !stream && (
          <Bus label="Return" sub="Off" colors={STRIP_COLORS.stream} value={0} center="Off" off />
        )}
      </div>

      <div className="flyout-apps">
        {mixer?.sessions.map((app) => (
          <AppRow
            key={app.id}
            name={app.name}
            icon={<AppIcon name={app.name} src={app.icon_id ? icons.icons[app.icon_id] : undefined} />}
            value={app.volume}
            level={app.peak_level}
            muted={app.muted}
            onVolume={(v) => api.setSessionVolume(app.pid, v).catch(fail(app.name))}
            onMute={() => {
              api.setSessionMute(app.pid, !app.muted).catch(fail(app.name));
              update((prev) => ({
                ...prev,
                sessions: prev.sessions.map((s) => (s.pid === app.pid ? { ...s, muted: !app.muted } : s)),
              }));
            }}
          />
        ))}
        {mixer && mixer.sessions.length === 0 && <p className="flyout-empty">No app is playing sound.</p>}
      </div>

      {error && <p className="flyout-error">{error}</p>}
    </div>
  );
}

function Bus({
  label,
  sub,
  warn,
  colors,
  value,
  level,
  muted = false,
  center,
  off,
  onVolume,
  onMute,
}: {
  label: string;
  sub: string;
  warn?: boolean;
  colors: React.CSSProperties;
  value: number;
  level?: number;
  muted?: boolean;
  center?: ReactNode;
  off?: boolean;
  onVolume?: (volume: number) => void;
  onMute?: () => void;
}) {
  const [shown, change] = useLiveValue(value, onVolume ?? (() => {}));
  return (
    <div className={`flyout-bus${off ? " is-off" : ""}`} style={colors}>
      <Dial
        value={shown}
        level={level}
        muted={muted}
        disabled={off}
        label={`${label} volume`}
        center={center}
        onChange={onVolume ? change : undefined}
      />
      <div className="flyout-bus-text">
        <span className="flyout-bus-label">{label}</span>
        <span className={`flyout-bus-sub${warn ? " warn" : ""}`} title={sub}>
          {sub}
        </span>
      </div>
      {onMute && (
        <button type="button" className="strip-mute" aria-pressed={muted} onClick={onMute}>
          {muted ? "Muted" : "Mute"}
        </button>
      )}
    </div>
  );
}

function AppRow({
  name,
  icon,
  value,
  level,
  muted,
  onVolume,
  onMute,
}: {
  name: string;
  icon: ReactNode;
  value: number;
  level: number;
  muted: boolean;
  onVolume: (volume: number) => void;
  onMute: () => void;
}) {
  const [shown, change] = useLiveValue(value, onVolume);
  return (
    <div className="flyout-app">
      <span className="flyout-app-icon">{icon}</span>
      <div className="flyout-app-main">
        <span className="flyout-app-name" title={name}>
          {name}
        </span>
        <Slider value={shown} level={muted ? 0 : level} muted={muted} label={`${name} volume`} onChange={change} />
      </div>
      <button
        type="button"
        className={`flyout-mute${muted ? " is-on" : ""}`}
        aria-pressed={muted}
        title={muted ? `Unmute ${name}` : `Mute ${name}`}
        onClick={onMute}
      >
        {muted ? <VolumeX /> : <Volume2 />}
      </button>
    </div>
  );
}
