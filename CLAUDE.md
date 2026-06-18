# lingot_tuner_rust

A Rust rewrite of [lingot](https://github.com/ibancg/lingot), a musical instrument tuner.
The original C source lives at `../lingot/src/` and is the reference implementation.

## Crate layout

- **`src/lib.rs`** — the `lingot_tuner_rust` library crate. All reusable, testable
  modules live here (signal processing, config, core). Declared via `pub mod`.
- **`src/main.rs`** — the binary entry point. Depends on the library
  (`use lingot_tuner_rust::...`) and wires up audio capture + GUI.
- Keep logic in the library; `main.rs` should stay thin.

## Architecture

The project is structured in layers, built bottom-up:

### 1. Config & Scale types
- Mirror `lingot-config.h` and `lingot-config-scale.h` as idiomatic Rust structs.
- Use `uom` crate for physical quantities (frequencies in Hz, time in seconds, etc.) where it adds clarity.

### 2. Signal processing
- **Written in native Rust** — no FFI to C signal processing libraries.
- Covers: FFT (via `rustfft`), IIR filtering (`lingot-filter`), windowing (Hanning/Hamming), peak detection, Newton-Raphson frequency refinement (`lingot-signal`).
- Use the `uom` crate for units where appropriate.
- All signal processing code must be pure functions / unit-testable with no I/O dependencies.

### 3. Audio capture
- Use the `cpal` crate for audio input.
- **Must be cross-platform: Linux and Windows.**
- No ALSA/JACK/OSS/PulseAudio direct calls — `cpal` abstracts these.
- Audio delivers samples via a callback; keep the callback lightweight (no allocation, no blocking).

### 4. Core loop
- Ties audio capture → signal processing → frequency result.
- Mirror the threading model of `lingot-core.h`: audio runs on its own thread, results are shared with the UI thread.
- Use `Arc` for shared state where needed, but **minimise shared mutable state** to avoid data races.
- Prefer message-passing (`std::sync::mpsc` or `crossbeam`) over mutex-guarded shared buffers wherever possible.
- Mutex usage is acceptable when unavoidable, but document why at each site.

### 5. GUI
- Use **egui** (via `eframe`).
- Replicate the core UI of lingot: tuning gauge, spectrum display, strobe disc.
- The GUI polls or receives frequency results from the core via a channel — it must never block the audio thread.

## DSP pipeline (how the original works)

Understanding this is essential before touching any signal processing code.

**Audio thread** receives raw PCM and appends it to a sliding `temporal_buffer` (a ring-queue). If `oversampling > 1`, an 8th-order Chebyshev IIR anti-alias filter runs first, then the signal is downsampled by taking every Nth sample.

**Computation thread** runs at `calculation_rate` Hz. Each iteration:

1. **Window + FFT** — take the most recent `fft_size` samples, apply Hanning/Hamming window, run FFT. Compute SPD as normalized squared magnitude in dB.
2. **Noise floor subtraction** — a short-window IIR smooths the SPD into a noise estimate; subtract it to get an SNR spectrum.
3. **Peak detection** — find the top N peaks above the SNR threshold. Refine each peak's subbin frequency using **Quinn's 2nd estimator** (complex interpolation over the three surrounding FFT bins).
4. **Harmonic analysis** — for each peak / divisor (up to 4), test how many other peaks are integer multiples. A quality score (sum of related-peak SNRs, weighted by frequency) picks the best fundamental. This is how lingot finds the true fundamental even when the lowest partial is weak or absent.
5. **Newton-Raphson refinement (two passes)** — refine with NR on the analytic DTFT power derivatives (`lingot_fft_spd_diffs_eval`). Pass 1 uses the FFT window; pass 2 uses the full (longer) `temporal_buffer` for higher resolution. This is the main accuracy mechanism — resolution scales with window length, not FFT size.
6. **Frequency locker** — state machine that requires N consistent readings to lock, N failures to unlock, and handles octave-jump artifacts.

**GUI thread** reads frequency + SPD (under a mutex) and renders the gauge, spectrum, and strobe disc.

## Reference files (original C)

| Concern | C file |
|---|---|
| Config | `lingot-config.{c,h}`, `lingot-config-scale.{c,h}` |
| FFT | `lingot-fft.{c,h}` |
| Filter | `lingot-filter.{c,h}` |
| Signal / peak finding | `lingot-signal.{c,h}` |
| Audio abstraction | `lingot-audio.{c,h}` |
| Core loop | `lingot-core.{c,h}` |
| GUI | `lingot-gui-*.{c,h}` |

## Key constants (from `lingot-defs.h`)

```rust
const MID_A_FREQUENCY: f64 = 440.0;   // Hz
const MID_C_FREQUENCY: f64 = 261.625565; // Hz
```

## Cargo.toml dependencies

| Crate | Purpose |
|---|---|
| `uom` | Physical units in signal processing |
| `cpal` | Cross-platform audio input (Linux + Windows) |
| `rustfft` | FFT implementation |
| `eframe` + `egui` | GUI |
| `crossbeam-channel` | Efficient channels between threads |

## Guidelines

- Signal processing is the foundation — implement and unit-test it before wiring up audio or GUI.
- Keep platform-specific code isolated behind `cpal`; do not let ALSA/Windows WASAPI details leak into the core or signal layers.
- No unsafe code unless strictly necessary (e.g. FFI); document any `unsafe` block with a safety comment.
- Default to writing no comments; add one only when the *why* is non-obvious.