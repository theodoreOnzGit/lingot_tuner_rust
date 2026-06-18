# lingot_tuner_rust

A Rust rewrite of [lingot](https://github.com/ibancg/lingot), a musical instrument tuner.
The original C source lives at `../lingot/src/` and is the reference implementation.

## Status

| Layer | State | Modules |
|---|---|---|
| 1 — Config & Scale | ✅ done (file I/O deferred) | `defs.rs`, `scale.rs`, `config.rs` |
| 2 — Signal processing | ✅ done | `fft.rs`, `window.rs`, `filter.rs`, `signal.rs` |
| 3 — Audio capture (cpal) | ✅ done | `lingot/src/audio.rs` |
| 4 — Core loop | ✅ done (verified on real guitar) | `lingot-tuner/src/core.rs` |
| 5 — GUI (egui) | ⬜ next | — |

**Now a Cargo workspace:** `lingot/` (library, Layers 1–3) + `lingot-tuner/`
(binary, Layers 4–5). `crossbeam-channel` is a binary-only dependency.

**Core concurrency — the key C→Rust difference:** lingot guards a shared
`temporal_buffer` with a mutex (audio ↔ computation) and the results with another
mutex (computation ↔ UI). The Rust core instead uses **message passing**: the audio
callback filters+decimates and *sends* blocks over a `crossbeam` channel; the
computation thread owns the temporal buffer privately; results flow to the UI over a
second channel. The only shared state is an `AtomicBool` stop flag — no shared-buffer
mutex.

**Layer 4 TODO:** sample-rate renegotiation. If the device won't honour the requested
rate, the core currently only warns; it should re-derive `oversampling` and the
dependent params (as `lingot-core.c` does) and keep the audio callback's decimation in
sync. See `Core::start` in `lingot-tuner/src/core.rs`.

38 unit tests passing (`cargo test`), clean build with no warnings. Stateful DSP pieces
(`Filter`, `FrequencyLocker`) are structs with `&mut self`; everything else is pure
functions. `WindowType` lives in `window.rs`. `FftPlan::spectrum()` exposes the complex
FFT for Quinn interpolation.

**Audio (Layer 3) notes:** cpal inverts lingot's model — it drives its own realtime
thread and calls our data callback (no blocking-read mainloop). The whole C multi-backend
registry collapses into one `audio.rs`. `AudioInput::new(config, callback)` delivers mono
`f64` blocks (normalised to `[-1,1]`, multi-channel downmixed by averaging). The callback
runs on the realtime thread — keep it lightweight (push into a channel; no blocking/alloc).
`sample_rate()` may differ from the request; `is_healthy()` mirrors lingot's `interrupted`.

## Crate layout

There is a hard boundary between the library and the binary:

- **`src/lib.rs` — the library crate (Layers 1–3 only).** Pure, reusable, testable
  primitives: config/scale types, signal processing, and the `audio` capture wrapper.
  Declared via `pub mod`. **The library must NOT contain `egui`/`eframe`, application-level
  threading/orchestration, channels between threads, or the core loop.** It exposes
  building blocks (e.g. `AudioInput` takes a callback); it does not spawn or coordinate
  threads itself. (cpal's own internal realtime thread is encapsulated inside `audio.rs`
  and doesn't count — the rule is about *our* orchestration.)
- **`src/main.rs` — the binary crate (Layers 4–5 only).** Depends on the library
  (`use lingot_tuner_rust::...`) and owns everything application-specific: the core loop,
  the multithreading/channel wiring that connects audio → DSP → UI, and the `egui` GUI.
  Layer 4/5 code lives in modules declared from `main.rs` (`mod core; mod gui;`), **not**
  in `lib.rs`.

Rationale: keep the DSP/audio library free of UI and concurrency policy so it stays
portable and unit-testable; confine threading and `egui` to the application.

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

### 4. Core loop  *(binary only — lives in `main.rs` modules, not `lib.rs`)*
- Ties audio capture → signal processing → frequency result.
- Mirror the threading model of `lingot-core.h`: audio runs on its own thread, results are shared with the UI thread.
- Use `Arc` for shared state where needed, but **minimise shared mutable state** to avoid data races.
- Prefer message-passing (`std::sync::mpsc` or `crossbeam`) over mutex-guarded shared buffers wherever possible.
- Mutex usage is acceptable when unavoidable, but document why at each site.

### 5. GUI  *(binary only — lives in `main.rs` modules, not `lib.rs`)*
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

| Crate | Purpose | Status |
|---|---|---|
| `uom` | Physical units in signal processing | added |
| `rustfft` | FFT implementation | added |
| `num-complex` | Complex math for Chebyshev pole/bilinear design | added |
| `cpal` | Cross-platform audio input (Linux + Windows) | added |
| `thiserror` | Library error types (`AudioError`) | added |
| `eframe` + `egui` | GUI | Layer 5 |
| `crossbeam-channel` | Efficient channels between threads | Layer 4 |

Deliberately **not** used: `apodize`/`dasp_window` (windowing), `biquad`, `sci-rs`
(filter design). Window, IIR, and Chebyshev design are written natively so the output
matches lingot's exact coefficients (e.g. optimal Hamming 0.53836/0.46164). `rustfft`
only covers the FFT step.

## Guidelines

- Signal processing is the foundation — implement and unit-test it before wiring up audio or GUI.
- Keep platform-specific code isolated behind `cpal`; do not let ALSA/Windows WASAPI details leak into the core or signal layers.
- No unsafe code unless strictly necessary (e.g. FFI); document any `unsafe` block with a safety comment.
- Default to writing no comments; add one only when the *why* is non-obvious.