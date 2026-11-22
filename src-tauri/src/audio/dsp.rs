// Trovul - Audio DSP building blocks (no I/O, unit-tested).
//
// Capture side: decode the loopback mix format and downmix any speaker layout
// to stereo. Receive side: an adaptive jitter buffer that resamples whole
// frames (channels never bleed into each other), keeps interpolation state
// across packets, and steers playback speed with a PI controller so the buffer
// settles at a small target instead of drifting full or empty.

use crate::util::AtomicF32;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

// ───────────────────────── capture: decode + downmix ─────────────────────────

pub const WAVE_FORMAT_PCM: u16 = 1;
pub const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleKind {
    F32,
    I16,
    I24,
    I32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureFormat {
    pub kind: SampleKind,
    pub sample_rate: u32,
    pub channels: u16,
    pub channel_mask: u32,
    pub block_align: usize,
    pub bytes_per_sample: usize,
}

impl CaptureFormat {
    /// `format_tag` is the effective tag — for WAVE_FORMAT_EXTENSIBLE, the
    /// sub-format's. `bits_per_sample` is the container size.
    pub fn new(
        format_tag: u16,
        bits_per_sample: u16,
        channels: u16,
        sample_rate: u32,
        block_align: u16,
        channel_mask: u32,
    ) -> Result<Self, String> {
        let kind = match (format_tag, bits_per_sample) {
            (WAVE_FORMAT_IEEE_FLOAT, 32) => SampleKind::F32,
            (WAVE_FORMAT_PCM, 16) => SampleKind::I16,
            (WAVE_FORMAT_PCM, 24) => SampleKind::I24,
            (WAVE_FORMAT_PCM, 32) => SampleKind::I32,
            _ => {
                return Err(format!(
                    "unsupported capture format (tag {format_tag}, {bits_per_sample} bits)"
                ))
            }
        };
        let bytes_per_sample = bits_per_sample as usize / 8;
        if channels == 0 || block_align as usize != bytes_per_sample * channels as usize {
            return Err(format!(
                "inconsistent capture format ({channels} channels, block align {block_align})"
            ));
        }
        Ok(Self {
            kind,
            sample_rate,
            channels,
            channel_mask,
            block_align: block_align as usize,
            bytes_per_sample,
        })
    }
}

#[inline]
fn read_sample(kind: SampleKind, b: &[u8]) -> f32 {
    match kind {
        SampleKind::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        SampleKind::I16 => i16::from_le_bytes([b[0], b[1]]) as f32 / 32_768.0,
        SampleKind::I24 => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
        SampleKind::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
    }
}

// KSAUDIO speaker bits in the order their channels appear in an interleaved
// frame, with (left, right) downmix gains (ITU-R BS.775 style, LFE dropped).
const SPEAKER_GAINS: [(u32, [f32; 2]); 18] = [
    (0x1, [1.0, 0.0]),         // front left
    (0x2, [0.0, 1.0]),         // front right
    (0x4, [0.707, 0.707]),     // front center
    (0x8, [0.0, 0.0]),         // low frequency
    (0x10, [0.707, 0.0]),      // back left
    (0x20, [0.0, 0.707]),      // back right
    (0x40, [0.924, 0.383]),    // front left of center
    (0x80, [0.383, 0.924]),    // front right of center
    (0x100, [0.5, 0.5]),       // back center
    (0x200, [0.707, 0.0]),     // side left
    (0x400, [0.0, 0.707]),     // side right
    (0x800, [0.5, 0.5]),       // top center
    (0x1000, [0.707, 0.0]),    // top front left
    (0x2000, [0.5, 0.5]),      // top front center
    (0x4000, [0.0, 0.707]),    // top front right
    (0x8000, [0.5, 0.0]),      // top back left
    (0x10000, [0.354, 0.354]), // top back center
    (0x20000, [0.0, 0.5]),     // top back right
];

/// Standard Windows layouts, for formats that don't carry a channel mask.
fn default_channel_mask(channels: u16) -> u32 {
    match channels {
        1 => 0x4,
        3 => 0x7,
        4 => 0x33,
        5 => 0x37,
        6 => 0x3F,
        7 => 0x13F,
        8 => 0x63F,
        _ => 0x3,
    }
}

/// Folds an arbitrary speaker layout (mono … 7.1 and beyond) into stereo.
pub struct Downmix {
    gains: Vec<[f32; 2]>,
}

impl Downmix {
    pub fn new(channels: u16, channel_mask: u32) -> Self {
        if channels == 1 {
            return Self {
                gains: vec![[1.0, 1.0]],
            };
        }
        let mask = if channel_mask == 0 {
            default_channel_mask(channels)
        } else {
            channel_mask
        };
        let mut gains: Vec<[f32; 2]> = SPEAKER_GAINS
            .iter()
            .filter(|(bit, _)| mask & bit != 0)
            .map(|(_, gain)| *gain)
            .take(channels as usize)
            .collect();
        // Channels beyond the mask carry no position; leave them silent.
        gains.resize(channels as usize, [0.0, 0.0]);
        Self { gains }
    }

    /// Mix one interleaved frame of raw capture bytes down to stereo.
    #[inline]
    pub fn frame(&self, frame: &[u8], format: &CaptureFormat) -> [f32; 2] {
        let mut out = [0.0f32; 2];
        for (channel, gain) in self.gains.iter().enumerate() {
            let at = channel * format.bytes_per_sample;
            let x = read_sample(format.kind, &frame[at..at + format.bytes_per_sample]);
            if x.is_finite() {
                out[0] += gain[0] * x;
                out[1] += gain[1] * x;
            }
        }
        out
    }
}

#[inline]
pub fn to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * 32_767.0).round() as i16
}

