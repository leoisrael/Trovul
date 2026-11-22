// Trovul - App entry: Tauri commands, system tray, lifecycle.
//
//   network          discovery broadcast, ports, protocol version
//   peer             control-link framing/messages, PeerLink
//   connection       listener, pairing, paired devices, active link, auto-reconnect
//   audio            streaming engine (sender / receiver)
//   audio_sessions   per-app volume, devices, master, icons (Core Audio)
//   companion_server opt-in WebSocket API
//   settings         persisted preferences
//
// Commands that touch COM or the network are `async` so they run off the main
// (UI) thread; the rest are trivial reads.

mod app_routing;
mod audio;
mod audio_sessions;
mod clipboard;
mod clipfiles;
mod com;
mod companion_server;
mod connection;
mod ducking;
mod flyout;
mod focus_mix;
mod hotkeys;
mod input;
mod input_service;
mod netprofile;
mod network;
mod peer;
mod popups;
mod scenes;
mod settings;
mod tray_icon;
mod update;
mod util;

use audio::{AudioStats, Direction, LatencyMode, Mode};
use audio_sessions::{AppSend, AudioDevice, MixerSnapshot, SendMix, StreamBus};
use companion_server::CompanionStatus;
use connection::{ConnectResult, ConnectionInfo, PairedDevice, PendingRequest};
use peer::{CloseReason, LinkInfo, Request, Response};
use serde::{Deserialize, Serialize};
use settings::LastPeer;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};
use util::lock;

static APP: OnceLock<AppHandle> = OnceLock::new();

/// Passed by the Windows sign-in entry so we can start quietly in the tray.
const AUTOSTART_FLAG: &str = "--autostart";

/// Tray handles, mutated in place when state changes.
struct Tray {
    icon: TrayIcon,
    /// App icon the status badge is drawn onto, and the state shown now.
    base_icon: Option<tauri::image::Image<'static>>,
    shown_state: Option<tray_icon::TrayState>,
    status: MenuItem<tauri::Wry>,
    audio: MenuItem<tauri::Wry>,
    hear: CheckMenuItem<tauri::Wry>,
    send: CheckMenuItem<tauri::Wry>,
    stop: MenuItem<tauri::Wry>,
    mute: CheckMenuItem<tauri::Wry>,
    disconnect: MenuItem<tauri::Wry>,
    autostart: CheckMenuItem<tauri::Wry>,
}

static TRAY: Mutex<Option<Tray>> = Mutex::new(None);
/// Last direction that actually moved audio, for the on/off shortcut.
static LAST_ACTIVE: Mutex<Direction> = Mutex::new(Direction::Receive);

const UNEXPECTED_ANSWER: &str = "Unexpected answer from the other PC";

// ============ Events ============

#[derive(Clone, Serialize)]
struct ConnectionEvent {
    /// "established" | "closed"
    kind: &'static str,
    info: ConnectionInfo,
    peer_hostname: String,
    reason: Option<String>,
    /// Closed on purpose (no automatic reconnect will follow).
    intentional: bool,
}

#[derive(Clone, Serialize)]
struct AudioModeEvent {
    mode: Mode,
    direction: Direction,
    reason: Option<String>,
}

pub(crate) fn app_handle() -> Option<AppHandle> {
    APP.get().cloned()
}

fn emit<S: Serialize + Clone>(event: &str, payload: S) {
    // Controllers (Stream Deck…) follow the same changes as the windows.
    if matches!(
        event,
        "audio-mode-changed" | "settings-changed" | "scenes-changed"
    ) {
        companion_server::notify_state();
    }
    if let Some(app) = APP.get() {
        if let Err(e) = app.emit(event, payload) {
            log::debug!("Could not emit {event}: {e}");
        }
    }
}

pub(crate) fn show_main_window() {
    let Some(app) = APP.get() else { return };
    flyout::hide_now(app);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    emit("window-visibility", true);
}

fn emit_audio_mode(reason: Option<String>) {
    let mode = audio::mode();
    if let Some(reason) = &reason {
        popups::show(
            popups::Popup::new("info", "volume", "Sound between the PCs", reason.clone()),
            None,
        );
    }
    emit(
        "audio-mode-changed",
        AudioModeEvent {
            mode,
            direction: Direction::from_local_mode(mode),
            reason,
        },
    );
    refresh_tray();
}

pub(crate) fn on_link_established(link: &LinkInfo) {
    emit(
        "connection-changed",
        ConnectionEvent {
            kind: "established",
            info: connection::info(),
            peer_hostname: link.peer_hostname.clone(),
            reason: None,
            intentional: false,
        },
    );
    popups::show(
        popups::Popup::new(
            "success",
            "link",
            format!("Linked to {}", link.peer_hostname),
            "Sound, keyboard and mouse are ready.",
        ),
        None,
    );
    companion_server::notify_connection_changed();
    refresh_tray();
    clipboard::link_changed();
    tauri::async_runtime::spawn(input::refresh());
    tauri::async_runtime::spawn(update::check());
}

/// The update offer or its progress changed.
pub(crate) fn on_update_changed() {
    let state = update::state();
    emit("update-changed", state.clone());
    // A new offer: one card, with the button.
    static OFFERED: Mutex<Option<String>> = Mutex::new(None);
    let offer = state
        .available
        .as_ref()
        .map(|a| (a.version.clone(), a.from.clone()));
    let mut offered = lock(&OFFERED);
    if let Some((version, from)) = offer {
        if offered.as_deref() != Some(version.as_str()) {
            *offered = Some(version.clone());
            let popup = popups::Popup::new(
                "info",
                "update",
                format!("Trovul {version} is on {from}"),
                "Update this PC from it, no download needed.",
            )
            .action("later", "Later", false)
            .action("update", "Update now", true);
            popups::show(
                popup,
                Some(Box::new(|action: &str| {
                    if action == "update" {
                        tauri::async_runtime::spawn(async {
                            if let Err(e) = install_update().await {
                                log::warn!("Update from the pop-up failed: {e}");
                            }
                        });
                    } else if action == "later" {
                        update::dismiss();
                    }
                })),
            );
        }
    } else {
        *offered = None;
    }
}

/// Quit so the installer can replace this Trovul; the peer reconnects later.
pub(crate) fn quit_for_update() {
    let Some(app) = APP.get().cloned() else {
        return;
    };
    std::thread::spawn(move || {
        // Let the UI show "Installing…" for a moment.
        std::thread::sleep(std::time::Duration::from_millis(800));
        connection::disconnect("Trovul is updating on the other PC", false);
        audio::stop();
        app.exit(0);
    });
}

