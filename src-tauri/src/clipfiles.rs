// Trovul - Files through the shared clipboard: copy files or folders in one
// PC's Explorer, paste them in the other's.
//
// The copy offers only the list (names, sizes). The receiving PC pulls the
// bytes in pieces over the link:
// - up to AUTO_BYTES, at once, into a temporary folder, so pasting simply works;
// - above that, only after "Receive" on a pop-up (a big copy made for this PC
//   alone never crosses the network by itself), into Downloads\Trovul.
// Then the files go on this PC's clipboard (CF_HDROP), ready to paste.
//
// The sender serves only the files it listed; the receiver only writes below
// its target folder (relative paths are checked).

use crate::peer::{Request, Response};
use crate::popups::{self, Popup};
use crate::util::lock;
use crate::{clipboard, connection};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Copies up to this size arrive by themselves.
const AUTO_BYTES: u64 = 50 * 1024 * 1024;
const MAX_ITEMS: usize = 20_000;
const CHUNK: u32 = 256 * 1024;
/// Show progress for automatic copies bigger than this.
const PROGRESS_FROM: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileItem {
    /// Relative, '/'-separated, starting with the copied file or folder's name.
    pub path: String,
    pub size: u64,
    pub dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FilesInfo {
    pub items: Vec<FileItem>,
    pub total: u64,
}

impl FilesInfo {
    pub fn files(&self) -> usize {
        self.items.iter().filter(|i| !i.dir).count()
    }

    /// "3 files · 12.4 MB", "a folder · 1.2 GB".
    pub fn describe(&self) -> String {
        let tops = self.items.iter().filter(|i| !i.path.contains('/')).count();
        let what = match (tops, self.files()) {
            (1, _) if self.items[0].dir => "a folder".to_string(),
            (_, 1) => "1 file".to_string(),
            (_, n) => format!("{n} files"),
        };
        format!("{what} · {}", size_text(self.total))
    }
}

pub fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{bytes} bytes")
    }
}

/// List what was copied: every file and folder below the copied paths.
pub fn list(paths: &[PathBuf]) -> Result<(FilesInfo, Vec<PathBuf>), String> {
    let mut items = Vec::new();
    let mut sources = Vec::new();
    let mut total = 0u64;
    for path in paths {
        let name = path
            .file_name()
            .ok_or("can't share a drive")?
            .to_string_lossy()
            .into_owned();
        walk(path, &name, &mut items, &mut sources, &mut total)?;
    }
    Ok((FilesInfo { items, total }, sources))
}

fn walk(
    path: &Path,
    rel: &str,
    items: &mut Vec<FileItem>,
    sources: &mut Vec<PathBuf>,
    total: &mut u64,
) -> Result<(), String> {
    if items.len() >= MAX_ITEMS {
        return Err(format!("more than {MAX_ITEMS} files"));
    }
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.is_dir() {
        items.push(FileItem {
            path: rel.to_string(),
            size: 0,
            dir: true,
        });
        sources.push(path.to_path_buf());
        let mut children: Vec<_> = std::fs::read_dir(path)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok())
            .collect();
        children.sort_by_key(|e| e.file_name());
        for child in children {
            let name = child.file_name().to_string_lossy().into_owned();
            walk(
                &child.path(),
                &format!("{rel}/{name}"),
                items,
                sources,
                total,
            )?;
        }
    } else {
        *total += meta.len();
        items.push(FileItem {
            path: rel.to_string(),
            size: meta.len(),
            dir: false,
        });
        sources.push(path.to_path_buf());
    }
    Ok(())
}

/// Same copy, same hash (so it isn't offered twice): paths, sizes and times.
pub fn hash(sources: &[PathBuf]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"files");
    for path in sources {
        h.update(path.to_string_lossy().as_bytes());
        if let Ok(meta) = std::fs::metadata(path) {
            h.update(meta.len().to_le_bytes());
            if let Ok(modified) = meta.modified() {
                if let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH) {
                    h.update(since.as_nanos().to_le_bytes());
                }
            }
        }
    }
    h.finalize().into()
}

