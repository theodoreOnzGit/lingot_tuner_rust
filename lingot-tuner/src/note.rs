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

//! Shared note-mapping helper used by both the CLI and GUI frontends.

use lingot::scale::Scale;
use uom::si::f64::Frequency;
use uom::si::frequency::hertz;

/// Map a frequency to its nearest note name (with octave) and the deviation in
/// cents. The scale's base note (index 0) is C4, so octave 4 is added.
pub fn nearest_note(scale: &Scale, frequency: f64) -> (String, f64) {
    let (index, cents) = scale.closest_note_index(Frequency::new::<hertz>(frequency), 0.0);
    let note = &scale.notes()[scale.note_index(index) as usize];
    let octave = 4 + scale.octave(index);
    (format!("{}{}", note.name, octave), cents)
}
