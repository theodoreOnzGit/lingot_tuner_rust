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

//! Command-line tuner entry point (`lingot-tuner-cli`): prints the detected
//! pitch as text. The graphical frontend is the default `lingot-tuner` binary.

use lingot::config::Config;
use lingot_tuner::core::Core;
use lingot_tuner::note::nearest_note;

fn main() {
    let config = Config::default();
    let scale = config.scale.clone();

    let (_core, results) = match Core::start(config) {
        Ok(running) => running,
        Err(e) => {
            eprintln!("failed to start audio: {e}");
            // On Android the input stream is opened via AAudio, which refuses
            // to open a recording stream unless the *app* holding the process
            // has RECORD_AUDIO. Under Termux that is not granted by default and
            // the underlying error says nothing about permissions.
            if cfg!(target_os = "android") {
                eprintln!(
                    "\nhint: on Android/Termux, recording requires the microphone permission.\n\
                     Grant it to Termux (Settings > Apps > Termux > Permissions > Microphone),\n\
                     or via adb: pm grant com.termux android.permission.RECORD_AUDIO\n\
                     Note that Termux must declare RECORD_AUDIO for the grant to be possible;\n\
                     installing the Termux:API add-on (which shares Termux's UID) is the usual way."
                );
            }
            std::process::exit(1);
        }
    };

    println!("lingot-tuner-cli — listening (Ctrl-C to quit)\n");

    for result in results.iter() {
        if result.frequency > 0.0 {
            let (note, cents) = nearest_note(&scale, result.frequency);
            println!(
                "{:8.2} Hz   {:<4}  {:+6.1} cents",
                result.frequency, note, cents
            );
        }
    }
}
