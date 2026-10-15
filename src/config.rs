//! sidechain.toml — audio routing, inputs, and the signal -> parameter modulation matrix.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const DEFAULT_TOML: &str = include_str!("../sidechain.default.toml");

/// DSP parameters the routes modulate, in route units (see sidechain.default.toml).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Param {
    GainDb,
    LowpassOct,
    HighpassOct,
    Resonance,
    BassDb,
    MidDb,
    TrebleDb,
    Width,
    Drive,
    DrumsDb,
    VocalsDb,
    MusicDb,
}

pub const PARAMS: [Param; 12] = [
    Param::GainDb,
    Param::LowpassOct,
    Param::HighpassOct,
    Param::Resonance,
    Param::BassDb,
    Param::MidDb,
    Param::TrebleDb,
    Param::Width,
    Param::Drive,
    Param::DrumsDb,
    Param::VocalsDb,
    Param::MusicDb,
];

/// The stem gains (drums, vocals, music), in PARAMS order.
pub const STEM_PARAMS: [Param; 3] = [Param::DrumsDb, Param::VocalsDb, Param::MusicDb];

impl Param {
    pub fn index(self) -> usize {
        self as usize
    }
    pub fn name(self) -> &'static str {
        match self {
            Param::GainDb => "gain",
            Param::LowpassOct => "lp",
            Param::HighpassOct => "hp",
            Param::Resonance => "res",
            Param::BassDb => "bass",
            Param::MidDb => "mid",
            Param::TrebleDb => "treb",
            Param::Width => "width",
            Param::Drive => "drive",
            Param::DrumsDb => "drums",
            Param::VocalsDb => "vocals",
            Param::MusicDb => "music",
        }
    }
    pub fn is_stem(self) -> bool {
        STEM_PARAMS.contains(&self)
    }
    pub fn label(self) -> &'static str {
        match self {
            Param::GainDb => "Level (dB)",
            Param::LowpassOct => "Low-pass (oct)",
            Param::HighpassOct => "High-pass (oct)",
            Param::Resonance => "Resonance",
            Param::BassDb => "Bass (dB)",
            Param::MidDb => "Mid (dB)",
            Param::TrebleDb => "Treble (dB)",
            Param::Width => "Width",
            Param::Drive => "Drive",
            Param::DrumsDb => "Drums stem (dB)",
            Param::VocalsDb => "Vocals stem (dB)",
            Param::MusicDb => "Music stem (dB)",
        }
    }
    /// Value with no modulation.
    pub fn neutral(self) -> f32 {
        if self == Param::Width { 1.0 } else { 0.0 }
    }
    pub fn range(self) -> (f32, f32) {
        match self {
            Param::GainDb => (-30.0, 9.0),
            Param::LowpassOct => (-7.0, 0.0),
            Param::HighpassOct => (0.0, 7.0),
            Param::Resonance => (0.0, 1.0),
            Param::BassDb | Param::MidDb | Param::TrebleDb => (-18.0, 12.0),
            Param::Width => (0.0, 2.0),
            Param::Drive => (0.0, 1.0),
            Param::DrumsDb | Param::VocalsDb | Param::MusicDb => (-40.0, 6.0),
        }
    }
    /// Sensible slider range for a route amount on this param.
    pub fn amount_range(self) -> (f32, f32) {
        match self {
            Param::GainDb => (-18.0, 9.0),
            Param::LowpassOct => (-7.0, 0.0),
            Param::HighpassOct => (0.0, 7.0),
            Param::Resonance | Param::Drive => (0.0, 1.0),
            Param::BassDb | Param::MidDb | Param::TrebleDb => (-18.0, 12.0),
            Param::Width => (-1.0, 1.0),
            Param::DrumsDb | Param::VocalsDb | Param::MusicDb => (-40.0, 40.0),
        }
    }
    pub fn clamp(self, v: f32) -> f32 {
        let (lo, hi) = self.range();
        v.clamp(lo, hi)
    }
}

