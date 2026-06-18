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

//! Core tuner configuration, mirroring `lingot-config.{c,h}`.
//!
//! Holds the user-facing parameters that drive the tuner, plus the internal
//! parameters derived from them by [`Config::update_internal_params`].

use uom::si::f64::{Frequency, Time};
use uom::si::frequency::hertz;
use uom::si::time::second;

use crate::scale::Scale;

/// Analysis window applied before the FFT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowType {
    None,
    Hanning,
    Hamming,
}

#[derive(Clone, Debug)]
pub struct Config {
    // ---- user-facing parameters ----------------------------------------

    /// Selected input device; `None` uses the system default (cpal).
    pub audio_device: Option<String>,
    /// Hardware sample rate.
    pub sample_rate: Frequency,
    /// Deviation of the root frequency.
    pub root_frequency_error: Frequency,
    /// Lowest frequency of the instrument.
    pub min_frequency: Frequency,
    /// Highest frequency of the instrument.
    pub max_frequency: Frequency,
    /// Whether fft_size / temporal_window are governed automatically.
    pub optimize_internal_parameters: bool,
    /// Number of samples of the FFT.
    pub fft_size: usize,
    /// Duration of the temporal window.
    pub temporal_window: Time,
    /// Rate at which the fundamental frequency is recomputed.
    pub calculation_rate: Frequency,
    /// Minimum SNR required for the overall set of peaks (dB).
    pub min_overall_snr: f64,
    pub window_type: WindowType,
    /// Number of maximum peaks considered.
    pub peak_number: usize,
    /// Max iterations for the Newton-Raphson refinement.
    pub max_nr_iter: usize,

    // ---- derived / internal parameters ---------------------------------

    /// Oversampling (decimation) factor.
    pub oversampling: u32,
    /// Lowest valid frequency.
    pub internal_min_frequency: Frequency,
    /// Highest frequency we want to tune.
    pub internal_max_frequency: Frequency,
    /// Samples stored in the temporal window.
    pub temporal_buffer_size: usize,
    /// Minimum SNR required for each peak (dB).
    pub min_snr: f64,
    /// Adjacent samples needed to consider a peak.
    pub peak_half_width: usize,
    /// Global range for the gauge, in cents.
    pub gauge_range: f64,
    /// Gauge rest value, in cents.
    pub gauge_rest_value: f64,

    pub scale: Scale,
}

impl Config {
    /// Derive the internal parameters from the user-facing ones.
    ///
    /// Mirrors `lingot_config_update_internal_params`. Faithful to the C
    /// original, including its use of the *current* `fft_size` (not the
    /// suggested one) when computing the suggested temporal window.
    pub fn update_internal_params(&mut self) {
        let sample_rate = self.sample_rate.get::<hertz>();

        let mut internal_min = 0.8 * self.min_frequency.get::<hertz>();
        let mut internal_max = 3.1 * self.max_frequency.get::<hertz>();
        if internal_min < 0.0 {
            internal_min = 0.0;
        }
        internal_max = internal_max.clamp(500.0, 20000.0);
        self.internal_min_frequency = Frequency::new::<hertz>(internal_min);
        self.internal_max_frequency = Frequency::new::<hertz>(internal_max);

        self.oversampling = (0.5 * sample_rate / internal_max).floor().max(1.0) as u32;

        let suggested_fft_size = if internal_max > 5000.0 { 1024 } else { 512 };
        let mut suggested_window =
            self.fft_size as f64 * self.oversampling as f64 / sample_rate;
        if suggested_window < 0.3 {
            suggested_window = 0.3;
        }

        if self.optimize_internal_parameters {
            self.fft_size = suggested_fft_size;
            self.temporal_window = Time::new::<second>(suggested_window);
        } else {
            // If the configured parameters already match what we'd suggest,
            // mark them as optimal.
            self.optimize_internal_parameters = self.fft_size == suggested_fft_size
                && self.temporal_window.get::<second>() == suggested_window;
        }

        self.temporal_buffer_size = (self.temporal_window.get::<second>() * sample_rate
            / self.oversampling as f64)
            .ceil() as usize;

        self.min_snr = 0.5 * self.min_overall_snr;
        self.peak_half_width = if self.fft_size > 256 { 2 } else { 1 };

        let mut gauge_range = 1200.0;
        let notes = self.scale.notes();
        for i in 1..notes.len() {
            let offset = notes[i].offset_cents - notes[i - 1].offset_cents;
            if offset < gauge_range {
                gauge_range = offset;
            }
        }
        self.gauge_range = gauge_range;
        self.gauge_rest_value = -0.45 * gauge_range;
    }
}

impl Default for Config {
    /// The default configuration, matching
    /// `lingot_config_restore_default_values` (followed by the internal-param
    /// derivation). Audio-system selection is left to the cpal layer.
    fn default() -> Self {
        let mut config = Config {
            audio_device: None,
            sample_rate: Frequency::new::<hertz>(44100.0),
            root_frequency_error: Frequency::new::<hertz>(0.0),
            min_frequency: Frequency::new::<hertz>(82.407),    // E2
            max_frequency: Frequency::new::<hertz>(329.6276),  // E4
            optimize_internal_parameters: false,
            fft_size: 512,
            temporal_window: Time::new::<second>(0.3),
            calculation_rate: Frequency::new::<hertz>(15.0),
            min_overall_snr: 20.0,
            window_type: WindowType::Hamming,
            peak_number: 8,
            max_nr_iter: 10,

            // overwritten by update_internal_params:
            oversampling: 21,
            internal_min_frequency: Frequency::new::<hertz>(0.0),
            internal_max_frequency: Frequency::new::<hertz>(0.0),
            temporal_buffer_size: 0,
            min_snr: 0.0,
            peak_half_width: 1,
            gauge_range: 0.0,
            gauge_rest_value: 0.0,

            scale: Scale::default(),
        };
        config.update_internal_params();
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_derives_internal_params() {
        let config = Config::default();

        // internal_max = 3.1 * 329.6276 = 1021.84..., above the 500 floor.
        let internal_max = config.internal_max_frequency.get::<hertz>();
        assert!((internal_max - 3.1 * 329.6276).abs() < 1e-6);

        // internal_min = 0.8 * 82.407
        let internal_min = config.internal_min_frequency.get::<hertz>();
        assert!((internal_min - 0.8 * 82.407).abs() < 1e-6);

        // oversampling = floor(0.5 * 44100 / internal_max)
        let expected_oversampling = (0.5 * 44100.0 / internal_max).floor() as u32;
        assert_eq!(config.oversampling, expected_oversampling);

        // min_snr is half of min_overall_snr
        assert_eq!(config.min_snr, 10.0);

        // fft_size 512 → peak_half_width 2
        assert_eq!(config.peak_half_width, 2);
    }

    #[test]
    fn gauge_range_for_equal_tempered_is_100_cents() {
        let config = Config::default();
        assert!((config.gauge_range - 100.0).abs() < 1e-9);
        assert!((config.gauge_rest_value + 45.0).abs() < 1e-9);
    }

    #[test]
    fn temporal_buffer_size_is_positive() {
        let config = Config::default();
        assert!(config.temporal_buffer_size > 0);
    }
}
