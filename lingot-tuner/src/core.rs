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

//! The tuner core loop, mirroring `lingot-core.c`.
//!
//! This is application-level orchestration and therefore lives in the binary
//! crate, not the `lingot` library (see /CLAUDE.md). It ties together:
//!
//! - the **audio thread** (driven by cpal): filters + decimates each captured
//!   block and sends it over a channel — replacing lingot's mutex-guarded
//!   `temporal_buffer` with message passing;
//! - the **computation thread**: owns the temporal buffer privately, runs the
//!   DSP pipeline at `calculation_rate`, and sends [`TunerResult`]s to the UI.
//!
//! The only shared state is a stop flag (`AtomicBool`); everything else flows
//! through `crossbeam` channels, so there is no shared mutable buffer to guard.

use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};
use uom::si::f64::Frequency;
use uom::si::frequency::hertz;

use lingot::audio::{AudioError, AudioInput, AudioInputConfig};
use lingot::config::Config;
use lingot::fft::{spd_diffs_eval_w, FftPlan};
use lingot::filter::Filter;
use lingot::signal::{
    compute_noise_level, estimate_fundamental_frequency, FrequencyLocker,
};
use lingot::window::{self, WindowType};

/// A single tuner reading delivered to the UI.
#[derive(Clone, Debug)]
pub struct TunerResult {
    /// Detected fundamental frequency in Hz, or 0.0 if none.
    pub frequency: f64,
    /// SNR spectrum (dB) for display — one value per FFT bin in the lower half.
    /// Consumed by the spectrum view in the GUI (Layer 5).
    #[allow(dead_code)]
    pub spd: Vec<f64>,
}

/// Minimum dB value the SPD is floored to (matches lingot's `minSPL`).
const MIN_SPL_DB: f64 = -200.0;
/// Width of the noise-estimation window, in Hz (matches lingot).
const NOISE_FILTER_WIDTH_HZ: f64 = 150.0;

/// Owns the running tuner: the cpal stream and the computation thread.
///
/// The cpal stream is `!Send`, so `Core` must stay on the thread that created
/// it. Dropping `Core` stops both threads.
pub struct Core {
    audio: AudioInput,
    stop: Arc<AtomicBool>,
    compute_handle: Option<JoinHandle<()>>,
}

impl Core {
    /// Start capturing and analysing. Returns the running [`Core`] plus a
    /// receiver of [`TunerResult`]s for the UI. The audio stream is already
    /// playing on return.
    pub fn start(mut config: Config) -> Result<(Self, Receiver<TunerResult>), AudioError> {
        let requested_rate = config.sample_rate.get::<hertz>() as u32;

        // audio thread → computation thread (raw mono sample blocks)
        let (audio_tx, audio_rx) = bounded::<Vec<f64>>(64);
        // computation thread → UI (tuner results)
        let (result_tx, result_rx) = bounded::<TunerResult>(8);

        let audio_config = AudioInputConfig {
            device: config.audio_device.clone(),
            sample_rate: requested_rate,
        };

        // The audio callback is a lightweight forwarder: it does no rate-dependent
        // DSP, so it can be built before the device's real sample rate is known.
        // Filtering + decimation happen on the computation thread instead.
        let audio = AudioInput::new(&audio_config, move |block: &[f64]| {
            // Drop the block if the computation thread is lagging rather than
            // block the realtime audio thread.
            let _ = audio_tx.try_send(block.to_vec());
        })?;

        // Sample-rate renegotiation: if the device won't honour the requested
        // rate, adopt its real rate and re-derive the dependent parameters
        // (oversampling, fft/buffer sizes, …) — as lingot-core.c does. Because
        // the renegotiation happens before the computation thread is spawned,
        // everything downstream uses the correct rate.
        let real_rate = audio.sample_rate();
        if real_rate != requested_rate {
            eprintln!(
                "info: input device runs at {real_rate} Hz (requested {requested_rate} Hz); \
                 adapting analysis parameters"
            );
            config.sample_rate = Frequency::new::<hertz>(real_rate as f64);
            config.update_internal_params();
        }

        let stop = Arc::new(AtomicBool::new(false));
        let stop_compute = stop.clone();

        let compute_handle = thread::spawn(move || {
            run_computation(config, audio_rx, result_tx, stop_compute);
        });

        audio.play()?;

        Ok((
            Core {
                audio,
                stop,
                compute_handle: Some(compute_handle),
            },
            result_rx,
        ))
    }

