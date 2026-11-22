// Dev-only fake of the Tauri IPC so the UI can be designed and screenshotted in
// a plain browser: `npm run dev`, then open
//   http://localhost:1420/dev/preview.html?state=receiving#/audio
//   ?state=firstrun | disconnected | public | reconnecting | receiving | sending | idle
//     (default: receiving; public = disconnected on a network Windows treats as Public)
//   &ducked       voice-chat ducking active
//   &handsfree    default output is a Bluetooth headset in call mode
//   &request      a connection request is waiting
//   &controlling  the pointer is on the other PC
//   &update       the linked PC offers a newer Trovul (&updating: downloading it)
//   &oldpeer      the linked PC runs 0.7.1 (no stream strip or output pickers there)
//   &noscenes     no saved mixer scenes yet
//   &follow       follow the pointer on (&there: the pointer is on the other PC)
//   &apps         the sending PC sends chosen apps only (&win10: its Windows can't)
//   &news         the "What's new" dialog shown after an update
//   &window=flyout  the tray mixer instead of the main window (view it at 360 × 500)
//   &window=popups  the pop-up cards (view it at 360 wide)
//   #/page        any route: home, audio (the mixer), input, settings
// Nothing here ships: Vite only bundles index.html.

import { NEWS_VERSION, SEEN_KEY } from "../src/components/Discover";

type Args = Record<string, unknown>;

const params = new URLSearchParams(location.search);
const state = params.get("state") ?? "receiving";
const firstRun = state === "firstrun";
const publicNetwork = state === "public";
const reconnecting = state === "reconnecting";
const connected = state !== "disconnected" && !publicNetwork && !firstRun && !reconnecting;
const direction = state === "sending" ? "send" : state === "receiving" ? "receive" : "off";
const now = Date.now();

// A gently moving level so meters and the flow animation look alive.
const level = (phase = 0) => 0.22 + 0.2 * Math.abs(Math.sin(Date.now() / 280 + phase));

const appInfo = { hostname: "GAMING-RIG", computer_name: "DESKTOP-4F7Q2KD", local_ip: "192.168.1.10", version: "0.6.0" };
const connection = connected
  ? { connected: true, is_initiator: true, peer_hostname: "STREAM-PC", peer_ip: "192.168.1.20", peer_device_id: "b7f1" }
  : { connected: false, is_initiator: false, peer_hostname: null, peer_ip: null, peer_device_id: null };

const stats = () =>
  direction === "off"
    ? { mode: "off", state: "idle", level: 0 }
    : {
        mode: direction === "send" ? "sender" : "host",
        state: direction === "send" ? "sending" : "playing",
        last_error: null,
        device: direction === "send" ? "Speakers (Realtek(R) Audio)" : "Headset (Arctis 7)",
        kbps: 1571,
        ms_since_last_packet: 3,
        packets_sent: 184_233,
        send_errors: 0,
        capture_rate: 48_000,
        capture_channels: 2,
        packets_received: 184_120,
        packets_lost: 7,
        packets_late: 0,
        packets_rejected: 0,
        underruns: 0,
        skips: 0,
        buffer_ms: 47.6,
        target_ms: 45,
        drift_ppm: 38,
        input_rate: 48_000,
        output_rate: 48_000,
        output_channels: 2,
        level: level(),
        ducked: params.has("ducked"),
      };

const settings = {
  pc_name: "GAMING-RIG",
  start_minimized: true,
  auto_reconnect: true,
  latency_mode: "balanced",
  remote_volume: 0.8,
  remote_muted: false,
  notifications: true,
  hotkeys: true,
  voice_ducking: true,
  follow_pointer: params.has("follow"),
  follow_depth: 0.3,
  shortcuts: [
    { action: "toggle_mute", keys: "Ctrl+Alt+M", registered: true },
    { action: "toggle_audio", keys: "Ctrl+Alt+T", registered: true },
  ],
  autostart: true,
  last_peer: firstRun
    ? null
    : { device_id: "b7f1", hostname: "STREAM-PC", ip: "192.168.1.20", direction: "receive", resume: reconnecting || (!connected && !publicNetwork && state !== "disconnected") },
};

