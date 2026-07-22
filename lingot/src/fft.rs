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

use rustfft::{num_complex::Complex, FftPlanner};
use std::f64::consts::PI;
use std::sync::Arc;
use uom::si::f64::Frequency;
use uom::si::frequency::hertz;

pub struct FftPlan {
    fft: Arc<dyn rustfft::Fft<f64>>,
    buf: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
}

impl FftPlan {
    pub fn new(n: usize) -> Self {
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(n);
        let scratch_len = fft.get_inplace_scratch_len();
        Self {
            fft,
            buf: vec![Complex::new(0.0, 0.0); n],
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
        }
    }

    /// FFT size this plan was built for.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Always false in practice — a plan is built for a fixed non-zero size.
    /// Present because a public `len` without `is_empty` is a wart.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Full FFT → Spectral Power Distribution (normalised squared magnitude).
    /// `input` must be exactly `self.len()` samples.
    /// `output` receives the first `output.len()` bins; must be <= `self.len()`.
    pub fn compute_spd(&mut self, input: &[f64], output: &mut [f64]) {
        let n = self.buf.len();
        assert_eq!(input.len(), n, "input length must equal FFT size");
        assert!(output.len() <= n, "output length must not exceed FFT size");

        for (c, &x) in self.buf.iter_mut().zip(input) {
            *c = Complex::new(x, 0.0);
        }

        self.fft.process_with_scratch(&mut self.buf, &mut self.scratch);

        let scale = 1.0 / (n as f64 * n as f64);
        for (out, c) in output.iter_mut().zip(&self.buf) {
            *out = c.norm_sqr() * scale;
        }
    }

    /// The complex spectrum from the most recent [`compute_spd`](Self::compute_spd)
    /// call. Used for sub-bin peak interpolation (Quinn's estimator).
    pub fn spectrum(&self) -> &[Complex<f64>] {
        &self.buf
    }
}

/// Selective DTFT-based SPD evaluation at `output.len()` evenly-spaced frequencies.
///
/// Equivalent to `lingot_fft_spd_eval` — O(N1 * N2), used only over narrow
/// frequency ranges during peak refinement, not on the full spectrum.
pub fn spd_eval(
    input: &[f64],
    sample_rate: Frequency,
    f_start: Frequency,
    f_step: Frequency,
    output: &mut [f64],
) {
    let n = input.len();
    let scale = 1.0 / (n as f64 * n as f64);
    let fs = sample_rate.get::<hertz>();
    let wi = 2.0 * PI * f_start.get::<hertz>() / fs; // rad/sample
    let dw = 2.0 * PI * f_step.get::<hertz>() / fs;  // rad/sample

    for (i, out) in output.iter_mut().enumerate() {
        let w = wi + dw * i as f64;
        let (mut xr, mut xi) = (0.0f64, 0.0f64);
        for (n_idx, &x) in input.iter().enumerate() {
            let phase = w * n_idx as f64;
            xr += phase.cos() * x;
            xi -= phase.sin() * x;
        }
        *out = (xr * xr + xi * xi) * scale;
    }
}

/// SPD value and its first and second derivatives at frequency `f`.
///
/// Returns `(d0, d1, d2)`. Used by Newton-Raphson to refine a peak to
/// sub-bin precision — see `lingot_fft_spd_diffs_eval`.
pub fn spd_diffs_eval(input: &[f64], sample_rate: Frequency, f: Frequency) -> (f64, f64, f64) {
    let w = 2.0 * PI * f.get::<hertz>() / sample_rate.get::<hertz>(); // rad/sample
    spd_diffs_eval_w(input, w)
}

