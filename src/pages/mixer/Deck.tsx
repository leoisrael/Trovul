// One PC's side of the mixer, laid out like an audio interface: bus strips
// (the PC's master and Trovul's stream: the return from the other PC, or the
// send to it), then one strip per app. Every strip: scribble strip, dial with
// its live level, mute and, where Windows allows, the output it plays on.

import { useEffect, useState, type CSSProperties, type ReactNode } from "react";
import { ArrowRight, CirclePause, Headphones, Send, Volume2 } from "lucide-react";
import { useApp } from "../../app/AppContext";
import { errorMessage } from "../../api";
import { useLiveValue, usePolling, useIconCache } from "../../hooks";
import type { AudioDevice, AudioProcess, MixerSnapshot, SendMix } from "../../types";
import { Dial, Slider, Spinner, dialColors } from "../../ui";

export interface DeckActions {
  setMasterVolume: (volume: number) => Promise<void>;
  setMasterMute: (muted: boolean) => Promise<void>;
  setAppVolume: (pid: number, volume: number) => Promise<void>;
  setAppMute: (pid: number, muted: boolean) => Promise<void>;
  setAppOutput: (pid: number, deviceId: string | null) => Promise<void>;
  setStreamVolume: (volume: number) => Promise<void>;
  setStreamMute: (muted: boolean) => Promise<void>;
  setStreamOutput: (deviceId: string | null) => Promise<void>;
  /** What goes to the other PC: everything, or only the chosen apps. */
  setSendMode: (appsOnly: boolean) => Promise<void>;
  setAppSend: (name: string, on: boolean, level: number | null) => Promise<void>;
}

type Update = (change: (prev: MixerSnapshot | null) => MixerSnapshot | null) => void;

/** A PC's mixer, polled while `active`; `scope` (e.g. the peer's address) starts afresh. */
export function useMixer(
  fetch: () => Promise<MixerSnapshot>,
  fetchIcons: (ids: string[]) => Promise<Record<string, string>>,
  active: boolean,
  scope?: unknown,
) {
  const [mixer, setMixer] = useState<MixerSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const icons = useIconCache(fetchIcons, scope);

  useEffect(() => {
    setMixer(null);
    setError(null);
  }, [scope]);

  usePolling(async () => {
    try {
      const next = await fetch();
      setMixer(next);
      setError(null);
      icons.ensure(next.sessions.map((s) => s.icon_id));
    } catch (e) {
      setError(errorMessage(e));
    }
  }, 100, active);

  return { mixer, error, icons: icons.icons, update: setMixer as Update };
}

/** A PC's output devices, refreshed now and then. */
export function useOutputs(fetch: () => Promise<AudioDevice[]>, active: boolean, scope?: unknown) {
  const [outputs, setOutputs] = useState<AudioDevice[]>([]);
  useEffect(() => setOutputs([]), [scope]);
  usePolling(async () => {
    try {
      setOutputs(await fetch());
    } catch {
      // An older Trovul on the other PC can't list them: no output pickers there.
    }
  }, 4000, active);
  return outputs;
}

export const STRIP_COLORS = {
  master: dialColors("#7c3aed", "#c4b5fd"),
  stream: dialColors("#06b6d4", "#34d399"),
  send: dialColors("#10b981", "#6ee7b7"),
  app: dialColors("#8b5cf6", "#22d3ee"),
};

