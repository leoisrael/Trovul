// Trovul - Companion API: an opt-in WebSocket server (5534) plus a UDP
// discovery responder (5533) for external controllers such as a phone app.
//
// Disabled by default. Clients connect to `ws://<host>:5534/?token=<token>`;
// the token is shown in the app and checked during the WebSocket handshake,
// which also keeps web pages open in a local browser from driving the mixer.
//
// Messages are JSON `{ "type", "payload", "timestamp" }`. Requests:
// get_sessions, get_devices, set_volume, set_mute, set_master_volume,
// set_master_mute, mute_all, unmute_all — each accepts `"remote": true` to act
// on the connected PC instead. For controllers like the Stream Deck plugin
// (streamdeck/): get_state, apply_scene {id}, set_direction {direction},
// set_follow_pointer {on}, and adjust {target: master|incoming|app, name?,
// remote?, delta?, volume?, mute?: true|false|"toggle"}, which answers with
// `target` (volume, muted). Pushed: audio_sessions (1 Hz), peak_update
// (10 Hz), connection_status, state (on changes), error.
//
// While the server runs, `companion.json` (port, token) sits in the app-data
// folder so a plugin on this PC connects without asking for the token.

use crate::audio_sessions::{self, MixerSnapshot};
use crate::connection;
use crate::network::{self, COMPANION_DISCOVERY_PORT, COMPANION_WS_PORT};
use crate::peer::{Request, Response};
use crate::settings;
use crate::util::{lock, now_ms, random_hex};
use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch};
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HandshakeRequest, Response as HandshakeResponse,
};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Clone, Serialize)]
pub struct CompanionStatus {
    pub enabled: bool,
    pub running: bool,
    pub ws_port: u16,
    pub discovery_port: u16,
    pub connected_clients: usize,
    pub token: Option<String>,
    pub last_error: Option<String>,
}

struct Server {
    shutdown: watch::Sender<bool>,
    discovery_stop: Arc<AtomicBool>,
}

static SERVER: Mutex<Option<Server>> = Mutex::new(None);
static LISTENING: AtomicBool = AtomicBool::new(false);
static CLIENTS: AtomicUsize = AtomicUsize::new(0);
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);
static BROADCAST: LazyLock<broadcast::Sender<String>> = LazyLock::new(|| broadcast::channel(64).0);

// ============ Public API ============

pub fn status() -> CompanionStatus {
    let settings = settings::get();
    CompanionStatus {
        enabled: settings.companion_enabled,
        running: LISTENING.load(Ordering::Acquire),
        ws_port: COMPANION_WS_PORT,
        discovery_port: COMPANION_DISCOVERY_PORT,
        connected_clients: CLIENTS.load(Ordering::Relaxed),
        token: settings.companion_token,
        last_error: lock(&LAST_ERROR).clone(),
    }
}

pub fn set_enabled(enabled: bool) -> CompanionStatus {
    let settings = settings::update(|s| {
        s.companion_enabled = enabled;
        if enabled && s.companion_token.is_none() {
            s.companion_token = Some(random_hex::<16>());
        }
    });
    apply(settings.companion_enabled);
    status()
}

/// New token; connected clients are dropped and must reconnect with it.
pub fn regenerate_token() -> CompanionStatus {
    let settings = settings::update(|s| s.companion_token = Some(random_hex::<16>()));
    if settings.companion_enabled {
        stop();
        start();
    }
    status()
}

/// Start or stop the server to match the setting.
pub fn apply(enabled: bool) {
    if enabled {
        start();
    } else {
        stop();
    }
}

/// Push the current connection state to companion clients.
pub fn notify_connection_changed() {
    if CLIENTS.load(Ordering::Relaxed) > 0 {
        let _ = BROADCAST.send(connection_status_message());
        let _ = BROADCAST.send(state_message());
    }
}

/// Direction, incoming volume, scenes… changed: tell companion clients.
pub fn notify_state() {
    if CLIENTS.load(Ordering::Relaxed) > 0 {
        let _ = BROADCAST.send(state_message());
    }
}