/// SPD value and its first and second derivatives w.r.t. the angular frequency
/// `w` (radians per sample). This is the form used directly by the core's
/// Newton-Raphson refinement and matches `lingot_fft_spd_diffs_eval`.
///
/// The derivatives `d1`, `d2` are taken with respect to `w`.
pub fn spd_diffs_eval_w(input: &[f64], w: f64) -> (f64, f64, f64) {
    let n = input.len() as f64;
    let n2 = n * n;

    let (mut sc, mut ss, mut snc, mut sns, mut sn2c, mut sn2s) =
        (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);

    for (idx, &x) in input.iter().enumerate() {
        let ni = idx as f64;
        let xc = x * (w * ni).cos();
        let xs = x * (w * ni).sin();
        sc += xc;
        ss += xs;
        snc += xc * ni;
        sns += xs * ni;
        sn2c += xc * ni * ni;
        sn2s += xs * ni * ni;
    }

    let d0 = (sc * sc + ss * ss) / n2;
    let d1 = 2.0 * (ss * snc - sc * sns) / n2;
    let d2 = 2.0 * (snc * snc - ss * sn2s + sns * sns - sc * sn2c) / n2;

    (d0, d1, d2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uom::si::frequency::hertz;

    const SAMPLE_RATE: f64 = 44100.0;

    fn sample_rate() -> Frequency {
        Frequency::new::<hertz>(SAMPLE_RATE)
    }

    fn sine_wave(freq_hz: f64, n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| (2.0 * PI * freq_hz / SAMPLE_RATE * i as f64).sin())
            .collect()
    }

    #[test]
    fn fft_peak_at_correct_bin() {
        let n = 4096;
        let freq = 440.0; // A4
        let samples = sine_wave(freq, n);

        let mut plan = FftPlan::new(n);
        let mut spd = vec![0.0f64; n / 2];
        plan.compute_spd(&samples, &mut spd);

        let peak_bin = spd[..n / 2]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();

        let expected_bin = (freq * n as f64 / SAMPLE_RATE).round() as usize;
        assert_eq!(peak_bin, expected_bin, "peak should be at bin {expected_bin}");
    }

    #[test]
    fn spd_eval_matches_fft_peak() {
        let n = 4096;
        let freq = 440.0;
        let samples = sine_wave(freq, n);

        // evaluate a narrow band around 440 Hz
        let f_start = Frequency::new::<hertz>(430.0);
        let f_step = Frequency::new::<hertz>(1.0);
        let mut out = vec![0.0f64; 20];
        spd_eval(&samples, sample_rate(), f_start, f_step, &mut out);

        let peak = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();

        // peak should be at bin 10 (430 + 10 = 440 Hz)
        assert_eq!(peak, 10, "narrow DTFT peak should be at 440 Hz");
    }

    #[test]
    fn spd_diffs_d1_d2_match_numerical_derivatives() {
        // Validate that the analytical d1 and d2 agree with central-difference
        // estimates computed from d0 evaluations at nearby frequencies.
        let n = 4096;
        let freq = 440.0;
        let samples = sine_wave(freq, n);
        let fs = sample_rate();

        let eps_hz = 0.5_f64;
        let f      = Frequency::new::<hertz>(freq);
        let f_plus = Frequency::new::<hertz>(freq + eps_hz);
        let f_minus = Frequency::new::<hertz>(freq - eps_hz);

        let (d0_plus, d1_plus, _) = spd_diffs_eval(&samples, fs, f_plus);
        let (d0_minus, d1_minus, _) = spd_diffs_eval(&samples, fs, f_minus);
        let (_, d1, d2) = spd_diffs_eval(&samples, fs, f);

        // d1 = d(d0)/dw; convert finite difference from Hz to rad/sample
        let dw_per_hz = 2.0 * PI / SAMPLE_RATE;
        let numerical_d1 = (d0_plus - d0_minus) / (2.0 * eps_hz * dw_per_hz);
        let numerical_d2 = (d1_plus - d1_minus) / (2.0 * eps_hz * dw_per_hz);

        let d1_err = (d1 - numerical_d1).abs() / d1.abs().max(numerical_d1.abs());
        let d2_err = (d2 - numerical_d2).abs() / d2.abs().max(numerical_d2.abs());

        // Central-difference truncation is O(eps²); 1% agreement is sufficient.
        assert!(d1_err < 1e-2, "d1 relative error {d1_err:.2e}: got {d1}, numerical {numerical_d1}");
        assert!(d2_err < 1e-2, "d2 relative error {d2_err:.2e}: got {d2}, numerical {numerical_d2}");
    }
}
