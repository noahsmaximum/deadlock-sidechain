//! sidechain.toml — audio routing, inputs, and the signal -> parameter modulation matrix.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

pub const DEFAULT_TOML: &str = include_str!("../sidechain.default.toml");

/// DSP parameters the routes modulate, in route units (see sidechain.default.toml).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
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
}

pub const PARAMS: [Param; 9] = [
    Param::GainDb,
    Param::LowpassOct,
    Param::HighpassOct,
    Param::Resonance,
    Param::BassDb,
    Param::MidDb,
    Param::TrebleDb,
    Param::Width,
    Param::Drive,
];

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
        }
    }
    /// Value with no modulation.
    pub fn neutral(self) -> f32 {
        if self == Param::Width { 1.0 } else { 0.0 }
    }
    pub fn clamp(self, v: f32) -> f32 {
        let (lo, hi) = match self {
            Param::GainDb => (-30.0, 9.0),
            Param::LowpassOct => (-7.0, 0.0),
            Param::HighpassOct => (0.0, 7.0),
            Param::Resonance => (0.0, 1.0),
            Param::BassDb | Param::MidDb | Param::TrebleDb => (-18.0, 12.0),
            Param::Width => (0.0, 2.0),
            Param::Drive => (0.0, 1.0),
        };
        v.clamp(lo, hi)
    }
}

pub type ParamValues = [f32; PARAMS.len()];

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub audio: Audio,
    pub input: Input,
    #[serde(default)]
    pub base: BTreeMap<Param, f32>,
    pub signals: BTreeMap<String, SignalCfg>,
    pub mix: Mix,
    #[serde(default)]
    pub text: Text,
}

#[derive(Clone, Debug, Deserialize)]
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
fn yes() -> bool {
    true
}
fn default_latency() -> f32 {
    30.0
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    #[serde(default)]
    pub netcon_port: u16,
    #[serde(default)]
    pub console_log: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalCfg {
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
    #[serde(default)]
    pub threshold: Option<f32>,
    /// speed: readout mapped min..max -> 0..1.
    #[serde(default)]
    pub min: Option<f32>,
    #[serde(default)]
    pub max: Option<f32>,
}
fn default_release() -> f32 {
    300.0
}
fn one() -> f32 {
    1.0
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mix {
    pub routes: Vec<Route>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub signal: String,
    pub param: Param,
    pub amount: f32,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    #[serde(default)]
    pub rules: Vec<RuleCfg>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleCfg {
    pub source: String,
    #[serde(rename = "match", default)]
    pub pattern: String,
    #[serde(default)]
    pub positive: Option<bool>,
    pub signal: String,
}

impl Config {
    pub fn parse(src: &str) -> Result<Self> {
        let cfg: Config = toml::from_str(src)?;
        for r in &cfg.mix.routes {
            if !cfg.signals.contains_key(&r.signal) {
                bail!("route uses unknown signal {:?} (define it under [signals])", r.signal);
            }
        }
        for r in &cfg.text.rules {
            if !cfg.signals.contains_key(&r.signal) {
                bail!("text rule uses unknown signal {:?}", r.signal);
            }
            regex::Regex::new(&r.pattern).with_context(|| format!("bad regex {:?}", r.pattern))?;
        }
        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let src = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(src.trim_start_matches('\u{feff}')).with_context(|| format!("parse {}", path.display()))
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
        return Ok(std::fs::canonicalize(&local).unwrap_or(local));
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
    #[test]
    fn default_config_parses() {
        let c = super::Config::parse(super::DEFAULT_TOML).expect("default config");
        assert!(c.mix.routes.len() > 20);
        assert!(c.audio.only_with_game);
    }
}
