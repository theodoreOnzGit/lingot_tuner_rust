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

//! Analysis windows, mirroring `lingot_signal_window`.

use std::f64::consts::PI;

/// Analysis window applied to a frame before the FFT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowType {
    None,
    Hanning,
    Hamming,
}

/// Generate an `n`-sample window. `WindowType::None` yields all-ones (a no-op
/// when multiplied into a frame).
///
/// Uses lingot's *optimal* Hamming coefficients (0.53836 / 0.46164), which
/// differ slightly from the classic 0.54 / 0.46 used by some libraries.
pub fn generate(n: usize, window_type: WindowType) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![1.0];
    }

    let denom = (n - 1) as f64;
    (0..n)
        .map(|i| {
            let phase = 2.0 * PI * i as f64 / denom;
            match window_type {
                WindowType::None => 1.0,
                WindowType::Hanning => 0.5 * (1.0 - phase.cos()),
                WindowType::Hamming => 0.53836 - 0.46164 * phase.cos(),
            }
        })
        .collect()
}

/// Multiply a frame in place by a precomputed window. Lengths must match.
pub fn apply(frame: &mut [f64], window: &[f64]) {
    assert_eq!(frame.len(), window.len(), "frame and window length mismatch");
    for (x, w) in frame.iter_mut().zip(window) {
        *x *= w;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_is_all_ones() {
        let w = generate(8, WindowType::None);
        assert!(w.iter().all(|&x| x == 1.0));
    }

    #[test]
    fn hanning_endpoints_are_zero() {
        let w = generate(16, WindowType::Hanning);
        assert!(w[0].abs() < 1e-12);
        assert!(w[15].abs() < 1e-12);
        // peak near the centre is ~1
        assert!(w[8] > 0.95);
    }

    #[test]
    fn hamming_endpoints_match_optimal_coeffs() {
        let w = generate(16, WindowType::Hamming);
        // 0.53836 - 0.46164 = 0.07672
        assert!((w[0] - 0.07672).abs() < 1e-9);
        assert!((w[15] - 0.07672).abs() < 1e-9);
    }

    #[test]
    fn window_is_symmetric() {
        let w = generate(32, WindowType::Hanning);
        for i in 0..16 {
            assert!((w[i] - w[31 - i]).abs() < 1e-12);
        }
    }

    #[test]
    fn apply_multiplies_in_place() {
        let mut frame = vec![2.0; 4];
        let window = vec![0.5, 1.0, 1.0, 0.5];
        apply(&mut frame, &window);
        assert_eq!(frame, vec![1.0, 2.0, 2.0, 1.0]);
    }
}