// ============ Sending ============

/// The files this PC offered last: (offer id, one source path per item).
static OFFERED: Mutex<Option<(u64, Arc<Vec<PathBuf>>)>> = Mutex::new(None);

pub fn offered(id: u64, sources: Vec<PathBuf>) {
    *lock(&OFFERED) = Some((id, Arc::new(sources)));
}

pub fn forget() {
    lock(&OFFERED).take();
}

/// A piece of item `index` of the files this PC offered.
pub fn chunk(id: u64, index: u32, offset: u64, len: u32) -> Response {
    if !crate::settings::get().clipboard_sharing {
        return Response::Error {
            message: "Clipboard sharing is off on this PC".into(),
        };
    }
    let offered = lock(&OFFERED).clone();
    let Some((offered_id, sources)) = offered else {
        return Response::Error {
            message: "Those files were replaced by a newer copy".into(),
        };
    };
    let Some(path) = sources.get(index as usize).filter(|_| offered_id == id) else {
        return Response::Error {
            message: "Those files were replaced by a newer copy".into(),
        };
    };
    let read = (|| -> std::io::Result<Vec<u8>> {
        let mut file = std::fs::File::open(path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len.min(CHUNK) as usize];
        let mut filled = 0;
        while filled < buf.len() {
            let n = file.read(&mut buf[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        buf.truncate(filled);
        Ok(buf)
    })();
    match read {
        Ok(data) => Response::ClipboardChunk {
            data: STANDARD.encode(data),
        },
        Err(e) => Response::Error {
            message: format!("Could not read {}: {e}", path.display()),
        },
    }
}

// ============ Receiving ============

/// Only plain relative paths: no root, no drive, no "..".
fn safe_relative(path: &str) -> Option<PathBuf> {
    let rel = PathBuf::from(path.replace('/', "\\"));
    let ok = !path.is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_)));
    ok.then_some(rel)
}

fn temp_root() -> PathBuf {
    std::env::temp_dir().join("Trovul").join("clipboard")
}

/// Files received earlier and no longer on the clipboard's mind.
pub fn clean_temp() {
    let _ = std::fs::remove_dir_all(temp_root());
}

fn downloads() -> PathBuf {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    let known = unsafe { SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None) }
        .ok()
        .map(|p| unsafe {
            let path = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            PathBuf::from(path)
        })
        .filter(|p| !p.as_os_str().is_empty());
    known
        .or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Downloads")))
        .unwrap_or_else(std::env::temp_dir)
        .join("Trovul")
}

/// "name (2).ext" when `name` is taken in `dir`.
fn free_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..1000)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|candidate| !dir.join(candidate).exists())
        .unwrap_or_else(|| name.to_string())
}

/// The peer copied files: take them now, or ask first when they're big.
pub fn receive(id: u64, from: String, info: FilesInfo) {
    if info.items.is_empty() || info.items.iter().any(|i| safe_relative(&i.path).is_none()) {
        log::warn!("Clipboard: refused a file list with unsafe paths");
        return;
    }
    if info.total <= AUTO_BYTES {
        let dir = temp_root().join(id.to_string());
        tauri::async_runtime::spawn(download(id, from, info, dir, false));
        return;
    }
    let popup = Popup::new(
        "info",
        "files",
        format!("{from} copied {}", info.describe()),
        "Receive them here? They go to Downloads\\Trovul, ready to paste.",
    )
    .action("dismiss", "Not now", false)
    .action("receive", "Receive", true);
    let (from2, info2) = (from.clone(), info.clone());
    popups::show(
        popup,
        Some(Box::new(move |action: &str| {
            if action == "receive" {
                tauri::async_runtime::spawn(download(
                    id,
                    from2.clone(),
                    info2.clone(),
                    downloads(),
                    true,
                ));
            }
        })),
    );
}