// ───────────────────────── receive: adaptive jitter buffer ─────────────────────────

pub type Frame = [f32; 2];

/// Where the renderer pulls decoded frames from (the SPSC ring in production).
pub trait FrameSource {
    fn available(&self) -> usize;
    fn pop(&mut self) -> Option<Frame>;
    fn discard(&mut self, count: usize) -> usize;
}

impl FrameSource for ringbuf::HeapCons<Frame> {
    fn available(&self) -> usize {
        ringbuf::traits::Observer::occupied_len(self)
    }

    fn pop(&mut self) -> Option<Frame> {
        ringbuf::traits::Consumer::try_pop(self)
    }

    fn discard(&mut self, count: usize) -> usize {
        ringbuf::traits::Consumer::skip(self, count)
    }
}

/// Lock-free state shared by the network thread (producer), the renderer
/// (audio callback) and the stats reader.
#[derive(Debug)]
pub struct JitterShared {
    /// Sample rate of the incoming stream; 0 until the first packet.
    pub input_rate: AtomicU32,
    /// Raised by the network thread when the stream format changes.
    pub flush: AtomicBool,
    /// `util::mono_ms()` of the last accepted packet; 0 = never.
    pub last_packet_ms: AtomicU64,
    /// Output gain (0–1, 0 when muted); the renderer glides towards it.
    pub gain: AtomicF32,
    // Published by the renderer.
    pub playing: AtomicBool,
    pub underruns: AtomicU64,
    pub skips: AtomicU64,
    pub buffer_ms: AtomicF32,
    pub target_ms: AtomicF32,
    pub drift_ppm: AtomicF32,
    /// Peak level with a short decay, for meters (the sender writes its capture level here).
    pub level: AtomicF32,
}

impl Default for JitterShared {
    fn default() -> Self {
        Self {
            input_rate: AtomicU32::new(0),
            flush: AtomicBool::new(false),
            last_packet_ms: AtomicU64::new(0),
            gain: AtomicF32::new(1.0),
            playing: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
            skips: AtomicU64::new(0),
            buffer_ms: AtomicF32::new(0.0),
            target_ms: AtomicF32::new(0.0),
            drift_ppm: AtomicF32::new(0.0),
            level: AtomicF32::new(0.0),
        }
    }
}

impl JitterShared {
    /// Fold a new block peak into the decaying meter level.
    pub fn publish_level(&self, block_peak: f32) {
        self.level.store(block_peak.max(self.level.load() * 0.85));
    }
}

#[derive(Debug, Clone, Copy)]
pub struct JitterConfig {
    pub start_target_ms: f64,
    pub min_target_ms: f64,
    pub max_target_ms: f64,
    /// Added to the target after an underrun caused by late packets.
    pub underrun_step_ms: f64,
    /// Removed from the target after `relax_after_s` without underruns.
    pub relax_step_ms: f64,
    pub relax_after_s: f64,
    /// Buffered audio beyond target + this is dropped at once.
    pub max_excess_ms: f64,
    /// Packets resuming within this long after an underrun mean network jitter;
    /// longer means the sender simply went quiet.
    pub jitter_gap_ms: u64,
}

