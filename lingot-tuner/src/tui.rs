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

//! Terminal frontend (`lingot-tuner-tui`): the tuning gauge drawn with text.
//!
//! This exists for Android/Termux. winit's X11 backend is compiled out on
//! `target_os = "android"` (`free_unix` excludes it in winit's build.rs), so the
//! egui frontend cannot be reached under Termux-X11 at all — a terminal gauge is
//! the only frontend that runs natively there. It is not Android-only, though:
//! it builds and runs anywhere, which is also what makes it testable on a
//! desktop.

use std::io;
use std::time::Duration;

use crossbeam_channel::Receiver;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use lingot::config::Config;
use lingot::scale::Scale;

use crate::core::{Core, TunerResult};
use crate::gauge::{Needle, IN_TUNE_CENTS};
use crate::note::nearest_note;

/// How often the terminal is redrawn. Well under the needle's fixed 60 Hz
/// filter step — [`Needle`] accumulates real elapsed time, so a slower redraw
/// changes the sampling of the motion, not the motion itself.
const FRAME: Duration = Duration::from_millis(33);

/// Run the terminal tuner. Blocks until the user quits.
pub fn run() -> io::Result<()> {
    let config = Config::default();
    let scale = config.scale.clone();
    let gauge_range = config.gauge_range;
    let gauge_rest_value = config.gauge_rest_value;

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

    let mut app = TunerTui {
        _core: core,
        results,
        scale,
        gauge_range,
        frequency: 0.0,
        note: "--".to_string(),
        cents: 0.0,
        needle: Needle::new(gauge_rest_value),
    };

    let mut terminal = ratatui::init();
    let outcome = app.event_loop(&mut terminal);
    // Restore the terminal even if the loop failed; leaving a Termux session in
    // raw mode with a hidden cursor is worse than losing the error.
    ratatui::restore();
    outcome
}

struct TunerTui {
    // Kept alive to keep capture + computation running; dropped on quit.
    _core: Core,
    results: Receiver<TunerResult>,
    scale: Scale,
    /// Full cents range spanned by the gauge (from config).
    gauge_range: f64,

    frequency: f64,
    note: String,
    cents: f64,
    needle: Needle,
}

impl TunerTui {
    fn event_loop(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        loop {
            self.drain_results();
            self.needle
                .advance((self.frequency > 0.0).then_some(self.cents));
            terminal.draw(|frame| self.draw(frame))?;

            // Poll rather than block so the needle keeps gliding with no input.
            if event::poll(FRAME)? {
                if let Event::Key(key) = event::read()? {
                    if should_quit(key) {
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Pull the most recent result from the channel (discarding any backlog).
    fn drain_results(&mut self) {
        let mut latest = None;
        while let Ok(r) = self.results.try_recv() {
            latest = Some(r);
        }
        if let Some(r) = latest {
            self.frequency = r.frequency;
            if r.frequency > 0.0 {
                let (note, cents) = nearest_note(&self.scale, r.frequency);
                self.note = note;
                self.cents = cents;
            } else {
                self.note = "--".to_string();
            }
        }
    }

    fn draw(&self, frame: &mut Frame) {
        let locked = self.frequency > 0.0;
        let colour = tune_colour(self.cents, locked);

        let outer = Block::default()
            .borders(Borders::ALL)
            .title(" lingot-tuner ")
            .title_alignment(Alignment::Center);
        let inner = outer.inner(frame.area());
        frame.render_widget(outer, frame.area());

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // note + Hz
                Constraint::Length(1), // scale labels
                Constraint::Length(1), // gauge bar
                Constraint::Length(1), // needle
                Constraint::Length(2), // cents readout
                Constraint::Min(0),    // status
            ])
            .split(inner);

        self.draw_readout(frame, rows[0], locked, colour);
        draw_scale_labels(frame, rows[1], self.gauge_range);
        draw_gauge_bar(frame, rows[2]);
        self.draw_needle(frame, rows[3], colour);
        self.draw_cents(frame, rows[4], locked, colour);
        self.draw_status(frame, rows[5], locked, colour);
    }

    fn draw_readout(&self, frame: &mut Frame, area: Rect, locked: bool, colour: Color) {
        let hz = if locked {
            format!("{:.2} Hz", self.frequency)
        } else {
            "-- Hz".to_string()
        };
        let text = vec![
            Line::from(Span::styled(
                self.note.clone(),
                Style::default().fg(colour).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(hz, Style::default().fg(Color::DarkGray))),
        ];
        frame.render_widget(Paragraph::new(text).alignment(Alignment::Center), area);
    }

    /// The needle: a caret under the gauge bar, positioned by the smoothed
    /// value so it glides exactly as the graphical gauge's needle does.
    fn draw_needle(&self, frame: &mut Frame, area: Rect, colour: Color) {
        let half = self.gauge_range / 2.0;
        let clamped = self.needle.position().clamp(-half, half);
        let col = cents_to_col(clamped, self.gauge_range, area.width);

        let mut row = " ".repeat(area.width as usize);
        if let Some(byte) = row.char_indices().nth(col).map(|(i, _)| i) {
            row.replace_range(byte..byte + 1, "▲");
        }
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(row, Style::default().fg(colour)))),
            area,
        );
    }

    fn draw_cents(&self, frame: &mut Frame, area: Rect, locked: bool, colour: Color) {
        let text = if locked {
            format!("{:+.1} cents", self.cents)
        } else {
            "------ cents".to_string()
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                text,
                Style::default().fg(colour).add_modifier(Modifier::BOLD),
            )))
            .alignment(Alignment::Center),
            area,
        );
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect, locked: bool, colour: Color) {
        let status = if !locked {
            Span::styled("listening…", Style::default().fg(Color::DarkGray))
        } else if self.cents.abs() < IN_TUNE_CENTS {
            Span::styled("in tune ✓", Style::default().fg(colour))
        } else if self.cents < 0.0 {
            Span::styled("flat — tune up", Style::default().fg(colour))
        } else {
            Span::styled("sharp — tune down", Style::default().fg(colour))
        };
        let lines = vec![
            Line::from(status),
            Line::from(Span::styled(
                "q / Esc / Ctrl-C to quit",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
    }
}

fn should_quit(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
        || (key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')))
}

/// Map a cents offset to a column, so the needle, the bar and the labels all
/// agree on where zero is. Returns a column inside `width`.
fn cents_to_col(cents: f64, gauge_range: f64, width: u16) -> usize {
    if width == 0 {
        return 0;
    }
    let last = (width - 1) as f64;
    let frac = (cents / gauge_range + 0.5).clamp(0.0, 1.0);
    (frac * last).round() as usize
}

fn draw_gauge_bar(frame: &mut Frame, area: Rect) {
    let w = area.width as usize;
    if w == 0 {
        return;
    }
    let mid = w / 2;
    let mut bar = String::with_capacity(w * 3);
    for i in 0..w {
        // Centre gets a doubled tick so "in tune" is unmistakable even on a
        // narrow phone terminal; quarter points get minor ticks.
        if i == mid {
            bar.push('╫');
        } else if i == 0 {
            bar.push('├');
        } else if i == w - 1 {
            bar.push('┤');
        } else if i == w / 4 || i == 3 * w / 4 {
            bar.push('┼');
        } else {
            bar.push('─');
        }
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            bar,
            Style::default().fg(Color::Blue),
        ))),
        area,
    );
}

