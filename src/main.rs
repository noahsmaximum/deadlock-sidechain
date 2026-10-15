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
mod engine;
mod game;
mod gui;
mod hub;
mod neural;
mod presets;
mod routing;
mod sessions;
mod stems;
mod wav;

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

use crate::config::Config;
use crate::control::Control;
use crate::game::Dedupe;

const USAGE: &str = "\
sidechain — Deadlock gameplay -> Spotify EQ

usage:
  sidechain [--config FILE] [--sim] [--quiet]   run with the settings window (closing it hides to the tray)
            [--hidden]                          start with the window hidden (tray only)
            [--demo]                            play a scripted 25 s match (hear every effect, no game needed)
            [--headless]                        no window at all, console only
  sidechain devices                             list output devices
  sidechain selftest                            check capture + routing on this machine (silent)
  sidechain restore                             point Spotify back at its normal output
  sidechain scan <console.log> [--late]         replay a saved log: events, energy + stems over time
  sidechain render <in.wav> <out.wav> <script>  offline: run a scripted event timeline over a file

keys while running (console focused):
  b bypass   w open window   v status line on/off   q quit
  --sim adds fake gameplay: f fire  h hit  c crit  d damage  D big damage  k kill  a assist
       x dead/alive  s speed  t low health  u ult  o objective+  l objective-  m midboss  p shop";

static START: OnceLock<Instant> = OnceLock::new();
static STATUS_SHOWN: Mutex<bool> = Mutex::new(false);
pub static STOP: AtomicBool = AtomicBool::new(false);
pub static CLEANED: AtomicBool = AtomicBool::new(false);

/// Console columns (so the \r status line never wraps — cmd.exe is often 120 wide).
fn console_width() -> usize {
    use windows::Win32::System::Console::{CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo, GetStdHandle, STD_OUTPUT_HANDLE};
    unsafe {
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        match GetStdHandle(STD_OUTPUT_HANDLE) {
            Ok(h) if GetConsoleScreenBufferInfo(h, &mut info).is_ok() => (info.srWindow.Right - info.srWindow.Left + 1).max(20) as usize,
            _ => 120,
        }
    }
}

pub fn log(msg: impl AsRef<str>) {
    let t = START.get_or_init(Instant::now).elapsed().as_secs_f32();
    let line = format!("[{t:7.2}] {}", msg.as_ref());
    hub::push_log(line.clone());
    let mut shown = STATUS_SHOWN.lock().unwrap();
    let mut out = std::io::stdout().lock();
    if *shown {
        let _ = write!(out, "\r{:w$}\r", "", w = console_width() - 1);
        *shown = false;
    }
    let _ = writeln!(out, "{line}");
}

pub fn status(line: &str) {
    let w = console_width() - 1;
    let line: String = line.chars().take(w).collect();
    let mut shown = STATUS_SHOWN.lock().unwrap();
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "\r{line:<w$}");
    let _ = out.flush();
    *shown = true;
}

unsafe extern "C" {
    fn _kbhit() -> i32;
    fn _getch() -> i32;
}

pub fn key() -> Option<char> {
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
        Some("capture") => diag::capture(
            pos.get(1).ok_or_else(|| anyhow!("capture <secs> <out.wav> [exe]"))?.parse()?,
            pos.get(2).copied().unwrap_or("capture.wav"),
            pos.get(3).copied().unwrap_or("Spotify.exe"),
        ),
        Some("neural-sum") => neural::check_sum(std::path::Path::new("models/htdemucs_fwd.onnx"), pos[1]),
        Some("neural-check") => neural::check(
            std::path::Path::new(pos.get(1).copied().unwrap_or("models/htdemucs_fp16weights.onnx")),
            std::path::Path::new(pos.get(2).copied().unwrap_or("models/htdemucs_fwd.onnx")),
        ),
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
        Some("scan") => scan(pos.get(1).copied().ok_or_else(|| anyhow!("scan <console.log>"))?, load()?.1, flag("--late")),
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
            let opts = engine::Opts { demo: flag("--demo"), sim: flag("--sim"), quiet: flag("--quiet"), for_secs };
            run(path, cfg, opts, flag("--headless"), flag("--hidden"))
        }
        Some(other) => bail!("unknown command {other:?}\n\n{USAGE}"),
    }
}

