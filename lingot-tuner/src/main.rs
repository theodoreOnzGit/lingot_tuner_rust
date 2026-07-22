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

//! GUI tuner entry point (`lingot-tuner`).

#[cfg(not(target_os = "android"))]
fn main() -> eframe::Result<()> {
    lingot_tuner::gui::run()
}

// Android/Termux has no windowing stack, so `eframe` is not a dependency there.
// The binary still builds — the Termux rule is compile-everything-but-the-GUI —
// it just points at the CLI instead of rendering a gauge.
#[cfg(target_os = "android")]
fn main() {
    eprintln!(
        "the graphical tuner is not available on Android/Termux.\n\
         \n\
         \x20 lingot-tuner-web   serves the gauge to this phone's browser (real graphics)\n\
         \x20 lingot-tuner-tui   a gauge in the terminal\n\
         \x20 lingot-tuner-cli   plain text"
    );
    std::process::exit(1);
}