fn draw_scale_labels(frame: &mut Frame, area: Rect, gauge_range: f64) {
    let w = area.width as usize;
    if w == 0 {
        return;
    }
    let half = gauge_range / 2.0;
    let mut row = vec![' '; w];
    for &cents in &[-half, -half / 2.0, 0.0, half / 2.0, half] {
        // "+0" reads oddly for the centre tick; everything else keeps its sign.
        let label = if cents == 0.0 {
            "0".to_string()
        } else {
            format!("{:+.0}", cents)
        };
        let centre = cents_to_col(cents, gauge_range, area.width);
        // Keep the label inside the row; it is centred on its tick where there
        // is room, and nudged inward at the edges.
        let start = centre
            .saturating_sub(label.len() / 2)
            .min(w.saturating_sub(label.len()));
        for (i, ch) in label.chars().enumerate() {
            if start + i < w {
                row[start + i] = ch;
            }
        }
    }
    let text: String = row.into_iter().collect();
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text,
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
}

/// Green when in tune, amber when close, red when well off — mirrors the
/// graphical gauge's colouring.
fn tune_colour(cents: f64, locked: bool) -> Color {
    if !locked {
        return Color::DarkGray;
    }
    let a = cents.abs();
    if a < IN_TUNE_CENTS {
        Color::Green
    } else if a < 3.0 * IN_TUNE_CENTS {
        Color::Yellow
    } else {
        Color::Red
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_cents_maps_to_the_middle_column() {
        // The needle at 0 must land on the same column the bar draws '╫',
        // otherwise "in tune" looks off-centre.
        let width = 41u16;
        assert_eq!(cents_to_col(0.0, 100.0, width), (width / 2) as usize);
    }

    #[test]
    fn extremes_map_to_the_end_columns() {
        assert_eq!(cents_to_col(-50.0, 100.0, 41), 0);
        assert_eq!(cents_to_col(50.0, 100.0, 41), 40);
    }

    #[test]
    fn out_of_range_cents_are_clamped_inside_the_row() {
        assert_eq!(cents_to_col(-500.0, 100.0, 41), 0);
        assert_eq!(cents_to_col(500.0, 100.0, 41), 40);
    }

    #[test]
    fn zero_width_does_not_panic() {
        assert_eq!(cents_to_col(0.0, 100.0, 0), 0);
    }

    #[test]
    fn colour_reflects_tuning_accuracy() {
        assert_eq!(tune_colour(0.0, false), Color::DarkGray);
        assert_eq!(tune_colour(1.0, true), Color::Green);
        assert_eq!(tune_colour(10.0, true), Color::Yellow);
        assert_eq!(tune_colour(40.0, true), Color::Red);
    }

    #[test]
    fn quit_keys_are_recognised() {
        assert!(should_quit(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE
        )));
        assert!(should_quit(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(should_quit(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
        assert!(!should_quit(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::NONE
        )));
    }
}
