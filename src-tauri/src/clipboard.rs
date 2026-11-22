// Trovul - Shared clipboard: text and images copied on one PC paste on the
// other. A thread owns a message-only window that hears clipboard changes
// (AddClipboardFormatListener). A change is offered to the linked PC: text
// travels in the offer, images as PNG that the peer pulls in pieces (so the
// link keeps carrying mouse moves in between). What the peer offers is put on
// this clipboard by the same thread, which then ignores the change it caused.
//
// Content that password managers mark as private never leaves the PC. Both
// PCs must have the switch on: each one checks its own before sending and
// before accepting.

use crate::peer::{Request, Response};
use crate::util::{lock, now_ms};
use crate::{connection, settings};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    GetClipboardOwner, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::{CF_DIB, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, KillTimer, PostMessageW,
    RegisterClassW, SetTimer, TranslateMessage, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_APP, WM_CLIPBOARDUPDATE, WM_TIMER, WNDCLASSW,
};

/// Text travels inline, up to this many bytes of UTF-8.
const MAX_TEXT: usize = 2 * 1024 * 1024;
/// Bigger images stay on this PC.
const MAX_PNG: u64 = 48 * 1024 * 1024;
const MAX_PIXELS: u64 = 80_000_000;
const CHUNK: u32 = 512 * 1024;
/// Apps often write the clipboard several times in a row; wait for quiet.
const SETTLE_MS: u32 = 150;
const TIMER_ID: usize = 1;
const WM_APPLY: u32 = WM_APP + 1;

/// What the peer offers when something was copied there.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipOffer {
    pub id: u64,
    pub text: Option<String>,
    pub image: Option<ImageInfo>,
    /// Files and folders copied in Explorer (clipfiles.rs); older peers ignore it.
    #[serde(default)]
    pub files: Option<crate::clipfiles::FilesInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    /// PNG bytes, pulled with `GetClipboardChunk`.
    pub size: u64,
    pub sha256: String,
}

/// The last thing shared, for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct Activity {
    /// This PC sent it (else it came from the peer).
    pub sent: bool,
    /// "text" | "image"
    pub kind: &'static str,
    /// "42 characters", "1920 × 1080".
    pub detail: String,
    /// Wall clock, ms.
    pub at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClipboardView {
    pub enabled: bool,
    pub last: Option<Activity>,
}

/// Content waiting for the clipboard thread to put it on the clipboard.
#[derive(Default)]
struct Incoming {
    text: Option<String>,
    dib: Option<Vec<u8>>,
    png: Option<Vec<u8>>,
    /// Files received into a local folder, to paste (CF_HDROP).
    files: Option<Vec<std::path::PathBuf>>,
}

static WINDOW: AtomicIsize = AtomicIsize::new(0);
static PENDING: Mutex<Option<Incoming>> = Mutex::new(None);
/// The image this PC offered last: (offer id, PNG).
static OFFERED: Mutex<Option<(u64, Arc<Vec<u8>>)>> = Mutex::new(None);
/// Numbers this PC's copies in order. A newer copy makes an older one that is
/// still being prepared (a big image) stale, so it's never sent after it.
static LOCAL_SEQ: AtomicU64 = AtomicU64::new(0);
/// Id of the newest offer from the peer: older ones arriving late, and images
/// still downloading when something newer was copied, are dropped.
static NEWEST_OFFER: AtomicU64 = AtomicU64::new(0);
/// What both clipboards hold since the last copy that crossed (accepted by
/// the peer, or received from it). The same content isn't sent again, so
/// nothing bounces back and forth.
static SHARED_HASH: Mutex<Option<[u8; 32]>> = Mutex::new(None);
static LAST: Mutex<Option<Activity>> = Mutex::new(None);

pub fn view() -> ClipboardView {
    ClipboardView {
        enabled: settings::get().clipboard_sharing,
        last: lock(&LAST).clone(),
    }
}

