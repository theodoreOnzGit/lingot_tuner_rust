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