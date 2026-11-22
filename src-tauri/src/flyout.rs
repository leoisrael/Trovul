// Trovul - The tray flyout: a small mixer that opens above the tray icon on a
// left click (master, the other PC's sound, the apps) and hides as soon as it
// loses focus, like the Windows volume flyout. The webview is created on the
// first click and kept; the frontend renders `Flyout` for the "flyout" label.

use crate::util::mono_ms;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

pub const LABEL: &str = "flyout";
const WIDTH: f64 = 360.0;
const HEIGHT: f64 = 500.0;
/// Gap to the taskbar and the screen edges, in logical pixels.
const MARGIN: f64 = 12.0;

/// When the flyout last hid because it lost focus. A click on the tray icon
/// takes the focus away first, so without this the same click would reopen it.
static HIDDEN_AT: AtomicU64 = AtomicU64::new(0);
static CREATING: AtomicBool = AtomicBool::new(false);

fn create(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
        .title("Trovul")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .shadow(true)
        .skip_taskbar(true)
        .always_on_top(true)
        .visible(false)
        .focused(true)
        .theme(Some(tauri::Theme::Dark))
        .background_color(tauri::window::Color(11, 11, 18, 255))
        .build()
}

/// Left click on the tray icon at `click` (physical pixels).
pub fn toggle(app: &AppHandle, click: PhysicalPosition<f64>) {
    let Some(window) = app.get_webview_window(LABEL) else {
        // Built off the tray handler: creating a webview from an event
        // handler can deadlock on Windows.
        if !CREATING.swap(true, Ordering::AcqRel) {
            let app = app.clone();
            std::thread::spawn(move || {
                match create(&app) {
                    Ok(window) => show(&app, &window, click),
                    Err(e) => {
                        log::warn!("Could not open the tray mixer: {e}");
                        crate::show_main_window();
                    }
                }
                CREATING.store(false, Ordering::Release);
            });
        }
        return;
    };
    if window.is_visible().unwrap_or(false) {
        hide(&window);
        return;
    }
    if mono_ms().saturating_sub(HIDDEN_AT.load(Ordering::Relaxed)) < 400 {
        return; // this click is what closed it
    }
    show(app, &window, click);
}

fn show(app: &AppHandle, window: &WebviewWindow, click: PhysicalPosition<f64>) {
    place(app, window, click);
    let _ = window.show();
    let _ = window.set_focus();
    let _ = app.emit_to(LABEL, "flyout-visibility", true);
}

pub fn hide(window: &WebviewWindow) {
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        HIDDEN_AT.store(mono_ms(), Ordering::Relaxed);
        let _ = window.emit_to(LABEL, "flyout-visibility", false);
    }
}

pub fn hide_now(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        hide(&window);
    }
}

/// Above the tray (or beside it, for a taskbar on the side), inside the work area.
fn place(app: &AppHandle, window: &WebviewWindow, click: PhysicalPosition<f64>) {
    let Ok(Some(monitor)) = app.monitor_from_point(click.x, click.y) else {
        return;
    };
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let (ax, ay) = (f64::from(area.position.x), f64::from(area.position.y));
    let (aw, ah) = (f64::from(area.size.width), f64::from(area.size.height));
    let (w, h, m) = (WIDTH * scale, HEIGHT * scale, MARGIN * scale);
    let (x, y) = anchor(click.x, click.y, (ax, ay, aw, ah), (w, h), m);
    let at = PhysicalPosition::new(x.round() as i32, y.round() as i32);
    let _ = window.set_position(at);
    // Moving to a monitor with another scale rescales the window: size it
    // there, then place it again.
    let _ = window.set_size(LogicalSize::new(WIDTH, HEIGHT));
    let _ = window.set_position(at);
}

/// Top-left corner for a `size` window next to a tray click at (cx, cy),
/// within `area` (x, y, width, height). The taskbar is wherever the click
/// falls outside the work area.
fn anchor(
    cx: f64,
    cy: f64,
    area: (f64, f64, f64, f64),
    size: (f64, f64),
    margin: f64,
) -> (f64, f64) {
    let (ax, ay, aw, ah) = area;
    let (w, h) = size;
    let mut x = cx - w / 2.0;
    let mut y = if cy >= ay + ah {
        ay + ah - h - margin // taskbar at the bottom
    } else if cy <= ay {
        ay + margin // at the top
    } else {
        cy - h / 2.0 // on a side
    };
    if cx <= ax {
        x = ax + margin;
    } else if cx >= ax + aw {
        x = ax + aw - w - margin;
    }
    x = x.clamp(ax + margin, (ax + aw - w - margin).max(ax + margin));
    y = y.clamp(ay + margin, (ay + ah - h - margin).max(ay + margin));
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::anchor;

    const AREA: (f64, f64, f64, f64) = (0.0, 0.0, 1920.0, 1040.0); // taskbar: 40 px at the bottom
    const SIZE: (f64, f64) = (360.0, 500.0);

    #[test]
    fn opens_above_a_bottom_taskbar_under_the_click() {
        assert_eq!(anchor(1700.0, 1060.0, AREA, SIZE, 12.0), (1520.0, 528.0));
    }

    #[test]
    fn stays_on_screen_near_the_corner() {
        assert_eq!(anchor(1900.0, 1060.0, AREA, SIZE, 12.0), (1548.0, 528.0));
    }

    #[test]
    fn follows_a_top_or_side_taskbar() {
        let top = (0.0, 40.0, 1920.0, 1040.0);
        assert_eq!(anchor(1700.0, 20.0, top, SIZE, 12.0), (1520.0, 52.0));
        let left = (60.0, 0.0, 1860.0, 1080.0);
        assert_eq!(anchor(30.0, 900.0, left, SIZE, 12.0), (72.0, 568.0));
    }
}