pub fn set_enabled(enabled: bool) {
    settings::update(|s| s.clipboard_sharing = enabled);
    if !enabled {
        lock(&OFFERED).take();
        crate::clipfiles::forget();
    }
    crate::on_clipboard_changed();
}

/// A link was made or ended: nothing offered or shared carries over to the next.
pub fn link_changed() {
    lock(&OFFERED).take();
    crate::clipfiles::forget();
    lock(&SHARED_HASH).take();
    NEWEST_OFFER.store(0, Ordering::Release);
}

/// Offer ids grow across restarts too (wall clock, then the copy's number), so
/// the peer can tell an older offer arriving late from a newer one.
fn offer_id(seq: u64) -> u64 {
    now_ms() * 1024 + seq % 1024
}

fn record(sent: bool, kind: &'static str, detail: String) {
    if !sent && kind == "image" {
        let from = connection::current()
            .map(|l| l.info().peer_hostname.clone())
            .unwrap_or_else(|| "the other PC".into());
        crate::popups::show(
            crate::popups::Popup::new(
                "info",
                "image",
                format!("Image from {from}"),
                format!("{detail} · ready to paste"),
            ),
            None,
        );
    }
    *lock(&LAST) = Some(Activity {
        sent,
        kind,
        detail,
        at: now_ms(),
    });
    crate::on_clipboard_changed();
}

fn text_detail(text: &str) -> String {
    let chars = text.chars().count();
    format!("{chars} character{}", if chars == 1 { "" } else { "s" })
}

fn hash(kind: &[u8], data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(kind);
    h.update(data);
    h.finalize().into()
}

fn already_shared(h: [u8; 32]) -> bool {
    *lock(&SHARED_HASH) == Some(h)
}

fn remember_shared(h: [u8; 32]) {
    *lock(&SHARED_HASH) = Some(h);
}

// ============ The clipboard thread ============

/// Start listening for clipboard changes (once, for the app's lifetime).
pub fn start() {
    if WINDOW.load(Ordering::Acquire) != 0 {
        return;
    }
    // Files received in an earlier run: pasted by now, or forgotten.
    crate::clipfiles::clean_temp();
    let spawned = std::thread::Builder::new()
        .name("trovul-clipboard".into())
        .spawn(|| unsafe { run() });
    if let Err(e) = spawned {
        log::warn!("Clipboard sharing unavailable: {e}");
    }
}

