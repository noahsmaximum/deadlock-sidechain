# Sidechain — handoff (2026-10-16, after the neural optimization pass)

Local git only (no remote). The neural pass is done (see *Neural v2* below). Next up: **the user checks it in a real match**, then the remaining optimizations.

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
| `src/neural.rs` | HT-Demucs forward-only ONNX (fp16 copy preferred) via `ort` rc.14 + DirectML; own torch-exact STFT/iSTFT (threaded); streaming = per-sample weighted blend of every window covering it; GPU priority policy (D3DKMT); `neural-bench` / `neural-stream` test commands |
| `src/bin/gpuload.rs` | stand-in for a game's GPU load (off-screen uncapped shader, prints fps + worst frame): `gpuload 3000 <secs>` ≈ 139 fps, 100 % GPU |
| `tools/convert_fp16.py` | makes `models/htdemucs_fwd_fp16.onnx` |
| `src/dsp.rs` | SVF chain + `response_db` for the GUI curve |
| `src/gui/` | egui window: `mod.rs` shell (rail, header, save bar, tray, presets dialog), `theme.rs` (palette, Segoe UI + Phosphor icons, widgets: card, toggle, segmented, range_slider, meters, eq_plot), `catalog.rs` (plain names for signals/params), pages `now.rs`, `sound.rs`, `moments.rs`, `setup.rs`. Screenshots: `SIDECHAIN_TAB=now|sound|moments|setup`, `SIDECHAIN_MOMENT=<signal>`, `SIDECHAIN_WINDOW_POS=-4000,-4000`, `SIDECHAIN_WINDOW_SIZE=w,h`, then `tools/shot.ps1` |
| `src/app.rs`, `routing.rs` | per-app output override (AudioPolicyConfig), verification, guardian process |
| `tools/*.py` | offline checks (`offline_check.py`, `stems_check.py`) |

## Measured numbers (baseline for optimization)
- Neural separation of one 7.8 s window: **0.43–0.48 s on GPU idle**, **4.3–5.5 s on GPU while Deadlock
  runs** (my test instance was also competing), 2.7 s CPU at 8 threads, 3.4 s CPU at 2 threads.
- Neural latency = 1.0 + hop + 0.25 + 0.6 s (3.85 s at hop 2).
- Real match (95 min): energy mean ~0.5; hook lines ~17/s.
- Release exe is 32 MB (ort is statically linked).

## Neural v2 (2026-10-16): what was measured and decided
The user wanted neural only (no switching to Fast, which sounded like a quality shift every few seconds), GPU only, faster and lighter.
- **fp16** (`tools/convert_fp16.py`, keep_io_types): network 365 → 85 ms on an idle 4070, 45–52 dB SNR vs fp32. The whole
  separation (spec + net + ispec, threaded) went from 415 → ~110 ms.
- **DirectML knobs don't matter:** graph fusion is no faster and still wrong (fp32: +90 dB garbage, fp16: NaN). Metacommands
  off, graph capture, and performance preference made no difference.
- **Contention** (`gpuload`, 139 fps game stand-in): normal priority 415 → 1330 ms fp32 / 425 ms fp16, and the game is
  unaffected. Process GPU priority REALTIME (`D3DKMTSetProcessSchedulingPriorityClass(…, 5)`, works without admin; 3 and 4 do
  nothing) makes it ~115 ms under load, but costs the game a ~45–55 ms frame per separation. Raising it during warm-up
  (kernel compile) caused a 290 ms hitch, so priority is only raised after a warm-up run.
- **Policy:** `neural_priority = auto` stays normal and raises only after 2 separations over MARGIN+250 ms (1.1 s). Once raised,
  it tries normal again every 30 s. Also `normal` and `high`.
