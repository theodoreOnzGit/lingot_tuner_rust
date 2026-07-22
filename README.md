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

### 3. Frontends

In this rewrite the computation thread sends each result (frequency + noise-subtracted SPD) to the frontend over a channel; the frontend renders at its own rate. Four frontends share the same core:

- **`lingot-tuner-cli`** — prints the detected note, cents, and frequency to the terminal.
- **`lingot-tuner-tui`** — a [ratatui](https://ratatui.rs) terminal gauge: note, cents, frequency, and a needle on a cents scale. It runs anywhere, including Android/Termux, where the GUI cannot (see [Android / Termux](#android--termux)). It uses the same needle smoothing as the GUI, so the motion is identical.
- **`lingot-tuner-web`** — serves a small page over HTTP and streams readings to it over a WebSocket, so any browser becomes the display: the same analog gauge as the GUI, drawn on a canvas, plus the spectrum. This is the frontend with real graphics on Android — the phone's own browser does the rendering, so no windowing stack, NDK or add-on app is involved.
- **`lingot-tuner`** — an [egui](https://github.com/emilk/egui) GUI rendering:
  - the **note name**, the **cents off-tune**, and the **frequency**;
  - an **analog tuning gauge** in the style of lingot's cairo gauge — a cents arc with minor/major tics and labels, a green/red in-tune band, and a needle hinged near the bottom. The needle is smoothed by a 2nd-order "damped spring" IIR filter (ported from lingot) driven at a fixed 60 Hz, and rests near the left (`gauge_rest_value`) when no pitch is present;
  - a live **spectrum** view of the SNR distribution.

## Building

The project is a Cargo workspace with two crates:

- **`lingot/`** — the library (Layers 1–3): config/scale types, signal processing, and audio capture. No GUI or threading dependencies.
- **`lingot-tuner/`** — the binary package (Layers 4–5): the core loop and the CLI/GUI frontends.

From the workspace root:

```
cargo build --release
```

## Running

The `lingot-tuner` package builds four binaries:

- **`lingot-tuner`** — the graphical (egui) tuner, behind the optional `gui` feature.
- **`lingot-tuner-tui`** — a terminal gauge, behind the optional `tui` feature.
- **`lingot-tuner-web`** — a browser tuner, behind the optional `web` feature.
- **`lingot-tuner-cli`** — a command-line tuner (always builds, no frontend dependencies).

`gui`, `tui` and `web` are all enabled by default, so `cargo install lingot-tuner` gives you all four.

### GUI tuner

The graphical tuner shows the note, cent deviation, an analog tuning gauge (in
the style of lingot's), and a live spectrum. It is behind the `gui` feature:

```
cargo run --release --bin lingot-tuner --features gui
```

### Terminal tuner

```
cargo run --release --bin lingot-tuner-tui
```

```
┌────────────────────lingot-tuner────────────────────┐
│                        A4                          │
│                     440.00 Hz                      │
│-50          -25             0            +25    +50│
│├─────────────┼──────────────╫─────────────┼───────┤│
│                             ▲                      │
│                    +2.3 cents                      │
│                     in tune ✓                      │
└────────────────────────────────────────────────────┘
```

Quit with `q`, `Esc`, or `Ctrl-C`.

### Browser tuner

Serves the gauge to any browser — including the one on the phone that is running
it. The page is embedded in the binary, so there is nothing to install or fetch:

```
cargo run --release --bin lingot-tuner-web
```

Then open <http://127.0.0.1:8080/>. You get the same analog gauge as the GUI, a
live spectrum, and the note/cents/frequency readout.

It binds loopback by default. Pass an address to reach it from another machine —
handy for putting the phone next to the amp and tuning from a laptop:

```
lingot-tuner-web 0.0.0.0:8080
```

Note that this makes the readings visible to anyone who can reach the port; it is
opt-in for that reason.

### Android / Termux

Everything except the egui GUI builds and runs natively on Termux:

```
pkg install rust
cargo install lingot-tuner
lingot-tuner-web        # then open http://127.0.0.1:8080/ in the phone's browser
lingot-tuner-tui        # or stay in the terminal
```

**The graphical tuner cannot work under Termux, and no configuration changes
that.** winit gates its X11 backend on `free_unix`, which explicitly excludes
`target_os = "android"`, so the backend is compiled out — running the Termux:X11
app will not help. Use the browser or terminal tuner instead; the browser one is
the way to get a real gauge and a spectrum on a phone. (`lingot-tuner` still
builds on Android; it just tells you to use another binary.)

**Grant the microphone permission**, or audio will fail to start:

1. Check that the **Termux:API** add-on app is installed. It declares
   `RECORD_AUDIO` and shares Termux's UID, which is what makes the permission
   grantable at all.
2. Grant it: *Settings → Apps → Termux → Permissions → Microphone*
   (or over adb: `pm grant com.termux android.permission.RECORD_AUDIO`).

Without this, capture fails with an AAudio error that says nothing about
permissions — so the binaries print this advice when startup fails on Android.

### iOS — not supported, and not cheap to add

There is no iOS build, and the cost of adding one is a licensing and hardware
problem rather than a code problem. Recorded here so nobody assumes otherwise:

- **A Mac is required.** Xcode is macOS-only, the iOS SDK ships *inside* Xcode,
  and `codesign` is a macOS binary. Apple's SDK licence further restricts use to
  Apple-branded hardware, so cross-compiling from Linux is on shaky ground even
  where it technically works. You do not have to *own* one — GitHub Actions
  provides macOS runners, free for public repositories — but something running
  macOS has to build and sign every release.
- **An Apple Developer Program membership is required** (~$99/year) to install on
  a physical device beyond short-lived development builds, to use TestFlight, or
  to ship to the App Store. There is no sideloading equivalent to handing someone
  an APK.
- **A terminal frontend is not a way around this.** iOS has no system shell, and
  App Store guideline 2.5.2 forbids apps that download, install, or execute code;
  the OS enforces it, so only code signed into an app bundle runs. Terminal apps
  like a-Shell (commands linked *into* the app) and iSH (a usermode x86 emulator)
  work around that rather than through it — and neither can give a process the
  microphone entitlement, so the Termux approach has nowhere to land.

The DSP itself would port without drama: the `lingot` crate is pure and
frontend-free, and `cpal` supports iOS via CoreAudio. It is the build and
distribution pipeline, not the code, that costs.

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
