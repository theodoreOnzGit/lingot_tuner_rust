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

//! Digital IIR filtering and Chebyshev Type I design, mirroring
//! `lingot-filter.{c,h}`.

use num_complex::Complex64;
use std::f64::consts::PI;

/// A digital IIR filter in Direct Form II.
///
/// Holds feedforward (`b`) and feedback (`a`) coefficients plus the delay-line
/// state. Filtering mutates the state, so this is inherently stateful — each
/// independent signal needs its own `Filter` (or a `reset` between runs).
#[derive(Clone, Debug)]
pub struct Filter {
    a: Vec<f64>,
    b: Vec<f64>,
    s: Vec<f64>,
    n: usize,
}

impl Filter {
    /// Build a filter from feedback coefficients `a` and feedforward
    /// coefficients `b`. The order is `max(a.len(), b.len()) - 1`; both
    /// coefficient sets are zero-padded to that order and normalised by `a[0]`.
    pub fn new(a: &[f64], b: &[f64]) -> Self {
        assert!(!a.is_empty() && !b.is_empty(), "coefficients must be non-empty");
        assert!(a[0] != 0.0, "a[0] must be non-zero");

        let order_a = a.len() - 1;
        let order_b = b.len() - 1;
        let n = order_a.max(order_b);

        let mut ca = vec![0.0; n + 1];
        let mut cb = vec![0.0; n + 1];
        ca[..a.len()].copy_from_slice(a);
        cb[..b.len()].copy_from_slice(b);

        let a0 = a[0];
        for i in 0..=n {
            ca[i] /= a0;
            cb[i] /= a0;
        }

        Filter {
            a: ca,
            b: cb,
            s: vec![0.0; n + 1],
            n,
        }
    }

    /// Clear the delay-line state.
    pub fn reset(&mut self) {
        self.s.iter_mut().for_each(|x| *x = 0.0);
    }

    /// Filter `input` into `output` (Direct Form II). `input` and `output`
    /// must have equal length and may alias the same buffer.
    pub fn filter(&mut self, input: &[f64], output: &mut [f64]) {
        assert_eq!(input.len(), output.len(), "input/output length mismatch");

        for (xi, yi) in input.iter().zip(output.iter_mut()) {
            let mut w = *xi;
            let mut y = 0.0;

            for j in (0..self.n).rev() {
                w -= self.a[j + 1] * self.s[j];
                y += self.b[j + 1] * self.s[j];
                self.s[j + 1] = self.s[j];
            }

            y += w * self.b[0];
            self.s[0] = w;
            *yi = y;
        }
    }

    /// Filter a single sample.
    pub fn filter_sample(&mut self, input: f64) -> f64 {
        let mut out = [0.0];
        self.filter(&[input], &mut out);
        out[0]
    }

    /// Design a Chebyshev Type I low-pass filter of the given `order` with
    /// `rp_db` of pass-band ripple and normalised cutoff `wc` (in units of π
    /// rad/sample, i.e. fraction of the Nyquist frequency).
    ///
    /// Port of `lingot_filter_cheby_design`. Uses signed integers for the pole
    /// angle (the C original relied on unsigned wraparound, which works only by
    /// the periodicity of sin/cos and loses some precision).
    pub fn cheby_design(order: usize, rp_db: f64, wc: f64) -> Self {
        let n = order;
        let t = 2.0;

        // pre-warped analogue cutoff
        let w = 2.0 / t * (PI * wc / t).tan();

        let epsilon = (10.0_f64.powf(0.1 * rp_db) - 1.0).sqrt();
        let v0 = (1.0 / epsilon).asinh() / n as f64;
        let sv0 = v0.sinh();
        let cv0 = v0.cosh();

        // locate analogue poles on the Chebyshev ellipse
        let mut pole = vec![Complex64::new(0.0, 0.0); n];
        let mut idx: i32 = -((n as i32) - 1);
        for p in pole.iter_mut() {
            let angle = PI * idx as f64 / (2.0 * n as f64);
            *p = Complex64::new(-sv0 * angle.cos(), cv0 * angle.sin());
            idx += 2;
        }

        let mut gain = vector_product(&pole);

        if n & 1 == 0 {
            gain *= 10.0_f64.powf(-0.05 * rp_db);
        }
        gain *= Complex64::new(w.powi(n as i32), 0.0);

        for p in pole.iter_mut() {
            *p *= w;
        }

        // bilinear transform
        let sp: Vec<Complex64> = pole.iter().map(|p| Complex64::new(2.0 / t, 0.0) - p).collect();
        gain /= vector_product(&sp);

        for p in pole.iter_mut() {
            let num = Complex64::new(2.0, 0.0) + *p * t;
            let den = Complex64::new(2.0, 0.0) - *p * t;
            *p = num / den;
        }

        // expand pole/zero form into polynomial coefficients
        let mut a = vec![0.0; n + 1];
        let mut b = vec![0.0; n + 1];
        a[0] = 1.0;
        b[0] = 1.0;

        if n & 1 == 1 {
            // odd order: leading first-order subfilter from the real pole
            a[1] = -pole[n / 2].re;
            b[1] = 1.0;
        }

        let mut new_a = vec![0.0; n + 1];
        let mut new_b = vec![0.0; n + 1];
        new_a[0] = 1.0;
        new_b[0] = 1.0;

        // multiply in each conjugate pole pair as a 2nd-order section
        for p in pole.iter().take(n / 2) {
            let b1 = 2.0;
            let b2 = 1.0;
            let a1 = -2.0 * p.re;
            let a2 = p.re * p.re + p.im * p.im;

            new_a[1] = a[1] + a1 * a[0];
            new_b[1] = b[1] + b1 * b[0];
            for i in 2..=n {
                new_a[i] = a[i] + a1 * a[i - 1] + a2 * a[i - 2];
                new_b[i] = b[i] + b1 * b[i - 1] + b2 * b[i - 2];
            }
            a[1..=n].copy_from_slice(&new_a[1..=n]);
            b[1..=n].copy_from_slice(&new_b[1..=n]);
        }

        let g = gain.re.abs();
        for bi in b.iter_mut() {
            *bi *= g;
        }

        Filter::new(&a, &b)
    }
}

