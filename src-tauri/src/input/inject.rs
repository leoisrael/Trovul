// Trovul - The controlled side: replays the other PC's mouse and keyboard with
// SendInput. Everything pressed is remembered, so control ending for any
// reason (leave, link drop, local mouse moved) never leaves a key stuck down.

use super::monitors::physical_pixels;
use super::{InputEvent, MouseButton};
use crate::util::lock;
use std::collections::HashSet;
use std::sync::Mutex;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
    MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

#[derive(Default)]
struct Held {
    keys: HashSet<(u16, u16, bool)>,
    buttons: HashSet<MouseButton>,
}

static HELD: Mutex<Option<Held>> = Mutex::new(None);

/// Replay one event from the controlling PC.
pub fn apply(event: &InputEvent) {
    match *event {
        InputEvent::Enter { x, y } | InputEvent::Move { x, y } => move_to(x, y),
        InputEvent::Button { button, down } => {
            track(|h| {
                if down {
                    h.buttons.insert(button);
                } else {
                    h.buttons.remove(&button);
                }
            });
            send_button(button, down);
        }
        InputEvent::Wheel { delta, horizontal } => {
            let flag = if horizontal {
                MOUSEEVENTF_HWHEEL
            } else {
                MOUSEEVENTF_WHEEL
            };
            send_mouse(0, 0, delta as u32, flag);
        }
        InputEvent::Key {
            vk,
            scan,
            extended,
            down,
        } => {
            track(|h| {
                if down {
                    h.keys.insert((vk, scan, extended));
                } else {
                    h.keys.remove(&(vk, scan, extended));
                }
            });
            send_key(vk, scan, extended, down);
        }
        _ => {}
    }
}

/// Let go of keys on this PC (vk, scan, extended): the pointer went to the
/// other PC, which gets their key-ups instead. Letting go of Win or Alt on
/// its own would open Start or a menu bar, so an unassigned key is tapped
/// first, as AutoHotkey does.
pub fn let_go(keys: &[(u16, u16, bool)]) {
    const MASK_KEY: u16 = 0xE8; // unassigned
    let win_or_alt = |vk: u16| matches!(vk, 0x5B | 0x5C | 0x12 | 0xA4 | 0xA5);
    if keys.iter().any(|&(vk, ..)| win_or_alt(vk)) {
        send_key(MASK_KEY, 0, false, true);
        send_key(MASK_KEY, 0, false, false);
    }
    for &(vk, scan, extended) in keys {
        send_key(vk, scan, extended, false);
    }
}

/// Let go of every key and button the other PC is holding down.
pub fn release_all() {
    let held = lock(&HELD).take().unwrap_or_default();
    for (vk, scan, extended) in held.keys {
        send_key(vk, scan, extended, false);
    }
    for button in held.buttons {
        send_button(button, false);
    }
}

fn track(change: impl FnOnce(&mut Held)) {
    change(lock(&HELD).get_or_insert_with(Held::default));
}

fn move_to(x: i32, y: i32) {
    physical_pixels();
    let (vx, vy, vw, vh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
        )
    };
    // Absolute coordinates are 0..65535 across the whole virtual desktop.
    let norm = |v: i32, origin: i32, size: i32| {
        (((v - origin) as i64 * 65536 + size as i64 - 1) / size as i64).clamp(0, 65535) as i32
    };
    send_mouse(
        norm(x, vx, vw),
        norm(y, vy, vh),
        0,
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
    );
}

fn send_button(button: MouseButton, down: bool) {
    let (flag, data) = match (button, down) {
        (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (MouseButton::X1, true) => (MOUSEEVENTF_XDOWN, 1),
        (MouseButton::X1, false) => (MOUSEEVENTF_XUP, 1),
        (MouseButton::X2, true) => (MOUSEEVENTF_XDOWN, 2),
        (MouseButton::X2, false) => (MOUSEEVENTF_XUP, 2),
    };
    send_mouse(0, 0, data, flag);
}

fn send_mouse(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

fn send_key(vk: u16, scan: u16, extended: bool, down: bool) {
    // Scan codes: the physical key, so each PC applies its own keyboard layout.
    let mut flags = if scan != 0 {
        KEYEVENTF_SCANCODE
    } else {
        KEYBD_EVENT_FLAGS(0)
    };
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(if scan != 0 { 0 } else { vk }),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, MapVirtualKeyW, MAPVK_VK_TO_VSC, VK_F24,
    };

    /// A key held on this PC is let go of by scan code, as the hook records
    /// it. F24 is a key no app uses. `cargo test -- --ignored hardware`.
    #[test]
    #[ignore]
    fn hardware_let_go_releases_a_held_key() {
        let vk = VK_F24.0;
        let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
        let down = || unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 };
        send_key(vk, scan, false, true);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(down(), "F24 should be down");
        let_go(&[(vk, scan, false)]);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!down(), "F24 should be up after let_go");
        println!("let_go released F24 (scan {scan:#x})");
    }
}