pub type ParamValues = [f32; PARAMS.len()];

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub audio: Audio,
    pub input: Input,
    #[serde(default)]
    pub master: Master,
    #[serde(default)]
    pub base: BTreeMap<Param, f32>,
    pub signals: BTreeMap<String, SignalCfg>,
    #[serde(default)]
    pub energy: Energy,
    pub mix: Mix,
    #[serde(default)]
    pub text: Text,
    #[serde(default)]
    pub stems: StemsCfg,
}

/// Approximate real-time stems (drums / vocals / music). Their gains are the drums_db /
/// vocals_db / music_db params, so the calm state and routes drive them like anything else.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StemsCfg {
    #[serde(default)]
    pub enabled: bool,
    /// "dsp" = real-time CPU approximation (~43 ms); "neural" = HT-Demucs on the GPU (seconds of delay).
    #[serde(default)]
    pub mode: StemMode,
    /// Neural: seconds between separations. Smaller = less delay, more GPU.
    #[serde(default = "default_hop")]
    pub neural_hop_s: f32,
    /// Neural: forward-only HT-Demucs ONNX (relative paths are from the config file's folder).
    #[serde(default = "default_model")]
    pub neural_model: String,
    /// How long a stem takes to swell in / die away, however fast energy moves.
    #[serde(default = "default_fade_in")]
    pub fade_in_ms: f32,
    #[serde(default = "default_fade_out")]
    pub fade_out_ms: f32,
}

fn default_fade_in() -> f32 {
    1200.0
}
fn default_fade_out() -> f32 {
    2500.0
}

