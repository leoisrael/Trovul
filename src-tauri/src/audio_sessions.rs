// Trovul - Windows Core Audio: per-process sessions, endpoint devices, master
// volume and process icons. Every public function initialises COM for the
// calling thread first (see `com.rs`).

use crate::com::ensure_initialized;
use crate::util::{lock, mono_ms};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::sync::{LazyLock, Mutex};
use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, BOOL, HANDLE, MAX_PATH};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC,
    SelectObject, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, AudioSessionStateExpired, EDataFlow, IAudioSessionControl,
    IAudioSessionControl2, IAudioSessionManager2, IChannelAudioVolume, IMMDevice,
    IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ};
use windows::Win32::System::ProcessStatus::K32GetModuleBaseNameW;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Shell::ExtractIconExW;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

// ============ Types ============

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioProcess {
    pub id: String,
    pub pid: u32,
    pub name: String,
    pub volume: f32,
    pub muted: bool,
    pub peak_level: f32,
    /// Key for `get_icons`; icons are fetched once instead of with every poll.
    pub icon_id: Option<String>,
    /// Output device chosen for this app (endpoint id); `None` = the Windows
    /// default. Absent from peers before protocol 2.2.
    #[serde(default)]
    pub output: Option<String>,
    /// Goes to the other PC in "chosen apps" mode, and how loud (Windows 11).
    #[serde(default)]
    pub send: Option<AppSend>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSend {
    pub on: bool,
    pub level: f32,
}

/// What this PC sends to the other: everything, or the chosen apps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMix {
    /// This Windows can capture single apps (Windows 11).
    pub supported: bool,
    pub apps_only: bool,
    /// The chosen apps, playing or not (names as the mixer shows them).
    #[serde(default)]
    pub chosen: Vec<String>,
}

/// The default output device, shown as the "Master" strip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MasterInfo {
    pub id: String,
    pub name: String,
    pub volume: f32,
    pub muted: bool,
    pub peak: f32,
}

/// Trovul's own stream on a PC, shown as its own strip: the other PC's sound
/// playing there, or that PC's sound going out. Filled in by `lib.rs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamBus {
    /// Follow the pointer is acting: the pointer is here (true) or on the
    /// other PC. None when it's off or doesn't apply.
    #[serde(default)]
    pub focus: Option<bool>,
    /// The other PC's sound plays here; otherwise this PC's sound is sent.
    pub receiving: bool,
    /// Volume and mute of the sound received.
    pub volume: f32,
    pub muted: bool,
    /// Stream level, 0–1.
    pub level: f32,
    /// Lowered while someone talks in a call.
    pub ducked: bool,
    /// Device playing the sound received (endpoint id); `None` = default.
    pub output: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MixerSnapshot {
    pub master: Option<MasterInfo>,
    pub sessions: Vec<AudioProcess>,
    /// Apps can be sent to their own output device (`app_routing`).
    #[serde(default)]
    pub per_app_output: bool,
    /// Present while audio flows between the PCs.
    #[serde(default)]
    pub stream: Option<StreamBus>,
    /// What this PC sends (protocol 2.5+).
    #[serde(default)]
    pub send_mix: Option<SendMix>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub device_type: String, // "output" or "input"
    pub volume: f32,
    pub muted: bool,
    pub is_default: bool,
}

// ============ Device helpers ============

fn enumerator() -> Result<IMMDeviceEnumerator, String> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .map_err(|e| format!("Audio device enumerator unavailable: {e}"))
}

fn default_render() -> Result<IMMDevice, String> {
    let enumerator = enumerator()?;
    unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .map_err(|e| format!("No default output device: {e}"))
}

fn device_by_id(id: &str) -> Result<IMMDevice, String> {
    let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    let enumerator = enumerator()?;
    unsafe { enumerator.GetDevice(PCWSTR(wide.as_ptr())) }
        .map_err(|e| format!("Audio device not found: {e}"))
}

/// Convert and free a string allocated by COM (e.g. `IMMDevice::GetId`).
unsafe fn take_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let text = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as *const _));
    text
}