export function Deck({
  name,
  tag,
  action,
  other,
  linked,
  mixer,
  error,
  icons,
  outputs,
  actions,
  update,
  emptyText,
}: {
  name: string;
  tag?: ReactNode;
  action?: ReactNode;
  /** The PC at the other end of the stream, as this deck's PC calls it. */
  other: string;
  linked: boolean;
  mixer: MixerSnapshot | null;
  error: string | null;
  icons: Record<string, string>;
  outputs: AudioDevice[];
  actions: DeckActions;
  update: Update;
  emptyText: string;
}) {
  const { report } = useApp();
  const fail = (what: string) => (e: unknown) => report(`${what}: ${errorMessage(e)}`);

  const head = (
    <header className="deck-head">
      <div className="deck-title">
        <h2>{name}</h2>
        {tag}
      </div>
      {action}
    </header>
  );

  if (!mixer) {
    return (
      <section className="deck card" aria-label={`${name} mixer`}>
        {head}
        <div className="deck-loading">{error ? <span className="hint">{error}</span> : <Spinner />}</div>
      </section>
    );
  }

  const { master, sessions, stream } = mixer;
  const pickers = !!mixer.per_app_output && outputs.length > 0;
  const setApp = (pid: number, change: Partial<AudioProcess>) =>
    update((prev) => prev && { ...prev, sessions: prev.sessions.map((s) => (s.pid === pid ? { ...s, ...change } : s)) });
  const setStream = (change: Partial<NonNullable<MixerSnapshot["stream"]>>) =>
    update((prev) => (prev?.stream ? { ...prev, stream: { ...prev.stream, ...change } } : prev));

  // Sending, and only the chosen apps: each app strip gets its "➜" switch.
  const sendMix = mixer.send_mix ?? null;
  const sending = linked && !!stream && !stream.receiving;
  const routing = sending && !!sendMix?.supported && sendMix.apps_only;
  const setSendMix = (change: Partial<SendMix>) =>
    update((prev) => (prev?.send_mix ? { ...prev, send_mix: { ...prev.send_mix, ...change } } : prev));
  const chooseMode = (appsOnly: boolean) => {
    actions.setSendMode(appsOnly).catch(fail("Send"));
    setSendMix({ apps_only: appsOnly });
  };
  const toggleSend = (app: AudioProcess) => {
    const on = !app.send?.on;
    actions.setAppSend(app.name, on, null).catch(fail(app.name));
    setApp(app.pid, { send: { on, level: app.send?.level ?? 1 } });
    const chosen = sendMix?.chosen ?? [];
    setSendMix({
      chosen: on ? [...chosen, app.name] : chosen.filter((n) => n.toLowerCase() !== app.name.toLowerCase()),
    });
  };

  return (
    <section className="deck card" aria-label={`${name} mixer`}>
      {head}
      {routing && sendMix && <SendGuide mix={sendMix} sessions={sessions} pc={name} to={other} />}
      <div className="deck-board">
        <div className="deck-buses">
          {master && (
            <Strip
              bus
              label="Master"
              icon={<Volume2 />}
              colors={STRIP_COLORS.master}
              value={master.volume}
              level={master.peak}
              muted={master.muted}
              onVolume={(v) => actions.setMasterVolume(v).catch(fail("Master volume"))}
              onMute={() => {
                actions.setMasterMute(!master.muted).catch(fail("Master mute"));
                update((prev) => (prev?.master ? { ...prev, master: { ...prev.master, muted: !master.muted } } : prev));
              }}
              footer={
                <span className="strip-device" title={master.name}>
                  {master.name}
                </span>
              }
            />
          )}
          {/* An older Trovul on the other PC doesn't report its stream (undefined). */}
          {linked && stream === null && (
            <Strip bus off label="Return" sub={`from ${other}`} icon={<CirclePause />} colors={STRIP_COLORS.stream} value={0} center="Off" />
          )}
          {linked && stream && !stream.receiving && (
            <Strip
              bus
              label="Send"
              sub={
                routing
                  ? `${sendMix?.chosen.length ?? 0} app${sendMix?.chosen.length === 1 ? "" : "s"} chosen`
                  : `to ${other}`
              }
              icon={<Send />}
              colors={STRIP_COLORS.send}
              value={1}
              level={stream.level}
              center={<Send />}
              footer={<SendModeSwitch mix={sendMix} onChange={chooseMode} />}
            />
          )}
          {linked && stream?.receiving && (
            <Strip
              bus
              label="Return"
              sub={`from ${other}`}
              icon={<Headphones />}
              colors={STRIP_COLORS.stream}
              value={stream.volume}
              level={stream.level}
              muted={stream.muted}
              warn={stream.ducked && !stream.muted ? "Lowered for a call" : undefined}
              onVolume={(v) => actions.setStreamVolume(v).catch(fail(`${other} volume`))}
              onMute={() => {
                actions.setStreamMute(!stream.muted).catch(fail(`${other} mute`));
                setStream({ muted: !stream.muted });
              }}
              footer={
                outputs.length > 0 && (
                  <OutputSelect
                    outputs={outputs}
                    value={stream.output}
                    label={`Output for ${other}`}
                    onChange={(id) => {
                      actions.setStreamOutput(id).catch(fail("Output device"));
                      setStream({ output: id });
                    }}
                  />
                )
              }
            />
          )}
        </div>
        <div className="deck-apps">
          {sessions.map((app) => (
            <Strip
              key={app.id}
              label={app.name}
              icon={<AppIcon name={app.name} src={app.icon_id ? icons[app.icon_id] : undefined} />}
              colors={STRIP_COLORS.app}
              value={app.volume}
              level={app.peak_level}
              muted={app.muted}
              onVolume={(v) => actions.setAppVolume(app.pid, v).catch(fail(app.name))}
              onMute={() => {
                actions.setAppMute(app.pid, !app.muted).catch(fail(app.name));
                setApp(app.pid, { muted: !app.muted });
              }}
              footer={
                pickers && (
                  <OutputSelect
                    outputs={outputs}
                    value={app.output ?? null}
                    label={`Output for ${app.name}`}
                    onChange={(id) => {
                      actions.setAppOutput(app.pid, id).catch(fail(`${app.name} output`));
                      setApp(app.pid, { output: id });
                    }}
                  />
                )
              }
              route={
                routing && (
                  <SendControl
                    to={other}
                    on={!!app.send?.on}
                    level={app.send?.level ?? 1}
                    onToggle={() => toggleSend(app)}
                    onLevel={(level) => {
                      actions.setAppSend(app.name, true, level).catch(fail(app.name));
                      setApp(app.pid, { send: { on: true, level } });
                    }}
                  />
                )
              }
            />
          ))}
          {sessions.length === 0 && <div className="deck-empty">{error ?? emptyText}</div>}
        </div>
      </div>
    </section>
  );
}