unsafe fn run() {
    let instance = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: w!("TrovulClipboard"),
        ..Default::default()
    };
    RegisterClassW(&class);
    let hwnd = match CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("TrovulClipboard"),
        w!("Trovul clipboard"),
        WINDOW_STYLE::default(),
        0,
        0,
        0,
        0,
        HWND_MESSAGE,
        None,
        instance,
        None,
    ) {
        Ok(hwnd) => hwnd,
        Err(e) => {
            log::warn!("Clipboard sharing unavailable: {e}");
            return;
        }
    };
    if let Err(e) = AddClipboardFormatListener(hwnd) {
        log::warn!("Clipboard sharing unavailable: {e}");
        return;
    }
    WINDOW.store(hwnd.0 as isize, Ordering::Release);
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, HWND::default(), 0, 0).0 > 0 {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CLIPBOARDUPDATE => {
            // Our own write (content from the peer) isn't offered back.
            if GetClipboardOwner().ok() != Some(hwnd) {
                SetTimer(hwnd, TIMER_ID, SETTLE_MS, None);
            }
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_ID => {
            let _ = KillTimer(hwnd, TIMER_ID);
            let result = std::panic::catch_unwind(|| copied_here(hwnd));
            if result.is_err() {
                log::warn!("Clipboard: reading the clipboard failed");
            }
            LRESULT(0)
        }
        WM_APPLY => {
            // Taken first: a slow clipboard must not hold the lock the link's
            // request thread needs for the next offer.
            let incoming = lock(&PENDING).take();
            if let Some(incoming) = incoming {
                if let Err(e) = write_clipboard(hwnd, &incoming) {
                    log::warn!("Clipboard: could not paste what the other PC copied: {e}");
                }
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

/// Another app may hold the clipboard for a moment.
unsafe fn open(hwnd: HWND) -> Result<(), String> {
    for attempt in 0..10 {
        if OpenClipboard(hwnd).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10 + attempt * 10));
    }
    Err("the clipboard is busy".into())
}

fn format_id(name: PCWSTR) -> u32 {
    unsafe { RegisterClipboardFormatW(name) }
}

unsafe fn global_bytes(handle: HANDLE) -> Option<Vec<u8>> {
    let global = HGLOBAL(handle.0);
    let size = GlobalSize(global);
    let ptr = GlobalLock(global) as *const u8;
    if ptr.is_null() {
        return None;
    }
    let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
    let _ = GlobalUnlock(global);
    Some(bytes)
}

enum Copied {
    Text(String),
    Png(Vec<u8>),
    Dib(Vec<u8>),
    Files(Vec<std::path::PathBuf>),
}

/// Something was copied on this PC: offer it to the peer.
fn copied_here(hwnd: HWND) {
    if !settings::get().clipboard_sharing || connection::current().is_none() {
        return;
    }
    let copied = match unsafe { read_clipboard(hwnd) } {
        Ok(Some(copied)) => copied,
        Ok(None) => return,
        Err(e) => {
            log::debug!("Clipboard: {e}");
            return;
        }
    };
    let seq = LOCAL_SEQ.fetch_add(1, Ordering::AcqRel) + 1;
    // Images are converted and hashed off this thread; the window stays responsive.
    std::thread::spawn(move || {
        if let Err(e) = offer(copied, seq) {
            log::debug!("Clipboard: not shared: {e}");
        }
    });
}

unsafe fn read_clipboard(hwnd: HWND) -> Result<Option<Copied>, String> {
    open(hwnd)?;
    let copied = (|| {
        // Password managers ask clipboard tools to leave their content alone.
        if IsClipboardFormatAvailable(format_id(w!(
            "ExcludeClipboardContentFromMonitorProcessing"
        )))
        .is_ok()
        {
            return None;
        }
        let history = format_id(w!("CanIncludeInClipboardHistory"));
        if IsClipboardFormatAvailable(history).is_ok() {
            let allowed = GetClipboardData(history)
                .ok()
                .and_then(|h| global_bytes(h))
                .is_none_or(|b| b.len() < 4 || b[..4] != [0, 0, 0, 0]);
            if !allowed {
                return None;
            }
        }
        // Files copied in Explorer (they may come with text, e.g. their names).
        if IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_ok() {
            let drop = HDROP(GetClipboardData(CF_HDROP.0 as u32).ok()?.0);
            let count = DragQueryFileW(drop, u32::MAX, None);
            let paths: Vec<std::path::PathBuf> = (0..count)
                .filter_map(|i| {
                    let len = DragQueryFileW(drop, i, None) as usize;
                    let mut buf = vec![0u16; len + 1];
                    let got = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
                    (got > 0).then(|| String::from_utf16_lossy(&buf[..got]).into())
                })
                .collect();
            if !paths.is_empty() {
                return Some(Copied::Files(paths));
            }
        }
        if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_ok() {
            let bytes = global_bytes(GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?)?;
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|&u| u != 0)
                .collect();
            let text = String::from_utf16_lossy(&units);
            return (!text.is_empty()).then_some(Copied::Text(text));
        }
        let png = format_id(w!("PNG"));
        if IsClipboardFormatAvailable(png).is_ok() {
            if let Some(bytes) = GetClipboardData(png).ok().and_then(|h| global_bytes(h)) {
                return Some(Copied::Png(bytes));
            }
        }
        if IsClipboardFormatAvailable(CF_DIB.0 as u32).is_ok() {
            return global_bytes(GetClipboardData(CF_DIB.0 as u32).ok()?).map(Copied::Dib);
        }
        None
    })();
    let _ = CloseClipboard();
    Ok(copied)
}

fn offer(copied: Copied, seq: u64) -> Result<(), String> {
    let id = offer_id(seq);
    let stale = || LOCAL_SEQ.load(Ordering::Acquire) != seq;
    let (offer, kind, detail, content) = match copied {
        Copied::Text(text) => {
            if text.len() > MAX_TEXT {
                return Err(format!("text too long ({} bytes)", text.len()));
            }
            let content = hash(b"text", text.as_bytes());
            if already_shared(content) || stale() {
                return Ok(());
            }
            let detail = text_detail(&text);
            let offer = ClipOffer {
                id,
                text: Some(text),
                image: None,
                files: None,
            };
            (offer, "text", detail, content)
        }
        Copied::Files(paths) => {
            let (info, sources) = crate::clipfiles::list(&paths)?;
            let content = crate::clipfiles::hash(&sources);
            if already_shared(content) || stale() {
                return Ok(());
            }
            let detail = info.describe();
            crate::clipfiles::offered(id, sources);
            let offer = ClipOffer {
                id,
                text: None,
                image: None,
                files: Some(info),
            };
            (offer, "files", detail, content)
        }
        copied => {
            let (width, height, rgba) = match copied {
                Copied::Png(png) => decode_png(&png)?,
                Copied::Dib(dib) => dib_to_rgba(&dib).ok_or("unsupported bitmap format")?,
                Copied::Text(_) | Copied::Files(_) => unreachable!(),
            };
            let content = image_hash(width, height, &rgba);
            if already_shared(content) || stale() {
                return Ok(());
            }
            let png = encode_png(width, height, &rgba)?;
            if png.len() as u64 > MAX_PNG {
                return Err(format!("image too large ({} bytes)", png.len()));
            }
            if stale() {
                return Ok(()); // something newer was copied while encoding
            }
            let info = ImageInfo {
                width,
                height,
                size: png.len() as u64,
                sha256: hex(&Sha256::digest(&png)),
            };
            *lock(&OFFERED) = Some((id, Arc::new(png)));
            let offer = ClipOffer {
                id,
                text: None,
                image: Some(info),
                files: None,
            };
            (offer, "image", format!("{width} × {height}"), content)
        }
    };
    tauri::async_runtime::spawn(async move {
        match connection::request(Request::ClipboardOffer { offer }).await {
            Ok(_) => {
                // Only once the peer took it: a refused copy is offered again next time.
                if LOCAL_SEQ.load(Ordering::Acquire) == seq {
                    remember_shared(content);
                }
                record(true, kind, detail);
            }
            Err(e) => log::debug!("Clipboard: the other PC didn't take it: {e}"),
        }
    });
    Ok(())
}

// ============ From the peer ============

/// The peer copied something.
pub fn offer_from_peer(offer: ClipOffer) -> Response {
    if !settings::get().clipboard_sharing {
        return Response::Error {
            message: "Clipboard sharing is off on this PC".into(),
        };
    }
    if WINDOW.load(Ordering::Acquire) == 0 {
        return Response::Error {
            message: "Clipboard sharing is unavailable on this PC".into(),
        };
    }
    if NEWEST_OFFER.fetch_max(offer.id, Ordering::AcqRel) >= offer.id {
        return Response::Ok; // an older copy that arrived late
    }
    if let Some(text) = offer.text {
        let content = hash(b"text", text.as_bytes());
        if text.len() <= MAX_TEXT && !already_shared(content) {
            remember_shared(content);
            let detail = text_detail(&text);
            apply(Incoming {
                text: Some(text),
                dib: None,
                png: None,
                files: None,
            });
            record(false, "text", detail);
        }
        return Response::Ok;
    }
    if let Some(files) = offer.files {
        let from = connection::current()
            .map(|l| l.info().peer_hostname.clone())
            .unwrap_or_else(|| "The other PC".into());
        crate::clipfiles::receive(offer.id, from, files);
        return Response::Ok;
    }
    if let Some(image) = offer.image {
        if image.size > MAX_PNG || u64::from(image.width) * u64::from(image.height) > MAX_PIXELS {
            return Response::Error {
                message: "Image too large".into(),
            };
        }
        tauri::async_runtime::spawn(async move {
            if let Err(e) = fetch_image(offer.id, image).await {
                log::warn!("Clipboard: could not get the image from the other PC: {e}");
            }
        });
    }
    Response::Ok
}

async fn fetch_image(id: u64, image: ImageInfo) -> Result<(), String> {
    let mut png = Vec::with_capacity(image.size as usize);
    while (png.len() as u64) < image.size {
        if NEWEST_OFFER.load(Ordering::Acquire) != id || !settings::get().clipboard_sharing {
            return Ok(()); // something newer was copied, or sharing was turned off
        }
        let len = (image.size - png.len() as u64).min(u64::from(CHUNK)) as u32;
        let request = Request::GetClipboardChunk {
            id,
            offset: png.len() as u64,
            len,
        };
        match connection::request(request).await? {
            Response::ClipboardChunk { data } => {
                let bytes = STANDARD.decode(data).map_err(|e| e.to_string())?;
                if bytes.is_empty() {
                    return Err("the image is no longer there".into());
                }
                png.extend_from_slice(&bytes);
            }
            _ => return Err("unexpected answer".into()),
        }
    }
    if hex(&Sha256::digest(&png)) != image.sha256 {
        return Err("the image arrived damaged".into());
    }
    let prepared = tauri::async_runtime::spawn_blocking(move || -> Result<_, String> {
        let (width, height, rgba) = decode_png(&png)?;
        let content = image_hash(width, height, &rgba);
        Ok((
            content,
            rgba_to_dib(width, height, &rgba),
            png,
            width,
            height,
        ))
    })
    .await
    .map_err(|e| e.to_string())??;
    let (content, dib, png, width, height) = prepared;
    let current = NEWEST_OFFER.load(Ordering::Acquire) == id && settings::get().clipboard_sharing;
    if current && !already_shared(content) {
        remember_shared(content);
        apply(Incoming {
            text: None,
            dib: Some(dib),
            png: Some(png),
            files: None,
        });
        record(false, "image", format!("{width} × {height}"));
    }
    Ok(())
}

/// The files are in a local folder now: put them on this PC's clipboard.
pub(crate) fn put_files(paths: Vec<std::path::PathBuf>, info: &crate::clipfiles::FilesInfo) {
    remember_shared(crate::clipfiles::hash(&paths));
    apply(Incoming {
        files: Some(paths),
        ..Default::default()
    });
    record(false, "files", info.describe());
}

/// CF_HDROP: a DROPFILES header, then the wide paths, each ending in 0, and a final 0.
fn hdrop(paths: &[std::path::PathBuf]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&20u32.to_le_bytes()); // pFiles: right after the header
    bytes.extend_from_slice(&[0u8; 12]); // pt, fNC
    bytes.extend_from_slice(&1u32.to_le_bytes()); // fWide
    for path in paths {
        for unit in path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
        {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&[0, 0]);
    bytes
}

/// A piece of the image this PC offered.
pub fn chunk(id: u64, offset: u64, len: u32) -> Response {
    if !settings::get().clipboard_sharing {
        return Response::Error {
            message: "Clipboard sharing is off on this PC".into(),
        };
    }
    let offered = lock(&OFFERED).clone();
    match offered {
        Some((offered_id, png)) if offered_id == id => {
            let start = (offset as usize).min(png.len());
            let end = (start + len.min(CHUNK) as usize).min(png.len());
            Response::ClipboardChunk {
                data: STANDARD.encode(&png[start..end]),
            }
        }
        _ => Response::Error {
            message: "That copy was replaced by a newer one".into(),
        },
    }
}

fn apply(incoming: Incoming) {
    *lock(&PENDING) = Some(incoming);
    let hwnd = HWND(WINDOW.load(Ordering::Acquire) as *mut c_void);
    unsafe {
        let _ = PostMessageW(hwnd, WM_APPLY, WPARAM(0), LPARAM(0));
    }
}

unsafe fn write_clipboard(hwnd: HWND, incoming: &Incoming) -> Result<(), String> {
    open(hwnd)?;
    let result = (|| {
        EmptyClipboard().map_err(|e| e.to_string())?;
        if let Some(text) = &incoming.text {
            let mut bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
            bytes.extend_from_slice(&[0, 0]);
            put(CF_UNICODETEXT.0 as u32, &bytes)?;
        }
        if let Some(dib) = &incoming.dib {
            put(CF_DIB.0 as u32, dib)?;
        }
        if let Some(png) = &incoming.png {
            // Browsers and Office prefer it: it keeps transparency.
            put(format_id(w!("PNG")), png)?;
        }
        if let Some(files) = &incoming.files {
            put(CF_HDROP.0 as u32, &hdrop(files))?;
            // Paste copies (it doesn't move them out of our folder).
            put(format_id(w!("Preferred DropEffect")), &1u32.to_le_bytes())?;
        }
        Ok(())
    })();
    let _ = CloseClipboard();
    result
}

unsafe fn put(format: u32, bytes: &[u8]) -> Result<(), String> {
    let global = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).map_err(|e| e.to_string())?;
    let ptr = GlobalLock(global) as *mut u8;
    if ptr.is_null() {
        let _ = GlobalFree(global);
        return Err("out of memory".into());
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
    let _ = GlobalUnlock(global);
    if let Err(e) = SetClipboardData(format, HANDLE(global.0)) {
        // Still ours to free: the clipboard didn't take it.
        let _ = GlobalFree(global);
        return Err(e.to_string());
    }
    Ok(())
}

// ============ Pixels ============

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn image_hash(width: u32, height: u32, rgba: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"image");
    h.update(width.to_le_bytes());
    h.update(height.to_le_bytes());
    h.update(rgba);
    h.finalize().into()
}

