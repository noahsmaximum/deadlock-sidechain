## Install

1. Download `Sidechain.exe` below and put it anywhere (for example `%LOCALAPPDATA%\Programs\Sidechain`). Windows may warn that it's unsigned: **More info → Run anyway**.
2. Start it. Open **Setup → Connection → Game data → Get the addon file**, then add the saved `sidechain-hook.vpk` in **Deadlock Mod Manager** as a local mod (or use `sidechain-hook.vpk` from this page). Disable any old `DEADLOCK_DEATH_HOOK` / Venator kill hook: both override the same HUD file.
3. **Steam → Deadlock → Properties → Launch options:** `-netconport 29000 -condebug` (Setup has a Copy button).
4. Start Spotify and play. Sidechain only takes over Spotify while Deadlock runs. Closing the window hides it to the tray; **Setup → App → Start with Windows** starts it hidden at sign-in.

Settings live in `%APPDATA%\Sidechain`. If Spotify ever stays silent after a crash: Windows Settings → System → Sound → Volume mixer → Spotify → output device → Default.

## What's new in 0.1.0

- First release: one `Sidechain.exe` with the neural stem model, DirectML and the hook addon built in
- Neural stems on the GPU (drums, vocals, music, bass) with no glitching between separations
- Fixed kills and deaths being read from the wrong player after spectating a teammate (hook v0.1.3)
- New settings window: Now, Sound, Moments and Setup

## Known issues

- Kill counting from the scoreboard and the neural stems are not yet confirmed in a real match
- The music plays about 4 s behind Spotify in Neural mode, so pause and skip lag by that much
