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

//! egui frontend (Layer 5): tuning gauge, spectrum view, and strobe disc.

use std::f32::consts::PI;
use std::time::Instant;

use crossbeam_channel::Receiver;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use lingot::config::Config;
use lingot::scale::Scale;

use crate::core::{Core, TunerResult};
use crate::note::nearest_note;

/// Cents either side of centre shown on the gauge.
const GAUGE_HALF_RANGE: f64 = 50.0;
/// Within this many cents the note is considered in tune (turns green).
const IN_TUNE_CENTS: f64 = 5.0;

/// Launch the GUI tuner. Blocks until the window is closed.
pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([520.0, 560.0]),
        ..Default::default()
    };

    eframe::run_native(
        "lingot-tuner",
        options,
        Box::new(|_cc| {
            let config = Config::default();
            let scale = config.scale.clone();
            let (core, results) = Core::start(config)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;
            Ok(Box::new(TunerApp::new(core, results, scale)))
        }),
    )
}

struct TunerApp {
    // Kept alive to keep capture + computation running; dropped on close.
    _core: Core,
    results: Receiver<TunerResult>,
    scale: Scale,

    frequency: f64,
    note: String,
    cents: f64,
    spd: Vec<f64>,
    /// Accumulated strobe phase (radians); advances proportionally to pitch error.
    strobe_phase: f32,
    last_frame: Instant,
}

impl TunerApp {
    fn new(core: Core, results: Receiver<TunerResult>, scale: Scale) -> Self {
        TunerApp {
            _core: core,
            results,
            scale,
            frequency: 0.0,
            note: "--".to_string(),
            cents: 0.0,
            spd: Vec::new(),
            strobe_phase: 0.0,
            last_frame: Instant::now(),
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
            self.spd = r.spd;
            if r.frequency > 0.0 {
                let (note, cents) = nearest_note(&self.scale, r.frequency);
                self.note = note;
                self.cents = cents;
            } else {
                self.note = "--".to_string();
            }
        }
    }
}

impl eframe::App for TunerApp {
    // eframe 0.34: `ui` is the required method; the given `Ui` is the root
    // central panel (no margin/background).
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_results();

        let dt = self.last_frame.elapsed().as_secs_f32();
        self.last_frame = Instant::now();
        // Strobe spins at a rate proportional to the cent error; still when in tune.
        if self.frequency > 0.0 {
            self.strobe_phase += dt * self.cents as f32 * 0.25;
        }

        ui.vertical_centered(|ui| {
            ui.add_space(12.0);
            let locked = self.frequency > 0.0;
            let colour = tune_colour(self.cents, locked);

            ui.heading(egui::RichText::new(&self.note).size(64.0).color(colour));
            if locked {
                ui.label(egui::RichText::new(format!("{:.2} Hz", self.frequency)).size(20.0));
                ui.label(
                    egui::RichText::new(format!("{:+.1} cents", self.cents))
                        .size(18.0)
                        .color(colour),
                );
            } else {
                ui.label(egui::RichText::new("listening…").size(18.0).weak());
            }
        });

        ui.add_space(8.0);
        self.draw_gauge(ui);
        ui.add_space(8.0);
        self.draw_strobe(ui);
        ui.add_space(8.0);
        self.draw_spectrum(ui);

        // Animate continuously so channel polling and the strobe keep running.
        ui.ctx().request_repaint();
    }
}

