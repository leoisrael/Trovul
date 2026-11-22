// Trovul - Audio streaming engine.
//
// Sender:   WASAPI loopback → downmix to stereo → PCM16 packets over UDP.
// Receiver: UDP → adaptive jitter buffer → cpal playback on the chosen device.
//
// Every start creates a fresh session with its own control flags and counters,
// and stopping joins the session's threads before returning. An old session
// can therefore never race or clobber a newer one, and the UDP port is always
// released before the next session binds it.

pub mod app_capture;
mod capture;
pub mod dsp;
mod playback;
pub mod tone;
pub mod wire;

use crate::util::{lock, mono_ms, read, write, AtomicF32};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const AUDIO_PORT: u16 = 5336;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Off,
    Sender,
    Host,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Sender => "sender",
            Self::Host => "host",
        }
    }
}

/// Which way audio flows across the link, from this PC's point of view.
/// Setting a direction configures both PCs (see `lib.rs::apply_direction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    #[default]
    Off,
    /// This PC's audio plays on the other PC.
    Send,
    /// The other PC's audio plays here.
    Receive,
}

impl Direction {
    pub fn from_local_mode(mode: Mode) -> Self {
        match mode {
            Mode::Off => Self::Off,
            Mode::Sender => Self::Send,
            Mode::Host => Self::Receive,
        }
    }
}

/// Jitter-buffer presets exposed in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LatencyMode {
    Low,
    #[default]
    Balanced,
    Stable,
}

impl LatencyMode {
    pub fn jitter_config(self) -> dsp::JitterConfig {
        match self {
            Self::Low => dsp::JitterConfig::low_latency(),
            Self::Balanced => dsp::JitterConfig::default(),
            Self::Stable => dsp::JitterConfig::stable(),
        }
    }
}

/// The peer a session streams with, taken from the control link.
#[derive(Debug, Clone, Copy)]
pub struct StreamPeer {
    pub link_id: u64,
    pub ip: IpAddr,
    /// Random id negotiated on the link; stamped on every audio packet.
    pub session: u32,
}

/// Stop/restart flags of one session. Each thread holds its session's `Arc`,
/// so a thread from an older session can never touch a newer one's flags.
#[derive(Debug, Default)]
pub(crate) struct SessionCtl {
    stop: AtomicBool,
    restart: AtomicBool,
}

impl SessionCtl {
    pub fn should_stop(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    pub fn restart_requested(&self) -> bool {
        self.restart.load(Ordering::Acquire)
    }

    pub fn clear_restart(&self) {
        self.restart.store(false, Ordering::Release);
    }

    /// Sleep up to `duration`, waking early if the session is stopped.
    pub fn sleep(&self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while !self.should_stop() {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            std::thread::sleep((deadline - now).min(Duration::from_millis(20)));
        }
    }
}

pub(crate) const STATE_STARTING: u8 = 0;
pub(crate) const STATE_RUNNING: u8 = 1;
pub(crate) const STATE_RETRYING: u8 = 2;

/// Live counters of one session.
#[derive(Debug, Default)]
pub(crate) struct Stats {
    pub state: AtomicU8,
    pub packets_sent: AtomicU64,
    pub send_errors: AtomicU64,
    pub packets_received: AtomicU64,
    pub packets_lost: AtomicU64,
    pub packets_late: AtomicU64,
    pub packets_rejected: AtomicU64,
    pub overflows: AtomicU64,
    pub kbps: AtomicF32,
    pub capture_rate: AtomicU32,
    pub capture_channels: AtomicU32,
    pub output_rate: AtomicU32,
    pub output_channels: AtomicU32,
    /// Receiver jitter-buffer state; the sender only uses `last_packet_ms`.
    pub jitter: Arc<dsp::JitterShared>,
    info: Mutex<StatsInfo>,
}

#[derive(Debug, Default, Clone)]
struct StatsInfo {
    last_error: Option<String>,
    device: Option<String>,
}

impl Stats {
    pub fn set_error(&self, message: Option<String>) {
        lock(&self.info).last_error = message;
    }

