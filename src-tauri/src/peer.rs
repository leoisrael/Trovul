// Trovul - Peer control link (TCP 5334).
//
// One persistent connection per paired peer carrying length-prefixed JSON
// frames (u32 big-endian length + payload). The initiator sends `Hello`; the
// listener answers `Accept` (after asking the user, unless the peer is
// trusted) or `Reject`. The same socket then carries requests in both
// directions, heartbeats and a final `Bye`. As a result:
// - control requests are only served to the paired peer — anything else that
//   connects to 5334 has to go through the handshake first;
// - a vanished peer is noticed within seconds, independent of audio traffic;
// - responses of any size arrive whole (no single-`read()` truncation).

use crate::audio::AudioStats;
use crate::audio::Mode;
use crate::audio_sessions::{AudioDevice, MixerSnapshot};
use crate::clipboard::ClipOffer;
use crate::input::{InputEvent, Monitor, SharedLayout};
use crate::update::InstallerOffer;
use crate::util::{lock, mono_ms};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

pub const MAX_FRAME_LEN: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    /// First frame from the listener: a fresh nonce the initiator must sign.
    Challenge {
        nonce: String,
    },
    Hello {
        hostname: String,
        app_version: String,
        proto_major: u16,
        proto_minor: u16,
        /// Persistent id of the connecting installation.
        #[serde(default)]
        device_id: String,
        /// HMAC-SHA256(pairing token, "<nonce>|<device_id>") when already
        /// paired — proves the pairing without sending the token.
        #[serde(default)]
        proof: Option<String>,
        /// Automatic reconnect: never show a prompt for it.
        #[serde(default)]
        auto: bool,
    },
    Accept {
        hostname: String,
        /// Random id stamped on this link's audio packets.
        audio_session: u32,
        #[serde(default)]
        device_id: String,
        /// New pairing token, sent once when two PCs pair for the first time.
        #[serde(default)]
        token: Option<String>,
    },
    Reject {
        reason: String,
    },
    Request {
        id: u64,
        body: Request,
    },
    Response {
        id: u64,
        body: Response,
    },
    Ping,
    /// Keyboard & mouse sharing: one-way, no response.
    Input {
        event: InputEvent,
    },
    Bye {
        reason: Option<String>,
        /// The user ended the connection on purpose: don't reconnect automatically.
        #[serde(default)]
        intentional: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    GetMixer,
    GetIcons {
        ids: Vec<String>,
    },
    SetSessionVolume {
        pid: u32,
        volume: f32,
    },
    SetSessionMute {
        pid: u32,
        muted: bool,
    },
    SetAllMuted {
        muted: bool,
    },
    SetMasterVolume {
        volume: f32,
    },
    SetMasterMute {
        muted: bool,
    },
    GetAudioStats,
    /// Switch the peer's audio engine (the other half of a direction change).
    SetAudioMode {
        mode: Mode,
    },
    /// Play the test chime on the sending PC so it travels through the stream.
    PlayTestSound,
    /// Keyboard & mouse sharing: the peer's switch, screens and arrangement.
    GetInputInfo,
    /// The user rearranged the screens.
    SetInputLayout {
        layout: SharedLayout,
    },
    /// The peer's Trovul version, and its installer if it can share it.
    GetUpdateInfo,
    /// A piece of the peer's installer.
    GetUpdateChunk {
        offset: u64,
        len: u32,
    },
    /// The peer's active output devices.
    GetOutputDevices,
    /// Send an app to an output device (`None` = the Windows default).
    SetAppOutput {
        pid: u32,
        device: Option<String>,
    },
    /// Volume / mute of the sound the peer receives from this PC.
    SetIncoming {
        volume: Option<f32>,
        muted: Option<bool>,
    },
    /// Device the peer plays the received sound on (`None` = default).
    SetPlaybackDevice {
        device: Option<String>,
    },
    /// Something was copied on the peer (shared clipboard).
    ClipboardOffer {
        offer: ClipOffer,
    },
    /// A piece of the image the peer offered.
    GetClipboardChunk {
        id: u64,
        offset: u64,
        len: u32,
    },
    /// Send everything, or only the chosen apps.
    SetSendMode {
        apps_only: bool,
    },
    /// Choose (or not) an app to send, and its level.
    SetAppSend {
        name: String,
        on: bool,
        level: Option<f32>,
    },
    /// A piece of file `index` of the files the peer offered.
    GetClipboardFileChunk {
        id: u64,
        index: u32,
        offset: u64,
        len: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    Mixer {
        mixer: MixerSnapshot,
    },
    Icons {
        icons: HashMap<String, String>,
    },
    AudioStats {
        stats: AudioStats,
    },
    InputInfo {
        enabled: bool,
        monitors: Vec<Monitor>,
        layout: Option<SharedLayout>,
    },
    UpdateInfo {
        version: String,
        installer: Option<InstallerOffer>,
    },
    /// Base64.
    UpdateChunk {
        data: String,
    },
    Devices {
        devices: Vec<AudioDevice>,
    },
    /// Base64.
    ClipboardChunk {
        data: String,
    },
    Ok,
    Error {
        message: String,
    },
}

// ============ Framing ============

pub fn encode_frame(frame: &Frame) -> io::Result<Vec<u8>> {
    let json =
        serde_json::to_vec(frame).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if json.len() > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut out = Vec::with_capacity(4 + json.len());
    out.extend_from_slice(&(json.len() as u32).to_be_bytes());
    out.extend_from_slice(&json);
    Ok(out)
}

pub fn parse_frame(bytes: &[u8]) -> io::Result<Frame> {
    serde_json::from_slice(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn write_frame(stream: &mut impl Write, frame: &Frame) -> io::Result<()> {
    stream.write_all(&encode_frame(frame)?)
}

/// Reassembles frames from a stream that may deliver them in arbitrary pieces.
#[derive(Debug, Default)]
pub struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    /// Return the next complete frame, reading from `stream` at most once.
    /// `Ok(None)` means none is complete yet (partial data or read timeout).
    pub fn poll(&mut self, stream: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
        if let Some(frame) = self.take()? {
            return Ok(Some(frame));
        }
        let mut chunk = [0u8; 16 * 1024];
        match stream.read(&mut chunk) {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed",
            )),
            Ok(n) => {
                self.buf.extend_from_slice(&chunk[..n]);
                self.take()
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    fn take(&mut self) -> io::Result<Option<Vec<u8>>> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if len > MAX_FRAME_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame of {len} bytes exceeds the limit"),
            ));
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let frame = self.buf[4..4 + len].to_vec();
        self.buf.drain(..4 + len);
        Ok(Some(frame))
    }
}

