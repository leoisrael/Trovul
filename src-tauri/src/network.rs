// Trovul - Peer discovery (UDP broadcast on 5333) and shared network constants.

use crate::settings;
use crate::util::{mono_ms, now_ms, read, write};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, Instant};

pub const DISCOVERY_PORT: u16 = 5333;
/// Peer control link (TCP). Audio uses `audio::AUDIO_PORT` (UDP 5336).
pub const CONTROL_PORT: u16 = 5334;

// Companion API ports (separate from peer-to-peer discovery)
pub const COMPANION_DISCOVERY_PORT: u16 = 5533;
pub const COMPANION_WS_PORT: u16 = 5534;

// Wire-protocol version. Bump the major on any incompatible change to control
// frames, discovery packets or audio packets; peers with another major are
// listed as incompatible and refused. v2 = persistent framed control link,
// PCM16 stereo audio packets with session ids.
pub const PROTOCOL_VERSION_MAJOR: u16 = 2;
/// 1: keyboard & mouse sharing (Frame::Input, GetInputInfo, SetInputLayout),
///    updates over the link (GetUpdateInfo, GetUpdateChunk).
/// 2: mixer — output per app, the stream strip (GetOutputDevices,
///    SetAppOutput, SetIncoming, SetPlaybackDevice; MixerSnapshot.stream).
/// 3: shared clipboard (ClipboardOffer, GetClipboardChunk).
/// 4: files through the clipboard (ClipOffer.files, GetClipboardFileChunk).
/// 5: chosen apps to send (SetSendMode, SetAppSend; MixerSnapshot.send_mix).
pub const PROTOCOL_VERSION_MINOR: u16 = 5;

const BROADCAST_INTERVAL: Duration = Duration::from_secs(2);
const TARGET_REFRESH: Duration = Duration::from_secs(30);
const PEER_EXPIRY_MS: u64 = 10_000;

/// A Trovul instance seen on the LAN.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    /// Persistent device id of that installation.
    pub id: String,
    pub hostname: String,
    pub ip: String,
    pub last_seen: u64,
    pub version: String,
    /// False when it speaks another protocol major — shown, but can't connect.
    pub compatible: bool,
    /// Paired before: connects without asking.
    #[serde(default)]
    pub paired: bool,
}

/// Discovery announcement packet
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryPacket {
    /// Persistent device id (see `settings::device_id`).
    pub id: String,
    pub hostname: String,
    pub version: String,
    // Packets from builds that predate these fields parse as v0 (incompatible).
    #[serde(default)]
    pub proto_major: u16,
    #[serde(default)]
    pub proto_minor: u16,
}

static HOSTNAME: LazyLock<String> = LazyLock::new(|| {
    hostname::get()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "Unknown".into())
});
// id → (peer, mono_ms last seen)
static PEERS: LazyLock<RwLock<HashMap<String, (Peer, u64)>>> = LazyLock::new(Default::default);
static DISCOVERY_STARTED: AtomicBool = AtomicBool::new(false);

/// The Windows computer name.
pub fn computer_name() -> String {
    HOSTNAME.clone()
}

/// The name other PCs see: the one chosen in Settings, else the computer name.
pub fn hostname() -> String {
    settings::pc_name().unwrap_or_else(computer_name)
}

pub fn local_ip() -> Option<IpAddr> {
    local_ip_address::local_ip().ok()
}

/// Returns true if the remote major version is compatible with ours.
pub fn is_proto_compatible(remote_major: u16) -> bool {
    remote_major == PROTOCOL_VERSION_MAJOR
}

/// Broadcast addresses for discovery: the limited broadcast plus a directed
/// broadcast per IPv4 interface assuming a /24 (covers the common home and
/// office LANs without hard-coding a subnet).
pub fn discovery_broadcast_targets() -> Vec<Ipv4Addr> {
    let mut out = vec![Ipv4Addr::BROADCAST];
    if let Ok(ifaces) = local_ip_address::list_afinet_netifas() {
        for (_name, addr) in ifaces {
            if let IpAddr::V4(v4) = addr {
                if v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() {
                    continue;
                }
                let o = v4.octets();
                let broadcast = Ipv4Addr::new(o[0], o[1], o[2], 255);
                if !out.contains(&broadcast) {
                    out.push(broadcast);
                }
            }
        }
    }
    out
}

