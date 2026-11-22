// Trovul - The input helper: keyboard & mouse sharing in admin windows, UAC
// prompts and on the lock screen.
//
// Windows drops input that a normal app injects into windows running as
// administrator (UIPI), and the UAC prompt and the lock screen are on a
// separate, protected desktop no normal app reaches. So, as Mouse Without
// Borders does with its "Use service" option:
//
// - The MSI registers a service, TrovulInput (LocalSystem, manual start, and
//   the signed-in user may start and stop it). It stays stopped unless the
//   user turns the option on; the app then starts it.
// - The service (`trovul.exe --input-service`, session 0) starts
//   `trovul.exe --input-helper` in the user's session as SYSTEM, and again
//   after a session change or if it exits.
// - The helper takes the other PC's input from the app over a named pipe and
//   injects it on whichever desktop has the input: the normal one, a UAC
//   prompt or the lock screen. It only talks to this same trovul.exe.
//
// The trade-off, shown next to the switch: while it's on, a program running
// as this user could use the helper to answer UAC prompts too.

use crate::input::InputEvent;
use crate::settings;
use crate::util::{lock, mono_ms};
use std::ffi::c_void;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::FromRawHandle;
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::sync::Mutex;
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_PIPE_CONNECTED, ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_DOES_NOT_EXIST,
    HANDLE, INVALID_HANDLE_VALUE, NO_ERROR, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    DuplicateTokenEx, SecurityImpersonation, SetTokenInformation, TokenPrimary, TokenSessionId,
    PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_ALL_ACCESS,
};
use windows::Win32::Storage::FileSystem::PIPE_ACCESS_INBOUND;
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId};
use windows::Win32::System::Services::{
    CloseServiceHandle, ControlService, OpenSCManagerW, OpenServiceW, QueryServiceStatus,
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW, StartServiceW,
    SC_MANAGER_CONNECT, SERVICE_ACCEPT_SESSIONCHANGE, SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP,
    SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_SESSIONCHANGE, SERVICE_CONTROL_SHUTDOWN,
    SERVICE_CONTROL_STOP, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START,
    SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STATUS_HANDLE,
    SERVICE_STOP, SERVICE_STOPPED, SERVICE_STOP_PENDING, SERVICE_TABLE_ENTRYW,
    SERVICE_WIN32_OWN_PROCESS,
};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, OpenInputDesktop, SetThreadDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS,
    HDESK,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateProcessAsUserW, GetCurrentProcess, GetCurrentProcessId, OpenProcess,
    OpenProcessToken, QueryFullProcessImageNameW, SetEvent, TerminateProcess,
    WaitForMultipleObjects, CREATE_NO_WINDOW, PROCESS_INFORMATION, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
};

const SERVICE_NAME: PCWSTR = w!("TrovulInput");
const PIPE_NAME: PCWSTR = w!(r"\\.\pipe\TrovulInput");
const PIPE_PATH: &str = r"\\.\pipe\TrovulInput";
/// SYSTEM: full access; interactive users: read/write (the app connects).
const PIPE_SDDL: PCWSTR = w!("D:P(A;;GA;;;SY)(A;;GRGW;;;IU)");

// ============ In the app: the switch and the client ============

/// The helper as the app sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperState {
    /// The switch is off.
    Off,
    /// On, and input goes through the helper.
    Ready,
    /// On, the service is starting or the helper isn't there yet.
    Starting,
    /// This install has no service (the per-user installer, or a dev build).
    NotInstalled,
    /// Windows refused to start it (e.g. an older install without the rights).
    Failed,
}

static STATE: Mutex<HelperState> = Mutex::new(HelperState::Off);
static CLIENT: Mutex<Option<std::fs::File>> = Mutex::new(None);
/// When the app last tried to reach the helper (it retries every 2 s).
static LAST_TRY: AtomicU64 = AtomicU64::new(0);

pub fn state() -> HelperState {
    *lock(&STATE)
}

fn set_state(state: HelperState) {
    let changed = std::mem::replace(&mut *lock(&STATE), state) != state;
    if changed {
        crate::on_input_changed();
    }
}

/// Turn the option on or off (and start or stop the service).
pub fn set_enabled(enabled: bool) {
    settings::update(|s| s.admin_input = enabled);
    apply();
}

/// Bring the service in line with the setting (app start, option changed).
pub fn apply() {
    let enabled = settings::get().admin_input;
    std::thread::spawn(move || {
        if enabled {
            set_state(HelperState::Starting);
            let state = match control(true) {
                Ok(()) => {
                    LAST_TRY.store(0, Ordering::Relaxed);
                    if connect() {
                        HelperState::Ready
                    } else {
                        HelperState::Starting
                    }
                }
                Err(state) => state,
            };
            set_state(state);
        } else {
            lock(&CLIENT).take();
            let _ = control(false);
            set_state(HelperState::Off);
        }
    });
}

