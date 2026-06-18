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

//! egui frontend (Layer 5): an analog tuning gauge (in the style of lingot's
//! cairo gauge) plus a live spectrum view.

use std::time::Instant;

use crossbeam_channel::Receiver;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use lingot::config::Config;
use lingot::filter::Filter;
use lingot::scale::Scale;

use crate::core::{Core, TunerResult};
use crate::note::nearest_note;

/// Within this many cents the note is considered in tune (turns green).
const IN_TUNE_CENTS: f64 = 5.0;
/// Half-sweep of the needle, in degrees (matches lingot's `overtureAngle`).
const OVERTURE_DEG: f32 = 65.0;

// Needle-smoothing filter, modelling the needle as a damped spring driven each
// tick toward the target cents. Values and 2nd-order IIR design match lingot's
// gauge filter (lingot-gui-mainframe.c / lingot-io-ui-settings.c defaults).
const GAUGE_RATE: f64 = 60.0; // Hz — fixed update rate the coefficients assume
const GAUGE_ADAPTATION: f64 = 150.0; // quicker to target as this grows
const GAUGE_DAMPING: f64 = 15.0; // less "bounce" as this grows

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

/// Launch the GUI tuner. Blocks until the window is closed.
pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([520.0, 520.0]),
        ..Default::default()
    };

    eframe::run_native(
        "lingot-tuner",
        options,
        Box::new(|_cc| {
            let config = Config::default();
            let scale = config.scale.clone();
            let gauge_range = config.gauge_range;
            let gauge_rest_value = config.gauge_rest_value;
            let (core, results) = Core::start(config)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;
            Ok(Box::new(TunerApp::new(
                core,
                results,
                scale,
                gauge_range,
                gauge_rest_value,
            )))
        }),
    )
}

struct TunerApp {
    // Kept alive to keep capture + computation running; dropped on close.
    _core: Core,
    results: Receiver<TunerResult>,
    scale: Scale,
    /// Full cents range spanned by the gauge (from config).
    gauge_range: f64,
    /// Where the needle rests when no pitch is detected (from config; ≈ −45¢).
    gauge_rest_value: f64,

    frequency: f64,
    note: String,
    cents: f64,
    spd: Vec<f64>,

    // Needle smoothing.
    gauge_filter: Filter,
    /// Smoothed needle position in cents (what the needle actually points at).
    gauge_pos: f64,
    /// Accumulated time for the fixed 60 Hz filter step.
    gauge_accumulator: f64,
    last_frame: Instant,
}

impl TunerApp {
    fn new(
        core: Core,
        results: Receiver<TunerResult>,
        scale: Scale,
        gauge_range: f64,
        gauge_rest_value: f64,
    ) -> Self {
        TunerApp {
            _core: core,
            results,
            scale,
            gauge_range,
            gauge_rest_value,
            frequency: 0.0,
            note: "--".to_string(),
            cents: 0.0,
            spd: Vec::new(),
            gauge_filter: gauge_filter(gauge_rest_value),
            gauge_pos: gauge_rest_value,
            gauge_accumulator: 0.0,
            last_frame: Instant::now(),
        }
    }