/// `trovul.exe --input-service`: run as the Windows service.
pub fn run_input_service() {
    input_service::run_service();
}

/// `trovul.exe --input-helper`: the service's helper in the user's session.
pub fn run_input_helper() {
    input_service::run_helper();
}

/// Keyboard & mouse sharing changed state (status, screens, arrangement).
pub(crate) fn on_input_changed() {
    emit("input-changed", input::view());
}

/// Something about keyboard & mouse sharing the user should know.
pub(crate) fn on_input_notice(message: String) {
    log::info!("Keyboard & mouse: {message}");
    emit("input-notice", message.clone());
    popups::show(
        popups::Popup::new("warning", "shield", "Keyboard & mouse", message),
        None,
    );
}

pub(crate) fn on_clipboard_changed() {
    emit("clipboard-changed", clipboard::view());
}

pub(crate) fn on_link_closed(link: &LinkInfo, reason: &CloseReason) {
    // The user who disconnected knows; anything else is worth a card.
    if !matches!(reason, CloseReason::Local { .. }) {
        let s = settings::get();
        let back = s.auto_reconnect && s.last_peer.as_ref().is_some_and(|p| p.resume);
        popups::show(
            popups::Popup::new(
                "warning",
                "unlink",
                format!("Lost the link to {}", link.peer_hostname),
                if back {
                    format!("{}. Reconnecting when it's back.", reason.describe())
                } else {
                    reason.describe()
                },
            ),
            None,
        );
    }
    // The pointer and any held keys come home before anything else.
    input::link_closed();
    update::link_closed();
    clipboard::link_changed();
    // Audio belongs to the link: never keep streaming to a peer we left.
    if audio::stop_for_link(link.id) {
        // This PC's user already knows when they disconnected themselves.
        let note = match reason {
            CloseReason::Local { .. } => None,
            other => Some(format!("{} — audio stopped", other.describe())),
        };
        emit_audio_mode(note);
    }
    emit(
        "connection-changed",
        ConnectionEvent {
            kind: "closed",
            info: connection::info(),
            peer_hostname: link.peer_hostname.clone(),
            reason: Some(reason.describe()),
            intentional: reason.intentional(),
        },
    );
    emit("settings-changed", settings_view());
    companion_server::notify_connection_changed();
    refresh_tray();
}

pub(crate) fn on_connection_request(request: PendingRequest) {
    // The app usually lives in the tray: bring the prompt to the user.
    show_main_window();
    emit("connection-request", request);
}

pub(crate) fn on_connection_request_cancelled() {
    emit("connection-request-cancelled", ());
}

pub(crate) fn on_settings_changed() {
    emit("settings-changed", settings_view());
    refresh_tray();
}

/// After an automatic reconnect, put the audio back the way it was.
pub(crate) fn on_reconnected(direction: Direction) {
    if direction == Direction::Off {
        return;
    }
    tauri::async_runtime::spawn(async move {
        match apply_direction(direction).await {
            Ok(_) => log::info!("Restored audio ({direction:?}) after reconnecting"),
            Err(e) => log::warn!("Could not restore audio after reconnecting: {e}"),
        }
    });
}

// ============ Audio direction ============

/// Start/stop this PC's half and remember the direction for reconnects.
/// `note` tells the UI why it changed when it wasn't this PC's user.
fn apply_local_mode(mode: Mode, note: Option<String>) -> Result<(), String> {
    let before = audio::mode();
    let result = audio::start(mode, connection::stream_peer);
    if result.is_ok() {
        let direction = Direction::from_local_mode(mode);
        connection::remember_direction(direction);
        if direction != Direction::Off {
            *lock(&LAST_ACTIVE) = direction;
        }
    }
    let changed = audio::mode() != before;
    emit_audio_mode(note.filter(|_| changed));
    result
}

/// The other PC switched direction; this is our half of it.
pub(crate) fn apply_peer_audio_mode(mode: Mode) -> Result<(), String> {
    log::info!("The other PC set this PC to {}", mode.as_str());
    let peer = connection::info()
        .peer_hostname
        .unwrap_or_else(|| "The other PC".into());
    let note = match mode {
        Mode::Host => format!("{peer} started playing its audio here"),
        Mode::Sender => format!("{peer} is now playing this PC's audio"),
        Mode::Off => format!("{peer} turned the audio off"),
    };
    apply_local_mode(mode, Some(note))
}

/// Configure both PCs for `direction` in one step. The receiving side starts
/// first so nothing is lost; if the other PC can't follow, we roll back.
async fn apply_direction(direction: Direction) -> Result<Direction, String> {
    // UI, tray, shortcuts and reconnects can all ask at once; interleaving the
    // two halves could leave both PCs receiving.
    static DIRECTION_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));
    let _serial = DIRECTION_LOCK.lock().await;
    if connection::current().is_none() {
        return Err("Connect to another PC first".into());
    }
    let set_local = |mode: Mode| blocking(move || apply_local_mode(mode, None));
    let set_peer = |mode: Mode| connection::request(Request::SetAudioMode { mode });
    match direction {
        Direction::Receive => {
            set_local(Mode::Host).await?;
            if let Err(e) = set_peer(Mode::Sender).await {
                let _ = set_local(Mode::Off).await;
                return Err(format!("The other PC could not start sending: {e}"));
            }
        }
        Direction::Send => {
            set_peer(Mode::Host)
                .await
                .map_err(|e| format!("The other PC could not start receiving: {e}"))?;
            if let Err(e) = set_local(Mode::Sender).await {
                let _ = set_peer(Mode::Off).await;
                return Err(e);
            }
        }
        Direction::Off => {
            set_local(Mode::Off).await?;
            // Best effort: the other PC also stops on its own when the link ends.
            let _ = set_peer(Mode::Off).await;
        }
    }
    Ok(direction)
}

fn spawn_direction(direction: Direction) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = apply_direction(direction).await {
            log::warn!("Could not change the audio direction: {e}");
            emit("app-error", e);
            refresh_tray();
        }
    });
}

// ============ Tray ============

fn autostart_enabled() -> bool {
    use tauri_plugin_autostart::ManagerExt;
    APP.get()
        .map(|app| app.autolaunch().is_enabled().unwrap_or(false))
        .unwrap_or(false)
}