pub(crate) unsafe fn device_id(device: &IMMDevice) -> Result<String, String> {
    device
        .GetId()
        .map(|p| take_pwstr(p))
        .map_err(|e| format!("Device id unavailable: {e}"))
}

pub(crate) unsafe fn friendly_name(device: &IMMDevice) -> String {
    device
        .OpenPropertyStore(STGM_READ)
        .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
        .map(|value| value.to_string())
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Unknown device".to_string())
}

pub fn default_render_device_id() -> Option<String> {
    ensure_initialized();
    let device = default_render().ok()?;
    unsafe { device_id(&device).ok() }
}

pub fn is_render_device_active(id: &str) -> bool {
    ensure_initialized();
    device_by_id(id)
        .ok()
        .and_then(|device| unsafe { device.GetState() }.ok())
        .is_some_and(|state| state == DEVICE_STATE_ACTIVE)
}

pub fn device_friendly_name(id: &str) -> Result<String, String> {
    ensure_initialized();
    let device = device_by_id(id)?;
    Ok(unsafe { friendly_name(&device) })
}

fn checked_volume(volume: f32) -> Result<f32, String> {
    if volume.is_finite() {
        Ok(volume.clamp(0.0, 1.0))
    } else {
        Err("Invalid volume".into())
    }
}

// ============ Devices ============

pub fn get_output_devices() -> Result<Vec<AudioDevice>, String> {
    list_devices(eRender, "output")
}

pub fn get_input_devices() -> Result<Vec<AudioDevice>, String> {
    list_devices(eCapture, "input")
}

fn list_devices(flow: EDataFlow, kind: &str) -> Result<Vec<AudioDevice>, String> {
    ensure_initialized();
    let enumerator = enumerator()?;
    unsafe {
        let default_id = enumerator
            .GetDefaultAudioEndpoint(flow, eConsole)
            .ok()
            .and_then(|device| device_id(&device).ok());
        let collection = enumerator
            .EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)
            .map_err(|e| format!("Could not list audio devices: {e}"))?;
        let count = collection.GetCount().unwrap_or(0);
        let mut devices = Vec::with_capacity(count as usize);
        for i in 0..count {
            let Ok(device) = collection.Item(i) else {
                continue;
            };
            let Ok(id) = device_id(&device) else { continue };
            let Ok(endpoint) = device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) else {
                continue;
            };
            devices.push(AudioDevice {
                is_default: default_id.as_deref() == Some(id.as_str()),
                name: friendly_name(&device),
                device_type: kind.to_string(),
                volume: endpoint.GetMasterVolumeLevelScalar().unwrap_or(1.0),
                muted: endpoint.GetMute().map(|m| m.as_bool()).unwrap_or(false),
                id,
            });
        }
        Ok(devices)
    }
}

fn endpoint_call(
    device: &IMMDevice,
    call: impl FnOnce(&IAudioEndpointVolume) -> windows::core::Result<()>,
) -> Result<(), String> {
    let endpoint: IAudioEndpointVolume = unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|e| format!("Volume control unavailable: {e}"))?;
    call(&endpoint).map_err(|e| format!("Could not change the volume: {e}"))
}

pub fn set_device_volume(device_id: &str, volume: f32) -> Result<(), String> {
    let volume = checked_volume(volume)?;
    ensure_initialized();
    endpoint_call(&device_by_id(device_id)?, |e| unsafe {
        e.SetMasterVolumeLevelScalar(volume, std::ptr::null())
    })
}

pub fn set_device_mute(device_id: &str, muted: bool) -> Result<(), String> {
    ensure_initialized();
    endpoint_call(&device_by_id(device_id)?, |e| unsafe {
        e.SetMute(muted, std::ptr::null())
    })
}

// ============ Master (default output) ============

pub fn set_master_volume(volume: f32) -> Result<(), String> {
    let volume = checked_volume(volume)?;
    ensure_initialized();
    endpoint_call(&default_render()?, |e| unsafe {
        e.SetMasterVolumeLevelScalar(volume, std::ptr::null())
    })
}

pub fn set_master_mute(muted: bool) -> Result<(), String> {
    ensure_initialized();
    endpoint_call(&default_render()?, |e| unsafe {
        e.SetMute(muted, std::ptr::null())
    })
}

// ============ Sessions ============

