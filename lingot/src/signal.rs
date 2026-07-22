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

//! Peak identification, harmonic analysis and frequency stabilisation,
//! mirroring `lingot-signal.{c,h}`.

use num_complex::Complex64;

use crate::filter::Filter;

/// True if `signal[index]` is a local maximum over a half-window of
/// `peak_half_width` samples on each side. Mirrors `lingot_signal_is_peak`.
fn is_peak(signal: &[f64], index: usize, peak_half_width: usize) -> bool {
    for j in 0..peak_half_width {
        if signal[index + j] < signal[index + j + 1] || signal[index - j] < signal[index - j - 1] {
            return false;
        }
    }
    true
}

/// Quinn's second estimator helper τ(x). Mirrors
/// `lingot_signal_fft_bin_interpolate_quinn2_tau`.
fn quinn2_tau(x: f64) -> f64 {
    0.25 * (3.0 * x * x + 6.0 * x + 1.0).ln()
        - 0.102062072615966
            * ((x + 1.0 - 0.816496580927726) / (x + 1.0 + 0.816496580927726)).ln()
}

/// Quinn's second estimator: sub-bin offset of a spectral peak, given the
/// complex FFT values at the peak bin (`y2`) and its neighbours (`y1`, `y3`).
fn quinn2_interpolate(y1: Complex64, y2: Complex64, y3: Complex64) -> f64 {
    let ap = (y3 / y2).re;
    let dp = -ap / (1.0 - ap);
    let am = (y1 / y2).re;
    let dm = am / (1.0 - am);
    0.5 * (dp + dm) + quinn2_tau(dp * dp) - quinn2_tau(dm * dm)
}

/// Weighting that gives slightly more importance to higher fundamental
/// frequencies (favours higher divisors for the same candidate set). Mirrors
/// `lingot_signal_frequency_penalty`.
fn frequency_penalty(freq: f64) -> f64 {
    const F0: f64 = 100.0;
    const F1: f64 = 1000.0;
    const ALPHA0: f64 = 0.99;
    const ALPHA1: f64 = 1.0;

    let a = (ALPHA0 - ALPHA1) / (F0 - F1);
    let b = -(ALPHA0 * F1 - F0 * ALPHA1) / (F0 - F1);
    freq * a + b
}

/// Test whether two frequencies are harmonically related. Returns the
/// multipliers `(mul1, mul2)` that bring each to the common ground frequency,
/// or `None` if unrelated (or either frequency is non-positive). Mirrors
/// `lingot_signal_frequencies_related`.
pub fn frequencies_related(freq1: f64, freq2: f64, min_frequency: f64) -> Option<(f64, f64)> {
    const TOL: f64 = 5e-2;
    const MAX_DIVISOR: i32 = 4;

    if freq1 == 0.0 || freq2 == 0.0 {
        return None;
    }

    let (small_freq, big_freq) = if freq2 < freq1 {
        (freq2, freq1)
    } else {
        (freq1, freq2)
    };

    for divisor in 1..=MAX_DIVISOR {
        if min_frequency * divisor as f64 > small_freq {
            break;
        }
        let frac = big_freq * divisor as f64 / small_freq;
        if (frac - frac.round()).abs() < TOL {
            return Some(if small_freq == freq1 {
                (1.0 / divisor as f64, 1.0 / frac.round())
            } else {
                (1.0 / frac.round(), 1.0 / divisor as f64)
            });
        }
    }

    None
}

/// Noise-floor estimate of an SPD, via a low-pass IIR run forward. `prime_len`
/// samples are filtered first to settle the filter state before the output
/// pass. Mirrors `lingot_signal_compute_noise_level`.
pub fn compute_noise_level(spd: &[f64], prime_len: usize) -> Vec<f64> {
    const C: f64 = 0.1;
    let mut filter = Filter::new(&[1.0, C - 1.0], &[C]);

    let prime = prime_len.min(spd.len());
    let mut scratch = vec![0.0; prime];
    filter.filter(&spd[..prime], &mut scratch);

    let mut noise = vec![0.0; spd.len()];
    filter.filter(spd, &mut noise);
    noise
}

/// Result of [`estimate_fundamental_frequency`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FundamentalEstimate {
    pub frequency: f64,
    /// Which harmonic of the fundamental the strongest peak corresponds to.
    pub divisor: i16,
}