const peers = [
  { id: "b7f1", hostname: "STREAM-PC", ip: "192.168.1.20", last_seen: now, version: "0.6.0", compatible: true, paired: true },
  { id: "c9d2", hostname: "OFFICE-LAPTOP", ip: "192.168.1.35", last_seen: now, version: "0.6.0", compatible: true, paired: false },
  { id: "e4a0", hostname: "LIVING-ROOM", ip: "192.168.1.40", last_seen: now, version: "0.5.0", compatible: false, paired: false },
];

const oldPeer = params.has("oldpeer");
// The "What's new" dialog only with &news.
localStorage.setItem(SEEN_KEY, params.has("news") ? "0.0.0" : NEWS_VERSION);
// Mixer state the mock keeps, so dials, mutes and output pickers respond.
const apps: Record<number, { volume: number; muted: boolean; output: string | null }> = {};
const app = (pid: number, name: string, volume: number, peak: number, output: string | null = null, muted = false) => {
  const s = (apps[pid] ??= { volume, muted, output });
  return { id: `session_${pid}`, pid, name, volume: s.volume, muted: s.muted, peak_level: s.muted ? 0 : peak, icon_id: null, output: s.output };
};
const masters = { local: { volume: 0.72, muted: false }, remote: { volume: 0.64, muted: false } };
const remoteIncoming = { remote_volume: 0.9, remote_muted: false };
let playbackDevice: string | null = null;
// Trovul's stream strip as each PC reports it.
const streamOf = (side: "local" | "remote") => {
  if (!connected || direction === "off") return null;
  const receiving = (direction === "receive") === (side === "local");
  const incoming = side === "local" ? settings : remoteIncoming;
  return {
    focus: settings.follow_pointer && direction === "receive" ? (side === "local") === !params.has("there") : null,
    receiving,
    volume: incoming.remote_volume,
    muted: incoming.remote_muted,
    level: incoming.remote_muted ? 0 : level(7),
    ducked: receiving && side === "local" && params.has("ducked"),
    output: side === "local" ? playbackDevice : null,
  };
};

// What each PC sends when it sends chosen apps (&apps), and whether its Windows can (&win10: no).
const sendState = {
  local: { apps_only: params.has("apps"), chosen: new Map<string, number>([["Discord", 1], ["RocketLeague", 0.7], ["OBS", 1]]) },
  remote: { apps_only: params.has("apps"), chosen: new Map<string, number>([["Valorant", 0.8], ["Discord", 1]]) },
};
const sendMixOf = (side: "local" | "remote") => ({
  supported: !params.has("win10"),
  apps_only: sendState[side].apps_only,
  chosen: [...sendState[side].chosen.keys()],
});
const withSend = (side: "local" | "remote", sessions: ReturnType<typeof app>[]) =>
  sessions.map((s) => {
    const level = [...sendState[side].chosen].find(([n]) => n.toLowerCase() === s.name.toLowerCase())?.[1];
    return { ...s, send: { on: level !== undefined, level: level ?? 1 } };
  });

const localMixer = () => ({
  master: { id: "m1", name: "Headset (Arctis 7)", ...masters.local, peak: masters.local.muted ? 0 : level() },
  sessions: withSend("local", [
    app(101, "Discord", 0.9, level(1) * 0.6, "o2"),
    app(102, "Spotify", 0.55, level(2)),
    app(103, "chrome", 1, 0.04),
    app(104, "RocketLeague", 0.8, level(3)),
    app(105, "obs64", 0.35, 0.02, null, true),
  ]),
  per_app_output: true,
  stream: streamOf("local"),
  send_mix: sendMixOf("local"),
});

