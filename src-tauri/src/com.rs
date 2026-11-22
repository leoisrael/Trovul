// Trovul - Per-thread COM initialisation.
//
// COM must be initialised on every thread that touches Core Audio. Async Tauri
// commands run on the tokio pool, sync ones on the main (STA) thread, and the
// audio engine and control link have threads of their own, so initialisation
// happens lazily per thread rather than once per process.

use std::cell::Cell;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

thread_local! {
    static COM_READY: Cell<bool> = const { Cell::new(false) };
}

/// Initialise COM (MTA) on the current thread if that hasn't happened yet.
/// Never uninitialises, so it is safe on pool threads we don't own. On a thread
/// that is already an STA the call fails with RPC_E_CHANGED_MODE and COM keeps
/// working in that apartment.
pub fn ensure_initialized() {
    COM_READY.with(|ready| {
        if !ready.get() {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            ready.set(true);
        }
    });
}

/// RAII guard for threads we own: initialises COM on creation and balances it
/// on drop. Declare it before any COM object so it is dropped after them.
pub struct ComGuard {
    initialized: bool,
}

impl ComGuard {
    pub fn new() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        // S_OK and S_FALSE both need a matching CoUninitialize; failures don't.
        Self {
            initialized: hr.is_ok(),
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { CoUninitialize() };
        }
    }
}