/// Sessions on every active output device, the default one first. Apps sent
/// to their own output (`app_routing`) play on other devices.
fn render_sessions() -> Result<Vec<IAudioSessionControl>, String> {
    let enumerator = enumerator()?;
    let mut devices: Vec<IMMDevice> = default_render().into_iter().collect();
    unsafe {
        let default_id = devices.first().and_then(|d| device_id(d).ok());
        let all = enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(|e| format!("Could not list audio devices: {e}"))?;
        for i in 0..all.GetCount().unwrap_or(0) {
            let Ok(device) = all.Item(i) else {
                continue;
            };
            if default_id.is_none() || device_id(&device).ok() != default_id {
                devices.push(device);
            }
        }
        let mut sessions = Vec::new();
        for device in devices {
            let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let Ok(list) = manager.GetSessionEnumerator() else {
                continue;
            };
            sessions
                .extend((0..list.GetCount().unwrap_or(0)).filter_map(|i| list.GetSession(i).ok()));
        }
        Ok(sessions)
    }
}

/// What the last scan found. Finding the sessions and the default device
/// costs tens of milliseconds (calls into the audio service); reading volumes
/// and meters from what was found costs microseconds. The mixer polls every
/// 100 ms on both PCs, so snapshots rescan once a second and read every time.
struct Scan {
    at: u64,
    master: Option<MasterHandle>,
    sessions: Vec<Tracked>,
}

struct MasterHandle {
    id: String,
    name: String,
    volume: IAudioEndpointVolume,
    meter: Option<IAudioMeterInformation>,
}

struct Tracked {
    pid: u32,
    name: String,
    icon_id: Option<String>,
    volume: ISimpleAudioVolume,
    meter: Option<IAudioMeterInformation>,
}

// SAFETY: the objects are proxies to the audio service, created and used only
// on MTA threads (snapshots and volume changes run on the blocking pool, the
// link's request thread and the companion server, which all call
// `ensure_initialized`), never on the UI thread.
unsafe impl Send for Scan {}

static SCAN: Mutex<Option<Scan>> = Mutex::new(None);
const RESCAN_MS: u64 = 1000;

fn scan() -> Result<Scan, String> {
    let master = default_render().ok().and_then(|device| unsafe {
        Some(MasterHandle {
            id: device_id(&device).ok()?,
            name: friendly_name(&device),
            volume: device.Activate(CLSCTX_ALL, None).ok()?,
            meter: device.Activate(CLSCTX_ALL, None).ok(),
        })
    });
    let own_pid = std::process::id();
    let mut names: HashMap<u32, (String, Option<String>)> = HashMap::new();
    let mut sessions = Vec::new();
    for session in render_sessions()? {
        unsafe {
            if session
                .GetState()
                .is_ok_and(|state| state == AudioSessionStateExpired)
            {
                continue;
            }
            let Ok(control) = session.cast::<IAudioSessionControl2>() else {
                continue;
            };
            let pid = control.GetProcessId().unwrap_or(0);
            if pid == 0 || pid == own_pid {
                continue; // system sounds, and our own playback
            }
            let Ok(volume) = session.cast::<ISimpleAudioVolume>() else {
                continue;
            };
            let (name, icon_id) = names
                .entry(pid)
                .or_insert_with(|| {
                    let (name, exe_path) = process_info(pid);
                    (name, icon_id_for(&exe_path))
                })
                .clone();
            sessions.push(Tracked {
                pid,
                name,
                icon_id,
                volume,
                meter: session.cast().ok(),
            });
        }
    }
    Ok(Scan {
        at: mono_ms(),
        master,
        sessions,
    })
}

/// Run `read` on a scan at most `RESCAN_MS` old.
fn with_scan<T>(read: impl FnOnce(&Scan) -> T) -> Result<T, String> {
    ensure_initialized();
    let mut cached = lock(&SCAN);
    let scan = match cached.take() {
        Some(scan) if mono_ms().saturating_sub(scan.at) < RESCAN_MS => scan,
        _ => scan()?,
    };
    Ok(read(cached.insert(scan)))
}

/// Scan again on the next snapshot, e.g. after an app moved to another device.
pub fn rescan_soon() {
    lock(&SCAN).take();
}