/// Estimate the fundamental frequency from an SNR spectrum and the complex FFT.
///
/// Finds the strongest peaks above `min_snr`, refines each to sub-bin precision
/// with Quinn's estimator, then scores every (candidate, divisor) pair by how
/// many other peaks are integer harmonics of the implied ground frequency.
/// Mirrors `lingot_signal_estimate_fundamental_frequency`.
///
/// - `freq`: previously detected frequency (0 if none), used to boost peaks
///   that land near its harmonics.
/// - Returns `None` if no fundamental is found or the best quality is below
///   `min_q`.
#[allow(clippy::too_many_arguments)]
pub fn estimate_fundamental_frequency(
    snr: &[f64],
    freq: f64,
    fft: &[Complex64],
    n_peaks: usize,
    lowest_index: usize,
    highest_index: usize,
    peak_half_width: usize,
    delta_f_fft: f64,
    min_snr: f64,
    min_q: f64,
    min_freq: f64,
) -> Option<FundamentalEstimate> {
    let big_n = snr.len();

    let lowest = lowest_index.max(peak_half_width);
    let highest = if peak_half_width + highest_index > big_n {
        big_n - peak_half_width
    } else {
        highest_index
    };

    // Collect up to `n_peaks` strongest peaks. `p_index[k] == -1` marks a free
    // slot; `magnitude` holds the (boosted) SNR that won the slot.
    let mut p_index = vec![-1_i32; n_peaks];
    let mut magnitude = vec![0.0_f64; n_peaks];
    let mut n_found_peaks = 0usize;

    for i in lowest..highest {
        let mut factor = 1.0;
        if freq != 0.0 {
            let f = i as f64 * delta_f_fft;
            if (f / freq - (f / freq).round()).abs() < 0.07 {
                factor = 1.5;
            }
        }
        let snri = snr[i] * factor;

        if snri > min_snr && is_peak(snr, i, peak_half_width) {
            // find a free slot, else the weakest existing peak
            let mut m = 0;
            for j in 0..n_peaks {
                if p_index[j] == -1 {
                    m = j;
                    break;
                }
                if magnitude[j] < magnitude[m] {
                    m = j;
                }
            }

            // take the slot if it is free, or if this peak beats the one already
            // there. The `-1` test must stay first: short-circuiting is what
            // stops the sentinel being used as an index.
            if p_index[m] == -1 || snr[i] > snr[p_index[m] as usize] {
                p_index[m] = i as i32;
                magnitude[m] = snri;
            }

            if n_found_peaks < n_peaks {
                n_found_peaks += 1;
            }
        }
    }

    if n_found_peaks == 0 {
        return None;
    }

    let maximum = magnitude[..n_found_peaks]
        .iter()
        .zip(&p_index[..n_found_peaks])
        .filter(|(_, &p)| p != -1)
        .map(|(&m, _)| m)
        .fold(0.0_f64, f64::max);

    // discard peaks far below the maximum, then sort the survivors by bin
    let mut delete_counter = 0;
    for i in 0..n_found_peaks {
        if p_index[i] == -1 || magnitude[i] < maximum - 20.0 {
            p_index[i] = big_n as i32; // sentinel: sorts to the end
            delete_counter += 1;
        }
    }
    p_index[..n_found_peaks].sort_unstable();
    n_found_peaks -= delete_counter;

    if n_found_peaks == 0 {
        return None;
    }

    // sub-bin frequency of each surviving peak
    let freq_interpolated: Vec<f64> = (0..n_found_peaks)
        .map(|i| {
            let pi = p_index[i] as usize;
            let delta = quinn2_interpolate(fft[pi - 1], fft[pi], fft[pi + 1]);
            delta_f_fft * (pi as f64 + delta)
        })
        .collect();

    const RATIO_TOL: f64 = 0.02;
    const MAX_DIVISOR: i32 = 4;

    let mut best_q = 0.0;
    let mut best_f = 0.0;
    let mut best_divisor = 1_i16;

    for &candidate in freq_interpolated.iter() {
        for div in 1..=MAX_DIVISOR {
            let ground_freq = candidate / div as f64;
            if ground_freq <= min_freq {
                break;
            }

            // peaks that are integer harmonics of this ground frequency
            let indices_related: Vec<usize> = (0..n_found_peaks)
                .filter(|&i| {
                    let ratio = freq_interpolated[i] / ground_freq;
                    (ratio - ratio.round()).abs() < RATIO_TOL
                })
                .collect();

            if indices_related.is_empty() {
                continue;
            }

            let mut q = 0.0;
            let mut highest_harmonic_k = 0;
            let mut highest_harmonic_magnitude = 0.0;
            for (k, &ir) in indices_related.iter().enumerate() {
                let s = snr[p_index[ir] as usize];
                q += s * frequency_penalty(ground_freq);
                if s > highest_harmonic_magnitude {
                    highest_harmonic_k = k;
                    highest_harmonic_magnitude = s;
                }
            }

            let f = freq_interpolated[indices_related[highest_harmonic_k]];
            if q > best_q {
                best_q = q;
                best_divisor = (f / ground_freq).round() as i16;
                best_f = f;
            }
        }
    }

    if best_f == 0.0 || best_q < min_q {
        return None;
    }

    Some(FundamentalEstimate {
        frequency: best_f,
        divisor: best_divisor,
    })
}

