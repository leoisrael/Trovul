// Trovul - Follow the pointer: in the headset, the PC you're using is louder.
//
// Only on the PC that plays the other PC's sound, and only while keyboard &
// mouse sharing works both ways (otherwise the pointer never moves between
// the PCs). With the pointer on this PC, the other PC's sound drops to
// `follow_depth`; with it on the other PC, this PC's apps drop instead.
//
// This PC's apps are lowered through each session's channel volumes, a
// multiplier Windows keeps apart from the volume the mixer's dials show, so
// the dials never jump. If Trovul stops while they're lowered, the next start
// puts them back (`settings.focus_lowered`).

use crate::audio::{self, Mode};
use crate::{audio_sessions, input, settings};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static RUNNING: AtomicBool = AtomicBool::new(false);
/// Pointer here (Some(true)) or on the other PC, while the feature is acting.
static STATE: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);

const TICK: Duration = Duration::from_millis(100);
/// This PC's apps fade in steps (each step writes every session).
const APP_STEPS: u32 = 5;

/// Where the pointer is, while the feature is acting (for the mixer's tags).
pub fn state() -> Option<bool> {
    *crate::util::lock(&STATE)
}

/// Start the watcher (idempotent); it idles when the feature doesn't apply.
pub fn start() {
    // An earlier run may have left this PC's apps lowered.
    if settings::get().focus_lowered {
        audio_sessions::set_apps_level(1.0);
        settings::update(|s| s.focus_lowered = false);
    }
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("trovul-focus-mix".into())
        .spawn(run);
}

fn run() {
    crate::com::ensure_initialized();
    let mut apps_level = 1.0f32;
    let mut last_rescan = std::time::Instant::now();
    loop {
        std::thread::sleep(TICK);
        let s = settings::get();
        let acting = s.follow_pointer && audio::mode() == Mode::Host && input::remote_ready();
        let depth = s.follow_depth.clamp(0.0, 1.0);
        let here = input::pointer_here();
        *crate::util::lock(&STATE) = acting.then_some(here);

        // The other PC's sound: the renderer glides to the new gain.
        audio::set_focus(if acting && here { depth } else { 1.0 });

        // This PC's apps.
        let target = if acting && !here { depth } else { 1.0 };
        if (target - apps_level).abs() > 0.001 {
            if (target - 1.0).abs() > 0.001 && !s.focus_lowered {
                settings::update(|s| s.focus_lowered = true);
            }
            let from = apps_level;
            for step in 1..=APP_STEPS {
                let level = from + (target - from) * step as f32 / APP_STEPS as f32;
                audio_sessions::set_apps_level(level);
                std::thread::sleep(Duration::from_millis(30));
            }
            apps_level = target;
            if (target - 1.0).abs() <= 0.001 {
                settings::update(|s| s.focus_lowered = false);
            }
            last_rescan = std::time::Instant::now();
        } else if apps_level < 0.999 && last_rescan.elapsed() > Duration::from_secs(1) {
            // Apps that started playing meanwhile get lowered too.
            audio_sessions::set_apps_level(apps_level);
            last_rescan = std::time::Instant::now();
        }
    }
}