- **Streaming:** latency = RIGHT 1.0 + hop + MARGIN 0.85 (3.85 s at hop 2, same as before). The output sample is
  `mix + Σ(g−1)·stem`, where stem = Σ wₖ·stemₖ / Σ wₖ over arrived windows (weight = smoothstep(left ctx / 0.5 s) ·
  smoothstep(right ctx / 1 s) + 1e-4, plus a 4096-sample fade-in per newly arrived window). A late window costs look-ahead,
  not a switch. A sample with no window at all plays unseparated, with a short de-click; that only happens if separations
  take longer than ~hop + margin. Silent windows skip the GPU. Buffers are recycled.
- **Results** (`neural-stream --realtime`, 48 s clip): idle / 1 / 2 / 3 load instances → 0 unseparated samples; full
  look-ahead 100 / 100 / 100 / 92 %; real-time vs offline render 45–47 dB SNR; unity exact.
- **Not verified:** a real Deadlock match. Deadlock's contention may be worse than `gpuload`'s (fp32 was 4.3–5.5 s in-game
  vs 1.3 s under gpuload). Ask the user for the Log tab lines (`separations running late … raised`) and the stems status pill.
- Config: `neural_device` is now ignored (read, never written). The CPU option was removed on purpose.

## Bass stem (2026-10-16, ddb9a96)
- Neural-only fourth stem `bass_stem_db` (Param::BassStemDb, short name "bassline", title "Bass"). STEM_PARAMS is now
  [drums, vocals, music, bass]; music = demucs "other" in neural mode. Fast mode ignores bass (`[db[0..3]]`, level -90) and
  the GUI greys out its row and hides its strip.
- Defaults: base -30, energy route 30-70 %. The user's sidechain.toml got base -40 + route 0.3-0.7 (their style).
- Verified: build, tests, and the bass-removed render: removed = 99 % < 250 Hz, centroid 73 Hz, kick kept. Unity at 0 dB is
  structurally unchanged but wasn't re-run (the user was in Deadlock). GUI not screenshotted for the same reason.
- First real-Deadlock data point: fp16 offline separations took 0.3-1.3 s each at normal priority while the game ran
  (fp32 was 4.3-5.5 s).

## Hook v0.1.3: wrong deaths / kills fixed (2026-10-16)
From the user's real log (hideout, then 2 matches; copy in session scratchpad kd/):
- The cached `.LocalPlayer` slot was reused for another player once the match started, so `dead` followed an enemy
  (it flipped in the same second as our kills and never when our HP hit 0).
- While dead, the HUD spectates a teammate, so their HP / ammo / hits read as ours.

Fixes:
- The hook re-checks `.LocalPlayer` on every read, and reports `spec` (another slot has `.SpectatorTarget`), `k` (our
  `.KillsLabel`) and `abn` (ability names, only while alive and not spectating).
- The engine ignores hp / ammo / speed / hit / ability input while dead or spectating, and re-baselines on transitions.
- Kills come from `k` increments once known; impact kills and the ks fallback remain for older hooks. Unit tests in
  control.rs.
- Venator Verses (`Projects\deadlock-venator-verses`, no git, backup `.bak-YYYYMMDD`) now reads `[SIDECHAIN]` lines too.

**NOT verified in a match yet:** the `.KillsLabel` text is assumed to be the kill count. Check `k` in the next log
(`sidechain scan`, or grep `"k":`).

## Optimization candidates (roughly by payoff)
1. **Neural, still open:** IoBinding (keep the ~55 MB of in/out tensors on the GPU); skipping separation when stems are
   all at 0 dB for a while; adaptive hop from measured time; a dedicated high-priority D3D12 compute queue via
   `SessionOptionsAppendExecutionProvider_DML1` (maybe gentler on the game than process-wide REALTIME); a lighter model.
2. **Neural CPU side:** spec/ispec are now threaded (≈15 + 17 ms). Remaining: the 44 MB `x_out` read and some per-job Vecs.
3. **Fast stems (`stems.rs`):** the per-bin `select_nth_unstable` time and frequency medians at 94
   frames/s × 1025 bins. Could use a running/sliding median, lower the time history, or use SIMD. (It no longer runs in neural mode.)
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