impl Default for JitterConfig {
    /// Balanced: good on wired and decent Wi-Fi.
    fn default() -> Self {
        Self {
            start_target_ms: 50.0,
            min_target_ms: 30.0,
            max_target_ms: 250.0,
            underrun_step_ms: 20.0,
            relax_step_ms: 5.0,
            relax_after_s: 60.0,
            max_excess_ms: 120.0,
            jitter_gap_ms: 500,
        }
    }
}

impl JitterConfig {
    /// Wired LAN / gaming: least delay, more sensitive to Wi-Fi hiccups.
    pub fn low_latency() -> Self {
        Self {
            start_target_ms: 30.0,
            min_target_ms: 20.0,
            max_target_ms: 150.0,
            underrun_step_ms: 15.0,
            ..Self::default()
        }
    }

    /// Busy Wi-Fi: rarely drops out, noticeably more delay.
    pub fn stable() -> Self {
        Self {
            start_target_ms: 120.0,
            min_target_ms: 80.0,
            max_target_ms: 400.0,
            underrun_step_ms: 40.0,
            max_excess_ms: 200.0,
            ..Self::default()
        }
    }
}

/// PI controller mapping "seconds buffered above target" to a playback speed.
#[derive(Debug, Clone)]
pub struct PiController {
    kp: f64,
    ki: f64,
    max_deviation: f64,
    max_integral: f64,
    integral: f64,
}

impl Default for PiController {
    fn default() -> Self {
        // Critically damped (ζ = 1) with ω = 0.05 rad/s: settles within a couple
        // of minutes, and a 20 ms error shifts playback speed by only 0.2 %
        // (≈3.5 cents). The integral absorbs crystal drift (tens to hundreds of ppm).
        Self {
            kp: 0.1,
            ki: 0.0025,
            max_deviation: 0.005,
            max_integral: 0.002,
            integral: 0.0,
        }
    }
}

impl PiController {
    /// `error_s`: seconds of audio buffered above target. Returns the playback
    /// speed multiplier (> 1 drains the buffer).
    pub fn update(&mut self, error_s: f64, dt: f64) -> f64 {
        let proportional = self.kp * error_s;
        let next =
            (self.integral + self.ki * error_s * dt).clamp(-self.max_integral, self.max_integral);
        // Anti-windup: stop integrating while saturated unless it unwinds.
        if (proportional + next).abs() <= self.max_deviation || next.abs() < self.integral.abs() {
            self.integral = next;
        }
        1.0 + (proportional + self.integral).clamp(-self.max_deviation, self.max_deviation)
    }

    #[cfg(test)]
    pub fn integral(&self) -> f64 {
        self.integral
    }
}

const CONTROL_PERIOD_S: f64 = 0.1;
const FILL_SMOOTHING_S: f64 = 1.0;
const FADE_S: f64 = 0.005;
const GAIN_GLIDE_S: f64 = 0.01;

/// Pulls stereo frames at the input rate and produces frames at the output
/// rate. Runs inside the audio callback: no allocation, no locks.
pub struct Renderer<S: FrameSource> {
    source: S,
    shared: Arc<JitterShared>,
    cfg: JitterConfig,
    clock: fn() -> u64,
    out_rate: f64,
    in_rate: f64,
    step: f64,
    ratio: f64,
    controller: PiController,
    history: [Frame; 4],
    frac: f64,
    playing: bool,
    target_ms: f64,
    fill_avg: f64,
    control_countdown: usize,
    stable_frames: u64,
    underrun_at: Option<u64>,
    fade_in: f32,
    tail: Frame,
    tail_gain: f32,
    fade_step: f32,
    gain: f32,
    gain_target: f32,
    gain_coeff: f32,
    block_peak: f32,
}

impl<S: FrameSource> Renderer<S> {
    pub fn new(source: S, shared: Arc<JitterShared>, out_rate: u32, cfg: JitterConfig) -> Self {
        Self::with_clock(source, shared, out_rate, cfg, crate::util::mono_ms)
    }

