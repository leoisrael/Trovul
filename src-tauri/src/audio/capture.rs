// Trovul - Sender: WASAPI loopback of the default output device → UDP.

use super::dsp::{to_i16, CaptureFormat, Downmix};
use super::wire::{encode_packet, FRAMES_PER_PACKET, MAX_PACKET_LEN};
use super::{SessionCtl, Stats, StreamPeer, AUDIO_PORT, STATE_RETRYING, STATE_RUNNING};
use crate::audio_sessions;
use crate::com::ComGuard;
use crate::util::{mono_ms, random_nonzero_u32};
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};

const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const FIRST_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(5);
/// Loopback buffer (100 ns units). Only capacity — data is read as it arrives.
const BUFFER_DURATION: i64 = 10_000_000;

pub(super) fn spawn(
    ctl: Arc<SessionCtl>,
    stats: Arc<Stats>,
    peer: StreamPeer,
) -> Result<JoinHandle<()>, String> {
    let socket = UdpSocket::bind(("0.0.0.0", 0))
        .map_err(|e| format!("Could not open the audio socket: {e}"))?;
    socket
        .set_nonblocking(true)
        .map_err(|e| format!("Could not configure the audio socket: {e}"))?;
    let sender = Sender {
        socket,
        target: SocketAddr::new(peer.ip, AUDIO_PORT),
        session: peer.session,
        // Random start so the receiver sees a restarted stream as a reset,
        // not as thousands of "late" packets.
        seq: random_nonzero_u32(),
        packet: Vec::with_capacity(MAX_PACKET_LEN),
        stats: stats.clone(),
        window_bytes: 0,
        window_start: Instant::now(),
        last_error_log: None,
    };
    std::thread::Builder::new()
        .name("trovul-audio-sender".into())
        .spawn(move || run(ctl, stats, sender))
        .map_err(|e| format!("Could not start the audio sender: {e}"))
}

pub(super) struct Sender {
    socket: UdpSocket,
    target: SocketAddr,
    session: u32,
    seq: u32,
    packet: Vec<u8>,
    pub(super) stats: Arc<Stats>,
    window_bytes: u64,
    window_start: Instant,
    last_error_log: Option<Instant>,
}

impl Sender {
    pub(super) fn send(&mut self, sample_rate: u32, frames: &[[i16; 2]]) {
        let peak = frames
            .iter()
            .map(|[l, r]| l.unsigned_abs().max(r.unsigned_abs()))
            .max()
            .unwrap_or(0);
        self.stats.jitter.publish_level(peak as f32 / 32_768.0);
        encode_packet(
            &mut self.packet,
            self.session,
            self.seq,
            sample_rate,
            frames,
        );
        self.seq = self.seq.wrapping_add(1);
        // Unconnected socket + send_to: Windows then never reports ICMP
        // "port unreachable" as send errors when the peer isn't receiving yet.
        match self.socket.send_to(&self.packet, self.target) {
            Ok(n) => {
                self.stats.packets_sent.fetch_add(1, Ordering::Relaxed);
                self.stats
                    .jitter
                    .last_packet_ms
                    .store(mono_ms(), Ordering::Relaxed);
                self.window_bytes += n as u64;
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {} // socket buffer full: drop
            Err(e) => {
                self.stats.send_errors.fetch_add(1, Ordering::Relaxed);
                if self
                    .last_error_log
                    .is_none_or(|t| t.elapsed() >= Duration::from_secs(10))
                {
                    log::warn!("Audio send to {} failed: {}", self.target, e);
                    self.last_error_log = Some(Instant::now());
                }
            }
        }
        let elapsed = self.window_start.elapsed();
        if elapsed >= Duration::from_secs(1) {
            let kbps = self.window_bytes as f64 * 8.0 / 1000.0 / elapsed.as_secs_f64();
            self.stats.kbps.store(kbps as f32);
            self.window_bytes = 0;
            self.window_start = Instant::now();
        }
    }
}

fn run(ctl: Arc<SessionCtl>, stats: Arc<Stats>, mut sender: Sender) {
    let _com = ComGuard::new();
    let mut backoff = FIRST_BACKOFF;
    while !ctl.should_stop() {
        ctl.clear_restart();
        // "Chosen apps" (Windows 11) or everything this PC plays.
        let result = if crate::settings::get().send_apps_only && super::app_capture::supported() {
            super::app_capture::capture(&ctl, &mut sender)
        } else {
            capture(&ctl, &mut sender)
        };
        match result {
            Ok(()) => backoff = FIRST_BACKOFF,
            Err(e) => {
                if ctl.should_stop() {
                    break;
                }
                log::warn!("Loopback capture stopped: {e}; retrying in {backoff:?}");
                stats.state.store(STATE_RETRYING, Ordering::Relaxed);
                stats.set_error(Some(e));
                ctl.sleep(backoff);
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
    log::info!("Audio sender exited");
}

/// One capture run on the current default device. Returns `Ok` when stopped or
/// when the device should be reopened, `Err` on failures (retried with backoff).
fn capture(ctl: &SessionCtl, sender: &mut Sender) -> Result<(), String> {
    let stats = sender.stats.clone();
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("audio device enumerator unavailable: {e}"))?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| format!("no default output device: {e}"))?;
        let device_id = audio_sessions::device_id(&device).ok();
        let device_name = audio_sessions::friendly_name(&device);

        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("could not open the output device: {e}"))?;
        let mix = client
            .GetMixFormat()
            .map_err(|e| format!("could not read the mix format: {e}"))?;
        let format = parse_mix_format(mix);
        let init = client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            BUFFER_DURATION,
            0,
            mix,
            None,
        );
        CoTaskMemFree(Some(mix as *const _));
        init.map_err(|e| format!("could not start loopback capture: {e}"))?;
        let format = format?;