/// State machine that stabilises a noisy stream of frequency estimates.
///
/// Requires several consistent readings before it "locks" onto a frequency,
/// and several failures before it unlocks. Handles octave-jump artifacts.
/// Mirrors the function-`static` state of `lingot_signal_frequency_locker`.
#[derive(Clone, Debug)]
pub struct FrequencyLocker {
    locked: bool,
    current_frequency: f64,
    hits_counter: i32,
    rehits_counter: i32,
    rehits_up_counter: i32,
    old_multiplier: f64,
    old_multiplier2: f64,
}

impl Default for FrequencyLocker {
    fn default() -> Self {
        FrequencyLocker {
            locked: false,
            current_frequency: -1.0,
            hits_counter: 0,
            rehits_counter: 0,
            rehits_up_counter: 0,
            old_multiplier: 0.0,
            old_multiplier2: 0.0,
        }
    }
}

impl FrequencyLocker {
    const NHITS_TO_LOCK: i32 = 4;
    const NHITS_TO_UNLOCK: i32 = 5;
    const NHITS_TO_RELOCK: i32 = 6;
    const NHITS_TO_RELOCK_UP: i32 = 8;

    pub fn new() -> Self {
        Self::default()
    }

    /// Feed the latest raw frequency estimate; returns the locked frequency
    /// (0 while unlocked or once a lock is lost).
    pub fn process(&mut self, freq: f64, min_frequency: f64) -> f64 {
        let (mut consistent, mut multiplier, mut multiplier2) =
            match frequencies_related(freq, self.current_frequency, min_frequency) {
                Some((m1, m2)) => (true, m1, m2),
                None => (false, 0.0, 0.0),
            };

        let mut fail = false;
        let mut result = 0.0;

        if !self.locked {
            if freq > 0.0 && self.current_frequency == 0.0 {
                consistent = true;
                multiplier = 1.0;
                multiplier2 = 1.0;
            }

            if consistent && multiplier == 1.0 && multiplier2 == 1.0 {
                self.current_frequency = freq * multiplier;
                self.hits_counter += 1;
                if self.hits_counter >= Self::NHITS_TO_LOCK {
                    self.locked = true;
                    self.hits_counter = 0;
                }
            } else {
                self.hits_counter = 0;
                self.current_frequency = 0.0;
            }
        } else if consistent {
            if (multiplier2 - 1.0).abs() < 1e-5 {
                result = freq * multiplier;
                self.current_frequency = result;
                self.rehits_counter = 0;

                if (multiplier - 1.0).abs() > 1e-5 {
                    if (multiplier - self.old_multiplier).abs() < 1e-5 {
                        self.rehits_up_counter += 1;
                        if self.rehits_up_counter >= Self::NHITS_TO_RELOCK_UP {
                            result = freq;
                            self.current_frequency = result;
                            self.rehits_up_counter = 0;
                        }
                    } else {
                        self.rehits_up_counter = 0;
                    }
                } else {
                    self.rehits_up_counter = 0;
                }
            } else {
                self.rehits_up_counter = 0;
                if (multiplier2 - 0.5).abs() < 1e-5 {
                    self.hits_counter -= 1;
                }
                fail = true;
                if freq * multiplier >= min_frequency
                    && (multiplier2 - self.old_multiplier2).abs() < 1e-5
                {
                    self.rehits_counter += 1;
                    if self.rehits_counter >= Self::NHITS_TO_RELOCK {
                        result = freq * multiplier;
                        self.current_frequency = result;
                        self.rehits_counter = 0;
                        fail = false;
                    }
                }
            }
        } else {
            fail = true;
        }

        if self.locked {
            if fail {
                result = self.current_frequency;
                self.hits_counter += 1;
                if self.hits_counter >= Self::NHITS_TO_UNLOCK {
                    self.current_frequency = 0.0;
                    self.locked = false;
                    self.hits_counter = 0;
                    result = 0.0;
                }
            } else {
                self.hits_counter = 0;
            }
        }

        self.old_multiplier = multiplier;
        self.old_multiplier2 = multiplier2;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octave_is_related() {
        // 440 is the 2nd harmonic of 220
        let rel = frequencies_related(220.0, 440.0, 50.0);
        assert_eq!(rel, Some((1.0, 0.5)));
    }

    #[test]
    fn unison_is_related() {
        let rel = frequencies_related(440.0, 440.0, 50.0);
        assert_eq!(rel, Some((1.0, 1.0)));
    }

    #[test]
    fn unrelated_frequencies() {
        assert_eq!(frequencies_related(440.0, 567.0, 50.0), None);
    }

    #[test]
    fn zero_frequency_is_unrelated() {
        assert_eq!(frequencies_related(440.0, 0.0, 50.0), None);
    }

    #[test]
    fn noise_level_of_flat_spd_is_flat() {
        let spd = vec![1.0; 100];
        let noise = compute_noise_level(&spd, 50);
        // after priming, the IIR (DC gain 1) tracks the flat input
        for &v in &noise[50..] {
            assert!((v - 1.0).abs() < 1e-3, "noise = {v}");
        }
    }

    #[test]
    fn estimate_recovers_fundamental_from_harmonics() {
        // N = 64 bins, 100 Hz/bin. Fundamental 300 Hz (bin 3) with harmonics
        // at 600 (bin 6) and 900 (bin 9).
        let n = 64;
        let delta_f = 100.0;
        let mut snr = vec![0.0; n];
        snr[3] = 30.0;
        snr[6] = 25.0;
        snr[9] = 20.0;

        // symmetric real neighbours → Quinn offset is ~0
        let mut fft = vec![Complex64::new(0.0, 0.0); n];
        for &bin in &[3usize, 6, 9] {
            fft[bin] = Complex64::new(1.0, 0.0);
            fft[bin - 1] = Complex64::new(0.5, 0.0);
            fft[bin + 1] = Complex64::new(0.5, 0.0);
        }

        let est = estimate_fundamental_frequency(
            &snr, 0.0, &fft, 8, 1, n - 1, 1, delta_f, 10.0, 0.0, 50.0,
        )
        .expect("should find a fundamental");

        assert!((est.frequency - 300.0).abs() < 1.0, "freq = {}", est.frequency);
        assert_eq!(est.divisor, 1);
    }

    #[test]
    fn estimate_returns_none_without_peaks() {
        let snr = vec![0.0; 64];
        let fft = vec![Complex64::new(0.0, 0.0); 64];
        let est = estimate_fundamental_frequency(
            &snr, 0.0, &fft, 8, 1, 63, 1, 100.0, 10.0, 0.0, 50.0,
        );
        assert_eq!(est, None);
    }

    #[test]
    fn locker_locks_onto_steady_frequency() {
        let mut locker = FrequencyLocker::new();
        let mut last = 0.0;
        for _ in 0..20 {
            last = locker.process(440.0, 50.0);
        }
        assert!((last - 440.0).abs() < 1e-6, "locked frequency = {last}");
    }

    #[test]
    fn locker_starts_unlocked() {
        let mut locker = FrequencyLocker::new();
        // first reading can't be locked yet
        assert_eq!(locker.process(440.0, 50.0), 0.0);
    }

    #[test]
    fn locker_releases_on_silence() {
        let mut locker = FrequencyLocker::new();
        for _ in 0..20 {
            locker.process(440.0, 50.0);
        }
        // feed silence until it unlocks
        let mut last = 440.0;
        for _ in 0..10 {
            last = locker.process(0.0, 50.0);
        }
        assert_eq!(last, 0.0);
    }
}
