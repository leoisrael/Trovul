// Trovul - This PC's monitors in physical pixels, with Windows' display number
// and the monitor's friendly name (e.g. "LG ULTRAGEAR").

use super::layout::{Monitor, Rect};
use std::collections::HashMap;
use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{
    SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

/// Make the calling thread see physical pixels (the app is per-monitor DPI
/// aware, but threads we don't own may not have inherited it).
pub fn physical_pixels() {
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn enumerate() -> Vec<Monitor> {
    physical_pixels();
    let names = friendly_names();
    let mut found: Vec<(String, RECT, bool)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(collect),
            LPARAM(&mut found as *mut _ as isize),
        );
    }
    let mut monitors: Vec<Monitor> = found
        .into_iter()
        .map(|(gdi, rect, primary)| {
            let number = gdi.trim_start_matches(r"\\.\DISPLAY").parse().unwrap_or(0);
            let (name, path) = names.get(&gdi).cloned().unwrap_or_default();
            Monitor {
                // The device path survives reboots; the GDI name can change.
                id: if path.is_empty() { gdi } else { path },
                number,
                name: if name.is_empty() {
                    format!("Display {number}")
                } else {
                    name
                },
                rect: Rect::new(
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                ),
                primary,
            }
        })
        .collect();
    monitors.sort_by_key(|m| m.number);
    monitors
}

unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let found = &mut *(data.0 as *mut Vec<(String, RECT, bool)>);
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(monitor, &mut info.monitorInfo).as_bool() {
        found.push((
            wide_to_string(&info.szDevice),
            info.monitorInfo.rcMonitor,
            info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        ));
    }
    true.into()
}

/// GDI device name ("\\.\DISPLAY1") → (friendly name, device path).
fn friendly_names() -> HashMap<String, (String, String)> {
    let mut out = HashMap::new();
    unsafe {
        let (mut path_count, mut mode_count) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count).0
            != 0
        {
            return out;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            None,
        )
        .0 != 0
        {
            return out;
        }
        for path in &paths[..path_count as usize] {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            source.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            source.header.adapterId = path.sourceInfo.adapterId;
            source.header.id = path.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0 {
                continue;
            }
            let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            target.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            target.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            target.header.adapterId = path.targetInfo.adapterId;
            target.header.id = path.targetInfo.id;
            if DisplayConfigGetDeviceInfo(&mut target.header) != 0 {
                continue;
            }
            out.insert(
                wide_to_string(&source.viewGdiDeviceName),
                (
                    wide_to_string(&target.monitorFriendlyDeviceName),
                    wide_to_string(&target.monitorDevicePath),
                ),
            );
        }
    }
    out
}

fn wide_to_string(wide: &[u16]) -> String {
    let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}

#[cfg(test)]
mod tests {
    /// Lists this PC's monitors (read-only): `cargo test -- --ignored hardware`.
    #[test]
    #[ignore]
    fn hardware_lists_monitors() {
        let monitors = super::enumerate();
        for m in &monitors {
            println!(
                "{} #{} {:?} primary={} id={}",
                m.name, m.number, m.rect, m.primary, m.id
            );
        }
        assert!(!monitors.is_empty());
        assert_eq!(monitors.iter().filter(|m| m.primary).count(), 1);
    }
}