fn refresh_tray() {
    let info = connection::info();
    let mode = audio::mode();
    let s = settings::get();
    let muted = s.remote_muted;
    let autostart = autostart_enabled();
    let mut tray = lock(&TRAY);
    let Some(tray) = tray.as_mut() else { return };
    let connected = info.connected;

    let state = match (connected, mode) {
        (true, Mode::Off) => tray_icon::TrayState::Connected,
        (true, _) => tray_icon::TrayState::Streaming,
        (false, _) if s.auto_reconnect && s.last_peer.as_ref().is_some_and(|p| p.resume) => {
            tray_icon::TrayState::Reconnecting
        }
        (false, _) => tray_icon::TrayState::Disconnected,
    };
    if tray.shown_state != Some(state) {
        if let Some(base) = &tray.base_icon {
            let (w, h) = (base.width(), base.height());
            let rgba = tray_icon::with_badge(base.rgba(), w, h, state);
            let _ = tray
                .icon
                .set_icon(Some(tauri::image::Image::new_owned(rgba, w, h)));
        }
        tray.shown_state = Some(state);
    }
    let peer = info.peer_hostname.as_deref().unwrap_or("the other PC");

    let (tooltip, status) = if connected {
        let doing = match mode {
            Mode::Sender => format!(" · sending audio to {peer}"),
            Mode::Host => format!(" · playing {peer}"),
            Mode::Off => String::new(),
        };
        (
            format!("Trovul - Connected to {peer}{doing}"),
            format!("Connected to {peer}"),
        )
    } else {
        (
            "Trovul - Not connected".to_string(),
            "Not connected".to_string(),
        )
    };
    let _ = tray.icon.set_tooltip(Some(&tooltip));
    let _ = tray.status.set_text(&status);
    let _ = tray.audio.set_text(match mode {
        Mode::Sender => "Audio: sending 🔊",
        Mode::Host => "Audio: receiving 🎧",
        Mode::Off => "Audio: off",
    });
    let _ = tray.hear.set_text(format!("Hear {peer} on this PC"));
    let _ = tray.hear.set_enabled(connected);
    let _ = tray.hear.set_checked(mode == Mode::Host);
    let _ = tray
        .send
        .set_text(format!("Send this PC's audio to {peer}"));
    let _ = tray.send.set_enabled(connected);
    let _ = tray.send.set_checked(mode == Mode::Sender);
    let _ = tray.stop.set_enabled(connected && mode != Mode::Off);
    let _ = tray.mute.set_checked(muted);
    let _ = tray.disconnect.set_enabled(connected);
    let _ = tray.autostart.set_checked(autostart);
}

fn on_hotkey(action: hotkeys::Action) {
    match action {
        hotkeys::Action::ToggleMute => toggle_incoming_mute(),
        hotkeys::Action::ToggleAudio => {
            if connection::current().is_none() {
                return;
            }
            let next = if audio::mode() == Mode::Off {
                *lock(&LAST_ACTIVE)
            } else {
                Direction::Off
            };
            spawn_direction(next);
        }
    }
}

fn toggle_incoming_mute() {
    incoming_changed(&settings::update(|s| s.remote_muted = !s.remote_muted));
}

/// Volume / mute of the sound received, from the other PC's mixer.
pub(crate) fn set_incoming(volume: Option<f32>, muted: Option<bool>) {
    incoming_changed(&settings::update(|s| {
        if let Some(v) = volume.filter(|v| v.is_finite()) {
            s.remote_volume = v.clamp(0.0, 1.0);
        }
        if let Some(m) = muted {
            s.remote_muted = m;
        }
    }));
}

fn incoming_changed(s: &settings::Settings) {
    audio::set_remote_volume(s.remote_volume, s.remote_muted);
    emit("settings-changed", settings_view());
    refresh_tray();
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "status", "Not connected", false, None::<&str>)?;
    let audio_item = MenuItem::with_id(app, "audio", "Audio: off", false, None::<&str>)?;
    let hear = CheckMenuItem::with_id(
        app,
        "hear",
        "Hear the other PC here",
        false,
        false,
        None::<&str>,
    )?;
    let send = CheckMenuItem::with_id(
        app,
        "send",
        "Send this PC's audio",
        false,
        false,
        None::<&str>,
    )?;
    let stop = MenuItem::with_id(app, "stop_audio", "Stop audio", false, None::<&str>)?;
    let mute = CheckMenuItem::with_id(
        app,
        "mute_incoming",
        "Mute incoming audio",
        true,
        settings::get().remote_muted,
        None::<&str>,
    )?;
    let show = MenuItem::with_id(app, "show_window", "Open Trovul", true, None::<&str>)?;
    let disconnect_item = MenuItem::with_id(app, "disconnect", "Disconnect", false, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "Start with Windows",
        true,
        autostart_enabled(),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit Trovul", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &audio_item,
            &PredefinedMenuItem::separator(app)?,
            &hear,
            &send,
            &stop,
            &mute,
            &PredefinedMenuItem::separator(app)?,
            &show,
            &disconnect_item,
            &PredefinedMenuItem::separator(app)?,
            &autostart,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::new()
        .tooltip("Trovul - Not connected")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "hear" => spawn_direction(Direction::Receive),
            "send" => spawn_direction(Direction::Send),
            "stop_audio" => spawn_direction(Direction::Off),
            "mute_incoming" => toggle_incoming_mute(),
            "show_window" => show_main_window(),
            "disconnect" => {
                std::thread::spawn(|| {
                    connection::disconnect("disconnected from the tray menu", true)
                });
            }
            "autostart" => {
                let _ = set_autostart(!autostart_enabled());
            }
            "quit" => {
                let app = app.clone();
                std::thread::spawn(move || {
                    log::info!("Quit requested from the tray");
                    // Tell the peer right away; not intentional, so it reconnects later.
                    connection::disconnect("Trovul was closed on the other PC", false);
                    audio::stop();
                    app.exit(0);
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                position,
                ..
            } = event
            {
                flyout::toggle(tray.app_handle(), position);
            }
        });
    let base_icon = app
        .default_window_icon()
        .map(|i| tauri::image::Image::new_owned(i.rgba().to_vec(), i.width(), i.height()));
    if let Some(icon) = &base_icon {
        builder = builder.icon(icon.clone());
    }
    let icon = builder.build(app)?;

    *lock(&TRAY) = Some(Tray {
        icon,
        base_icon,
        shown_state: None,
        status,
        audio: audio_item,
        hear,
        send,
        stop,
        mute,
        disconnect: disconnect_item,
        autostart,
    });
    Ok(())
}

