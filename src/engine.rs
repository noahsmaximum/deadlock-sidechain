//! The engine thread: game lines -> control -> DSP targets, app routing, config apply/reload,
//! console keys, and the snapshot the settings window draws.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;

use crate::app::Router;
use crate::audio::{self, Shared};
use crate::config::Config;
use crate::control::Control;
use crate::game::{self, Dedupe, InputStatus};
use crate::hub::{Cmd, Hub, Snapshot};
use crate::{STOP, key, log, sessions, status};

pub struct Opts {
    pub demo: bool,
    pub sim: bool,
    pub quiet: bool,
    pub for_secs: Option<f32>,
}

/// A scripted ~25 s "match" so every effect can be heard without the game.
pub struct Demo {
    start: Instant,
    next: usize,
    was_live: bool,
}

pub const DEMO_SECS: f32 = 25.0;

type Act = fn(&mut Control);

fn fire(c: &mut Control) {
    c.pulse("fire", 1.0);
}
fn fire_hit(c: &mut Control) {
    c.pulse("fire", 1.0);
    c.pulse("hit", 1.0);
    c.pulse("damage_dealt", 1.0);
}
fn fire_crit(c: &mut Control) {
    c.pulse("fire", 1.0);
    c.pulse("crit", 1.0);
}
fn hp(c: &mut Control, v: f32) {
    c.hook(&crate::game::HookLine { hp: Some(v), ..Default::default() });
}

const SCRIPT: &[(f32, &str, Act)] = &[
    (0.5, "standing still: calm", |c| c.spd = Some(0.0)),
    (2.0, "walking", |c| c.spd = Some(362.0)),
    (4.0, "sprinting", |c| c.spd = Some(579.0)),
    (5.5, "fight starts", |c| c.set_fact("combat", 1.0)),
    (5.6, "", fire),
    (5.75, "", fire_hit),
    (5.9, "", fire),
    (6.05, "", fire_hit),
    (6.2, "", fire),
    (6.35, "", fire_crit),
    (6.5, "", fire),
    (7.2, "taking damage", |c| hp(c, 720.0)),
    (8.0, "", |c| c.pulse("ability", 1.0)),
    (8.3, "", fire),
    (8.45, "", fire_hit),
    (8.6, "", fire),
    (8.75, "", fire_hit),
    (9.5, "KILL", |c| c.pulse("kill", 1.0)),
    (12.0, "ultimate", |c| c.pulse("ultimate", 1.0)),
    (13.0, "dash", |c| c.spd = Some(700.0)),
    (14.0, "big hit: low health", |c| hp(c, 180.0)),
    (16.0, "dead", |c| {
        c.hook(&crate::game::HookLine { dead: Some(1), hp: Some(0.0), ..Default::default() });
        c.spd = Some(0.0);
    }),
    (19.5, "respawn", |c| {
        c.hook(&crate::game::HookLine { dead: Some(0), hp: Some(1000.0), ..Default::default() });
        c.set_fact("combat", 0.0);
    }),
    (20.5, "enemy guardian destroyed", |c| c.pulse("objective_win", 1.0)),
    (21.0, "", |c| c.spd = Some(362.0)),
    (23.0, "back to calm", |c| c.spd = Some(0.0)),
];

impl Demo {
    fn new(ctl: &mut Control) -> Self {
        let was_live = ctl.live;
        ctl.live = true;
        ctl.hpm = Some(1000.0);
        ctl.hp = Some(1000.0);
        log("demo: scripted match, ~25 s");
        Demo { start: Instant::now(), next: 0, was_live }
    }

    /// Returns false when finished.
    fn step(&mut self, ctl: &mut Control) -> bool {
        let t = self.start.elapsed().as_secs_f32();
        while self.next < SCRIPT.len() && SCRIPT[self.next].0 <= t {
            let (_, label, act) = SCRIPT[self.next];
            if !label.is_empty() {
                log(format!("demo {t:4.1}s: {label}"));
            }
            act(ctl);
            self.next += 1;
        }
        if t < DEMO_SECS {
            return true;
        }
        ctl.spd = None;
        ctl.hp = None;
        ctl.hpm = None;
        ctl.set_fact("combat", 0.0);
        ctl.live = self.was_live;
        log("demo: done");
        false
    }

