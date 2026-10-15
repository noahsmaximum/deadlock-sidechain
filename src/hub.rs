//! What the engine thread and the settings window share: a snapshot the engine refreshes ~30x/s,
//! a command channel back, and the config generation the engine is running.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

use crate::audio::Shared;
use crate::config::{Config, PARAMS, ParamValues};

pub enum Cmd {
    /// Fire a pulse / hold a level for a moment.
    Test(String),
    Bypass(bool),
    /// Play the scripted demo match through the engine.
    Demo,
    /// Run this config now (not saved).
    Apply(Config),
    /// Run it and write it to sidechain.toml.
    Save(Config),
    Quit,
}

#[derive(Clone)]
pub struct Snapshot {
    pub signals: Vec<(String, f32)>,
    pub targets: ParamValues,
    pub hp: Option<f32>,
    pub hpm: Option<f32>,
    pub ammo: Option<f32>,
    pub ammom: Option<f32>,
    pub spd: Option<f32>,
    pub spd_range: Option<(f32, f32)>,
    pub dead: bool,
    pub live: bool,
    pub game_running: bool,
    pub input: String,
    pub hook: Option<String>,
    pub hook_lines: u64,
    pub last_line_secs: Option<f32>,
    pub app: String,
    pub app_ok: bool,
    pub output: String,
    pub peak_db: f32,
    pub drift_ppm: i64,
    pub xruns: u32,
    pub bypass: bool,
    /// (seconds ago, event)
    pub recent: Vec<(f32, String)>,
    pub cfg_gen: u64,
    /// Seconds into the demo, if one is playing.
    pub demo: Option<f32>,
    pub stems: bool,
    pub stem_levels: [f32; 3],
    pub stems_neural: bool,
    pub stems_latency_ms: u32,
    pub neural_ready: bool,
    pub neural_ms: u32,
    pub neural_misses: u64,
    pub neural_coverage: u32,
    pub neural_device: String,
    pub neural_error: String,
}

impl Default for Snapshot {
    fn default() -> Self {
        let mut t = [0.0; PARAMS.len()];
        for p in PARAMS {
            t[p.index()] = p.neutral();
        }
        Snapshot {
            signals: Vec::new(),
            targets: t,
            hp: None,
            hpm: None,
            ammo: None,
            ammom: None,
            spd: None,
            spd_range: None,
            dead: false,
            live: false,
            game_running: false,
            input: "starting".into(),
            hook: None,
            hook_lines: 0,
            last_line_secs: None,
            app: "starting".into(),
            app_ok: false,
            output: String::new(),
            peak_db: -120.0,
            drift_ppm: 0,
            xruns: 0,
            bypass: false,
            recent: Vec::new(),
            cfg_gen: 0,
            demo: None,
            stems: false,
            stem_levels: [-90.0; 3],
            stems_neural: false,
            stems_latency_ms: 0,
            neural_ready: false,
            neural_ms: 0,
            neural_misses: 0,
            neural_coverage: 0,
            neural_device: String::new(),
            neural_error: String::new(),
        }
    }
}

pub struct Hub {
    pub cfg_path: PathBuf,
    pub shared: Arc<Shared>,
    pub snap: Mutex<Snapshot>,
    /// (generation, config the engine is running). Bumped on every apply / disk reload.
    pub active: Mutex<(u64, Config)>,
    pub tx: Sender<Cmd>,
    /// Lets the engine wake / show the window.
    pub ui: OnceLock<eframe::egui::Context>,
    pub show_request: AtomicBool,
}

impl Hub {
    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    pub fn request_show(&self) {
        self.show_request.store(true, Ordering::Relaxed);
        if let Some(ctx) = self.ui.get() {
            ctx.request_repaint();
        }
    }
}

// ------------------------------------------------------------------------------- log ring

static LOG: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

pub fn push_log(line: String) {
    let mut l = LOG.lock().unwrap();
    l.push_back(line);
    while l.len() > 400 {
        l.pop_front();
    }
}

pub fn log_lines() -> Vec<String> {
    LOG.lock().unwrap().iter().cloned().collect()
}
