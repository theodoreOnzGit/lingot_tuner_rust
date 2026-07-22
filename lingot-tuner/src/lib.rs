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

//! Application-internal library for the `lingot-tuner` package.
//!
//! This is **not** the reusable `lingot` library (Layers 1-3) — it exists only
//! to share the core loop, needle smoothing and note-mapping helpers between
//! this package's binaries: `lingot-tuner` (GUI), `lingot-tuner-tui`
//! (terminal), `lingot-tuner-web` (browser) and `lingot-tuner-cli` (plain
//! text). It is allowed to contain application-level threading and `egui` code.

/// Advice to print when [`core::Core::start`] fails, or `None` where the
/// platform offers none.
///
/// Android surfaces a permission refusal as a bare AAudio error that says
/// nothing about permissions, so the cause has to be spelled out. Confirmed in
/// the field: granting Termux the microphone permission is what fixes it.
///
/// Shared by both frontends so the two copies cannot drift apart.
pub fn audio_start_hint() -> Option<&'static str> {
    if cfg!(target_os = "android") {
        Some(
            "hint: on Android/Termux this is almost always the microphone permission.\n\
             \n\
             \x20 1. Check that the Termux:API add-on app is installed. It declares\n\
             \x20    RECORD_AUDIO and shares Termux's UID, which is what makes the\n\
             \x20    permission grantable at all.\n\
             \x20 2. Grant it:  Settings > Apps > Termux > Permissions > Microphone\n\
             \x20    (or, over adb:  pm grant com.termux android.permission.RECORD_AUDIO)",
        )
    } else {
        None
    }
}

pub mod core;
pub mod gauge;
#[cfg(all(feature = "gui", not(target_os = "android")))]
pub mod gui;
pub mod note;
#[cfg(feature = "tui")]
pub mod tui;
#[cfg(feature = "web")]
pub mod web;
