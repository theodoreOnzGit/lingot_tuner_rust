# lingot_tuner_rust

A Rust rewrite of [lingot](https://github.com/ibancg/lingot), a musical instrument tuner.
The original C source lives at `vendor/lingot/src/` and is the reference implementation.

## Hard rules

These are standing constraints. They are not suggestions and they do not lapse
between sessions.

### 1. Everything except the GUI must build and test on Termux

Target: `aarch64-linux-android` (what Termux uses). The whole workspace — library,
CLI binary, **and every test target** — must compile there, and `cargo test` must
pass. **Only the egui/eframe GUI is exempt**; nothing else may be excluded, and
"it's just a test helper" is not an exemption.

Consequences that bind every change:

- **No dependency may break the Android target.** Before adding a crate, check it
  builds for `aarch64-linux-android`. Anything that drags in windowing, GL, X11,
  ALSA/PulseAudio dev headers, or an NDK-requiring C/C++ build will not.
- **GUI-only dependencies are declared per-target**, never as a plain dependency:

  ```toml
  [target.'cfg(not(target_os = "android"))'.dependencies]
  eframe = { version = "0.34.3", optional = true }
  ```

  and every item that uses them is gated with
  `#[cfg(all(feature = "gui", not(target_os = "android")))]`. This is why `gui`
  can stay a *default* feature without breaking Termux: on Android the feature
  resolves to no dependency at all.
- **Tests must not require a GUI, an audio device, or a display.** A test that
  touches hardware must tolerate its absence (see
  `audio::tests::listing_input_devices_does_not_panic`) rather than being skipped.
- `cpal` **is not exempt** — it builds for Android via the AAudio/`ndk` backend
  and must keep doing so.

**Gate to run before any commit** (a local proxy for Termux; `check` rather than
`build` because there is no Android linker on the dev box):

```sh
rustup target add aarch64-linux-android      # once
cargo check --target aarch64-linux-android --workspace --all-targets
cargo test --workspace                       # native, must stay green too
```

If that `check` fails, the change is not done.

### 2. Third-party source lives in `vendor/`, cloned, never committed

Any external source you need to read — first and foremost the original C lingot —
lives under `vendor/`, obtained by cloning it from its own git remote. `vendor/` is
in `.gitignore` and stays that way.

```sh
git clone --depth 1 https://github.com/ibancg/lingot.git vendor/lingot
```

Rules:

- **Never reference upstream source by a path outside this repo** (no `../lingot/`,
  no absolute home paths). Those break for everyone but the machine that made them.
  The reference implementation is `vendor/lingot/src/`.
- **Never commit vendored source.** It has its own upstream and its own history; a
  copy here would silently fork. If `vendor/` is missing, re-clone it — that is the
  recovery procedure, and it is why the clone command belongs in this file.
- **Never edit anything under `vendor/`.** It is read-only reference material. A
  change that seems to belong upstream is a patch sent upstream, or a bead here.
- Vendored trees are **not** part of the build. Nothing in `Cargo.toml` may point
  into `vendor/`, and hard rule 1 (Termux) does not apply to what is in there.

### 3. Track work in beads (`bd`)

This repo uses the Rust [beads](https://github.com/steveyegge/beads) issue tracker;
the store lives in `.beads/`. Use it as external memory — it is infrastructure for
the agent, not project management ceremony.

```sh
bd ready              # what can be worked on now
bd create "..."       # promise to handle something later
bd claim <id>         # taking it
bd close <id>         # done
bd prime              # full workflow context
```

Rules:

- When you notice out-of-scope work mid-task — tech debt, a bug, follow-on work —
  **file a bead instead of either derailing or forgetting.** Capture enough context
  that it can be picked up cold.
- Any multi-step task gets a bead before work starts, and the bead is closed (with
  what actually happened) when it lands.
- Never silently drop a known problem. If it is not fixed, it is a bead.

## Status

| Layer | State | Modules |
|---|---|---|
| 1 — Config & Scale | ✅ done (file I/O deferred) | `defs.rs`, `scale.rs`, `config.rs` |
| 2 — Signal processing | ✅ done | `fft.rs`, `window.rs`, `filter.rs`, `signal.rs` |
| 3 — Audio capture (cpal) | ✅ done | `lingot/src/audio.rs` |
| 4 — Core loop | ✅ done (verified on real guitar) | `lingot-tuner/src/core.rs` |
| 5 — GUI (egui) | ✅ done (analog gauge, verified on guitar) | `lingot-tuner/src/gui.rs` |
| 5b — TUI (ratatui) | ✅ done (verified on desktop **and on a Pixel 10a under Termux**) | `lingot-tuner/src/tui.rs` |

**Three binaries** in the `lingot-tuner` package: `lingot-tuner` (GUI, behind the
optional `gui` feature → `eframe`), `lingot-tuner-tui` (terminal gauge, behind the
optional `tui` feature → `ratatui`), and `lingot-tuner-cli` (plain text, always
builds, no frontend deps). Shared code (`core`, `gauge`, `note`) lives in this
package's own internal `lib.rs` — distinct from the reusable `lingot` library.
eframe 0.34: the `App` trait's required method is now `ui(&mut self, ui, frame)`
(not `update`). `gui` is a default feature but resolves to *no dependency* on
Android, so the `lingot-tuner` binary still builds there — it just prints a
pointer to the CLI (hard rule 1).

**Why the TUI exists.** winit's `build.rs` defines
`free_unix = all(unix, not(apple), not(android_platform), ...)` and gates
`x11_platform` on it, so on `target_os = "android"` the X11 backend is compiled
out *unconditionally* — no feature flag brings it back. The egui GUI therefore
cannot run under Termux-X11 at all, and a terminal frontend is the only one that
works natively on Termux. `ratatui` is pure Rust + termios and builds fine for
`aarch64-linux-android`.

**Frontend direction — decided, do not relitigate.** The TUI's linear cents bar
is the intended terminal presentation and is verified on-device. A higher-fidelity
arc gauge drawn with ratatui's `Canvas` (braille 2×4 sub-cells) was prototyped and
**deliberately abandoned** — it worked, but terminal cells are the wrong medium to
keep pushing. If a richer UI is ever wanted, the chosen route is a **Tauri**
frontend, not finer terminal pixels and not Termux:GUI. Options ruled out, with
reasons, so they are not re-explored: Termux:X11 (winit compiles X11 out on
Android), Termux:GUI (requires an add-on app, shared-signature install risk, no
Canvas), proot + X11 (abandons the native-build rule, ~1GB, second audio stack),
and a standalone APK (needs the full Android SDK/NDK toolchain).

**Needle smoothing is shared** (`gauge.rs`, no frontend deps): `Needle` owns
lingot's 2nd-order damped-spring IIR and steps it at a fixed 60 Hz via a time
accumulator, so the GUI (60+ Hz) and the TUI (≈30 Hz) show identical motion.
`advance()` reads the clock; `advance_by(target, dt)` takes explicit elapsed time
so the motion is deterministically testable — a tight test loop over `advance()`
advances no simulated time and the needle looks frozen.

**GUI gauge** (`gui.rs`) is a hand-painted port of lingot's cairo gauge: cents arc
with adaptive tics/labels, green/red in-tune band, needle hinged near the bottom.
The needle is smoothed by lingot's 2nd-order "damped spring" IIR (reusing
`lingot::filter::Filter`; constants `k`=adaptation 150, `q`=damping 30, rate 60 Hz)
driven at a **fixed 60 Hz timestep** via a time accumulator so motion is
refresh-rate-independent. It rests at `gauge_rest_value` (≈ −45¢) when unlocked.
Readout shows note + cents-off-tune + Hz.

**Now a Cargo workspace:** `lingot/` (library, Layers 1–3) + `lingot-tuner/`
(binary, Layers 4–5). `crossbeam-channel` is a binary-only dependency.

**Core concurrency — the key C→Rust difference:** lingot guards a shared
`temporal_buffer` with a mutex (audio ↔ computation) and the results with another
mutex (computation ↔ UI). The Rust core instead uses **message passing**: the audio
callback filters+decimates and *sends* blocks over a `crossbeam` channel; the
computation thread owns the temporal buffer privately; results flow to the UI over a
second channel. The only shared state is an `AtomicBool` stop flag — no shared-buffer
mutex.

**Decimation lives on the computation thread, not the audio callback.** The audio
callback is a lightweight forwarder (just sends raw mono blocks); a `Decimator`
(stateful: anti-alias IIR + decimation phase) on the computation thread does the
filtering/downsampling. This keeps the realtime callback trivial and means
**sample-rate renegotiation just works**: if the device won't honour the requested
rate, `Core::start` adopts the real rate and re-derives the dependent params
(`config.update_internal_params()`) *before* spawning the computation thread, so all
rate-dependent DSP uses the correct rate (mirrors `lingot-core.c`).

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

All paths are relative to `vendor/lingot/src/` (see hard rule 2).

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
| `eframe` + `egui` | GUI | added — **`cfg(not(target_os = "android"))` only** |
| `ratatui` | Terminal frontend (Termux/Android) | added — optional `tui` feature |
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