    /// Advance the needle-smoothing filter at a fixed 60 Hz, independent of the
    /// display refresh rate, toward the current target (the detected cents, or
    /// the rest value when no pitch is present).
    fn update_gauge(&mut self) {
        let target = if self.frequency > 0.0 {
            self.cents
        } else {
            self.gauge_rest_value
        };

        let dt = self.last_frame.elapsed().as_secs_f64();
        self.last_frame = Instant::now();
        // Cap to avoid a long catch-up burst after a stall.
        self.gauge_accumulator = (self.gauge_accumulator + dt).min(0.25);

        let step = 1.0 / GAUGE_RATE;
        while self.gauge_accumulator >= step {
            self.gauge_pos = self.gauge_filter.filter_sample(target);
            self.gauge_accumulator -= step;
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
        self.update_gauge();

        ui.vertical_centered(|ui| {
            ui.add_space(10.0);
            let locked = self.frequency > 0.0;
            let colour = tune_colour(self.cents, locked);
            ui.heading(egui::RichText::new(&self.note).size(56.0).color(colour));
            if locked {
                ui.label(egui::RichText::new(format!("{:.2} Hz", self.frequency)).size(18.0));
            } else {
                ui.label(egui::RichText::new("listening…").size(16.0).weak());
            }
        });

        ui.add_space(6.0);
        self.draw_gauge(ui);
        ui.add_space(8.0);
        self.draw_spectrum(ui);

        // Repaint continuously so the channel keeps being polled.
        ui.ctx().request_repaint();
    }
}

impl TunerApp {
    /// An analog needle gauge in the style of `lingot-gui-gauge.c`: a cents arc
    /// with minor/major tics and labels, a green/red in-tune band, and a red
    /// needle hinged near the bottom.
    fn draw_gauge(&self, ui: &mut egui::Ui) {
        let w = ui.available_width();
        let h = (w / 1.6).min(260.0);
        let (resp, painter) = ui.allocate_painter(Vec2::new(w, h), Sense::hover());
        let rect = resp.rect;
        painter.rect_filled(rect, 4.0, Color32::WHITE);

        let center = Pos2::new(rect.center().x, rect.top() + h * 0.94);
        let overture = OVERTURE_DEG.to_radians();
        let polar = |r: f32, a: f32| Pos2::new(center.x + r * a.sin(), center.y - r * a.cos());

        let ink = Color32::from_gray(20);
        let cents_bar_r = h * 0.75;
        let gauge_range = self.gauge_range as f32;

        // in-tune band: red across the full sweep, green in the centre
        let ok_r = h * 0.48;
        let ok_stroke = h * 0.07;
        arc(&painter, center, ok_r, -overture, overture,
            Stroke::new(ok_stroke, Color32::from_rgb(221, 170, 170)));
        arc(&painter, center, ok_r, -0.12 * overture, 0.12 * overture,
            Stroke::new(ok_stroke, Color32::from_rgb(153, 221, 153)));

        // cents arc
        arc(&painter, center, cents_bar_r, -1.05 * overture, 1.05 * overture,
            Stroke::new(h * 0.022, Color32::from_rgb(51, 51, 85)));

        // tic spacing (lingot's adaptive division logic)
        let (cents_per_minor, cents_per_major) = divisions(self.gauge_range);
        let cpm = cents_per_minor as f32;
        let cpmaj = cents_per_major as f32;

        // minor tics
        let minor_r = cents_bar_r - h * 0.03;
        let n_minor = (0.5 * gauge_range / cpm).floor() as i32;
        let step_minor = 2.0 * overture * cpm / gauge_range;
        for i in -n_minor..=n_minor {
            let a = i as f32 * step_minor;
            painter.line_segment([polar(minor_r, a), polar(cents_bar_r, a)],
                Stroke::new(h * 0.008, ink));
        }

        // major tics + numeric labels
        let major_r = cents_bar_r - h * 0.045;
        let n_major = (0.5 * gauge_range / cpmaj).floor() as i32;
        let step_major = 2.0 * overture * cpmaj / gauge_range;
        let font = FontId::proportional(h * 0.085);
        for i in -n_major..=n_major {
            let a = i as f32 * step_major;
            painter.line_segment([polar(major_r, a), polar(cents_bar_r, a)],
                Stroke::new(h * 0.022, ink));
            let cents = (i as f32 * cpmaj) as i32;
            let label = if cents > 0 { format!("+{cents}") } else { format!("{cents}") };
            painter.text(polar(major_r - h * 0.10, a), Align2::CENTER_CENTER, label,
                font.clone(), ink);
        }
        painter.text(Pos2::new(center.x, center.y - major_r * 0.80), Align2::CENTER_CENTER,
            "cent", font, ink);

        // needle — uses the smoothed gauge position (which glides toward the
        // detected cents, or toward gauge_rest_value when no pitch is present).
        let clamped = (self.gauge_pos as f32).clamp(-gauge_range / 2.0, gauge_range / 2.0);
        let a = 2.0 * (clamped / gauge_range) * overture;
        let red = Color32::from_rgb(170, 51, 51);
        painter.line_segment([polar(-h * 0.08, a), polar(h * 0.85, a)],
            Stroke::new(h * 0.013, red));
        painter.circle_filled(center, h * 0.045, red);
    }

    fn draw_spectrum(&self, ui: &mut egui::Ui) {
        let (resp, painter) =
            ui.allocate_painter(Vec2::new(ui.available_width(), 120.0), Sense::hover());
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
            let x = rect.left() + i as f32 * bar_w;
            let bar = Rect::from_min_max(
                Pos2::new(x, rect.bottom() - norm * rect.height()),
                Pos2::new(x + bar_w.max(1.0), rect.bottom()),
            );
            painter.rect_filled(bar, 0.0, Color32::from_rgb(80, 160, 220));
        }
    }
}

/// Draw a circular arc as a polyline. Angles are measured from straight up,
/// increasing clockwise (matching the gauge's needle convention).
fn arc(painter: &egui::Painter, center: Pos2, radius: f32, a0: f32, a1: f32, stroke: Stroke) {
    const SEGMENTS: usize = 48;
    let pts: Vec<Pos2> = (0..=SEGMENTS)
        .map(|k| {
            let a = a0 + (a1 - a0) * k as f32 / SEGMENTS as f32;
            Pos2::new(center.x + radius * a.sin(), center.y - radius * a.cos())
        })
        .collect();
    painter.add(Shape::line(pts, stroke));
}

/// lingot's adaptive tic spacing: returns `(cents_per_minor, cents_per_major)`
/// for a gauge spanning `gauge_range` cents (mirrors the 1/2/5/10 logic in
/// `lingot_gui_gauge_redraw_background`).
fn divisions(gauge_range: f64) -> (f64, f64) {
    let mut minor = gauge_range / 20.0;
    let base = 10f64.powf(minor.log10().floor());
    let norm = minor / base;
    let norm = if norm >= 6.0 {
        10.0
    } else if norm >= 2.5 {
        5.0
    } else if norm >= 1.2 {
        2.0
    } else {
        1.0
    };
    minor = norm * base;
    (minor, 5.0 * minor)
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
