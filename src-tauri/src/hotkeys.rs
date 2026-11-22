// Trovul - Global keyboard shortcuts, registered with RegisterHotKey on a
// dedicated thread that runs a Win32 message loop.
//
//   Ctrl+Alt+M   mute / unmute the audio coming from the other PC
//   Ctrl+Alt+T   audio between the PCs on / off (restores the last direction)

use crate::util::lock;
use serde::Serialize;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, PeekMessageW, PostThreadMessageW, MSG, PM_NOREMOVE, WM_HOTKEY, WM_QUIT,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    ToggleMute,
    ToggleAudio,
}

/// (action, label, virtual key) — all use Ctrl+Alt.
const SHORTCUTS: [(Action, &str, u8); 2] = [
    (Action::ToggleMute, "Ctrl+Alt+M", b'M'),
    (Action::ToggleAudio, "Ctrl+Alt+T", b'T'),
];

#[derive(Debug, Clone, Serialize)]
pub struct ShortcutStatus {
    pub action: Action,
    pub keys: &'static str,
    /// False when another app already owns the combination.
    pub registered: bool,
}

static THREAD_ID: AtomicU32 = AtomicU32::new(0);
static STATUS: Mutex<Vec<ShortcutStatus>> = Mutex::new(Vec::new());

/// Register or unregister the shortcuts to match the setting.
pub fn apply(enabled: bool, on_action: fn(Action)) {
    if enabled {
        start(on_action);
    } else {
        stop();
    }
}

pub fn status() -> Vec<ShortcutStatus> {
    lock(&STATUS).clone()
}

fn start(on_action: fn(Action)) {
    if THREAD_ID.load(Ordering::Acquire) != 0 {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("trovul-hotkeys".into())
        .spawn(move || unsafe {
            let mut msg = MSG::default();
            // Create this thread's message queue before publishing its id, so a
            // WM_QUIT posted by `stop()` can never get lost.
            let _ = PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_NOREMOVE);
            THREAD_ID.store(GetCurrentThreadId(), Ordering::Release);

            let mut status = Vec::with_capacity(SHORTCUTS.len());
            for (index, (action, keys, key)) in SHORTCUTS.iter().enumerate() {
                let registered = RegisterHotKey(
                    HWND::default(),
                    index as i32 + 1,
                    MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                    *key as u32,
                )
                .is_ok();
                if registered {
                    log::info!("Shortcut {keys} registered");
                } else {
                    log::warn!("Shortcut {keys} is already used by another app");
                }
                status.push(ShortcutStatus {
                    action: *action,
                    keys,
                    registered,
                });
            }
            *lock(&STATUS) = status;

            // GetMessageW returns 0 for WM_QUIT and -1 on error.
            while GetMessageW(&mut msg, HWND::default(), 0, 0).0 > 0 {
                if msg.message == WM_HOTKEY {
                    if let Some((action, _, _)) = SHORTCUTS.get(msg.wParam.0.wrapping_sub(1)) {
                        on_action(*action);
                    }
                }
            }

            for index in 0..SHORTCUTS.len() {
                let _ = UnregisterHotKey(HWND::default(), index as i32 + 1);
            }
            lock(&STATUS).clear();
            THREAD_ID.store(0, Ordering::Release);
            log::info!("Shortcuts unregistered");
        });
    if let Err(e) = spawned {
        log::error!("Could not start the shortcut thread: {e}");
    }
}

fn stop() {
    let thread = THREAD_ID.load(Ordering::Acquire);
    if thread != 0 {
        unsafe {
            let _ = PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_for(condition: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// Registers the real global shortcuts for a moment, then releases them.
    /// Run with `cargo test -- --ignored hardware`.
    #[test]
    #[ignore]
    fn hardware_shortcuts_register_and_release() {
        apply(true, |_| {});
        assert!(
            wait_for(|| status().len() == SHORTCUTS.len()),
            "shortcut thread did not start"
        );
        for shortcut in status() {
            // Another app may own a combination; the status must say so either way.
            println!("{} registered: {}", shortcut.keys, shortcut.registered);
        }
        apply(false, |_| {});
        assert!(
            wait_for(|| THREAD_ID.load(Ordering::Acquire) == 0),
            "shortcut thread did not stop"
        );
        assert!(status().is_empty());
    }
}
