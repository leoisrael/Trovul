// Trovul - Keyboard & mouse sharing: one mouse and keyboard for both PCs,
// following the screen arrangement the user draws in the app.
//
// - layout.rs   the shared arrangement and the pointer's path (pure, tested)
// - monitors.rs this PC's monitors
// - hook.rs     the controlling side (low-level hooks)
// - inject.rs   the controlled side (SendInput)
//
// Input travels on the paired control link as one-way `Frame::Input` frames.
// Both PCs must switch sharing on; the arrangement is synced between them and
// the most recent edit wins.

mod focus;
pub mod hook;
mod inject;
pub mod layout;
pub mod monitors;

use crate::connection;
use crate::peer::{Frame, Request, Response};
use crate::settings::{self, InputLayoutSetting};
use crate::util::{lock, now_ms, read, write};
pub use layout::{Monitor, Placement, SharedLayout};
use layout::{PcScreens, World};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// One-way messages between the two PCs' input engines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "e", rename_all = "snake_case")]
pub enum InputEvent {
    /// The pointer arrives on the receiving PC at (x, y), its own pixels.
    Enter {
        x: i32,
        y: i32,
    },
    Move {
        x: i32,
        y: i32,
    },
    Button {
        button: MouseButton,
        down: bool,
    },
    Wheel {
        delta: i32,
        horizontal: bool,
    },
    Key {
        vk: u16,
        scan: u16,
        extended: bool,
        down: bool,
    },
    /// The pointer went back to the sending PC.
    Leave,
    /// Sent by the controlled PC: its own mouse moved, so take the pointer back.
    Release,
    /// My monitors or my sharing switch changed: ask me again.
    Changed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Sharing is switched off on this PC.
    Off,
    /// No PC linked.
    Waiting,
    /// The linked PC runs a Trovul without keyboard & mouse sharing.
    Unsupported,
    /// The linked PC hasn't switched sharing on.
    PeerOff,
    Ready,
    /// The pointer is on the other PC.
    Controlling,
    /// The other PC is using this one.
    Controlled,
}

pub(crate) enum Outgoing {
    Event(InputEvent),
    /// Tell the UI the status changed.
    StatusChanged,
    /// Let go of keys the other PC was holding (done off the hook thread).
    ReleaseHeld,
}

struct PeerScreens {
    device_id: String,
    hostname: String,
    enabled: bool,
    monitors: Vec<Monitor>,
}

/// The linked PC's side, once it answered. None: unknown or unsupported.
static PEER: Mutex<Option<PeerScreens>> = Mutex::new(None);
static UNSUPPORTED: AtomicBool = AtomicBool::new(false);
static WORLD: RwLock<Option<Arc<World>>> = RwLock::new(None);
static CONTROLLED: AtomicBool = AtomicBool::new(false);
/// Both PCs have sharing on (cached: the hook checks it on every mouse move).
static READY: AtomicBool = AtomicBool::new(false);
// The user's options, read by the hook on every move (no settings clone there).
static FOCUS_FOLLOWS: AtomicBool = AtomicBool::new(true);
static GAME_MODE: AtomicBool = AtomicBool::new(true);
static GUARD_CORNERS: AtomicBool = AtomicBool::new(true);
/// The sharing switch, read for every event from the other PC.
static SHARING: AtomicBool = AtomicBool::new(false);
/// The pointer is on this PC (not on the other one). Follows the crossings,
/// and this PC's own mouse moving.
static POINTER_HERE: AtomicBool = AtomicBool::new(true);

pub fn pointer_here() -> bool {
    POINTER_HERE.load(Ordering::Acquire)
}

pub(crate) fn set_pointer_here(here: bool) {
    if POINTER_HERE.load(Ordering::Relaxed) != here {
        POINTER_HERE.store(here, Ordering::Release);
    }
}
/// Game-mode verdict and when it was taken (a flick can try to cross many
/// times a second; asking Windows each time is wasted work).
static GAME_CHECK: Mutex<(u64, bool)> = Mutex::new((0, false));
static OUTBOX: Mutex<Option<Sender<Outgoing>>> = Mutex::new(None);
static REFRESHING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn world() -> Option<Arc<World>> {
    read(&WORLD).clone()
}

/// Both PCs have sharing on and know each other's screens.
fn load_options() {
    let s = settings::get();
    FOCUS_FOLLOWS.store(s.input_focus_follows, Ordering::Release);
    GAME_MODE.store(s.input_game_mode, Ordering::Release);
    GUARD_CORNERS.store(s.input_guard_corners, Ordering::Release);
    SHARING.store(s.input_sharing, Ordering::Release);
}

