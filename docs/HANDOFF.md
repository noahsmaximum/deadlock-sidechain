# Sidechain — handoff (2026-10-16)

State at `e3b1a32`, local git only (no remote). Next up: **optimizations**.

## What it is
Deadlock gameplay drives Spotify. A Panorama hook reads the HUD and prints `[SIDECHAIN]{json}` to
console.log (`-condebug`) or netconsole (`-netconport 29000`). A Rust engine turns that into
signals → **energy** → a route matrix → DSP params. Spotify is re-pointed to a silent sink
(Steam Streaming Speakers), captured by process loopback, processed (stems + EQ chain) and played on
the real output. There's an egui settings window with a tray icon. The user tunes heavily in the GUI:
**never overwrite their `sidechain.toml`; patch it minimally.**

## Layout
| File | Role |
|---|---|
| `addon/panorama/scripts/sidechain_hook.js` | hook v0.1.2, ~30 Hz HUD poll (deployed via `build_addon.ps1 -Deploy`, game closed) |
| `src/game.rs` | netcon + console.log tail, `[SIDECHAIN]` parse, session/seq dedupe |
| `src/control.rs` | signals/envelopes on an internal `clock`, `in_match()`, energy, routes → `raw_targets()`, stem fades → `targets()`, `stem_fade_db` |
| `src/engine.rs` | engine thread: lines → control → `Shared.targets`, router, config apply/hot-reload, demo, snapshot (33 ms), `SIDECHAIN_TRACE` |
| `src/audio.rs` | capture (process loopback) → ring → `Resampler` (cubic + PI drift) → `StemProc` (Dsp / Neural) → `dsp::Chain` → render. **RATE = 44.1 kHz** |
| `src/stems.rs` | Fast stems: STFT 2048/512, causal HPSS + centre coherence, ~43 ms |
| `src/neural.rs` | HT-Demucs forward-only ONNX via `ort` rc.14 + DirectML; own torch-exact STFT/iSTFT; streaming (hop, 1 s right context, crossfade, Fast fallback for late slices, coverage) |
| `src/dsp.rs` | SVF chain + `response_db` for the GUI curve |
| `src/gui.rs` | egui window (Live / Energy / Signals / Routes / Text rules / Audio / Log), tray, off-screen screenshot env vars |
| `src/app.rs`, `routing.rs` | per-app output override (AudioPolicyConfig), verification, guardian process |
| `tools/*.py` | offline checks (`offline_check.py`, `stems_check.py`) |

## Measured numbers (baseline for optimization)
- Neural separation of one 7.8 s window: **0.43–0.48 s on GPU idle**, **4.3–5.5 s on GPU while Deadlock
  runs** (my test instance was also competing), 2.7 s CPU at 8 threads, 3.4 s CPU at 2 threads.
- Neural latency = 1.0 + hop + 0.25 + 0.6 s (3.85 s at hop 2).
- Real match (95 min): energy mean ~0.5; hook lines ~17/s.
- Release exe is 32 MB (ort is statically linked).

## Optimization candidates (roughly by payoff)
1. **Neural GPU cost in-game.** This is the real blocker. Options:
   - True fp16 compute on DirectML: convert the graph to fp16, or use an fp16 IO-bound session. That
     should roughly halve GPU time.
   - IoBinding: keep inputs and outputs on the GPU, and skip the ~13.7 M-float output copy plus the
     `to_vec` per run.
   - Skip separation when it can't matter: all stem targets ≈ 0 dB (full energy), or the stems are
     held at calm and their gains are unchanged. Also, the time and frequency branches are both always
     computed.
   - Adaptive hop: pick the interval from measured inference time instead of the user's guess.
   - DirectML queue priority and `PerformancePreference::MinimumPower`. Or the CPU EP at
     below-normal priority (the CPU option exists; measure it in-game).
   - A lighter model (e.g. a 2-stem vocals/accompaniment MDX model plus Fast drums).
2. **Neural CPU-side work per job:** the spec/ispec run 2 + 8 FFT passes single-threaded, plus
   `Vec` allocations of 2×W, 4×FR×T and 8×W per job. Parallelize (8 independent iSTFTs), preallocate
   and reuse buffers, cache the FFT planners (already cached) and the window.
3. **Fast stems (`stems.rs`):** the per-bin `select_nth_unstable` time and frequency medians at 94
   frames/s × 1025 bins. Could use a running/sliding median, lower the time history, or use SIMD. In
   neural mode it now *always* runs too, as the fallback.
4. **Render thread:** `Vec<u8>` / `Vec<f32>` conversions per callback (reused, but there's a byte
   copy); the `Chain` recomputes `tan`/`powf` coefficients every 16 frames.
5. **Capture:** VecDeque<u8> → flat Vec copy each packet.
6. **GUI:** `response_db` for 3 curves × 240 freqs every frame at 30 fps; a `Snapshot` clone with
   Strings every frame; check repaint stops while hidden.
7. **Engine loop:** a 5 ms tick; `raw_targets()` is called multiple times per tick (`targets()` +
   the stem smoothing + the trace).
8. **Hook:** the 30 Hz Panorama poll and `labelTexts` traversal. Check Panorama frame cost.
9. **Log tail** polls every 10 ms while the log is moving.

Measure first: add timing counters (per-stage µs in the render callback, the neural breakdown of
spec / run / ispec, and the engine tick) and surface them in the Log tab / `SIDECHAIN_TRACE`.

## Gotchas (don't relearn)
- DirectML: the full-graph HT-Demucs export OOMs/hangs; the forward-only export needs
  `ep.dml.disable_graph_fusion=1` or the outputs are garbage. Verify with `sidechain neural-sum <wav>`.
- One ONNX Runtime Environment per process (`neural::env()`).
- There's no cuDNN on this machine, and the CUDA 13.x install dirs are empty (only 12.9 is real).
- The user's sidechain.exe locks `target/release`: build into `CARGO_TARGET_DIR=target-dev` while it
  runs.
- Never pop windows over the game: use `SIDECHAIN_WINDOW_POS=-4000,-4000` + `shot.ps1`
  (PrintWindow).
- `sidechain capture <secs> <wav>` grabs real Spotify audio for offline renders (`render --config`).
- `build_addon.ps1 -Deploy` only scans VPKs < 1 MB (the addons folder is 22 GB).
- `presets/NMv1.toml` is the user's own preset; it's in git (ask before ignoring the folder).