async fn download(id: u64, from: String, info: FilesInfo, dir: PathBuf, asked: bool) {
    let cancel = Arc::new(AtomicBool::new(false));
    let show_progress = asked || info.total > PROGRESS_FROM;
    let progress_id = if show_progress {
        let popup = Popup::new(
            "info",
            "files",
            format!("Receiving {} from {from}", info.describe()),
            "Starting…",
        )
        .progress(0.0)
        .action("cancel", "Cancel", false);
        let id = popup.id;
        let flag = cancel.clone();
        popups::show(
            popup,
            Some(Box::new(move |action: &str| {
                if action == "cancel" || action == "close" {
                    flag.store(true, Ordering::Release);
                }
            })),
        );
        Some(id)
    } else {
        None
    };
    let result = fetch_all(id, &info, &dir, cancel.clone(), progress_id).await;
    let done = |popup: Popup| match progress_id {
        Some(pid) => popups::replace(pid, popup),
        None => popups::show(popup, None),
    };
    match result {
        Ok(tops) => {
            clipboard::put_files(tops.clone(), &info);
            let popup = Popup::new(
                "success",
                "files",
                format!("{} from {from}", capitalize(&info.describe())),
                "Ready to paste.",
            );
            if asked {
                let folder = tops.first().cloned();
                let popup = popup.action("open", "Show in folder", true);
                popups::show(
                    popup_with_id(popup, progress_id),
                    Some(Box::new(move |action: &str| {
                        if action == "open" {
                            if let Some(path) = &folder {
                                let _ = std::process::Command::new("explorer.exe")
                                    .arg(format!("/select,{}", path.display()))
                                    .spawn();
                            }
                        }
                    })),
                );
            } else {
                done(popup);
            }
        }
        Err(e) if cancel.load(Ordering::Acquire) => {
            log::info!("Clipboard: file transfer cancelled ({e})");
            if let Some(pid) = progress_id {
                popups::dismiss(pid);
            }
        }
        Err(e) => {
            log::warn!("Clipboard: could not receive files from {from}: {e}");
            done(Popup::new(
                "error",
                "files",
                format!("Files from {from} didn't arrive"),
                e,
            ));
        }
    }
}