/// Master info plus one entry per process playing on any output device, apart
/// from Trovul itself (the incoming sound has its own controls).
/// Icons are referenced by id; fetch them with `get_icons`.
pub fn get_mixer_snapshot() -> Result<MixerSnapshot, String> {
    let (master, mut sessions) = with_scan(|scan| unsafe {
        let master = scan.master.as_ref().map(|m| MasterInfo {
            id: m.id.clone(),
            name: m.name.clone(),
            volume: m.volume.GetMasterVolumeLevelScalar().unwrap_or(1.0),
            muted: m.volume.GetMute().map(|v| v.as_bool()).unwrap_or(false),
            peak: m
                .meter
                .as_ref()
                .and_then(|meter| meter.GetPeakValue().ok())
                .unwrap_or(0.0),
        });
        let mut sessions: Vec<AudioProcess> = Vec::new();
        let mut index_by_pid: HashMap<u32, usize> = HashMap::new();
        for tracked in &scan.sessions {
            let peak = tracked
                .meter
                .as_ref()
                .and_then(|meter| meter.GetPeakValue().ok())
                .unwrap_or(0.0);
            if let Some(&index) = index_by_pid.get(&tracked.pid) {
                // A process with several sessions shows as one strip.
                sessions[index].peak_level = sessions[index].peak_level.max(peak);
                continue;
            }
            index_by_pid.insert(tracked.pid, sessions.len());
            sessions.push(AudioProcess {
                id: format!("session_{}", tracked.pid),
                pid: tracked.pid,
                name: tracked.name.clone(),
                volume: tracked.volume.GetMasterVolume().unwrap_or(1.0),
                muted: tracked
                    .volume
                    .GetMute()
                    .map(|m| m.as_bool())
                    .unwrap_or(false),
                peak_level: peak,
                icon_id: tracked.icon_id.clone(),
                output: None,
                send: None,
            });
        }
        (master, sessions)
    })?;
    let pids: Vec<u32> = sessions.iter().map(|s| s.pid).collect();
    let outputs = crate::app_routing::app_outputs(&pids);
    if let Some(outputs) = &outputs {
        for session in &mut sessions {
            session.output = outputs.get(&session.pid).cloned().flatten();
        }
    }
    Ok(MixerSnapshot {
        master,
        sessions,
        per_app_output: outputs.is_some(),
        stream: None,
        send_mix: None,
    })
}

/// Level meters of chosen apps' sessions on every active output device.
/// Finding sessions costs milliseconds (several calls into the audio
/// service), reading a meter microseconds: `rescan` now and then, `loudest`
/// as often as needed. Keep it on the thread that created it.
#[derive(Default)]
pub struct AppMeters {
    meters: Vec<IAudioMeterInformation>,
    /// PID → passes `matches`; process names cost far more than meters.
    verdicts: HashMap<u32, bool>,
}

impl AppMeters {
    /// Find the sessions of processes whose exe name (lower-case, without
    /// ".exe") passes `matches`.
    pub fn rescan(&mut self, matches: impl Fn(&str) -> bool) -> Result<(), String> {
        ensure_initialized();
        let enumerator = enumerator()?;
        self.meters.clear();
        unsafe {
            let devices = enumerator
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
                .map_err(|e| format!("Could not list audio devices: {e}"))?;
            for i in 0..devices.GetCount().unwrap_or(0) {
                let Ok(device) = devices.Item(i) else {
                    continue;
                };
                let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                    continue;
                };
                let Ok(sessions) = manager.GetSessionEnumerator() else {
                    continue;
                };
                for j in 0..sessions.GetCount().unwrap_or(0) {
                    let Ok(session) = sessions.GetSession(j) else {
                        continue;
                    };
                    let Ok(control) = session.cast::<IAudioSessionControl2>() else {
                        continue;
                    };
                    let pid = control.GetProcessId().unwrap_or(0);
                    if pid == 0 {
                        continue;
                    }
                    let matched = *self
                        .verdicts
                        .entry(pid)
                        .or_insert_with(|| matches(&process_info(pid).0.to_lowercase()));
                    if let (true, Ok(meter)) = (matched, session.cast::<IAudioMeterInformation>()) {
                        self.meters.push(meter);
                    }
                }
            }
        }
        Ok(())
    }

    /// Forget the per-PID answers (PIDs get reused).
    pub fn forget_processes(&mut self) {
        self.verdicts.clear();
    }

    pub fn sessions(&self) -> usize {
        self.meters.len()
    }

    /// Loudest current peak (0–1) among the sessions found by the last rescan.
    pub fn loudest(&self) -> f32 {
        self.meters
            .iter()
            .filter_map(|meter| unsafe { meter.GetPeakValue() }.ok())
            .fold(0.0, f32::max)
    }
}

