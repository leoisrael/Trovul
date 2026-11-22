// Trovul - Low-level mouse and keyboard hooks (the controlling side).
//
// Runs on its own thread with a Win32 message loop. While this PC has the
// pointer, moves are checked against the shared arrangement (layout.rs) at
// every monitor edge. When the pointer goes to the other PC, the local cursor
// is parked under an invisible full-screen window, every mouse and key event is
// swallowed here and forwarded, and the pointer's position on the other PC is
// tracked from the deltas until it comes back.
//
// Safety nets, because a hook that swallows input can lock the user out:
// - panics inside a hook let the event through;
// - the link dropping, a send failing, or Ctrl+Alt+Esc bring the pointer home;
// - if the overlay ever receives mouse input, the hook is gone (Windows drops
//   hooks that stall) and the overlay gets out of the way.

use super::layout::{local_move, remote_step, LocalMove, Point, RemoteStep, World};
use super::monitors::physical_pixels;
use super::{InputEvent, MouseButton, Outgoing};
use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_SCROLL;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_CONTROL, VK_ESCAPE, VK_LCONTROL, VK_LMENU, VK_MENU, VK_RCONTROL, VK_RMENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
    GetMessageW, GetSystemMetrics, PeekMessageW, PostThreadMessageW, RegisterClassW, SetCursor,
    SetCursorPos, SetLayeredWindowAttributes, SetWindowPos, SetWindowsHookExW, ShowWindow,
    UnhookWindowsHookEx, HCURSOR, HC_ACTION, HHOOK, HWND_TOPMOST, KBDLLHOOKSTRUCT, LLKHF_EXTENDED,
    LLKHF_INJECTED, LLKHF_UP, LLMHF_INJECTED, LWA_ALPHA, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, SW_HIDE, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_DISPLAYCHANGE, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

/// Bring the pointer home. wParam 1 = also tell the other PC (Leave).
const WM_APP_RELEASE: u32 = WM_APP + 1;
/// Let go, on this PC, of the keys it saw go down (after the pointer left).
const WM_APP_LET_GO: u32 = WM_APP + 2;

static THREAD_ID: AtomicU32 = AtomicU32::new(0);
/// Hooks wanted. Set before the thread exists, so two quick starts can't both
/// spawn one, and a stop that comes before the thread is ready still counts.
static RUNNING: AtomicBool = AtomicBool::new(false);
/// The pointer is on the other PC (read by other threads for status).
static CONTROLLING: AtomicBool = AtomicBool::new(false);

pub fn controlling() -> bool {
    CONTROLLING.load(Ordering::Acquire)
}

struct HeldKey {
    scan: u16,
    extended: bool,
    /// Its key-down went to this PC's apps (not to the other PC).
    here: bool,
}

enum Mode {
    Local,
    Remote {
        world: Arc<World>,
        screen: usize,
        /// Pointer position on the other PC, in world coordinates.
        at: Point,
    },
}

struct State {
    mode: Mode,
    tx: Sender<Outgoing>,
    overlay: HWND,
    /// Physical mouse buttons held on this PC.
    buttons: u8,
    /// Buttons held on the other PC (forwarded): a drag there stays there.
    remote_buttons: u8,
    /// Physical keys held, by virtual-key code.
    keys: HashMap<u32, HeldKey>,
    /// Keys to let go of on this PC once the hook has returned.
    let_go: Vec<(u16, u16, bool)>,
    ctrl: bool,
    alt: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Start the hook thread (idempotent). Events for the other PC go to `tx`.
pub fn start(tx: Sender<Outgoing>) {
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("trovul-input".into())
        .spawn(move || unsafe {
            physical_pixels();
            let mut msg = MSG::default();
            // Create the message queue before publishing the thread id.
            let _ = PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_NOREMOVE);
            let instance: HINSTANCE = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
            let overlay = create_overlay(instance);
            STATE.with(|s| {
                *s.borrow_mut() = Some(State {
                    mode: Mode::Local,
                    tx,
                    overlay,
                    buttons: 0,
                    remote_buttons: 0,
                    keys: HashMap::new(),
                    let_go: Vec::new(),
                    ctrl: false,
                    alt: false,
                })
            });
            let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), instance, 0);
            let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), instance, 0);
            match (&mouse, &keyboard) {
                (Ok(_), Ok(_)) => log::info!("Keyboard & mouse sharing: hooks installed"),
                _ => log::error!("Keyboard & mouse sharing: could not install the input hooks"),
            }
            let me = GetCurrentThreadId();
            THREAD_ID.store(me, Ordering::Release);

            // A stop() that came before the id was published couldn't post
            // WM_QUIT, so check the flag too.
            while RUNNING.load(Ordering::Acquire)
                && GetMessageW(&mut msg, HWND::default(), 0, 0).0 > 0
            {
                if msg.message == WM_APP_RELEASE {
                    with_state(|s| s.go_home(msg.wParam.0 == 1));
                } else if msg.message == WM_APP_LET_GO {
                    let keys = with_state(|s| std::mem::take(&mut s.let_go));
                    super::inject::let_go(&keys);
                }
                DispatchMessageW(&msg);
            }

            with_state(|s| s.go_home(true));
            if let Ok(hook) = mouse {
                let _ = UnhookWindowsHookEx(hook);
            }
            if let Ok(hook) = keyboard {
                let _ = UnhookWindowsHookEx(hook);
            }
            if !overlay.is_invalid() {
                let _ = DestroyWindow(overlay);
            }
            STATE.with(|s| *s.borrow_mut() = None);
            // A newer input thread may already have taken over.
            let _ = THREAD_ID.compare_exchange(me, 0, Ordering::AcqRel, Ordering::Acquire);
            log::info!("Keyboard & mouse sharing: hooks removed");
        });
    if let Err(e) = spawned {
        log::error!("Could not start the input thread: {e}");
        RUNNING.store(false, Ordering::Release);
    }
}

