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

//! Needle smoothing, shared by every frontend.
//!
//! This lives outside `gui.rs` because the terminal frontend needs exactly the
//! same motion, and `gui.rs` is compiled out entirely on Android. The filter
//! design is subtle enough that two copies would drift.

use std::time::Instant;

use lingot::filter::Filter;

/// Within this many cents the note counts as in tune.
pub const IN_TUNE_CENTS: f64 = 5.0;

// The needle is modelled as a damped spring driven each tick toward the target
// cents. Values and 2nd-order IIR design match lingot's gauge filter
// (lingot-gui-mainframe.c / lingot-io-ui-settings.c defaults).
const GAUGE_RATE: f64 = 60.0; // Hz — fixed update rate the coefficients assume
const GAUGE_ADAPTATION: f64 = 150.0; // quicker to target as this grows
const GAUGE_DAMPING: f64 = 30.0; // less "bounce" as this grows (lingot default: 15)

/// Build lingot's 2nd-order gauge-smoothing filter, primed to rest at `rest`.
fn gauge_filter(rest: f64) -> Filter {
    let (k, q, r) = (GAUGE_ADAPTATION, GAUGE_DAMPING, GAUGE_RATE);
    let a = [k + r * (q + r), -r * (q + 2.0 * r), r * r];
    let b = [k];
    let mut filter = Filter::new(&a, &b);
    // Settle the filter state so the needle starts at rest (no startup sweep).
    for _ in 0..600 {
        filter.filter_sample(rest);
    }
    filter
}

/// A smoothed needle position in cents.
///
/// [`advance`](Self::advance) is driven at a fixed 60 Hz via a time
/// accumulator, so motion is independent of how often the frontend happens to
/// redraw — a 144 Hz window and a 20 Hz terminal show the same physical needle.
pub struct Needle {
    filter: Filter,
    /// Where the needle sits when nothing is detected (from config; ≈ −45¢).
    rest: f64,
    pos: f64,
    accumulator: f64,
    last_step: Instant,
}

impl Needle {
    pub fn new(rest: f64) -> Self {
        Needle {
            filter: gauge_filter(rest),
            rest,
            pos: rest,
            accumulator: 0.0,
            last_step: Instant::now(),
        }
    }

    /// Smoothed position, in cents.
    pub fn position(&self) -> f64 {
        self.pos
    }

    /// Step the filter toward `target` cents, or back toward rest when `None`
    /// (no pitch detected). Call once per frame; time elapsed since the last
    /// call drives the fixed-rate stepping.
    pub fn advance(&mut self, target: Option<f64>) {
        let dt = self.last_step.elapsed().as_secs_f64();
        self.last_step = Instant::now();
        self.advance_by(target, dt);
    }

    /// [`advance`](Self::advance) with an explicit elapsed time, in seconds.
    ///
    /// Separate from `advance` so the motion can be tested deterministically —
    /// reading the wall clock makes a tight test loop advance no simulated time
    /// at all, and the needle would appear frozen.
    pub fn advance_by(&mut self, target: Option<f64>, dt: f64) {
        let target = target.unwrap_or(self.rest);

        // Cap to avoid a long catch-up burst after a stall.
        self.accumulator = (self.accumulator + dt).min(0.25);

        let step = 1.0 / GAUGE_RATE;
        while self.accumulator >= step {
            self.pos = self.filter.filter_sample(target);
            self.accumulator -= step;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needle_starts_at_rest() {
        let needle = Needle::new(-45.0);
        assert!((needle.position() - (-45.0)).abs() < 1e-6);
    }

    /// One simulated frame at the gauge's own rate.
    const FRAME_DT: f64 = 1.0 / GAUGE_RATE;

    #[test]
    fn needle_converges_toward_target() {
        let mut needle = Needle::new(-45.0);
        for _ in 0..600 {
            needle.advance_by(Some(10.0), FRAME_DT);
        }
        assert!(
            (needle.position() - 10.0).abs() < 1.0,
            "needle stalled at {}",
            needle.position()
        );
    }

    #[test]
    fn needle_returns_to_rest_when_unlocked() {
        let mut needle = Needle::new(-45.0);
        for _ in 0..600 {
            needle.advance_by(Some(10.0), FRAME_DT);
        }
        for _ in 0..600 {
            needle.advance_by(None, FRAME_DT);
        }
        assert!(
            (needle.position() - (-45.0)).abs() < 1.0,
            "needle did not return to rest, at {}",
            needle.position()
        );
    }

    #[test]
    fn needle_motion_is_refresh_rate_independent() {
        // The whole point of the accumulator: the same simulated time must
        // produce the same needle position whether the frontend redraws at
        // 60 Hz (egui) or 30 Hz (terminal).
        let mut fast = Needle::new(-45.0);
        let mut slow = Needle::new(-45.0);
        for _ in 0..600 {
            fast.advance_by(Some(10.0), FRAME_DT);
        }
        for _ in 0..300 {
            slow.advance_by(Some(10.0), 2.0 * FRAME_DT);
        }
        assert!(
            (fast.position() - slow.position()).abs() < 1e-9,
            "60 Hz gave {} but 30 Hz gave {}",
            fast.position(),
            slow.position()
        );
    }

    #[test]
    fn a_long_stall_does_not_cause_a_catch_up_burst() {
        // A backgrounded terminal can return a huge dt; the accumulator caps it
        // so the needle never fast-forwards through the whole gap.
        let mut capped = Needle::new(-45.0);
        capped.advance_by(Some(10.0), 60.0);
        let mut stepped = Needle::new(-45.0);
        // 0.25 s cap / (1/60 s) = 15 steps.
        for _ in 0..15 {
            stepped.advance_by(Some(10.0), FRAME_DT);
        }
        assert!((capped.position() - stepped.position()).abs() < 1e-9);
    }
}