/// Start announcing ourselves and listening for peers (idempotent).
pub fn start_discovery() {
    if DISCOVERY_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Err(e) = std::thread::Builder::new()
        .name("trovul-discovery".into())
        .spawn(run_discovery)
    {
        log::error!("Could not start discovery: {e}");
        DISCOVERY_STARTED.store(false, Ordering::Release);
    }
}

pub fn discovered_peers() -> Vec<Peer> {
    let mut peers: Vec<Peer> = read(&PEERS)
        .values()
        .map(|(peer, _)| Peer {
            paired: crate::connection::is_paired(&peer.id),
            ..peer.clone()
        })
        .collect();
    peers.sort_by_key(|p| p.hostname.to_lowercase());
    peers
}

fn bind_discovery_socket() -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.set_broadcast(true)?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT).into())?;
    let socket: UdpSocket = socket.into();
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    Ok(socket)
}

fn run_discovery() {
    let socket = match bind_discovery_socket() {
        Ok(socket) => socket,
        Err(e) => {
            log::error!("Discovery unavailable (UDP {DISCOVERY_PORT}): {e}");
            DISCOVERY_STARTED.store(false, Ordering::Release);
            return;
        }
    };
    // Rebuilt for every broadcast so a new PC name shows up within seconds.
    let announcement = || {
        serde_json::to_vec(&DiscoveryPacket {
            id: settings::device_id(),
            hostname: hostname(),
            version: env!("CARGO_PKG_VERSION").into(),
            proto_major: PROTOCOL_VERSION_MAJOR,
            proto_minor: PROTOCOL_VERSION_MINOR,
        })
        .unwrap_or_default()
    };

    // Recomputed periodically so a laptop that changes networks keeps working.
    let mut targets = discovery_broadcast_targets();
    let mut targets_refreshed = Instant::now();
    let mut last_broadcast: Option<Instant> = None;
    let mut buf = [0u8; 2048];
    log::info!(
        "Discovery running as {} (targets {:?})",
        settings::device_id(),
        targets
    );

    loop {
        if last_broadcast.is_none_or(|t| t.elapsed() >= BROADCAST_INTERVAL) {
            if targets_refreshed.elapsed() >= TARGET_REFRESH {
                targets = discovery_broadcast_targets();
                targets_refreshed = Instant::now();
            }
            let packet = announcement();
            for target in &targets {
                let _ = socket.send_to(&packet, SocketAddrV4::new(*target, DISCOVERY_PORT));
            }
            last_broadcast = Some(Instant::now());
        }

        match socket.recv_from(&mut buf) {
            Ok((len, from)) => handle_announcement(&buf[..len], from.ip()),
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::ConnectionReset
                ) => {}
            Err(e) => {
                log::debug!("Discovery receive error: {e}");
                std::thread::sleep(Duration::from_millis(200));
            }
        }

        let now = mono_ms();
        write(&PEERS).retain(|_, (_, seen)| now.saturating_sub(*seen) < PEER_EXPIRY_MS);
    }
}

fn handle_announcement(bytes: &[u8], from: IpAddr) {
    let Ok(packet) = serde_json::from_slice::<DiscoveryPacket>(bytes) else {
        return;
    };
    if packet.id.is_empty() || packet.id == settings::device_id() {
        return;
    }
    let hostname: String = packet
        .hostname
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect();
    let peer = Peer {
        id: packet.id.clone(),
        hostname,
        ip: from.to_string(),
        last_seen: now_ms(),
        version: packet.version.chars().take(32).collect(),
        compatible: is_proto_compatible(packet.proto_major),
        paired: false,
    };
    write(&PEERS).insert(packet.id, (peer, mono_ms()));
}
