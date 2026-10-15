# Sidechain

Deadlock gameplay drives an EQ on your Spotify. Shots duck the low end like a sidechain
compressor, getting hit muffles the track, dying drops it underwater, kills and objectives hit like
a drop, and moving fast brightens and widens it. Works in normal matchmaking: client-side only, with no
server plugin and no memory reading.

## Install

1. **Engine:** `run.bat`. It builds on first run (needs Rust) and writes `sidechain.toml`.
2. **Hook addon:** `.\build_addon.ps1` → add `dist\sidechain-hook.vpk` in **Deadlock Mod Manager**
   as a local mod and enable it. It overrides `citadel_hud_top_bar`, so disable the old
   `DEADLOCK_DEATH_HOOK` / Venator kill hook if it's enabled. Only one of them can load.
3. **Steam → Deadlock → Launch options:** `-netconport 29000`. Add `-condebug` as well if you want
   the console.log fallback.
4. Start Spotify, run `run.bat`, play. Spotify is only taken over while `deadlock.exe` runs.

`Ctrl+C` / `q` stops it, and Spotify goes back to its normal output.

## How it works

```
Deadlock HUD ── sidechain_hook.js (Panorama, polls ~30 Hz) ── $.Msg  [SIDECHAIN]{json}
      └──> netconsole TCP :29000  (or console.log tail)  ──> sidechain.exe
                                                                │ signals x routes
Spotify ──(re-pointed to a silent sink)──> process loopback ──> EQ/filters ──> your output
```

- **Game state.** Panorama has no data API and `$.AsyncWebRequest` is gone. The hook reads the
  native HUD (ammo label, health labels, the movespeed readout it turns on, hit-marker classes,
  damage-impact `killed`/`assist` classes, the kill-streak counter, objective banners, announcement
  toasts, ability cooldowns, global `gShopOpen`/`gInCombat`) and prints changes to the console.
  It reports facts only; all meaning and mapping lives in the engine, so tuning never needs a VPK rebuild.
- **Audio.** Windows process loopback can't hear an app whose session is muted (it taps
  after session volume/mute, which was measured). So sidechain moves Spotify to a device nobody
  listens to (the per-app output override that Windows' volume mixer uses), captures it there by
  process ID, and plays the processed copy on the device Spotify was using. A cubic resampler with
  a PI loop absorbs the clock drift between the two devices (~30 ms added latency).
- **Safety.** A detached guardian process waits on sidechain. If sidechain is killed, it restores
  Spotify's output. If Spotify was closed by then, it waits for Spotify to start (the override
  persists per app) and fixes it then.

## What reacts (defaults)

| Game | Music |
|---|---|
| Firing (ammo ticks) | low end −5 dB, level −2.5 dB per burst. Sprays stack into a full duck |
| Hit / crit marker | treble sparkle +1.5 / +3 dB |
| Taking damage | low-pass sweep down to ~1.8 kHz, scaled by % of max HP lost |
| Low health (<35%) | progressive low-pass, narrower stereo |
| Dead | underwater: low-pass ~600 Hz + resonance, −6 dB, near-mono |
| Respawn | bright swell |
| Movement speed | brighter + wider as you go faster |
| Kill / assist | bass +4, treble +3, +1.5 dB, a little drive (assist: lighter) |
| Ult (slot 4) | high-pass whoosh that falls back in |
| Enemy objective down / ours lost | big smile EQ + width for 6 s / mid scoop + muffle |
| Patron / Mid-Boss | the biggest lift (8–12 s) |
| Shop open | "music in the next room" |

## Tuning

Everything is in `sidechain.toml`, and it hot-reloads on save:
- `[signals]`: attack/release per signal, pulse strength, damage scale, low-health threshold,
  speed range.
- `[mix] routes`: `{ signal, param, amount }` rows. Params: `gain_db lowpass_oct highpass_oct
  resonance bass_db mid_db treble_db width drive`.
- `[text] rules`: regex on objective banners / toasts → signal.

Audition without the game: `run.bat --sim`, then press `f` fire, `h` hit, `c` crit, `d`/`D` damage,
`k` kill, `a` assist, `x` dead, `s` speed, `t` low HP, `u` ult, `o`/`l` objective, `m` midboss,
`p` shop. In any mode, `b` toggles bypass for A/B comparison. The status line shows live signals and
parameter offsets.

## Commands

```
sidechain [--sim] [--quiet] [--config FILE]   run
sidechain selftest       prove capture + per-app routing on this machine (silent, doesn't touch Spotify)
sidechain devices        list outputs; mark default and sink
sidechain restore        point Spotify back at the default output
sidechain scan <log>     what the hook reported in a saved console.log (needs -condebug)
sidechain render <in.wav> <out.wav> <script> [--trace t.csv]   offline timeline render
```

`python tools/offline_check.py <sidechain.exe> <workdir>` renders a scripted match over pink noise
and prints per-window band/width/peak deltas.

## Status

| Piece | State |
|---|---|
| Process loopback from the sink; muted-session behaviour | **verified** on this machine (selftest) |
| Per-app override moves a running process; persists per app; restore | **verified** (stand-in app) |
| Guardian restores after a hard kill / after the app relaunches | **verified** (stand-in app) |
| Capture → resample → DSP → output, level-exact, 0 xruns | **verified** live (stand-in app, −74 dBFS tone) |
| DSP + signal/route matrix | **verified** offline (`tools/offline_check.py`) |
| Drift resampler | **unit-tested** (150 ppm, jittered bursts, no underruns/discontinuities) |
| Hook addon compiles + packs | **verified** (VPK contents checked) |
| Spotify itself following the override | **untested.** Spotify follows Windows default-device changes, so it should; first run will tell |
| Hook reads in a live match (ammo, HP, speed, hits, impact classes, banners, toasts) | **untested.** Panel ids/classes come from the current game files; class edges for hit markers and banner visibility are best guesses |
| netconsole carrying `$.Msg` lines | **untested.** `-condebug` log tail is the fallback, and both can run together (deduped) |

## Troubleshooting

- **Spotify silent after a crash:** `sidechain restore`, or Windows Settings → System → Sound →
  Volume mixer → Spotify → output device → Default.
- **Status says "no game":** the launch option is missing, or the hook isn't loaded. Check
  `sidechain scan "<Deadlock>\game\citadel\console.log"` (with `-condebug`) for `hook ready`.
- **"press play in Spotify":** Spotify has no audio session until it plays once, so there's nothing to route yet.
- **No sink device:** any output you never listen to works (VB-Cable, an unused HDMI out). Set
  `[audio] sink`.
- **After a Deadlock patch:** re-decompile `panorama/layout/citadel_hud_top_bar.vxml_c` and re-apply
  the one `<scripts>` include (see `build_addon.ps1`).
