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

//! Web frontend (`lingot-tuner-web`): an embedded HTTP + WebSocket server that
//! serves a single self-contained page, which draws the gauge on a canvas.
//!
//! This is the best frontend on Android. The egui GUI cannot run under Termux
//! at all (winit compiles its X11 backend out on `target_os = "android"`), and
//! the terminal frontend is limited to what fits in character cells — no
//! spectrum, no analog dial. Here the phone's own browser is the renderer, so
//! the UI is real graphics with no windowing stack, no NDK and no add-on app.
//!
//! Everything is deliberately hand-rolled over `std::net`: the only new
//! dependency is `tungstenite` for WebSocket framing, which is pure Rust and
//! builds for `aarch64-linux-android` (hard rule 1). There is no HTTP
//! framework and no async runtime — one page and one socket route do not need
//! either.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use tungstenite::{Message, WebSocket};

use lingot::config::Config;
use lingot::scale::Scale;

use crate::core::{Core, TunerResult};
use crate::gauge::Needle;
use crate::note::nearest_note;

/// The entire frontend, embedded so the binary is self-sufficient.
const INDEX_HTML: &str = include_str!("web/index.html");

/// Path the browser opens its WebSocket on.
const WS_PATH: &str = "/ws";

/// Broadcast tick. Matches the gauge filter's own 60 Hz step, so the browser
/// sees every needle position the shared [`Needle`] computes.
const TICK: Duration = Duration::from_millis(16);

/// How long a connection gets to send its request line before we give up on it.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Bounds a write to a client that has stopped reading, so its thread cannot
/// hang forever holding a slot in the broadcast list.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Run the web tuner: start audio, serve the page, and push snapshots to every
/// connected browser. Blocks forever (Ctrl-C to quit).
pub fn run(bind: SocketAddr) -> io::Result<()> {
    let config = Config::default();
    let scale = config.scale.clone();
    let gauge_range = config.gauge_range;
    let gauge_rest_value = config.gauge_rest_value;

    // Bind before starting audio: a port clash should fail immediately rather
    // than after the microphone has been opened.
    let listener = TcpListener::bind(bind)?;
    let local = listener.local_addr()?;

    let (core, results) = match Core::start(config) {
        Ok(running) => running,
        Err(e) => {
            eprintln!("failed to start audio: {e}");
            if let Some(hint) = crate::audio_start_hint() {
                eprintln!("\n{hint}");
            }
            std::process::exit(1);
        }
    };

    let broadcaster = Broadcaster::default();
    {
        let broadcaster = broadcaster.clone();
        thread::spawn(move || serve(listener, broadcaster));
    }

    println!("lingot-tuner-web — open http://{local}/ in a browser (Ctrl-C to quit)");
    if !local.ip().is_loopback() {
        eprintln!(
            "warning: bound to {local}, which is not loopback — anyone who can reach this \
             port can watch what you are playing"
        );
    }

    let mut state = State::new(scale, gauge_rest_value);
    loop {
        let fresh = state.drain(&results);
        state.needle.advance(state.target_cents());
        let healthy = core.is_healthy();
        broadcaster.send(&encode(&state.snapshot(fresh, healthy), gauge_range));
        thread::sleep(TICK);
    }
}

// ---------------------------------------------------------------------------
// tuner state
// ---------------------------------------------------------------------------

/// The latest reading, plus the smoothed needle driving the dial.
struct State {
    scale: Scale,
    frequency: f64,
    note: String,
    cents: f64,
    spd: Vec<f64>,
    needle: Needle,
}

impl State {
    fn new(scale: Scale, gauge_rest_value: f64) -> Self {
        State {
            scale,
            frequency: 0.0,
            note: "--".to_string(),
            cents: 0.0,
            spd: Vec::new(),
            needle: Needle::new(gauge_rest_value),
        }
    }

    /// Absorb the most recent result (discarding any backlog). Returns whether
    /// a new one arrived, which is what decides if this tick carries a
    /// spectrum.
    fn drain(&mut self, results: &Receiver<TunerResult>) -> bool {
        let mut latest = None;
        while let Ok(r) = results.try_recv() {
            latest = Some(r);
        }
        let Some(r) = latest else { return false };

        self.frequency = r.frequency;
        self.spd = r.spd;
        if r.frequency > 0.0 {
            let (note, cents) = nearest_note(&self.scale, r.frequency);
            self.note = note;
            self.cents = cents;
        } else {
            self.note = "--".to_string();
        }
        true
    }

    fn target_cents(&self) -> Option<f64> {
        (self.frequency > 0.0).then_some(self.cents)
    }