fn popup_with_id(mut popup: Popup, id: Option<u64>) -> Popup {
    if let Some(id) = id {
        popup.id = id;
    }
    popup
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// Pull every item into `dir`; returns the top-level paths written.
async fn fetch_all(
    id: u64,
    info: &FilesInfo,
    dir: &Path,
    cancel: Arc<AtomicBool>,
    progress: Option<u64>,
) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    // Top-level names that are taken get " (2)" etc. (Downloads is shared).
    let mut renamed: Vec<(String, String)> = Vec::new();
    let mut tops = Vec::new();
    let mut received = 0u64;
    let mut last_shown = std::time::Instant::now();
    for (index, item) in info.items.iter().enumerate() {
        if cancel.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        let rel = safe_relative(&item.path).ok_or("unsafe path")?;
        let mut parts = rel.components();
        let top = parts
            .next()
            .ok_or("empty path")?
            .as_os_str()
            .to_string_lossy()
            .into_owned();
        let top_name = match renamed.iter().find(|(from, _)| *from == top) {
            Some((_, to)) => to.clone(),
            None => {
                let to = free_name(dir, &top);
                renamed.push((top.clone(), to.clone()));
                tops.push(dir.join(&to));
                to
            }
        };
        let target = dir.join(&top_name).join(parts.as_path());
        if item.dir {
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut file =
            std::fs::File::create(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        let mut offset = 0u64;
        while offset < item.size {
            if cancel.load(Ordering::Acquire) {
                drop(file);
                let _ = std::fs::remove_file(&target);
                return Err("cancelled".into());
            }
            let len = (item.size - offset).min(u64::from(CHUNK)) as u32;
            let request = Request::GetClipboardFileChunk {
                id,
                index: index as u32,
                offset,
                len,
            };
            let data = match connection::request(request).await? {
                Response::ClipboardChunk { data } => {
                    STANDARD.decode(data).map_err(|e| e.to_string())?
                }
                _ => return Err("unexpected answer".into()),
            };
            if data.is_empty() {
                return Err(format!("{} changed on the other PC", item.path));
            }
            file.write_all(&data).map_err(|e| e.to_string())?;
            offset += data.len() as u64;
            received += data.len() as u64;
            if let Some(pid) = progress {
                if last_shown.elapsed().as_millis() > 200 {
                    last_shown = std::time::Instant::now();
                    let fraction = received as f32 / info.total.max(1) as f32;
                    popups::update(
                        pid,
                        Some(fraction),
                        Some(format!(
                            "{} of {}",
                            size_text(received),
                            size_text(info.total)
                        )),
                    );
                }
            }
        }
    }
    Ok(tops)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_relative_paths_are_accepted() {
        assert!(safe_relative("photos/beach.jpg").is_some());
        assert!(safe_relative("report.pdf").is_some());
        assert!(safe_relative("../secret.txt").is_none());
        assert!(safe_relative("photos/../../x").is_none());
        assert!(safe_relative("C:/Windows/System32/x.dll").is_none());
        assert!(safe_relative("/etc/passwd").is_none());
        assert!(safe_relative("\\\\server\\share\\x").is_none());
        assert!(safe_relative("").is_none());
    }

    #[test]
    fn folders_are_listed_with_their_contents() {
        let root = std::env::temp_dir().join(format!("trovul-list-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("album/raw")).unwrap();
        std::fs::write(root.join("album/a.jpg"), vec![0u8; 1000]).unwrap();
        std::fs::write(root.join("album/raw/b.cr2"), vec![0u8; 2500]).unwrap();
        std::fs::write(root.join("notes.txt"), b"hi").unwrap();
        let (info, sources) = list(&[root.join("album"), root.join("notes.txt")]).unwrap();
        let paths: Vec<_> = info
            .items
            .iter()
            .map(|i| (i.path.as_str(), i.dir))
            .collect();
        assert_eq!(
            paths,
            vec![
                ("album", true),
                ("album/a.jpg", false),
                ("album/raw", true),
                ("album/raw/b.cr2", false),
                ("notes.txt", false)
            ]
        );
        assert_eq!(info.total, 3502);
        assert_eq!(sources.len(), 5);
        assert_eq!(info.describe(), "3 files · 4 KB");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn taken_names_get_a_number() {
        let root = std::env::temp_dir().join(format!("trovul-name-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(free_name(&root, "a.txt"), "a.txt");
        std::fs::write(root.join("a.txt"), b"x").unwrap();
        assert_eq!(free_name(&root, "a.txt"), "a (2).txt");
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod serving_tests {
    use super::*;

    #[test]
    fn only_offered_files_are_served_in_pieces() {
        let path =
            std::env::temp_dir().join(format!("trovul-serve-test-{}.bin", std::process::id()));
        let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        offered(77, vec![path.clone()]);
        let piece = |id, index, offset, len| match chunk(id, index, offset, len) {
            Response::ClipboardChunk { data } => Some(STANDARD.decode(data).unwrap()),
            _ => None,
        };
        assert_eq!(piece(77, 0, 0, 400).unwrap(), data[..400]);
        assert_eq!(piece(77, 0, 900, 400).unwrap(), data[900..]);
        assert_eq!(piece(77, 0, 5000, 10).unwrap(), Vec::<u8>::new());
        // Another offer id, or an index that wasn't offered: nothing.
        assert!(piece(78, 0, 0, 10).is_none());
        assert!(piece(77, 1, 0, 10).is_none());
        forget();
        assert!(piece(77, 0, 0, 10).is_none());
        let _ = std::fs::remove_file(&path);
    }
}