/// Wait for one frame until `deadline` (the stream needs a short read timeout).
pub fn read_frame_until(
    reader: &mut FrameReader,
    stream: &mut impl Read,
    deadline: Instant,
) -> io::Result<Option<Frame>> {
    loop {
        if let Some(bytes) = reader.poll(stream)? {
            return parse_frame(&bytes).map(Some);
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
    }
}

// ============ Link ============

#[derive(Debug, Clone, Serialize)]
pub struct LinkInfo {
    pub id: u64,
    pub peer_hostname: String,
    pub peer_ip: IpAddr,
    /// Persistent id of the peer installation (empty for very old peers).
    pub peer_device_id: String,
    /// We opened the connection (the old `is_host`). Says nothing about audio direction.
    pub is_initiator: bool,
    pub audio_session: u32,
}

#[derive(Debug, Clone)]
pub enum CloseReason {
    /// We closed the link; `message` is sent to the peer in `Bye`.
    /// `intentional` = the user asked for it (no automatic reconnect).
    Local {
        message: String,
        intentional: bool,
    },
    PeerClosed {
        message: Option<String>,
        intentional: bool,
    },
    TimedOut,
    Failed(String),
}

impl CloseReason {
    pub fn local(message: impl Into<String>, intentional: bool) -> Self {
        Self::Local {
            message: message.into(),
            intentional,
        }
    }

    /// Someone chose to disconnect (as opposed to a crash, timeout or quit).
    pub fn intentional(&self) -> bool {
        matches!(
            self,
            Self::Local {
                intentional: true,
                ..
            } | Self::PeerClosed {
                intentional: true,
                ..
            }
        )
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Local { .. } => "Disconnected".into(),
            Self::PeerClosed {
                message: Some(reason),
                ..
            } => format!("The other PC disconnected ({reason})"),
            Self::PeerClosed { message: None, .. } => "The other PC disconnected".into(),
            Self::TimedOut => "The other PC stopped responding".into(),
            Self::Failed(error) => format!("Connection lost: {error}"),
        }
    }
}

