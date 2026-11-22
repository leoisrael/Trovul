import { useEffect, useState } from "react";
import { Copy, FolderOpen, KeyRound } from "lucide-react";
import { useApp, type LogEntry } from "../app/AppContext";
import { api, errorMessage } from "../api";
import { PcNameField } from "../components/Pcs";
import { usePolling } from "../hooks";
import type { CompanionStatus, LatencyMode, PairedDevice } from "../types";
import { timeAgo } from "../util";
import { Badge, Block, Button, Dot, Group, PageHeader, Row, Section, Segmented, Spinner, Toggle } from "../ui";

export function SettingsPage() {
  const { settings } = useApp();
  return (
    <div className="page page-narrow">
      <PageHeader title="Settings" />
      {!settings ? (
        <Spinner large />
      ) : (
        <>
          <ThisPc />
          <Connection />
          <AudioSettings />
          <Shortcuts />
          <Companion />
          <About />
        </>
      )}
    </div>
  );
}

function ThisPc() {
  const { settings, updateSettings, setAutostart } = useApp();
  if (!settings) return null;
  return (
    <Section title="This PC">
      <Group>
        <Row title="Name" detail="How the other PC sees this one">
          <PcNameField />
        </Row>
        <Row title="Start with Windows">
          <Toggle label="Start with Windows" checked={settings.autostart} onChange={setAutostart} />
        </Row>
        <Row title="Stay in the tray at startup" detail="Don't open the window when Windows starts Trovul">
          <Toggle label="Stay in the tray at startup" checked={settings.start_minimized} onChange={(v) => updateSettings({ start_minimized: v })} />
        </Row>
        <Row title="Pop-ups" detail="In the corner of the screen: a PC linked or lost, an image arrived, an update. Transfers and questions always show.">
          <Toggle label="Pop-ups" checked={settings.notifications} onChange={(v) => updateSettings({ notifications: v })} />
        </Row>
      </Group>
    </Section>
  );
}

function Connection() {
  const { settings, updateSettings, visible, report, connection } = useApp();
  const [devices, setDevices] = useState<PairedDevice[]>([]);

  usePolling(async () => {
    setDevices(await api.pairedDevices());
  }, 5000, visible);

  const forget = async (device: PairedDevice) => {
    try {
      await api.forgetPairedDevice(device.device_id);
      setDevices((list) => list.filter((d) => d.device_id !== device.device_id));
    } catch (e) {
      report(errorMessage(e));
    }
  };

  if (!settings) return null;
  return (
    <>
      <Section title="Connection">
        <Group>
          <Row title="Reconnect automatically" detail="After restarts, sleep or network drops, with the sound restored">
            <Toggle label="Reconnect automatically" checked={settings.auto_reconnect} onChange={(v) => updateSettings({ auto_reconnect: v })} />
          </Row>
        </Group>
      </Section>
      <Section title="Paired PCs" footer="Paired PCs connect without asking. Forget one to require approval again.">
        <Group>
          {devices.length === 0 && <Row title="No paired PCs yet" />}
          {devices.map((device) => (
            <Row
              key={device.device_id}
              title={
                <>
                  {device.hostname}
                  {connection.peer_device_id === device.device_id && <Badge tone="success">Connected</Badge>}
                </>
              }
              detail={`Last connected ${timeAgo(device.last_seen)}`}
            >
              <Button variant="ghost" size="sm" onClick={() => forget(device)}>
                Forget
              </Button>
            </Row>
          ))}
        </Group>
      </Section>
    </>
  );
}

const LATENCY: { value: LatencyMode; label: string; detail: string }[] = [
  { value: "low", label: "Low", detail: "About 30 ms. For wired networks and strong Wi-Fi." },
  { value: "balanced", label: "Balanced", detail: "About 50 ms. Works well on most networks." },
  { value: "stable", label: "Stable", detail: "About 120 ms. Fewest dropouts on busy Wi-Fi." },
];

function AudioSettings() {
  const { settings, updateSettings } = useApp();
  if (!settings) return null;
  const current = LATENCY.find((l) => l.value === settings.latency_mode) ?? LATENCY[1];
  return (
    <Section title="Audio">
      <Group>
        <Row title="Latency" detail={current.detail}>
          <Segmented<LatencyMode>
            label="Latency"
            value={settings.latency_mode}
            options={LATENCY.map(({ value, label }) => ({ value, label }))}
            onChange={(latency_mode) => updateSettings({ latency_mode })}
          />
        </Row>
        <Row title="Lower the other PC during calls" detail="While Discord, Teams or Zoom on this PC makes sound">
          <Toggle label="Lower the other PC during calls" checked={settings.voice_ducking} onChange={(v) => updateSettings({ voice_ducking: v })} />
        </Row>
        <Row title="Follow the pointer" detail="In the headset, the PC you're using is louder. Needs keyboard & mouse sharing.">
          <Toggle label="Follow the pointer" checked={settings.follow_pointer} onChange={(v) => updateSettings({ follow_pointer: v })} />
        </Row>
        {settings.follow_pointer && (
          <Row title="The other PC meanwhile" detail="How loud the PC you're not using stays">
            <select
              className="select select-compact"
              aria-label="How loud the PC you're not using stays"
              value={String(settings.follow_depth)}
              onChange={(e) => updateSettings({ follow_depth: Number(e.target.value) })}
            >
              {[
                [0, "Silent"],
                [0.15, "15%"],
                [0.3, "30%"],
                [0.5, "50%"],
                [0.7, "70%"],
              ].map(([value, label]) => (
                <option key={value} value={String(value)}>
                  {label}
                </option>
              ))}
            </select>
          </Row>
        )}
      </Group>
    </Section>
  );
}

