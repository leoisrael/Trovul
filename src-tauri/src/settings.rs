// Trovul - Persisted user settings (`settings.json` under the app-data dir).

use crate::audio::{Direction, LatencyMode};
use crate::input::SharedLayout;
use crate::util::{random_hex, read, write};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock, RwLock};

/// An app chosen to go to the other PC, and how loud it goes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendApp {
    /// Process name as the mixer shows it (compared without case).
    pub name: String,
    pub level: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputLayoutSetting {
    /// Device id of the PC this arrangement was made with.
    pub peer: String,
    pub layout: SharedLayout,
}

/// The PC we were last connected to — used to reconnect automatically.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastPeer {
    pub device_id: String,
    pub hostname: String,
    pub ip: String,
    /// Audio direction from this PC's point of view, restored after reconnecting.
    #[serde(default)]
    pub direction: Direction,
    /// Cleared once either side disconnects on purpose: no reconnect then.
    #[serde(default)]
    pub resume: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Stable identity of this installation (trust list, reconnects).
    pub device_id: String,
    /// Name other PCs see instead of the Windows computer name.
    pub pc_name: Option<String>,
    /// Companion API (WebSocket + UDP discovery responder). Off until enabled.
    pub companion_enabled: bool,
    /// Token companion clients must present (`ws://host:5534/?token=…`).
    pub companion_token: Option<String>,
    /// Endpoint id of the device that plays remote audio. `None` = system default.
    pub playback_device: Option<String>,
    /// Stay in the tray when launched at Windows sign-in.
    pub start_minimized: bool,
    /// Reconnect to `last_peer` whenever it is reachable.
    pub auto_reconnect: bool,
    pub latency_mode: LatencyMode,
    /// Gain applied to received audio (0–1), independent of Windows volumes.
    pub remote_volume: f32,
    pub remote_muted: bool,
    /// Windows notifications on connect/disconnect.
    pub notifications: bool,
    /// Global shortcuts (Ctrl+Alt+M mute incoming, Ctrl+Alt+T audio on/off).
    pub hotkeys: bool,
    /// Lower the other PC's audio while a call app on this PC is talking.
    pub voice_ducking: bool,
    /// Share one keyboard and mouse with the linked PC (both must opt in).
    pub input_sharing: bool,
    /// Keyboard & mouse: the window under the pointer gets the keyboard when
    /// the pointer arrives on a screen.
    pub input_focus_follows: bool,
    /// Keyboard & mouse: the pointer stays on this PC while a game is in front.
    pub input_game_mode: bool,
    /// Keyboard & mouse: no switching PCs in the corners of a screen.
    pub input_guard_corners: bool,
    /// Send only the chosen apps to the other PC (Windows 11, app_capture.rs).
    pub send_apps_only: bool,
    pub send_apps: Vec<SendApp>,
    /// Follow the pointer: the PC in use is louder in the headset (focus_mix.rs).
    pub follow_pointer: bool,
    /// How loud the PC not in use stays (0–1).
    pub follow_depth: f32,
    /// This PC's apps were lowered by follow-the-pointer (restored at start).
    pub focus_lowered: bool,
    /// Keyboard & mouse through the input service: works in admin windows,
    /// UAC prompts and on the lock screen (input_service.rs).
    pub admin_input: bool,
    /// Copy on one PC, paste on the other (both must have it on).
    pub clipboard_sharing: bool,
    /// Saved mixes of both PCs (`scenes.rs`).
    pub mix_scenes: Vec<crate::scenes::Scene>,
    /// The screen arrangement agreed with `peer`.
    pub input_layout: Option<InputLayoutSetting>,
    pub last_peer: Option<LastPeer>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            device_id: String::new(),
            pc_name: None,
            companion_enabled: false,
            companion_token: None,
            playback_device: None,
            start_minimized: true,
            auto_reconnect: true,
            latency_mode: LatencyMode::default(),
            remote_volume: 1.0,
            remote_muted: false,
            notifications: true,
            hotkeys: true,
            voice_ducking: true,
            input_sharing: false,
            clipboard_sharing: true,
            input_focus_follows: true,
            input_game_mode: true,
            input_guard_corners: true,
            admin_input: false,
            send_apps_only: false,
            send_apps: Vec::new(),
            follow_pointer: false,
            follow_depth: 0.3,
            focus_lowered: false,
            mix_scenes: Vec::new(),
            input_layout: None,
            last_peer: None,
        }
    }
}