/// Product of the negated elements: `∏ (-v[i])`. Mirrors
/// `lingot_filter_vector_product`.
fn vector_product(v: &[Complex64]) -> Complex64 {
    v.iter().fold(Complex64::new(1.0, 0.0), |acc, x| acc * (-x))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_gain_doubles_input() {
        let mut f = Filter::new(&[1.0], &[2.0]);
        assert_eq!(f.filter_sample(3.0), 6.0);
        assert_eq!(f.filter_sample(-1.5), -3.0);
    }

    #[test]
    fn first_order_impulse_response() {
        // y[n] = 0.5 x[n] + 0.5 x[n-1] (moving average), a = [1], b = [0.5, 0.5]
        let mut f = Filter::new(&[1.0], &[0.5, 0.5]);
        let input = [1.0, 0.0, 0.0, 0.0];
        let mut out = [0.0; 4];
        f.filter(&input, &mut out);
        assert_eq!(out, [0.5, 0.5, 0.0, 0.0]);
    }

    #[test]
    fn reset_clears_state() {
        let mut f = Filter::new(&[1.0], &[0.5, 0.5]);
        f.filter_sample(1.0);
        f.reset();
        // after reset the delayed sample is gone, so a fresh impulse behaves
        // as the first sample again
        assert_eq!(f.filter_sample(1.0), 0.5);
    }

    #[test]
    fn in_place_filtering() {
        let mut f = Filter::new(&[1.0], &[0.5, 0.5]);
        let mut buf = [1.0, 0.0, 0.0, 0.0];
        let copy = buf;
        f.filter(&copy, &mut buf);
        assert_eq!(buf, [0.5, 0.5, 0.0, 0.0]);
    }

    #[test]
    fn cheby_lowpass_passes_dc() {
        // 8th-order, 0.5 dB ripple, cutoff at half-Nyquist
        let mut f = Filter::cheby_design(8, 0.5, 0.5);
        // drive with a DC input until steady state
        let mut last = 0.0;
        for _ in 0..2000 {
            last = f.filter_sample(1.0);
        }
        // even-order Chebyshev I dips to -Rp dB at DC: 10^(-0.5/20) ≈ 0.944
        assert!((0.9..=1.0).contains(&last), "DC gain = {last}");
    }

    #[test]
    fn cheby_lowpass_rejects_nyquist() {
        let mut f = Filter::cheby_design(8, 0.5, 0.5);
        // Nyquist input: alternating +1 / -1
        let mut peak = 0.0_f64;
        for n in 0..4000 {
            let x = if n % 2 == 0 { 1.0 } else { -1.0 };
            let y = f.filter_sample(x);
            if n > 2000 {
                peak = peak.max(y.abs());
            }
        }
        assert!(peak < 1e-2, "Nyquist leakage = {peak}");
    }

    #[test]
    fn cheby_dc_gain_via_coeffs() {
        // DC gain of an IIR filter is sum(b)/sum(a).
        let f = Filter::cheby_design(8, 0.5, 0.5);
        let sum_b: f64 = f.b.iter().sum();
        let sum_a: f64 = f.a.iter().sum();
        let dc_gain = sum_b / sum_a;
        assert!((0.9..=1.0).contains(&dc_gain), "DC gain = {dc_gain}");
    }
}