/// Start or stop the service.
fn control(start: bool) -> Result<(), HelperState> {
    unsafe {
        let manager = OpenSCManagerW(None, None, SC_MANAGER_CONNECT).map_err(|e| {
            log::warn!("Input helper: service manager unavailable: {e}");
            HelperState::Failed
        })?;
        let access = SERVICE_QUERY_STATUS | if start { SERVICE_START } else { SERVICE_STOP };
        let service = match OpenServiceW(manager, SERVICE_NAME, access) {
            Ok(service) => service,
            Err(e) => {
                let _ = CloseServiceHandle(manager);
                return Err(if e.code() == ERROR_SERVICE_DOES_NOT_EXIST.to_hresult() {
                    HelperState::NotInstalled
                } else {
                    log::warn!("Input helper: can't open the service: {e}");
                    HelperState::Failed
                });
            }
        };
        let result = if start {
            match StartServiceW(service, None) {
                Ok(()) => Ok(()),
                Err(e) if e.code() == ERROR_SERVICE_ALREADY_RUNNING.to_hresult() => Ok(()),
                Err(e) => {
                    log::warn!("Input helper: can't start the service: {e}");
                    Err(HelperState::Failed)
                }
            }
        } else {
            let mut status = SERVICE_STATUS::default();
            let _ = QueryServiceStatus(service, &mut status);
            if status.dwCurrentState != SERVICE_STOPPED {
                let _ = ControlService(service, SERVICE_CONTROL_STOP, &mut status);
            }
            Ok(())
        };
        let _ = CloseServiceHandle(service);
        let _ = CloseServiceHandle(manager);
        result
    }
}