        let capture_client: IAudioCaptureClient = client
            .GetService()
            .map_err(|e| format!("loopback capture unavailable: {e}"))?;
        client
            .Start()
            .map_err(|e| format!("could not start loopback capture: {e}"))?;

        log::info!(
            "Capturing '{}' ({} Hz, {} ch, {:?})",
            device_name,
            format.sample_rate,
            format.channels,
            format.kind
        );
        stats
            .capture_rate
            .store(format.sample_rate, Ordering::Relaxed);
        stats
            .capture_channels
            .store(format.channels as u32, Ordering::Relaxed);
        stats.set_device(Some(device_name));
        stats.set_error(None);
        stats.state.store(STATE_RUNNING, Ordering::Relaxed);

        let result = pump(ctl, sender, &capture_client, &format, device_id.as_deref());
        let _ = client.Stop();
        result
    }
}

unsafe fn pump(
    ctl: &SessionCtl,
    sender: &mut Sender,
    capture: &IAudioCaptureClient,
    format: &CaptureFormat,
    device_id: Option<&str>,
) -> Result<(), String> {
    let downmix = Downmix::new(format.channels, format.channel_mask);
    let mut pending: Vec<[i16; 2]> = Vec::with_capacity(FRAMES_PER_PACKET * 8);
    let mut last_device_check = Instant::now();
    let failed = |e: windows::core::Error| format!("capture failed: {e}");

    loop {
        if ctl.should_stop() || ctl.restart_requested() {
            return Ok(());
        }
        if last_device_check.elapsed() >= Duration::from_secs(1) {
            last_device_check = Instant::now();
            if crate::settings::get().send_apps_only && super::app_capture::supported() {
                return Ok(()); // switched to "chosen apps"
            }
            // Apps follow the new default device; capturing the old one would go silent.
            if audio_sessions::default_render_device_id().as_deref() != device_id {
                log::info!("Default output device changed; restarting capture");
                return Ok(());
            }
        }

        let available = capture.GetNextPacketSize().map_err(failed)?;
        if available == 0 {
            // Idle: flush the partial packet instead of holding the tail back.
            if !pending.is_empty() {
                sender.send(format.sample_rate, &pending);
                pending.clear();
            }
            std::thread::sleep(Duration::from_millis(3));
            continue;
        }

        let mut data = std::ptr::null_mut();
        let mut frames = 0u32;
        let mut flags = 0u32;
        capture
            .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
            .map_err(failed)?;
        let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
        if silent || data.is_null() {
            // The buffer contents are undefined when flagged silent.
            pending.resize(pending.len() + frames as usize, [0, 0]);
        } else {
            let bytes = std::slice::from_raw_parts(data, frames as usize * format.block_align);
            for frame in bytes.chunks_exact(format.block_align) {
                let [l, r] = downmix.frame(frame, format);
                pending.push([to_i16(l), to_i16(r)]);
            }
        }
        capture.ReleaseBuffer(frames).map_err(failed)?;

        let mut sent = 0;
        while pending.len() - sent >= FRAMES_PER_PACKET {
            sender.send(format.sample_rate, &pending[sent..sent + FRAMES_PER_PACKET]);
            sent += FRAMES_PER_PACKET;
        }
        pending.drain(..sent);
    }
}

/// Copy what we need out of the (packed, COM-allocated) mix format.
unsafe fn parse_mix_format(ptr: *const WAVEFORMATEX) -> Result<CaptureFormat, String> {
    let base = std::ptr::read_unaligned(ptr);
    let (tag, mask) = if base.wFormatTag == WAVE_FORMAT_EXTENSIBLE && base.cbSize >= 22 {
        let ext = std::ptr::read_unaligned(ptr as *const WAVEFORMATEXTENSIBLE);
        let sub = ext.SubFormat;
        // Standard KSDATAFORMAT sub-types carry the WAVE_FORMAT tag in data1.
        let standard = sub.data2 == 0x0000
            && sub.data3 == 0x0010
            && sub.data4 == [0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71];
        if !standard {
            return Err(format!("unsupported mix sub-format {sub:?}"));
        }
        (sub.data1 as u16, ext.dwChannelMask)
    } else {
        (base.wFormatTag, 0)
    };
    CaptureFormat::new(
        tag,
        base.wBitsPerSample,
        base.nChannels,
        base.nSamplesPerSec,
        base.nBlockAlign,
        mask,
    )
}
