// Trovul - Sender, "chosen apps" mode: instead of everything this PC plays,
// only the apps the user picked go to the other PC, each at its own level.
//
// Windows 11 can capture one process's sound (process loopback: activation
// through ActivateAudioInterfaceAsync on VAD\Process_Loopback). Each chosen
// app that plays gets its own capture at 48 kHz stereo float; a clock mixes
// what they delivered every 5 ms, 20 ms behind real time so a late source
// doesn't break up, and hands the mix to the same packets as the whole-device
// capture (capture.rs). Apps that start or stop playing are picked up within
// a second.

use super::capture::Sender;
use super::dsp::to_i16;
use super::wire::FRAMES_PER_PACKET;
use super::{SessionCtl, STATE_RUNNING};
use crate::util::lock;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::core::{implement, Interface, HRESULT};
use windows::Win32::Media::Audio::{
    ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioCaptureClient, IAudioClient, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVEFORMATEX,
};
use windows::Win32::System::Com::IAgileObject;

const RATE: u32 = 48_000;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
/// Each app's capture buffer (100 ns units).
const BUFFER_DURATION: i64 = 2_000_000;
/// The mix runs this far behind real time, so a late source isn't cut.
const BEHIND: u64 = RATE as u64 / 50; // 20 ms
/// More than this waiting in a source means it drifted: drop the oldest.
const MAX_QUEUED: usize = RATE as usize / 5; // 200 ms

/// Windows 11 (build 20348+) can capture one app's sound.
pub fn supported() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| windows_build() >= 20348)
}

fn windows_build() -> u32 {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let mut buf = [0u16; 32];
    let mut size = std::mem::size_of_val(&buf) as u32;
    let read = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"),
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if read.is_err() {
        return 0;
    }
    let text = String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)]);
    text.trim().parse().unwrap_or(0)
}

// ---------- The chosen apps and their levels (settings mirror) ----------

/// Chosen app (lower-case process name) → send level. Read by the mixer
/// every 5 ms; written when the user changes a level.
static CHOSEN: Mutex<Option<HashMap<String, f32>>> = Mutex::new(None);

pub fn set_chosen(apps: &[crate::settings::SendApp]) {
    *lock(&CHOSEN) = Some(
        apps.iter()
            .map(|a| (a.name.to_lowercase(), a.level.clamp(0.0, 1.0)))
            .collect(),
    );
}

fn chosen() -> HashMap<String, f32> {
    lock(&CHOSEN).clone().unwrap_or_default()
}

// ---------- Activation ----------

#[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
struct Activation {
    done: Mutex<Option<mpsc::Sender<()>>>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for Activation_Impl {
    fn ActivateCompleted(
        &self,
        _operation: Option<&IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        if let Some(done) = lock(&self.done).take() {
            let _ = done.send(());
        }
        Ok(())
    }
}

impl windows::Win32::System::Com::IAgileObject_Impl for Activation_Impl {}

/// A PROPVARIANT holding a VT_BLOB, laid out as Windows expects.
#[repr(C)]
struct BlobVariant {
    vt: u16,
    reserved: [u16; 3],
    size: u32,
    data: *const u8,
}

const VT_BLOB: u16 = 65;

/// One app's capture.
struct Source {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    queue: VecDeque<[f32; 2]>,
}

impl Drop for Source {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

unsafe fn open(pid: u32) -> Result<Source, String> {
    let params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };
    let blob = BlobVariant {
        vt: VT_BLOB,
        reserved: [0; 3],
        size: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
        data: &params as *const _ as *const u8,
    };
    let (tx, rx) = mpsc::channel();
    let handler: IActivateAudioInterfaceCompletionHandler = Activation {
        done: Mutex::new(Some(tx)),
    }
    .into();
    let operation = ActivateAudioInterfaceAsync(
        VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        &IAudioClient::IID,
        Some(&blob as *const BlobVariant as *const windows::core::PROPVARIANT),
        &handler,
    )
    .map_err(|e| format!("activation: {e}"))?;
    rx.recv_timeout(Duration::from_secs(3))
        .map_err(|_| "activation timed out".to_string())?;
    let mut result = HRESULT(0);
    let mut activated = None;
    operation
        .GetActivateResult(&mut result, &mut activated)
        .map_err(|e| format!("activation result: {e}"))?;
    result.ok().map_err(|e| format!("activation: {e}"))?;
    let client: IAudioClient = activated
        .ok_or("no audio client")?
        .cast()
        .map_err(|e| e.to_string())?;
    let format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
        nChannels: 2,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * 8,
        nBlockAlign: 8,
        wBitsPerSample: 32,
        cbSize: 0,
    };
    client
        .Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            BUFFER_DURATION,
            0,
            &format,
            None,
        )
        .map_err(|e| format!("initialize: {e}"))?;
    let capture: IAudioCaptureClient = client
        .GetService()
        .map_err(|e| format!("capture client: {e}"))?;
    client.Start().map_err(|e| format!("start: {e}"))?;
    Ok(Source {
        client,
        capture,
        queue: VecDeque::with_capacity(RATE as usize / 10),
    })
}