fn state_message() -> String {
    let s = settings::get();
    let info = connection::info();
    let direction = crate::audio::Direction::from_local_mode(crate::audio::mode());
    envelope(
        "state",
        json!({
            "connected": info.connected,
            "peer_hostname": info.peer_hostname,
            "direction": direction,
            "incoming": { "volume": s.remote_volume, "muted": s.remote_muted },
            "follow_pointer": s.follow_pointer,
            "send_apps_only": s.send_apps_only,
            "scenes": s.mix_scenes.iter().map(|sc| json!({ "id": sc.id, "name": sc.name })).collect::<Vec<_>>(),
        }),
    )
}

/// The file a plugin on this PC reads to connect (removed when the server stops).
fn token_file() -> Option<std::path::PathBuf> {
    settings::data_file("companion.json")
}

fn write_token_file(token: &str) {
    if let Some(path) = token_file() {
        let body = json!({ "port": COMPANION_WS_PORT, "token": token }).to_string();
        if let Err(e) = std::fs::write(&path, body) {
            log::warn!("[Companion] Could not write {}: {e}", path.display());
        }
    }
}

fn start() {
    let mut server = lock(&SERVER);
    if server.is_some() {
        return;
    }
    let token = match settings::get().companion_token {
        Some(token) => token,
        None => settings::update(|s| s.companion_token = Some(random_hex::<16>()))
            .companion_token
            .unwrap_or_default(),
    };
    let (shutdown, shutdown_rx) = watch::channel(false);
    let discovery_stop = Arc::new(AtomicBool::new(false));
    spawn_discovery_responder(discovery_stop.clone());
    write_token_file(&token);
    tauri::async_runtime::spawn(serve(Arc::new(token), shutdown_rx));
    *server = Some(Server {
        shutdown,
        discovery_stop,
    });
}

fn stop() {
    if let Some(server) = lock(&SERVER).take() {
        log::info!("[Companion] Stopping");
        if let Some(path) = token_file() {
            let _ = std::fs::remove_file(path);
        }
        let _ = server.shutdown.send(true);
        server.discovery_stop.store(true, Ordering::Release);
    }
}

// ============ WebSocket server ============

async fn serve(token: Arc<String>, mut shutdown: watch::Receiver<bool>) {
    let listener = match TcpListener::bind(("0.0.0.0", COMPANION_WS_PORT)).await {
        Ok(listener) => listener,
        Err(e) => {
            let message = format!("Could not listen on port {COMPANION_WS_PORT}: {e}");
            log::error!("[Companion] {message}");
            *lock(&LAST_ERROR) = Some(message);
            return;
        }
    };
    *lock(&LAST_ERROR) = None;
    LISTENING.store(true, Ordering::Release);
    log::info!("[Companion] WebSocket API listening on port {COMPANION_WS_PORT}");
    tokio::spawn(broadcast_loop(shutdown.clone()));

    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, addr)) => {
                    tokio::spawn(handle_client(stream, addr, token.clone(), shutdown.clone()));
                }
                Err(e) => {
                    log::warn!("[Companion] Accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            _ = shutdown.changed() => break,
        }
    }
    LISTENING.store(false, Ordering::Release);
    log::info!("[Companion] WebSocket API stopped");
}