    fn snapshot(&self, fresh: bool, healthy: bool) -> Snapshot<'_> {
        Snapshot {
            frequency: self.frequency,
            note: &self.note,
            cents: self.cents,
            needle: self.needle.position(),
            locked: self.frequency > 0.0,
            healthy,
            // The spectrum is ~256 values and only changes at `calculation_rate`
            // (15 Hz), so sending it on all 60 ticks would be 4x the bytes for
            // no extra information.
            spd: fresh.then_some(self.spd.as_slice()),
        }
    }
}

/// One frame of tuner state as the browser receives it.
struct Snapshot<'a> {
    frequency: f64,
    note: &'a str,
    cents: f64,
    needle: f64,
    locked: bool,
    healthy: bool,
    spd: Option<&'a [f64]>,
}

/// Serialise a snapshot as JSON.
///
/// Hand-written rather than via serde: the shape is fixed and tiny, and this
/// keeps a proc-macro dependency out of a tree that has to cross-compile.
fn encode(s: &Snapshot, gauge_range: f64) -> String {
    let mut out = String::with_capacity(96 + s.spd.map_or(0, |v| v.len() * 4));
    let _ = write!(
        out,
        "{{\"hz\":{:.2},\"note\":{},\"cents\":{:.2},\"needle\":{:.3},\"locked\":{},\"ok\":{},\"range\":{}",
        finite(s.frequency),
        json_string(s.note),
        finite(s.cents),
        finite(s.needle),
        s.locked,
        s.healthy,
        finite(gauge_range),
    );
    if let Some(spd) = s.spd {
        out.push_str(",\"spd\":[");
        for (i, v) in spd.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            // Whole dB is far finer than a bar chart can render, and rounding
            // roughly halves the size of the biggest field in the message.
            let _ = write!(out, "{}", finite(*v).round() as i64);
        }
        out.push(']');
    }
    out.push('}');
    out
}

/// Replace NaN/infinity with 0.
///
/// JSON has no encoding for either, so letting one through would emit a bare
/// `NaN` and every subsequent frame would die in the browser's `JSON.parse`.
fn finite(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// Quote and escape a string as a JSON literal. Note names come from the scale,
/// which is user-supplied data, so they cannot be trusted to be quote-free.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// broadcast
// ---------------------------------------------------------------------------

/// The set of connected browsers.
///
/// The mutex guards only the client registry — a `Vec` of channel senders. It
/// is never held across any I/O: the fan-out is a non-blocking `try_send` per
/// client, and the actual socket writes happen on each client's own thread.
/// That is why a lock here cannot stall the 60 Hz loop.
#[derive(Clone, Default)]
struct Broadcaster {
    clients: Arc<Mutex<Vec<Sender<Arc<str>>>>>,
}

impl Broadcaster {
    fn register(&self) -> Receiver<Arc<str>> {
        let (tx, rx) = bounded(8);
        self.clients.lock().unwrap().push(tx);
        rx
    }

    fn send(&self, message: &str) {
        let message: Arc<str> = Arc::from(message);
        let mut clients = self.clients.lock().unwrap();
        clients.retain(|tx| match tx.try_send(message.clone()) {
            // A backed-up client drops this frame rather than slowing everyone
            // down; at 60 Hz the next one is 16 ms away.
            Err(TrySendError::Full(_)) => true,
            Err(TrySendError::Disconnected(_)) => false,
            Ok(()) => true,
        });
    }
}

// ---------------------------------------------------------------------------
// HTTP / WebSocket server
// ---------------------------------------------------------------------------

fn serve(listener: TcpListener, broadcaster: Broadcaster) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let broadcaster = broadcaster.clone();
        // A thread per connection: there is one browser, occasionally two.
        thread::spawn(move || {
            if let Err(e) = handle(stream, broadcaster) {
                // A browser closing a tab is not worth reporting; anything that
                // fails before that is.
                if e.kind() != io::ErrorKind::BrokenPipe {
                    eprintln!("lingot-tuner-web: connection error: {e}");
                }
            }
        });
    }
}

fn handle(stream: TcpStream, broadcaster: Broadcaster) -> io::Result<()> {
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;

    match peek_request_target(&stream)?.as_deref() {
        Some(WS_PATH) => {
            // `tungstenite::accept` performs the whole HTTP handshake itself and
            // therefore needs the request still unread — which is exactly why
            // routing above had to peek instead of reading.
            stream.set_read_timeout(None)?;
            let updates = broadcaster.register();
            let ws = tungstenite::accept(stream)
                .map_err(|e| io::Error::other(format!("websocket handshake failed: {e}")))?;
            pump(ws, updates);
            Ok(())
        }
        Some("/") => {
            drain_request(&stream);
            respond(stream, "200 OK", "text/html; charset=utf-8", INDEX_HTML)
        }
        _ => {
            drain_request(&stream);
            respond(
                stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                "not found\n",
            )
        }
    }
}

