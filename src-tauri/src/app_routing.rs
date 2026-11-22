// Trovul - Output device per app, as in Windows Settings → Sound → "App volume
// and device preferences". Windows has no public API for it; this uses the
// same internal AudioPolicyConfig interface as Windows Settings (and tools
// such as EarTrumpet). Its interface id changed in build 21390, so both ids
// are tried; if neither answers, the feature reports itself unavailable and
// the UI hides the per-app output picker.
//
// Windows remembers the choice per app (by executable), not per process.

use crate::com;
use crate::util::{lock, mono_ms};
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::{LazyLock, Mutex, OnceLock};
use windows::core::{IInspectable, Interface, GUID, HRESULT, HSTRING};
use windows::Win32::System::WinRT::RoGetActivationFactory;

const IID_21H2: GUID = GUID::from_u128(0xab3d4648_e242_459f_b02f_541c70306324);
const IID_DOWNLEVEL: GUID = GUID::from_u128(0x2a59116d_6c4f_45e0_a74f_707e3fef9258);
/// Device interface path around an endpoint id: \\?\SWD#MMDEVAPI#<id>#{render class}.
const MMDEVAPI: &str = r"\\?\SWD#MMDEVAPI#";
const RENDER_CLASS: &str = "#{e6327cad-dcec-4949-ae8a-991e976a79d2}";
const E_RENDER: i32 = 0;
const E_CONSOLE: i32 = 0;
const E_MULTIMEDIA: i32 = 1;

#[repr(C)]
struct Vtbl {
    /// IUnknown (3) + IInspectable (3).
    base: [usize; 6],
    /// Volume groups, ringer, chat apps… (19 methods) — not used.
    unused: [usize; 19],
    set_persisted: unsafe extern "system" fn(*mut c_void, u32, i32, i32, *mut c_void) -> HRESULT,
    get_persisted: unsafe extern "system" fn(*mut c_void, u32, i32, i32, *mut HSTRING) -> HRESULT,
}

/// Owned pointer to the policy interface.
struct Policy(*mut c_void);

impl Policy {
    fn open() -> Result<Self, String> {
        com::ensure_initialized();
        let factory: IInspectable = unsafe {
            RoGetActivationFactory(&HSTRING::from("Windows.Media.Internal.AudioPolicyConfig"))
        }
        .map_err(|e| format!("Per-app output isn't available on this Windows ({e})"))?;
        for iid in [IID_21H2, IID_DOWNLEVEL] {
            let mut ptr = null_mut();
            if unsafe { factory.query(&iid, &mut ptr) }.is_ok() && !ptr.is_null() {
                return Ok(Self(ptr));
            }
        }
        Err("Per-app output isn't available on this Windows".into())
    }

    fn vtbl(&self) -> &Vtbl {
        unsafe { &**(self.0 as *const *const Vtbl) }
    }
}

impl Drop for Policy {
    fn drop(&mut self) {
        unsafe {
            let release: unsafe extern "system" fn(*mut c_void) -> u32 =
                std::mem::transmute(self.vtbl().base[2]);
            release(self.0);
        }
    }
}

/// Whether this Windows answers; checked once.
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| Policy::open().is_ok())
}

/// Endpoint id from the device interface path Windows stores.
fn endpoint_id(path: &str) -> Option<String> {
    let rest = path.strip_prefix(MMDEVAPI).unwrap_or(path);
    let id = rest.rfind('#').map_or(rest, |i| &rest[..i]);
    (!id.is_empty()).then(|| id.to_string())
}

fn read(policy: &Policy, pid: u32) -> Result<Option<String>, String> {
    let mut path = HSTRING::default();
    unsafe { (policy.vtbl().get_persisted)(policy.0, pid, E_RENDER, E_MULTIMEDIA, &mut path) }
        .ok()
        .map_err(|e| format!("Could not read the app's output: {e}"))?;
    Ok(endpoint_id(&path.to_string_lossy()))
}

/// The output device chosen for `pid`'s app; `None` = the Windows default.
#[cfg(test)]
fn app_output(pid: u32) -> Result<Option<String>, String> {
    read(&Policy::open()?, pid)
}