    pub fn set_device(&self, name: Option<String>) {
        lock(&self.info).device = name;
    }
}

/// Snapshot for the diagnostics panel (also sent to the peer on request).
/// Missing fields default, so stats from an older or newer peer still parse.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioStats {
    pub mode: Mode,
    /// idle | starting | sending | waiting | buffering | playing | retrying
    pub state: String,
    pub last_error: Option<String>,
    /// Capture device (sender) or playback device (receiver).
    pub device: Option<String>,
    pub kbps: f32,
    pub ms_since_last_packet: Option<u64>,
    // Sender
    pub packets_sent: u64,
    pub send_errors: u64,
    pub capture_rate: u32,
    pub capture_channels: u32,
    // Receiver
    pub packets_received: u64,
    pub packets_lost: u64,
    pub packets_late: u64,
    pub packets_rejected: u64,
    pub underruns: u64,
    pub skips: u64,
    pub buffer_ms: f32,
    pub target_ms: f32,
    pub drift_ppm: f32,
    pub input_rate: u32,
    pub output_rate: u32,
    pub output_channels: u32,
    /// Current stream level 0–1 (captured audio when sending, played audio when receiving).
    pub level: f32,
    /// Incoming audio is lowered because a call app on this PC is talking.
    pub ducked: bool,
}

struct Running {
    mode: Mode,
    peer: StreamPeer,
    ctl: Arc<SessionCtl>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Clone)]
struct Current {
    mode: Mode,
    ctl: Arc<SessionCtl>,
    stats: Arc<Stats>,
}

static ENGINE: Mutex<Option<Running>> = Mutex::new(None);
// Read side for stats/restart without waiting on ENGINE while it joins threads.
static CURRENT: RwLock<Option<Current>> = RwLock::new(None);
static OUTPUT_DEVICE: RwLock<Option<String>> = RwLock::new(None);
static LATENCY: RwLock<LatencyMode> = RwLock::new(LatencyMode::Balanced);
static REMOTE_VOLUME: AtomicF32 = AtomicF32::new(1.0);
static REMOTE_MUTED: AtomicBool = AtomicBool::new(false);
/// Extra gain from voice-chat ducking (1 = not ducked), see `ducking.rs`.
static DUCK: AtomicF32 = AtomicF32::new(1.0);
/// "Follow the pointer" (focus_mix.rs): the other PC quieter while you use this one.
static FOCUS: AtomicF32 = AtomicF32::new(1.0);
/// Orders gain updates: the volume/mute and ducking setters run on different
/// threads, and an older computed gain must never land last.
static GAIN_UPDATE: Mutex<()> = Mutex::new(());

/// Switch to `mode`, stopping any running session first. `peer` is resolved
/// under the engine lock, so a link that closes concurrently is never used.
/// Asking for what is already running is a no-op (no audible restart).
pub fn start(mode: Mode, peer: impl FnOnce() -> Option<StreamPeer>) -> Result<(), String> {
    let mut engine = lock(&ENGINE);
    if mode == Mode::Off {
        stop_locked(&mut engine);
        return Ok(());
    }
    let peer = peer();
    if let (Some(running), Some(peer)) = (engine.as_ref(), peer) {
        if running.mode == mode && running.peer.link_id == peer.link_id {
            return Ok(());
        }
    }
    stop_locked(&mut engine);
    let peer = peer.ok_or("Connect to the other PC first")?;
    let spawn = if mode == Mode::Sender {
        capture::spawn
    } else {
        playback::spawn
    };
    let ctl = Arc::new(SessionCtl::default());
    let stats = Arc::new(Stats::default());
    stats.jitter.gain.store(remote_gain());
    let thread = spawn(ctl.clone(), stats.clone(), peer)?;
    *write(&CURRENT) = Some(Current {
        mode,
        ctl: ctl.clone(),
        stats,
    });
    // A volume or ducking change may have landed since the gain above.
    publish_gain();
    *engine = Some(Running {
        mode,
        peer,
        ctl,
        thread: Some(thread),
    });
    log::info!("Audio {} started (peer {})", mode.as_str(), peer.ip);
    Ok(())
}

pub fn stop() {
    stop_locked(&mut lock(&ENGINE));
}

/// Stop only if the running session belongs to link `link_id`.
pub fn stop_for_link(link_id: u64) -> bool {
    let mut engine = lock(&ENGINE);
    if engine.as_ref().is_some_and(|r| r.peer.link_id == link_id) {
        stop_locked(&mut engine);
        true
    } else {
        false
    }
}

