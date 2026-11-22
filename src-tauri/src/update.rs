// Trovul - Updates over the link. Install a new Trovul on one PC and the
// linked PC is invited to take the same installer from it, with one click.
//
// - The MSI keeps a copy of itself in %ProgramData%\Trovul\updates (see
//   windows/firewall.wxs), so an installed Trovul can hand its installer on.
// - After linking, each PC asks the other for its version. When the other one
//   is newer and has its installer, the UI offers the update.
// - The offer is signed with the pairing token (HMAC-SHA256) and the file must
//   match the offered SHA-256: nobody else on the network can slip in an
//   installer. Installing still goes through Windows' admin prompt.
// - msiexec runs in passive mode and starts Trovul again when it's done; if
//   the install fails or is cancelled, a helper starts the old Trovul again.

use crate::connection;
use crate::peer::{Request, Response};
use crate::util::{constant_time_eq, hmac_sha256, lock, to_hex};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// Bytes per request: small enough not to hold up mouse events on the link.
const CHUNK: u32 = 256 * 1024;
/// Windows Installer packages are OLE compound files.
const MSI_SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallerOffer {
    pub size: u64,
    pub sha256: String,
    /// HMAC-SHA256(pairing token, "trovul-update|<version>|<sha256>").
    pub mac: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    #[default]
    Idle,
    Downloading,
    Installing,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct Available {
    pub version: String,
    /// Name of the PC that has it.
    pub from: String,
    pub size: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateState {
    pub current: String,
    /// The linked PC's Trovul version, when known.
    pub peer_version: Option<String>,
    /// A newer Trovul the linked PC can give us.
    pub available: Option<Available>,
    pub stage: Stage,
    /// 0–1 while downloading.
    pub progress: f32,
    pub error: Option<String>,
}

struct VerifiedOffer {
    peer_id: String,
    version: String,
    offer: InstallerOffer,
}

static STATE: Mutex<Option<UpdateState>> = Mutex::new(None);
static OFFER: Mutex<Option<VerifiedOffer>> = Mutex::new(None);
/// "Later": don't offer this version again until Trovul restarts.
static DISMISSED: Mutex<Option<String>> = Mutex::new(None);
/// Our own installer's SHA-256, cached until the file changes.
struct CachedHash {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
    sha256: String,
}

static OWN: Mutex<Option<CachedHash>> = Mutex::new(None);

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn state() -> UpdateState {
    lock(&STATE).clone().unwrap_or_else(|| UpdateState {
        current: version().into(),
        ..Default::default()
    })
}

fn change(f: impl FnOnce(&mut UpdateState)) {
    {
        let mut slot = lock(&STATE);
        let state = slot.get_or_insert_with(|| UpdateState {
            current: version().into(),
            ..Default::default()
        });
        f(state);
    }
    crate::on_update_changed();
}

/// "0.7.1" > "0.7.0"; a pre-release suffix ("-beta.2") is ignored.
pub fn newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.split('-')
            .next()
            .unwrap_or("")
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    parse(candidate) > parse(current)
}

fn offer_mac(token: &str, version: &str, sha256: &str) -> String {
    to_hex(&hmac_sha256(
        token.as_bytes(),
        format!("trovul-update|{version}|{sha256}").as_bytes(),
    ))
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

/// True when the file's owner is SYSTEM or Administrators. Standard users can
/// create files under %ProgramData%; one of those must never be passed on as
/// "our installer" (the other PC would install it as admin).
fn owned_by_admins(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows::Win32::Security::{
        IsWellKnownSid, WinBuiltinAdministratorsSid, WinLocalSystemSid, OWNER_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, PSID,
    };
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut owner = PSID::default();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe {
        let error = GetNamedSecurityInfoW(
            PCWSTR(wide.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            &mut descriptor,
        );
        if error.0 != 0 {
            return false;
        }
        let trusted = IsWellKnownSid(owner, WinLocalSystemSid).as_bool()
            || IsWellKnownSid(owner, WinBuiltinAdministratorsSid).as_bool();
        let _ = LocalFree(HLOCAL(descriptor.0));
        trusted
    }
}

/// The installer this Trovul was installed from, if the MSI kept a copy.
fn own_installer() -> Option<(PathBuf, u64, String)> {
    let dir = PathBuf::from(std::env::var_os("ProgramData")?)
        .join("Trovul")
        .join("updates");
    let path = dir.join(format!("Trovul_{}.msi", version()));
    if !owned_by_admins(&dir) || !owned_by_admins(&path) {
        return None;
    }
    let meta = std::fs::metadata(&path).ok()?;
    let modified = meta.modified().ok();
    let mut cache = lock(&OWN);
    if let Some(c) = cache.as_ref() {
        if c.path == path && c.size == meta.len() && c.modified == modified {
            return Some((path, meta.len(), c.sha256.clone()));
        }
    }
    let sha256 = sha256_file(&path).ok()?;
    *cache = Some(CachedHash {
        path: path.clone(),
        size: meta.len(),
        modified,
        sha256: sha256.clone(),
    });
    Some((path, meta.len(), sha256))
}

// ---------- Serving the linked PC ----------

/// Request::GetUpdateInfo: our version, and our installer if we can share it.
pub fn info_for(peer_id: &str) -> Response {
    let installer = own_installer().and_then(|(_, size, sha256)| {
        let token = connection::pairing_token(peer_id)?;
        Some(InstallerOffer {
            size,
            mac: offer_mac(&token, version(), &sha256),
            sha256,
        })
    });
    Response::UpdateInfo {
        version: version().into(),
        installer,
    }
}

/// Request::GetUpdateChunk: a piece of our installer.
pub fn chunk(offset: u64, len: u32) -> Response {
    let read = || -> Result<String, String> {
        let (path, size, _) = own_installer().ok_or("No installer to share")?;
        if offset >= size {
            return Err("Past the end of the installer".into());
        }
        let len = (len.min(CHUNK) as u64).min(size - offset) as usize;
        let mut file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; len];
        file.read_exact(&mut buf).map_err(|e| e.to_string())?;
        Ok(STANDARD.encode(buf))
    };
    match read() {
        Ok(data) => Response::UpdateChunk { data },
        Err(message) => Response::Error { message },
    }
}

// ---------- Taking an update from the linked PC ----------

/// After linking: is the other PC's Trovul newer, and can it give it to us?
pub async fn check() {
    let Some(link) = connection::current() else {
        return;
    };
    let peer_id = link.info().peer_device_id.clone();
    let peer_name = link.info().peer_hostname.clone();
    let (peer_version, installer) = match connection::request(Request::GetUpdateInfo).await {
        Ok(Response::UpdateInfo { version, installer }) => (version, installer),
        // An older Trovul doesn't know the request.
        _ => {
            change(|s| s.peer_version = None);
            return;
        }
    };

    let verified = installer.filter(|offer| {
        newer(&peer_version, version())
            && connection::pairing_token(&peer_id).is_some_and(|token| {
                let expected = offer_mac(&token, &peer_version, &offer.sha256);
                constant_time_eq(expected.as_bytes(), offer.mac.as_bytes())
            })
    });
    let dismissed = lock(&DISMISSED).as_deref() == Some(peer_version.as_str());
    let available = verified
        .as_ref()
        .filter(|_| !dismissed)
        .map(|offer| Available {
            version: peer_version.clone(),
            from: peer_name.clone(),
            size: offer.size,
        });
    if let Some(offer) = &available {
        log::info!(
            "{} has Trovul {} ({} bytes) to share",
            offer.from,
            offer.version,
            offer.size
        );
    }
    *lock(&OFFER) = verified.map(|offer| VerifiedOffer {
        peer_id,
        version: peer_version.clone(),
        offer,
    });
    change(|s| {
        s.peer_version = Some(peer_version);
        if s.stage == Stage::Idle || s.stage == Stage::Failed {
            s.available = available;
        }
    });
}

pub fn link_closed() {
    if matches!(state().stage, Stage::Downloading | Stage::Installing) {
        return;
    }
    *lock(&OFFER) = None;
    change(|s| {
        s.available = None;
        s.peer_version = None;
    });
}

/// "Later".
pub fn dismiss() {
    let version = state().available.map(|a| a.version);
    *lock(&DISMISSED) = version;
    change(|s| {
        s.available = None;
        s.error = None;
        s.stage = Stage::Idle;
    });
}

fn fail(message: String) -> Result<(), String> {
    log::warn!("Update failed: {message}");
    change(|s| {
        s.stage = Stage::Failed;
        s.error = Some(message.clone());
    });
    Err(message)
}

/// Download the linked PC's installer, check it, and run it. Trovul quits so
/// the installer can replace it, and starts again afterwards.
pub async fn install(download_dir: PathBuf) -> Result<(), String> {
    let (peer_id, version, offer) = match lock(&OFFER).as_ref() {
        Some(o) => (o.peer_id.clone(), o.version.clone(), o.offer.clone()),
        None => return Err("No update to install".into()),
    };
    if matches!(state().stage, Stage::Downloading | Stage::Installing) {
        return Err("The update is already running".into());
    }
    change(|s| {
        s.stage = Stage::Downloading;
        s.progress = 0.0;
        s.error = None;
    });

    let mut data: Vec<u8> = Vec::with_capacity(offer.size as usize);
    while (data.len() as u64) < offer.size {
        if connection::current().is_none_or(|l| l.info().peer_device_id != peer_id) {
            return fail("The other PC disconnected during the download".into());
        }
        let request = Request::GetUpdateChunk {
            offset: data.len() as u64,
            len: CHUNK,
        };
        let piece = match connection::request(request).await {
            Ok(Response::UpdateChunk { data }) => STANDARD.decode(data).map_err(|e| e.to_string()),
            Ok(_) => Err("Unexpected answer from the other PC".into()),
            Err(e) => Err(e),
        };
        match piece {
            Ok(bytes) if !bytes.is_empty() => data.extend_from_slice(&bytes),
            Ok(_) => return fail("The other PC sent an empty piece".into()),
            Err(e) => return fail(format!("Download failed: {e}")),
        }
        let progress = data.len() as f32 / offer.size.max(1) as f32;
        change(|s| s.progress = progress.min(1.0));
    }

    let actual = to_hex(&Sha256::digest(&data));
    if data.len() as u64 != offer.size
        || !constant_time_eq(actual.as_bytes(), offer.sha256.as_bytes())
    {
        return fail("The installer didn't arrive intact (checksum mismatch)".into());
    }
    if !data.starts_with(&MSI_SIGNATURE) {
        return fail("What arrived isn't a Windows installer".into());
    }

    let _ = std::fs::create_dir_all(&download_dir);
    let msi = download_dir.join(format!("Trovul_{version}.msi"));
    if let Err(e) = std::fs::write(&msi, &data) {
        return fail(format!("Could not save the installer: {e}"));
    }
    change(|s| {
        s.stage = Stage::Installing;
        s.progress = 1.0;
    });
    if let Err(e) = run_installer(&msi, &download_dir.join("install.log")) {
        return fail(format!("Could not start the installer: {e}"));
    }
    log::info!(
        "Installing Trovul {version} from the linked PC; quitting so it can replace this one"
    );
    crate::quit_for_update();
    Ok(())
}

/// msiexec in passive mode (it asks for admin rights itself) with a hidden
/// PowerShell watching it: if the install fails or is cancelled, the current
/// Trovul starts again. On success the MSI starts the new one (AUTOLAUNCHAPP).
fn run_installer(msi: &Path, log_file: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let quote = |p: &Path| p.display().to_string().replace('\'', "''");
    let exe = std::env::current_exe()?;
    let script = format!(
        "$p = Start-Process -FilePath msiexec.exe -ArgumentList '/i \"{msi}\" /passive AUTOLAUNCHAPP=1 /l*v \"{log}\"' -Wait -PassThru; \
         if ($p.ExitCode -ne 0 -and $p.ExitCode -ne 3010) {{ Start-Process -FilePath '{exe}' }}",
        msi = quote(msi),
        log = quote(log_file),
        exe = quote(&exe),
    );
    std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
}

/// Installers downloaded for an earlier update aren't needed any more.
pub fn clean_downloads(download_dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(download_dir) {
        for entry in entries.flatten() {
            if entry.path().extension().is_some_and(|e| e == "msi") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(newer("0.7.1", "0.7.0"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(newer("1.0.0", "0.99.0"));
        assert!(!newer("0.7.0", "0.7.0"));
        assert!(!newer("0.6.9", "0.7.0"));
        assert!(!newer("0.7.0-beta.2", "0.7.0"));
    }

    #[test]
    fn offers_are_bound_to_the_token_version_and_file() {
        let mac = offer_mac("token-a", "0.7.1", "abc");
        assert_eq!(mac, offer_mac("token-a", "0.7.1", "abc"));
        assert_ne!(mac, offer_mac("token-b", "0.7.1", "abc"));
        assert_ne!(mac, offer_mac("token-a", "0.7.2", "abc"));
        assert_ne!(mac, offer_mac("token-a", "0.7.1", "abd"));
    }
}
