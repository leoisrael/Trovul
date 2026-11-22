// Trovul - Voice-chat ducking. While a call app on this PC (Discord, Teams,
// Zoom, …) plays someone's voice, the audio coming from the other PC drops to
// DUCKED_GAIN, and it comes back once the call has been quiet for a moment.
// Only acts while this PC receives audio; can be turned off in Settings.

use crate::audio::{self, Mode};
use crate::audio_sessions::AppMeters;
use crate::com::ComGuard;
use crate::util::lock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Call apps, by lower-case exe name without ".exe".
const VOICE_APPS: &[&str] = &[
    "discord",
    "discordptb",
    "discordcanary",
    "vesktop",
    "teams",
    "ms-teams",
    "lync",
    "zoom",
    "skype",
    "slack",
    "webex",
    "ciscocollabhost",
    "whatsapp",
    "telegram",
    "signal",
    "ts3client_win64",
    "ts3client_win32",
    "teamspeak",
    "mumble",
    "guilded",
    "element",
];

pub fn is_voice_app(exe: &str) -> bool {
    VOICE_APPS.contains(&exe)
}

const TICK_MS: u64 = 50;
/// A call app peaking above this counts as someone talking (≈ −34 dBFS).
const TALK_THRESHOLD: f32 = 0.02;
/// Gain on the other PC's audio while someone talks (≈ −10 dB).
pub const DUCKED_GAIN: f32 = 0.3;
const ATTACK_MS: f32 = 150.0;
/// Pauses between words shorter than this keep the audio lowered.
const HOLD_MS: f32 = 600.0;
const RELEASE_MS: f32 = 900.0;
/// Look for new call-app sessions this often (a scan costs a few ms).
const RESCAN_TICKS: u32 = 20;
/// PIDs get reused, so the per-PID "is it a call app" answers expire.
const FORGET_TICKS: u32 = 1_200;

/// Gain envelope: down quickly when a voice starts, steady through short
/// pauses, then slowly back up.
#[derive(Debug)]
pub struct Envelope {
    gain: f32,
    quiet_ms: f32,
}

impl Default for Envelope {
    fn default() -> Self {
        Self {
            gain: 1.0,
            quiet_ms: f32::INFINITY,
        }
    }
}

impl Envelope {
    /// Advance by `dt_ms` and return the gain to apply.
    pub fn step(&mut self, talking: bool, dt_ms: f32) -> f32 {
        self.quiet_ms = if talking { 0.0 } else { self.quiet_ms + dt_ms };
        let (target, ramp_ms) = if self.quiet_ms < HOLD_MS {
            (DUCKED_GAIN, ATTACK_MS)
        } else {
            (1.0, RELEASE_MS)
        };
        let step = (1.0 - DUCKED_GAIN) * dt_ms / ramp_ms;
        self.gain = if self.gain > target {
            (self.gain - step).max(target)
        } else {
            (self.gain + step).min(target)
        };
        self.gain
    }
}

struct Monitor {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

static MONITOR: Mutex<Option<Monitor>> = Mutex::new(None);

/// Start or stop watching call apps to match the setting (idempotent).
pub fn apply(enabled: bool) {
    let mut monitor = lock(&MONITOR);
    if enabled == monitor.is_some() {
        return;
    }
    if let Some(Monitor { stop, thread }) = monitor.take() {
        stop.store(true, Ordering::Relaxed);
        let _ = thread.join();
        audio::set_duck(1.0);
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    match std::thread::Builder::new()
        .name("trovul-ducking".into())
        .spawn(move || run(&flag))
    {
        Ok(thread) => *monitor = Some(Monitor { stop, thread }),
        Err(e) => log::warn!("Voice-chat ducking unavailable: {e}"),
    }
}

fn run(stop: &AtomicBool) {
    let _com = ComGuard::new();
    let mut envelope = Envelope::default();
    let mut meters = AppMeters::default();
    let mut applied = 1.0f32;
    let mut ticks = 0u32;
    let mut reported = false;
    let mut watched = 0;

    while !stop.load(Ordering::Relaxed) {
        let gain = if audio::mode() == Mode::Host {
            if ticks.is_multiple_of(FORGET_TICKS) {
                meters.forget_processes();
            }
            if ticks.is_multiple_of(RESCAN_TICKS) {
                if let Err(e) = meters.rescan(is_voice_app) {
                    if !reported {
                        log::warn!("Voice-chat ducking can't see app levels: {e}");
                        reported = true;
                    }
                }
                if meters.sessions() != watched {
                    watched = meters.sessions();
                    log::info!("Voice-chat ducking: {watched} call-app audio session(s)");
                }
            }
            ticks = ticks.wrapping_add(1);
            envelope.step(meters.loudest() > TALK_THRESHOLD, TICK_MS as f32)
        } else {
            // Rescan right away once this PC starts receiving again.
            ticks = 0;
            envelope = Envelope::default();
            1.0
        };
        if (gain - applied).abs() > 0.001 {
            audio::set_duck(gain);
            applied = gain;
        }
        std::thread::sleep(Duration::from_millis(TICK_MS));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads every app's level on every output device (silent):
    /// `cargo test -- --ignored hardware`.
    #[test]
    #[ignore]
    fn hardware_reads_app_levels() {
        let _com = ComGuard::new();
        let mut meters = AppMeters::default();
        let start = std::time::Instant::now();
        meters.rescan(|_| true).unwrap();
        println!(
            "rescan: {:?}, {} app sessions",
            start.elapsed(),
            meters.sessions()
        );

        // Reading the meters is what runs every tick: it must be cheap.
        let start = std::time::Instant::now();
        let mut peak = 0.0;
        for _ in 0..100 {
            peak = meters.loudest();
        }
        let per_read = start.elapsed() / 100;
        println!("read: {per_read:?}, loudest peak {peak:.3}");
        assert!((0.0..=1.0).contains(&peak));
        assert!(per_read < Duration::from_millis(1));
    }

    fn run_for(envelope: &mut Envelope, talking: bool, ms: u64) -> f32 {
        let mut gain = 0.0;
        for _ in 0..ms / TICK_MS {
            gain = envelope.step(talking, TICK_MS as f32);
        }
        gain
    }

    #[test]
    fn ducks_fast_holds_through_pauses_and_recovers_slowly() {
        let mut env = Envelope::default();
        assert_eq!(run_for(&mut env, false, 2_000), 1.0, "no call, no ducking");

        // A voice starts: fully lowered within the attack time.
        assert_eq!(
            run_for(&mut env, true, ATTACK_MS as u64 + TICK_MS),
            DUCKED_GAIN
        );

        // A pause between words shorter than the hold keeps it down.
        assert_eq!(
            run_for(&mut env, false, HOLD_MS as u64 - 2 * TICK_MS),
            DUCKED_GAIN
        );
        assert_eq!(run_for(&mut env, true, TICK_MS), DUCKED_GAIN);

        // Silence: stays down for the hold, then rises gradually…
        let halfway = run_for(&mut env, false, HOLD_MS as u64 + RELEASE_MS as u64 / 2);
        assert!(
            halfway > DUCKED_GAIN && halfway < 1.0,
            "halfway gain {halfway}"
        );
        // …and is fully back after the release.
        assert_eq!(run_for(&mut env, false, RELEASE_MS as u64), 1.0);
    }

    #[test]
    fn call_apps_are_recognised() {
        for app in ["discord", "ms-teams", "zoom", "ts3client_win64"] {
            assert!(is_voice_app(app), "{app}");
        }
        for app in ["chrome", "spotify", "trovul", "Discord"] {
            assert!(!is_voice_app(app), "{app}");
        }
    }
}