const FILE_NAME: &str = "settings.json";

static SETTINGS: LazyLock<RwLock<Settings>> = LazyLock::new(|| RwLock::new(Settings::default()));
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Remember the app-data dir and load `settings.json` from it. Called once from setup.
pub fn init(data_dir: PathBuf) {
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        log::warn!("Could not create {}: {}", data_dir.display(), e);
    }
    let _ = DATA_DIR.set(data_dir);

    if let Some(path) = data_file(FILE_NAME) {
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(loaded) => *write(&SETTINGS) = loaded,
                Err(e) => log::warn!("Ignoring unreadable {}: {}", path.display(), e),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("Could not read {}: {}", path.display(), e),
        }
    }
    if read(&SETTINGS).device_id.is_empty() {
        update(|s| s.device_id = random_hex::<16>());
    }
}

pub fn get() -> Settings {
    read(&SETTINGS).clone()
}

pub fn device_id() -> String {
    read(&SETTINGS).device_id.clone()
}

pub fn pc_name() -> Option<String> {
    read(&SETTINGS).pc_name.clone()
}

/// Longest name accepted for `pc_name` (the UI enforces the same limit).
pub const PC_NAME_MAX_CHARS: usize = 32;

/// Trimmed, printable, length-capped name; `None` when nothing is left.
pub fn clean_pc_name(name: &str) -> Option<String> {
    let printable: String = name.chars().filter(|c| !c.is_control()).collect();
    let capped: String = printable.trim().chars().take(PC_NAME_MAX_CHARS).collect();
    let name = capped.trim_end();
    (!name.is_empty()).then(|| name.to_string())
}

/// Apply `change`, persist, and return the new settings.
pub fn update(change: impl FnOnce(&mut Settings)) -> Settings {
    let mut guard = write(&SETTINGS);
    change(&mut guard);
    let snapshot = guard.clone();
    // Written while still holding the lock so concurrent updates hit the disk in order.
    if let Some(path) = data_file(FILE_NAME) {
        match serde_json::to_vec_pretty(&snapshot) {
            Ok(bytes) => {
                if let Err(e) = write_atomic(&path, &bytes) {
                    log::warn!("Could not save {}: {}", path.display(), e);
                }
            }
            Err(e) => log::warn!("Could not serialise settings: {}", e),
        }
    }
    snapshot
}

pub fn data_file(name: &str) -> Option<PathBuf> {
    DATA_DIR.get().map(|dir| dir.join(name))
}

/// Write through a temp file + rename so a crash mid-write can't leave a
/// truncated file behind.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pc_names_are_trimmed_capped_and_printable() {
        assert_eq!(clean_pc_name("  Living room  "), Some("Living room".into()));
        assert_eq!(clean_pc_name("Studio\n\tPC"), Some("StudioPC".into()));
        assert_eq!(clean_pc_name("   "), None);
        assert_eq!(clean_pc_name(""), None);
        let long = "x".repeat(40);
        assert_eq!(
            clean_pc_name(&long).unwrap().chars().count(),
            PC_NAME_MAX_CHARS
        );
        // Capping never leaves a trailing space behind.
        let spaced = format!("{} tail", "y".repeat(PC_NAME_MAX_CHARS - 1));
        assert_eq!(
            clean_pc_name(&spaced),
            Some("y".repeat(PC_NAME_MAX_CHARS - 1))
        );
        assert_eq!(
            clean_pc_name("Sala de Estar — PC"),
            Some("Sala de Estar — PC".into())
        );
    }
}
