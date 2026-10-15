# Sidechain

Deadlock gameplay drives your Spotify. With nothing going on, the track sits back: near-mono,
quieter, no bass. As more happens at once (moving fast, fighting, shooting, landing hits, kills),
**energy** builds and the track opens up to full level, wide stereo, normal bass and a little extra
air. On top of that, individual events have their own moves: shots pump the low end like a
sidechain compressor, getting hit muffles the track, dying drops it underwater, kills and
objectives hit like a drop.

It works in normal matchmaking: client-side only, with no server plugin and no memory reading.

## Install

1. **Engine + window:** `run.bat` (from cmd or Explorer; in PowerShell use `.\run.bat`). It
   rebuilds if needed and opens the settings window. Closing the window hides it to the
   **tray**. Right-click the tray icon → Quit to stop, which gives Spotify its output back.
2. **Hook addon:** `.\build_addon.ps1`, then add `dist\sidechain-hook.vpk` in **Deadlock Mod
   Manager** as a local mod. For later updates, close the game and run `.\build_addon.ps1 -Deploy`,
   which overwrites the installed copy. The addon overrides `citadel_hud_top_bar`, so disable the
   old `DEADLOCK_DEATH_HOOK` / Venator kill hook.
3. **Steam → Deadlock → Launch options:** `-netconport 29000 -condebug`.
4. Start Spotify and play. Spotify is only taken over while `deadlock.exe` runs.

To hear everything without the game, click **Play demo match** on the Live tab (or run
`run.bat --demo`): a scripted 25 s fight with walking, sprinting, shooting, a kill, an ult, a death,
a respawn and an objective.

## The settings window

| Tab | What's there |
|---|---|
| **Live** | energy meter; live EQ curve against the calm / full-energy curves; current level, width, filters; game facts (HP, ammo, speed, and the observed speed range with one-click calibration); recent events; active signal meters; test buttons for every event |
| **Energy** | the core idea: the calm-state sliders, what "full energy" adds, rise/fall time, and the weight of each signal in energy (with live contribution bars) |
| **Signals** | per-signal on/off, live meter, test, attack/release, strength, and damage scale / low-HP threshold / speed range |
| **Routes** | the full modulation matrix (signal → parameter → amount), filterable |
| **Text rules** | regex rules that map objective banners / announcement toasts to signals |
| **Audio** | music app, silent sink, output device, latency, only-while-Deadlock-runs, game input settings, config folder, reset to defaults |
| **Log** | everything the engine reports |

The top bar has **Bypass** (A/B against the dry track), **Intensity** (scales every move),
**Trim**, **Presets** (Default, Subtle, Cinematic, Club, Events only, plus your own via "Save as
preset…") and **Save / Revert**. Every edit applies live; Save writes `sidechain.toml` (which also
hot-reloads if you edit it by hand).

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
- **Safety.** A detached guardian process waits on Sidechain. If Sidechain is killed, the
  guardian restores Spotify's output. If Spotify was closed by then, it waits for Spotify to start
  again (the override persists per app) and fixes it then.

## Commands

```
sidechain [--sim] [--demo] [--hidden] [--headless] [--quiet] [--config FILE]
sidechain selftest       prove capture + per-app routing on this machine (silent, doesn't touch Spotify)
sidechain devices        list outputs; mark default and sink
sidechain restore        point Spotify back at the default output
sidechain scan <log>     what the hook reported in a saved console.log (needs -condebug)
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

## Troubleshooting

- **Spotify silent after a crash:** `sidechain restore`, or Windows Settings → System → Sound →
  Volume mixer → Spotify → output device → Default.
- **"also still playing on …":** Spotify didn't follow the switch. Pause and play, or restart
  Spotify.
- **Hook pill red:** the launch options are missing, or the addon isn't enabled in Mod Manager.
- **Effects too subtle / too much:** use the Intensity slider, or the Subtle / Club presets.
- **After a Deadlock patch:** re-decompile `panorama/layout/citadel_hud_top_bar.vxml_c` and
  re-apply the one `<scripts>` include (see `build_addon.ps1`).