fn token_matches(query: Option<&str>, expected: &str) -> bool {
    let Some(given) = query
        .unwrap_or_default()
        .split('&')
        .find_map(|pair| pair.strip_prefix("token="))
    else {
        return false;
    };
    // Constant-time comparison.
    given.len() == expected.len()
        && given
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

// The handshake callback's error type is tungstenite's HTTP response.
#[allow(clippy::result_large_err)]
async fn handle_client(
    stream: TcpStream,
    addr: SocketAddr,
    token: Arc<String>,
    mut shutdown: watch::Receiver<bool>,
) {
    let authorize = move |request: &HandshakeRequest, response: HandshakeResponse| {
        if token_matches(request.uri().query(), &token) {
            Ok(response)
        } else {
            let mut denied = ErrorResponse::new(Some("Invalid or missing token".into()));
            *denied.status_mut() = StatusCode::UNAUTHORIZED;
            Err(denied)
        }
    };
    let ws = match accept_hdr_async(stream, authorize).await {
        Ok(ws) => ws,
        Err(e) => {
            log::info!("[Companion] Refused {addr}: {e}");
            return;
        }
    };
    let clients = CLIENTS.fetch_add(1, Ordering::Relaxed) + 1;
    log::info!("[Companion] {addr} connected ({clients} client(s))");

    let (mut write, mut read) = ws.split();
    let mut broadcasts = BROADCAST.subscribe();
    let mut initial = vec![connection_status_message(), state_message()];
    if let Ok(sessions) = sessions_message(false).await {
        initial.push(sessions);
    }
    for message in initial {
        if write.send(Message::Text(message)).await.is_err() {
            CLIENTS.fetch_sub(1, Ordering::Relaxed);
            return;
        }
    }

    loop {
        tokio::select! {
            incoming = read.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if let Some(reply) = handle_message(&text).await {
                        if write.send(Message::Text(reply)).await.is_err() {
                            break;
                        }
                    }
                }
                Some(Ok(Message::Ping(data))) => {
                    let _ = write.send(Message::Pong(data)).await;
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            pushed = broadcasts.recv() => match pushed {
                Ok(text) => {
                    if write.send(Message::Text(text)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = shutdown.changed() => {
                let _ = write.send(Message::Close(None)).await;
                break;
            }
        }
    }
    let remaining = CLIENTS.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
    log::info!("[Companion] {addr} disconnected ({remaining} remaining)");
}

// ============ Messages ============

#[derive(Debug, Deserialize)]
struct WsMessage {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    payload: Value,
}

#[derive(Deserialize)]
struct VolumePayload {
    pid: u32,
    volume: f32,
    #[serde(default)]
    remote: bool,
}

#[derive(Deserialize)]
struct MutePayload {
    pid: u32,
    muted: bool,
    #[serde(default)]
    remote: bool,
}

#[derive(Deserialize)]
struct MasterVolumePayload {
    volume: f32,
    #[serde(default)]
    remote: bool,
}

#[derive(Deserialize)]
struct MasterMutePayload {
    muted: bool,
    #[serde(default)]
    remote: bool,
}

#[derive(Deserialize)]
struct ScenePayload {
    id: String,
}

#[derive(Deserialize)]
struct DirectionPayload {
    direction: crate::audio::Direction,
}

#[derive(Deserialize)]
struct FollowPayload {
    on: bool,
}

/// `mute`: true, false or "toggle".
#[derive(Deserialize, Clone)]
#[serde(untagged)]
enum MuteChange {
    Set(bool),
    Toggle(String),
}

#[derive(Deserialize)]
struct AdjustPayload {
    /// "master", "incoming" (the other PC's sound here) or "app".
    target: String,
    /// The app's name as the mixer shows it (target "app").
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    remote: bool,
    /// Added to the volume (0–1 scale), e.g. 0.02 per dial tick.
    #[serde(default)]
    delta: Option<f32>,
    #[serde(default)]
    volume: Option<f32>,
    #[serde(default)]
    mute: Option<MuteChange>,
}

#[derive(Deserialize, Default)]
struct RemotePayload {
    #[serde(default)]
    remote: bool,
}

fn envelope(kind: &str, payload: Value) -> String {
    json!({ "type": kind, "payload": payload, "timestamp": now_ms() }).to_string()
}

fn error_message(message: &str) -> String {
    envelope("error", json!({ "message": message }))
}

fn connection_status_message() -> String {
    let info = connection::info();
    envelope(
        "connection_status",
        json!({
            "connected": info.connected,
            // Kept as `is_host` for API compatibility: "we initiated the link".
            "is_host": info.is_initiator,
            "peer_hostname": info.peer_hostname,
            "peer_ip": info.peer_ip,
        }),
    )
}

async fn handle_message(text: &str) -> Option<String> {
    let message: WsMessage = match serde_json::from_str(text) {
        Ok(message) => message,
        Err(e) => return Some(error_message(&format!("Invalid message: {e}"))),
    };
    match dispatch(&message).await {
        Ok(reply) => reply,
        Err(e) => Some(error_message(&e)),
    }
}

fn payload<T: DeserializeOwned>(message: &WsMessage) -> Result<T, String> {
    let value = if message.payload.is_null() {
        json!({})
    } else {
        message.payload.clone()
    };
    serde_json::from_value(value)
        .map_err(|e| format!("Invalid payload for '{}': {e}", message.kind))
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| format!("Task failed: {e}"))?
}

async fn remote(request: Request) -> Result<(), String> {
    connection::request(request).await.map(|_| ())
}

async fn dispatch(message: &WsMessage) -> Result<Option<String>, String> {
    match message.kind.as_str() {
        "get_sessions" => {
            let p: RemotePayload = payload(message)?;
            sessions_message(p.remote).await.map(Some)
        }
        "get_devices" => {
            let (output, input) = blocking(|| {
                Ok((
                    audio_sessions::get_output_devices()?,
                    audio_sessions::get_input_devices()?,
                ))
            })
            .await?;
            Ok(Some(envelope(
                "devices",
                json!({ "output": output, "input": input }),
            )))
        }
        "set_volume" => {
            let p: VolumePayload = payload(message)?;
            if p.remote {
                remote(Request::SetSessionVolume {
                    pid: p.pid,
                    volume: p.volume,
                })
                .await?;
            } else {
                blocking(move || audio_sessions::set_session_volume(p.pid, p.volume)).await?;
            }
            Ok(None)
        }
        "set_mute" => {
            let p: MutePayload = payload(message)?;
            if p.remote {
                remote(Request::SetSessionMute {
                    pid: p.pid,
                    muted: p.muted,
                })
                .await?;
            } else {
                blocking(move || audio_sessions::set_session_mute(p.pid, p.muted)).await?;
            }
            Ok(None)
        }
        "set_master_volume" => {
            let p: MasterVolumePayload = payload(message)?;
            if p.remote {
                remote(Request::SetMasterVolume { volume: p.volume }).await?;
            } else {
                blocking(move || audio_sessions::set_master_volume(p.volume)).await?;
            }
            Ok(None)
        }
        "set_master_mute" => {
            let p: MasterMutePayload = payload(message)?;
            if p.remote {
                remote(Request::SetMasterMute { muted: p.muted }).await?;
            } else {
                blocking(move || audio_sessions::set_master_mute(p.muted)).await?;
            }
            Ok(None)
        }
        "mute_all" | "unmute_all" => {
            let muted = message.kind == "mute_all";
            let p: RemotePayload = payload(message)?;
            if p.remote {
                remote(Request::SetAllMuted { muted }).await?;
            } else {
                blocking(move || audio_sessions::set_all_sessions_mute(muted)).await?;
            }
            Ok(None)
        }
        "get_state" => Ok(Some(state_message())),
        "apply_scene" => {
            let p: ScenePayload = payload(message)?;
            let summary = crate::scenes::apply(&p.id).await?;
            Ok(Some(envelope(
                "scene_applied",
                json!({ "id": p.id, "summary": summary }),
            )))
        }
        "set_direction" => {
            let p: DirectionPayload = payload(message)?;
            crate::set_direction_from_api(p.direction).await?;
            Ok(Some(state_message()))
        }
        "set_follow_pointer" => {
            let p: FollowPayload = payload(message)?;
            crate::set_follow_pointer(p.on);
            Ok(Some(state_message()))
        }
        "adjust" => {
            let p: AdjustPayload = payload(message)?;
            adjust(p).await.map(Some)
        }
        other => Err(format!("Unknown message type '{other}'")),
    }
}

/// One control on a controller (a key, a dial) acting on master, the
/// incoming sound or an app, here or on the other PC. Answers with its state.
async fn adjust(p: AdjustPayload) -> Result<String, String> {
    let new_volume = |current: f32| -> Option<f32> {
        p.volume
            .or(p.delta.map(|d| current + d))
            .map(|v| v.clamp(0.0, 1.0))
    };
    let new_mute = |current: bool| -> Option<bool> {
        match &p.mute {
            Some(MuteChange::Set(m)) => Some(*m),
            Some(MuteChange::Toggle(word)) if word == "toggle" => Some(!current),
            _ => None,
        }
    };
    let (volume, muted) = match p.target.as_str() {
        "incoming" if !p.remote => {
            let s = settings::get();
            let (v, m) = (new_volume(s.remote_volume), new_mute(s.remote_muted));
            // Only a real change saves and notifies (a plain query must not
            // echo a "state" back to the controller that asked).
            if v.is_some() || m.is_some() {
                crate::set_incoming(v, m);
            }
            (v.unwrap_or(s.remote_volume), m.unwrap_or(s.remote_muted))
        }
        "master" | "app" => {
            let mixer = if p.remote {
                match connection::request(Request::GetMixer).await? {
                    Response::Mixer { mixer } => mixer,
                    _ => return Err("Unexpected answer from the other PC".into()),
                }
            } else {
                blocking(audio_sessions::get_mixer_snapshot).await?
            };
            let (current_volume, current_muted, pids) = if p.target == "master" {
                let m = mixer.master.as_ref().ok_or("No output device")?;
                (m.volume, m.muted, Vec::new())
            } else {
                let name = p.name.as_deref().ok_or("Which app? (name)")?;
                let apps: Vec<_> = mixer
                    .sessions
                    .iter()
                    .filter(|s| s.name.eq_ignore_ascii_case(name))
                    .collect();
                let first = apps
                    .first()
                    .ok_or_else(|| format!("{name} isn't playing"))?;
                (
                    first.volume,
                    first.muted,
                    apps.iter().map(|s| s.pid).collect(),
                )
            };
            let v = new_volume(current_volume);
            let m = new_mute(current_muted);
            let mut changes = Vec::new();
            if p.target == "master" {
                if let Some(volume) = v {
                    changes.push(Request::SetMasterVolume { volume });
                }
                if let Some(muted) = m {
                    changes.push(Request::SetMasterMute { muted });
                }
            } else {
                for pid in &pids {
                    if let Some(volume) = v {
                        changes.push(Request::SetSessionVolume { pid: *pid, volume });
                    }
                    if let Some(muted) = m {
                        changes.push(Request::SetSessionMute { pid: *pid, muted });
                    }
                }
            }
            for change in changes {
                if p.remote {
                    remote(change).await?;
                } else {
                    blocking(move || match change {
                        Request::SetMasterVolume { volume } => {
                            audio_sessions::set_master_volume(volume)
                        }
                        Request::SetMasterMute { muted } => audio_sessions::set_master_mute(muted),
                        Request::SetSessionVolume { pid, volume } => {
                            audio_sessions::set_session_volume(pid, volume)
                        }
                        Request::SetSessionMute { pid, muted } => {
                            audio_sessions::set_session_mute(pid, muted)
                        }
                        _ => Ok(()),
                    })
                    .await?;
                }
            }
            (v.unwrap_or(current_volume), m.unwrap_or(current_muted))
        }
        "incoming" => return Err("The other PC's incoming sound can't be set from here".into()),
        other => return Err(format!("Unknown target '{other}'")),
    };
    Ok(envelope(
        "target",
        json!({ "target": p.target, "name": p.name, "remote": p.remote, "volume": volume, "muted": muted }),
    ))
}

async fn sessions_message(remote: bool) -> Result<String, String> {
    if remote {
        let mixer = match connection::request(Request::GetMixer).await? {
            Response::Mixer { mixer } => mixer,
            _ => return Err("Unexpected answer from the other PC".into()),
        };
        let ids = mixer
            .sessions
            .iter()
            .filter_map(|s| s.icon_id.clone())
            .collect();
        let icons = match connection::request(Request::GetIcons { ids }).await {
            Ok(Response::Icons { icons }) => icons,
            _ => HashMap::new(),
        };
        Ok(envelope(
            "audio_sessions",
            sessions_payload(&mixer, true, |id| icons.get(id).cloned()),
        ))
    } else {
        let mixer = blocking(audio_sessions::get_mixer_snapshot).await?;
        Ok(envelope(
            "audio_sessions",
            sessions_payload(&mixer, false, audio_sessions::icon_data),
        ))
    }
}

fn sessions_payload(
    mixer: &MixerSnapshot,
    remote: bool,
    icon: impl Fn(&str) -> Option<String>,
) -> Value {
    let sessions: Vec<Value> = mixer
        .sessions
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "pid": s.pid,
                "name": s.name,
                "volume": s.volume,
                "muted": s.muted,
                "peak": s.peak_level,
                "icon": s.icon_id.as_deref().and_then(&icon),
            })
        })
        .collect();
    let master = match &mixer.master {
        Some(m) => json!({ "volume": m.volume, "muted": m.muted, "peak": m.peak, "name": m.name }),
        None => json!({ "volume": 1.0, "muted": false, "peak": 0.0 }),
    };
    json!({ "sessions": sessions, "master": master, "remote": remote })
}