const remoteMixer = () => ({
  master: { id: "m2", name: "Speakers (Realtek(R) Audio)", ...masters.remote, peak: masters.remote.muted ? 0 : level(4) },
  sessions: withSend("remote", [app(201, "obs64", 1, 0.08), app(202, "Valorant", 0.8, level(5)), app(203, "Spotify", 0.4, level(6) * 0.5)]),
  // A 0.7.1 peer reports neither.
  ...(oldPeer ? {} : { per_app_output: true, stream: streamOf("remote"), send_mix: sendMixOf("remote") }),
});

const outputs = [
  { id: "o1", name: "Headset (Arctis 7)", device_type: "output", volume: 0.72, muted: false, is_default: true },
  { id: "o2", name: "Speakers (Realtek(R) Audio)", device_type: "output", volume: 0.5, muted: false, is_default: false },
  { id: "o3", name: "LG ULTRAGEAR (NVIDIA High Definition Audio)", device_type: "output", volume: 1, muted: true, is_default: false },
];
if (params.has("handsfree")) {
  outputs.unshift({ id: "o4", name: "Headset (WH-1000XM4 Hands-Free AG Audio)", device_type: "output", volume: 1, muted: false, is_default: true });
  outputs[1].is_default = false;
}
const inputs = [{ id: "i1", name: "Microphone (Arctis 7)", device_type: "input", volume: 0.85, muted: false, is_default: true }];

const paired = [{ device_id: "b7f1", hostname: "STREAM-PC", ip: "192.168.1.20", first_seen: now - 12 * 86_400_000, last_seen: now - 180_000 }];
const companion = { enabled: false, running: false, ws_port: 5534, discovery_port: 5533, connected_clients: 0, token: null, last_error: null };

// Keyboard & mouse: this PC has two screens, STREAM-PC's screen sits between them.
const screens = [
  { pc: "me", local: true, id: "A", number: 1, name: "ASUS VG27AQ", primary: true, x: 0, y: 0, width: 2560, height: 1440 },
  { pc: "b7f1", local: false, id: "C", number: 1, name: "LG FULL HD", primary: true, x: 2560, y: 180, width: 1920, height: 1080 },
  { pc: "me", local: true, id: "B", number: 2, name: "DELL P2419H", primary: false, x: 4480, y: 180, width: 1920, height: 1080 },
];
const inputOptions = { focus_follows: true, game_mode: true, guard_corners: true, admin_input: "off" as string };
const inputView = () => ({
  enabled: connected,
  ...inputOptions,
  status: connected ? (params.has("controlling") ? "controlling" : "ready") : "waiting",
  this_pc: "me",
  peer_name: connected ? "STREAM-PC" : null,
  screens: connected ? screens : screens.filter((s) => s.local),
  reachable: true,
});

const sceneOf = (name: string, discord: number, game: number, music: number) => ({
  id: name.toLowerCase(),
  name,
  this_pc: {
    master: { volume: 0.72, muted: false },
    apps: [
      { name: "Discord", volume: discord, muted: false, output: "o2" },
      { name: "RocketLeague", volume: game, muted: false, output: null },
      { name: "Spotify", volume: music, muted: music === 0, output: null },
    ],
  },
  other_pc: { master: { volume: 0.64, muted: false }, apps: [{ name: "Spotify", volume: music, muted: false, output: null }] },
  incoming: { volume: 0.8, muted: false },
});
let scenes = params.has("noscenes")
  ? []
  : [sceneOf("Game", 0.9, 1, 0.2), sceneOf("Stream", 0.7, 0.8, 0.35), sceneOf("Meeting", 1, 0.3, 0)];

const clipboard = {
  enabled: true,
  last: connected ? { sent: false, kind: "image", detail: "1920 × 1080", at: now - 95_000 } : null,
};

function setSend(side: "local" | "remote", args: Args) {
  const chosen = sendState[side].chosen;
  const name = args.name as string;
  if (args.on) chosen.set(name, (args.level as number | null) ?? chosen.get(name) ?? 1);
  else chosen.delete(name);
}