    pub fn elapsed(&self) -> f32 {
        self.start.elapsed().as_secs_f32()
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Build the shared state and start the audio threads. The loop itself runs in `run`.
pub fn start(cfg_path: PathBuf, cfg: &Config) -> (Arc<Hub>, Receiver<Cmd>) {
    let shared = Shared::new(cfg.audio.latency_ms);
    audio::spawn_capture(shared.clone());
    audio::spawn_render(shared.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    let hub = Arc::new(Hub {
        cfg_path,
        shared,
        snap: Mutex::new(Snapshot::default()),
        active: Mutex::new((1, cfg.clone())),
        tx,
        ui: OnceLock::new(),
        show_request: Default::default(),
    });
    (hub, rx)
}

fn make_router(cfg: &Config, sh: &Shared) -> (Option<Router>, String) {
    match Router::new(&cfg.audio.app, &cfg.audio.sink, &cfg.audio.output, sh) {
        Ok(r) => (Some(r), String::new()),
        Err(e) => {
            log(format!("routing disabled: {e:#}"));
            (None, format!("{e:#}"))
        }
    }
}

pub fn run(hub: Arc<Hub>, rx: Receiver<Cmd>, cfg: Config, opts: Opts) -> Result<()> {
    let started = Instant::now();
    wasapi::initialize_mta().ok()?;
    let sh = hub.shared.clone();
    log(format!("config: {}", hub.cfg_path.display()));

    let (mut router, mut router_err) = make_router(&cfg, &sh);
    sh.stems_enabled.store(cfg.stems.enabled, Ordering::Relaxed);

    let (tx, lines) = std::sync::mpsc::channel::<String>();
    let inputs = Arc::new(InputStatus::default());
    game::spawn_netcon(cfg.input.netcon_port, tx.clone(), inputs.clone());
    let explicit_log = (!cfg.input.console_log.is_empty()).then(|| PathBuf::from(&cfg.input.console_log));
    game::spawn_log_tail(explicit_log, tx, inputs.clone());

    let mut ctl = Control::new(cfg);
    let mut dedupe = Dedupe::default();
    let mut cfg_mtime = mtime(&hub.cfg_path);
    let mut show_status = !opts.quiet;
    if opts.sim {
        ctl.live = true;
        log("sim mode: f fire  h hit  c crit  d/D damage  k kill  a assist  x dead  s speed  t low hp  u ult  o/l objective  m midboss  p shop  b bypass  w window  q quit");
    }
    let (mut sim_speed, mut sim_lowhp) = (0usize, false);
    let mut game_up = false;
    let mut hook_ver: Option<String> = None;
    let mut hook_lines = 0u64;
    let mut last_line: Option<Instant> = None;
    let mut demo: Option<Demo> = if opts.demo { Some(Demo::new(&mut ctl)) } else { None };

    let step = Duration::from_millis(5);
    let mut last = Instant::now();
    let mut next_slow = Instant::now();
    let mut next_status = Instant::now();
    let mut next_snap = Instant::now();
    while !STOP.load(Ordering::Relaxed) {
        let now = Instant::now();
        if opts.for_secs.is_some_and(|s| started.elapsed().as_secs_f32() > s) {
            break;
        }
        let dt = (now - last).as_secs_f32();
        last = now;

        // ---- game lines
        while let Ok(line) = lines.try_recv() {
            if let Some(l) = game::parse_line(&line) {
                if dedupe.accept(&l) {
                    hook_lines += 1;
                    last_line = Some(now);
                    if let Some(v) = &l.ready {
                        hook_ver = Some(v.clone());
                    }
                    ctl.hook(&l);
                }
            }
        }

        // ---- settings window
        let mut apply: Option<(Config, bool)> = None;
        while let Ok(c) = rx.try_recv() {
            match c {
                Cmd::Test(name) => ctl.test(&name, 1.5),
                Cmd::Bypass(b) => {
                    sh.bypass.store(b, Ordering::Relaxed);
                    log(if b { "BYPASS on (dry signal)" } else { "BYPASS off" });
                }
                Cmd::Demo => demo = Some(Demo::new(&mut ctl)),
                Cmd::Apply(c) => apply = Some((c, false)),
                Cmd::Save(c) => apply = Some((c, true)),
                Cmd::Quit => STOP.store(true, Ordering::Relaxed),
            }
        }
        if let Some((c, save)) = apply {
            if save {
                match c.save(&hub.cfg_path) {
                    Ok(()) => log(format!("saved {}", hub.cfg_path.display())),
                    Err(e) => log(format!("save FAILED: {e:#}")),
                }
                cfg_mtime = mtime(&hub.cfg_path);
            }
            apply_config(&hub, &mut ctl, &mut router, &mut router_err, &sh, c);
        }

        // ---- console keys
        while let Some(k) = key() {
            match k {
                'q' => STOP.store(true, Ordering::Relaxed),
                'b' => {
                    let b = !sh.bypass.load(Ordering::Relaxed);
                    sh.bypass.store(b, Ordering::Relaxed);
                    log(if b { "BYPASS on (dry signal)" } else { "BYPASS off" });
                }
                'v' => show_status = !show_status,
                'w' => hub.request_show(),
                _ if !opts.sim => {}
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
                    sim_speed = (sim_speed + 1) % 4;
                    ctl.spd = [None, Some(362.0), Some(579.0), Some(700.0)][sim_speed];
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
                    ctl.set_fact("shop", if on { 1.0 } else { 0.0 });
                }
                _ => {}
            }
        }

        if let Some(d) = demo.as_mut() {
            if !d.step(&mut ctl) {
                demo = None;
            }
        }
        ctl.tick(dt);
        let t = ctl.targets();
        *sh.targets.lock().unwrap() = t;

        // ---- once a second: routing + config file
        if now >= next_slow {
            next_slow = now + Duration::from_secs(1);
            game_up = opts.sim || demo.is_some() || sessions::find_tree("deadlock.exe").is_some();
            if let Some(r) = router.as_mut() {
                if game_up || !ctl.cfg.audio.only_with_game {
                    r.poll(&sh);
                } else {
                    r.release(&sh);
                }
            }
            if cfg_mtime != mtime(&hub.cfg_path) {
                cfg_mtime = mtime(&hub.cfg_path);
                match Config::load(&hub.cfg_path) {
                    Ok(c) => {
                        if c != ctl.cfg {
                            log("config reloaded from disk");
                            apply_config(&hub, &mut ctl, &mut router, &mut router_err, &sh, c);
                        }
                    }
                    Err(e) => log(format!("config NOT reloaded: {e:#}")),
                }
            }
        }

        let src = match (inputs.netcon.load(Ordering::Relaxed), inputs.log.load(Ordering::Relaxed)) {
            (true, _) => "netconsole",
            (false, true) => "console.log",
            _ => "no game input",
        };
        let peak = f32::from_bits(sh.out_peak.load(Ordering::Relaxed));
        let peak_db = 20.0 * peak.max(1e-6).log10();
        let ppm = sh.ratio_ppm.load(Ordering::Relaxed) as i64 - 1_000_000;

        // ---- snapshot for the window
        if now >= next_snap {
            next_snap = now + Duration::from_millis(33);
            let (app, app_ok) = match &router {
                Some(r) => (r.state.clone(), r.ok),
                None => (format!("routing off: {router_err}"), false),
            };
            let mut s = hub.snap.lock().unwrap();
            s.signals = ctl.all_signals();
            s.targets = t;
            (s.hp, s.hpm, s.ammo, s.ammom, s.spd) = (ctl.hp, ctl.hpm, ctl.ammo, ctl.ammom, ctl.spd);
            s.spd_range = ctl.spd_range;
            s.dead = ctl.dead;
            s.live = ctl.in_match();
            s.game_running = game_up;
            s.input = src.to_string();
            s.hook = hook_ver.clone();
            s.hook_lines = hook_lines;
            s.last_line_secs = last_line.map(|t| now.duration_since(t).as_secs_f32());
            s.app = app;
            s.app_ok = app_ok;
            s.output = sh.output_name.lock().unwrap().clone();
            s.peak_db = peak_db;
            s.drift_ppm = ppm;
            s.xruns = sh.underruns.load(Ordering::Relaxed);
            s.bypass = sh.bypass.load(Ordering::Relaxed);
            s.recent = ctl.recent.iter().rev().map(|(t, n)| ((ctl.clock - t) as f32, n.clone())).collect();
            s.cfg_gen = hub.active.lock().unwrap().0;
            s.demo = demo.as_ref().map(Demo::elapsed);
            s.stems = sh.stems_enabled.load(Ordering::Relaxed);
            s.stem_levels = [0, 1, 2].map(|i| f32::from_bits(sh.stem_levels[i].load(Ordering::Relaxed)));
        }

        // ---- console status line
        if show_status && now >= next_status {
            next_status = now + Duration::from_millis(100);
            status(&format!(
                "[{src}{}] {}  out {peak_db:>5.1}dB drift {ppm:+}ppm xrun {}",
                if sh.bypass.load(Ordering::Relaxed) { " BYPASS" } else { "" },
                ctl.status_line(&t),
                sh.underruns.load(Ordering::Relaxed),
            ));
        }
        std::thread::sleep(step.saturating_sub(now.elapsed()));
    }

    log(format!(
        "stopping{}",
        if router.as_ref().is_some_and(|r| r.active()) { format!(" — giving {} its output back", ctl.cfg.audio.app) } else { String::new() }
    ));
    sh.stop.store(true, Ordering::Relaxed);
    if let Some(r) = router.as_mut() {
        r.restore();
    }
    Ok(())
}

fn apply_config(hub: &Hub, ctl: &mut Control, router: &mut Option<Router>, router_err: &mut String, sh: &Arc<Shared>, c: Config) {
    if let Err(e) = c.validate() {
        log(format!("config rejected: {e:#}"));
        return;
    }
    sh.latency_frames.store((c.audio.latency_ms.max(5.0) * audio::RATE as f32 / 1000.0) as u32, Ordering::Relaxed);
    sh.stems_enabled.store(c.stems.enabled, Ordering::Relaxed);
    let audio_changed = c.audio.app != ctl.cfg.audio.app || c.audio.sink != ctl.cfg.audio.sink || c.audio.output != ctl.cfg.audio.output;
    if c.input != ctl.cfg.input {
        log("input settings change takes effect after a restart");
    }
    if audio_changed {
        log("audio routing changed — re-routing");
        if let Some(r) = router.as_mut() {
            r.release(sh);
        }
        sh.output_id.lock().unwrap().clear();
        (*router, *router_err) = make_router(&c, sh);
    }
    ctl.set_config(c.clone());
    let mut a = hub.active.lock().unwrap();
    a.0 += 1;
    a.1 = c;
}
