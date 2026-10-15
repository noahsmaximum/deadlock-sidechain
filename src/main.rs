//! sidechain — EQ Spotify from Deadlock gameplay.
//!
//!   game (Panorama hook) --$.Msg--> netcon / console.log --> control (signals x routes)
//!   Spotify --(re-pointed to a silent sink)--> process loopback --> DSP --> your output

mod app;
mod audio;
mod config;
mod control;
mod diag;
mod dsp;
mod game;
mod routing;
mod sessions;
mod wav;

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, anyhow, bail};

use crate::config::Config;
use crate::control::Control;
use crate::game::{Dedupe, InputStatus};

const USAGE: &str = "\
sidechain — Deadlock gameplay -> Spotify EQ

usage:
  sidechain [--config FILE] [--sim] [--quiet]   run (Ctrl+C to stop; Spotify is put back)
  sidechain devices                             list output devices
  sidechain selftest                            check capture + routing on this machine (silent)
  sidechain restore                             point Spotify back at its normal output
  sidechain scan <console.log>                  show what the hook reported in a saved log
  sidechain render <in.wav> <out.wav> <script>  offline: run a scripted event timeline over a file

keys while running (console focused):
  b bypass   v status line on/off   q quit
  --sim adds fake gameplay: f fire  h hit  c crit  d damage  D big damage  k kill  a assist
       x dead/alive  s speed  t low health  u ult  o objective+  l objective-  m midboss  p shop";

static START: OnceLock<Instant> = OnceLock::new();
static STATUS_SHOWN: Mutex<bool> = Mutex::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
static CLEANED: AtomicBool = AtomicBool::new(false);

pub fn log(msg: impl AsRef<str>) {
    let t = START.get_or_init(Instant::now).elapsed().as_secs_f32();
    let mut shown = STATUS_SHOWN.lock().unwrap();
    let mut out = std::io::stdout().lock();
    if *shown {
        let _ = write!(out, "\r{:150}\r", "");
        *shown = false;
    }
    let _ = writeln!(out, "[{t:7.2}] {}", msg.as_ref());
}

fn status(line: &str) {
    let mut shown = STATUS_SHOWN.lock().unwrap();
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "\r{:<150.150}", line);
    let _ = out.flush();
    *shown = true;
}

unsafe extern "C" {
    fn _kbhit() -> i32;
    fn _getch() -> i32;
}

fn key() -> Option<char> {
    unsafe { (_kbhit() != 0).then(|| char::from_u32(_getch() as u32).unwrap_or('\0')) }
}