fn stop_locked(engine: &mut Option<Running>) {
    let Some(mut running) = engine.take() else {
        return;
    };
    *write(&CURRENT) = None;
    running.ctl.stop.store(true, Ordering::Release);
    if let Some(handle) = running.thread.take() {
        // Threads poll the stop flag every ≤50 ms; 3 s only covers a wedged driver.
        let deadline = Instant::now() + Duration::from_secs(3);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !handle.is_finished() {
            log::warn!(
                "Audio {} thread did not stop within 3 s; detaching it",
                running.mode.as_str()
            );
        } else if handle.join().is_err() {
            log::error!("Audio {} thread panicked", running.mode.as_str());
        }
    }
    log::info!("Audio {} stopped", running.mode.as_str());
}

pub fn mode() -> Mode {
    read(&CURRENT).as_ref().map(|c| c.mode).unwrap_or_default()
}

/// Rebuild the running session's device handles (resume from sleep, device change).
pub fn request_restart() {
    if let Some(current) = read(&CURRENT).as_ref() {
        current.ctl.restart.store(true, Ordering::Release);
    }
}

/// Endpoint id of the device that plays remote audio (`None` = system default).
pub fn set_output_device(id: Option<String>) {
    let changed = {
        let mut device = write(&OUTPUT_DEVICE);
        let changed = *device != id;
        *device = id;
        changed
    };
    if changed && mode() == Mode::Host {
        request_restart();
    }
}

pub fn output_device() -> Option<String> {
    read(&OUTPUT_DEVICE).clone()
}

/// Jitter-buffer preset for playback; a running receiver restarts to apply it.
pub fn set_latency_mode(latency: LatencyMode) {
    let changed = {
        let mut current = write(&LATENCY);
        let changed = *current != latency;
        *current = latency;
        changed
    };
    if changed && mode() == Mode::Host {
        request_restart();
    }
}

pub(crate) fn latency_config() -> dsp::JitterConfig {
    read(&LATENCY).jitter_config()
}

fn remote_gain() -> f32 {
    if REMOTE_MUTED.load(Ordering::Relaxed) {
        0.0
    } else {
        REMOTE_VOLUME.load().clamp(0.0, 1.0) * DUCK.load() * FOCUS.load()
    }
}

/// Voice-chat ducking on top of the incoming volume (1 = none). The renderer
/// glides to the new gain, so small steps sound like a smooth fade.
pub fn set_duck(factor: f32) {
    DUCK.store(if factor.is_finite() {
        factor.clamp(0.0, 1.0)
    } else {
        1.0
    });
    publish_gain();
}

/// "Follow the pointer" on top of the incoming volume (1 = none).
pub fn set_focus(factor: f32) {
    let factor = if factor.is_finite() {
        factor.clamp(0.0, 1.0)
    } else {
        1.0
    };
    if (FOCUS.load() - factor).abs() > f32::EPSILON {
        FOCUS.store(factor);
        publish_gain();
    }
}

/// Hand the running session the gain for the current volume, mute and duck.
/// Serialised, so whichever call runs last sees every setter's latest value.
fn publish_gain() {
    let _order = lock(&GAIN_UPDATE);
    if let Some(current) = read(&CURRENT).as_ref() {
        current.stats.jitter.gain.store(remote_gain());
    }
}

/// Volume and mute of the audio received from the other PC. Applied inside
/// the renderer (glides, no restart) and independent of Windows volumes.
pub fn set_remote_volume(volume: f32, muted: bool) {
    let volume = if volume.is_finite() {
        volume.clamp(0.0, 1.0)
    } else {
        1.0
    };
    REMOTE_VOLUME.store(volume);
    REMOTE_MUTED.store(muted, Ordering::Relaxed);
    publish_gain();
}

