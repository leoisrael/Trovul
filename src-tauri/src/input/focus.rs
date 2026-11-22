// Trovul - Keyboard & mouse sharing: what Windows is doing around the pointer.
//
// - Game mode: is a game in front (then the pointer stays on this PC, so a
//   flick to the screen edge never throws it out of the game)?
// - Scroll Lock keeps the pointer on the PC it's on, as in Synergy/Barrier.
// - The keyboard follows the pointer: when it arrives on a screen, the window
//   under it gets the keyboard, unless the one being typed in is on that same
//   screen. Windows itself only moves the keyboard on a click.
// - Admin windows: this PC can't type into them for the other PC (UIPI).

use super::layout::Point;
use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONULL,
};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcess, GetCurrentThreadId, OpenProcess, OpenProcessToken,
    QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON, VK_SCROLL, VK_XBUTTON1,
    VK_XBUTTON2,
};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetClipCursor, GetCursorInfo, GetForegroundWindow,
    GetSystemMetrics, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, PeekMessageW,
    SetForegroundWindow, WindowFromPoint, CURSORINFO, CURSOR_SHOWING, GA_ROOT, GWL_EXSTYLE, MSG,
    PM_NOREMOVE, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

/// Full-screen apps that aren't games: browsers, video players, slideshows.
const NOT_GAMES: &[&str] = &[
    "chrome",
    "msedge",
    "firefox",
    "brave",
    "opera",
    "opera_gx",
    "vivaldi",
    "iexplore",
    "vlc",
    "mpc-hc",
    "mpc-hc64",
    "mpc-be",
    "mpc-be64",
    "mpv",
    "potplayer",
    "potplayermini",
    "potplayermini64",
    "wmplayer",
    "microsoft.media.player",
    "video.ui",
    "powerpnt",
    "explorer",
    "code",
    "discord",
    "ms-teams",
    "teams",
    "zoom",
    "obs64",
    "spotify",
    "applicationframehost",
];

/// Windows that are the desktop or the taskbar, never "the app in front".
const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
];

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

fn is_shell(hwnd: HWND) -> bool {
    let class = class_name(hwnd);
    SHELL_CLASSES.contains(&class.as_str())
}

/// Lower-case exe name without ".exe" of the window's process.
fn process_name(hwnd: HWND) -> Option<String> {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(process);
        if !ok {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        let file = path
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(&path)
            .to_lowercase();
        Some(file.strip_suffix(".exe").unwrap_or(&file).to_string())
    }
}