    /// Stop capture and join the computation thread. Idempotent.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.audio.pause();
        if let Some(handle) = self.compute_handle.take() {
            let _ = handle.join();
        }
    }

    /// Whether the audio stream is still healthy. Used by the GUI (Layer 5) to
    /// surface device errors.
    #[allow(dead_code)]
    pub fn is_healthy(&self) -> bool {
        self.audio.is_healthy()
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The computation thread: drains decimated blocks, runs the DSP pipeline at
/// `calculation_rate`, and forwards results.
fn run_computation(
    config: Config,
    audio_rx: Receiver<Vec<f64>>,
    result_tx: Sender<TunerResult>,
    stop: Arc<AtomicBool>,
) {
    let tick = Duration::from_secs_f64(1.0 / config.calculation_rate.get::<hertz>());
    let mut decimator = Decimator::new(config.oversampling);
    let mut analyzer = Analyzer::new(config);

    while !stop.load(Ordering::Relaxed) {
        let started = Instant::now();

        // Filter + decimate everything captured since the last tick, then append.
        while let Ok(block) = audio_rx.try_recv() {
            let decimated = decimator.process(&block);
            analyzer.push_block(&decimated);
        }

        let (frequency, spd) = analyzer.compute();
        if result_tx.send(TunerResult { frequency, spd }).is_err() {
            break; // UI hung up
        }

        if let Some(remaining) = tick.checked_sub(started.elapsed()) {
            thread::sleep(remaining);
        }
    }
}

/// Anti-alias filtering + decimation of the captured stream — the
/// computation-thread equivalent of the decimation half of lingot's
/// `lingot_core_read_callback`. Stateful: the IIR filter and the decimation
/// phase carry across blocks, so a single `Decimator` must process the whole
/// stream in order.
struct Decimator {
    oversampling: usize,
    antialias: Option<Filter>,
    /// Phase carried into the next block, for continuous downsampling.
    phase: usize,
    scratch: Vec<f64>,
}

impl Decimator {
    fn new(oversampling: u32) -> Self {
        // 8th-order Chebyshev low-pass at wc = 0.9 / oversampling (10% margin
        // below Nyquist) to prevent aliasing at decimation.
        let antialias =
            (oversampling > 1).then(|| Filter::cheby_design(8, 0.5, 0.9 / oversampling as f64));
        Decimator {
            oversampling: oversampling as usize,
            antialias,
            phase: 0,
            scratch: Vec::new(),
        }
    }

    /// Filter and downsample one captured block. With `oversampling == 1` it is
    /// a pass-through.
    fn process(&mut self, block: &[f64]) -> Vec<f64> {
        if self.oversampling <= 1 {
            return block.to_vec();
        }

        self.scratch.clear();
        self.scratch.extend_from_slice(block);
        if let Some(f) = &mut self.antialias {
            let input: Vec<f64> = self.scratch.clone();
            f.filter(&input, &mut self.scratch);
        }

        let mut out = Vec::with_capacity(self.scratch.len() / self.oversampling + 1);
        let mut i = self.phase;
        while i < self.scratch.len() {
            out.push(self.scratch[i]);
            i += self.oversampling;
        }
        self.phase = i - self.scratch.len();
        out
    }
}

/// Owns the temporal buffer and all per-analysis scratch state. Runs the
/// frequency-domain pipeline of `lingot_core_compute_fundamental_fequency`.
struct Analyzer {
    config: Config,
    /// Decimated sample rate (Hz): `sample_rate / oversampling`.
    decimated_rate: f64,
    temporal_buffer: Vec<f64>,
    window_temporal: Vec<f64>,
    window_fft: Vec<f64>,
    windowed_fft_buffer: Vec<f64>,
    windowed_temporal_buffer: Vec<f64>,
    fft_plan: FftPlan,
    /// SPD scratch: linear power → dB → SNR (length `fft_size / 2`).
    spd: Vec<f64>,
    locker: FrequencyLocker,
    last_freq: f64,
}

impl Analyzer {
    fn new(config: Config) -> Self {
        let fft_size = config.fft_size;
        let tbs = config.temporal_buffer_size.max(fft_size);
        let sample_rate = config.sample_rate.get::<hertz>();
        let decimated_rate = sample_rate / config.oversampling as f64;

        let (window_temporal, window_fft) = if config.window_type == WindowType::None {
            (Vec::new(), Vec::new())
        } else {
            (
                window::generate(tbs, config.window_type),
                window::generate(fft_size, config.window_type),
            )
        };

        Analyzer {
            decimated_rate,
            temporal_buffer: vec![0.0; tbs],
            window_temporal,
            window_fft,
            windowed_fft_buffer: vec![0.0; fft_size],
            windowed_temporal_buffer: vec![0.0; tbs],
            fft_plan: FftPlan::new(fft_size),
            spd: vec![0.0; fft_size / 2],
            locker: FrequencyLocker::new(),
            last_freq: 0.0,
            config,
        }
    }

    /// Append a block of decimated samples, shifting the temporal buffer like a
    /// queue (oldest samples fall off the front).
    fn push_block(&mut self, block: &[f64]) {
        let size = self.temporal_buffer.len();
        if block.is_empty() {
            return;
        }
        if block.len() >= size {
            self.temporal_buffer
                .copy_from_slice(&block[block.len() - size..]);
        } else {
            let shift = block.len();
            self.temporal_buffer.copy_within(shift.., 0);
            self.temporal_buffer[size - shift..].copy_from_slice(block);
        }
    }

    /// Run one analysis pass; returns `(frequency_hz, snr_spectrum_db)`.
    fn compute(&mut self) -> (f64, Vec<f64>) {
        let fft_size = self.config.fft_size;
        let tbs = self.temporal_buffer.len();
        let spd_size = fft_size / 2;
        let sample_rate = self.config.sample_rate.get::<hertz>();
        let oversampling = self.config.oversampling as f64;
        // FFT resolution in Hz.
        let index2f = self.decimated_rate / fft_size as f64;

        // --- windowing for the FFT (most recent fft_size samples) ---
        let tail = &self.temporal_buffer[tbs - fft_size..];
        if self.window_fft.is_empty() {
            self.windowed_fft_buffer.copy_from_slice(tail);
        } else {
            for (dst, (&s, &w)) in self
                .windowed_fft_buffer
                .iter_mut()
                .zip(tail.iter().zip(&self.window_fft))
            {
                *dst = s * w;
            }
        }

        // --- FFT → SPD (linear power), then to dB ---
        self.fft_plan
            .compute_spd(&self.windowed_fft_buffer, &mut self.spd);
        for v in &mut self.spd {
            *v = (10.0 * v.log10()).max(MIN_SPL_DB);
        }

        // --- noise floor subtraction → SNR spectrum ---
        let noise_width_samples = (NOISE_FILTER_WIDTH_HZ / index2f).ceil() as usize;
        let noise = compute_noise_level(&self.spd, noise_width_samples);
        for (s, n) in self.spd.iter_mut().zip(&noise) {
            *s -= n;
        }

        // --- coarse fundamental estimate over the spectrum ---
        let lowest_index =
            (self.config.internal_min_frequency.get::<hertz>() / index2f).ceil() as usize;
        let highest_index = (0.95 * spd_size as f64).ceil() as usize;

        let estimate = estimate_fundamental_frequency(
            &self.spd,
            0.5 * self.last_freq,
            self.fft_plan.spectrum(),
            self.config.peak_number,
            lowest_index,
            highest_index,
            self.config.peak_half_width,
            index2f,
            self.config.min_snr,
            self.config.min_overall_snr,
            self.config.internal_min_frequency.get::<hertz>(),
        );

        let (f0, divisor) = match estimate {
            Some(e) => (e.frequency, e.divisor as f64),
            None => (0.0, 1.0),
        };

        // angular frequency, rad per decimated sample
        let mut w = if f0 == 0.0 {
            0.0
        } else {
            2.0 * PI * f0 / self.decimated_rate
        };

        // --- window the whole temporal buffer for the high-resolution pass ---
        if w != 0.0 {
            if self.window_temporal.is_empty() {
                self.windowed_temporal_buffer
                    .copy_from_slice(&self.temporal_buffer);
            } else {
                for (dst, (&s, &win)) in self
                    .windowed_temporal_buffer
                    .iter_mut()
                    .zip(self.temporal_buffer.iter().zip(&self.window_temporal))
                {
                    *dst = s * win;
                }
            }

            // Newton-Raphson pass 1: the FFT window.
            w = newton_raphson(
                &self.windowed_fft_buffer,
                w,
                self.config.max_nr_iter,
                false,
            );

            // Pass 2: the full temporal window, for higher resolution.
            if w > 0.0 {
                w = newton_raphson(
                    &self.windowed_temporal_buffer,
                    w,
                    self.config.max_nr_iter,
                    true,
                );
            }
        }

        let freq = if w <= 0.0 {
            0.0
        } else {
            w * sample_rate / (divisor * 2.0 * PI * oversampling)
        };

        let locked = self
            .locker
            .process(freq, self.config.internal_min_frequency.get::<hertz>());
        self.last_freq = locked;

        (locked, self.spd.clone())
    }
}

/// Refine an angular-frequency estimate `w0` (rad/sample) by Newton-Raphson on
/// the analytic SPD power derivatives. Mirrors the two NR loops in
/// `lingot_core_compute_fundamental_fequency`. Returns 0.0 if the iteration
/// diverges (the SPD decreased). `min_two_iters` forces at least two iterations
/// (the second NR pass in the C original).
fn newton_raphson(buffer: &[f64], w0: f64, max_iter: usize, min_two_iters: bool) -> f64 {
    let mut wk = -1.0e5;
    let mut wkm1 = w0;
    let mut d0 = 0.0;
    let mut k = 0;

    loop {
        let force = min_two_iters && k <= 1;
        if !force && !(k < max_iter && (wk - wkm1).abs() > 1.0e-4) {
            break;
        }
        wk = wkm1;
        let d0_old = d0;
        let (nd0, d1, d2) = spd_diffs_eval_w(buffer, wk);
        d0 = nd0;
        wkm1 = wk - d1 / d2;
        if d0 < d0_old {
            return 0.0;
        }
        k += 1;
    }

    wkm1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq_hz: f64, sample_rate: f64, n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| (2.0 * PI * freq_hz / sample_rate * i as f64).sin())
            .collect()
    }

    #[test]
    fn decimator_passthrough_when_oversampling_one() {
        let mut d = Decimator::new(1);
        assert_eq!(d.process(&[1.0, 2.0, 3.0]), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn decimator_downsamples_and_carries_phase() {
        // oversampling 4: roughly one in four samples survives, and the phase
        // carries across block boundaries so the stream stays evenly sampled.
        let mut d = Decimator::new(4);
        let n_in = 400;
        let stream: Vec<f64> = (0..n_in).map(|i| i as f64).collect();

        // process in two halves; the total decimated count should match a
        // single-pass decimation (continuity across the boundary).
        let mut split = d.process(&stream[..150]);
        split.extend(d.process(&stream[150..]));

        let mut whole = Decimator::new(4);
        let single = whole.process(&stream);

        // Filtering differs at block edges, so compare counts, not values.
        assert_eq!(split.len(), single.len());
        assert!((split.len() as i64 - n_in / 4).abs() <= 1);
    }

    #[test]
    fn push_block_keeps_most_recent_samples() {
        let config = Config::default();
        let mut analyzer = Analyzer::new(config);
        let size = analyzer.temporal_buffer.len();

        // fill with a ramp longer than the buffer
        let ramp: Vec<f64> = (0..size + 10).map(|i| i as f64).collect();
        analyzer.push_block(&ramp);

        // buffer should hold the last `size` values of the ramp
        let expected_first = (size + 10 - size) as f64;
        assert_eq!(analyzer.temporal_buffer[0], expected_first);
        assert_eq!(
            *analyzer.temporal_buffer.last().unwrap(),
            (size + 10 - 1) as f64
        );
    }

    #[test]
    fn detects_frequency_of_pure_tone() {
        // Drive the analyzer directly in the decimated domain with a steady
        // tone inside the instrument range (E2..E4). Use 220 Hz (A3).
        let config = Config::default();
        let decimated_rate = config.sample_rate.get::<hertz>() / config.oversampling as f64;
        let tone = 220.0;

        let mut analyzer = Analyzer::new(config);
        let block = sine(tone, decimated_rate, analyzer.temporal_buffer.len());
        analyzer.push_block(&block);

        // The frequency locker needs several consistent readings before it
        // reports a value, so run a handful of passes on the steady tone.
        let mut freq = 0.0;
        for _ in 0..12 {
            let (f, _spd) = analyzer.compute();
            if f > 0.0 {
                freq = f;
            }
        }

        assert!(
            (freq - tone).abs() < 1.0,
            "detected {freq} Hz, expected ~{tone} Hz"
        );
    }
}
