// Shapes of the data exchanged with the Rust backend (see src-tauri/src/lib.rs).

export type AudioMode = "off" | "sender" | "host";
/** Audio flow from this PC's point of view; setting it configures both PCs. */
export type Direction = "off" | "send" | "receive";
export type LatencyMode = "low" | "balanced" | "stable";

export interface AppInfo {
  /** The name other PCs see: `pc_name` from settings, else `computer_name`. */
  hostname: string;
  /** The Windows computer name. */
  computer_name: string;
  local_ip: string;
  version: string;
}

/** Windows network profile of this PC's LAN adapter (src-tauri/src/netprofile.rs). */
export interface NetworkProfile {
  name: string;
  category: "public" | "private" | "domain";
  wifi: boolean;
}

export interface Peer {
  id: string;
  hostname: string;
  ip: string;
  last_seen: number;
  version: string;
  /** False when it runs an incompatible Trovul version. */
  compatible: boolean;
  /** Paired before: connects without asking. */
  paired: boolean;
}

export interface ConnectionInfo {
  connected: boolean;
  /** This PC opened the connection. Says nothing about audio direction. */
  is_initiator: boolean;
  peer_hostname: string | null;
  peer_ip: string | null;
  peer_device_id: string | null;
}

export interface ConnectionEvent {
  kind: "established" | "closed";
  info: ConnectionInfo;
  peer_hostname: string;
  reason: string | null;
  /** Someone disconnected on purpose — no automatic reconnect follows. */
  intentional: boolean;
}

export interface ConnectionRequest {
  hostname: string;
  ip: string;
}

export interface ConnectResult {
  accepted: boolean;
  reason: string | null;
}

export interface AudioModeEvent {
  mode: AudioMode;
  direction: Direction;
  reason: string | null;
}

export interface AudioProcess {
  id: string;
  pid: number;
  name: string;
  volume: number;
  muted: boolean;
  peak_level: number;
  icon_id: string | null;
  /** Output device chosen for this app; null = the Windows default. Missing from peers before 0.7.2. */
  output?: string | null;
  /** In "chosen apps" mode: goes to the other PC, and how loud (Windows 11 senders). */
  send?: { on: boolean; level: number } | null;
}

/** What a PC sends to the other: everything, or the chosen apps. */
export interface SendMix {
  /** That PC's Windows can capture single apps (Windows 11). */
  supported: boolean;
  apps_only: boolean;
  /** The chosen apps, playing or not. */
  chosen: string[];
}

export interface MasterInfo {
  id: string;
  name: string;
  volume: number;
  muted: boolean;
  peak: number;
}

/** Trovul's own stream on a PC: the other PC's sound playing there, or that PC's sound going out. */
export interface StreamBus {
  /** Follow the pointer is acting: the pointer is on this PC (true) or the other. */
  focus?: boolean | null;
  receiving: boolean;
  /** Volume and mute of the sound received. */
  volume: number;
  muted: boolean;
  level: number;
  /** Lowered while someone talks in a call. */
  ducked: boolean;
  /** Device playing the sound received; null = the Windows default. */
  output: string | null;
}

export interface MixerSnapshot {
  master: MasterInfo | null;
  sessions: AudioProcess[];
  /** Apps can be sent to their own output device. */
  per_app_output?: boolean;
  /** Present while audio flows between the PCs (0.7.2+). */
  stream?: StreamBus | null;
  send_mix?: SendMix | null;
}

export interface AudioDevice {
  id: string;
  name: string;
  device_type: "output" | "input";
  volume: number;
  muted: boolean;
  is_default: boolean;
}

export interface AudioStats {
  mode: AudioMode;
  state: string;
  last_error: string | null;
  device: string | null;
  kbps: number;
  ms_since_last_packet: number | null;
  packets_sent: number;
  send_errors: number;
  capture_rate: number;
  capture_channels: number;
  packets_received: number;
  packets_lost: number;
  packets_late: number;
  packets_rejected: number;
  underruns: number;
  skips: number;
  buffer_ms: number;
  target_ms: number;
  drift_ppm: number;
  input_rate: number;
  output_rate: number;
  output_channels: number;
  /** 0–1: captured level when sending, played level when receiving. */
  level: number;
  /** Incoming audio is lowered because a call app on this PC is talking. */
  ducked: boolean;
}