fn run(cfg_path: std::path::PathBuf, cfg: Config, opts: engine::Opts, headless: bool, hidden: bool) -> Result<()> {
    unsafe {
        windows::Win32::System::Console::SetConsoleCtrlHandler(Some(on_ctrl), true)?;
    }
    let (hub, rx) = engine::start(cfg_path, &cfg);
    let h = hub.clone();
    let engine = std::thread::Builder::new().name("engine".into()).spawn(move || {
        let r = engine::run(h, rx, cfg, opts);
        if let Err(e) = &r {
            log(format!("engine error: {e:#}"));
        }
        CLEANED.store(true, Ordering::SeqCst);
        // The window closes itself on CLEANED; if it's hidden and can't repaint, don't hang.
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(1500));
            std::process::exit(0);
        });
        r
    })?;
    if !headless {
        if let Err(e) = gui::run(hub.clone(), hidden) {
            log(format!("{e:#} — continuing without a window"));
            let _ = engine.join();
            return Ok(());
        }
        STOP.store(true, Ordering::SeqCst);
    }
    engine.join().map_err(|_| anyhow!("engine thread panicked"))?
}

/// `MM/DD HH:MM:SS` at the start of a console.log line -> seconds since the start of the year-ish
/// (only differences matter).
fn line_secs(line: &str) -> Option<f64> {
    let b = line.as_bytes();
    if b.len() < 14 || b[2] != b'/' || b[5] != b' ' || b[8] != b':' || b[11] != b':' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| line.get(r)?.parse::<f64>().ok();
    Some(((n(0..2)? * 31.0 + n(3..5)?) * 24.0 + n(6..8)?) * 3600.0 + n(9..11)? * 60.0 + n(12..14)?)
}