let nextId = 1;
// Event callbacks registered by listen(), so the preview can push events.
const callbacks: Record<number, (event: unknown) => void> = {};
const listeners: Record<string, (event: unknown) => void> = {};
const emit = (event: string, payload: unknown) => listeners[event]?.({ event, id: 0, payload });

const handlers: Record<string, (args: Args) => unknown> = {
  get_app_info: () => appInfo,
  get_window_visible: () => true,
  get_connection_info: () => connection,
  get_discovered_peers: () => (firstRun || publicNetwork || reconnecting ? [] : peers),
  get_network_profile: () => ({ name: "Home Wi-Fi", category: publicNetwork ? "public" : "private", wifi: true }),
  open_network_settings: () => null,
  get_update_state: () => ({
    current: "0.7.0",
    peer_version: connected ? (params.has("update") || params.has("updating") ? "0.7.1" : "0.7.0") : null,
    available: params.has("update") || params.has("updating") ? { version: "0.7.1", from: "STREAM-PC", size: 5_275_648 } : null,
    stage: params.has("updating") ? "downloading" : "idle",
    progress: 0.62,
    error: null,
  }),
  dismiss_update: () => null,
  install_update: () => null,
  get_input_view: inputView,
  get_clipboard_view: () => clipboard,
  set_send_mode_cmd: (args) => void (sendState.local.apps_only = args.appsOnly as boolean),
  set_remote_send_mode: (args) => void (sendState.remote.apps_only = args.appsOnly as boolean),
  set_app_send_cmd: (args) => setSend("local", args),
  set_remote_app_send: (args) => setSend("remote", args),
  get_popups: () => [
    { id: 1, tone: "info", icon: "files", title: "STREAM-PC copied 3 files · 2.1 GB", text: "Receive them here? They go to Downloads\\Trovul, ready to paste.", progress: null, actions: [{ id: "dismiss", label: "Not now", primary: false }, { id: "receive", label: "Receive", primary: true }], sticky: true },
    { id: 2, tone: "info", icon: "files", title: "Receiving a folder · 840 MB from STREAM-PC", text: "512.0 MB of 840.0 MB", progress: 0.61, actions: [{ id: "cancel", label: "Cancel", primary: false }], sticky: true },
    { id: 3, tone: "info", icon: "image", title: "Image from STREAM-PC", text: "1920 × 1080 · ready to paste", progress: null, actions: [], sticky: true },
    { id: 4, tone: "success", icon: "link", title: "Linked to STREAM-PC", text: "Sound, keyboard and mouse are ready.", progress: null, actions: [], sticky: true },
  ],
  resize_popups: () => null,
  hide_popups: () => null,
  popup_action: () => null,
  get_mix_scenes: () => scenes,
  capture_scene: (args) => ({ ...sceneOf(args.name as string, 0.8, 0.8, 0.5), id: (args.id as string) ?? "s" + Date.now() }),
  apply_scene: (args) => `“${scenes.find((s) => s.id === args.id)?.name ?? "Scene"}” applied to 4 apps`,
  set_mix_scenes: (args) => {
    scenes = args.scenes as typeof scenes;
    emit("scenes-changed", scenes);
    return scenes;
  },
  set_clipboard_sharing: (args) => Object.assign(clipboard, { enabled: args.enabled }),
  refresh_input: inputView,
  set_input_sharing: inputView,
  set_admin_input: (args) => {
    inputOptions.admin_input = args.enabled ? "ready" : "off";
    return inputView();
  },
  set_input_options: (args) => {
    if (args.focusFollows !== null) inputOptions.focus_follows = args.focusFollows as boolean;
    if (args.gameMode !== null) inputOptions.game_mode = args.gameMode as boolean;
    if (args.guardCorners !== null) inputOptions.guard_corners = args.guardCorners as boolean;
    return inputView();
  },
  reset_input_layout: inputView,
  set_input_layout: (args) => {
    for (const placement of args.placements as { monitor: string; x: number; y: number }[]) {
      const screen = screens.find((s) => s.id === placement.monitor);
      if (screen) Object.assign(screen, { x: placement.x, y: placement.y });
    }
    return inputView();
  },
  get_stream_direction: () => direction,
  set_stream_direction: (args) => args.direction,
  play_test_sound: () => null,
  support_info: () => "Trovul 0.6.0 (protocol 2.0)\nWindows: UI preview",
  get_audio_stats: stats,
  get_remote_audio_stats: stats,
  get_settings: () => settings,
  update_settings: (args) => Object.assign(settings, args.patch as object),
  set_autostart: (args) => args.enabled,
  get_paired_devices: () => paired,
  get_companion_status: () => companion,
  get_local_mixer: localMixer,
  get_remote_mixer: remoteMixer,
  get_icons: () => ({}),
  get_remote_icons: () => ({}),
  get_output_devices: () => outputs,
  get_input_devices: () => inputs,
  get_playback_device: () => playbackDevice,
  set_playback_device: (args) => {
    playbackDevice = args.deviceId as string | null;
  },
  set_session_volume: (args) => void (apps[args.pid as number].volume = args.volume as number),
  set_session_mute: (args) => void (apps[args.pid as number].muted = args.muted as boolean),
  set_app_output: (args) => void (apps[args.pid as number].output = args.deviceId as string | null),
  set_remote_session_volume: (args) => void (apps[args.pid as number].volume = args.volume as number),
  set_remote_session_mute: (args) => void (apps[args.pid as number].muted = args.muted as boolean),
  set_remote_app_output: (args) => void (apps[args.pid as number].output = args.deviceId as string | null),
  set_master_volume: (args) => void (masters.local.volume = args.volume as number),
  set_master_mute: (args) => void (masters.local.muted = args.muted as boolean),
  set_remote_master_volume: (args) => void (masters.remote.volume = args.volume as number),
  set_remote_master_mute: (args) => void (masters.remote.muted = args.muted as boolean),
  get_remote_output_devices: () => {
    if (oldPeer) throw "Unsupported request — update Trovul on both PCs";
    return outputs.filter((d) => d.id !== "o1");
  },
  set_remote_incoming: (args) => {
    if (args.volume !== null) remoteIncoming.remote_volume = args.volume as number;
    if (args.muted !== null) remoteIncoming.remote_muted = args.muted as boolean;
  },
  set_remote_playback_device: () => null,
  "plugin:event|listen": (args) => {
    const callback = callbacks[args.handler as number];
    if (callback) listeners[args.event as string] = callback;
    return nextId++;
  },
  "plugin:notification|is_permission_granted": () => true,
};

const windowLabel = params.get("window") ?? "main";
Object.assign(window, {
  __TAURI_INTERNALS__: {
    metadata: { currentWindow: { label: windowLabel }, currentWebview: { windowLabel, label: windowLabel } },
    invoke: async (cmd: string, args: Args = {}) => (cmd in handlers ? handlers[cmd](args) : null),
    transformCallback: (callback: (event: unknown) => void) => {
      const id = nextId++;
      callbacks[id] = callback;
      return id;
    },
    unregisterCallback: () => {},
    convertFileSrc: (src: string) => src,
  },
  __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} },
});

// Recent activity, as the backend would have pushed it.
if (connected) {
  window.setTimeout(() => {
    emit("connection-changed", { kind: "established", info: connection, peer_hostname: "STREAM-PC", reason: null, intentional: false });
  }, 300);
}

// A pending pairing prompt, as the backend would push it.
if (params.has("request")) {
  window.setTimeout(() => emit("connection-request", { hostname: "OFFICE-LAPTOP", ip: "192.168.1.35" }), 1500);
}