function Strip({
  label,
  sub,
  icon,
  colors,
  bus,
  off,
  value,
  level,
  muted = false,
  center,
  warn,
  footer,
  route,
  onVolume,
  onMute,
}: {
  label: string;
  sub?: string;
  icon: ReactNode;
  colors: CSSProperties;
  bus?: boolean;
  off?: boolean;
  value: number;
  level?: number;
  muted?: boolean;
  center?: ReactNode;
  /** Replaces `sub`, e.g. the sound is lowered for a call. */
  warn?: string;
  footer?: ReactNode;
  /** Where the app goes besides here (the "➜ other PC" switch). */
  route?: ReactNode;
  onVolume?: (volume: number) => void;
  onMute?: () => void;
}) {
  const [shown, change] = useLiveValue(value, onVolume ?? (() => {}));
  return (
    <div className={`strip${bus ? " bus" : ""}${muted ? " is-muted" : ""}${off ? " is-off" : ""}`} style={colors}>
      <div className="strip-scribble" title={[label, sub, warn].filter(Boolean).join(" · ")}>
        <span className="strip-icon">{icon}</span>
        <span className="strip-label">{label}</span>
        {warn ? <span className="strip-sub warn">{warn}</span> : sub && <span className="strip-sub">{sub}</span>}
      </div>
      <Dial
        value={shown}
        level={level}
        muted={muted}
        disabled={off}
        label={`${label}${sub ? ` ${sub}` : ""} volume`}
        center={center}
        onChange={onVolume ? change : undefined}
      />
      {onMute ? (
        <button type="button" className="strip-mute" aria-pressed={muted} onClick={onMute} title={muted ? `Unmute ${label}` : `Mute ${label}`}>
          {muted ? "Muted" : "Mute"}
        </button>
      ) : (
        <span className="strip-mute placeholder" aria-hidden="true" />
      )}
      <div className="strip-foot">{footer}</div>
      {route && <div className="strip-route">{route}</div>}
    </div>
  );
}