fn decode_png(png: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .map_err(|e| format!("bad PNG: {e}"))?
        .to_rgba8();
    Ok((image.width(), image.height(), image.into_raw()))
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut png,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
    .map_err(|e| e.to_string())?;
    Ok(png)
}

const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// CF_DIB (24- or 32-bit, bottom-up or top-down) → RGBA.
fn dib_to_rgba(dib: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let header = u32_at(dib, 0)? as usize;
    let width = i32::from_le_bytes(dib.get(4..8)?.try_into().ok()?);
    let height = i32::from_le_bytes(dib.get(8..12)?.try_into().ok()?);
    let bits = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?);
    let compression = u32_at(dib, 16)?;
    let colors = u32_at(dib, 32).unwrap_or(0) as usize;
    if width <= 0 || height == 0 || header < 40 || !(bits == 24 || bits == 32) {
        return None;
    }
    if compression != BI_RGB && !(compression == BI_BITFIELDS && bits == 32) {
        return None;
    }
    if compression == BI_BITFIELDS {
        // Only the usual layout: blue, green, red bytes.
        // Right after a 40-byte header, or inside a V4/V5 one: offset 40 either way.
        let (r, g, b) = (u32_at(dib, 40)?, u32_at(dib, 44)?, u32_at(dib, 48)?);
        if (r, g, b) != (0x00ff_0000, 0x0000_ff00, 0x0000_00ff) {
            return None;
        }
    }
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    if (w as u64) * (h as u64) > MAX_PIXELS {
        return None;
    }
    let masks = if compression == BI_BITFIELDS && header == 40 {
        12
    } else {
        0
    };
    let pixels = header + masks + colors * 4;
    let stride = (w * bits as usize / 8).div_ceil(4) * 4;
    if dib.len() < pixels + stride * h {
        return None;
    }
    let bytes_per = bits as usize / 8;
    let mut rgba = vec![0u8; w * h * 4];
    let mut any_alpha = false;
    for y in 0..h {
        // Bottom-up unless the height is negative.
        let src_row = if height > 0 { h - 1 - y } else { y };
        let row = &dib[pixels + src_row * stride..];
        for x in 0..w {
            let p = &row[x * bytes_per..];
            let o = (y * w + x) * 4;
            rgba[o] = p[2];
            rgba[o + 1] = p[1];
            rgba[o + 2] = p[0];
            rgba[o + 3] = if bytes_per == 4 { p[3] } else { 255 };
            any_alpha |= bytes_per == 4 && p[3] != 0;
        }
    }
    if bytes_per == 4 && !any_alpha {
        // Most 32-bit bitmaps leave alpha at 0: they're opaque.
        rgba.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    }
    Some((w as u32, h as u32, rgba))
}