/// Read the request target (the middle field of the request line) **without
/// consuming any of it**, so the stream can still be handed to
/// `tungstenite::accept` untouched.
fn peek_request_target(stream: &TcpStream) -> io::Result<Option<String>> {
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut buf = [0u8; 512];
    loop {
        let n = stream.peek(&mut buf)?;
        if n == 0 {
            return Ok(None); // client hung up
        }
        if let Some(end) = buf[..n].windows(2).position(|w| w == b"\r\n") {
            let line = String::from_utf8_lossy(&buf[..end]);
            return Ok(line.split(' ').nth(1).map(str::to_owned));
        }
        // The request line virtually always arrives in the first segment; this
        // only covers a pathological split, and gives up rather than spinning.
        if n == buf.len() || Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(2));
    }
}

/// Consume the request headers so the response is not answered into a socket
/// with unread data — closing one of those can RST the connection and lose the
/// response before the browser reads it.
fn drain_request(stream: &TcpStream) {
    let mut stream = stream;
    let mut buf = [0u8; 1024];
    let mut seen = Vec::new();
    while seen.len() < 16 * 1024 {
        match stream.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                seen.extend_from_slice(&buf[..n]);
                if seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

fn respond(mut stream: TcpStream, status: &str, content_type: &str, body: &str) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

/// Feed one connected browser.
///
/// Write-only: the page never sends anything, so there is nothing to read, and
/// a closed tab surfaces as a write error — which ends this thread, drops the
/// receiver, and lets the next broadcast prune the client.
fn pump(mut ws: WebSocket<TcpStream>, updates: Receiver<Arc<str>>) {
    for message in updates {
        if ws.send(Message::text(message.as_ref())).is_err() {
            break;
        }
    }
    let _ = ws.close(None);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Snapshot<'static> {
        Snapshot {
            frequency: 220.0,
            note: "A3",
            cents: -3.25,
            needle: -3.0,
            locked: true,
            healthy: true,
            spd: None,
        }
    }

    #[test]
    fn encodes_a_locked_reading() {
        let json = encode(&snapshot(), 100.0);
        assert_eq!(
            json,
            r#"{"hz":220.00,"note":"A3","cents":-3.25,"needle":-3.000,"locked":true,"ok":true,"range":100}"#
        );
    }

    #[test]
    fn spectrum_is_only_present_when_fresh() {
        let mut s = snapshot();
        assert!(!encode(&s, 100.0).contains("spd"));

        let spd = [0.4, 12.6, -3.2];
        s.spd = Some(&spd);
        // Values are rounded to whole dB; a bar chart cannot show more.
        assert!(encode(&s, 100.0).ends_with(r#","spd":[0,13,-3]}"#));
    }

    /// A single non-finite value must not be able to poison the stream: JSON
    /// cannot represent NaN, and the browser would fail every later parse.
    #[test]
    fn non_finite_values_are_neutralised() {
        let mut s = snapshot();
        s.frequency = f64::NAN;
        s.needle = f64::INFINITY;
        let spd = [f64::NAN];
        s.spd = Some(&spd);

        let json = encode(&s, 100.0);
        assert!(json.contains(r#""hz":0.00"#), "{json}");
        assert!(json.contains(r#""needle":0.000"#), "{json}");
        assert!(json.contains(r#""spd":[0]"#), "{json}");
        assert!(!json.contains("NaN") && !json.contains("inf"), "{json}");
    }

    #[test]
    fn note_names_are_escaped() {
        assert_eq!(json_string(r#"A"3"#), r#""A\"3""#);
        assert_eq!(json_string("A\\3"), r#""A\\3""#);
        assert_eq!(json_string("A\u{1}"), r#""A\u0001""#);
        assert_eq!(json_string("C♯4"), "\"C♯4\"");
    }

    #[test]
    fn broadcaster_drops_clients_that_have_gone_away() {
        let broadcaster = Broadcaster::default();
        let alive = broadcaster.register();
        drop(broadcaster.register()); // a browser that closed its tab

        broadcaster.send("first");
        assert_eq!(broadcaster.clients.lock().unwrap().len(), 1);
        assert_eq!(alive.try_recv().unwrap().as_ref(), "first");
    }

    /// A client that stops reading must not stall the broadcast loop: its
    /// channel fills, frames are dropped, and it stays registered.
    #[test]
    fn a_lagging_client_drops_frames_instead_of_blocking() {
        let broadcaster = Broadcaster::default();
        let _slow = broadcaster.register();
        for i in 0..100 {
            broadcaster.send(&format!("frame {i}"));
        }
        assert_eq!(broadcaster.clients.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_embedded_page_is_present_and_self_contained() {
        assert!(INDEX_HTML.contains("<canvas id=\"gauge\">"));
        // A CDN reference would break the whole point: Termux with no network.
        assert!(!INDEX_HTML.contains("https://cdn"));
        assert!(!INDEX_HTML.contains("<script src="));
    }
}
