# lingot_tuner_rust

A musical instrument tuner — Rust rewrite of [lingot](https://github.com/ibancg/lingot).

## How it works

### Threading model

Three concurrent entities cooperate:

- **Audio thread** — hardware callback, fires continuously with raw PCM chunks
- **Computation thread** — runs the DSP pipeline at a fixed `calculation_rate`
- **GUI thread** — polls results at its own redraw rate

**Original C (lingot):** the three share state behind **two mutexes** — one guards the temporal sample buffer (audio ↔ computation), another guards the frequency/SPD results (computation ↔ GUI).

**This Rust rewrite:** the same entities communicate by **message passing** instead. The audio callback filters, decimates, and *sends* sample blocks over a channel; the computation thread owns the temporal buffer privately (no lock needed), and sends results to the UI over a second channel. The only shared state is an atomic stop flag. This removes the shared-buffer mutex entirely and keeps each buffer owned by a single thread.

### 1. Audio capture and decimation (audio thread)

The audio callback receives a raw PCM chunk and appends it to a **sliding window queue** (`temporal_buffer`): new samples shift in at the end, old ones fall off the front.

If `oversampling > 1`, an **8th-order Chebyshev IIR low-pass filter** is applied first to prevent aliasing, then the signal is **downsampled** by taking every Nth sample. The cut frequency is set to `0.9 / oversampling`, leaving a 10% margin against non-ideal filter roll-off.

### 2. Frequency estimation (computation thread)

Each iteration of the computation loop runs the following pipeline:

**a) FFT-based coarse estimate**

The most recent `fft_size` samples are extracted from the temporal buffer and multiplied by a **Hanning or Hamming window**. An FFT is computed (either FFTW or a built-in Cooley-Tukey implementation). The spectral power distribution (SPD) is computed as the normalized squared magnitude and converted to dB.

A short-window IIR low-pass filter smooths the SPD to estimate the noise floor; this is subtracted, yielding an SNR spectrum.

**b) Peak detection and harmonic analysis**

The top N peaks above the SNR threshold are identified. For each candidate, the subbin frequency is refined using **Quinn's 2nd estimator** — a complex-valued interpolation using the three FFT bins surrounding the peak.

Peaks are then tested for harmonic relationships: for each candidate fundamental (each peak divided by integer divisors up to 4), the algorithm counts how many other peaks land on integer multiples. A quality score (sum of SNRs of harmonically-related peaks, weighted by frequency) selects the best fundamental. This handles instruments whose fundamental may be weak or absent in the spectrum.

**c) Newton-Raphson refinement (two passes)**

The coarse FFT estimate is refined by Newton-Raphson iteration on the **analytic DTFT power derivatives** — computed directly at an arbitrary frequency, not constrained to FFT bins. Pass 1 uses the shorter FFT window. Pass 2 uses the **full temporal buffer** for higher resolution. The iteration stops when the frequency change is below 1e-4 rad or power starts decreasing (indicating divergence).

This two-stage refinement (FFT bin → Quinn interpolation → Newton-Raphson on full buffer) is the core accuracy mechanism: the FFT gives a coarse bin, Quinn gives subbin accuracy, and Newton-Raphson on the longer window pushes resolution well below what the FFT size alone would allow.

**d) Frequency locker**

A state machine filters out transient glitches. It requires several consistent readings before "locking" onto a note, and several consecutive failures before unlocking. It also handles **octave jumps** (half/double frequency detections) by requiring a run of consistent readings before re-locking at the new octave.

### 3. GUI

The GUI reads the latest frequency and noise-subtracted SPD (copied under the results mutex) at its own redraw rate and renders:

- **Tuning gauge** — deviation from the nearest note in cents
- **Spectrum display** — the noise-subtracted SPD
- **Strobe disc** — a rotating pattern whose speed reflects pitch error

## Building

The project is a Cargo workspace with two crates:

- **`lingot/`** — the library: config/scale types, signal processing, and audio capture.
- **`lingot-tuner/`** — the binary: the core loop and (eventually) the GUI.

From the workspace root:

```
cargo build --release
```

## Running

The `lingot-tuner` package builds two binaries:

- **`lingot-tuner-cli`** — a working command-line tuner (always builds).
- **`lingot-tuner`** — the egui GUI; behind the optional `gui` feature and **not
  yet finished**.

### CLI tuner

Run the command-line tuner in release mode (recommended for smooth, low-latency
analysis):

```
cargo run --release --bin lingot-tuner-cli
```

It listens on your default input device and prints a line whenever it locks onto
a tone, for example:

```
lingot-tuner — listening (Ctrl-C to quit)

  220.00 Hz   A3    +0.3 cents
```

Notes:

- **Silence produces no output** — the frequency locker needs several consistent
  readings before it reports a pitch.
- The default configuration covers the guitar range (E2–E4, ~82–330 Hz). Play or
  whistle within that range to see it track.
- It captures from the **system default input device**. On PulseAudio/PipeWire you
  can pick a specific microphone first with `pactl set-default-source <name>`
  (list candidates with `pactl list short sources`).
- Quit with **Ctrl-C**.

## License

Copyright (C) 2004-2020 Iban Cereijo  
Copyright (C) 2004-2008 Jairo Chapela  
Copyright (C) 2026 lingot_tuner_rust contributors

This program is free software: you can redistribute it and/or modify it under
the terms of the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY
WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A
PARTICULAR PURPOSE. See the GNU General Public License for more details.

You should have received a copy of the GNU General Public License along with
this program. If not, see <https://www.gnu.org/licenses/>.