export interface CompanionStatus {
  enabled: boolean;
  running: boolean;
  ws_port: number;
  discovery_port: number;
  connected_clients: number;
  token: string | null;
  last_error: string | null;
}

export interface LastPeer {
  device_id: string;
  hostname: string;
  ip: string;
  direction: Direction;
  resume: boolean;
}

export interface Settings {
  /** Name shown to other PCs; null = the Windows computer name. An empty string in a patch clears it. */
  pc_name: string | null;
  start_minimized: boolean;
  auto_reconnect: boolean;
  latency_mode: LatencyMode;
  remote_volume: number;
  remote_muted: boolean;
  notifications: boolean;
  hotkeys: boolean;
  /** Lower the other PC's audio while a call app on this PC is talking. */
  voice_ducking: boolean;
  /** In the headset, the PC with the pointer is louder. */
  follow_pointer: boolean;
  /** How loud the other PC stays meanwhile (0–1). */
  follow_depth: number;
  shortcuts: ShortcutStatus[];
  autostart: boolean;
  last_peer: LastPeer | null;
}

export interface ShortcutStatus {
  action: "toggle_mute" | "toggle_audio";
  keys: string;
  /** False when another app already uses the combination. */
  registered: boolean;
}

/** Keyboard & mouse sharing (src-tauri/src/input). */
export type InputStatus = "off" | "waiting" | "unsupported" | "peer_off" | "ready" | "controlling" | "controlled";

/** A monitor in the shared arrangement, in pixels (world coordinates). */
export interface ScreenView {
  /** Device id of the PC the monitor belongs to. */
  pc: string;
  local: boolean;
  id: string;
  /** Windows' display number on its own PC. */
  number: number;
  name: string;
  primary: boolean;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface InputView {
  enabled: boolean;
  /** The input service (admin windows, UAC, lock screen): off, or how it's doing. */
  admin_input: "off" | "ready" | "starting" | "not_installed" | "failed";
  /** The window under the pointer gets the keyboard when the pointer arrives on a screen. */
  focus_follows: boolean;
  /** The pointer stays on this PC while a game is in front. */
  game_mode: boolean;
  /** No switching PCs in the corners of a screen. */
  guard_corners: boolean;
  status: InputStatus;
  this_pc: string;
  peer_name: string | null;
  screens: ScreenView[];
  /** A screen of each PC touches the other, so the pointer can cross. */
  reachable: boolean;
}

export interface MixLevel {
  volume: number;
  muted: boolean;
}

/** One PC's part of a scene. Apps are matched by name when it's applied. */
export interface MixSide {
  master: MixLevel | null;
  apps: { name: string; volume: number; muted: boolean; output: string | null }[];
}

/** A saved mix of both PCs (src/app/scenes.ts, src-tauri/src/scenes.rs). */
export interface MixScene {
  id: string;
  name: string;
  this_pc: MixSide;
  other_pc: MixSide | null;
  /** Volume / mute of the sound received from the other PC. */
  incoming: MixLevel | null;
}

/** A pop-up card (src-tauri/src/popups.rs). */
export interface PopupCard {
  id: number;
  tone: "info" | "success" | "warning" | "error";
  icon: string;
  title: string;
  text: string;
  progress: number | null;
  actions: { id: string; label: string; primary: boolean }[];
  sticky: boolean;
}

/** Shared clipboard: the switch and the last thing that crossed. */
export interface ClipboardView {
  enabled: boolean;
  last: {
    /** This PC sent it; otherwise it came from the other PC. */
    sent: boolean;
    kind: "text" | "image" | "files";
    /** "42 characters", "1920 × 1080". */
    detail: string;
    at: number;
  } | null;
}

export interface Placement {
  pc: string;
  monitor: string;
  x: number;
  y: number;
}

/** Updates from the linked PC (src-tauri/src/update.rs). */
export interface UpdateState {
  current: string;
  peer_version: string | null;
  available: { version: string; from: string; size: number } | null;
  stage: "idle" | "downloading" | "installing" | "failed";
  progress: number;
  error: string | null;
}

export interface PairedDevice {
  device_id: string;
  hostname: string;
  ip: string;
  first_seen: number;
  last_seen: number;
}
