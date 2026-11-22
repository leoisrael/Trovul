// Trovul - Connection management on top of `peer`: the TCP listener, pairing
// (prompt + paired-device list), the single active link and the automatic
// reconnect to the last PC.

use crate::audio::{self, Direction, StreamPeer};
use crate::audio_sessions;
use crate::network::{self, CONTROL_PORT, PROTOCOL_VERSION_MAJOR, PROTOCOL_VERSION_MINOR};
use crate::peer::{
    self, CloseReason, Frame, FrameReader, LinkHandler, LinkInfo, LinkTiming, PeerLink, Request,
    Response,
};
use crate::settings::{self, LastPeer};
use crate::util::{
    constant_time_eq, hmac_sha256, lock, now_ms, random_hex, random_nonzero_u32, read, to_hex,
    write,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the user has to answer the pairing prompt.
const DECISION_TIMEOUT: Duration = Duration::from_secs(45);
/// How long the initiator waits for that answer (a bit longer than the prompt).
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(50);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
/// After the user declines a PC, further requests from it are refused silently for this long.
const DECLINE_COOLDOWN: Duration = Duration::from_secs(60);
const HANDSHAKE_WORKERS: usize = 2;
const PAIRED_FILE: &str = "paired_devices.json";
/// Reconnect: the side with the lower device id dials right away; the other
/// waits this long first, so both don't dial at once and collide as "busy".
const FOLLOWER_GRACE: Duration = Duration::from_secs(12);
/// Without discovery, dial the last known address after this long (leader;
/// the other side waits twice as long).
const DIRECT_DIAL_AFTER: Duration = Duration::from_secs(20);
const RECONNECT_BACKOFF: [u64; 5] = [3, 6, 15, 30, 60];

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionInfo {
    pub connected: bool,
    /// We opened the connection. Audio direction is chosen separately.
    pub is_initiator: bool,
    pub peer_hostname: Option<String>,
    pub peer_ip: Option<String>,
    pub peer_device_id: Option<String>,
}

/// A PC the user paired with, keyed by its persistent device id. The shared
/// random token proves it is the same installation when it reconnects, so a
/// new IP address (DHCP) or hostname doesn't bring the prompt back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub device_id: String,
    pub hostname: String,
    pub ip: String,
    /// Never serialised for the UI; `save_paired` writes it to disk explicitly.
    #[serde(skip_serializing, default)]
    token: String,
    pub first_seen: u64,
    pub last_seen: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingRequest {
    pub hostname: String,
    pub ip: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectResult {
    pub accepted: bool,
    pub reason: Option<String>,
}

struct Pending {
    hostname: String,
    ip: IpAddr,
    decision: mpsc::Sender<bool>,
}

static LINK: RwLock<Option<Arc<PeerLink>>> = RwLock::new(None);
/// Serialises "is anyone connected? → install link" so two handshakes can't both win.
static REGISTRATION: Mutex<()> = Mutex::new(());
static CONNECTING: AtomicBool = AtomicBool::new(false);
static NEXT_LINK_ID: AtomicU64 = AtomicU64::new(1);
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);
static DECLINED: LazyLock<Mutex<HashMap<IpAddr, Instant>>> = LazyLock::new(Default::default);
static PAIRED: LazyLock<RwLock<HashMap<String, PairedDevice>>> = LazyLock::new(Default::default);

/// Load the paired devices, accept control connections, and keep the link to
/// the last PC alive.
pub fn start() {
    load_paired();
    start_listener();
    start_reconnector();
}

pub fn current() -> Option<Arc<PeerLink>> {
    read(&LINK).clone()
}

pub fn info() -> ConnectionInfo {
    match current() {
        Some(link) => {
            let info = link.info();
            ConnectionInfo {
                connected: true,
                is_initiator: info.is_initiator,
                peer_hostname: Some(info.peer_hostname.clone()),
                peer_ip: Some(info.peer_ip.to_string()),
                peer_device_id: Some(info.peer_device_id.clone()),
            }
        }
        None => ConnectionInfo {
            connected: false,
            is_initiator: false,
            peer_hostname: None,
            peer_ip: None,
            peer_device_id: None,
        },
    }
}

/// Where audio goes / comes from, for the audio engine.
pub fn stream_peer() -> Option<StreamPeer> {
    current().map(|link| {
        let info = link.info();
        StreamPeer {
            link_id: info.id,
            ip: info.peer_ip,
            session: info.audio_session,
        }
    })
}

/// Send a request over the active link. `Response::Error` becomes `Err`.
pub async fn request(body: Request) -> Result<Response, String> {
    let link = current().ok_or("Not connected")?;
    match link.request(body, REQUEST_TIMEOUT).await? {
        Response::Error { message } => Err(message),
        other => Ok(other),
    }
}

/// End the active link (blocking: stops audio and notifies the peer).
/// `intentional` = the user chose to disconnect, so neither side reconnects
/// automatically; quitting the app is not intentional in that sense.
pub fn disconnect(reason: &str, intentional: bool) {
    if let Some(link) = current() {
        link.close(CloseReason::local(reason, intentional));
    }
}

/// Remember the audio direction for the connected PC (restored after reconnects).
pub fn remember_direction(direction: Direction) {
    let Some(link) = current() else { return };
    let device_id = link.info().peer_device_id.clone();
    settings::update(|s| {
        if let Some(last) = s.last_peer.as_mut().filter(|p| p.device_id == device_id) {
            last.direction = direction;
        }
    });
}

// ============ Outgoing ============

pub async fn connect(ip: String, device_id: Option<String>) -> Result<ConnectResult, String> {
    let ip: IpAddr = ip
        .trim()
        .parse()
        .map_err(|_| format!("'{ip}' is not a valid IP address"))?;
    tauri::async_runtime::spawn_blocking(move || connect_guarded(ip, device_id, false))
        .await
        .map_err(|e| format!("Connection attempt failed: {e}"))?
}

/// One connection attempt at a time, and never while already connected.
fn connect_guarded(
    ip: IpAddr,
    device_id: Option<String>,
    auto: bool,
) -> Result<ConnectResult, String> {
    if current().is_some() {
        return Err("Already connected — disconnect first".into());
    }
    if CONNECTING.swap(true, Ordering::AcqRel) {
        return Err("Another connection attempt is in progress".into());
    }
    let result = connect_blocking(ip, device_id, auto);
    CONNECTING.store(false, Ordering::Release);
    result
}

fn connect_blocking(
    ip: IpAddr,
    device_id: Option<String>,
    auto: bool,
) -> Result<ConnectResult, String> {
    connect_to(SocketAddr::new(ip, CONTROL_PORT), device_id, auto)
}

fn connect_to(
    addr: SocketAddr,
    device_id: Option<String>,
    auto: bool,
) -> Result<ConnectResult, String> {
    let ip = addr.ip();
    log::info!(
        "Connecting to {addr}{}",
        if auto { " (automatic)" } else { "" }
    );
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|e| format!("Could not reach {ip}: {e}"))?;
    prepare_socket(&stream).map_err(|e| format!("Socket setup failed: {e}"))?;
    let mut reader = FrameReader::default();
    let nonce =
        match peer::read_frame_until(&mut reader, &mut stream, Instant::now() + HELLO_TIMEOUT) {
            Ok(Some(Frame::Challenge { nonce })) => nonce,
            Ok(_) => return Err("The other PC runs an incompatible version of Trovul".into()),
            Err(e) => return Err(format!("Connection lost during the handshake: {e}")),
        };
    let my_id = settings::device_id();
    let proof = pairing_token_for(device_id.as_deref(), ip)
        .map(|token| pairing_proof(&token, &nonce, &my_id));
    peer::write_frame(
        &mut stream,
        &Frame::Hello {
            hostname: network::hostname(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            proto_major: PROTOCOL_VERSION_MAJOR,
            proto_minor: PROTOCOL_VERSION_MINOR,
            device_id: my_id,
            proof,
            auto,
        },
    )
    .map_err(|e| format!("Could not send the request: {e}"))?;

    let reply = peer::read_frame_until(&mut reader, &mut stream, Instant::now() + ACCEPT_TIMEOUT)
        .map_err(|e| format!("Connection lost while waiting for an answer: {e}"))?;
    match reply {
        Some(Frame::Accept {
            hostname,
            audio_session,
            device_id: peer_device_id,
            token,
        }) => {
            let hostname = sanitize_hostname(&hostname);
            match token.filter(|t| !t.is_empty()) {
                // New pairing. It is mutual: the PC we dialled can later dial us.
                Some(token) => remember_paired(&peer_device_id, &hostname, ip, &token),
                None => touch_paired(&peer_device_id, &hostname, ip),
            }
            let info = LinkInfo {
                id: NEXT_LINK_ID.fetch_add(1, Ordering::Relaxed),
                peer_hostname: hostname,
                peer_ip: ip,
                peer_device_id,
                is_initiator: true,
                audio_session,
            };
            register(stream, reader, info, |_| Ok(())).map_err(|e| match e {
                RegisterError::Busy(_) => "Already connected to another PC".to_string(),
                RegisterError::Failed(message) => message,
            })?;
            Ok(ConnectResult {
                accepted: true,
                reason: None,
            })
        }
        Some(Frame::Reject { reason }) => {
            log::info!("{ip} declined the connection: {reason}");
            Ok(ConnectResult {
                accepted: false,
                reason: Some(reason),
            })
        }
        Some(other) => Err(format!("Unexpected answer from the other PC: {other:?}")),
        None => Err("The other PC did not answer in time".into()),
    }
}

fn prepare_socket(stream: &TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))
}

