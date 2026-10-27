# Sidechain

Deadlock gameplay drives your Spotify. With nothing going on, the track sits back: near-mono,
quieter, no bass, with the **drums, vocals and bassline stripped out**. As more happens at once (moving fast,
fighting, shooting, landing hits, kills), **energy** builds. The vocals come back in through the
middle of the range, the drums near the top, and the track opens up to full level, wide stereo,
normal bass and a little extra air. On top of that, individual events have their own moves: shots pump the low end like a
sidechain compressor, getting hit muffles the track, dying drops it underwater, kills and
objectives hit like a drop.

It works in normal matchmaking: client-side only, with no server plugin and no memory reading.

## Install

1. **The app:** `Sidechain.exe` is the whole thing. The neural model, DirectML and the hook addon
   are built in. Download it from [Releases](https://github.com/noahsmaximum/deadlock-sidechain/releases)
   and put it anywhere (Windows may warn that it's unsigned: More info → Run anyway). Or build and
   install it with `.\tools\package.ps1 -Install`, which:
   - puts it in `%LOCALAPPDATA%\Programs\Sidechain`
   - adds a Start menu entry
   - copies this repo's settings to `%APPDATA%\Sidechain`, where the app keeps its settings and presets

   Or just run `dist\Sidechain.exe` from anywhere. Closing the window hides it to the **tray**.
   Right-click the tray icon → Quit to stop, which gives Spotify its output back. Setup → App →
   **Start with Windows** starts it hidden in the tray when you sign in. Launching it again while
   it runs brings the window back.
2. **Hook addon:** Setup → Connection → Game data → **Get the addon file** saves
   `sidechain-hook.vpk` to Downloads. Add it in **Deadlock Mod Manager** as a local mod. Later
   versions offer **Update addon** there (with the game closed). The addon overrides
   `citadel_hud_top_bar`, so disable the old `DEADLOCK_DEATH_HOOK` / Venator kill hook.
   (Venator Verses reads Sidechain's lines too.)
3. **Steam → Deadlock → Launch options:** `-netconport 29000 -condebug` (Setup has a Copy button).
4. Start Spotify and play. Spotify is only taken over while `deadlock.exe` runs.

**Developing:** `run.bat` still builds and runs from the repo with the repo's `sidechain.toml`
(a `sidechain.toml` in the working folder or next to the exe takes priority over `%APPDATA%`).
From a terminal the exe prints as before; double-clicked it has no console window.

To hear everything without the game, use **Try one → Play a 25 s demo match** on the Now page (or run
`Sidechain.exe --demo`): a scripted 25 s fight with walking, sprinting, shooting, a kill, an ult, a death,
a respawn and an objective.

## The settings window

Four places, on the left:

| Page | What's there |
|---|---|
| **Now** | Energy, with where each stem comes back marked on the bar, and what's building it right now. The tone curve now vs calm vs full. Stems as channel strips. Your game state (alive / dead / spectating, health, ammo, speed, kills). A feed of recent moments, plus **Try one** (demo match, or fire any moment). |
| **Sound** | The core idea. **Calm and full energy**: every setting as a pair of sliders, where the music sits when calm and where it ends up at full energy. **Stems**: each stem's calm level and a two-handle slider for the energy range it comes back over. **What builds energy**, with live contribution bars. Output trim. |
| **Moments** | Every game event in one list (gun, fighting, you, objectives, movement & menus). Pick one to see what it does to the music (add or remove effects), its feel (swell in, fade back, strength, plus damage sensitivity / low-health threshold / speed range), whether it builds energy, the on-screen-message triggers that fire it, and a Test button. **Message triggers** (under Advanced) lists them all. |
| **Setup** | Connection checklist (Deadlock, game data, launch options with a Copy button, music routing), audio devices, the stem engine (Neural / Fast, interval, GPU priority, live status), game input, files, and diagnostics with the activity log. |

The header has **Presets** (built in, plus your own via "Save current sound as preset…"),
**Intensity** (scales every move) and an **Effects** switch (off = the dry track, for A/B). Every
edit plays immediately; a bar at the bottom offers **Save** (Ctrl+S) or **Revert** while there
are unsaved changes. Save writes `sidechain.toml`, which also hot-reloads if you edit it by hand.

## How it works

```
Deadlock HUD ── sidechain_hook.js (Panorama, ~30 Hz) ── $.Msg  [SIDECHAIN]{json}
      └──> netconsole TCP :29000  /  console.log tail  ──> engine: signals → energy → routes
Spotify ──(re-pointed to a silent sink)──> process loopback ──> EQ / filters / width ──> your output
```

- **Game state.** Panorama has no data API, and `$.AsyncWebRequest` is gone. The hook reads the
  native HUD: the ammo label, the health labels (`.currentHealthLabel` / `.totalHealthLabel`), the
  movespeed readout (it turns that on), hit-marker classes, the damage-impact `killed` / `assist`
  classes, the kill-streak counter, objective banners, announcement toasts, ability cooldowns, and
  the global `gShopOpen` / `gInCombat` flags. It prints changes to the console. It only reports
  facts; all meaning lives in the engine, so tuning never needs a VPK rebuild.
- **Audio.** Windows process loopback can't hear an app whose session is muted, so Sidechain
  moves Spotify to a device nobody listens to (the per-app output override from Windows' volume
  mixer) and captures it there by process. It then plays the processed copy where Spotify was
  playing. A cubic resampler with a PI loop absorbs the clock drift between the two devices, adding
  about 30 ms. The window shows where Spotify actually is (`Spotify → sink → Sidechain ✓`) and
  warns if it's still on the old device too.
- **Neural stems (optional).** HT-Demucs v4 runs on the GPU through ONNX Runtime + DirectML,
  using the MIT-licensed forward-only export
  [webnn/stem-separator](https://huggingface.co/webnn/stem-separator), converted to half precision
  (4x faster, matches full precision at 45–52 dB SNR). The spectrogram front and back end is
  reimplemented in Rust; `sidechain neural-check` verifies it against the full-graph export. Every
  *hop* (default 2 s) the last 7.8 s are separated (~0.11 s on an idle 4070). Each sample plays as
  the weighted blend of every separated window that covers it, weighted by how much context it had
  there, so overlapping windows average the way demucs' own overlap-add does. A separation that
  runs late doesn't switch anything: the newest samples just get less look-ahead. The music plays
  ~3.85 s behind Spotify: game reactions stay instant, but pause and skip lag by that much (a pause
  fades the queued audio out immediately). It is GPU-only; there is no fallback to the Fast stems.
  Neural mode adds a fourth stem, **Bass** (the bassline: bass guitar, 808, synth bass), which HT-Demucs
  separates anyway. Unlike the Bass EQ it leaves the kick drum alone. By default it's out at calm and
  comes back over 30–70 % energy. The Fast stems have no bass stem; their music keeps the bass.
  **GPU priority:** a busy game slows separations down (3–6x in tests) without being slowed itself.
  *Auto* (the default) stays at normal priority and raises Sidechain's GPU scheduling priority only
  while separations run so late that look-ahead would suffer. It checks every 30 s whether that's
  still needed. *High* always cuts ahead (~0.12 s per separation however busy the GPU is, with a
  short frame hitch each time); *Normal* never does. DirectML's graph fusion has to be off: fused,
  it miscomputes this model.
  Setup: put `htdemucs_fwd.onnx` + `htdemucs_fwd.onnx.data` from that repo's `onnx/` folder into
  `models/`, then run `python tools/convert_fp16.py` (needs `pip install onnx onnxconverter-common`)
  to make `models/htdemucs_fwd_fp16.onnx`, which is picked up automatically.
- **Stems (Fast).** These are real-time approximations on the CPU, not a neural model, so your GPU stays
  free for the game. An STFT (2048 / 512) splits each frame three ways. **Drums** are the
  percussive part of a causal harmonic/percussive split (a median over past frames vs a median
  across frequency, soft masks). **Vocals** are the harmonic part that's centre-panned (L/R
  coherence) inside the vocal band. **Music** is the rest. The masks sum to 1, so with all stems at
  0 dB the output is bit-exact, just ~43 ms later. Expect bleed on real songs: consonants count as
  drums, and a centred lead instrument counts as vocals.
- **Safety.** A detached guardian process waits on Sidechain. If Sidechain is killed, the
  guardian restores Spotify's output. If Spotify was closed by then, it waits for Spotify to start
  again (the override persists per app) and fixes it then.

## Commands

```
sidechain [--sim] [--demo] [--hidden] [--headless] [--quiet] [--config FILE]
sidechain selftest       prove capture + per-app routing on this machine (silent, doesn't touch Spotify)
sidechain devices        list outputs; mark default and sink
sidechain restore        point Spotify back at the default output
sidechain capture <secs> <out.wav>   record what Sidechain hears from Spotify (for offline tuning)
sidechain neural-check   verify the neural front/back end against the full-graph HT-Demucs export
sidechain neural-bench [wav] [runs]   time a GPU separation per stage, compare with the CPU result
sidechain neural-stream <in.wav> <out.wav> [drums vocals music dB] [--realtime] [--prio auto|normal|high]
                         run the streaming separator over a file (offline, or paced like the live engine)
sidechain scan <log> [--late]   replay a saved console.log on its timestamps: events, energy + stems over time
sidechain render <in.wav> <out.wav> <script> [--trace t.csv]   offline timeline render
```

Console keys (console focused): `b` bypass · `w` open window · `v` status line · `q` quit.
`--sim` adds fake gameplay keys (`f` fire, `k` kill, `x` dead, `s` speed, …).

## Status

| Piece | State |
|---|---|
| Capture from sink, per-app routing, restore, guardian (incl. hard kill) | **verified** (stand-in app) |
| Real Spotify: routing + processed playback | **verified** by you (music played through it; the pause was the switch) |
| Hook in a live match: ammo/fire, speed, hit/crit, combat, reload | **verified** from your log |
| Hook health (`.currentHealthLabel`) → damage / low HP | **fixed in v0.1.1 of the VPK, untested.** v0.1.0 used ids that no longer exist |
| Kill / assist / death / objectives / toasts in a real match | **untested** (none happened in your first session) |
| netconsole carrying `$.Msg` | **untested**; console.log tail works |
| Settings window (all tabs), demo, presets | **verified** by screenshot / offline; tray click-through **untested** |
| DSP + energy + route matrix | **verified** offline (`tools/offline_check.py`) |
| Stems: exact pass-through; separation on synthetic parts; energy arc (calm −28/−30 dB → sprint vocals in → fight all in) | **verified** (unit tests + `tools/stems_check.py`) |
| Stems on real music | Fast: bleed as expected. Neural: clean, and solos sum back to the mix (+0.2 dB) on a real clip; A/B files in `ab/` |
| Neural stems: GPU, bit-exact passthrough at unity, live load + separation timing | **verified** (offline + headless live run); not yet in a real match |
| Neural stems v2 (fp16, multi-window blend, GPU priority): exact at unity; real-time output = offline render (45–47 dB SNR) idle and under a synthetic game load (1–3 GPU stress instances): 0 unseparated samples | **verified** with `neural-stream --realtime` + `gpuload`; **not yet in a real match** |

## Troubleshooting

- **Spotify silent after a crash:** `sidechain restore`, or Windows Settings → System → Sound →
  Volume mixer → Spotify → output device → Default.
- **"also still playing on …":** Spotify didn't follow the switch. Pause and play, or restart
  Spotify.
- **Hook pill red:** the launch options are missing, or the addon isn't enabled in Mod Manager.
- **Effects too subtle / too much:** use the Intensity slider, or the Subtle / Club presets.
- **After a Deadlock patch:** re-decompile `panorama/layout/citadel_hud_top_bar.vxml_c` and
  re-apply the one `<scripts>` include (see `build_addon.ps1`).

## License

MIT (see `LICENSE`). The built-in neural model, ONNX Runtime, DirectML, fonts and Rust crates keep
their own licenses, and the addon's HUD layout is Valve's: see `THIRD_PARTY_NOTICES.md`. Not
affiliated with Valve or Spotify.
