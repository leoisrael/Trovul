// Trovul - Pop-ups: small cards in the corner of the screen, above the clock,
// for what happens between the PCs (a PC linked or lost, files or an image
// arriving, an update to install). Same look as the tray mixer, but they never
// take the focus, and they stay away while a game runs in exclusive full
// screen. One frameless window (label "popups") holds the stack; the frontend
// sizes it to its cards (`resize`) and asks to hide it when the last one goes.

use crate::util::lock;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

pub const LABEL: &str = "popups";
const WIDTH: f64 = 360.0;
const MARGIN: f64 = 12.0;

#[derive(Debug, Clone, Serialize)]
pub struct Action {
    pub id: String,
    pub label: String,
    pub primary: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Popup {
    pub id: u64,
    /// "info" | "success" | "warning" | "error"
    pub tone: &'static str,
    /// Icon key the frontend maps to a Lucide icon ("link", "files", …).
    pub icon: &'static str,
    pub title: String,
    pub text: String,
    /// 0–1 while something is on its way.
    pub progress: Option<f32>,
    pub actions: Vec<Action>,
    /// Stays until acted on or closed (else it fades after a few seconds).
    pub sticky: bool,
}

impl Popup {
    pub fn new(
        tone: &'static str,
        icon: &'static str,
        title: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            tone,
            icon,
            title: title.into(),
            text: text.into(),
            progress: None,
            actions: Vec::new(),
            sticky: false,
        }
    }

    pub fn action(mut self, id: &str, label: &str, primary: bool) -> Self {
        self.actions.push(Action {
            id: id.into(),
            label: label.into(),
            primary,
        });
        self.sticky = true;
        self
    }

    pub fn progress(mut self, progress: f32) -> Self {
        self.progress = Some(progress.clamp(0.0, 1.0));
        self.sticky = true;
        self
    }
}

type Handler = Box<dyn Fn(&str) + Send + 'static>;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// Popups shown and not dismissed yet: sent again when the window (re)loads.
static SHOWN: Mutex<Vec<Popup>> = Mutex::new(Vec::new());
static HANDLERS: Mutex<Option<HashMap<u64, Handler>>> = Mutex::new(None);
static CREATING: AtomicBool = AtomicBool::new(false);

/// Show (or update, same id) a pop-up. `on_action` runs when one of its
/// buttons is clicked, with the action id.
pub fn show(popup: Popup, on_action: Option<Handler>) {
    let Some(app) = crate::app_handle() else {
        return;
    };
    let interactive = !popup.actions.is_empty() || popup.progress.is_some();
    if !interactive && !crate::settings::get().notifications {
        return;
    }
    // A card over an exclusive full-screen game can minimize it.
    if crate::input::exclusive_full_screen() {
        return;
    }
    if let Some(handler) = on_action {
        lock(&HANDLERS)
            .get_or_insert_with(HashMap::new)
            .insert(popup.id, handler);
    }
    {
        let mut shown = lock(&SHOWN);
        match shown.iter_mut().find(|p| p.id == popup.id) {
            Some(existing) => *existing = popup.clone(),
            None => shown.push(popup.clone()),
        }
    }
    match app.get_webview_window(LABEL) {
        Some(window) => {
            let _ = app.emit_to(LABEL, "popup", popup);
            reveal(&window);
        }
        None => {
            // Built off the caller's thread: creating a webview from an event
            // handler can deadlock on Windows. The page asks for SHOWN on load.
            if !CREATING.swap(true, Ordering::AcqRel) {
                std::thread::spawn(move || {
                    match create(&app) {
                        Ok(window) => reveal(&window),
                        Err(e) => log::warn!("Could not open the pop-ups window: {e}"),
                    }
                    CREATING.store(false, Ordering::Release);
                });
            }
        }
    }
}

/// Update a pop-up's progress (and text) without re-sending its actions.
pub fn update(id: u64, progress: Option<f32>, text: Option<String>) {
    let updated = {
        let mut shown = lock(&SHOWN);
        let Some(popup) = shown.iter_mut().find(|p| p.id == id) else {
            return;
        };
        if let Some(p) = progress {
            popup.progress = Some(p.clamp(0.0, 1.0));
        }
        if let Some(t) = text {
            popup.text = t;
        }
        popup.clone()
    };
    if let Some(app) = crate::app_handle() {
        let _ = app.emit_to(LABEL, "popup", updated);
    }
}