/// Run `apply` on the sessions of `pid` that the mixer shows (the last scan):
/// a slider drag sends dozens of changes, too many to scan for each.
fn apply_to_app(
    pid: u32,
    apply: impl Fn(&ISimpleAudioVolume) -> windows::core::Result<()>,
) -> Result<(), String> {
    let applied = with_scan(|scan| {
        scan.sessions
            .iter()
            .filter(|tracked| tracked.pid == pid)
            .map(|tracked| apply(&tracked.volume))
            .collect::<windows::core::Result<Vec<_>>>()
            .map(|done| done.len())
    })?
    .map_err(|e| format!("Could not change the app volume: {e}"))?;
    match applied {
        0 => Err(format!("No audio session for process {pid}")),
        _ => Ok(()),
    }
}

pub fn set_session_volume(pid: u32, volume: f32) -> Result<(), String> {
    let volume = checked_volume(volume)?;
    apply_to_app(pid, |s| unsafe {
        s.SetMasterVolume(volume, std::ptr::null())
    })
}

pub fn set_session_mute(pid: u32, muted: bool) -> Result<(), String> {
    apply_to_app(pid, |s| unsafe { s.SetMute(muted, std::ptr::null()) })
}

/// Scale every app's sound (not Trovul's) by `level` through its session's
/// channel volumes: a multiplier apart from the volume the mixer shows.
pub fn set_apps_level(level: f32) {
    let level = level.clamp(0.0, 1.0);
    ensure_initialized();
    let own_pid = std::process::id();
    let Ok(sessions) = render_sessions() else {
        return;
    };
    for session in sessions {
        unsafe {
            let Ok(control) = session.cast::<IAudioSessionControl2>() else {
                continue;
            };
            let pid = control.GetProcessId().unwrap_or(0);
            if pid == 0 || pid == own_pid {
                continue;
            }
            let Ok(channels) = session.cast::<IChannelAudioVolume>() else {
                continue;
            };
            for channel in 0..channels.GetChannelCount().unwrap_or(0) {
                let _ = channels.SetChannelVolume(channel, level, std::ptr::null());
            }
        }
    }
}

/// Mute or unmute every app on every output device, apart from Trovul itself
/// (it isn't in the mixer). Scans afresh: it's one call and must reach all.
pub fn set_all_sessions_mute(muted: bool) -> Result<(), String> {
    ensure_initialized();
    let own_pid = std::process::id();
    for session in render_sessions()? {
        unsafe {
            let Ok(control) = session.cast::<IAudioSessionControl2>() else {
                continue;
            };
            let pid = control.GetProcessId().unwrap_or(0);
            if pid == 0 || pid == own_pid {
                continue;
            }
            if let Ok(volume) = session.cast::<ISimpleAudioVolume>() {
                volume
                    .SetMute(muted, std::ptr::null())
                    .map_err(|e| format!("Could not change the app volume: {e}"))?;
            }
        }
    }
    Ok(())
}

/// Opt Trovul's own playback out of Windows "communications" auto-ducking on
/// the default endpoint and on the selected one — the preference is per endpoint.
pub fn disable_ducking(selected_id: Option<&str>) {
    ensure_initialized();
    let mut targets: Vec<(&str, IMMDevice)> = Vec::new();
    if let Ok(device) = default_render() {
        targets.push(("default output", device));
    }
    if let Some(device) = selected_id.and_then(|id| device_by_id(id).ok()) {
        targets.push(("selected output", device));
    }
    for (label, device) in targets {
        let result = unsafe {
            device
                .Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)
                .and_then(|manager| manager.GetAudioSessionControl(None, 0))
                .and_then(|control| control.cast::<IAudioSessionControl2>())
                .and_then(|control| control.SetDuckingPreference(true))
        };
        match result {
            Ok(()) => log::debug!("Auto-ducking disabled on the {label}"),
            Err(e) => log::warn!("Could not disable auto-ducking on the {label}: {e}"),
        }
    }
}