/// How the pointer may cross right now.
pub(crate) fn rules(holding: bool) -> layout::MoveRules {
    layout::MoveRules {
        holding,
        guard_corners: GUARD_CORNERS.load(Ordering::Acquire),
    }
}

/// The pointer must stay on this PC: Scroll Lock is on, or game mode sees a game in front.
pub(crate) fn pointer_stays() -> bool {
    if focus::scroll_locked() {
        return true;
    }
    if !GAME_MODE.load(Ordering::Acquire) {
        return false;
    }
    let now = crate::util::mono_ms();
    let mut check = lock(&GAME_CHECK);
    if check.0 == 0 || now.saturating_sub(check.0) > 500 {
        *check = (now, focus::game_in_front());
    }
    check.1
}

/// The pointer arrived on this PC at `p`: the keyboard follows (if wanted).
pub(crate) fn pointer_arrived(p: layout::Point) {
    if FOCUS_FOLLOWS.load(Ordering::Acquire) {
        focus::keyboard_to_window_at(p);
    }
}

/// Nothing should pop up over what's in front (see focus.rs).
pub fn exclusive_full_screen() -> bool {
    focus::exclusive_full_screen()
}

pub(crate) fn scroll_locked() -> bool {
    focus::scroll_locked()
}

pub(crate) fn any_button_down() -> bool {
    focus::any_button_down()
}

pub fn remote_ready() -> bool {
    READY.load(Ordering::Acquire)
}

pub fn controlled() -> bool {
    CONTROLLED.load(Ordering::Acquire)
}

fn outbox() -> Sender<Outgoing> {
    let mut slot = lock(&OUTBOX);
    if let Some(tx) = slot.as_ref() {
        return tx.clone();
    }
    let (tx, rx) = mpsc::channel();
    if let Err(e) = std::thread::Builder::new()
        .name("trovul-input-send".into())
        .spawn(move || run_outbox(rx))
    {
        log::error!("Could not start the input sender: {e}");
    }
    *slot = Some(tx.clone());
    tx
}

/// Sends the hook's events without ever blocking the hook itself. Consecutive
/// moves are merged: only the latest position matters.
fn run_outbox(rx: Receiver<Outgoing>) {
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        while let Ok(next) = rx.try_recv() {
            batch.push(next);
        }
        for (i, item) in batch.iter().enumerate() {
            match item {
                Outgoing::Event(InputEvent::Move { .. })
                    if matches!(
                        batch.get(i + 1),
                        Some(Outgoing::Event(InputEvent::Move { .. }))
                    ) => {}
                Outgoing::Event(event) => {
                    let sent = connection::current().is_some_and(|link| {
                        link.send(&Frame::Input {
                            event: event.clone(),
                        })
                    });
                    if !sent && hook::controlling() {
                        hook::release(false);
                    }
                }
                Outgoing::StatusChanged => crate::on_input_changed(),
                Outgoing::ReleaseHeld => inject::release_all(),
            }
        }
    }
}

// ---------- Keeping both PCs in sync ----------

/// Ask the linked PC for its screens, sharing switch and arrangement, settle
/// on one arrangement, and start or stop the hooks. Safe to call any time.
pub async fn refresh() {
    let _one_at_a_time = REFRESHING.lock().await;
    let Some(link) = connection::current() else {
        forget_peer();
        return;
    };
    let peer_id = link.info().peer_device_id.clone();
    let hostname = link.info().peer_hostname.clone();
    match connection::request(Request::GetInputInfo).await {
        Ok(Response::InputInfo {
            enabled,
            monitors,
            layout,
        }) => {
            UNSUPPORTED.store(false, Ordering::Release);
            *lock(&PEER) = Some(PeerScreens {
                device_id: peer_id.clone(),
                hostname,
                enabled,
                monitors,
            });
            settle_layout(&peer_id, layout).await;
        }
        Ok(_) | Err(_) => {
            // An older Trovul answers "Unsupported request".
            UNSUPPORTED.store(true, Ordering::Release);
            *lock(&PEER) = None;
        }
    }
    rebuild().await;
}

/// The newest arrangement wins; the other PC gets ours if ours is newer.
async fn settle_layout(peer_id: &str, theirs: Option<SharedLayout>) {
    let mine = stored_layout(peer_id);
    let theirs_updated = theirs.as_ref().map_or(0, |l| l.updated);
    match (mine, theirs) {
        (Some(mine), _) if mine.updated > theirs_updated => {
            let _ = connection::request(Request::SetInputLayout { layout: mine }).await;
        }
        (mine, Some(theirs)) if theirs.updated > mine.as_ref().map_or(0, |l| l.updated) => {
            store_layout(peer_id, theirs);
        }
        _ => {}
    }
}