/// Replay a saved console.log through the parser/control on its real timestamps and report what
/// the engine would have done — events, energy and stems over time. `late` drops the hook's
/// `live` flag (= an engine started mid-match, the case that used to leave energy at 0).
fn scan(path: &str, cfg: Config, late: bool) -> Result<()> {
    let text = std::fs::read(path).with_context(|| path.to_string())?;
    let text = String::from_utf8_lossy(&text);
    // (seconds, line) for our lines; spread the lines of one second evenly across it
    let mut rows: Vec<(f64, game::HookLine)> = Vec::new();
    let mut dedupe = Dedupe::default();
    let mut lines_seen = 0;
    for line in text.lines() {
        let Some(mut l) = game::parse_line(line) else { continue };
        lines_seen += 1;
        if !dedupe.accept(&l) {
            continue;
        }
        if late {
            l.live = None;
        }
        if let Some(t) = line_secs(line) {
            rows.push((t, l));
        }
    }
    let mut i = 0;
    while i < rows.len() {
        let t0 = rows[i].0;
        let j = rows[i..].iter().position(|r| r.0 != t0).map_or(rows.len(), |k| i + k);
        let n = (j - i) as f64;
        for (k, r) in rows[i..j].iter_mut().enumerate() {
            r.0 = t0 + k as f64 / n;
        }
        i = j;
    }
    let Some(first) = rows.first().map(|r| r.0) else {
        println!("no [SIDECHAIN] lines in {path}");
        return Ok(());
    };

    let mut ctl = Control::new(cfg);
    ctl.log_events = false;
    let step = 1.0 / 60.0;
    let (mut t, mut ri) = (first, 0);
    let end = rows.last().map(|r| r.0).unwrap_or(first) + 5.0;
    let (mut in_match, mut e_sum, mut e_max) = (0.0f64, 0.0f64, 0.0f32);
    let (mut e_half, mut e_full, mut vox_in, mut drums_in) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut hist = [0.0f64; 10];
    // per-minute: (match secs, energy sum, energy max, vocals-in secs, drums-in secs)
    let mut minutes: std::collections::BTreeMap<i64, (f64, f64, f32, f64, f64)> = Default::default();
    while t < end {
        while ri < rows.len() && rows[ri].0 <= t {
            ctl.hook(&rows[ri].1);
            ri += 1;
        }
        ctl.tick(step as f32);
        if ctl.in_match() {
            let e = ctl.signal("energy");
            let tv = ctl.targets();
            let vox = tv[config::Param::VocalsDb.index()] > -6.0;
            let drums = tv[config::Param::DrumsDb.index()] > -6.0;
            in_match += step;
            e_sum += e as f64 * step;
            e_max = e_max.max(e);
            hist[((e * 10.0) as usize).min(9)] += step;
            if e >= 0.5 {
                e_half += step;
            }
            if e >= 0.9 {
                e_full += step;
            }
            if vox {
                vox_in += step;
            }
            if drums {
                drums_in += step;
            }
            let m = minutes.entry(((t - first) / 60.0) as i64).or_default();
            m.0 += step;
            m.1 += e as f64 * step;
            m.2 = m.2.max(e);
            if vox {
                m.3 += step;
            }
            if drums {
                m.4 += step;
            }
        }
        t += step;
    }

    println!("\n{lines_seen} hook lines, {} after dedupe, {:.0} min of log{}", rows.len(), (end - first) / 60.0, if late { "  [--late: hook's live flag ignored]" } else { "" });
    println!("in a match: {:.1} min", in_match / 60.0);
    if in_match > 0.0 {
        println!(
            "energy: mean {:.2}, max {:.2}, >=0.5 for {:.0}% of the match, >=0.9 for {:.0}%",
            e_sum / in_match,
            e_max,
            100.0 * e_half / in_match,
            100.0 * e_full / in_match
        );
        let bars: String = hist.iter().map(|h| [' ', '.', ':', '-', '=', '#'][((h / in_match) * 25.0).min(5.0) as usize]).collect();
        println!("energy histogram 0.0 [{bars}] 1.0");
        println!("stems: vocals in (> -6 dB) {:.0}% of the match, drums in {:.0}%", 100.0 * vox_in / in_match, 100.0 * drums_in / in_match);
        println!("\n min | energy avg  max | vocals in | drums in");
        for (m, (secs, es, emax, v, d)) in &minutes {
            if *secs < 5.0 {
                continue;
            }
            println!(" {m:>3} |      {:.2}  {:.2} |      {:>3.0}% |     {:>3.0}%", es / secs, emax, 100.0 * v / secs, 100.0 * d / secs);
        }
    }
    println!("\nsignal pulses:");
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
    enum Off {
        Dsp(stems::Stems),
        Neural(neural::NeuralStems),
    }
    let mut stems = if !ctl.cfg.stems.enabled {
        None
    } else if ctl.cfg.stems.mode == config::StemMode::Neural {
        if w.rate as usize != neural::SR {
            bail!("neural stems need a {} Hz file", neural::SR);
        }
        let status = std::sync::Arc::new(neural::NeuralStatus::default());
        let gpu = ctl.cfg.stems.neural_device != "cpu";
        Some(Off::Neural(neural::NeuralStems::new(std::path::Path::new(&ctl.cfg.stems.neural_model), ctl.cfg.stems.neural_hop_s, true, gpu, status)))
    } else {
        Some(Off::Dsp(stems::Stems::new(w.rate as f32)))
    };
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
        let db = config::STEM_PARAMS.map(|p| tv[p.index()]);
        match stems.as_mut() {
            Some(Off::Dsp(s)) => s.process(chunk, db),
            Some(Off::Neural(n)) => n.process(chunk, db),
            None => {}
        }
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
