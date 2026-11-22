// Trovul - Receiver: UDP → adaptive jitter buffer → cpal playback.

use super::dsp::{Frame, Renderer};
use super::wire::{self, SeqTracker, SeqVerdict};
use super::{SessionCtl, Stats, StreamPeer, AUDIO_PORT, STATE_RETRYING, STATE_RUNNING};
use crate::audio_sessions;
use crate::com::ComGuard;
use crate::util::mono_ms;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use ringbuf::traits::{Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use std::io::ErrorKind;
use std::net::{IpAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// 0.5 s even at 192 kHz — far above the largest target plus allowed excess.
const RING_FRAMES: usize = 96_000;
const FIRST_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(5);

/// Only packets from the paired peer, stamped with this link's session id, are played.
#[derive(Debug, Clone, Copy)]
struct StreamFilter {
    ip: IpAddr,
    session: u32,
}

pub(super) fn spawn(
    ctl: Arc<SessionCtl>,
    stats: Arc<Stats>,
    peer: StreamPeer,
) -> Result<JoinHandle<()>, String> {
    let socket = UdpSocket::bind(("0.0.0.0", AUDIO_PORT))
        .map_err(|e| format!("Could not listen for audio on UDP port {AUDIO_PORT}: {e}"))?;
    socket
        .set_read_timeout(Some(Duration::from_millis(50)))
        .map_err(|e| format!("Could not configure the audio socket: {e}"))?;
    // A deeper kernel buffer rides out short scheduling hiccups on busy machines.
    let _ = socket2::SockRef::from(&socket).set_recv_buffer_size(1 << 20);
    let filter = StreamFilter {
        ip: peer.ip,
        session: peer.session,
    };
    std::thread::Builder::new()
        .name("trovul-audio-receiver".into())
        .spawn(move || run(ctl, stats, Arc::new(socket), filter))
        .map_err(|e| format!("Could not start the audio receiver: {e}"))
}

fn run(ctl: Arc<SessionCtl>, stats: Arc<Stats>, socket: Arc<UdpSocket>, filter: StreamFilter) {
    let _com = ComGuard::new();
    let mut backoff = FIRST_BACKOFF;
    while !ctl.should_stop() {
        ctl.clear_restart();
        match play(&ctl, &stats, &socket, filter) {
            Ok(()) => backoff = FIRST_BACKOFF,
            Err(e) => {
                if ctl.should_stop() {
                    break;
                }
                log::warn!("Playback stopped: {e}; retrying in {backoff:?}");
                stats.state.store(STATE_RETRYING, Ordering::Relaxed);
                stats.set_error(Some(e));
                ctl.sleep(backoff);
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
    log::info!("Audio receiver exited");
}

/// Which endpoint playback should use right now.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Selected(String),
    /// Following the Windows default (its id, to notice when it changes).
    SystemDefault(Option<String>),
}

fn desired_target() -> Target {
    if let Some(id) = super::output_device() {
        if audio_sessions::is_render_device_active(&id) {
            return Target::Selected(id);
        }
    }
    Target::SystemDefault(audio_sessions::default_render_device_id())
}

fn open_device(host: &cpal::Host, target: &Target) -> Result<cpal::Device, String> {
    if let Target::Selected(id) = target {
        // cpal doesn't expose endpoint ids; its device names are the friendly names.
        match audio_sessions::device_friendly_name(id) {
            Ok(name) => {
                let found = host
                    .output_devices()
                    .map_err(|e| format!("could not list output devices: {e}"))?
                    .find(|d| d.name().is_ok_and(|n| n == name));
                if let Some(device) = found {
                    return Ok(device);
                }
                log::warn!(
                    "Output '{name}' not available to the audio backend; using the system default"
                );
            }
            Err(e) => log::warn!("Selected output unavailable ({e}); using the system default"),
        }
    }
    host.default_output_device()
        .ok_or_else(|| "no output device available".to_string())
}

/// One playback run: network thread + output stream on the current target.
/// `Ok` when stopped or when the device should be reopened, `Err` on failure.
fn play(
    ctl: &SessionCtl,
    stats: &Arc<Stats>,
    socket: &Arc<UdpSocket>,
    filter: StreamFilter,
) -> Result<(), String> {
    let host = cpal::default_host();
    let target = desired_target();
    let device = open_device(&host, &target)?;
    let (producer, consumer) = HeapRb::<Frame>::new(RING_FRAMES).split();
    let net_stop = Arc::new(AtomicBool::new(false));
    let net = spawn_network(
        socket.clone(),
        filter,
        producer,
        stats.clone(),
        net_stop.clone(),
    )?;

    let result = run_stream(ctl, stats, &device, &target, consumer);

    net_stop.store(true, Ordering::Release);
    let _ = net.join();
    result
}

fn run_stream(
    ctl: &SessionCtl,
    stats: &Stats,
    device: &cpal::Device,
    target: &Target,
    consumer: HeapCons<Frame>,
) -> Result<(), String> {
    let device_name = device.name().unwrap_or_else(|_| "Unknown device".into());
    let selected_id = match target {
        Target::Selected(id) => Some(id.as_str()),
        Target::SystemDefault(_) => None,
    };
    audio_sessions::disable_ducking(selected_id);

    let supported = device
        .default_output_config()
        .map_err(|e| format!("could not read the output format of '{device_name}': {e}"))?;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let out_rate = config.sample_rate.0;
    let out_channels = config.channels;

    let renderer = Renderer::new(
        consumer,
        stats.jitter.clone(),
        out_rate,
        super::latency_config(),
    );
    let failed = Arc::new(AtomicBool::new(false));
    let stream = match sample_format {
        SampleFormat::F32 => build::<f32>(device, &config, renderer, failed.clone()),
        SampleFormat::I16 => build::<i16>(device, &config, renderer, failed.clone()),
        SampleFormat::U16 => build::<u16>(device, &config, renderer, failed.clone()),
        SampleFormat::I32 => build::<i32>(device, &config, renderer, failed.clone()),
        SampleFormat::F64 => build::<f64>(device, &config, renderer, failed.clone()),
        other => Err(format!("unsupported output sample format {other:?}")),
    }?;
    stream
        .play()
        .map_err(|e| format!("could not start playback on '{device_name}': {e}"))?;

    log::info!("Playing remote audio on '{device_name}' ({out_rate} Hz, {out_channels} ch, {sample_format:?})");
    stats.output_rate.store(out_rate, Ordering::Relaxed);
    stats
        .output_channels
        .store(out_channels as u32, Ordering::Relaxed);
    stats.set_device(Some(device_name));
    stats.set_error(None);
    stats.state.store(STATE_RUNNING, Ordering::Relaxed);

    supervise(ctl, stats, &failed, target)
    // `stream` is dropped here, closing the device before any reopen.
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut renderer: Renderer<HeapCons<Frame>>,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = (config.channels as usize).max(1);
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                renderer.begin_block(data.len() / channels);
                for frame in data.chunks_exact_mut(channels) {
                    let [l, r] = renderer.next_frame();
                    if channels == 1 {
                        frame[0] = T::from_sample((l + r) * 0.5);
                    } else {
                        // Stereo goes to front left/right; extra channels stay silent.
                        frame[0] = T::from_sample(l);
                        frame[1] = T::from_sample(r);
                        for sample in &mut frame[2..] {
                            *sample = T::EQUILIBRIUM;
                        }
                    }
                }
                renderer.end_block();
            },
            move |err| {
                log::error!("Output stream error: {err}");
                failed.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(|e| format!("could not open the output stream: {e}"))
}

fn supervise(
    ctl: &SessionCtl,
    stats: &Stats,
    failed: &AtomicBool,
    target: &Target,
) -> Result<(), String> {
    let mut last_device_check = Instant::now();
    let mut last_log = Instant::now();
    loop {
        if ctl.should_stop() {
            return Ok(());
        }
        if ctl.restart_requested() {
            log::info!("Reopening playback (device change or resume from sleep)");
            return Ok(());
        }
        if failed.load(Ordering::Acquire) {
            return Err("the output device stopped working".into());
        }
        if last_device_check.elapsed() >= Duration::from_secs(1) {
            last_device_check = Instant::now();
            if desired_target() != *target {
                log::info!("Output device changed; reopening playback");
                return Ok(());
            }
        }
        if last_log.elapsed() >= Duration::from_secs(30) {
            last_log = Instant::now();
            let j = &stats.jitter;
            log::debug!(
                "playback: buffer {:.1}/{:.0} ms, drift {:.0} ppm, lost {}, late {}, underruns {}",
                j.buffer_ms.load(),
                j.target_ms.load(),
                j.drift_ppm.load(),
                stats.packets_lost.load(Ordering::Relaxed),
                stats.packets_late.load(Ordering::Relaxed),
                j.underruns.load(Ordering::Relaxed),
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn spawn_network(
    socket: Arc<UdpSocket>,
    filter: StreamFilter,
    mut producer: HeapProd<Frame>,
    stats: Arc<Stats>,
    stop: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, String> {
    std::thread::Builder::new()
        .name("trovul-audio-net".into())
        .spawn(move || {
            let jitter = stats.jitter.clone();
            let mut buf = [0u8; 2048];
            let mut sequence = SeqTracker::default();
            let mut window_bytes = 0u64;
            let mut window_start = Instant::now();
            let mut last_error_log: Option<Instant> = None;

            while !stop.load(Ordering::Acquire) {
                match socket.recv_from(&mut buf) {
                    Ok((len, from)) => {
                        if from.ip() != filter.ip {
                            stats.packets_rejected.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        let Ok((header, payload)) = wire::decode_header(&buf[..len]) else {
                            stats.packets_rejected.fetch_add(1, Ordering::Relaxed);
                            continue;
                        };
                        if header.session != filter.session {
                            stats.packets_rejected.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        match sequence.check(header.seq) {
                            SeqVerdict::Late => {
                                stats.packets_late.fetch_add(1, Ordering::Relaxed);
                                continue;
                            }
                            SeqVerdict::Gap(missing) => {
                                stats
                                    .packets_lost
                                    .fetch_add(missing as u64, Ordering::Relaxed);
                            }
                            SeqVerdict::InOrder | SeqVerdict::Reset => {}
                        }
                        stats.packets_received.fetch_add(1, Ordering::Relaxed);
                        window_bytes += len as u64;

                        let previous_rate =
                            jitter.input_rate.swap(header.sample_rate, Ordering::AcqRel);
                        if previous_rate != 0 && previous_rate != header.sample_rate {
                            jitter.flush.store(true, Ordering::Release);
                        }
                        let mut overflowed = false;
                        wire::for_each_frame(payload, header.channels, |frame| {
                            if producer.try_push(frame).is_err() {
                                overflowed = true;
                            }
                        });
                        if overflowed {
                            stats.overflows.fetch_add(1, Ordering::Relaxed);
                        }
                        jitter.last_packet_ms.store(mono_ms(), Ordering::Relaxed);
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            ErrorKind::WouldBlock
                                | ErrorKind::TimedOut
                                | ErrorKind::ConnectionReset
                        ) => {}
                    Err(e) => {
                        if last_error_log.is_none_or(|t| t.elapsed() >= Duration::from_secs(10)) {
                            log::warn!("Audio receive error: {e}");
                            last_error_log = Some(Instant::now());
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
                let elapsed = window_start.elapsed();
                if elapsed >= Duration::from_secs(1) {
                    let kbps = window_bytes as f64 * 8.0 / 1000.0 / elapsed.as_secs_f64();
                    stats.kbps.store(kbps as f32);
                    window_bytes = 0;
                    window_start = Instant::now();
                }
            }
        })
        .map_err(|e| format!("could not start the audio network thread: {e}"))
}