    pub fn with_clock(
        source: S,
        shared: Arc<JitterShared>,
        out_rate: u32,
        cfg: JitterConfig,
        clock: fn() -> u64,
    ) -> Self {
        let out_rate = out_rate.max(1) as f64;
        shared.target_ms.store(cfg.start_target_ms as f32);
        let gain = shared.gain.load().clamp(0.0, 1.0);
        Self {
            source,
            shared,
            cfg,
            clock,
            out_rate,
            in_rate: 0.0,
            step: 1.0,
            ratio: 1.0,
            controller: PiController::default(),
            history: [[0.0; 2]; 4],
            frac: 0.0,
            playing: false,
            target_ms: cfg.start_target_ms,
            fill_avg: 0.0,
            control_countdown: 0,
            stable_frames: 0,
            underrun_at: None,
            fade_in: 0.0,
            tail: [0.0; 2],
            tail_gain: 0.0,
            fade_step: (1.0 / (FADE_S * out_rate)) as f32,
            gain,
            gain_target: gain,
            // One-pole glide of ~10 ms so volume changes never click.
            gain_coeff: (1.0 - (-1.0 / (GAIN_GLIDE_S * out_rate)).exp()) as f32,
            block_peak: 0.0,
        }
    }

    /// Call once per audio callback, before producing `frames` output frames.
    pub fn begin_block(&mut self, frames: usize) {
        self.gain_target = self.shared.gain.load().clamp(0.0, 1.0);
        self.block_peak = 0.0;
        if self.shared.flush.swap(false, Ordering::AcqRel) {
            let buffered = self.source.available();
            self.source.discard(buffered);
            self.halt();
            self.underrun_at = None;
        }
        let rate = self.shared.input_rate.load(Ordering::Acquire);
        if rate == 0 {
            return;
        }
        if rate as f64 != self.in_rate {
            self.in_rate = rate as f64;
            self.fill_avg = self.source.available() as f64;
        }

        let fill = self.source.available() as f64;
        let alpha = (frames as f64 / (FILL_SMOOTHING_S * self.out_rate)).min(1.0);
        self.fill_avg += alpha * (fill - self.fill_avg);
        let target = self.target_frames();

        if self.playing {
            let max_fill = target + self.cfg.max_excess_ms / 1000.0 * self.in_rate;
            if fill > max_fill {
                // Burst after a network stall: jump back to the target rather
                // than pitch-shifting through the backlog for a minute.
                self.source.discard((fill - target) as usize);
                self.fill_avg = target;
                self.shared.skips.fetch_add(1, Ordering::Relaxed);
            }
            if self.control_countdown <= frames {
                self.control_countdown = (CONTROL_PERIOD_S * self.out_rate) as usize;
                let error_s = (self.fill_avg - target) / self.in_rate;
                self.ratio = self.controller.update(error_s, CONTROL_PERIOD_S);
            } else {
                self.control_countdown -= frames;
            }
            self.stable_frames += frames as u64;
            if self.stable_frames as f64 >= self.cfg.relax_after_s * self.out_rate {
                self.stable_frames = 0;
                self.target_ms =
                    (self.target_ms - self.cfg.relax_step_ms).max(self.cfg.min_target_ms);
            }
        } else {
            self.classify_underrun();
            if fill >= target.max(4.0) {
                self.start();
            }
        }
        self.step = self.in_rate / self.out_rate * self.ratio;
    }

    #[inline]
    pub fn next_frame(&mut self) -> Frame {
        let [l, r] = self.next_unscaled();
        self.gain += (self.gain_target - self.gain) * self.gain_coeff;
        let out = [l * self.gain, r * self.gain];
        self.block_peak = self.block_peak.max(out[0].abs()).max(out[1].abs());
        out
    }

    #[inline]
    fn next_unscaled(&mut self) -> Frame {
        if !self.playing {
            if self.tail_gain > 0.0 {
                self.tail_gain = (self.tail_gain - self.fade_step).max(0.0);
                return [self.tail[0] * self.tail_gain, self.tail[1] * self.tail_gain];
            }
            return [0.0, 0.0];
        }
        let y = catmull_rom(&self.history, self.frac as f32);
        self.frac += self.step;
        while self.frac >= 1.0 {
            match self.source.pop() {
                Some(frame) => {
                    self.history = [self.history[1], self.history[2], self.history[3], frame];
                    self.frac -= 1.0;
                }
                None => {
                    self.on_underrun();
                    break;
                }
            }
        }
        if self.fade_in < 1.0 {
            self.fade_in = (self.fade_in + self.fade_step).min(1.0);
        }
        let out = [y[0] * self.fade_in, y[1] * self.fade_in];
        self.tail = out;
        out
    }