enum RegisterError {
    /// Another link won the race; the stream is handed back to answer it.
    Busy(TcpStream),
    Failed(String),
}

/// Install a handshaken connection as the active link unless another one won
/// the race. `announce` runs under the registration lock right before the link
/// starts (the listener uses it to send `Accept`).
fn register(
    mut stream: TcpStream,
    reader: FrameReader,
    info: LinkInfo,
    announce: impl FnOnce(&mut TcpStream) -> io::Result<()>,
) -> Result<(), RegisterError> {
    let guard = lock(&REGISTRATION);
    if current().is_some() {
        return Err(RegisterError::Busy(stream));
    }
    announce(&mut stream).map_err(|e| RegisterError::Failed(format!("Handshake failed: {e}")))?;
    let link = PeerLink::spawn(
        stream,
        reader,
        info,
        LinkTiming::default(),
        Arc::new(AppLinkHandler),
    )
    .map_err(|e| RegisterError::Failed(format!("Could not start the connection: {e}")))?;
    *write(&LINK) = Some(link.clone());
    drop(guard);
    let info = link.info();
    log::info!(
        "Connected to {} ({}) as {}",
        info.peer_hostname,
        info.peer_ip,
        if info.is_initiator {
            "initiator"
        } else {
            "listener"
        }
    );
    remember_last_peer(info);
    crate::on_link_established(info);
    Ok(())
}

