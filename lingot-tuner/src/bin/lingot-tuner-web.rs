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

//! Web tuner entry point (`lingot-tuner-web`) — serves the gauge to a browser.
//!
//! On Android/Termux this is the frontend with real graphics: the egui GUI
//! cannot run there at all, and the terminal one is limited to character cells.

use std::net::SocketAddr;
use std::process::ExitCode;

/// Loopback by default: the stream is derived from a live microphone, so
/// exposing it to the network has to be an explicit choice.
const DEFAULT_BIND: &str = "127.0.0.1:8080";

const USAGE: &str = "\
usage: lingot-tuner-web [ADDR]

  ADDR   address to bind, as HOST:PORT (default 127.0.0.1:8080).
         Use 0.0.0.0:8080 to reach the tuner from another device on the
         network — note that this lets anyone who can reach the port watch
         what you are playing.
";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let bind = match args.next() {
        Some(arg) if arg == "-h" || arg == "--help" => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(arg) => arg,
        None => DEFAULT_BIND.to_string(),
    };

    let bind: SocketAddr = match bind.parse() {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!("lingot-tuner-web: cannot parse {bind:?} as HOST:PORT: {e}\n");
            eprint!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = lingot_tuner::web::run(bind) {
        eprintln!("lingot-tuner-web: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