/// RGBA → CF_DIB, 32-bit bottom-up (what every app can paste).
fn rgba_to_dib(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut dib = Vec::with_capacity(40 + w * h * 4);
    dib.extend_from_slice(&40u32.to_le_bytes());
    dib.extend_from_slice(&(width as i32).to_le_bytes());
    dib.extend_from_slice(&(height as i32).to_le_bytes());
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&32u16.to_le_bytes());
    dib.extend_from_slice(&BI_RGB.to_le_bytes());
    dib.extend_from_slice(&((w * h * 4) as u32).to_le_bytes());
    dib.extend_from_slice(&[0u8; 16]); // resolution, colours used/important
    for y in (0..h).rev() {
        for p in rgba[y * w * 4..(y + 1) * w * 4].chunks_exact(4) {
            dib.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    dib
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (u32, u32, Vec<u8>) {
        let (w, h) = (3u32, 2u32);
        let rgba = (0..w * h)
            .flat_map(|i| [i as u8 * 10, 100 + i as u8, 200 - i as u8, 255])
            .collect();
        (w, h, rgba)
    }

    #[test]
    fn dib_round_trips() {
        let (w, h, rgba) = sample();
        let dib = rgba_to_dib(w, h, &rgba);
        assert_eq!(dib_to_rgba(&dib), Some((w, h, rgba)));
    }

    #[test]
    fn dib_24_bit_top_down_with_padding() {
        // 3 × 2 at 24 bits: rows of 9 bytes padded to 12; negative height = top-down.
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&3i32.to_le_bytes());
        dib.extend_from_slice(&(-2i32).to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&24u16.to_le_bytes());
        dib.extend_from_slice(&[0u8; 24]);
        for row in 0..2u8 {
            for x in 0..3u8 {
                dib.extend_from_slice(&[x, row, 9]); // B G R
            }
            dib.extend_from_slice(&[0, 0, 0]);
        }
        let (w, h, rgba) = dib_to_rgba(&dib).unwrap();
        assert_eq!((w, h), (3, 2));
        assert_eq!(&rgba[0..4], &[9, 0, 0, 255]);
        assert_eq!(&rgba[(3 + 2) * 4..(3 + 2) * 4 + 4], &[9, 1, 2, 255]);
    }

    #[test]
    fn transparent_looking_32_bit_bitmaps_are_opaque() {
        let (w, h, mut rgba) = sample();
        rgba.chunks_exact_mut(4).for_each(|p| p[3] = 0);
        let (_, _, back) = dib_to_rgba(&rgba_to_dib(w, h, &rgba)).unwrap();
        assert!(back.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn png_round_trips() {
        let (w, h, rgba) = sample();
        let png = encode_png(w, h, &rgba).unwrap();
        assert_eq!(decode_png(&png).unwrap(), (w, h, rgba));
    }

    #[test]
    fn unsupported_bitmaps_are_refused() {
        let (w, h, rgba) = sample();
        let mut dib = rgba_to_dib(w, h, &rgba);
        dib[14] = 8; // 8-bit palette
        assert_eq!(dib_to_rgba(&dib), None);
        assert_eq!(dib_to_rgba(&dib[..20]), None);
    }

    /// Writes text and an image to the real clipboard and reads them back
    /// the way a copy on this PC is read, then puts the previous text back.
    /// `cargo test -- --ignored hardware`.
    #[test]
    #[ignore]
    fn hardware_clipboard_round_trip() {
        unsafe {
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("clipboard test"),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                None,
                None,
                None,
            )
            .unwrap();
            let before = match read_clipboard(hwnd) {
                Ok(Some(Copied::Text(text))) => Some(text),
                _ => None,
            };

            let text = "Trovul clipboard test — ação ✓";
            let incoming = Incoming {
                text: Some(text.into()),
                dib: None,
                png: None,
                files: None,
            };
            write_clipboard(hwnd, &incoming).unwrap();
            assert!(matches!(read_clipboard(hwnd), Ok(Some(Copied::Text(t))) if t == text));

            let (w, h, rgba) = sample();
            let png = encode_png(w, h, &rgba).unwrap();
            let incoming = Incoming {
                text: None,
                dib: Some(rgba_to_dib(w, h, &rgba)),
                png: Some(png.clone()),
                files: None,
            };
            write_clipboard(hwnd, &incoming).unwrap();
            match read_clipboard(hwnd) {
                Ok(Some(Copied::Png(bytes))) => {
                    assert_eq!(decode_png(&bytes).unwrap(), (w, h, rgba.clone()))
                }
                other => panic!(
                    "expected the PNG back, got {}",
                    matches!(other, Ok(Some(_)))
                ),
            }
            // Apps that only read CF_DIB get the same pixels.
            open(hwnd).unwrap();
            let dib = global_bytes(GetClipboardData(CF_DIB.0 as u32).unwrap()).unwrap();
            let _ = CloseClipboard();
            assert_eq!(dib_to_rgba(&dib), Some((w, h, rgba)));

            // Files: what Explorer pastes (CF_HDROP) comes back as the same paths.
            let file = std::env::temp_dir().join("trovul-clipboard-test.txt");
            std::fs::write(&file, b"hello").unwrap();
            let incoming = Incoming {
                files: Some(vec![file.clone()]),
                ..Default::default()
            };
            write_clipboard(hwnd, &incoming).unwrap();
            match read_clipboard(hwnd) {
                Ok(Some(Copied::Files(paths))) => assert_eq!(paths, vec![file.clone()]),
                other => panic!(
                    "expected the file back, got {}",
                    matches!(other, Ok(Some(_)))
                ),
            }
            let _ = std::fs::remove_file(&file);

            let restore = Incoming {
                text: before.or(Some(String::new())),
                dib: None,
                png: None,
                files: None,
            };
            write_clipboard(hwnd, &restore).unwrap();
            println!("clipboard round trip ok (text, PNG and DIB)");
        }
    }

    #[test]
    fn shared_content_is_not_offered_again() {
        let h = hash(b"text", b"clipboard test value 7f3a");
        assert!(!already_shared(h));
        remember_shared(h);
        assert!(already_shared(h));
        assert!(!already_shared(hash(b"text", b"something else")));
        link_changed();
        assert!(!already_shared(h));
    }

    #[test]
    fn offer_ids_grow() {
        let a = offer_id(1023);
        let b = offer_id(1024);
        std::thread::sleep(Duration::from_millis(2));
        assert!(offer_id(1) > a.max(b));
    }
}