unsafe extern "system" fn on_ctrl(kind: u32) -> windows::core::BOOL {
    STOP.store(true, Ordering::SeqCst);
    if kind >= 2 {
        // console closing / logoff / shutdown: we get ~5 s; wait for the restore to finish
        let t = Instant::now();
        while !CLEANED.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(4) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    true.into()
}

fn main() {
    START.get_or_init(Instant::now);
    if let Err(e) = real_main() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str);
    let flag = |name: &str| args.iter().any(|a| a == name);
    let pos: Vec<&str> = {
        let mut v = Vec::new();
        let mut skip = false;
        for a in &args {
            if skip {
                skip = false;
            } else if a == "--config" || a == "--for" {
                skip = true;
            } else if !a.starts_with("--") {
                v.push(a.as_str());
            }
        }
        v
    };
    let load = || -> Result<(std::path::PathBuf, Config)> {
        let p = config::resolve_path(opt("--config"))?;
        let c = Config::load(&p)?;
        Ok((p, c))
    };
    match pos.first().copied() {
        Some("__tone") => diag::tone(pos[1], pos[2].parse()?, pos[3].parse()?),
        Some("__guardian") => app::guardian(pos[1].parse()?, pos[2], pos.get(3).copied().unwrap_or("")),
        Some("__route") => {
            wasapi::initialize_mta().ok()?;
            let pc = routing::PolicyConfig::new()?;
            for pid in sessions::find_tree(pos[1]).map(|t| t.1).unwrap_or_default() {
                println!("{} pid {pid}: {:?}", pos[1], pc.render_device(pid).map_err(|e| e.to_string()));
            }
            Ok(())
        }
        Some("devices") => diag::devices(&load().map(|c| c.1.audio.sink).unwrap_or_default()),
        Some("selftest") => diag::selftest(&load()?.1.audio.sink),
        Some("restore") => {
            wasapi::initialize_mta().ok()?;
            let cfg = load()?.1;
            let ok = app::restore_app(&routing::PolicyConfig::new()?, &cfg.audio.app, None);
            let app = &cfg.audio.app;
            println!("{}", if ok { format!("restored: {app} follows the default output again") } else { format!("{app} isn't running with an audio session — start playback and run again") });
            Ok(())
        }
        Some("scan") => scan(pos.get(1).copied().ok_or_else(|| anyhow!("scan <console.log>"))?, load()?.1),
        Some("render") => {
            if pos.len() < 4 {
                bail!("render <in.wav> <out.wav> <script>");
            }
            render_offline(pos[1], pos[2], pos[3], load()?.1, opt("--trace"))
        }
        Some("help") => {
            println!("{USAGE}");
            Ok(())
        }
        None => {
            let (path, cfg) = load()?;
            let for_secs = opt("--for").map(str::parse::<f32>).transpose()?;
            run(path, cfg, flag("--sim"), flag("--quiet"), for_secs)
        }
        Some(other) => bail!("unknown command {other:?}\n\n{USAGE}"),
    }
}