pub fn stats() -> AudioStats {
    let Some(current) = read(&CURRENT).clone() else {
        return AudioStats {
            state: "idle".into(),
            ..Default::default()
        };
    };
    let s = &current.stats;
    let jitter = &s.jitter;
    let info = lock(&s.info).clone();
    let state_code = s.state.load(Ordering::Relaxed);
    let last_packet = jitter.last_packet_ms.load(Ordering::Relaxed);
    let since_last = (last_packet != 0).then(|| mono_ms().saturating_sub(last_packet));
    let state = match (current.mode, state_code) {
        (_, STATE_RETRYING) => "retrying",
        (_, STATE_STARTING) => "starting",
        (Mode::Sender, _) => "sending",
        (Mode::Host, _) if jitter.playing.load(Ordering::Relaxed) => "playing",
        (Mode::Host, _) if last_packet == 0 => "waiting",
        (Mode::Host, _) => "buffering",
        (Mode::Off, _) => "idle",
    };
    // Throughput is measured per second while packets flow; report 0 when idle.
    let kbps = if since_last.is_some_and(|ms| ms < 2_000) {
        s.kbps.load()
    } else {
        0.0
    };
    // The receiver's meter decays on its own; the sender only updates it per packet.
    let level = if current.mode == Mode::Host || since_last.is_some_and(|ms| ms < 250) {
        jitter.level.load()
    } else {
        0.0
    };
    AudioStats {
        mode: current.mode,
        state: state.into(),
        last_error: info.last_error,
        device: info.device,
        kbps,
        ms_since_last_packet: since_last,
        packets_sent: s.packets_sent.load(Ordering::Relaxed),
        send_errors: s.send_errors.load(Ordering::Relaxed),
        capture_rate: s.capture_rate.load(Ordering::Relaxed),
        capture_channels: s.capture_channels.load(Ordering::Relaxed),
        packets_received: s.packets_received.load(Ordering::Relaxed),
        packets_lost: s.packets_lost.load(Ordering::Relaxed),
        packets_late: s.packets_late.load(Ordering::Relaxed),
        packets_rejected: s.packets_rejected.load(Ordering::Relaxed),
        underruns: jitter.underruns.load(Ordering::Relaxed),
        skips: jitter.skips.load(Ordering::Relaxed),
        buffer_ms: jitter.buffer_ms.load(),
        target_ms: jitter.target_ms.load(),
        drift_ppm: jitter.drift_ppm.load(),
        input_rate: jitter.input_rate.load(Ordering::Relaxed),
        output_rate: s.output_rate.load(Ordering::Relaxed),
        output_channels: s.output_channels.load(Ordering::Relaxed),
        level,
        ducked: current.mode == Mode::Host && DUCK.load() < 0.99,
    }
}

#[cfg(test)]
mod hardware_tests {
    use super::*;

    fn wait_running(stats: &Stats) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if stats.state.load(Ordering::Relaxed) == STATE_RUNNING {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(lock(&stats.info)
            .last_error
            .clone()
            .unwrap_or_else(|| "timed out".into()))
    }

    /// Opens the real loopback capture and output stream without making any
    /// sound (nothing listens to the capture, nothing feeds the playback).
    /// Run with `cargo test -- --ignored hardware`.
    #[test]
    #[ignore]
    fn hardware_capture_and_playback_open_cleanly() {
        let peer = StreamPeer {
            link_id: 0,
            ip: "127.0.0.1".parse().unwrap(),
            session: 0x5EED,
        };

        let ctl = Arc::new(SessionCtl::default());
        let stats = Arc::new(Stats::default());
        let sender = capture::spawn(ctl.clone(), stats.clone(), peer).unwrap();
        let captured = wait_running(&stats);
        let (rate, channels) = (
            stats.capture_rate.load(Ordering::Relaxed),
            stats.capture_channels.load(Ordering::Relaxed),
        );
        ctl.stop.store(true, Ordering::Release);
        sender.join().unwrap();
        captured.expect("loopback capture did not start");
        assert!(
            rate >= 8_000 && channels >= 1,
            "capture format {rate} Hz / {channels} ch"
        );

        let ctl = Arc::new(SessionCtl::default());
        let stats = Arc::new(Stats::default());
        let receiver = playback::spawn(ctl.clone(), stats.clone(), peer).unwrap();
        let playing = wait_running(&stats);
        let out_rate = stats.output_rate.load(Ordering::Relaxed);
        ctl.stop.store(true, Ordering::Release);
        let started = Instant::now();
        receiver.join().unwrap();
        playing.expect("playback did not start");
        assert!(out_rate >= 8_000, "output rate {out_rate}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "receiver took {:?} to stop",
            started.elapsed()
        );
    }
}