async fn blocking<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("Background task failed: {e}"))?
}

// ============ App & connection ============

#[derive(Serialize)]
struct AppInfo {
    /// The name other PCs see (see `network::hostname`).
    hostname: String,
    computer_name: String,
    local_ip: String,
    version: &'static str,
}

#[tauri::command]
fn get_app_info() -> AppInfo {
    AppInfo {
        hostname: network::hostname(),
        computer_name: network::computer_name(),
        local_ip: network::local_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "Unknown".into()),
        version: env!("CARGO_PKG_VERSION"),
    }
}

// ============ Updates from the linked PC ============

#[tauri::command]
fn get_update_state() -> update::UpdateState {
    update::state()
}

#[tauri::command]
async fn install_update() -> Result<(), String> {
    let app = APP.get().ok_or("The app is still starting")?;
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())?
        .join("updates");
    update::install(dir).await
}

#[tauri::command]
fn dismiss_update() {
    update::dismiss();
}

// ============ Keyboard & mouse sharing ============

#[tauri::command]
async fn get_input_view() -> Result<input::InputView, String> {
    blocking(|| Ok(input::view())).await
}

/// Ask the linked PC again (its screens, switch and arrangement).
#[tauri::command]
async fn refresh_input() -> Result<input::InputView, String> {
    input::refresh().await;
    blocking(|| Ok(input::view())).await
}

#[tauri::command]
async fn set_input_sharing(enabled: bool) -> Result<input::InputView, String> {
    input::set_enabled(enabled).await;
    blocking(|| Ok(input::view())).await
}

#[tauri::command]
fn get_mix_scenes() -> Vec<scenes::Scene> {
    scenes::list()
}

/// Replaces the whole list (add, rename, delete and reorder are all edits of it).
#[tauri::command]
fn set_mix_scenes(scenes: Vec<scenes::Scene>) -> Vec<scenes::Scene> {
    let saved = scenes::save(scenes);
    emit("scenes-changed", saved.clone());
    saved
}

#[tauri::command]
fn get_popups() -> Vec<popups::Popup> {
    popups::current()
}

#[tauri::command]
fn resize_popups(app: AppHandle, height: f64) {
    popups::resize(&app, height);
}

#[tauri::command]
fn hide_popups(app: AppHandle) {
    popups::hide(&app);
}

#[tauri::command]
fn popup_action(id: u64, action: String) {
    popups::act(id, &action);
}

/// "Hear", "Send" or "Off" from a controller (companion API).
pub(crate) async fn set_direction_from_api(direction: Direction) -> Result<(), String> {
    apply_direction(direction).await.map(|_| ())
}

/// Follow the pointer on or off (from the Mixer or a controller).
pub(crate) fn set_follow_pointer(on: bool) {
    settings::update(|s| s.follow_pointer = on);
    emit("settings-changed", settings_view());
}

/// The current mix of both PCs as a scene (not saved: the UI saves the list).
#[tauri::command]
async fn capture_scene(name: String, id: Option<String>) -> Result<scenes::Scene, String> {
    scenes::capture(name, id).await
}

#[tauri::command]
async fn apply_scene(id: String) -> Result<String, String> {
    scenes::apply(&id).await
}

/// A scene was applied (from anywhere): the UIs and the Stream Deck hear it.
pub(crate) fn on_scene_applied(name: &str) {
    emit("scene-applied", name.to_string());
    companion_server::notify_state();
}

/// From the tray mixer.
#[tauri::command]
fn open_main_window() {
    show_main_window();
}

#[tauri::command]
fn hide_flyout(app: AppHandle) {
    flyout::hide_now(&app);
}

#[tauri::command]
fn get_clipboard_view() -> clipboard::ClipboardView {
    clipboard::view()
}

#[tauri::command]
fn set_clipboard_sharing(enabled: bool) -> clipboard::ClipboardView {
    clipboard::set_enabled(enabled);
    clipboard::view()
}

/// Keyboard & mouse in admin windows, UAC prompts and on the lock screen.
#[tauri::command]
async fn set_admin_input(enabled: bool) -> Result<input::InputView, String> {
    input_service::set_enabled(enabled);
    blocking(|| Ok(input::view())).await
}

#[tauri::command]
async fn set_input_options(
    focus_follows: Option<bool>,
    game_mode: Option<bool>,
    guard_corners: Option<bool>,
) -> Result<input::InputView, String> {
    input::set_options(focus_follows, game_mode, guard_corners);
    blocking(|| Ok(input::view())).await
}

#[tauri::command]
async fn set_input_layout(placements: Vec<input::Placement>) -> Result<input::InputView, String> {
    input::set_layout(placements).await?;
    blocking(|| Ok(input::view())).await
}

#[tauri::command]
async fn reset_input_layout() -> Result<input::InputView, String> {
    input::reset_layout().await?;
    blocking(|| Ok(input::view())).await
}

/// Windows network profile of the LAN adapter; `None` when it can't be told.
#[tauri::command]
async fn get_network_profile() -> Result<Option<netprofile::NetworkProfile>, String> {
    blocking(|| Ok(lan_network_profile())).await
}

fn lan_network_profile() -> Option<netprofile::NetworkProfile> {
    let std::net::IpAddr::V4(ip) = network::local_ip()? else {
        return None;
    };
    netprofile::profile_for(ip)
        .map_err(|e| log::debug!("Network profile unavailable: {e}"))
        .ok()
}

/// Settings page where the user can switch the network profile to Private.
#[tauri::command]
fn open_network_settings(wifi: bool) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let app = APP.get().ok_or("The app is still starting")?;
    let page = if wifi {
        "ms-settings:network-wifi"
    } else {
        "ms-settings:network-ethernet"
    };
    app.opener()
        .open_url(page, None::<&str>)
        .map_err(|e| format!("Could not open Windows network settings: {e}"))
}

#[tauri::command]
fn get_window_visible(window: tauri::WebviewWindow) -> bool {
    window.is_visible().unwrap_or(true)
}

#[tauri::command]
fn get_connection_info() -> ConnectionInfo {
    connection::info()
}