fn virtual_screen() -> RECT {
    unsafe {
        let (x, y) = (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
        );
        RECT {
            left: x,
            top: y,
            right: x + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: y + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

/// The window covers its whole monitor (full screen, not just maximized).
fn covers_its_monitor(hwnd: HWND) -> bool {
    unsafe {
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return false;
        }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if monitor.is_invalid() || !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }
        let m = info.rcMonitor;
        rect.left <= m.left && rect.top <= m.top && rect.right >= m.right && rect.bottom >= m.bottom
    }
}

/// A game (or something playing like one) is in front: the pointer should
/// stay on this PC.
pub fn game_in_front() -> bool {
    unsafe {
        let front = GetForegroundWindow();
        if front.is_invalid() || is_shell(front) {
            return false;
        }
        // The app confined the cursor, as games do while playing.
        let mut clip = RECT::default();
        if GetClipCursor(&mut clip).is_ok() {
            let all = virtual_screen();
            let confined = clip.left > all.left
                || clip.top > all.top
                || clip.right < all.right
                || clip.bottom < all.bottom;
            if confined {
                return true;
            }
        }
        // Exclusive full screen (Direct3D).
        if SHQueryUserNotificationState().is_ok_and(|s| s == QUNS_RUNNING_D3D_FULL_SCREEN) {
            return true;
        }
        if !covers_its_monitor(front) {
            return false;
        }
        // Full screen with the cursor hidden: aiming, not browsing.
        let mut cursor = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        if GetCursorInfo(&mut cursor).is_ok() && cursor.flags.0 & CURSOR_SHOWING.0 == 0 {
            return true;
        }
        // Full screen with the cursor showing: a game, unless it's a browser,
        // a video player or a slideshow.
        process_name(front).is_some_and(|name| !NOT_GAMES.contains(&name.as_str()))
    }
}

/// A game in exclusive full screen or a presentation: nothing may pop up
/// over it (a topmost window can minimize an exclusive full-screen game).
pub fn exclusive_full_screen() -> bool {
    unsafe { SHQueryUserNotificationState() }
        .is_ok_and(|s| s == QUNS_RUNNING_D3D_FULL_SCREEN || s == QUNS_PRESENTATION_MODE)
}

/// Scroll Lock is on: the pointer stays on the PC it's on.
pub fn scroll_locked() -> bool {
    unsafe { GetKeyState(VK_SCROLL.0 as i32) & 1 != 0 }
}

pub fn key_down_now(vk: u32) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

/// Any mouse button physically down right now.
pub fn any_button_down() -> bool {
    [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON, VK_XBUTTON1, VK_XBUTTON2]
        .iter()
        .any(|vk| key_down_now(vk.0 as u32))
}

/// Give the keyboard to the window under `p` (native pixels, this PC), unless
/// the window being typed in is on the same screen. Runs on its own short
/// thread: it attaches to the foreground window's input, and a hung app must
/// never stall the input hooks.
pub fn keyboard_to_window_at(p: Point) {
    let _ = std::thread::Builder::new()
        .name("trovul-focus".into())
        .spawn(move || unsafe { focus_at(p) });
}

unsafe fn focus_at(p: Point) {
    let point = POINT { x: p.x, y: p.y };
    let hit = WindowFromPoint(point);
    if hit.is_invalid() {
        return;
    }
    let target = GetAncestor(hit, GA_ROOT);
    let front = GetForegroundWindow();
    if target.is_invalid() || target == front || is_shell(target) {
        return;
    }
    let ex_style = GetWindowLongW(target, GWL_EXSTYLE) as u32;
    if ex_style & (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) != 0 {
        return; // overlays, tooltips, our own parking window
    }
    // Already typing on this screen: keep that window.
    if !front.is_invalid()
        && MonitorFromWindow(front, MONITOR_DEFAULTTONULL)
            == MonitorFromPoint(point, MONITOR_DEFAULTTONULL)
    {
        return;
    }
    // Windows lets a thread that shares the foreground window's input move
    // the focus; this thread needs a message queue for that.
    let mut msg = MSG::default();
    let _ = PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_NOREMOVE);
    let me = GetCurrentThreadId();
    let front_thread = if front.is_invalid() {
        0
    } else {
        GetWindowThreadProcessId(front, None)
    };
    let attached = front_thread != 0
        && front_thread != me
        && AttachThreadInput(me, front_thread, true).as_bool();
    let _ = SetForegroundWindow(target);
    if attached {
        let _ = AttachThreadInput(me, front_thread, false);
    }
}

// ---------- Admin windows ----------

fn elevated(process: HANDLE) -> Option<bool> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let result = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut TOKEN_ELEVATION as *mut c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        let _ = CloseHandle(token);
        result.ok()?;
        Some(elevation.TokenIsElevated != 0)
    }
}

/// The foreground window last checked: a key press in the same window costs nothing.
static CHECKED: AtomicIsize = AtomicIsize::new(0);
/// Admin windows already reported, so each is reported once.
static WARNED: Mutex<Vec<isize>> = Mutex::new(Vec::new());
static SELF_ELEVATED: OnceLock<Option<bool>> = OnceLock::new();

