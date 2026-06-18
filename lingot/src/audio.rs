/*
 * lingot_tuner_rust - a musical instrument tuner.
 * Rust rewrite of lingot (https://github.com/ibancg/lingot).
 *
 * Copyright (C) 2004-2020  Iban Cereijo.
 * Copyright (C) 2004-2008  Jairo Chapela.
 * Copyright (C) 2026       lingot_tuner_rust contributors.
 *
 * This file is part of lingot_tuner_rust.
 *
 * lingot_tuner_rust is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * lingot_tuner_rust is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with lingot_tuner_rust. If not, see <https://www.gnu.org/licenses/>.
 */

//! Cross-platform audio capture via `cpal`, replacing lingot's per-backend
//! audio layer (`lingot-audio*.c`). cpal abstracts ALSA/PulseAudio/JACK on
//! Linux and WASAPI on Windows.
//!
//! cpal drives its own audio thread and calls our data callback, so there is no
//! blocking-read mainloop as in the C original. Captured frames are downmixed
//! to mono and normalised to `f64` in `[-1, 1]` before being handed to the
//! caller's callback. Keep that callback lightweight — it runs on the realtime
//! audio thread (push into a channel/ring buffer; never block or allocate).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no default input device available")]
    NoDefaultDevice,
    #[error("input device not found: {0}")]
    DeviceNotFound(String),
    #[error("unsupported sample format: {0:?}")]
    UnsupportedSampleFormat(SampleFormat),
    #[error(transparent)]
    Cpal(#[from] cpal::Error),
}

/// Which device to open and at what sample rate.
#[derive(Clone, Debug)]
pub struct AudioInputConfig {
    /// Device name, or `None` for the system default.
    pub device: Option<String>,
    /// Desired sample rate (Hz). The opened stream may use a different rate if
    /// the device does not support this one; see [`AudioInput::sample_rate`].
    pub sample_rate: u32,
}

/// Names of all available input devices on the default host.
pub fn input_devices() -> Result<Vec<String>, AudioError> {
    let host = cpal::default_host();
    let mut names = Vec::new();
    for device in host.input_devices()? {
        if let Ok(desc) = device.description() {
            names.push(desc.name().to_string());
        }
    }
    Ok(names)
}

/// An open audio input stream. Dropping it stops capture.
///
/// The underlying cpal stream is not `Send`, so an `AudioInput` must live on
/// the thread that created it.
pub struct AudioInput {
    stream: cpal::Stream,
    sample_rate: u32,
    channels: u16,
    healthy: Arc<AtomicBool>,
}

impl AudioInput {
    /// Open an input stream. `callback` is invoked on the audio thread with
    /// each block of mono `f64` samples (normalised to `[-1, 1]`). The stream
    /// starts paused — call [`play`](Self::play) to begin capture.
    pub fn new<C>(config: &AudioInputConfig, callback: C) -> Result<Self, AudioError>
    where
        C: FnMut(&[f64]) + Send + 'static,
    {
        let host = cpal::default_host();

        let device = match &config.device {
            Some(name) => host
                .input_devices()?
                .find(|d| {
                    d.description()
                        .map(|desc| desc.name() == name.as_str())
                        .unwrap_or(false)
                })
                .ok_or_else(|| AudioError::DeviceNotFound(name.clone()))?,
            None => host.default_input_device().ok_or(AudioError::NoDefaultDevice)?,
        };

        let supported = pick_config(&device, config.sample_rate)?;
        let sample_format = supported.sample_format();
        let stream_config: cpal::StreamConfig = supported.config();
        let sample_rate = stream_config.sample_rate;
        let channels = stream_config.channels;

        let healthy = Arc::new(AtomicBool::new(true));

        let stream = match sample_format {
            SampleFormat::I8 => build_stream::<i8, C>(&device, stream_config, channels, healthy.clone(), callback)?,
            SampleFormat::I16 => build_stream::<i16, C>(&device, stream_config, channels, healthy.clone(), callback)?,
            SampleFormat::I32 => build_stream::<i32, C>(&device, stream_config, channels, healthy.clone(), callback)?,
            SampleFormat::U8 => build_stream::<u8, C>(&device, stream_config, channels, healthy.clone(), callback)?,
            SampleFormat::U16 => build_stream::<u16, C>(&device, stream_config, channels, healthy.clone(), callback)?,
            SampleFormat::U32 => build_stream::<u32, C>(&device, stream_config, channels, healthy.clone(), callback)?,
            SampleFormat::F32 => build_stream::<f32, C>(&device, stream_config, channels, healthy.clone(), callback)?,
            SampleFormat::F64 => build_stream::<f64, C>(&device, stream_config, channels, healthy.clone(), callback)?,
            other => return Err(AudioError::UnsupportedSampleFormat(other)),
        };

        Ok(AudioInput {
            stream,
            sample_rate,
            channels,
            healthy,
        })
    }

