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

//! Musical scale definition, mirroring `lingot-config-scale.{c,h}`.

use uom::si::f64::Frequency;
use uom::si::frequency::hertz;

use crate::defs::{CENTS_PER_OCTAVE, MID_C_FREQUENCY_HZ};

/// A single note within a scale.
#[derive(Clone, Debug, PartialEq)]
pub struct ScaleNote {
    pub name: String,
    /// Offset from the scale's base note, in cents.
    pub offset_cents: f64,
    /// Exact offset as an integer ratio `(numerator, denominator)`, when the
    /// scale defines one. `None` for tempered scales that only give cents.
    pub offset_ratio: Option<(i16, i16)>,
}

/// A musical scale: an ordered set of notes within one octave, anchored to a
/// base frequency (the frequency of note index 0, typically C4).
#[derive(Clone, Debug, PartialEq)]
pub struct Scale {
    pub name: String,
    pub base_frequency: Frequency,
    notes: Vec<ScaleNote>,
}

impl Scale {
    /// Number of notes in the scale.
    pub fn len(&self) -> usize {
        self.notes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    pub fn notes(&self) -> &[ScaleNote] {
        &self.notes
    }

    fn num_notes(&self) -> i32 {
        self.notes.len() as i32
    }

    /// Octave number for an absolute note index. Octave 0 starts at index 0.
    pub fn octave(&self, index: i32) -> i32 {
        let n = self.num_notes();
        if index < 0 {
            ((index + 1) / n) - 1
        } else {
            index / n
        }
    }

    /// Note index within `[0, len)` for an absolute note index.
    pub fn note_index(&self, index: i32) -> i32 {
        let r = index % self.num_notes();
        if r < 0 {
            r + self.num_notes()
        } else {
            r
        }
    }

    /// Absolute offset from the base note, in cents, for an absolute index.
    pub fn absolute_offset_cents(&self, index: i32) -> f64 {
        let note = self.note_index(index) as usize;
        self.octave(index) as f64 * CENTS_PER_OCTAVE + self.notes[note].offset_cents
    }

    /// Frequency of a given absolute note index.
    ///
    /// Index 0 is the base note (typically C4). To get C1 on a 12-note scale,
    /// pass `-36`.
    pub fn frequency(&self, index: i32) -> Frequency {
        let factor = 2.0_f64.powf(self.absolute_offset_cents(index) / CENTS_PER_OCTAVE);
        self.base_frequency * factor
    }

    /// Closest note to `freq`, accounting for a `deviation` (cents) of the
    /// scale's root. Returns the absolute note index and the tuning error in
    /// cents (negative = flat, positive = sharp relative to the note).
    pub fn closest_note_index(&self, freq: Frequency, deviation_cents: f64) -> (i32, f64) {
        let n = self.num_notes();

        let mut offset = CENTS_PER_OCTAVE
            * (freq.get::<hertz>() / self.base_frequency.get::<hertz>()).log2()
            - deviation_cents;
        let mut octave = (offset / CENTS_PER_OCTAVE).floor() as i32;
        offset = offset.rem_euclid(CENTS_PER_OCTAVE);

        let mut index = (n as f64 * offset / CENTS_PER_OCTAVE).floor() as i32;

        let (pitch_inf, pitch_sup) = loop {
            let inf = self.notes[index as usize].offset_cents;
            let sup = if index + 1 < n {
                self.notes[(index + 1) as usize].offset_cents
            } else {
                CENTS_PER_OCTAVE
            };

            if offset > sup {
                index += 1;
                continue;
            }
            if offset < inf {
                index -= 1;
                continue;
            }
            break (inf, sup);
        };

        let (mut note_index, error_cents) = if (offset - pitch_inf).abs() < (offset - pitch_sup).abs()
        {
            (index, offset - pitch_inf)
        } else {
            (index + 1, offset - pitch_sup)
        };

        if note_index == n {
            note_index = 0;
            octave += 1;
        }

        (note_index + octave * n, error_cents)
    }
}

impl Default for Scale {
    /// The hard-coded 12-tone equal-tempered scale (base note C4).
    fn default() -> Self {
        const TONE_NAMES: [&str; 12] = [
            "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
        ];

        let notes = TONE_NAMES
            .iter()
            .enumerate()
            .map(|(i, name)| ScaleNote {
                name: (*name).to_string(),
                offset_cents: 100.0 * i as f64,
                offset_ratio: if i == 0 { Some((1, 1)) } else { None },
            })
            .collect();

        Scale {
            name: "Default equal-tempered scale".to_string(),
            base_frequency: Frequency::new::<hertz>(MID_C_FREQUENCY_HZ),
            notes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::MID_A_FREQUENCY_HZ;

    #[test]
    fn default_scale_has_twelve_notes() {
        let scale = Scale::default();
        assert_eq!(scale.len(), 12);
        assert_eq!(scale.notes()[9].name, "A");
    }

    #[test]
    fn a4_frequency_is_440() {
        let scale = Scale::default();
        // A4 is 9 semitones above C4 (index 0).
        let a4 = scale.frequency(9).get::<hertz>();
        assert!((a4 - MID_A_FREQUENCY_HZ).abs() < 1e-6, "A4 = {a4}, expected 440");
    }

    #[test]
    fn c1_is_three_octaves_below_base() {
        let scale = Scale::default();
        let c1 = scale.frequency(-36).get::<hertz>();
        assert!((c1 - MID_C_FREQUENCY_HZ / 8.0).abs() < 1e-6, "C1 = {c1}");
    }

    #[test]
    fn octave_and_note_index_wrap() {
        let scale = Scale::default();
        assert_eq!(scale.octave(9), 0);
        assert_eq!(scale.note_index(9), 9);
        assert_eq!(scale.octave(-36), -3);
        assert_eq!(scale.note_index(-36), 0);
        assert_eq!(scale.octave(13), 1);
        assert_eq!(scale.note_index(13), 1);
    }

    #[test]
    fn closest_note_to_a4() {
        let scale = Scale::default();
        let (index, error) = scale.closest_note_index(Frequency::new::<hertz>(440.0), 0.0);
        assert_eq!(index, 9);
        // base C4 constant is rounded, so the error is ~2e-6 cents, not exactly 0.
        assert!(error.abs() < 1e-3, "error = {error}");
    }

    #[test]
    fn closest_note_slightly_sharp() {
        let scale = Scale::default();
        // ~20 cents sharp of A4
        let freq = Frequency::new::<hertz>(440.0 * 2.0_f64.powf(20.0 / 1200.0));
        let (index, error) = scale.closest_note_index(freq, 0.0);
        assert_eq!(index, 9);
        assert!((error - 20.0).abs() < 1e-3, "error = {error}");
    }
}