pub fn stop() {
    if RUNNING.swap(false, Ordering::AcqRel) {
        post(WM_QUIT, 0);
    }
}

/// Bring the pointer back to this PC; `tell_peer` sends Leave.
pub fn release(tell_peer: bool) {
    post(WM_APP_RELEASE, tell_peer as usize);
}

fn post(message: u32, wparam: usize) {
    let thread = THREAD_ID.load(Ordering::Acquire);
    if thread != 0 {
        unsafe {
            let _ = PostThreadMessageW(thread, message, WPARAM(wparam), LPARAM(0));
        }
    }
}

/// Runs `f` on the hook thread's state. Re-entrant calls (Windows can call the
/// overlay's window procedure while a hook is running) get the default
/// instead of a panic across the FFI boundary.
fn with_state<R: Default>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| match s.try_borrow_mut() {
        Ok(mut state) => state.as_mut().map(f).unwrap_or_default(),
        Err(_) => R::default(),
    })
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = *(lparam.0 as *const MSLLHOOKSTRUCT);
        let swallow = catch_unwind(AssertUnwindSafe(|| {
            with_state(|s| s.on_mouse(wparam.0 as u32, &info))
        }));
        if matches!(swallow, Ok(true)) {
            return LRESULT(1);
        }
    }
    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = *(lparam.0 as *const KBDLLHOOKSTRUCT);
        let swallow = catch_unwind(AssertUnwindSafe(|| with_state(|s| s.on_key(&info))));
        if matches!(swallow, Ok(true)) {
            return LRESULT(1);
        }
    }
    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

fn cursor() -> Point {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    Point::new(p.x, p.y)
}

fn warp(p: Point) {
    unsafe {
        let _ = SetCursorPos(p.x, p.y);
    }
}

impl State {
    fn send(&self, event: InputEvent) {
        let _ = self.tx.send(Outgoing::Event(event));
    }

    fn status_changed(&self) {
        let _ = self.tx.send(Outgoing::StatusChanged);
    }