const SHORTCUT_LABELS: Record<string, string> = {
  toggle_mute: "Mute the other PC",
  toggle_audio: "Sound between the PCs on / off",
};

function Shortcuts() {
  const { settings, updateSettings } = useApp();
  if (!settings) return null;
  const shortcuts = settings.shortcuts.length
    ? settings.shortcuts
    : [
        { action: "toggle_mute" as const, keys: "Ctrl+Alt+M", registered: true },
        { action: "toggle_audio" as const, keys: "Ctrl+Alt+T", registered: true },
      ];
  return (
    <Section title="Keyboard shortcuts" footer="They work everywhere, even in full-screen games.">
      <Group>
        <Row title="Use keyboard shortcuts">
          <Toggle label="Use keyboard shortcuts" checked={settings.hotkeys} onChange={(hotkeys) => updateSettings({ hotkeys })} />
        </Row>
        {settings.hotkeys &&
          shortcuts.map((s) => (
            <Row
              key={s.action}
              title={SHORTCUT_LABELS[s.action] ?? s.action}
              detail={!s.registered ? "Another app already uses this combination" : undefined}
            >
              <span className="keys">
                {s.keys.split("+").map((key) => (
                  <kbd key={key}>{key}</kbd>
                ))}
              </span>
            </Row>
          ))}
      </Group>
    </Section>
  );
}

function Companion() {
  const { visible, appInfo, report, notify } = useApp();
  const [status, setStatus] = useState<CompanionStatus | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.companionStatus().then(setStatus).catch(() => {});
  }, []);

  usePolling(async () => {
    setStatus(await api.companionStatus());
  }, 3000, visible && !!status?.enabled);

  const run = async (action: () => Promise<CompanionStatus>) => {
    setBusy(true);
    try {
      setStatus(await action());
    } catch (e) {
      report(`Companion API: ${errorMessage(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const url = status?.token ? `ws://${appInfo?.local_ip ?? "<this-pc>"}:${status.ws_port}/?token=${status.token}` : "";

  return (
    <Section
      title="Stream Deck & controllers"
      footer="The Trovul plugin for Stream Deck connects by itself while this is on: scenes, where the sound goes, mute and volume dials for both PCs."
    >
      <Group>
        <Row
          title="Allow controllers"
          detail={
            status?.enabled
              ? `${status.running ? "Listening" : "Not listening"} · ${status.connected_clients} connected`
              : "Stream Deck on this PC, phones and other controllers on this network"
          }
        >
          {status && (
            <Toggle label="Companion API" checked={status.enabled} disabled={busy} onChange={(enabled) => run(() => api.setCompanionEnabled(enabled))} />
          )}
        </Row>
        {status?.enabled && (
          <Block>
            <div className="field-row">
              <input className="input mono" readOnly value={url} onFocus={(e) => e.currentTarget.select()} aria-label="Companion URL" />
              <Button
                icon={Copy}
                onClick={() =>
                  navigator.clipboard
                    .writeText(url)
                    .then(() => notify("Companion URL copied.", "success"))
                    .catch(() => report("Could not copy — select the text instead"))
                }
              >
                Copy
              </Button>
              <Button icon={KeyRound} disabled={busy} onClick={() => run(api.regenerateCompanionToken)} title="Make a new token">
                New token
              </Button>
            </div>
          </Block>
        )}
        {status?.last_error && <Row title="Error" detail={status.last_error} tone="danger" />}
      </Group>
    </Section>
  );
}

const TONE_DOTS = { info: "neutral", success: "success", warning: "warning", error: "danger" } as const;

function About() {
  const { appInfo, report, notify, update, peerName, link, logs } = useApp();
  const [copying, setCopying] = useState(false);
  const [showLog, setShowLog] = useState(false);

  const copySupportInfo = async () => {
    setCopying(true);
    try {
      await navigator.clipboard.writeText(await api.supportInfo());
      notify("Support info copied — paste it into your support request.", "success");
    } catch (e) {
      report(`Support info: ${errorMessage(e)}`);
    } finally {
      setCopying(false);
    }
  };

  const versionDetail =
    link === "linked" && update?.peer_version
      ? update.peer_version === update.current
        ? `${peerName} runs the same version`
        : `${peerName} runs ${update.peer_version}`
      : "Updates pass from one connected PC to the other";

  return (
    <Section title="About">
      <Group>
        <Row title={`Trovul ${appInfo?.version ?? ""}`} detail={versionDetail} />
        <Row title="Support info" detail="Versions, connection, audio state and the recent log. Never tokens.">
          <Button size="sm" icon={Copy} loading={copying} onClick={copySupportInfo}>
            Copy
          </Button>
        </Row>
        <Row title="Logs">
          <Button size="sm" icon={FolderOpen} onClick={() => api.openLogFolder().catch((e) => report(errorMessage(e)))}>
            Open folder
          </Button>
        </Row>
        <Row title="Activity" detail={`${logs.length} event${logs.length === 1 ? "" : "s"} since Trovul started`}>
          <button className="link-action" onClick={() => setShowLog(!showLog)} aria-expanded={showLog}>
            {showLog ? "Hide" : "Show"}
          </button>
        </Row>
        {showLog && (
          <Block>
            <ActivityList logs={logs} />
          </Block>
        )}
      </Group>
    </Section>
  );
}

function ActivityList({ logs }: { logs: LogEntry[] }) {
  if (logs.length === 0) return <span className="hint">Nothing yet.</span>;
  return (
    <ol className="activity">
      {logs.map((entry) => (
        <li key={entry.id} className="activity-item">
          <Dot tone={TONE_DOTS[entry.tone]} />
          <span className="activity-text">{entry.text}</span>
          <time className="activity-time mono">
            {new Date(entry.time).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" })}
          </time>
        </li>
      ))}
    </ol>
  );
}