/// Move what the app delivered into its queue.
unsafe fn drain(source: &mut Source) -> windows::core::Result<()> {
    loop {
        if source.capture.GetNextPacketSize()? == 0 {
            return Ok(());
        }
        let mut data = std::ptr::null_mut();
        let mut frames = 0u32;
        let mut flags = 0u32;
        source
            .capture
            .GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
        if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
            source
                .queue
                .extend(std::iter::repeat_n([0.0, 0.0], frames as usize));
        } else {
            let samples = std::slice::from_raw_parts(data as *const f32, frames as usize * 2);
            source
                .queue
                .extend(samples.chunks_exact(2).map(|s| [s[0], s[1]]));
        }
        source.capture.ReleaseBuffer(frames)?;
        if source.queue.len() > MAX_QUEUED {
            let extra = source.queue.len() - MAX_QUEUED;
            source.queue.drain(..extra);
        }
    }
}

/// The chosen apps' processes that play sound now: pid → lower-case name.
fn playing_chosen(chosen: &HashMap<String, f32>) -> HashMap<u32, String> {
    crate::audio_sessions::get_mixer_snapshot()
        .map(|mixer| {
            mixer
                .sessions
                .into_iter()
                .map(|s| (s.pid, s.name.to_lowercase()))
                .filter(|(_, name)| chosen.contains_key(name))
                .collect()
        })
        .unwrap_or_default()
}

/// Mix `frames` frames from the sources at their levels.
fn mix(
    sources: &mut HashMap<u32, (String, Source)>,
    levels: &HashMap<String, f32>,
    out: &mut Vec<[i16; 2]>,
) {
    let mut acc = vec![[0.0f32; 2]; FRAMES_PER_PACKET];
    for (name, source) in sources.values_mut() {
        let level = levels.get(name).copied().unwrap_or(0.0);
        for slot in acc.iter_mut() {
            let Some([l, r]) = source.queue.pop_front() else {
                break;
            };
            slot[0] += l * level;
            slot[1] += r * level;
        }
    }
    out.clear();
    out.extend(
        acc.iter()
            .map(|[l, r]| [to_i16(l.clamp(-1.0, 1.0)), to_i16(r.clamp(-1.0, 1.0))]),
    );
}