// ============ Incoming ============

fn start_listener() {
    let (tx, rx) = mpsc::sync_channel::<TcpStream>(16);
    let rx = Arc::new(Mutex::new(rx));
    for n in 0..HANDSHAKE_WORKERS {
        let rx = rx.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("trovul-handshake-{n}"))
            .spawn(move || loop {
                let next = lock(&rx).recv();
                match next {
                    Ok(stream) => handle_incoming(stream),
                    Err(_) => break,
                }
            });
        if let Err(e) = spawned {
            log::error!("Could not start a handshake worker: {e}");
        }
    }

    let spawned = std::thread::Builder::new()
        .name("trovul-control-accept".into())
        .spawn(move || {
            let listener = match TcpListener::bind(("0.0.0.0", CONTROL_PORT)) {
                Ok(listener) => listener,
                Err(e) => {
                    log::error!("Could not listen on TCP port {CONTROL_PORT}: {e}");
                    return;
                }
            };
            log::info!("Control listener on TCP port {CONTROL_PORT}");
            for stream in listener.incoming() {
                match stream {
                    // Handshakes are rare; a full queue means someone is flooding the port.
                    Ok(stream) => {
                        if tx.try_send(stream).is_err() {
                            log::warn!("Handshake queue full; dropping a connection");
                        }
                    }
                    Err(e) => {
                        log::warn!("Accept failed: {e}");
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        });
    if let Err(e) = spawned {
        log::error!("Could not start the control listener: {e}");
    }
}

fn handle_incoming(mut stream: TcpStream) {
    let Ok(peer_addr) = stream.peer_addr() else {
        return;
    };
    let ip = peer_addr.ip();
    if prepare_socket(&stream).is_err() {
        return;
    }
    // A fresh nonce per connection, so a recorded handshake can't be replayed.
    let nonce = random_hex::<16>();
    if peer::write_frame(
        &mut stream,
        &Frame::Challenge {
            nonce: nonce.clone(),
        },
    )
    .is_err()
    {
        return;
    }
    let mut reader = FrameReader::default();
    let hello = peer::read_frame_until(&mut reader, &mut stream, Instant::now() + HELLO_TIMEOUT);
    let Ok(Some(Frame::Hello {
        hostname,
        app_version,
        proto_major,
        proto_minor,
        device_id,
        proof,
        auto,
    })) = hello
    else {
        log::debug!("Ignoring a control connection from {ip} without a handshake");
        return;
    };
    let hostname = sanitize_hostname(&hostname);
    let reject = |stream: &mut TcpStream, reason: &str| {
        let _ = peer::write_frame(
            stream,
            &Frame::Reject {
                reason: reason.into(),
            },
        );
    };

    if !network::is_proto_compatible(proto_major) {
        log::warn!(
            "Rejecting {hostname} ({ip}): Trovul {app_version} speaks protocol {proto_major}.{proto_minor}"
        );
        reject(
            &mut stream,
            "incompatible version — update Trovul on both PCs",
        );
        return;
    }
    if current().is_some() || CONNECTING.load(Ordering::Acquire) {
        reject(&mut stream, "busy with another connection");
        return;
    }

    let known_token = verified_token(&device_id, &nonce, proof.as_deref());
    if known_token.is_none() {
        if auto {
            // Background reconnects must never pop a dialog up.
            reject(&mut stream, "not paired");
            return;
        }
        let refusal = match ask_user(&hostname, ip) {
            Answer::Accepted => None,
            Answer::Declined => Some("declined"),
            Answer::Expired => Some("no answer"),
            Answer::Busy => Some("busy with another request"),
        };
        if let Some(reason) = refusal {
            reject(&mut stream, reason);
            return;
        }
    }

    // Already paired: keep the token (and don't send it again). New: create one.
    let new_pairing = known_token.is_none() && !device_id.is_empty();
    let pair_token = known_token.unwrap_or_else(random_hex::<16>);
    let audio_session = random_nonzero_u32();
    let info = LinkInfo {
        id: NEXT_LINK_ID.fetch_add(1, Ordering::Relaxed),
        peer_hostname: hostname.clone(),
        peer_ip: ip,
        peer_device_id: device_id.clone(),
        is_initiator: false,
        audio_session,
    };
    let accept = Frame::Accept {
        hostname: network::hostname(),
        audio_session,
        device_id: settings::device_id(),
        token: new_pairing.then(|| pair_token.clone()),
    };
    match register(stream, reader, info, |s| peer::write_frame(s, &accept)) {
        Ok(()) => {
            if !device_id.is_empty() {
                remember_paired(&device_id, &hostname, ip, &pair_token);
            }
        }
        Err(RegisterError::Busy(mut stream)) => reject(&mut stream, "busy with another connection"),
        Err(RegisterError::Failed(e)) => log::warn!("Could not accept {hostname}: {e}"),
    }
}

enum Answer {
    Accepted,
    Declined,
    Expired,
    Busy,
}

/// Ask the user in the UI and block this handshake worker until they answer.
/// A PC the user declined can't prompt again for `DECLINE_COOLDOWN`, so a host
/// on the LAN can't keep popping the dialog up.
fn ask_user(hostname: &str, ip: IpAddr) -> Answer {
    {
        let mut declined = lock(&DECLINED);
        declined.retain(|_, at| at.elapsed() < DECLINE_COOLDOWN);
        if declined.contains_key(&ip) {
            log::info!("Declining {hostname} ({ip}) again without asking: declined moments ago");
            return Answer::Declined;
        }
    }

    let (tx, rx) = mpsc::channel();
    {
        let mut pending = lock(&PENDING);
        if pending.is_some() {
            log::info!("Declining {hostname}: another request is waiting for an answer");
            return Answer::Busy;
        }
        *pending = Some(Pending {
            hostname: hostname.to_string(),
            ip,
            decision: tx,
        });
    }
    log::info!("Asking the user whether {hostname} ({ip}) may connect");
    crate::on_connection_request(PendingRequest {
        hostname: hostname.to_string(),
        ip: ip.to_string(),
    });

    let decision = match rx.recv_timeout(DECISION_TIMEOUT) {
        Ok(decision) => Some(decision),
        Err(_) => {
            let mut pending = lock(&PENDING);
            if pending.as_ref().is_some_and(|p| p.ip == ip) {
                *pending = None;
                drop(pending);
                log::info!("Connection request from {hostname} expired");
                crate::on_connection_request_cancelled();
                None
            } else {
                // Answered right at the deadline: the decision is on its way.
                drop(pending);
                rx.recv_timeout(Duration::from_millis(200)).ok()
            }
        }
    };
    match decision {
        Some(true) => Answer::Accepted,
        Some(false) => {
            lock(&DECLINED).insert(ip, Instant::now());
            Answer::Declined
        }
        None => Answer::Expired,
    }
}

pub fn accept_pending() -> Result<(), String> {
    answer_pending(true)
}

pub fn reject_pending() -> Result<(), String> {
    answer_pending(false)
}

fn answer_pending(accept: bool) -> Result<(), String> {
    let pending = lock(&PENDING)
        .take()
        .ok_or("This connection request has already expired")?;
    log::info!(
        "User {} {} ({})",
        if accept { "accepted" } else { "declined" },
        pending.hostname,
        pending.ip
    );
    let _ = pending.decision.send(accept);
    Ok(())
}

fn sanitize_hostname(hostname: &str) -> String {
    let clean: String = hostname
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect();
    if clean.trim().is_empty() {
        "Unknown PC".into()
    } else {
        clean
    }
}

// ============ Link handler ============

struct AppLinkHandler;

impl LinkHandler for AppLinkHandler {
    fn handle_input(&self, _link: &LinkInfo, event: crate::input::InputEvent) {
        crate::input::handle_event(event);
    }

    fn handle_request(&self, link: &LinkInfo, request: Request) -> Response {
        let done = |result: Result<(), String>| match result {
            Ok(()) => Response::Ok,
            Err(message) => Response::Error { message },
        };
        match request {
            Request::GetMixer => match crate::mixer_snapshot() {
                Ok(mixer) => Response::Mixer { mixer },
                Err(message) => Response::Error { message },
            },
            Request::GetIcons { ids } => Response::Icons {
                icons: audio_sessions::get_icons(&ids),
            },
            Request::SetSessionVolume { pid, volume } => {
                done(audio_sessions::set_session_volume(pid, volume))
            }
            Request::SetSessionMute { pid, muted } => {
                done(audio_sessions::set_session_mute(pid, muted))
            }
            Request::SetAllMuted { muted } => done(audio_sessions::set_all_sessions_mute(muted)),
            Request::SetMasterVolume { volume } => done(audio_sessions::set_master_volume(volume)),
            Request::SetMasterMute { muted } => done(audio_sessions::set_master_mute(muted)),
            Request::GetOutputDevices => match audio_sessions::get_output_devices() {
                Ok(devices) => Response::Devices { devices },
                Err(message) => Response::Error { message },
            },
            Request::SetAppOutput { pid, device } => done(crate::route_app(pid, device)),
            Request::SetIncoming { volume, muted } => {
                crate::set_incoming(volume, muted);
                Response::Ok
            }
            Request::ClipboardOffer { offer } => crate::clipboard::offer_from_peer(offer),
            Request::SetSendMode { apps_only } => {
                crate::set_send_mode(apps_only);
                Response::Ok
            }
            Request::SetAppSend { name, on, level } => {
                crate::set_app_send(&name, on, level);
                Response::Ok
            }
            Request::GetClipboardChunk { id, offset, len } => {
                crate::clipboard::chunk(id, offset, len)
            }
            Request::GetClipboardFileChunk {
                id,
                index,
                offset,
                len,
            } => crate::clipfiles::chunk(id, index, offset, len),
            Request::SetPlaybackDevice { device } => {
                crate::apply_playback_device(device);
                Response::Ok
            }
            Request::GetAudioStats => Response::AudioStats {
                stats: audio::stats(),
            },
            Request::SetAudioMode { mode } => done(crate::apply_peer_audio_mode(mode)),
            Request::GetInputInfo => crate::input::info_for(&link.peer_device_id),
            Request::GetUpdateInfo => crate::update::info_for(&link.peer_device_id),
            Request::GetUpdateChunk { offset, len } => crate::update::chunk(offset, len),
            Request::SetInputLayout { layout } => {
                crate::input::layout_from_peer(&link.peer_device_id, layout);
                Response::Ok
            }
            Request::PlayTestSound => {
                // Answer right away; the chime takes a second to play.
                std::thread::spawn(|| {
                    if let Err(e) = audio::tone::play_chime() {
                        log::warn!("Test sound failed: {e}");
                    }
                });
                Response::Ok
            }
        }
    }

    fn on_closed(&self, link: &LinkInfo, reason: &CloseReason) {
        {
            let mut current = write(&LINK);
            if current.as_ref().is_some_and(|l| l.info().id == link.id) {
                *current = None;
            }
        }
        if reason.intentional() {
            // Someone chose to disconnect: don't dial back on our own.
            settings::update(|s| {
                if let Some(last) = s
                    .last_peer
                    .as_mut()
                    .filter(|p| p.device_id == link.peer_device_id)
                {
                    last.resume = false;
                }
            });
        }
        crate::on_link_closed(link, reason);
    }
}

// ============ Automatic reconnect ============

fn remember_last_peer(info: &LinkInfo) {
    if info.peer_device_id.is_empty() {
        return;
    }
    settings::update(|s| {
        let direction = s
            .last_peer
            .as_ref()
            .filter(|p| p.device_id == info.peer_device_id)
            .map(|p| p.direction)
            .unwrap_or_default();
        s.last_peer = Some(LastPeer {
            device_id: info.peer_device_id.clone(),
            hostname: info.peer_hostname.clone(),
            ip: info.peer_ip.to_string(),
            direction,
            resume: true,
        });
    });
}

/// Background loop: when the last PC is back (after a reboot, sleep, Wi-Fi drop
/// or app restart), reconnect and restore the audio direction.
///
/// The PC is found through discovery; when broadcasts are blocked (common on
/// office and guest networks) its last known address is dialled directly, less
/// often. The side with the lower device id goes first so both don't collide.
fn start_reconnector() {
    let spawned = std::thread::Builder::new()
        .name("trovul-reconnect".into())
        .spawn(|| {
            let mut wanting_since: Option<Instant> = None;
            let mut discovered_since: Option<Instant> = None;
            let mut failures = 0usize;
            let mut next_attempt = Instant::now();
            loop {
                std::thread::sleep(Duration::from_secs(2));
                let settings = settings::get();
                let target = settings
                    .last_peer
                    .filter(|p| p.resume && settings.auto_reconnect);
                let busy = current().is_some()
                    || CONNECTING.load(Ordering::Acquire)
                    || lock(&PENDING).is_some();
                let Some(last) = target.filter(|_| !busy) else {
                    wanting_since = None;
                    discovered_since = None;
                    failures = 0;
                    continue;
                };
                let wanting = *wanting_since.get_or_insert_with(Instant::now);
                let discovered = network::discovered_peers()
                    .into_iter()
                    .find(|p| p.compatible && p.id == last.device_id);
                let leader = settings.device_id < last.device_id;
                let (ip, grace_passed) = match &discovered {
                    Some(peer) => {
                        let seen = *discovered_since.get_or_insert_with(Instant::now);
                        (peer.ip.clone(), leader || seen.elapsed() >= FOLLOWER_GRACE)
                    }
                    None => {
                        discovered_since = None;
                        let wait = if leader {
                            DIRECT_DIAL_AFTER
                        } else {
                            DIRECT_DIAL_AFTER * 2
                        };
                        (last.ip.clone(), wanting.elapsed() >= wait)
                    }
                };
                if !grace_passed || Instant::now() < next_attempt {
                    continue;
                }
                let Ok(ip) = ip.parse::<IpAddr>() else {
                    continue;
                };
                log::info!(
                    "Reconnecting to {} ({ip}{})",
                    last.hostname,
                    if discovered.is_some() {
                        ""
                    } else {
                        ", last known address"
                    }
                );
                match connect_guarded(ip, Some(last.device_id.clone()), true) {
                    Ok(ConnectResult { accepted: true, .. }) => {
                        failures = 0;
                        crate::on_reconnected(last.direction);
                    }
                    Ok(ConnectResult { reason, .. }) => {
                        log::info!("Reconnect to {} refused: {reason:?}", last.hostname);
                        failures += 1;
                        if reason.as_deref() == Some("not paired") {
                            // It forgot us; only the user can pair again.
                            log::info!(
                                "{} no longer knows this PC; stopping reconnects",
                                last.hostname
                            );
                            settings::update(|s| {
                                if let Some(p) = s.last_peer.as_mut() {
                                    p.resume = false;
                                }
                            });
                            crate::on_settings_changed();
                        }
                    }
                    Err(e) => {
                        log::info!("Reconnect to {} failed: {e}", last.hostname);
                        failures += 1;
                    }
                }
                let wait = RECONNECT_BACKOFF[failures.min(RECONNECT_BACKOFF.len() - 1)];
                next_attempt = Instant::now() + Duration::from_secs(wait);
            }
        });
    if let Err(e) = spawned {
        log::error!("Could not start the reconnect loop: {e}");
    }
}

// ============ Paired devices ============

fn load_paired() {
    let Some(path) = settings::data_file(PAIRED_FILE) else {
        return;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    match serde_json::from_slice::<HashMap<String, PairedDevice>>(&bytes) {
        Ok(map) => {
            log::info!("Loaded {} paired device(s)", map.len());
            *write(&PAIRED) = map;
        }
        Err(e) => log::warn!("Ignoring unreadable {}: {}", path.display(), e),
    }
}

fn save_paired(map: &HashMap<String, PairedDevice>) {
    let Some(path) = settings::data_file(PAIRED_FILE) else {
        return;
    };
    // Tokens are skipped by `Serialize` for the UI; store them explicitly here.
    let stored: HashMap<&String, serde_json::Value> = map
        .iter()
        .map(|(id, p)| {
            let mut value = serde_json::to_value(p).unwrap_or_default();
            value["token"] = serde_json::Value::String(p.token.clone());
            (id, value)
        })
        .collect();
    match serde_json::to_vec_pretty(&stored) {
        Ok(bytes) => {
            if let Err(e) = settings::write_atomic(&path, &bytes) {
                log::warn!("Could not save {}: {}", path.display(), e);
            }
        }
        Err(e) => log::warn!("Could not serialise the paired devices: {e}"),
    }
}

/// Proof that the dialling PC knows the pairing token, bound to this
/// connection's nonce and to its own device id. The token itself never goes
/// over the network after pairing.
fn pairing_proof(token: &str, nonce: &str, device_id: &str) -> String {
    to_hex(&hmac_sha256(
        token.as_bytes(),
        format!("{nonce}|{device_id}").as_bytes(),
    ))
}

/// The pairing token shared with `device_id` (signs update offers).
pub fn pairing_token(device_id: &str) -> Option<String> {
    read(&PAIRED)
        .get(device_id)
        .map(|p| p.token.clone())
        .filter(|t| !t.is_empty())
}

/// The stored pairing token for `device_id` if `proof` was made with it.
fn verified_token(device_id: &str, nonce: &str, proof: Option<&str>) -> Option<String> {
    let proof = proof?;
    let paired = read(&PAIRED);
    let entry = paired.get(device_id).filter(|p| !p.token.is_empty())?;
    let expected = pairing_proof(&entry.token, nonce, device_id);
    constant_time_eq(expected.as_bytes(), proof.as_bytes()).then(|| entry.token.clone())
}

/// Token to present when dialling: by device id when discovery told us who it
/// is, otherwise by address (manual connects).
fn pairing_token_for(device_id: Option<&str>, ip: IpAddr) -> Option<String> {
    let paired = read(&PAIRED);
    let ip = ip.to_string();
    device_id
        .and_then(|id| paired.get(id))
        .or_else(|| paired.values().find(|p| p.ip == ip))
        .map(|p| p.token.clone())
        .filter(|t| !t.is_empty())
}

fn remember_paired(device_id: &str, hostname: &str, ip: IpAddr, token: &str) {
    if device_id.is_empty() || token.is_empty() {
        return;
    }
    let now = now_ms();
    let mut map = write(&PAIRED);
    let entry = map
        .entry(device_id.to_string())
        .or_insert_with(|| PairedDevice {
            device_id: device_id.to_string(),
            hostname: hostname.to_string(),
            ip: ip.to_string(),
            token: token.to_string(),
            first_seen: now,
            last_seen: now,
        });
    entry.hostname = hostname.to_string();
    entry.ip = ip.to_string();
    entry.token = token.to_string();
    entry.last_seen = now;
    save_paired(&map);
}

/// Reconnect of an existing pairing: refresh how we last saw it.
fn touch_paired(device_id: &str, hostname: &str, ip: IpAddr) {
    let mut map = write(&PAIRED);
    if let Some(entry) = map.get_mut(device_id) {
        entry.hostname = hostname.to_string();
        entry.ip = ip.to_string();
        entry.last_seen = now_ms();
        save_paired(&map);
    }
}

pub fn is_paired(device_id: &str) -> bool {
    read(&PAIRED).contains_key(device_id)
}

pub fn paired_devices() -> Vec<PairedDevice> {
    let mut devices: Vec<PairedDevice> = read(&PAIRED).values().cloned().collect();
    devices.sort_by_key(|p| std::cmp::Reverse(p.last_seen));
    devices
}

/// Forget a paired PC: it will need approval again, and we stop reconnecting to it.
pub fn forget_device(device_id: &str) {
    {
        let mut map = write(&PAIRED);
        if map.remove(device_id).is_some() {
            save_paired(&map);
        }
    }
    settings::update(|s| {
        if s.last_peer
            .as_ref()
            .is_some_and(|p| p.device_id == device_id)
        {
            s.last_peer = None;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_tokens_are_checked_and_never_exposed() {
        let ip: IpAddr = "10.0.0.7".parse().unwrap();
        remember_paired("test-device-1", "Desk", ip, "tok-1234");

        let nonce = "n-0001";
        let good = pairing_proof("tok-1234", nonce, "test-device-1");
        assert_eq!(
            verified_token("test-device-1", nonce, Some(&good)).as_deref(),
            Some("tok-1234")
        );
        // Wrong token, replayed proof from another handshake, proof presented
        // by a different device, no proof, or the raw token: all refused.
        let wrong_token = pairing_proof("tok-1235", nonce, "test-device-1");
        assert!(verified_token("test-device-1", nonce, Some(&wrong_token)).is_none());
        assert!(verified_token("test-device-1", "n-0002", Some(&good)).is_none());
        let other_device = pairing_proof("tok-1234", nonce, "someone-else");
        assert!(verified_token("someone-else", nonce, Some(&other_device)).is_none());
        assert!(verified_token("test-device-1", nonce, None).is_none());
        assert!(verified_token("test-device-1", nonce, Some("tok-1234")).is_none());

        // Found by device id (discovery) or by address (manual connect).
        let other_ip: IpAddr = "10.0.0.99".parse().unwrap();
        assert_eq!(
            pairing_token_for(Some("test-device-1"), other_ip).as_deref(),
            Some("tok-1234")
        );
        assert_eq!(pairing_token_for(None, ip).as_deref(), Some("tok-1234"));
        assert!(pairing_token_for(None, other_ip).is_none());

        let for_ui = serde_json::to_string(&paired_devices()).unwrap();
        assert!(for_ui.contains("test-device-1") && !for_ui.contains("tok-1234"));

        forget_device("test-device-1");
        assert!(verified_token("test-device-1", nonce, Some(&good)).is_none());
        assert!(!is_paired("test-device-1"));
    }

    /// Real handshake over loopback: challenge, HMAC proof, accept/reject.
    /// Both ends share this process's globals, so a successful handshake ends
    /// with the dialling side finding the link already taken — by the other end.
    #[test]
    fn handshake_proves_pairing_without_sending_the_token() {
        settings::update(|s| s.device_id = "handshake-dev".into());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let serve_one = move |listener: &TcpListener| {
            let (stream, _) = listener.accept().unwrap();
            handle_incoming(stream);
        };

        // Not paired + automatic: refused without any prompt.
        let server = std::thread::scope(|scope| {
            let handle = scope.spawn(|| serve_one(&listener));
            let result = connect_to(addr, Some("handshake-dev".into()), true);
            handle.join().unwrap();
            result
        });
        let refused = server.unwrap();
        assert!(!refused.accepted);
        assert_eq!(refused.reason.as_deref(), Some("not paired"));

        // Paired: the proof is accepted and the listener installs the link.
        remember_paired("handshake-dev", "Loopback", addr.ip(), "shared-secret");
        let result = std::thread::scope(|scope| {
            let handle = scope.spawn(|| serve_one(&listener));
            let result = connect_to(addr, Some("handshake-dev".into()), true);
            handle.join().unwrap();
            result
        });
        // Reaching "busy" means the proof was accepted and Accept came back.
        assert_eq!(
            result.unwrap_err(),
            "Already connected to another PC",
            "the pairing proof must be accepted"
        );
        // The existing pairing was reused, not replaced by a fresh token.
        assert_eq!(
            pairing_token_for(Some("handshake-dev"), addr.ip()).as_deref(),
            Some("shared-secret")
        );
        // The listener's link ends as soon as the dialling side drops its socket.
        let deadline = Instant::now() + Duration::from_secs(5);
        while current().is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(current().is_none());
        forget_device("handshake-dev");
    }
}