#[tauri::command]
fn get_discovered_peers() -> Vec<network::Peer> {
    network::discovered_peers()
}

#[tauri::command]
async fn request_connection(
    peer_ip: String,
    device_id: Option<String>,
) -> Result<ConnectResult, String> {
    connection::connect(peer_ip, device_id).await
}

#[tauri::command]
fn accept_connection() -> Result<(), String> {
    connection::accept_pending()
}

#[tauri::command]
fn reject_connection() -> Result<(), String> {
    connection::reject_pending()
}

#[tauri::command]
async fn disconnect() -> Result<(), String> {
    blocking(|| {
        connection::disconnect("disconnected by the user", true);
        Ok(())
    })
    .await
}

#[tauri::command]
fn get_paired_devices() -> Vec<PairedDevice> {
    connection::paired_devices()
}

#[tauri::command]
fn forget_paired_device(device_id: String) {
    connection::forget_device(&device_id);
    emit("settings-changed", settings_view());
}

// ============ Audio streaming ============

#[tauri::command]
fn get_stream_direction() -> Direction {
    Direction::from_local_mode(audio::mode())
}

/// "receive" = play the other PC here, "send" = play this PC there, "off".
/// Configures both PCs.
#[tauri::command]
async fn set_stream_direction(direction: Direction) -> Result<Direction, String> {
    apply_direction(direction).await
}

/// Play a chime through the stream: the sending PC plays it locally, the
/// loopback picks it up and the other PC hears it — an end-to-end check.
#[tauri::command]
async fn play_test_sound() -> Result<(), String> {
    match Direction::from_local_mode(audio::mode()) {
        Direction::Send => blocking(audio::tone::play_chime).await,
        Direction::Receive => connection::request(Request::PlayTestSound)
            .await
            .map(|_| ()),
        Direction::Off => Err("Start sending or receiving audio first".into()),
    }
}

#[tauri::command]
fn get_audio_stats() -> AudioStats {
    audio::stats()
}

#[tauri::command]
async fn get_remote_audio_stats() -> Result<AudioStats, String> {
    match connection::request(Request::GetAudioStats).await? {
        Response::AudioStats { stats } => Ok(stats),
        _ => Err(UNEXPECTED_ANSWER.into()),
    }
}

#[tauri::command]
fn get_playback_device() -> Option<String> {
    audio::output_device()
}

/// Device (endpoint id) that plays the remote audio; `None` = system default.
#[tauri::command]
fn set_playback_device(device_id: Option<String>) {
    apply_playback_device(device_id);
}

pub(crate) fn apply_playback_device(device_id: Option<String>) {
    settings::update(|s| s.playback_device = device_id.clone());
    audio::set_output_device(device_id);
}

// ============ Settings ============

#[derive(Clone, Serialize)]
struct SettingsView {
    pc_name: Option<String>,
    start_minimized: bool,
    auto_reconnect: bool,
    latency_mode: LatencyMode,
    remote_volume: f32,
    remote_muted: bool,
    notifications: bool,
    hotkeys: bool,
    voice_ducking: bool,
    follow_pointer: bool,
    follow_depth: f32,
    shortcuts: Vec<hotkeys::ShortcutStatus>,
    autostart: bool,
    last_peer: Option<LastPeer>,
}

fn settings_view() -> SettingsView {
    let s = settings::get();
    SettingsView {
        pc_name: s.pc_name,
        start_minimized: s.start_minimized,
        auto_reconnect: s.auto_reconnect,
        latency_mode: s.latency_mode,
        remote_volume: s.remote_volume,
        remote_muted: s.remote_muted,
        notifications: s.notifications,
        hotkeys: s.hotkeys,
        voice_ducking: s.voice_ducking,
        follow_pointer: s.follow_pointer,
        follow_depth: s.follow_depth,
        shortcuts: hotkeys::status(),
        autostart: autostart_enabled(),
        last_peer: s.last_peer,
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SettingsPatch {
    /// An empty string goes back to the computer name.
    pc_name: Option<String>,
    start_minimized: Option<bool>,
    auto_reconnect: Option<bool>,
    latency_mode: Option<LatencyMode>,
    remote_volume: Option<f32>,
    remote_muted: Option<bool>,
    notifications: Option<bool>,
    hotkeys: Option<bool>,
    voice_ducking: Option<bool>,
    follow_pointer: Option<bool>,
    follow_depth: Option<f32>,
}

#[tauri::command]
fn get_settings() -> SettingsView {
    settings_view()
}

#[tauri::command]
fn update_settings(patch: SettingsPatch) -> SettingsView {
    let s = settings::update(|s| {
        if let Some(v) = patch.pc_name {
            s.pc_name = settings::clean_pc_name(&v);
        }
        if let Some(v) = patch.start_minimized {
            s.start_minimized = v;
        }
        if let Some(v) = patch.auto_reconnect {
            s.auto_reconnect = v;
        }
        if let Some(v) = patch.latency_mode {
            s.latency_mode = v;
        }
        if let Some(v) = patch.remote_volume.filter(|v| v.is_finite()) {
            s.remote_volume = v.clamp(0.0, 1.0);
        }
        if let Some(v) = patch.remote_muted {
            s.remote_muted = v;
        }
        if let Some(v) = patch.notifications {
            s.notifications = v;
        }
        if let Some(v) = patch.hotkeys {
            s.hotkeys = v;
        }
        if let Some(v) = patch.voice_ducking {
            s.voice_ducking = v;
        }
        if let Some(v) = patch.follow_pointer {
            s.follow_pointer = v;
        }
        if let Some(v) = patch.follow_depth.filter(|v| v.is_finite()) {
            s.follow_depth = v.clamp(0.0, 1.0);
        }
    });
    hotkeys::apply(s.hotkeys, on_hotkey);
    // Stopping the ducking monitor joins its thread, so keep it off the UI
    // thread. It reads the setting when it runs: quick toggles still settle
    // on the latest value, whatever order the workers run in.
    tauri::async_runtime::spawn_blocking(|| ducking::apply(settings::get().voice_ducking));
    audio::set_latency_mode(s.latency_mode);
    audio::set_remote_volume(s.remote_volume, s.remote_muted);
    let view = settings_view();
    emit("settings-changed", view.clone());
    refresh_tray();
    view
}

#[tauri::command]
fn set_autostart(enabled: bool) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    let app = APP.get().ok_or("The app is still starting")?;
    let launcher = app.autolaunch();
    let result = if enabled {
        launcher.enable()
    } else {
        launcher.disable()
    };
    result.map_err(|e| format!("Could not change Start with Windows: {e}"))?;
    refresh_tray();
    emit("settings-changed", settings_view());
    Ok(autostart_enabled())
}

#[tauri::command]
fn open_log_folder() -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let app = APP.get().ok_or("The app is still starting")?;
    let dir = app.path().app_log_dir().map_err(|e| e.to_string())?;
    let _ = std::fs::create_dir_all(&dir);
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("Could not open the log folder: {e}"))
}