pub trait LinkHandler: Send + Sync + 'static {
    /// Serve a request from the peer (runs on the link's request thread, one
    /// request at a time).
    fn handle_request(&self, link: &LinkInfo, request: Request) -> Response;
    /// Called exactly once when the link closes, whatever the cause.
    fn on_closed(&self, link: &LinkInfo, reason: &CloseReason);
    /// Keyboard & mouse events from the peer (link reader thread; keep it quick).
    fn handle_input(&self, _link: &LinkInfo, _event: InputEvent) {}
}

#[derive(Debug, Clone, Copy)]
pub struct LinkTiming {
    /// Send a ping when nothing else was sent for this long.
    pub heartbeat: Duration,
    /// Close the link when nothing was received for this long.
    pub timeout: Duration,
    /// Socket read timeout — how often the reader wakes up to check timers.
    pub poll: Duration,
}

impl Default for LinkTiming {
    fn default() -> Self {
        Self {
            heartbeat: Duration::from_secs(2),
            timeout: Duration::from_secs(7),
            poll: Duration::from_millis(250),
        }
    }
}

pub struct PeerLink {
    info: LinkInfo,
    timing: LinkTiming,
    writer: Mutex<TcpStream>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Response>>>,
    next_request: AtomicU64,
    closed: AtomicBool,
    closed_locally: AtomicBool,
    last_rx: AtomicU64,
    last_tx: AtomicU64,
    handler: Arc<dyn LinkHandler>,
}

impl PeerLink {
    /// Take over an already-handshaken connection and start its reader thread.
    /// `reader` may already hold bytes that arrived right after the handshake.
    pub fn spawn(
        stream: TcpStream,
        reader: FrameReader,
        info: LinkInfo,
        timing: LinkTiming,
        handler: Arc<dyn LinkHandler>,
    ) -> io::Result<Arc<Self>> {
        stream.set_read_timeout(Some(timing.poll))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let _ = stream.set_nodelay(true);
        let writer = stream.try_clone()?;
        let now = mono_ms();
        let link = Arc::new(Self {
            info,
            timing,
            writer: Mutex::new(writer),
            pending: Mutex::new(HashMap::new()),
            next_request: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            closed_locally: AtomicBool::new(false),
            last_rx: AtomicU64::new(now),
            last_tx: AtomicU64::new(now),
            handler,
        });
        let (requests, queue) = mpsc::channel();
        let worker_link = link.clone();
        std::thread::Builder::new()
            .name(format!("trovul-link-{}-requests", link.info.id))
            .spawn(move || worker_link.serve_requests(queue))?;
        let reader_link = link.clone();
        std::thread::Builder::new()
            .name(format!("trovul-link-{}", link.info.id))
            .spawn(move || reader_link.reader_loop(stream, reader, requests))?;
        Ok(link)
    }

    pub fn info(&self) -> &LinkInfo {
        &self.info
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Send one frame; a failed write closes the link. Blocking — call from a
    /// blocking context (the async `request` wraps it in `spawn_blocking`).
    pub fn send(&self, frame: &Frame) -> bool {
        let bytes = match encode_frame(frame) {
            Ok(bytes) => bytes,
            Err(e) => {
                log::warn!(
                    "Could not encode a frame for {}: {}",
                    self.info.peer_hostname,
                    e
                );
                return false;
            }
        };
        let written = lock(&self.writer).write_all(&bytes);
        match written {
            Ok(()) => {
                self.last_tx.store(mono_ms(), Ordering::Relaxed);
                true
            }
            Err(e) => {
                self.close(CloseReason::Failed(format!("send failed: {e}")));
                false
            }
        }
    }

    /// Close the link (idempotent). Pending requests fail immediately.
    pub fn close(&self, reason: CloseReason) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        {
            let mut writer = lock(&self.writer);
            if let CloseReason::Local {
                message,
                intentional,
            } = &reason
            {
                self.closed_locally.store(true, Ordering::Release);
                if let Ok(bytes) = encode_frame(&Frame::Bye {
                    reason: Some(message.clone()),
                    intentional: *intentional,
                }) {
                    let _ = writer.write_all(&bytes);
                }
                // Half-close only: shutting down the read side on Windows resets
                // the connection if data is still in flight, which can destroy
                // the Bye. The reader thread drains and drops the socket.
                let _ = writer.shutdown(Shutdown::Write);
            } else {
                let _ = writer.shutdown(Shutdown::Both);
            }
        }
        lock(&self.pending).clear();
        log::info!(
            "Link to {} ({}) closed: {}",
            self.info.peer_hostname,
            self.info.peer_ip,
            reason.describe()
        );
        self.handler.on_closed(&self.info, &reason);
    }

