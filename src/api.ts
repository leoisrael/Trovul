// Typed wrappers around the Tauri commands registered in src-tauri/src/lib.rs.

import { invoke } from "@tauri-apps/api/core";
import type {
  PopupCard,
  MixScene,
  ClipboardView,
  AppInfo,
  AudioDevice,
  AudioStats,
  CompanionStatus,
  ConnectionInfo,
  ConnectResult,
  Direction,
  InputView,
  MixerSnapshot,
  NetworkProfile,
  PairedDevice,
  Peer,
  Placement,
  Settings,
  UpdateState,
} from "./types";

type Icons = Record<string, string>;
export type SettingsPatch = Partial<Omit<Settings, "autostart" | "last_peer" | "shortcuts">>;

export const api = {
  appInfo: () => invoke<AppInfo>("get_app_info"),
  openMainWindow: () => invoke<void>("open_main_window"),
  popups: () => invoke<PopupCard[]>("get_popups"),
  resizePopups: (height: number) => invoke<void>("resize_popups", { height }),
  hidePopups: () => invoke<void>("hide_popups"),
  popupAction: (id: number, action: string) => invoke<void>("popup_action", { id, action }),
  hideFlyout: () => invoke<void>("hide_flyout"),
  windowVisible: () => invoke<boolean>("get_window_visible"),
  connectionInfo: () => invoke<ConnectionInfo>("get_connection_info"),
  discoveredPeers: () => invoke<Peer[]>("get_discovered_peers"),
  connect: (peerIp: string, deviceId?: string) =>
    invoke<ConnectResult>("request_connection", { peerIp, deviceId: deviceId ?? null }),
  acceptConnection: () => invoke<void>("accept_connection"),
  rejectConnection: () => invoke<void>("reject_connection"),
  disconnect: () => invoke<void>("disconnect"),
  pairedDevices: () => invoke<PairedDevice[]>("get_paired_devices"),
  forgetPairedDevice: (deviceId: string) => invoke<void>("forget_paired_device", { deviceId }),
  networkProfile: () => invoke<NetworkProfile | null>("get_network_profile"),
  openNetworkSettings: (wifi: boolean) => invoke<void>("open_network_settings", { wifi }),

  direction: () => invoke<Direction>("get_stream_direction"),
  setDirection: (direction: Direction) => invoke<Direction>("set_stream_direction", { direction }),
  playTestSound: () => invoke<void>("play_test_sound"),
  audioStats: () => invoke<AudioStats>("get_audio_stats"),
  remoteAudioStats: () => invoke<AudioStats>("get_remote_audio_stats"),
  playbackDevice: () => invoke<string | null>("get_playback_device"),
  setPlaybackDevice: (deviceId: string | null) => invoke<void>("set_playback_device", { deviceId }),

  settings: () => invoke<Settings>("get_settings"),
  updateSettings: (patch: SettingsPatch) => invoke<Settings>("update_settings", { patch }),
  setAutostart: (enabled: boolean) => invoke<boolean>("set_autostart", { enabled }),
  openLogFolder: () => invoke<void>("open_log_folder"),
  supportInfo: () => invoke<string>("support_info"),

  localMixer: () => invoke<MixerSnapshot>("get_local_mixer"),
  localIcons: (ids: string[]) => invoke<Icons>("get_icons", { ids }),
  outputDevices: () => invoke<AudioDevice[]>("get_output_devices"),
  inputDevices: () => invoke<AudioDevice[]>("get_input_devices"),
  setSessionVolume: (pid: number, volume: number) => invoke<void>("set_session_volume", { pid, volume }),
  setSessionMute: (pid: number, muted: boolean) => invoke<void>("set_session_mute", { pid, muted }),
  setAppOutput: (pid: number, deviceId: string | null) => invoke<void>("set_app_output", { pid, deviceId }),
  setSendMode: (appsOnly: boolean) => invoke<void>("set_send_mode_cmd", { appsOnly }),
  setAppSend: (name: string, on: boolean, level: number | null) => invoke<void>("set_app_send_cmd", { name, on, level }),
  setRemoteSendMode: (appsOnly: boolean) => invoke<void>("set_remote_send_mode", { appsOnly }),
  setRemoteAppSend: (name: string, on: boolean, level: number | null) =>
    invoke<void>("set_remote_app_send", { name, on, level }),
  setMasterVolume: (volume: number) => invoke<void>("set_master_volume", { volume }),
  setMasterMute: (muted: boolean) => invoke<void>("set_master_mute", { muted }),
  setDeviceVolume: (deviceId: string, volume: number) => invoke<void>("set_device_volume", { deviceId, volume }),
  setDeviceMute: (deviceId: string, muted: boolean) => invoke<void>("set_device_mute", { deviceId, muted }),

  remoteMixer: () => invoke<MixerSnapshot>("get_remote_mixer"),
  remoteIcons: (ids: string[]) => invoke<Icons>("get_remote_icons", { ids }),
  setRemoteSessionVolume: (pid: number, volume: number) =>
    invoke<void>("set_remote_session_volume", { pid, volume }),
  setRemoteSessionMute: (pid: number, muted: boolean) => invoke<void>("set_remote_session_mute", { pid, muted }),
  setRemoteMasterVolume: (volume: number) => invoke<void>("set_remote_master_volume", { volume }),
  setRemoteMasterMute: (muted: boolean) => invoke<void>("set_remote_master_mute", { muted }),
  remoteOutputDevices: () => invoke<AudioDevice[]>("get_remote_output_devices"),
  setRemoteAppOutput: (pid: number, deviceId: string | null) => invoke<void>("set_remote_app_output", { pid, deviceId }),
  /** Volume / mute of the sound the other PC receives from this one. */
  setRemoteIncoming: (volume: number | null, muted: boolean | null) =>
    invoke<void>("set_remote_incoming", { volume, muted }),
  setRemotePlaybackDevice: (deviceId: string | null) => invoke<void>("set_remote_playback_device", { deviceId }),

  inputView: () => invoke<InputView>("get_input_view"),
  refreshInput: () => invoke<InputView>("refresh_input"),
  setInputSharing: (enabled: boolean) => invoke<InputView>("set_input_sharing", { enabled }),
  setInputLayout: (placements: Placement[]) => invoke<InputView>("set_input_layout", { placements }),
  resetInputLayout: () => invoke<InputView>("reset_input_layout"),
  setAdminInput: (enabled: boolean) => invoke<InputView>("set_admin_input", { enabled }),
  setInputOptions: (options: { focusFollows?: boolean; gameMode?: boolean; guardCorners?: boolean }) =>
    invoke<InputView>("set_input_options", {
      focusFollows: options.focusFollows ?? null,
      gameMode: options.gameMode ?? null,
      guardCorners: options.guardCorners ?? null,
    }),
  mixScenes: () => invoke<MixScene[]>("get_mix_scenes"),
  captureScene: (name: string, id: string | null) => invoke<MixScene>("capture_scene", { name, id }),
  applyScene: (id: string) => invoke<string>("apply_scene", { id }),
  setMixScenes: (scenes: MixScene[]) => invoke<MixScene[]>("set_mix_scenes", { scenes }),
  clipboardView: () => invoke<ClipboardView>("get_clipboard_view"),
  setClipboardSharing: (enabled: boolean) => invoke<ClipboardView>("set_clipboard_sharing", { enabled }),

  updateState: () => invoke<UpdateState>("get_update_state"),
  installUpdate: () => invoke<void>("install_update"),
  dismissUpdate: () => invoke<void>("dismiss_update"),

  companionStatus: () => invoke<CompanionStatus>("get_companion_status"),
  setCompanionEnabled: (enabled: boolean) => invoke<CompanionStatus>("set_companion_enabled", { enabled }),
  regenerateCompanionToken: () => invoke<CompanionStatus>("regenerate_companion_token"),
};

/** Tauri rejects with the command's error string; anything else is unexpected. */
export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return String(error);
}