/// Everything support needs in one paste: versions, state, settings (no
/// secrets) and the end of the log.
#[tauri::command]
async fn support_info() -> Result<String, String> {
    blocking(|| {
        use std::fmt::Write as _;
        let s = settings::get();
        let connection = connection::info();
        let stats = audio::stats();
        let mut out = String::new();
        let _ = writeln!(
            out,
            "Trovul {} (protocol {}.{})",
            env!("CARGO_PKG_VERSION"),
            network::PROTOCOL_VERSION_MAJOR,
            network::PROTOCOL_VERSION_MINOR
        );
        let _ = writeln!(out, "Windows: {}", windows_version());
        let _ = writeln!(
            out,
            "This PC: {} (computer {}, {}), device {}",
            network::hostname(),
            network::computer_name(),
            network::local_ip().map(|ip| ip.to_string()).unwrap_or_default(),
            s.device_id
        );
        let _ = writeln!(
            out,
            "Network: {}",
            match lan_network_profile() {
                Some(p) => format!(
                    "{:?} ({}, {})",
                    p.category,
                    p.name,
                    if p.wifi { "Wi-Fi" } else { "wired" }
                ),
                None => "unknown".into(),
            }
        );
        let _ = writeln!(
            out,
            "Connection: {}",
            match (&connection.peer_hostname, &connection.peer_ip) {
                (Some(host), Some(ip)) => format!(
                    "{host} ({ip}), {}",
                    if connection.is_initiator { "dialled" } else { "accepted" }
                ),
                _ => "not connected".into(),
            }
        );
        let _ = writeln!(
            out,
            "Audio: {:?} / {} on {}; {} Hz -> {} Hz x{}; buffer {:.0}/{:.0} ms; drift {:.0} ppm; received {} lost {} late {} underruns {}; sent {} (errors {}); last error: {}",
            stats.mode,
            stats.state,
            stats.device.as_deref().unwrap_or("-"),
            stats.input_rate.max(stats.capture_rate),
            stats.output_rate,
            stats.output_channels.max(stats.capture_channels),
            stats.buffer_ms,
            stats.target_ms,
            stats.drift_ppm,
            stats.packets_received,
            stats.packets_lost,
            stats.packets_late,
            stats.underruns,
            stats.packets_sent,
            stats.send_errors,
            stats.last_error.as_deref().unwrap_or("none")
        );
        let _ = writeln!(
            out,
            "Settings: latency {:?}, auto-reconnect {}, start minimized {}, shortcuts {}, voice ducking {}, companion {}, playback device {}",
            s.latency_mode,
            s.auto_reconnect,
            s.start_minimized,
            s.hotkeys,
            s.voice_ducking,
            s.companion_enabled,
            if s.playback_device.is_some() { "custom" } else { "default" }
        );
        let _ = writeln!(out, "Paired PCs: {}", connection::paired_devices().len());
        let _ = writeln!(out, "\n--- Recent log ---");
        out.push_str(&recent_log_lines(120));
        Ok(out)
    })
    .await
}

fn windows_version() -> String {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("cmd")
        .args(["/C", "ver"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn recent_log_lines(count: usize) -> String {
    let Some(dir) = APP.get().and_then(|app| app.path().app_log_dir().ok()) else {
        return "(log folder unavailable)\n".into();
    };
    match std::fs::read(dir.join("trovul.log")) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(count);
            lines[start..].join("\n") + "\n"
        }
        Err(e) => format!("(could not read the log: {e})\n"),
    }
}

// ============ Local mixer & devices ============

/// This PC's mixer as both PCs show it: master, apps and Trovul's own stream.
pub(crate) fn mixer_snapshot() -> Result<MixerSnapshot, String> {
    let mut mixer = audio_sessions::get_mixer_snapshot()?;
    let s = settings::get();
    let supported = audio::app_capture::supported();
    mixer.send_mix = Some(SendMix {
        supported,
        apps_only: s.send_apps_only && supported,
        chosen: s.send_apps.iter().map(|a| a.name.clone()).collect(),
    });
    if supported {
        for app in &mut mixer.sessions {
            let chosen = s
                .send_apps
                .iter()
                .find(|a| a.name.eq_ignore_ascii_case(&app.name));
            app.send = Some(AppSend {
                on: chosen.is_some(),
                level: chosen.map_or(1.0, |a| a.level),
            });
        }
    }
    let stats = audio::stats();
    if stats.mode != Mode::Off {
        let s = settings::get();
        mixer.stream = Some(StreamBus {
            focus: focus_mix::state(),
            receiving: stats.mode == Mode::Host,
            volume: s.remote_volume,
            muted: s.remote_muted,
            level: stats.level,
            ducked: stats.ducked,
            output: s.playback_device,
        });
    }
    Ok(mixer)
}

/// Send everything, or only the chosen apps (the sender switches on the fly).
pub(crate) fn set_send_mode(apps_only: bool) {
    let s = settings::update(|s| s.send_apps_only = apps_only);
    audio::app_capture::set_chosen(&s.send_apps);
    if audio::mode() == Mode::Sender {
        audio::request_restart();
    }
}

/// Choose an app to send (or not), and its level.
pub(crate) fn set_app_send(name: &str, on: bool, level: Option<f32>) {
    let name = name.trim().to_string();
    if name.is_empty() {
        return;
    }
    let s = settings::update(|s| {
        let at = s
            .send_apps
            .iter()
            .position(|a| a.name.eq_ignore_ascii_case(&name));
        match (on, at) {
            (true, Some(i)) => {
                if let Some(level) = level.filter(|l| l.is_finite()) {
                    s.send_apps[i].level = level.clamp(0.0, 1.0);
                }
            }
            (true, None) => s.send_apps.push(settings::SendApp {
                name: name.clone(),
                level: level
                    .filter(|l| l.is_finite())
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0),
            }),
            (false, Some(i)) => {
                s.send_apps.remove(i);
            }
            (false, None) => {}
        }
    });
    audio::app_capture::set_chosen(&s.send_apps);
}