    /// Call after the block has been produced; publishes the measurements.
    pub fn end_block(&mut self) {
        let shared = &self.shared;
        shared.publish_level(self.block_peak);
        shared.playing.store(self.playing, Ordering::Relaxed);
        shared.target_ms.store(self.target_ms as f32);
        shared.drift_ppm.store(((self.ratio - 1.0) * 1e6) as f32);
        if self.in_rate > 0.0 {
            shared
                .buffer_ms
                .store((self.fill_avg / self.in_rate * 1000.0) as f32);
        }
    }

    fn target_frames(&self) -> f64 {
        self.target_ms / 1000.0 * self.in_rate
    }

    fn start(&mut self) {
        let first = self.source.pop().unwrap_or_default();
        let second = self.source.pop().unwrap_or(first);
        let third = self.source.pop().unwrap_or(second);
        self.history = [first, first, second, third];
        self.frac = 0.0;
        self.fade_in = 0.0;
        self.tail_gain = 0.0;
        self.playing = true;
        self.stable_frames = 0;
    }

    fn halt(&mut self) {
        if self.playing {
            self.tail_gain = 1.0; // fade out from the last frame instead of clicking
        }
        self.playing = false;
        self.frac = 0.0;
    }

    fn on_underrun(&mut self) {
        self.halt();
        self.underrun_at = Some((self.clock)());
    }