/// The foreground window runs as administrator and this app doesn't: Windows
/// drops what we type or click into it (UIPI). Returns its title once per window.
pub fn blocked_admin_window() -> Option<String> {
    unsafe {
        let front = GetForegroundWindow();
        let id = front.0 as isize;
        if front.is_invalid() || CHECKED.swap(id, Ordering::Relaxed) == id {
            return None;
        }
        if *SELF_ELEVATED.get_or_init(|| elevated(GetCurrentProcess())) != Some(false) {
            return None; // this app is admin too: nothing is blocked
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(front, Some(&mut pid));
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let admin = elevated(process);
        let _ = CloseHandle(process);
        if admin != Some(true) {
            return None;
        }
        {
            let mut warned = crate::util::lock(&WARNED);
            if warned.contains(&id) {
                return None;
            }
            if warned.len() >= 32 {
                warned.remove(0);
            }
            warned.push(id);
        }
        let mut title = [0u16; 256];
        let len = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(front, &mut title);
        let title = String::from_utf16_lossy(&title[..len.max(0) as usize]);
        Some(if title.trim().is_empty() {
            process_name(front).unwrap_or_else(|| "an admin window".into())
        } else {
            title
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prints what this PC's desktop looks like to game mode right now.
    /// `cargo test -- --ignored hardware`.
    #[test]
    #[ignore]
    fn hardware_reads_the_foreground() {
        let front = unsafe { GetForegroundWindow() };
        println!(
            "front: {:?} ({}) · full screen {} · game {} · scroll lock {} · button down {}",
            process_name(front),
            class_name(front),
            covers_its_monitor(front),
            game_in_front(),
            scroll_locked(),
            any_button_down()
        );
        let _ = blocked_admin_window();
    }
}

#[cfg(test)]
mod scroll_probe {
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    };

    fn tap_scroll_lock() {
        let key = |up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_SCROLL,
                    wScan: 0,
                    dwFlags: if up {
                        KEYEVENTF_KEYUP
                    } else {
                        KEYBD_EVENT_FLAGS(0)
                    },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe {
            SendInput(
                &[key(false), key(true)],
                std::mem::size_of::<INPUT>() as i32,
            );
        }
    }

    #[test]
    #[ignore]
    fn hardware_scroll_lock_from_a_background_thread() {
        let before = std::thread::spawn(scroll_locked).join().unwrap();
        // A thread with a message queue that never gets keyboard messages, like the hook's.
        let waiter = std::thread::spawn(|| unsafe {
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_NOREMOVE);
            let first = scroll_locked();
            std::thread::sleep(std::time::Duration::from_millis(200));
            (first, scroll_locked())
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        tap_scroll_lock();
        let (first, after) = waiter.join().unwrap();
        let fresh = std::thread::spawn(scroll_locked).join().unwrap();
        tap_scroll_lock(); // put it back
        println!(
            "scroll lock: before {before} · queue thread {first} → {after} · fresh thread {fresh}"
        );
        assert_eq!(first, before);
        assert_ne!(after, before, "a thread with a queue sees the toggle");
    }
}

#[cfg(test)]
mod async_state_probe {
    use super::*;
    use std::sync::atomic::{AtomicI32, Ordering};
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY, VK_F23, VK_F24,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
        UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, LLKHF_UP, WH_KEYBOARD_LL, WM_QUIT,
    };

    /// GetAsyncKeyState(F24) seen from inside the hook when F23 goes down.
    static SEEN: AtomicI32 = AtomicI32::new(-1);

    unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let info = *(lparam.0 as *const KBDLLHOOKSTRUCT);
        if code >= 0 && info.vkCode == VK_F23.0 as u32 && info.flags.0 & LLKHF_UP.0 == 0 {
            SEEN.store(key_down_now(VK_F24.0 as u32) as i32, Ordering::SeqCst);
        }
        CallNextHookEx(HHOOK::default(), code, wparam, lparam)
    }

    fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if up {
                        KEYEVENTF_KEYUP
                    } else {
                        KEYBD_EVENT_FLAGS(0)
                    },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    /// Is a key that's down visible to GetAsyncKeyState from a low-level
    /// hook's thread? F23/F24 are keys no app uses. `cargo test -- --ignored hardware`.
    #[test]
    #[ignore]
    fn hardware_async_state_inside_a_low_level_hook() {
        let (tx, rx) = std::sync::mpsc::channel();
        let hook_thread = std::thread::spawn(move || unsafe {
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_NOREMOVE);
            let handle = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), None, 0).unwrap();
            tx.send(GetCurrentThreadId()).unwrap();
            while GetMessageW(&mut msg, HWND::default(), 0, 0).0 > 0 {
                DispatchMessageW(&msg);
            }
            let _ = UnhookWindowsHookEx(handle);
        });
        let thread = rx.recv().unwrap();
        let size = std::mem::size_of::<INPUT>() as i32;
        unsafe {
            SendInput(&[key(VK_F24, false)], size);
            std::thread::sleep(std::time::Duration::from_millis(50));
            SendInput(&[key(VK_F23, false), key(VK_F23, true)], size);
            std::thread::sleep(std::time::Duration::from_millis(50));
            SendInput(&[key(VK_F24, true)], size);
            std::thread::sleep(std::time::Duration::from_millis(50));
            let _ = PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        hook_thread.join().unwrap();
        let seen = SEEN.load(Ordering::SeqCst);
        println!("F24 held, seen from the hook: {seen} (1 = down, 0 = up, -1 = hook never ran)");
        assert_eq!(seen, 1);
    }
}