function OutputSelect({
  outputs,
  value,
  label,
  onChange,
}: {
  outputs: AudioDevice[];
  value: string | null;
  label: string;
  onChange: (deviceId: string | null) => void;
}) {
  const fallback = outputs.find((d) => d.is_default);
  const chosen = outputs.find((d) => d.id === value);
  return (
    <select
      className="select strip-output"
      value={value ?? ""}
      aria-label={label}
      title={chosen ? chosen.name : `Default${fallback ? `: ${fallback.name}` : ""}`}
      onChange={(e) => onChange(e.target.value || null)}
    >
      <option value="">Default</option>
      {outputs.map((d) => (
        <option key={d.id} value={d.id}>
          {d.name}
        </option>
      ))}
      {value !== null && !chosen && <option value={value}>Unavailable device</option>}
    </select>
  );
}

/** The app's own icon, or a coloured initial when Windows has none. */
export function AppIcon({ name, src }: { name: string; src?: string }) {
  if (src) return <img src={src} alt="" />;
  let hash = 0;
  for (const ch of name) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0;
  return (
    <span className="app-letter" style={{ "--hue": hash % 360 } as CSSProperties}>
      {name.charAt(0).toUpperCase()}
    </span>
  );
}

/** "Everything" or "Chosen apps", on the Send strip. */
function SendModeSwitch({ mix, onChange }: { mix: SendMix | null; onChange: (appsOnly: boolean) => void }) {
  if (!mix?.supported) {
    return (
      <span className="strip-device" title="Choosing which apps to send needs Windows 11 on this PC">
        All sound
      </span>
    );
  }
  return (
    <div className="send-mode" role="radiogroup" aria-label="What goes to the other PC">
      <button type="button" role="radio" aria-checked={!mix.apps_only} title="Everything this PC plays" onClick={() => mix.apps_only && onChange(false)}>
        All
      </button>
      <button type="button" role="radio" aria-checked={mix.apps_only} title="Only the apps you choose" onClick={() => !mix.apps_only && onChange(true)}>
        Apps
      </button>
    </div>
  );
}

/** On each app strip while sending chosen apps: goes there or not, and how loud. */
function SendControl({
  to,
  on,
  level,
  onToggle,
  onLevel,
}: {
  to: string;
  on: boolean;
  level: number;
  onToggle: () => void;
  onLevel: (level: number) => void;
}) {
  const [shown, change] = useLiveValue(level, onLevel);
  return (
    <div className={`send-ctl${on ? " on" : ""}`}>
      <button
        type="button"
        className="send-pill"
        aria-pressed={on}
        title={on ? `Sent to ${to}. Click to stop sending it.` : `Not sent. Click to send it to ${to}.`}
        onClick={onToggle}
      >
        <ArrowRight aria-hidden="true" />
        <span>{to}</span>
      </button>
      {on && (
        <div className="send-level">
          <span className="send-caption" title={`Level on ${to}`}>
            <span>on {to}</span>
            <strong>{Math.round(shown * 100)}%</strong>
          </span>
          <Slider value={shown} label={`Level on ${to}`} showValue={false} onChange={change} />
        </div>
      )}
    </div>
  );
}

/** One line at the top of the deck: what goes where, and what each control does. */
function SendGuide({ mix, sessions, pc, to }: { mix: SendMix; sessions: AudioProcess[]; pc: string; to: string }) {
  const playing = new Set(sessions.map((s) => s.name.toLowerCase()));
  return (
    <div className="send-guide">
      <ArrowRight aria-hidden="true" />
      {mix.chosen.length === 0 ? (
        <p>
          <strong>Choose what goes to {to}:</strong> turn on <em>➜ {to}</em> on each app below. Nothing is sent until you do.
        </p>
      ) : (
        <p>
          <strong>Going to {to}:</strong>{" "}
          {mix.chosen.map((name) => (
            <span key={name} className={`send-chip${playing.has(name.toLowerCase()) ? "" : " idle"}`} title={playing.has(name.toLowerCase()) ? "Playing" : "Not playing now"}>
              {name}
            </span>
          ))}
          <span className="send-note">
            Each dial is what plays on {pc}; the bar under ➜ is how loud it is on {to}.
          </span>
        </p>
      )}
    </div>
  );
}