#[tauri::command]
fn set_send_mode_cmd(apps_only: bool) {
    set_send_mode(apps_only);
}

#[tauri::command]
fn set_app_send_cmd(name: String, on: bool, level: Option<f32>) {
    set_app_send(&name, on, level);
}

#[tauri::command]
async fn set_remote_send_mode(apps_only: bool) -> Result<(), String> {
    connection::request(Request::SetSendMode { apps_only })
        .await
        .map(|_| ())
}

#[tauri::command]
async fn set_remote_app_send(name: String, on: bool, level: Option<f32>) -> Result<(), String> {
    connection::request(Request::SetAppSend { name, on, level })
        .await
        .map(|_| ())
}

/// Send an app to an output device (`None` = the Windows default).
pub(crate) fn route_app(pid: u32, device_id: Option<String>) -> Result<(), String> {
    if device_id
        .as_deref()
        .is_some_and(|id| !audio_sessions::is_render_device_active(id))
    {
        return Err("That output device isn't available".into());
    }
    app_routing::set_app_output(pid, device_id.as_deref())?;
    // The app moves to another device: find its session there right away.
    audio_sessions::rescan_soon();
    Ok(())
}

#[tauri::command]
async fn get_local_mixer() -> Result<MixerSnapshot, String> {
    blocking(mixer_snapshot).await
}

#[tauri::command]
async fn set_app_output(pid: u32, device_id: Option<String>) -> Result<(), String> {
    blocking(move || route_app(pid, device_id)).await
}

#[tauri::command]
async fn get_icons(ids: Vec<String>) -> Result<HashMap<String, String>, String> {
    blocking(move || Ok(audio_sessions::get_icons(&ids))).await
}

#[tauri::command]
async fn get_output_devices() -> Result<Vec<AudioDevice>, String> {
    blocking(audio_sessions::get_output_devices).await
}

#[tauri::command]
async fn get_input_devices() -> Result<Vec<AudioDevice>, String> {
    blocking(audio_sessions::get_input_devices).await
}

#[tauri::command]
async fn set_session_volume(pid: u32, volume: f32) -> Result<(), String> {
    blocking(move || audio_sessions::set_session_volume(pid, volume)).await
}

#[tauri::command]
async fn set_session_mute(pid: u32, muted: bool) -> Result<(), String> {
    blocking(move || audio_sessions::set_session_mute(pid, muted)).await
}

#[tauri::command]
async fn set_master_volume(volume: f32) -> Result<(), String> {
    blocking(move || audio_sessions::set_master_volume(volume)).await
}

#[tauri::command]
async fn set_master_mute(muted: bool) -> Result<(), String> {
    blocking(move || audio_sessions::set_master_mute(muted)).await
}

#[tauri::command]
async fn set_device_volume(device_id: String, volume: f32) -> Result<(), String> {
    blocking(move || audio_sessions::set_device_volume(&device_id, volume)).await
}

#[tauri::command]
async fn set_device_mute(device_id: String, muted: bool) -> Result<(), String> {
    blocking(move || audio_sessions::set_device_mute(&device_id, muted)).await
}

// ============ Remote mixer ============

#[tauri::command]
async fn get_remote_mixer() -> Result<MixerSnapshot, String> {
    match connection::request(Request::GetMixer).await? {
        Response::Mixer { mixer } => Ok(mixer),
        _ => Err(UNEXPECTED_ANSWER.into()),
    }
}

#[tauri::command]
async fn get_remote_icons(ids: Vec<String>) -> Result<HashMap<String, String>, String> {
    match connection::request(Request::GetIcons { ids }).await? {
        Response::Icons { icons } => Ok(icons),
        _ => Err(UNEXPECTED_ANSWER.into()),
    }
}

#[tauri::command]
async fn set_remote_session_volume(pid: u32, volume: f32) -> Result<(), String> {
    connection::request(Request::SetSessionVolume { pid, volume })
        .await
        .map(|_| ())
}

#[tauri::command]
async fn set_remote_session_mute(pid: u32, muted: bool) -> Result<(), String> {
    connection::request(Request::SetSessionMute { pid, muted })
        .await
        .map(|_| ())
}

#[tauri::command]
async fn set_remote_master_volume(volume: f32) -> Result<(), String> {
    connection::request(Request::SetMasterVolume { volume })
        .await
        .map(|_| ())
}

#[tauri::command]
async fn set_remote_master_mute(muted: bool) -> Result<(), String> {
    connection::request(Request::SetMasterMute { muted })
        .await
        .map(|_| ())
}

#[tauri::command]
async fn get_remote_output_devices() -> Result<Vec<AudioDevice>, String> {
    match connection::request(Request::GetOutputDevices).await? {
        Response::Devices { devices } => Ok(devices),
        _ => Err(UNEXPECTED_ANSWER.into()),
    }
}

#[tauri::command]
async fn set_remote_app_output(pid: u32, device_id: Option<String>) -> Result<(), String> {
    connection::request(Request::SetAppOutput {
        pid,
        device: device_id,
    })
    .await
    .map(|_| ())
}

/// Volume / mute of the sound the other PC receives from this one.
#[tauri::command]
async fn set_remote_incoming(volume: Option<f32>, muted: Option<bool>) -> Result<(), String> {
    connection::request(Request::SetIncoming { volume, muted })
        .await
        .map(|_| ())
}

#[tauri::command]
async fn set_remote_playback_device(device_id: Option<String>) -> Result<(), String> {
    connection::request(Request::SetPlaybackDevice { device: device_id })
        .await
        .map(|_| ())
}

// ============ Companion API ============

#[tauri::command]
fn get_companion_status() -> CompanionStatus {
    companion_server::status()
}

#[tauri::command]
fn set_companion_enabled(enabled: bool) -> CompanionStatus {
    companion_server::set_enabled(enabled)
}

#[tauri::command]
fn regenerate_companion_token() -> CompanionStatus {
    companion_server::regenerate_token()
}

// ============ Lifecycle ============