fn mtime(p: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn run(cfg_path: std::path::PathBuf, cfg: Config, sim: bool, quiet: bool, for_secs: Option<f32>) -> Result<()> {
    let started = Instant::now();
    wasapi::initialize_mta().ok()?;
    unsafe {
        windows::Win32::System::Console::SetConsoleCtrlHandler(Some(on_ctrl), true)?;
    }
    log(format!("config: {}", cfg_path.display()));

    let sh = audio::Shared::new(cfg.audio.latency_ms);
    let mut router = app::Router::new(&cfg.audio.app, &cfg.audio.sink, &cfg.audio.output, &sh)?;
    audio::spawn_capture(sh.clone());
    audio::spawn_render(sh.clone());

    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let inputs = Arc::new(InputStatus::default());
    game::spawn_netcon(cfg.input.netcon_port, tx.clone(), inputs.clone());
    let explicit_log = (!cfg.input.console_log.is_empty()).then(|| std::path::PathBuf::from(&cfg.input.console_log));
    game::spawn_log_tail(explicit_log, tx, inputs.clone());

    let mut ctl = Control::new(cfg);
    let mut dedupe = Dedupe::default();
    let mut cfg_mtime = mtime(&cfg_path);
    let mut show_status = !quiet;
    if sim {
        ctl.live = true;
        log("sim mode: f fire  h hit  c crit  d/D damage  k kill  a assist  x dead  s speed  t low hp  u ult  o/l objective  m midboss  p shop  b bypass  q quit");
    }
    let mut idle = false;
    let mut sim_speed = 0;
    let mut sim_lowhp = false;

    let step = Duration::from_millis(5);
    let mut last = Instant::now();
    let mut next_slow = Instant::now();
    let mut next_status = Instant::now();
    while !STOP.load(Ordering::Relaxed) {
        let now = Instant::now();
        if for_secs.is_some_and(|s| started.elapsed().as_secs_f32() > s) {
            break;
        }
        let dt = (now - last).as_secs_f32();
        last = now;

        while let Ok(line) = rx.try_recv() {
            if let Some(l) = game::parse_line(&line) {
                if dedupe.accept(&l) {
                    ctl.hook(&l);
                }
            }
        }

        while let Some(k) = key() {
            match k {
                'q' => STOP.store(true, Ordering::Relaxed),
                'b' => {
                    let b = !sh.bypass.load(Ordering::Relaxed);
                    sh.bypass.store(b, Ordering::Relaxed);
                    log(if b { "BYPASS on (dry signal)" } else { "BYPASS off" });
                }
                'v' => show_status = !show_status,
                _ if !sim => {}
                'f' => ctl.pulse("fire", 3.0),
                'h' => ctl.pulse("hit", 1.0),
                'c' => ctl.pulse("crit", 1.0),
                'd' => ctl.pulse("damage", 0.12 * ctl.cfg.signals.get("damage").map_or(1.0, |s| s.scale)),
                'D' => ctl.pulse("damage", 0.45 * ctl.cfg.signals.get("damage").map_or(1.0, |s| s.scale)),
                'k' => ctl.pulse("kill", 1.0),
                'a' => ctl.pulse("assist", 1.0),
                'u' => ctl.pulse("ultimate", 1.0),
                'o' => ctl.pulse("objective_win", 1.0),
                'l' => ctl.pulse("objective_loss", 1.0),
                'm' => ctl.pulse("midboss", 1.0),
                'x' => {
                    let d = if ctl.dead { 0 } else { 1 };
                    ctl.hook(&game::HookLine { dead: Some(d), ..Default::default() });
                }
                's' => {
                    sim_speed = (sim_speed + 1) % 3;
                    ctl.spd = [None, Some(10.0), Some(14.0)][sim_speed];
                    log(format!("sim speed: {:?}", ctl.spd));
                }
                't' => {
                    sim_lowhp = !sim_lowhp;
                    ctl.hpm = Some(1000.0);
                    ctl.hp = Some(if sim_lowhp { 120.0 } else { 1000.0 });
                    log(format!("sim low health: {sim_lowhp}"));
                }
                'p' => {
                    let on = ctl.signal("shop") < 0.5;
                    ctl.level("shop", if on { 1.0 } else { 0.0 });
                }
                _ => {}
            }
        }

        ctl.tick(dt);
        let t = ctl.targets();
        *sh.targets.lock().unwrap() = t;

        if now >= next_slow {
            next_slow = now + Duration::from_secs(1);
            let game_up = sim || !ctl.cfg.audio.only_with_game || sessions::find_tree("deadlock.exe").is_some();
            if game_up {
                router.poll(&sh);
            } else {
                router.release(&sh);
            }
            idle = !game_up;
            if cfg_mtime != mtime(&cfg_path) {
                cfg_mtime = mtime(&cfg_path);
                match Config::load(&cfg_path) {
                    Ok(c) => {
                        sh.latency_frames.store((c.audio.latency_ms.max(5.0) * audio::RATE as f32 / 1000.0) as u32, Ordering::Relaxed);
                        ctl.set_config(c);
                        log("config reloaded");
                    }
                    Err(e) => log(format!("config NOT reloaded: {e:#}")),
                }
            }
        }
        if show_status && now >= next_status {
            next_status = now + Duration::from_millis(100);
            let src = match (inputs.netcon.load(Ordering::Relaxed), inputs.log.load(Ordering::Relaxed)) {
                (true, _) => "netcon",
                (false, true) => "log",
                _ => "no game",
            };
            let peak = f32::from_bits(sh.out_peak.load(Ordering::Relaxed));
            let ppm = sh.ratio_ppm.load(Ordering::Relaxed) as i64 - 1_000_000;
            let wait = if idle {
                " (Deadlock not running: Spotify untouched)"
            } else if router.waiting_for_session() {
                " (press play in Spotify)"
            } else {
                ""
            };
            status(&format!(
                "[{src}{}{wait}] {}  out {:>5.1}dB drift {ppm:+}ppm xrun {}",
                if sh.bypass.load(Ordering::Relaxed) { " BYPASS" } else { "" },
                ctl.status_line(&t),
                20.0 * peak.max(1e-6).log10(),
                sh.underruns.load(Ordering::Relaxed),
            ));
        }
        std::thread::sleep(step.saturating_sub(now.elapsed()));
    }

    log(format!("stopping{}", if router.active() { format!(" — giving {} its output back", ctl.cfg.audio.app) } else { String::new() }));
    sh.stop.store(true, Ordering::Relaxed);
    router.restore();
    CLEANED.store(true, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}

/// Replay a saved console.log through the parser/control and print what it would have done.
fn scan(path: &str, cfg: Config) -> Result<()> {
    let text = std::fs::read(path).with_context(|| path.to_string())?;
    let text = String::from_utf8_lossy(&text);
    let mut ctl = Control::new(cfg);
    let mut dedupe = Dedupe::default();
    let (mut lines, mut kept) = (0, 0);
    let mut spd = (f32::MAX, f32::MIN);
    for line in text.lines() {
        let Some(l) = game::parse_line(line) else { continue };
        lines += 1;
        if !dedupe.accept(&l) {
            continue;
        }
        kept += 1;
        if let Some(s) = l.spd {
            spd = (spd.0.min(s), spd.1.max(s));
        }
        ctl.hook(&l);
        ctl.tick(0.033);
    }
    println!("\n{lines} hook lines ({kept} after dedupe)");
    if spd.0 <= spd.1 {
        println!("speed readout range: {:.1} .. {:.1}", spd.0, spd.1);
    }
    println!("signal pulses:");
    for (k, v) in &ctl.counts {
        println!("  {k:<16} {v}");
    }
    Ok(())
}

/// Script lines: `<seconds> <hook json>`, e.g. `1.0 {"ev":["kill"]}` or `2.5 {"dead":1}`.
/// Also accepts `<seconds> pulse <signal> [amount]`.
fn render_offline(input: &str, output: &str, script: &str, cfg: Config, trace: Option<&str>) -> Result<()> {
    let w = wav::read(input)?;
    if w.channels != 2 {
        bail!("need a stereo file");
    }
    let mut events: Vec<(f32, String)> = std::fs::read_to_string(script)?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (t, rest) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
            Ok((t.parse::<f32>().with_context(|| format!("bad time in {l:?}"))?, rest.trim().to_string()))
        })
        .collect::<Result<_>>()?;
    events.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut ctl = Control::new(cfg);
    ctl.live = true;
    ctl.log_events = false;
    let mut chain = dsp::Chain::new(w.rate as f32);
    let mut out = w.samples.clone();
    let block = (w.rate as usize / 200).max(1); // 5 ms control rate
    let mut csv = String::from("t");
    for p in config::PARAMS {
        csv += &format!(",{}", p.name());
    }
    csv += "\n";
    let mut ei = 0;
    for (bi, chunk) in out.chunks_mut(block * 2).enumerate() {
        let t = (bi * block) as f32 / w.rate as f32;
        while ei < events.len() && events[ei].0 <= t {
            let e = &events[ei].1;
            if let Some(rest) = e.strip_prefix("pulse ") {
                let mut it = rest.split_whitespace();
                let name = it.next().unwrap_or("");
                ctl.pulse(name, it.next().and_then(|a| a.parse().ok()).unwrap_or(1.0));
            } else {
                let l: game::HookLine = serde_json::from_str(e).with_context(|| format!("bad event json {e:?}"))?;
                ctl.hook(&l);
            }
            ei += 1;
        }
        ctl.tick(block as f32 / w.rate as f32);
        let tv = ctl.targets();
        chain.process(chunk, &tv);
        if bi % 10 == 0 {
            csv += &format!("{t:.3}");
            for v in tv {
                csv += &format!(",{v:.3}");
            }
            csv += "\n";
        }
    }
    wav::write(output, w.rate, 2, &out)?;
    if let Some(p) = trace {
        std::fs::write(p, csv)?;
    }
    println!("wrote {output} ({:.1}s)", out.len() as f32 / 2.0 / w.rate as f32);
    Ok(())
}