fn stored_layout(peer_id: &str) -> Option<SharedLayout> {
    settings::get()
        .input_layout
        .filter(|l| l.peer == peer_id)
        .map(|l| l.layout)
}

fn store_layout(peer_id: &str, layout: SharedLayout) {
    settings::update(|s| {
        s.input_layout = Some(InputLayoutSetting {
            peer: peer_id.to_string(),
            layout,
        });
    });
}

/// Rebuild the arrangement from both PCs' screens and start/stop the hooks.
async fn rebuild() {
    let local = tauri::async_runtime::spawn_blocking(monitors::enumerate)
        .await
        .unwrap_or_default();
    let me = settings::device_id();
    let world = {
        let peer = lock(&PEER);
        peer.as_ref().map(|p| {
            let placements = stored_layout(&p.device_id)
                .map(|l| l.placements)
                .unwrap_or_default();
            Arc::new(World::build(
                PcScreens {
                    pc: &me,
                    monitors: &local,
                },
                PcScreens {
                    pc: &p.device_id,
                    monitors: &p.monitors,
                },
                &placements,
            ))
        })
    };
    load_options();
    let run = world.is_some() && settings::get().input_sharing;
    let peer_on = lock(&PEER).as_ref().is_some_and(|p| p.enabled);
    *write(&WORLD) = world;
    READY.store(run && peer_on, Ordering::Release);
    if run {
        hook::start(outbox());
    } else {
        stop_hooks();
    }
    crate::on_input_changed();
}

fn stop_hooks() {
    if hook::controlling() {
        hook::release(true);
    }
    hook::stop();
}

fn forget_peer() {
    READY.store(false, Ordering::Release);
    *lock(&PEER) = None;
    UNSUPPORTED.store(false, Ordering::Release);
    *write(&WORLD) = None;
    stop_hooks();
    if CONTROLLED.swap(false, Ordering::AcqRel) {
        inject::release_all();
    }
    crate::on_input_changed();
}

/// The link closed: the pointer comes home and nothing stays pressed.
pub fn link_closed() {
    forget_peer();
}

/// Tell the linked PC to ask us again (switch or monitors changed).
fn announce_change() {
    let _ = outbox().send(Outgoing::Event(InputEvent::Changed));
}

/// Windows' display settings changed (from the hook's window).
pub fn displays_changed() {
    announce_change();
    tauri::async_runtime::spawn(refresh());
}

// ---------- Commands from the UI ----------

pub async fn set_enabled(enabled: bool) {
    settings::update(|s| s.input_sharing = enabled);
    announce_change();
    refresh().await;
}

pub async fn set_layout(placements: Vec<Placement>) -> Result<(), String> {
    let peer_id = lock(&PEER)
        .as_ref()
        .map(|p| p.device_id.clone())
        .ok_or("Link a PC first")?;
    let layout = SharedLayout {
        updated: now_ms(),
        placements,
    };
    store_layout(&peer_id, layout.clone());
    rebuild().await;
    connection::request(Request::SetInputLayout { layout })
        .await
        .map(|_| ())
}

/// The keyboard-follows, game-mode and corner options (each PC has its own).
pub fn set_options(
    focus_follows: Option<bool>,
    game_mode: Option<bool>,
    guard_corners: Option<bool>,
) {
    settings::update(|s| {
        if let Some(v) = focus_follows {
            s.input_focus_follows = v;
        }
        if let Some(v) = game_mode {
            s.input_game_mode = v;
        }
        if let Some(v) = guard_corners {
            s.input_guard_corners = v;
        }
    });
    load_options();
    *lock(&GAME_CHECK) = (0, false);
}

/// Back to the default: the other PC to the right of this one.
pub async fn reset_layout() -> Result<(), String> {
    set_layout(Vec::new()).await
}

// ---------- The linked PC's requests and events ----------

pub fn info_for(peer_id: &str) -> Response {
    Response::InputInfo {
        enabled: settings::get().input_sharing,
        monitors: monitors::enumerate(),
        layout: stored_layout(peer_id),
    }
}

pub fn layout_from_peer(peer_id: &str, layout: SharedLayout) {
    let newer = stored_layout(peer_id).is_none_or(|mine| layout.updated >= mine.updated);
    if newer {
        store_layout(peer_id, layout);
        tauri::async_runtime::spawn(rebuild());
    }
}