/// Capture and send the chosen apps until stopped, or until the mode changes
/// back to "all sound" (then `Ok`, and capture.rs takes over).
pub(super) fn capture(ctl: &SessionCtl, sender: &mut Sender) -> Result<(), String> {
    let stats = sender.stats.clone();
    stats.capture_rate.store(RATE, Ordering::Relaxed);
    stats.capture_channels.store(2, Ordering::Relaxed);
    stats.set_error(None);
    stats.state.store(STATE_RUNNING, Ordering::Relaxed);
    log::info!("Sending chosen apps only");

    let mut sources: HashMap<u32, (String, Source)> = HashMap::new();
    let mut failed: HashMap<u32, Instant> = HashMap::new();
    let mut frames = Vec::with_capacity(FRAMES_PER_PACKET);
    let start = Instant::now();
    let mut produced: u64 = 0;
    let mut last_scan = Instant::now() - Duration::from_secs(5);

    loop {
        if ctl.should_stop() || ctl.restart_requested() {
            return Ok(());
        }
        if !crate::settings::get().send_apps_only {
            return Ok(());
        }
        let levels = chosen();
        if last_scan.elapsed() >= Duration::from_secs(1) {
            last_scan = Instant::now();
            let wanted = playing_chosen(&levels);
            sources.retain(|pid, _| wanted.contains_key(pid));
            for (pid, name) in wanted {
                if sources.contains_key(&pid)
                    || failed
                        .get(&pid)
                        .is_some_and(|t| t.elapsed() < Duration::from_secs(10))
                {
                    continue;
                }
                match unsafe { open(pid) } {
                    Ok(source) => {
                        log::info!("Sending {name} (pid {pid})");
                        sources.insert(pid, (name, source));
                    }
                    Err(e) => {
                        log::warn!("Could not capture {name} (pid {pid}): {e}");
                        failed.insert(pid, Instant::now());
                    }
                }
            }
            let names: Vec<&str> = {
                let mut n: Vec<&str> = sources.values().map(|(name, _)| name.as_str()).collect();
                n.sort_unstable();
                n.dedup();
                n
            };
            stats.set_device(Some(match names.len() {
                0 => "Chosen apps (none playing)".to_string(),
                _ => format!("Chosen apps: {}", names.join(", ")),
            }));
        }
        let mut broken = Vec::new();
        for (pid, (_, source)) in sources.iter_mut() {
            if unsafe { drain(source) }.is_err() {
                broken.push(*pid);
            }
        }
        for pid in broken {
            sources.remove(&pid);
        }
        let due = (start.elapsed().as_secs_f64() * RATE as f64) as u64;
        while produced + FRAMES_PER_PACKET as u64 + BEHIND <= due {
            mix(&mut sources, &levels, &mut frames);
            sender.send(RATE, &frames);
            produced += FRAMES_PER_PACKET as u64;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_build_is_read() {
        let build = windows_build();
        assert!(build >= 10240, "read build {build}");
        println!(
            "Windows build {build}: chosen apps {}",
            if supported() {
                "supported"
            } else {
                "not supported"
            }
        );
    }

    /// On Windows 11 only: capture this test process (it plays a quiet tone)
    /// and check sound arrives. `cargo test -- --ignored hardware`.
    #[test]
    #[ignore]
    fn hardware_captures_one_app() {
        if !supported() {
            println!("skipped: needs Windows 11");
            return;
        }
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let _com = crate::com::ComGuard::new();
        let device = cpal::default_host()
            .default_output_device()
            .expect("no output device");
        let config = device.default_output_config().unwrap().config();
        let channels = config.channels as usize;
        let mut phase = 0f32;
        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    for frame in data.chunks_mut(channels) {
                        phase = (phase + 440.0 / 48_000.0) % 1.0;
                        let v = (phase * std::f32::consts::TAU).sin() * 0.001; // inaudible
                        frame.iter_mut().for_each(|s| *s = v);
                    }
                },
                |e| eprintln!("{e}"),
                None,
            )
            .unwrap();
        stream.play().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let mut source = unsafe { open(std::process::id()) }.expect("capture this process");
        std::thread::sleep(Duration::from_millis(400));
        unsafe { drain(&mut source) }.unwrap();
        let frames = source.queue.len();
        let peak = source
            .queue
            .iter()
            .map(|[l, r]| l.abs().max(r.abs()))
            .fold(0.0, f32::max);
        drop(stream);
        println!("captured {frames} frames of this process, peak {peak:.5}");
        assert!(frames > 4_800, "no sound captured");
        assert!(peak > 0.0001, "only silence captured");
    }
}