impl Default for StemsCfg {
    fn default() -> Self {
        StemsCfg {
            enabled: false,
            mode: StemMode::Dsp,
            neural_hop_s: default_hop(),
            neural_model: default_model(),
            fade_in_ms: default_fade_in(),
            fade_out_ms: default_fade_out(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StemMode {
    #[default]
    Dsp,
    Neural,
}

fn default_hop() -> f32 {
    2.0
}
fn default_model() -> String {
    "models/htdemucs_fwd.onnx".into()
}

/// `energy` = clamp(sum of weight x signal) — how much is going on. Its envelope lives in
/// [signals.energy]; routes from it take the track from calm to full.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Energy {
    #[serde(default)]
    pub weights: BTreeMap<String, f32>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Audio {
    pub app: String,
    pub sink: String,
    #[serde(default)]
    pub output: String,
    #[serde(default = "default_latency")]
    pub latency_ms: f32,
    /// Only take the app over while deadlock.exe is running.
    #[serde(default = "yes")]
    pub only_with_game: bool,
}
fn default_latency() -> f32 {
    30.0
}
fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    #[serde(default)]
    pub netcon_port: u16,
    #[serde(default)]
    pub console_log: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Master {
    /// Scales every route amount (1 = as written).
    #[serde(default = "one")]
    pub intensity: f32,
    /// Constant output trim in dB.
    #[serde(default)]
    pub trim_db: f32,
}
impl Default for Master {
    fn default() -> Self {
        Master { intensity: 1.0, trim_db: 0.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SignalCfg {
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default)]
    pub attack_ms: f32,
    #[serde(default = "default_release")]
    pub release_ms: f32,
    /// Pulse height per event (pulses), clamped so stacked events top out at 1.
    #[serde(default = "one")]
    pub strength: f32,
    /// damage/heal: fraction of max hp x scale.
    #[serde(default = "one")]
    pub scale: f32,
    /// low_health: hp fraction where the level starts rising.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f32>,
    /// speed: readout mapped min..max -> 0..1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f32>,
}
fn default_release() -> f32 {
    300.0
}
fn one() -> f32 {
    1.0
}
fn is_true(b: &bool) -> bool {
    *b
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mix {
    pub routes: Vec<Route>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub signal: String,
    pub param: Param,
    pub amount: f32,
    /// The route only responds over this slice of the signal: below `from` it adds nothing,
    /// at `to` and above it adds the full amount (linear in between).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub from: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub to: f32,
}

impl Route {
    pub fn new(signal: &str, param: Param, amount: f32) -> Self {
        Route { signal: signal.into(), param, amount, from: 0.0, to: 1.0 }
    }
    /// Signal value 0..1 -> how much of `amount` applies.
    pub fn shape(&self, s: f32) -> f32 {
        if self.to <= self.from + 1e-4 {
            return if s >= self.from { 1.0 } else { 0.0 };
        }
        ((s - self.from) / (self.to - self.from)).clamp(0.0, 1.0)
    }
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}
fn is_one(v: &f32) -> bool {
    *v == 1.0
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    #[serde(default)]
    pub rules: Vec<RuleCfg>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuleCfg {
    pub source: String,
    #[serde(rename = "match", default)]
    pub pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub positive: Option<bool>,
    pub signal: String,
}

/// The musical half of a config — what a preset stores (devices/inputs stay per machine).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Sound {
    #[serde(default)]
    pub master: Master,
    #[serde(default)]
    pub base: BTreeMap<Param, f32>,
    pub signals: BTreeMap<String, SignalCfg>,
    #[serde(default)]
    pub energy: Energy,
    pub mix: Mix,
    #[serde(default)]
    pub text: Text,
}

impl Config {
    pub fn parse(src: &str) -> Result<Self> {
        let cfg: Config = toml::from_str(src)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        for r in &self.mix.routes {
            if !self.signals.contains_key(&r.signal) {
                bail!("route uses unknown signal {:?} (define it under [signals])", r.signal);
            }
        }
        for k in self.energy.weights.keys() {
            if !self.signals.contains_key(k) {
                bail!("energy weight for unknown signal {k:?}");
            }
        }
        for r in &self.text.rules {
            if !self.signals.contains_key(&r.signal) {
                bail!("text rule uses unknown signal {:?}", r.signal);
            }
            regex::Regex::new(&r.pattern).with_context(|| format!("bad regex {:?}", r.pattern))?;
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let src = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(src.trim_start_matches('\u{feff}')).with_context(|| format!("parse {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let body = toml::to_string_pretty(self)?;
        let text = format!(
            "# Sidechain settings — written by the settings window. Hand edits hot-reload too.\n\
             # Documented defaults: sidechain.default.toml\n\n{body}"
        );
        std::fs::write(path, text).with_context(|| format!("write {}", path.display()))
    }

    pub fn default_config() -> Self {
        Self::parse(DEFAULT_TOML).expect("built-in default config")
    }

    pub fn sound(&self) -> Sound {
        Sound {
            master: self.master.clone(),
            base: self.base.clone(),
            signals: self.signals.clone(),
            energy: self.energy.clone(),
            mix: self.mix.clone(),
            text: self.text.clone(),
        }
    }

    pub fn set_sound(&mut self, s: Sound) {
        self.master = s.master;
        self.base = s.base;
        self.signals = s.signals;
        self.energy = s.energy;
        self.mix = s.mix;
        self.text = s.text;
    }

    pub fn base_values(&self) -> ParamValues {
        let mut v = [0.0; PARAMS.len()];
        for p in PARAMS {
            v[p.index()] = *self.base.get(&p).unwrap_or(&p.neutral());
        }
        v
    }
}

/// `--config <path>`, else ./sidechain.toml, else sidechain.toml beside the exe (written from
/// defaults if missing).
pub fn resolve_path(explicit: Option<&str>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(PathBuf::from(p));
    }
    let local = PathBuf::from("sidechain.toml");
    if local.exists() {
        return Ok(std::path::absolute(&local).unwrap_or(local));
    }
    let dir = std::env::current_exe()?.parent().map(Path::to_path_buf).unwrap_or_default();
    let path = dir.join("sidechain.toml");
    if !path.exists() {
        std::fs::write(&path, DEFAULT_TOML).with_context(|| format!("write {}", path.display()))?;
        println!("wrote default config: {}", path.display());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let c = Config::default_config();
        assert!(c.mix.routes.len() > 20);
        assert!(c.audio.only_with_game);
    }

    #[test]
    fn save_roundtrips() {
        let c = Config::default_config();
        let text = toml::to_string_pretty(&c).unwrap();
        let back = Config::parse(&text).unwrap();
        assert_eq!(c, back);
    }
}