impl TunerApp {
    fn draw_gauge(&self, ui: &mut egui::Ui) {
        let (resp, painter) =
            ui.allocate_painter(Vec2::new(ui.available_width(), 90.0), Sense::hover());
        let rect = resp.rect;
        let mid_y = rect.center().y;
        let left = rect.left() + 20.0;
        let right = rect.right() - 20.0;
        let span = right - left;

        // baseline + tick marks every 10 cents
        painter.line_segment(
            [Pos2::new(left, mid_y), Pos2::new(right, mid_y)],
            Stroke::new(2.0, Color32::DARK_GRAY),
        );
        let mut c = -GAUGE_HALF_RANGE;
        while c <= GAUGE_HALF_RANGE {
            let x = left + span * ((c + GAUGE_HALF_RANGE) / (2.0 * GAUGE_HALF_RANGE)) as f32;
            let h = if c == 0.0 { 18.0 } else { 10.0 };
            let col = if c == 0.0 { Color32::GRAY } else { Color32::DARK_GRAY };
            painter.line_segment(
                [Pos2::new(x, mid_y - h), Pos2::new(x, mid_y + h)],
                Stroke::new(if c == 0.0 { 2.0 } else { 1.0 }, col),
            );
            c += 10.0;
        }

        // needle
        if self.frequency > 0.0 {
            let clamped = self.cents.clamp(-GAUGE_HALF_RANGE, GAUGE_HALF_RANGE);
            let x = left
                + span * ((clamped + GAUGE_HALF_RANGE) / (2.0 * GAUGE_HALF_RANGE)) as f32;
            let colour = tune_colour(self.cents, true);
            painter.line_segment(
                [Pos2::new(x, mid_y - 28.0), Pos2::new(x, mid_y + 28.0)],
                Stroke::new(3.0, colour),
            );
            painter.circle_filled(Pos2::new(x, mid_y), 5.0, colour);
        }
    }

    /// A simple strobe disc: a ring of segments rotating at a rate set by the
    /// pitch error. Appears to stand still when perfectly in tune.
    fn draw_strobe(&self, ui: &mut egui::Ui) {
        let (resp, painter) =
            ui.allocate_painter(Vec2::new(ui.available_width(), 120.0), Sense::hover());
        let center = resp.rect.center();
        let radius = 50.0;
        let segments = 16;

        painter.circle_stroke(center, radius, Stroke::new(1.0, Color32::DARK_GRAY));
        for i in 0..segments {
            let a = self.strobe_phase + (i as f32) * 2.0 * PI / segments as f32;
            // alternate filled/empty wedges
            let filled = i % 2 == 0;
            let col = if filled { Color32::from_gray(200) } else { Color32::from_gray(40) };
            let p = center + Vec2::new(a.cos(), a.sin()) * radius;
            painter.circle_filled(p, 6.0, col);
        }
    }

    fn draw_spectrum(&self, ui: &mut egui::Ui) {
        let (resp, painter) =
            ui.allocate_painter(Vec2::new(ui.available_width(), 140.0), Sense::hover());
        let rect = resp.rect;
        painter.rect_filled(rect, 2.0, Color32::from_gray(18));

        if self.spd.is_empty() {
            return;
        }

        // SNR spectrum is in dB; map a fixed [0, 40] dB window to bar height.
        let n = self.spd.len();
        let bar_w = rect.width() / n as f32;
        for (i, &v) in self.spd.iter().enumerate() {
            let norm = (v / 40.0).clamp(0.0, 1.0) as f32;
            let h = norm * rect.height();
            let x = rect.left() + i as f32 * bar_w;
            let bar = Rect::from_min_max(
                Pos2::new(x, rect.bottom() - h),
                Pos2::new(x + bar_w.max(1.0), rect.bottom()),
            );
            painter.rect_filled(bar, 0.0, Color32::from_rgb(80, 160, 220));
        }
    }
}

/// Green when in tune, amber when close, red when far; grey when not locked.
fn tune_colour(cents: f64, locked: bool) -> Color32 {
    if !locked {
        return Color32::GRAY;
    }
    let a = cents.abs();
    if a < IN_TUNE_CENTS {
        Color32::from_rgb(80, 220, 120)
    } else if a < 20.0 {
        Color32::from_rgb(230, 190, 70)
    } else {
        Color32::from_rgb(220, 90, 80)
    }
}