// ============ Processes & icons ============

fn process_info(pid: u32) -> (String, String) {
    unsafe {
        let handle: HANDLE =
            match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL::from(false), pid) {
                Ok(h) => h,
                Err(_) => return (format!("PID {pid}"), String::new()),
            };

        let mut path_buffer = [0u16; MAX_PATH as usize];
        let mut path_size = MAX_PATH;
        let path = if QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(path_buffer.as_mut_ptr()),
            &mut path_size,
        )
        .is_ok()
        {
            OsString::from_wide(&path_buffer[..path_size as usize])
                .to_string_lossy()
                .to_string()
        } else {
            String::new()
        };

        let mut name_buffer = [0u16; MAX_PATH as usize];
        let name_len = K32GetModuleBaseNameW(handle, None, &mut name_buffer);
        let name = if name_len > 0 {
            let name = OsString::from_wide(&name_buffer[..name_len as usize])
                .to_string_lossy()
                .to_string();
            name.strip_suffix(".exe").unwrap_or(&name).to_string()
        } else if !path.is_empty() {
            let file = path.rsplit('\\').next().unwrap_or(&path);
            file.strip_suffix(".exe").unwrap_or(file).to_string()
        } else {
            format!("PID {pid}")
        };

        let _ = CloseHandle(handle);
        (name, path)
    }
}

const MAX_ICONS_PER_REQUEST: usize = 64;

// icon id → PNG data URL (None when the executable has no icon).
static ICONS: LazyLock<Mutex<HashMap<String, Option<String>>>> = LazyLock::new(Default::default);

