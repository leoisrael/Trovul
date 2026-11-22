// Trovul - Test chime. Played on the sending PC's default output so it travels
// the real path (loopback capture → network → the other PC's speakers): if the
// other PC hears it, the whole setup works.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::f32::consts::TAU;
use std::time::Duration;

const LENGTH_S: f32 = 1.1;

/// Two bell-like notes (G5 then C6) with a click-free attack and a long decay.
pub fn chime_sample(t: f32) -> f32 {
    let note = |freq: f32, start: f32| {
        let local = t - start;
        if !(0.0..LENGTH_S - start).contains(&local) {
            return 0.0;
        }
        let attack = 1.0 - (-local * 400.0).exp();
        let decay = (-local * 5.5).exp();
        // Fade the tail to exactly zero at the end.
        let tail = ((LENGTH_S - start - local) / 0.05).min(1.0);
        (TAU * freq * local).sin() * attack * decay * tail
    };
    0.25 * (note(784.0, 0.0) + note(1046.5, 0.16))
}

/// Play the chime on the default output device. Blocks for about a second.
pub fn play_chime() -> Result<(), String> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("No output device available")?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("Could not read the output format: {e}"))?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let rate = config.sample_rate.0 as f32;
    let mut n = 0u64;
    let next = move || {
        let t = n as f32 / rate;
        n += 1;
        chime_sample(t)
    };
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, &config, next),
        SampleFormat::I16 => build::<i16>(&device, &config, next),
        SampleFormat::U16 => build::<u16>(&device, &config, next),
        SampleFormat::I32 => build::<i32>(&device, &config, next),
        SampleFormat::F64 => build::<f64>(&device, &config, next),
        other => Err(format!("Unsupported output sample format {other:?}")),
    }?;
    stream
        .play()
        .map_err(|e| format!("Could not play the test sound: {e}"))?;
    std::thread::sleep(Duration::from_secs_f32(LENGTH_S + 0.15));
    Ok(())
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut next: impl FnMut() -> f32 + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = (config.channels as usize).max(1);
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_exact_mut(channels) {
                    let sample = T::from_sample(next());
                    frame.fill(sample);
                }
            },
            |e| log::warn!("Test sound stream error: {e}"),
            None,
        )
        .map_err(|e| format!("Could not open the output device: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chime_is_bounded_and_click_free() {
        let rate = 48_000.0;
        let samples: Vec<f32> = (0..(rate * LENGTH_S) as usize + 100)
            .map(|n| chime_sample(n as f32 / rate))
            .collect();
        assert_eq!(samples[0], 0.0, "starts from silence");
        assert!(samples.iter().all(|s| s.abs() <= 0.5));
        assert!(samples.iter().any(|s| s.abs() > 0.2), "audible");
        assert!(samples.last().unwrap().abs() < 1e-6, "ends in silence");
        let max_step = samples
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(max_step < 0.08, "no clicks (max step {max_step})");
    }
}