    /// Actual sample rate of the opened stream (may differ from the request).
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Channel count of the underlying device (frames are downmixed to mono
    /// before reaching the callback).
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Whether the stream is still healthy (cleared if cpal reports an error,
    /// e.g. the device was unplugged).
    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    pub fn play(&self) -> Result<(), AudioError> {
        self.stream.play()?;
        Ok(())
    }

    pub fn pause(&self) -> Result<(), AudioError> {
        self.stream.pause()?;
        Ok(())
    }
}

/// Choose a supported config for `device`, preferring mono and the requested
/// sample rate; falls back to the device default if the rate is unavailable.
fn pick_config(
    device: &cpal::Device,
    desired_rate: u32,
) -> Result<cpal::SupportedStreamConfig, AudioError> {
    let mut ranges: Vec<_> = device.supported_input_configs()?.collect();
    ranges.sort_by_key(|r| r.channels());

    for range in &ranges {
        if range.min_sample_rate() <= desired_rate && desired_rate <= range.max_sample_rate() {
            return Ok(range.clone().with_sample_rate(desired_rate));
        }
    }

    Ok(device.default_input_config()?)
}

fn build_stream<T, C>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: u16,
    healthy: Arc<AtomicBool>,
    mut callback: C,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f64: FromSample<T>,
    C: FnMut(&[f64]) + Send + 'static,
{
    let channels = channels as usize;
    let mut converted: Vec<f64> = Vec::new();
    let mut mono: Vec<f64> = Vec::new();

    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            converted.clear();
            converted.extend(data.iter().map(|&s| f64::from_sample_(s)));
            downmix_into(&converted, channels, &mut mono);
            callback(&mono);
        },
        move |err| {
            eprintln!("audio input stream error: {err}");
            healthy.store(false, Ordering::Relaxed);
        },
        None,
    )
}

/// Downmix interleaved `channels`-channel audio to mono by averaging each
/// frame. With one channel it is a straight copy.
fn downmix_into(interleaved: &[f64], channels: usize, out: &mut Vec<f64>) {
    out.clear();
    if channels <= 1 {
        out.extend_from_slice(interleaved);
    } else {
        for frame in interleaved.chunks(channels) {
            let sum: f64 = frame.iter().sum();
            out.push(sum / frame.len() as f64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_downmix_is_passthrough() {
        let mut out = Vec::new();
        downmix_into(&[0.1, 0.2, 0.3], 1, &mut out);
        assert_eq!(out, vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn stereo_downmix_averages_frames() {
        let mut out = Vec::new();
        // frames: (1.0, 0.0), (0.5, 0.5), (-1.0, 1.0)
        downmix_into(&[1.0, 0.0, 0.5, 0.5, -1.0, 1.0], 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5, 0.0]);
    }

    #[test]
    fn downmix_reuses_output_buffer() {
        let mut out = vec![9.9; 8];
        downmix_into(&[1.0, 1.0], 2, &mut out);
        assert_eq!(out, vec![1.0]);
    }

    #[test]
    fn listing_input_devices_does_not_panic() {
        // In a headless/CI environment there may be no devices or no host;
        // either an Ok list or an Err is acceptable — we only assert no panic.
        let _ = input_devices();
    }
}