/// Open the pipe to the helper; the service may need a moment to start it.
fn connect() -> bool {
    for _ in 0..20 {
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(PIPE_PATH) {
            *lock(&CLIENT) = Some(file);
            log::info!("Input helper: connected (admin windows, UAC and the lock screen)");
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    false
}

/// Replay `event` through the helper. False = not available (inject directly).
pub fn forward(event: &InputEvent) -> bool {
    if !matches!(state(), HelperState::Ready | HelperState::Starting) {
        return false;
    }
    let mut client = lock(&CLIENT);
    if client.is_none() {
        // Reconnect at most every 2 s; meanwhile input is injected directly.
        let now = mono_ms();
        if now.saturating_sub(LAST_TRY.swap(now, Ordering::Relaxed)) < 2000 {
            return false;
        }
        match std::fs::OpenOptions::new().write(true).open(PIPE_PATH) {
            Ok(file) => {
                *client = Some(file);
                drop(client);
                set_state(HelperState::Ready);
                client = lock(&CLIENT);
            }
            Err(_) => return false,
        }
    }
    let Some(file) = client.as_mut() else {
        return false;
    };
    let mut line = match serde_json::to_vec(event) {
        Ok(line) => line,
        Err(_) => return false,
    };
    line.push(b'\n');
    if file.write_all(&line).is_ok() {
        true
    } else {
        client.take();
        drop(client);
        set_state(HelperState::Starting);
        false
    }
}

// ============ The service (session 0) ============

static STATUS_HANDLE: AtomicIsize = AtomicIsize::new(0);
static STOP_EVENT: AtomicIsize = AtomicIsize::new(0);
static SESSION_EVENT: AtomicIsize = AtomicIsize::new(0);

/// Entry point for `trovul.exe --input-service` (started by the service manager).
pub fn run_service() {
    let mut name: Vec<u16> = "TrovulInput\0".encode_utf16().collect();
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR(name.as_mut_ptr()),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    unsafe {
        if let Err(e) = StartServiceCtrlDispatcherW(table.as_ptr()) {
            note(&format!("not started by the service manager: {e}"));
        }
    }
}

fn report(state: SERVICE_STATUS_CURRENT_STATE, wait_hint: u32) {
    let handle = SERVICE_STATUS_HANDLE(STATUS_HANDLE.load(Ordering::Acquire) as *mut c_void);
    let accepted = if state == SERVICE_RUNNING {
        SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN | SERVICE_ACCEPT_SESSIONCHANGE
    } else {
        0
    };
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: accepted,
        dwWin32ExitCode: NO_ERROR.0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: wait_hint,
    };
    unsafe {
        let _ = SetServiceStatus(handle, &status);
    }
}

unsafe extern "system" fn control_handler(
    control: u32,
    _event: u32,
    _data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    let signal = |slot: &AtomicIsize| {
        let _ = SetEvent(HANDLE(slot.load(Ordering::Acquire) as *mut c_void));
    };
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            report(SERVICE_STOP_PENDING, 3000);
            signal(&STOP_EVENT);
            NO_ERROR.0
        }
        SERVICE_CONTROL_SESSIONCHANGE => {
            signal(&SESSION_EVENT);
            NO_ERROR.0
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR.0,
        _ => 120, // ERROR_CALL_NOT_IMPLEMENTED
    }
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let Ok(handle) = RegisterServiceCtrlHandlerExW(SERVICE_NAME, Some(control_handler), None)
    else {
        return;
    };
    STATUS_HANDLE.store(handle.0 as isize, Ordering::Release);
    report(SERVICE_START_PENDING, 3000);
    let (Ok(stop), Ok(session)) = (
        CreateEventW(None, true, false, None),
        CreateEventW(None, false, false, None),
    ) else {
        report(SERVICE_STOPPED, 0);
        return;
    };
    STOP_EVENT.store(stop.0 as isize, Ordering::Release);
    SESSION_EVENT.store(session.0 as isize, Ordering::Release);
    report(SERVICE_RUNNING, 0);
    note("service started");

    let mut helper: Option<(HANDLE, u32)> = None;
    loop {
        let console = WTSGetActiveConsoleSessionId();
        // (Re)start the helper in the session at the console.
        let stale = helper.is_some_and(|(_, s)| s != console);
        if stale {
            if let Some((process, _)) = helper.take() {
                let _ = TerminateProcess(process, 0);
                let _ = CloseHandle(process);
            }
        }
        if helper.is_none() && console != u32::MAX {
            match launch_helper(console) {
                Ok(process) => helper = Some((process, console)),
                Err(e) => note(&format!(
                    "could not start the helper in session {console}: {e}"
                )),
            }
        }
        let mut handles = vec![stop, session];
        if let Some((process, _)) = helper {
            handles.push(process);
        }
        let woke = WaitForMultipleObjects(&handles, false, 5000);
        if woke == WAIT_OBJECT_0 {
            break;
        }
        if woke.0 == WAIT_OBJECT_0.0 + 2 {
            // The helper exited: start it again (after a breath, if it keeps dying).
            if let Some((process, _)) = helper.take() {
                let _ = CloseHandle(process);
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }
    if let Some((process, _)) = helper.take() {
        let _ = TerminateProcess(process, 0);
        let _ = CloseHandle(process);
    }
    note("service stopped");
    report(SERVICE_STOPPED, 0);
}

fn own_exe() -> Option<String> {
    std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Start `trovul.exe --input-helper` in `session`, as SYSTEM.
unsafe fn launch_helper(session: u32) -> windows::core::Result<HANDLE> {
    let exe = own_exe().ok_or_else(windows::core::Error::from_win32)?;
    let mut token = HANDLE::default();
    OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut token)?;
    let mut primary = HANDLE::default();
    let duplicated = DuplicateTokenEx(
        token,
        TOKEN_ALL_ACCESS,
        None,
        SecurityImpersonation,
        TokenPrimary,
        &mut primary,
    );
    let _ = CloseHandle(token);
    duplicated?;
    let result = (|| {
        SetTokenInformation(
            primary,
            TokenSessionId,
            &session as *const u32 as *const c_void,
            std::mem::size_of::<u32>() as u32,
        )?;
        let mut command: Vec<u16> = format!("\"{exe}\" --input-helper\0")
            .encode_utf16()
            .collect();
        let mut desktop: Vec<u16> = "winsta0\\default\0".encode_utf16().collect();
        let startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop.as_mut_ptr()),
            ..Default::default()
        };
        let mut info = PROCESS_INFORMATION::default();
        CreateProcessAsUserW(
            primary,
            None,
            PWSTR(command.as_mut_ptr()),
            None,
            None,
            false,
            CREATE_NO_WINDOW,
            None,
            None,
            &startup,
            &mut info,
        )?;
        let _ = CloseHandle(info.hThread);
        note(&format!("helper started in session {session}"));
        Ok(info.hProcess)
    })();
    let _ = CloseHandle(primary);
    result
}

// ============ The helper (user session, SYSTEM) ============