    /// Once packets flow again, decide whether the last underrun was network
    /// jitter (packets came back quickly → buffer more from now on) or just the
    /// sender going quiet (nothing to fix).
    fn classify_underrun(&mut self) {
        let Some(at) = self.underrun_at else { return };
        let last_packet = self.shared.last_packet_ms.load(Ordering::Relaxed);
        if last_packet <= at {
            return;
        }
        self.underrun_at = None;
        if last_packet - at <= self.cfg.jitter_gap_ms {
            self.target_ms =
                (self.target_ms + self.cfg.underrun_step_ms).min(self.cfg.max_target_ms);
            self.shared.underruns.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Catmull-Rom interpolation between p[1] and p[2], per channel.
#[inline]
fn catmull_rom(p: &[Frame; 4], t: f32) -> Frame {
    let t2 = t * t;
    let t3 = t2 * t;
    let mut out = [0.0f32; 2];
    for (c, value) in out.iter_mut().enumerate() {
        let (p0, p1, p2, p3) = (p[0][c], p[1][c], p[2][c], p[3][c]);
        *value = 0.5
            * ((2.0 * p1)
                + (-p0 + p2) * t
                + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::f32::consts::TAU;
    use std::rc::Rc;

    // ── capture side ──

    fn f32_frame(samples: &[f32]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn stereo_passes_through() {
        let fmt = CaptureFormat::new(WAVE_FORMAT_IEEE_FLOAT, 32, 2, 48_000, 8, 0x3).unwrap();
        let mix = Downmix::new(2, 0x3);
        assert_eq!(mix.frame(&f32_frame(&[0.5, -0.25]), &fmt), [0.5, -0.25]);
    }

    #[test]
    fn surround_5_1_downmix() {
        let fmt = CaptureFormat::new(WAVE_FORMAT_IEEE_FLOAT, 32, 6, 48_000, 24, 0x3F).unwrap();
        let mix = Downmix::new(6, 0x3F);
        // FL FR FC LFE BL BR
        let [l, r] = mix.frame(&f32_frame(&[0.1, 0.2, 0.4, 0.9, 0.3, 0.5]), &fmt);
        assert!((l - (0.1 + 0.707 * 0.4 + 0.707 * 0.3)).abs() < 1e-6);
        assert!((r - (0.2 + 0.707 * 0.4 + 0.707 * 0.5)).abs() < 1e-6);
    }

    #[test]
    fn maskless_7_1_uses_the_standard_layout() {
        let fmt = CaptureFormat::new(WAVE_FORMAT_IEEE_FLOAT, 32, 8, 48_000, 32, 0).unwrap();
        let mix = Downmix::new(8, 0);
        // Only the side-left channel (7th) carries signal.
        let [l, r] = mix.frame(&f32_frame(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]), &fmt);
        assert!((l - 0.707).abs() < 1e-6 && r == 0.0);
    }

    #[test]
    fn mono_and_integer_formats() {
        let fmt = CaptureFormat::new(WAVE_FORMAT_PCM, 16, 1, 44_100, 2, 0).unwrap();
        let mix = Downmix::new(1, 0);
        let [l, r] = mix.frame(&(-16_384i16).to_le_bytes(), &fmt);
        assert_eq!((l, r), (-0.5, -0.5));

        let fmt24 = CaptureFormat::new(WAVE_FORMAT_PCM, 24, 2, 48_000, 6, 0x3).unwrap();
        let mix24 = Downmix::new(2, 0x3);
        let minus_half = (-4_194_304i32).to_le_bytes(); // -0.5 in 24-bit
        let bytes = [minus_half[0], minus_half[1], minus_half[2], 0, 0, 0];
        assert_eq!(mix24.frame(&bytes, &fmt24), [-0.5, 0.0]);
    }

    #[test]
    fn rejects_unsupported_formats() {
        assert!(CaptureFormat::new(WAVE_FORMAT_PCM, 8, 2, 48_000, 2, 0).is_err());
        assert!(CaptureFormat::new(WAVE_FORMAT_IEEE_FLOAT, 32, 2, 48_000, 6, 0).is_err());
    }

    #[test]
    fn i16_conversion_clamps() {
        assert_eq!(to_i16(2.0), 32_767);
        assert_eq!(to_i16(-2.0), -32_767);
        assert_eq!(to_i16(0.0), 0);
        assert_eq!(to_i16(0.5), 16_384);
    }

    // ── receive side ──

    #[derive(Clone, Default)]
    struct TestSource(Rc<RefCell<VecDeque<Frame>>>);

    impl FrameSource for TestSource {
        fn available(&self) -> usize {
            self.0.borrow().len()
        }
        fn pop(&mut self) -> Option<Frame> {
            self.0.borrow_mut().pop_front()
        }
        fn discard(&mut self, count: usize) -> usize {
            let mut q = self.0.borrow_mut();
            let n = count.min(q.len());
            q.drain(..n);
            n
        }
    }

    thread_local! {
        static NOW_MS: Cell<u64> = const { Cell::new(1) };
    }

    fn test_clock() -> u64 {
        NOW_MS.with(|n| n.get())
    }

    fn set_clock(ms: u64) {
        NOW_MS.with(|n| n.set(ms));
    }

    fn renderer(
        in_rate: u32,
        out_rate: u32,
    ) -> (Renderer<TestSource>, TestSource, Arc<JitterShared>) {
        let source = TestSource::default();
        let shared = Arc::new(JitterShared::default());
        shared.input_rate.store(in_rate, Ordering::Relaxed);
        let r = Renderer::with_clock(
            source.clone(),
            shared.clone(),
            out_rate,
            JitterConfig::default(),
            test_clock,
        );
        (r, source, shared)
    }

    fn render_block(r: &mut Renderer<TestSource>, frames: usize, out: &mut Vec<Frame>) {
        r.begin_block(frames);
        for _ in 0..frames {
            out.push(r.next_frame());
        }
        r.end_block();
    }

    /// Feeds a signal in real time: `packet` frames every `packet / rate` seconds.
    struct Feeder {
        rate: f64,
        sent: u64,
        signal: fn(u64, f64) -> Frame,
    }

    impl Feeder {
        fn feed_until(&mut self, source: &TestSource, shared: &JitterShared, t_s: f64) {
            let due = (t_s * self.rate) as u64;
            while self.sent + 240 <= due {
                let mut q = source.0.borrow_mut();
                for i in 0..240 {
                    q.push_back((self.signal)(self.sent + i, self.rate));
                }
                self.sent += 240;
                shared.last_packet_ms.store(test_clock(), Ordering::Relaxed);
            }
        }
    }

    fn sine_left(n: u64, rate: f64) -> Frame {
        [(TAU * 1000.0 * n as f32 / rate as f32).sin() * 0.5, 0.0]
    }

    fn sine_right(n: u64, rate: f64) -> Frame {
        [0.0, (TAU * 1000.0 * n as f32 / rate as f32).sin() * 0.5]
    }

    fn run_realtime(
        in_rate: u32,
        out_rate: u32,
        signal: fn(u64, f64) -> Frame,
        seconds: f64,
    ) -> Vec<Frame> {
        set_clock(1);
        let (mut r, source, shared) = renderer(in_rate, out_rate);
        let mut feeder = Feeder {
            rate: in_rate as f64,
            sent: 0,
            signal,
        };
        let block = (out_rate / 100) as usize; // 10 ms callbacks
        let mut out = Vec::new();
        let mut t = 0.0;
        while t < seconds {
            t += block as f64 / out_rate as f64;
            set_clock((t * 1000.0) as u64 + 1);
            feeder.feed_until(&source, &shared, t);
            render_block(&mut r, block, &mut out);
        }
        out
    }

    fn zero_crossings(samples: impl Iterator<Item = f32>) -> usize {
        let mut prev = 0.0f32;
        let mut count = 0;
        for s in samples {
            if (prev < 0.0 && s >= 0.0) || (prev >= 0.0 && s < 0.0) {
                count += 1;
            }
            prev = s;
        }
        count
    }

    #[test]
    fn channels_never_bleed_into_each_other() {
        for signal in [sine_left as fn(u64, f64) -> Frame, sine_right] {
            let out = run_realtime(48_000, 48_000, signal, 3.0);
            let steady = &out[48_000..];
            let (active, silent): (Vec<f32>, Vec<f32>) = if signal(1, 48_000.0)[0] != 0.0 {
                (
                    steady.iter().map(|f| f[0]).collect(),
                    steady.iter().map(|f| f[1]).collect(),
                )
            } else {
                (
                    steady.iter().map(|f| f[1]).collect(),
                    steady.iter().map(|f| f[0]).collect(),
                )
            };
            assert!(
                silent.iter().all(|s| s.abs() < 1e-6),
                "silent channel picked up signal"
            );
            let rms = (active.iter().map(|s| s * s).sum::<f32>() / active.len() as f32).sqrt();
            assert!((rms - 0.5 / 2f32.sqrt()).abs() < 0.01, "rms {rms}");
        }
    }

    #[test]
    fn resampling_keeps_pitch() {
        // 1 kHz recorded at 44.1 kHz, played at 48 kHz: still 2000 crossings/s.
        let out = run_realtime(44_100, 48_000, sine_left, 4.0);
        let last_two_seconds = &out[out.len() - 96_000..];
        let crossings = zero_crossings(last_two_seconds.iter().map(|f| f[0]));
        assert!(
            (3_960..=4_040).contains(&crossings),
            "crossings {crossings}"
        );
        assert!(last_two_seconds.iter().all(|f| f[1].abs() < 1e-6));
    }

    #[test]
    fn drift_is_absorbed_without_oscillation() {
        // Sender clock 200 ppm fast, packets arriving with up to 4 ms of jitter.
        set_clock(1);
        let (mut r, source, shared) = renderer(48_000, 48_000);
        let block = 480usize;
        let packet_interval = 0.005 / (1.0 + 200e-6);
        let mut next_packet = 0.0f64;
        let mut lcg = 12345u64;
        let mut out = Vec::with_capacity(block);
        let mut max_error_last_minutes = 0.0f32;
        let mut t = 0.0f64;
        let total_s = 600.0;
        while t < total_s {
            t += block as f64 / 48_000.0;
            set_clock((t * 1000.0) as u64 + 1);
            while next_packet <= t {
                lcg = lcg
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let jitter = (lcg >> 33) as f64 / (1u64 << 31) as f64 * 0.004;
                if next_packet + jitter > t {
                    break; // not arrived yet
                }
                source
                    .0
                    .borrow_mut()
                    .extend(std::iter::repeat_n([0.1, -0.1], 240));
                shared.last_packet_ms.store(test_clock(), Ordering::Relaxed);
                next_packet += packet_interval;
            }
            out.clear();
            render_block(&mut r, block, &mut out);
            if t > total_s - 120.0 {
                let err = (shared.buffer_ms.load() - shared.target_ms.load()).abs();
                max_error_last_minutes = max_error_last_minutes.max(err);
            }
        }
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 0);
        assert_eq!(shared.skips.load(Ordering::Relaxed), 0);
        assert!(
            max_error_last_minutes < 5.0,
            "buffer error {max_error_last_minutes} ms"
        );
        let ppm = shared.drift_ppm.load();
        assert!((ppm - 200.0).abs() < 60.0, "drift estimate {ppm} ppm");
        assert_eq!(
            shared.target_ms.load(),
            30.0,
            "target relaxes to the minimum on a clean link"
        );
    }

    /// Nothing arrives for `gap_s` starting at t = 5 s. With `delayed`, the
    /// packets were only held up by the network and arrive as one burst
    /// afterwards; otherwise the sender was silent and never produced them.
    fn stall_scenario(gap_s: f64, delayed: bool) -> Arc<JitterShared> {
        set_clock(1);
        let (mut r, source, shared) = renderer(48_000, 48_000);
        let mut feeder = Feeder {
            rate: 48_000.0,
            sent: 0,
            signal: sine_left,
        };
        let mut out = Vec::new();
        let mut t = 0.0;
        while t < 15.0 {
            t += 0.01;
            set_clock((t * 1000.0) as u64 + 1);
            if (5.0..5.0 + gap_s).contains(&t) {
                if !delayed {
                    feeder.sent = (t * 48_000.0) as u64;
                }
            } else {
                feeder.feed_until(&source, &shared, t);
            }
            render_block(&mut r, 480, &mut out);
        }
        shared
    }

    #[test]
    fn late_packets_grow_the_buffer() {
        let shared = stall_scenario(0.15, true);
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 1);
        assert_eq!(shared.target_ms.load(), 70.0);
    }

    #[test]
    fn a_quiet_sender_is_not_jitter() {
        let shared = stall_scenario(5.0, false);
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 0);
        assert_eq!(shared.target_ms.load(), 50.0);
    }

    #[test]
    fn backlog_is_dropped_back_to_target() {
        set_clock(1);
        let (mut r, source, shared) = renderer(48_000, 48_000);
        let mut out = Vec::new();
        source
            .0
            .borrow_mut()
            .extend(std::iter::repeat_n([0.0, 0.0], 2_880)); // 60 ms
        render_block(&mut r, 480, &mut out);
        assert!(shared.playing.load(Ordering::Relaxed));
        source
            .0
            .borrow_mut()
            .extend(std::iter::repeat_n([0.0, 0.0], 48_000)); // +1 s burst
        render_block(&mut r, 480, &mut out);
        assert_eq!(shared.skips.load(Ordering::Relaxed), 1);
        let buffered_ms = source.available() as f32 / 48.0;
        assert!(
            (buffered_ms - 50.0).abs() < 15.0,
            "buffered {buffered_ms} ms"
        );
    }

    #[test]
    fn format_change_flushes() {
        set_clock(1);
        let (mut r, source, shared) = renderer(48_000, 48_000);
        let mut out = Vec::new();
        source
            .0
            .borrow_mut()
            .extend(std::iter::repeat_n([0.3, 0.3], 4_800));
        render_block(&mut r, 480, &mut out);
        shared.flush.store(true, Ordering::Relaxed);
        shared.input_rate.store(44_100, Ordering::Relaxed);
        render_block(&mut r, 480, &mut out);
        assert_eq!(source.available(), 0);
        assert!(!shared.playing.load(Ordering::Relaxed));
    }

    #[test]
    fn volume_glides_and_mute_silences() {
        set_clock(1);
        let (mut r, source, shared) = renderer(48_000, 48_000);
        let mut feeder = Feeder {
            rate: 48_000.0,
            sent: 0,
            signal: |_, _| [0.5, 0.5],
        };
        let mut out = Vec::new();
        let mut t = 0.0;
        let mut run = |r: &mut Renderer<TestSource>, out: &mut Vec<Frame>, seconds: f64| {
            let end = t + seconds;
            while t < end {
                t += 0.01;
                set_clock((t * 1000.0) as u64 + 1);
                feeder.feed_until(&source, &shared, t);
                render_block(r, 480, out);
            }
        };
        run(&mut r, &mut out, 1.0);
        let full = out.last().unwrap()[0];
        assert!((full - 0.5).abs() < 0.01, "full volume {full}");
        assert!(shared.level.load() > 0.45);

        shared.gain.store(0.0);
        out.clear();
        run(&mut r, &mut out, 0.3);
        assert!(
            (out[0][0] - full).abs() < 0.01,
            "volume jumped instead of gliding"
        );
        assert!(
            out.windows(2).all(|w| (w[0][0] - w[1][0]).abs() < 0.01),
            "click while fading"
        );
        assert!(out.last().unwrap()[0].abs() < 1e-4, "mute is not silent");
        assert!(shared.level.load() < 0.01, "meter did not decay");
    }

    #[test]
    fn controller_saturates_without_winding_up() {
        let mut pi = PiController::default();
        for _ in 0..10_000 {
            assert!(pi.update(1.0, 0.1) <= 1.005 + 1e-12);
        }
        assert!(pi.integral() <= 0.002 + 1e-12);
        // Error flips: the output must come back below 1 quickly.
        let mut out = 1.0;
        for _ in 0..50 {
            out = pi.update(-0.2, 0.1);
        }
        assert!(out < 1.0);
    }
}