/// Stable id for an executable's icon: FNV-1a of the lower-cased path.
fn icon_key(exe_path: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in exe_path.to_lowercase().bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Extract the icon the first time an executable shows up (outside the cache
/// lock) and return its id, or `None` when it has no icon.
fn icon_id_for(exe_path: &str) -> Option<String> {
    if exe_path.is_empty() {
        return None;
    }
    let key = icon_key(exe_path);
    let cached = lock(&ICONS).get(&key).map(|icon| icon.is_some());
    let has_icon = match cached {
        Some(has_icon) => has_icon,
        None => {
            let icon = extract_process_icon(exe_path);
            let has_icon = icon.is_some();
            lock(&ICONS).insert(key.clone(), icon);
            has_icon
        }
    };
    has_icon.then_some(key)
}

pub fn get_icons(ids: &[String]) -> HashMap<String, String> {
    let cache = lock(&ICONS);
    ids.iter()
        .take(MAX_ICONS_PER_REQUEST)
        .filter_map(|id| {
            cache
                .get(id)
                .cloned()
                .flatten()
                .map(|icon| (id.clone(), icon))
        })
        .collect()
}

pub fn icon_data(id: &str) -> Option<String> {
    lock(&ICONS).get(id).cloned().flatten()
}

fn extract_process_icon(exe_path: &str) -> Option<String> {
    unsafe {
        let path_wide: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut large_icon = HICON::default();
        let extracted = ExtractIconExW(
            PCWSTR(path_wide.as_ptr()),
            0,
            Some(&mut large_icon),
            None,
            1,
        );
        if extracted == 0 || large_icon.is_invalid() {
            return None;
        }
        let icon_data = icon_to_base64(large_icon);
        let _ = DestroyIcon(large_icon);
        icon_data
    }
}

unsafe fn icon_to_base64(hicon: HICON) -> Option<String> {
    let mut icon_info = ICONINFO::default();
    if GetIconInfo(hicon, &mut icon_info).is_err() {
        return None;
    }

    let hbm_color = icon_info.hbmColor;
    let release_mask = |info: &ICONINFO| {
        if !info.hbmMask.is_invalid() {
            let _ = DeleteObject(info.hbmMask);
        }
    };
    if hbm_color.is_invalid() {
        release_mask(&icon_info);
        return None;
    }

    let mut bm = BITMAP::default();
    if GetObjectW(
        hbm_color,
        std::mem::size_of::<BITMAP>() as i32,
        Some(&mut bm as *mut _ as *mut _),
    ) == 0
    {
        let _ = DeleteObject(hbm_color);
        release_mask(&icon_info);
        return None;
    }

    let width = bm.bmWidth;
    let height = bm.bmHeight;
    let row_size = ((width * 32 + 31) / 32) * 4;
    let image_size = (row_size * height) as usize;

    let hdc_screen = GetDC(None);
    let hdc_mem = CreateCompatibleDC(hdc_screen);
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [Default::default()],
    };

    let mut pixels = vec![0u8; image_size];
    let old_bitmap = SelectObject(hdc_mem, hbm_color);
    GetDIBits(
        hdc_mem,
        hbm_color,
        0,
        height as u32,
        Some(pixels.as_mut_ptr() as *mut _),
        &mut bmi,
        DIB_RGB_COLORS,
    );
    SelectObject(hdc_mem, old_bitmap);
    let _ = DeleteDC(hdc_mem);
    ReleaseDC(None, hdc_screen);
    let _ = DeleteObject(hbm_color);
    release_mask(&icon_info);

    // BGRA → RGBA, then PNG.
    for px in pixels.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let image = image::RgbaImage::from_raw(width as u32, height as u32, pixels)?;
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(format!("data:image/png;base64,{}", STANDARD.encode(&png)))
}

#[cfg(test)]
mod hardware_tests {
    use super::*;

    /// The mixer polls this every 100 ms, on both PCs. Lists what plays on
    /// this PC and how long a snapshot takes. `cargo test -- --ignored hardware`.
    #[test]
    #[ignore]
    fn hardware_mixer_snapshot_is_quick() {
        let started = std::time::Instant::now();
        let first = get_mixer_snapshot().unwrap();
        let cold = started.elapsed();
        let started = std::time::Instant::now();
        let runs = 10;
        for _ in 0..runs {
            get_mixer_snapshot().unwrap();
        }
        let warm = started.elapsed() / runs;
        for s in &first.sessions {
            println!("{:>6}  {:<28} output {:?}", s.pid, s.name, s.output);
        }
        println!(
            "per-app output: {} · first snapshot {cold:?}, then {warm:?}",
            first.per_app_output
        );
        assert!(first.sessions.iter().all(|s| s.pid != std::process::id()));
        assert!(warm < std::time::Duration::from_millis(50), "{warm:?}");
    }

    /// Channel volumes (follow the pointer) scale apps without touching the
    /// volume the dials show: set 95 %, read it back, restore every original.
    #[test]
    #[ignore]
    fn hardware_channel_levels_leave_the_dials_alone() {
        ensure_initialized();
        let sessions = render_sessions().unwrap();
        let mut checked = 0;
        for session in &sessions {
            unsafe {
                let (Ok(channels), Ok(simple)) = (
                    session.cast::<IChannelAudioVolume>(),
                    session.cast::<ISimpleAudioVolume>(),
                ) else {
                    continue;
                };
                let count = channels.GetChannelCount().unwrap_or(0);
                if count == 0 {
                    continue;
                }
                let before: Vec<f32> = (0..count)
                    .map(|c| channels.GetChannelVolume(c).unwrap_or(1.0))
                    .collect();
                let dial = simple.GetMasterVolume().unwrap();
                for c in 0..count {
                    channels
                        .SetChannelVolume(c, 0.95, std::ptr::null())
                        .unwrap();
                }
                let now = channels.GetChannelVolume(0).unwrap();
                let dial_after = simple.GetMasterVolume().unwrap();
                for (c, v) in before.iter().enumerate() {
                    let _ = channels.SetChannelVolume(c as u32, *v, std::ptr::null());
                }
                assert!((now - 0.95).abs() < 0.001);
                assert!((dial - dial_after).abs() < 0.001, "the dial moved");
                checked += 1;
            }
        }
        println!("channel levels checked on {checked} session(s), originals restored");
    }
}