/// Entry point for `trovul.exe --input-helper` (started by the service).
pub fn run_helper() {
    crate::input::monitors::physical_pixels();
    let Some(me) = own_exe() else {
        return;
    };
    let my_session = session_of(unsafe { GetCurrentProcessId() });
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PIPE_SDDL,
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .is_err()
    {
        note("helper: bad pipe security descriptor");
        return;
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let mut desktop = InputDesktop::default();
    loop {
        let pipe = unsafe {
            CreateNamedPipeW(
                PIPE_NAME,
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                0,
                64 * 1024,
                0,
                Some(&attributes),
            )
        };
        if pipe == INVALID_HANDLE_VALUE {
            note("helper: could not create the pipe");
            std::thread::sleep(std::time::Duration::from_secs(2));
            continue;
        }
        let connected = match unsafe { ConnectNamedPipe(pipe, None) } {
            Ok(()) => true,
            Err(e) => e.code() == ERROR_PIPE_CONNECTED.to_hresult(),
        };
        if connected && client_is_trovul(pipe, &me, my_session) {
            // SAFETY: the handle is ours; File closes it when dropped.
            let file = unsafe { std::fs::File::from_raw_handle(pipe.0) };
            serve(file, &mut desktop);
            // The app left (or quit): nothing stays pressed.
            desktop.follow();
            crate::input::replay_here(&InputEvent::Leave);
        } else {
            unsafe {
                let _ = DisconnectNamedPipe(pipe);
                let _ = CloseHandle(pipe);
            }
        }
    }
}

fn serve(file: std::fs::File, desktop: &mut InputDesktop) {
    let reader = BufReader::new(file);
    for line in reader.split(b'\n') {
        let Ok(line) = line else {
            return;
        };
        if let Ok(event) = serde_json::from_slice::<InputEvent>(&line) {
            // The UAC prompt or the lock screen may have taken the input.
            desktop.follow();
            crate::input::replay_here(&event);
        }
    }
}

fn session_of(pid: u32) -> Option<u32> {
    let mut session = 0u32;
    unsafe { ProcessIdToSessionId(pid, &mut session) }
        .ok()
        .map(|_| session)
}

/// Only this same trovul.exe, in this session, may drive the helper.
fn client_is_trovul(pipe: HANDLE, me: &str, my_session: Option<u32>) -> bool {
    let mut pid = 0u32;
    if unsafe { GetNamedPipeClientProcessId(pipe, &mut pid) }.is_err() {
        return false;
    }
    let path = unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(process);
        if !ok {
            return false;
        }
        String::from_utf16_lossy(&buf[..len as usize])
    };
    let same_exe = path.eq_ignore_ascii_case(me);
    let same_session = my_session.is_some() && session_of(pid) == my_session;
    if !(same_exe && same_session) {
        note(&format!("helper: refused a client ({path})"));
    }
    same_exe && same_session
}

/// The desktop that has the input, attached to this thread before injecting.
#[derive(Default)]
struct InputDesktop {
    current: Option<HDESK>,
}

impl InputDesktop {
    fn follow(&mut self) {
        const GENERIC_ALL: u32 = 0x1000_0000;
        unsafe {
            let Ok(desk) = OpenInputDesktop(
                DESKTOP_CONTROL_FLAGS(0),
                false,
                DESKTOP_ACCESS_FLAGS(GENERIC_ALL),
            ) else {
                return;
            };
            if SetThreadDesktop(desk).is_ok() {
                if let Some(old) = self.current.replace(desk) {
                    let _ = CloseDesktop(old);
                }
            } else {
                let _ = CloseDesktop(desk);
            }
        }
    }
}

// ============ A small log (the service and helper have no app log) ============

static NOTE_LOCK: Mutex<()> = Mutex::new(());

/// Append to %ProgramData%\Trovul\input-helper.log (kept under 256 KB).
fn note(message: &str) {
    let _guard = lock(&NOTE_LOCK);
    let Some(dir) =
        std::env::var_os("ProgramData").map(|d| std::path::PathBuf::from(d).join("Trovul"))
    else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("input-helper.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 256 * 1024) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(
            file,
            "[{}] [{}] {message}",
            crate::util::now_ms(),
            std::process::id()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, MapVirtualKeyW, MAPVK_VK_TO_VSC, VK_F24,
    };

    /// The helper's pipe end to end, without the service: this test process is
    /// both the helper (on a thread) and its client, so the "same exe" check
    /// passes. F24 goes down and up through the pipe. `cargo test -- --ignored hardware`.
    #[test]
    #[ignore]
    fn hardware_helper_pipe_injects() {
        std::thread::spawn(run_helper);
        let mut client = None;
        for _ in 0..40 {
            if let Ok(file) = std::fs::OpenOptions::new().write(true).open(PIPE_PATH) {
                client = Some(file);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let mut client = client.expect("the helper's pipe never opened");
        let vk = VK_F24.0;
        let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
        let down = || unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 };
        let send = |file: &mut std::fs::File, event: InputEvent| {
            let mut line = serde_json::to_vec(&event).unwrap();
            line.push(b'\n');
            file.write_all(&line).unwrap();
        };
        send(
            &mut client,
            InputEvent::Key {
                vk,
                scan,
                extended: false,
                down: true,
            },
        );
        std::thread::sleep(std::time::Duration::from_millis(150));
        let was_down = down();
        send(
            &mut client,
            InputEvent::Key {
                vk,
                scan,
                extended: false,
                down: false,
            },
        );
        std::thread::sleep(std::time::Duration::from_millis(150));
        let released = !down();
        println!("through the helper: F24 down {was_down}, released {released}");
        assert!(was_down && released);
    }
}