    /// Send a request to the peer and wait for its response.
    pub async fn request(
        self: &Arc<Self>,
        body: Request,
        timeout: Duration,
    ) -> Result<Response, String> {
        if self.is_closed() {
            return Err("Not connected".into());
        }
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);

        let link = self.clone();
        let sent =
            tokio::task::spawn_blocking(move || link.send(&Frame::Request { id, body })).await;
        if !matches!(sent, Ok(true)) {
            lock(&self.pending).remove(&id);
            return Err("Connection to the other PC was lost".into());
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err("Connection to the other PC was lost".into()),
            Err(_) => {
                lock(&self.pending).remove(&id);
                Err("The other PC did not answer in time".into())
            }
        }
    }

    /// The peer's requests run here, in order, not on the reader: a slow one
    /// (a mixer snapshot, a piece of the installer) must not hold up input
    /// events or heartbeats. Ends when the reader does (it owns the sender).
    fn serve_requests(self: Arc<Self>, queue: mpsc::Receiver<(u64, Request)>) {
        while let Ok((id, body)) = queue.recv() {
            if self.is_closed() {
                break;
            }
            let response = self.handler.handle_request(&self.info, body);
            if !self.send(&Frame::Response { id, body: response }) {
                break;
            }
        }
    }

    fn reader_loop(
        self: Arc<Self>,
        mut stream: TcpStream,
        mut reader: FrameReader,
        requests: mpsc::Sender<(u64, Request)>,
    ) {
        let heartbeat_ms = self.timing.heartbeat.as_millis() as u64;
        let timeout_ms = self.timing.timeout.as_millis() as u64;
        while !self.is_closed() {
            match reader.poll(&mut stream) {
                Ok(Some(bytes)) => {
                    self.last_rx.store(mono_ms(), Ordering::Relaxed);
                    match parse_frame(&bytes) {
                        Ok(frame) => {
                            if !self.dispatch(frame, &requests) {
                                break;
                            }
                        }
                        Err(e) => self.reply_unsupported(&bytes, &e),
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    self.close(if e.kind() == io::ErrorKind::UnexpectedEof {
                        CloseReason::PeerClosed {
                            message: None,
                            intentional: false,
                        }
                    } else {
                        CloseReason::Failed(e.to_string())
                    });
                    break;
                }
            }
            let now = mono_ms();
            if now.saturating_sub(self.last_rx.load(Ordering::Relaxed)) >= timeout_ms {
                self.close(CloseReason::TimedOut);
                break;
            }
            if now.saturating_sub(self.last_tx.load(Ordering::Relaxed)) >= heartbeat_ms
                && !self.send(&Frame::Ping)
            {
                break;
            }
        }
        if self.closed_locally.load(Ordering::Acquire) {
            // Wait (briefly) for the peer's FIN so the socket closes with an
            // empty receive buffer — otherwise Windows sends RST instead of FIN.
            let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
            let deadline = Instant::now() + Duration::from_secs(1);
            let mut sink = [0u8; 4096];
            while Instant::now() < deadline {
                match stream.read(&mut sink) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) => {}
                    Err(_) => break,
                }
            }
        }
    }

    /// Returns false when the link ended.
    fn dispatch(&self, frame: Frame, requests: &mpsc::Sender<(u64, Request)>) -> bool {
        match frame {
            Frame::Request { id, body } => requests.send((id, body)).is_ok(),
            Frame::Response { id, body } => {
                if let Some(waiter) = lock(&self.pending).remove(&id) {
                    let _ = waiter.send(body);
                }
                true
            }
            Frame::Ping => true,
            Frame::Input { event } => {
                self.handler.handle_input(&self.info, event);
                true
            }
            Frame::Bye {
                reason,
                intentional,
            } => {
                self.close(CloseReason::PeerClosed {
                    message: reason,
                    intentional,
                });
                false
            }
            Frame::Challenge { .. }
            | Frame::Hello { .. }
            | Frame::Accept { .. }
            | Frame::Reject { .. } => {
                log::debug!("Ignoring a handshake frame on an established link");
                true
            }
        }
    }

    /// A newer peer may send requests we don't know; answer instead of letting
    /// it wait for a timeout.
    fn reply_unsupported(&self, bytes: &[u8], error: &io::Error) {
        #[derive(Deserialize)]
        struct Probe {
            #[serde(rename = "type")]
            kind: String,
            id: Option<u64>,
        }
        match serde_json::from_slice::<Probe>(bytes) {
            Ok(Probe { kind, id: Some(id) }) if kind == "request" => {
                self.send(&Frame::Response {
                    id,
                    body: Response::Error {
                        message: "Unsupported request — update Trovul on both PCs".into(),
                    },
                });
            }
            _ => log::debug!(
                "Ignoring an unreadable frame from {}: {}",
                self.info.peer_hostname,
                error
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// Delivers the given bytes a few at a time, then times out.
    struct Trickle {
        data: Vec<u8>,
        pos: usize,
        step: usize,
    }

    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.pos >= self.data.len() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "no data"));
            }
            let n = self.step.min(buf.len()).min(self.data.len() - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn frames_survive_arbitrary_splits() {
        let mut data = encode_frame(&Frame::Ping).unwrap();
        let big_ids: Vec<String> = (0..5_000).map(|i| format!("icon-{i}")).collect();
        data.extend(
            encode_frame(&Frame::Request {
                id: 7,
                body: Request::GetIcons {
                    ids: big_ids.clone(),
                },
            })
            .unwrap(),
        );
        for step in [1, 3, 1000, 100_000] {
            let mut stream = Trickle {
                data: data.clone(),
                pos: 0,
                step,
            };
            let mut reader = FrameReader::default();
            let mut frames = Vec::new();
            for _ in 0..200_000 {
                if let Some(bytes) = reader.poll(&mut stream).unwrap() {
                    frames.push(parse_frame(&bytes).unwrap());
                }
                if frames.len() == 2 {
                    break;
                }
            }
            assert!(matches!(frames[0], Frame::Ping));
            match &frames[1] {
                Frame::Request {
                    id: 7,
                    body: Request::GetIcons { ids },
                } => assert_eq!(ids, &big_ids),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn oversized_frames_are_rejected() {
        let mut stream = Trickle {
            data: ((MAX_FRAME_LEN + 1) as u32).to_be_bytes().to_vec(),
            pos: 0,
            step: 4,
        };
        let err = FrameReader::default().poll(&mut stream).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    struct TestHandler {
        name: &'static str,
        closed: Mutex<mpsc::Sender<String>>,
    }

    impl LinkHandler for TestHandler {
        fn handle_request(&self, _link: &LinkInfo, request: Request) -> Response {
            match request {
                Request::SetMasterVolume { volume } if volume > 1.0 => Response::Error {
                    message: format!("{} rejects {volume}", self.name),
                },
                Request::GetIcons { ids } => Response::Icons {
                    icons: ids
                        .into_iter()
                        .map(|id| (id, self.name.to_string()))
                        .collect(),
                },
                _ => Response::Ok,
            }
        }

        fn on_closed(&self, _link: &LinkInfo, reason: &CloseReason) {
            let _ = lock(&self.closed).send(format!("{}: {:?}", self.name, reason));
        }
    }

    fn link_pair(timing: LinkTiming) -> (Arc<PeerLink>, Arc<PeerLink>, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let (tx, rx) = mpsc::channel();
        let make = |stream: TcpStream, name: &'static str, id: u64| {
            let info = LinkInfo {
                id,
                peer_hostname: name.into(),
                peer_ip: "127.0.0.1".parse().unwrap(),
                peer_device_id: format!("device-{id}"),
                is_initiator: id == 1,
                audio_session: 42,
            };
            let handler = Arc::new(TestHandler {
                name,
                closed: Mutex::new(tx.clone()),
            });
            PeerLink::spawn(stream, FrameReader::default(), info, timing, handler).unwrap()
        };
        (make(client, "a", 1), make(server, "b", 2), rx)
    }

    #[tokio::test]
    async fn requests_flow_both_ways_and_close_propagates() {
        let (a, b, closed) = link_pair(LinkTiming::default());

        let ids: Vec<String> = (0..2_000).map(|i| format!("{i:08}")).collect();
        match a
            .request(
                Request::GetIcons { ids: ids.clone() },
                Duration::from_secs(5),
            )
            .await
        {
            Ok(Response::Icons { icons }) => {
                assert_eq!(icons.len(), ids.len());
                assert!(icons.values().all(|v| v == "b"));
            }
            other => panic!("unexpected {other:?}"),
        }
        match b
            .request(
                Request::SetMasterVolume { volume: 2.0 },
                Duration::from_secs(5),
            )
            .await
        {
            Ok(Response::Error { message }) => assert_eq!(message, "a rejects 2"),
            other => panic!("unexpected {other:?}"),
        }

        a.close(CloseReason::local("bye test", true));
        let mut events: Vec<String> = (0..2)
            .map(|_| closed.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect();
        events.sort();
        assert!(events[0].starts_with("a: Local"), "{events:?}");
        assert!(
            events[1].contains("PeerClosed { message: Some(\"bye test\"), intentional: true }"),
            "{events:?}"
        );
        assert!(b.is_closed());
        assert!(a
            .request(Request::GetMixer, Duration::from_secs(1))
            .await
            .is_err());
    }

    #[test]
    fn silent_peer_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_silent_server, _) = listener.accept().unwrap(); // never reads or writes
        let (tx, rx) = mpsc::channel();
        let info = LinkInfo {
            id: 9,
            peer_hostname: "silent".into(),
            peer_ip: "127.0.0.1".parse().unwrap(),
            peer_device_id: "silent".into(),
            is_initiator: true,
            audio_session: 1,
        };
        let timing = LinkTiming {
            heartbeat: Duration::from_millis(50),
            timeout: Duration::from_millis(400),
            poll: Duration::from_millis(20),
        };
        let handler = Arc::new(TestHandler {
            name: "client",
            closed: Mutex::new(tx),
        });
        let _link = PeerLink::spawn(client, FrameReader::default(), info, timing, handler).unwrap();
        let event = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(event.contains("TimedOut"), "{event}");
    }

    #[test]
    fn heartbeats_keep_an_idle_link_alive() {
        let timing = LinkTiming {
            heartbeat: Duration::from_millis(50),
            timeout: Duration::from_millis(400),
            poll: Duration::from_millis(20),
        };
        let (a, b, closed) = link_pair(timing);
        assert!(
            closed.recv_timeout(Duration::from_millis(1_500)).is_err(),
            "idle link closed"
        );
        assert!(!a.is_closed() && !b.is_closed());
        a.close(CloseReason::local("done", true));
    }

    #[test]
    fn unknown_requests_get_an_error_instead_of_silence() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut raw = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let (tx, _rx) = mpsc::channel();
        let info = LinkInfo {
            id: 3,
            peer_hostname: "raw".into(),
            peer_ip: "127.0.0.1".parse().unwrap(),
            peer_device_id: "raw".into(),
            is_initiator: false,
            audio_session: 1,
        };
        let handler = Arc::new(TestHandler {
            name: "server",
            closed: Mutex::new(tx),
        });
        let _link = PeerLink::spawn(
            server,
            FrameReader::default(),
            info,
            LinkTiming::default(),
            handler,
        )
        .unwrap();

        let json = br#"{"type":"request","id":5,"body":{"kind":"from_the_future"}}"#;
        raw.write_all(&(json.len() as u32).to_be_bytes()).unwrap();
        raw.write_all(json).unwrap();
        raw.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut reader = FrameReader::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match read_frame_until(&mut reader, &mut raw, deadline).unwrap() {
                Some(Frame::Response {
                    id: 5,
                    body: Response::Error { .. },
                }) => break,
                Some(Frame::Ping) => continue,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}