pub fn handle_event(event: InputEvent) {
    match event {
        InputEvent::Changed => {
            tauri::async_runtime::spawn(refresh());
        }
        InputEvent::Release => hook::release(false),
        InputEvent::Leave => {
            set_pointer_here(false);
            replay(&event);
            if CONTROLLED.swap(false, Ordering::AcqRel) {
                crate::on_input_changed();
            }
        }
        _ if !SHARING.load(Ordering::Acquire) => {}
        InputEvent::Enter { x, y } => {
            set_pointer_here(true);
            replay(&event);
            pointer_arrived(layout::Point::new(x, y));
            if !CONTROLLED.swap(true, Ordering::AcqRel) {
                crate::on_input_changed();
            }
        }
        InputEvent::Key { down: true, .. } | InputEvent::Button { down: true, .. } => {
            if replay(&event) {
                return; // the input helper reaches admin windows
            }
            // Windows silently drops input sent to an admin window (UIPI): say so once.
            if let Some(window) = focus::blocked_admin_window() {
                crate::on_input_notice(format!(
                    "The other PC can't type or click in “{window}”: it runs as administrator. Turn on “Admin windows, UAC and lock screen” in Keyboard & Mouse on this PC."
                ));
            }
        }
        other => {
            replay(&other);
        }
    }
}

/// Replay the other PC's input on this PC: through the input helper when it's
/// on (admin windows, UAC, lock screen), else directly. True = the helper did it.
fn replay(event: &InputEvent) -> bool {
    if crate::input_service::forward(event) {
        if matches!(event, InputEvent::Leave) {
            inject::release_all(); // anything injected directly before the helper was up
        }
        return true;
    }
    replay_here(event);
    false
}

/// Inject one event into this session (also what the input helper runs).
pub fn replay_here(event: &InputEvent) {
    match event {
        InputEvent::Leave => inject::release_all(),
        other => inject::apply(other),
    }
}

/// This PC's own mouse moved while the other PC was using it: hand it back
/// (called from the hook thread, so nothing here may block).
pub(crate) fn local_input_while_controlled() {
    if CONTROLLED.swap(false, Ordering::AcqRel) {
        let tx = outbox();
        let _ = tx.send(Outgoing::ReleaseHeld);
        let _ = tx.send(Outgoing::Event(InputEvent::Release));
        let _ = tx.send(Outgoing::StatusChanged);
    }
}

// ---------- What the UI shows ----------

#[derive(Debug, Clone, Serialize)]
pub struct ScreenView {
    pub pc: String,
    pub local: bool,
    pub id: String,
    pub number: u32,
    pub name: String,
    pub primary: bool,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct InputView {
    pub enabled: bool,
    /// Admin windows, UAC and the lock screen (the input service).
    pub admin_input: crate::input_service::HelperState,
    pub focus_follows: bool,
    pub game_mode: bool,
    pub guard_corners: bool,
    pub status: Status,
    pub this_pc: String,
    pub peer_name: Option<String>,
    pub screens: Vec<ScreenView>,
    /// A screen of each PC touches the other: the pointer can cross.
    pub reachable: bool,
}

pub fn status() -> Status {
    let s = settings::get();
    if !s.input_sharing {
        Status::Off
    } else if connection::current().is_none() {
        Status::Waiting
    } else if UNSUPPORTED.load(Ordering::Acquire) {
        Status::Unsupported
    } else if !lock(&PEER).as_ref().is_some_and(|p| p.enabled) {
        Status::PeerOff
    } else if hook::controlling() {
        Status::Controlling
    } else if controlled() {
        Status::Controlled
    } else {
        Status::Ready
    }
}

pub fn view() -> InputView {
    let me = settings::device_id();
    // Without a linked PC, show this PC's screens on their own.
    let world = world().unwrap_or_else(|| {
        let local = monitors::enumerate();
        Arc::new(World::build(
            PcScreens {
                pc: &me,
                monitors: &local,
            },
            PcScreens {
                pc: "",
                monitors: &[],
            },
            &[],
        ))
    });
    let screens = world
        .screens
        .iter()
        .map(|s| ScreenView {
            pc: s.pc.clone(),
            local: s.local,
            id: s.monitor.id.clone(),
            number: s.monitor.number,
            name: s.monitor.name.clone(),
            primary: s.monitor.primary,
            x: s.world.x,
            y: s.world.y,
            width: s.world.width,
            height: s.world.height,
        })
        .collect();
    InputView {
        enabled: settings::get().input_sharing,
        admin_input: crate::input_service::state(),
        focus_follows: settings::get().input_focus_follows,
        game_mode: settings::get().input_game_mode,
        guard_corners: settings::get().input_guard_corners,
        status: status(),
        this_pc: me,
        peer_name: lock(&PEER).as_ref().map(|p| p.hostname.clone()),
        screens,
        reachable: world.remote_reachable(),
    }
}