    /// Returns true to swallow the event.
    fn on_mouse(&mut self, message: u32, info: &MSLLHOOKSTRUCT) -> bool {
        if info.flags & LLMHF_INJECTED != 0 {
            // Our own replay of the other PC's input, or another tool's.
            return false;
        }
        if super::controlled() {
            // Someone is using this PC's own mouse: take the pointer back.
            super::local_input_while_controlled();
        }
        if matches!(self.mode, Mode::Local) {
            super::set_pointer_here(true);
        }
        let button = |b: MouseButton| -> u8 {
            match b {
                MouseButton::Left => 1,
                MouseButton::Right => 2,
                MouseButton::Middle => 4,
                MouseButton::X1 => 8,
                MouseButton::X2 => 16,
            }
        };
        let xbutton = || {
            if (info.mouseData >> 16) & 0xffff == 1 {
                MouseButton::X1
            } else {
                MouseButton::X2
            }
        };
        let pressed = match message {
            WM_LBUTTONDOWN => Some((MouseButton::Left, true)),
            WM_LBUTTONUP => Some((MouseButton::Left, false)),
            WM_RBUTTONDOWN => Some((MouseButton::Right, true)),
            WM_RBUTTONUP => Some((MouseButton::Right, false)),
            WM_MBUTTONDOWN => Some((MouseButton::Middle, true)),
            WM_MBUTTONUP => Some((MouseButton::Middle, false)),
            WM_XBUTTONDOWN => Some((xbutton(), true)),
            WM_XBUTTONUP => Some((xbutton(), false)),
            _ => None,
        };

        match &self.mode {
            Mode::Local => {
                if let Some((b, down)) = pressed {
                    if down {
                        self.buttons |= button(b);
                    } else {
                        self.buttons &= !button(b);
                    }
                    return false;
                }
                if message != WM_MOUSEMOVE {
                    return false;
                }
                let Some(world) = super::world() else {
                    return false;
                };
                if !super::remote_ready() {
                    return false;
                }
                if self.buttons != 0 && !super::any_button_down() {
                    // A button-up we never saw (lock screen, UAC, a hook
                    // timeout) must not leave every move treated as a drag.
                    self.buttons = 0;
                }
                let pt = Point::new(info.pt.x, info.pt.y);
                let before = cursor();
                match local_move(&world, before, pt, super::rules(self.buttons != 0)) {
                    LocalMove::Pass => false,
                    LocalMove::Warp(target) => {
                        warp(target);
                        true
                    }
                    LocalMove::Enter { .. } if super::pointer_stays() => {
                        // Game mode or Scroll Lock: stay on this screen.
                        if let Some(i) = world.local_at(before) {
                            warp(world.screens[i].native().clamp(pt));
                        }
                        true
                    }
                    LocalMove::Enter { screen, at } => {
                        self.enter(world, screen, at);
                        true
                    }
                }
            }
            Mode::Remote { .. } => {
                if let Some((b, down)) = pressed {
                    // A button pressed here before the pointer left is released here.
                    if !down && self.buttons & button(b) != 0 {
                        self.buttons &= !button(b);
                        return false;
                    }
                    if down {
                        self.remote_buttons |= button(b);
                    } else {
                        self.remote_buttons &= !button(b);
                    }
                    self.send(InputEvent::Button { button: b, down });
                    return true;
                }
                match message {
                    WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                        self.send(InputEvent::Wheel {
                            delta: ((info.mouseData >> 16) & 0xffff) as u16 as i16 as i32,
                            horizontal: message == WM_MOUSEHWHEEL,
                        });
                        true
                    }
                    WM_MOUSEMOVE => {
                        self.remote_move(Point::new(info.pt.x, info.pt.y));
                        true
                    }
                    _ => true,
                }
            }
        }
    }

    fn remote_move(&mut self, pt: Point) {
        let Mode::Remote { world, screen, at } = &self.mode else {
            return;
        };
        let (world, screen, at) = (world.clone(), *screen, *at);
        // The layout changed under us: come home rather than guess.
        if !super::world().is_some_and(|w| Arc::ptr_eq(&w, &world)) {
            self.go_home(true);
            return;
        }
        let park = cursor();
        let delta = Point::new(pt.x - park.x, pt.y - park.y);
        if delta == Point::default() {
            return;
        }
        let rules = super::rules(self.remote_buttons != 0);
        match remote_step(&world, screen, at, delta, rules) {
            RemoteStep::Move { screen, at } => {
                let native = world.to_native(screen, at);
                self.send(InputEvent::Move {
                    x: native.x,
                    y: native.y,
                });
                self.mode = Mode::Remote { world, screen, at };
            }
            // Scroll Lock keeps the pointer on the other PC too.
            RemoteStep::Leave { .. } if super::scroll_locked() => {}
            RemoteStep::Leave { screen, at } => {
                let target = world.to_native(screen, at);
                self.come_back(Some(target), true);
            }
        }
    }

    fn enter(&mut self, world: Arc<World>, screen: usize, at: Point) {
        let here = cursor();
        let park = world
            .local_at(here)
            .map(|i| world.screens[i].native().center())
            .unwrap_or(here);
        // From here on every key goes to the other PC, the first press
        // included. Keys this PC's apps saw go down are let go of here (their
        // key-ups now go to the other PC); a key whose key-up we never saw
        // just gets a harmless extra key-up.
        self.let_go = self
            .keys
            .drain()
            .filter(|(_, k)| k.here)
            .map(|(vk, k)| (vk as u16, k.scan, k.extended))
            .collect();
        if !self.let_go.is_empty() {
            post(WM_APP_LET_GO, 0);
        }
        self.remote_buttons = 0;
        show_overlay(self.overlay);
        warp(park);
        let native = world.to_native(screen, at);
        self.send(InputEvent::Enter {
            x: native.x,
            y: native.y,
        });
        self.mode = Mode::Remote { world, screen, at };
        super::set_pointer_here(false);
        CONTROLLING.store(true, Ordering::Release);
        self.status_changed();
    }

    /// Leave the other PC; `target` = where the cursor reappears here.
    fn come_back(&mut self, target: Option<Point>, tell_peer: bool) {
        if !matches!(self.mode, Mode::Remote { .. }) {
            return;
        }
        self.mode = Mode::Local;
        self.remote_buttons = 0;
        super::set_pointer_here(true);
        hide_overlay(self.overlay);
        if let Some(p) = target {
            warp(p);
            super::pointer_arrived(p);
        }
        if tell_peer {
            self.send(InputEvent::Leave);
        }
        CONTROLLING.store(false, Ordering::Release);
        self.status_changed();
    }

    /// Emergency / external return: the cursor stays where it was parked.
    fn go_home(&mut self, tell_peer: bool) {
        self.come_back(None, tell_peer);
    }

    fn on_key(&mut self, info: &KBDLLHOOKSTRUCT) -> bool {
        if info.flags.0 & LLKHF_INJECTED.0 != 0 {
            return false;
        }
        let vk = info.vkCode;
        let down = info.flags.0 & LLKHF_UP.0 == 0;
        let remote = matches!(self.mode, Mode::Remote { .. });
        if down {
            self.keys.insert(
                vk,
                HeldKey {
                    scan: info.scanCode as u16,
                    extended: info.flags.0 & LLKHF_EXTENDED.0 != 0,
                    here: !remote || vk == VK_SCROLL.0 as u32,
                },
            );
        } else {
            self.keys.remove(&vk);
        }
        let is = |k: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| vk == k.0 as u32;
        if is(VK_LCONTROL) || is(VK_RCONTROL) || is(VK_CONTROL) {
            self.ctrl = down;
        }
        if is(VK_LMENU) || is(VK_RMENU) || is(VK_MENU) {
            self.alt = down;
        }
        if !matches!(self.mode, Mode::Remote { .. }) {
            return false;
        }
        // Ctrl+Alt+Esc: always bring the pointer home.
        if down && is(VK_ESCAPE) && self.ctrl && self.alt {
            self.go_home(true);
            return true;
        }
        // Scroll Lock is this PC's: it locks the pointer where it is.
        if is(VK_SCROLL) {
            return false;
        }
        self.send(InputEvent::Key {
            vk: vk as u16,
            scan: info.scanCode as u16,
            extended: info.flags.0 & LLKHF_EXTENDED.0 != 0,
            down,
        });
        true
    }
}