/// Replace a pop-up (e.g. "Receiving…" becomes "Ready to paste").
pub fn replace(id: u64, mut popup: Popup) {
    popup.id = id;
    show(popup, None);
}

pub fn dismiss(id: u64) {
    lock(&SHOWN).retain(|p| p.id != id);
    if let Some(handlers) = lock(&HANDLERS).as_mut() {
        handlers.remove(&id);
    }
    if let Some(app) = crate::app_handle() {
        let _ = app.emit_to(LABEL, "popup-dismissed", id);
    }
}

fn create(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
        .title("Trovul")
        .inner_size(WIDTH, 100.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .shadow(true)
        .skip_taskbar(true)
        .always_on_top(true)
        .focusable(false)
        .focused(false)
        .visible(false)
        .theme(Some(tauri::Theme::Dark))
        .background_color(tauri::window::Color(11, 11, 18, 255))
        .build()
}

/// Show without activating, above everything, in the corner.
///
/// The window is shown and hidden with Win32 calls only (see `set_shown`):
/// tao never learns about a ShowWindow it didn't make, so its own `hide()`
/// would see "already hidden" and do nothing, leaving an empty strip behind.
fn reveal(window: &WebviewWindow) {
    place(window);
    set_shown(window, true);
}

/// Posted (async) so a hide and a show from different threads land in order.
#[cfg(windows)]
fn set_shown(window: &WebviewWindow, shown: bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, ShowWindowAsync, HWND_TOPMOST, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE,
    };
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    unsafe {
        let hwnd = HWND(hwnd.0);
        if !shown {
            let _ = ShowWindowAsync(hwnd, SW_HIDE);
            return;
        }
        let _ = ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_ASYNCWINDOWPOS,
        );
    }
}

#[cfg(not(windows))]
fn set_shown(window: &WebviewWindow, shown: bool) {
    let _ = if shown { window.show() } else { window.hide() };
}

/// Bottom-right of the primary screen's work area (top-right with a taskbar at the top).
fn place(window: &WebviewWindow) {
    let Ok(Some(monitor)) = window.primary_monitor() else {
        return;
    };
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let full = monitor.position();
    let size = window.outer_size().unwrap_or_default();
    let margin = (MARGIN * scale).round() as i32;
    let x = area.position.x + area.size.width as i32 - size.width as i32 - margin;
    let taskbar_on_top = area.position.y > full.y;
    let y = if taskbar_on_top {
        area.position.y + margin
    } else {
        area.position.y + area.size.height as i32 - size.height as i32 - margin
    };
    let _ = window.set_position(PhysicalPosition::new(x, y));
}

// ---------- Commands from the pop-ups window ----------

/// What's showing, for a window that just loaded.
pub fn current() -> Vec<Popup> {
    lock(&SHOWN).clone()
}

/// The cards take `height` logical pixels: fit the window and keep it in the corner.
pub fn resize(app: &AppHandle, height: f64) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.set_size(LogicalSize::new(WIDTH, height.clamp(40.0, 900.0)));
        place(&window);
    }
}

/// Nothing left to show. Checked under the lock, so a pop-up arriving right
/// now is revealed after this hide, not before it.
pub fn hide(app: &AppHandle) {
    let shown = lock(&SHOWN);
    if shown.is_empty() {
        if let Some(window) = app.get_webview_window(LABEL) {
            set_shown(&window, false);
        }
    }
}

/// A button on a pop-up was clicked (or it was closed: action "close").
pub fn act(id: u64, action: &str) {
    let handler = lock(&HANDLERS).as_mut().and_then(|h| h.remove(&id));
    if let Some(handler) = handler {
        handler(action);
    }
    if action == "close"
        || !lock(&SHOWN)
            .iter()
            .any(|p| p.id == id && p.progress.is_some())
    {
        dismiss(id);
    }
}