/// Detect resume-from-sleep by watching wall-clock gaps: our 2 s loop "wakes
/// up" tens of seconds late after a suspend. On detection the audio engine
/// rebuilds its WASAPI/cpal handles, which Windows invalidates during suspend.
/// Simpler than hooking WM_POWERBROADCAST on the webview window.
fn start_resume_watchdog() {
    static LAST_TICK: AtomicU64 = AtomicU64::new(0);
    LAST_TICK.store(util::now_ms(), Ordering::Relaxed);
    let spawned = std::thread::Builder::new()
        .name("trovul-resume-watchdog".into())
        .spawn(|| loop {
            std::thread::sleep(Duration::from_secs(2));
            let now = util::now_ms();
            let gap_ms = now.saturating_sub(LAST_TICK.swap(now, Ordering::Relaxed));
            if gap_ms > 10_000 {
                log::warn!(
                    "Detected a {:.1}s wall-clock gap — assuming resume from sleep, restarting audio",
                    gap_ms as f64 / 1000.0
                );
                audio::request_restart();
            }
        });
    if let Err(e) = spawned {
        log::error!("Could not start the resume watchdog: {e}");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri_plugin_log::{RotationStrategy, Target, TargetKind};

    // A release build has no console, so a panicking background thread (audio,
    // discovery, link reader) would die without a trace: log it, so the log
    // file and "Copy support info" show what happened.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        log::error!("Thread '{}' {info}", thread.name().unwrap_or("unnamed"));
        default_hook(info);
    }));

    tauri::Builder::default()
        // Must be the first plugin: a second launch focuses this instance and
        // exits instead of starting a copy that can't bind the ports.
        .plugin(tauri_plugin_single_instance::init(|_app, _argv, _cwd| {
            log::info!("Second launch — focusing the running instance");
            show_main_window();
        }))
        // Logs under %LOCALAPPDATA%\com.trovul.app\logs. Size-based rotation:
        // 5 MB per file, 5 old files kept — at most ~30 MB on disk.
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    Target::new(TargetKind::Stdout),
                    Target::new(TargetKind::LogDir {
                        file_name: Some("trovul".into()),
                    }),
                ])
                .level(log::LevelFilter::Info)
                .max_file_size(5_000_000)
                .rotation_strategy(RotationStrategy::KeepSome(5))
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let _ = APP.set(app.handle().clone());
            settings::init(app.path().app_data_dir()?);
            if let Ok(dir) = app.path().app_local_data_dir() {
                update::clean_downloads(&dir.join("updates"));
            }
            clipboard::start();
            input_service::apply();
            focus_mix::start();
            audio::app_capture::set_chosen(&settings::get().send_apps);
            let s = settings::get();
            audio::set_output_device(s.playback_device.clone());
            audio::set_latency_mode(s.latency_mode);
            audio::set_remote_volume(s.remote_volume, s.remote_muted);

            #[cfg(desktop)]
            {
                use tauri_plugin_autostart::ManagerExt;
                app.handle().plugin(tauri_plugin_autostart::init(
                    tauri_plugin_autostart::MacosLauncher::LaunchAgent,
                    Some(vec![AUTOSTART_FLAG]),
                ))?;
                // Entries written by older builds lack the flag: rewrite them.
                // Release builds only — `enable()` records the current exe, and
                // a dev build must not take over the installed app's entry.
                if cfg!(not(debug_assertions)) && app.autolaunch().is_enabled().unwrap_or(false) {
                    let _ = app.autolaunch().enable();
                }
            }

            connection::start();
            network::start_discovery();
            start_resume_watchdog();
            companion_server::apply(s.companion_enabled);
            hotkeys::apply(s.hotkeys, on_hotkey);
            ducking::apply(s.voice_ducking);
            build_tray(app)?;
            refresh_tray();

            // The window is created hidden; show it unless Windows started us
            // at sign-in and the user prefers to stay in the tray.
            let at_sign_in = std::env::args().any(|arg| arg == AUTOSTART_FLAG);
            if at_sign_in && s.start_minimized {
                log::info!("Started at sign-in; staying in the tray");
            } else {
                show_main_window();
            }
            log::info!("Trovul {} ready", env!("CARGO_PKG_VERSION"));
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == flyout::LABEL {
                // The tray mixer goes away like the Windows volume flyout.
                if let Some(flyout) = window.app_handle().get_webview_window(flyout::LABEL) {
                    match event {
                        tauri::WindowEvent::Focused(false) => flyout::hide(&flyout),
                        tauri::WindowEvent::CloseRequested { api, .. } => {
                            api.prevent_close();
                            flyout::hide(&flyout);
                        }
                        _ => {}
                    }
                }
                return;
            }
            // Closing the window hides it to the tray; quitting is in the tray menu.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                emit("window-visibility", false);
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_app_info,
            get_window_visible,
            get_connection_info,
            get_discovered_peers,
            request_connection,
            accept_connection,
            reject_connection,
            disconnect,
            get_paired_devices,
            forget_paired_device,
            get_stream_direction,
            set_stream_direction,
            play_test_sound,
            get_audio_stats,
            get_remote_audio_stats,
            get_playback_device,
            set_playback_device,
            get_settings,
            update_settings,
            set_autostart,
            open_log_folder,
            support_info,
            get_network_profile,
            open_network_settings,
            get_input_view,
            refresh_input,
            set_input_sharing,
            set_input_layout,
            reset_input_layout,
            get_update_state,
            install_update,
            dismiss_update,
            get_local_mixer,
            get_icons,
            get_output_devices,
            get_input_devices,
            set_session_volume,
            set_session_mute,
            set_master_volume,
            set_master_mute,
            set_device_volume,
            set_device_mute,
            get_remote_mixer,
            get_remote_icons,
            set_remote_session_volume,
            set_remote_session_mute,
            set_remote_master_volume,
            set_remote_master_mute,
            set_app_output,
            get_remote_output_devices,
            set_remote_app_output,
            set_remote_incoming,
            set_remote_playback_device,
            get_clipboard_view,
            set_clipboard_sharing,
            set_input_options,
            set_admin_input,
            set_send_mode_cmd,
            set_app_send_cmd,
            set_remote_send_mode,
            set_remote_app_send,
            open_main_window,
            hide_flyout,
            get_popups,
            resize_popups,
            hide_popups,
            popup_action,
            get_mix_scenes,
            set_mix_scenes,
            capture_scene,
            apply_scene,
            get_companion_status,
            set_companion_enabled,
            regenerate_companion_token,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