// ---------- The overlay that hides the parked cursor ----------

unsafe fn create_overlay(instance: HINSTANCE) -> HWND {
    let class = WNDCLASSW {
        lpfnWndProc: Some(overlay_proc),
        hInstance: instance,
        lpszClassName: w!("TrovulInputOverlay"),
        hCursor: HCURSOR::default(),
        ..Default::default()
    };
    RegisterClassW(&class);
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        w!("TrovulInputOverlay"),
        w!("Trovul"),
        WS_POPUP,
        0,
        0,
        1,
        1,
        HWND::default(),
        None,
        instance,
        None,
    )
    .unwrap_or_default();
    if !hwnd.is_invalid() {
        // Nearly transparent, but still hit-testable so its (empty) cursor shows.
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA);
    }
    hwnd
}

fn show_overlay(hwnd: HWND) {
    if hwnd.is_invalid() {
        return;
    }
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

fn hide_overlay(hwnd: HWND) {
    if !hwnd.is_invalid() {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_SETCURSOR => {
            SetCursor(HCURSOR::default());
            LRESULT(1)
        }
        // The hooks swallow every click while the overlay is up; if a click
        // still reaches it, they're gone. Never trap the user. (Moves don't
        // count: parking the cursor with SetCursorPos sends one here.)
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            log::warn!("Keyboard & mouse sharing: the input hook stopped answering; giving the pointer back");
            with_state(|s| s.go_home(true));
            hide_overlay(hwnd);
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            super::displays_changed();
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}