/// (read at, chosen output).
type Cached = (u64, Option<String>);
/// By PID. The mixer polls every 100 ms and a choice only changes when
/// someone sets it, here or in Windows Settings.
static CACHE: LazyLock<Mutex<HashMap<u32, Cached>>> = LazyLock::new(Default::default);
const CACHE_MS: u64 = 2000;

/// Chosen outputs of `pids` (processes with an audio session), or `None`
/// when this Windows can't route apps.
pub fn app_outputs(pids: &[u32]) -> Option<HashMap<u32, Option<String>>> {
    if !available() {
        return None;
    }
    let now = mono_ms();
    let mut cache = lock(&CACHE);
    cache.retain(|pid, (at, _)| pids.contains(pid) && now.saturating_sub(*at) < CACHE_MS);
    let missing: Vec<u32> = pids
        .iter()
        .copied()
        .filter(|pid| !cache.contains_key(pid))
        .collect();
    if !missing.is_empty() {
        let policy = Policy::open().ok()?;
        for pid in missing {
            // A process whose session just ended can't answer; it shows as default.
            cache.insert(pid, (now, read(&policy, pid).ok().flatten()));
        }
    }
    Some(
        pids.iter()
            .map(|pid| (*pid, cache.get(pid).and_then(|(_, out)| out.clone())))
            .collect(),
    )
}

/// Send `pid`'s app to `device_id` (an endpoint id), or back to the default.
pub fn set_app_output(pid: u32, device_id: Option<&str>) -> Result<(), String> {
    lock(&CACHE).remove(&pid);
    let policy = Policy::open()?;
    let path = device_id.map(|id| HSTRING::from(format!("{MMDEVAPI}{id}{RENDER_CLASS}")));
    let raw: *mut c_void = path.as_ref().map_or(null_mut(), |h| unsafe {
        std::mem::transmute_copy::<HSTRING, *mut c_void>(h)
    });
    for role in [E_MULTIMEDIA, E_CONSOLE] {
        unsafe { (policy.vtbl().set_persisted)(policy.0, pid, E_RENDER, role, raw) }
            .ok()
            .map_err(|e| format!("Could not change the app's output: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_paths_become_endpoint_ids() {
        let id = "{0.0.0.00000000}.{a3b5c7d9-1111-2222-3333-444455556666}";
        assert_eq!(
            endpoint_id(&format!("{MMDEVAPI}{id}{RENDER_CLASS}")).as_deref(),
            Some(id)
        );
        assert_eq!(endpoint_id(""), None);
    }

    /// Checks the undocumented interface on this PC without touching any real
    /// app: the test opens a silent output stream (so it has an audio session),
    /// routes itself to a device, reads it back and puts it back to default.
    /// `cargo test -- --ignored hardware`.
    #[test]
    #[ignore]
    fn hardware_routes_this_test_process() {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let device = host.default_output_device().expect("no output device");
        let config = device.default_output_config().unwrap().config();
        let stream = device
            .build_output_stream(
                &config,
                |data: &mut [f32], _| data.fill(0.0),
                |e| eprintln!("{e}"),
                None,
            )
            .unwrap();
        stream.play().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));

        let pid = std::process::id();
        let devices = crate::audio_sessions::get_output_devices().unwrap();
        let target = devices
            .iter()
            .find(|d| !d.is_default)
            .or(devices.first())
            .unwrap()
            .id
            .clone();
        assert_eq!(
            app_output(pid).unwrap(),
            None,
            "this test already had a preference"
        );
        let result = (|| -> Result<(), String> {
            set_app_output(pid, Some(&target))?;
            assert_eq!(app_output(pid)?.as_deref(), Some(target.as_str()));
            set_app_output(pid, None)?;
            assert_eq!(app_output(pid)?, None);
            Ok(())
        })();
        let _ = set_app_output(pid, None);
        drop(stream);
        result.unwrap();
        println!("per-app output works (routed this test to {target} and back)");
    }
}