/// Peaks at 10 Hz and the full local session list once a second, only while
/// someone is listening.
async fn broadcast_loop(mut shutdown: watch::Receiver<bool>) {
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    let mut last_full = Instant::now() - Duration::from_secs(10);
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = shutdown.changed() => break,
        }
        if CLIENTS.load(Ordering::Relaxed) == 0 {
            continue;
        }
        let Ok(mixer) = blocking(audio_sessions::get_mixer_snapshot).await else {
            continue;
        };
        let mut peaks: HashMap<String, f32> = mixer
            .sessions
            .iter()
            .map(|s| (s.pid.to_string(), s.peak_level))
            .collect();
        peaks.insert(
            "master".into(),
            mixer.master.as_ref().map_or(0.0, |m| m.peak),
        );
        let _ = BROADCAST.send(envelope("peak_update", json!({ "peaks": peaks })));

        if last_full.elapsed() >= Duration::from_secs(1) {
            last_full = Instant::now();
            let _ = BROADCAST.send(envelope(
                "audio_sessions",
                sessions_payload(&mixer, false, audio_sessions::icon_data),
            ));
        }
    }
}

// ============ UDP discovery responder ============

fn spawn_discovery_responder(stop: Arc<AtomicBool>) {
    let spawned = std::thread::Builder::new()
        .name("trovul-companion-discovery".into())
        .spawn(move || {
            // A responder from a previous start may hold the port for a moment.
            let mut socket = None;
            for _ in 0..10 {
                if stop.load(Ordering::Acquire) {
                    return;
                }
                match UdpSocket::bind(("0.0.0.0", COMPANION_DISCOVERY_PORT)) {
                    Ok(s) => {
                        socket = Some(s);
                        break;
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
            let Some(socket) = socket else {
                log::warn!("[Companion] Discovery responder could not bind UDP {COMPANION_DISCOVERY_PORT}");
                return;
            };
            let _ = socket.set_read_timeout(Some(Duration::from_millis(500)));
            log::info!("[Companion] Discovery responder on UDP {COMPANION_DISCOVERY_PORT}");
            let mut buf = [0u8; 1024];
            while !stop.load(Ordering::Acquire) {
                let (len, from) = match socket.recv_from(&mut buf) {
                    Ok(received) => received,
                    Err(e) => {
                        if !matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        continue;
                    }
                };
                let is_discover = serde_json::from_slice::<Value>(&buf[..len])
                    .ok()
                    .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(|t| t == "trovul_discover"))
                    .unwrap_or(false);
                if !is_discover {
                    continue;
                }
                let info = connection::info();
                let announce = json!({
                    "type": "trovul_announce",
                    "version": "2.0",
                    "device": "desktop",
                    "hostname": network::hostname(),
                    "ip": network::local_ip().map(|ip| ip.to_string()).unwrap_or_else(|| "0.0.0.0".into()),
                    "port": COMPANION_WS_PORT,
                    "auth": "token",
                    "connected_to": if info.connected { info.peer_hostname } else { None },
                    "timestamp": now_ms(),
                });
                let _ = socket.send_to(announce.to_string().as_bytes(), from);
            }
            log::info!("[Companion] Discovery responder stopped");
        });
    if let Err(e) = spawned {
        log::error!("[Companion] Could not start the discovery responder: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::token_matches;

    #[test]
    fn token_check() {
        assert!(token_matches(Some("token=abc123"), "abc123"));
        assert!(token_matches(Some("x=1&token=abc123"), "abc123"));
        assert!(!token_matches(Some("token=abc124"), "abc123"));
        assert!(!token_matches(Some("token=abc12"), "abc123"));
        assert!(!token_matches(None, "abc123"));
        assert!(!token_matches(Some(""), "abc123"));
    }
}
