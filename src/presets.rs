//! Presets = the musical half of a config (master, base, signals, energy, routes, text rules).
//! Built-ins are derived from the shipped defaults; user presets live in `<config dir>/presets/`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::{Config, Param, Sound};

#[derive(Clone)]
pub struct Preset {
    pub name: String,
    pub about: &'static str,
    pub sound: Sound,
    pub builtin: bool,
}

fn scale_routes(s: &mut Sound, signals: &[&str], k: f32) {
    for r in s.mix.routes.iter_mut().filter(|r| signals.contains(&r.signal.as_str())) {
        r.amount *= k;
    }
}

fn set_route(s: &mut Sound, signal: &str, param: Param, amount: f32) {
    match s.mix.routes.iter_mut().find(|r| r.signal == signal && r.param == param) {
        Some(r) => r.amount = amount,
        None => s.mix.routes.push(crate::config::Route::new(signal, param, amount)),
    }
}

fn set_base(s: &mut Sound, p: Param, v: f32) {
    s.base.insert(p, v);
}

pub fn builtin() -> Vec<Preset> {
    let def = Config::default_config().sound();
    let mut out = vec![Preset {
        name: "Default".into(),
        about: "Calm → full with energy; every event has its own move.",
        sound: def.clone(),
        builtin: true,
    }];

    // Subtle: same shape, half the depth (calm state halfway to neutral, routes x0.5).
    let mut s = def.clone();
    s.master.intensity = 0.5;
    for (p, v) in s.base.iter_mut() {
        *v = p.neutral() + (*v - p.neutral()) * 0.5;
    }
    out.push(Preset { name: "Subtle".into(), about: "Half the depth everywhere. Background-music friendly.", sound: s, builtin: true });

    // Cinematic: no per-shot pumping, bigger story beats, slower energy.
    let mut s = def.clone();
    s.mix.routes.retain(|r| !matches!(r.signal.as_str(), "fire" | "hit" | "crit" | "ability"));
    scale_routes(&mut s, &["damage", "dead", "low_health"], 1.3);
    scale_routes(&mut s, &["objective_win", "objective_loss", "midboss", "patron_win", "kill"], 1.2);
    for w in ["fire", "hit", "crit", "ability"] {
        s.energy.weights.remove(w);
    }
    s.energy.weights.insert("combat".into(), 0.45);
    if let Some(e) = s.signals.get_mut("energy") {
        e.attack_ms = 600.0;
        e.release_ms = 4500.0;
    }
    out.push(Preset { name: "Cinematic".into(), about: "Score-like: no gun pumping, big swells on story beats.", sound: s, builtin: true });

    // Club: thin when calm, huge drop into energy, hard sidechain pump on shots.
    let mut s = def.clone();
    set_base(&mut s, Param::BassDb, -13.0);
    set_base(&mut s, Param::HighpassOct, 2.5);
    set_route(&mut s, "energy", Param::BassDb, 13.0);
    set_route(&mut s, "energy", Param::HighpassOct, -2.5);
    set_route(&mut s, "fire", Param::BassDb, -10.0);
    set_route(&mut s, "fire", Param::GainDb, -4.0);
    set_route(&mut s, "kill", Param::Drive, 0.6);
    if let Some(f) = s.signals.get_mut("fire") {
        f.release_ms = 220.0;
    }
    if let Some(e) = s.signals.get_mut("energy") {
        e.attack_ms = 150.0;
        e.release_ms = 1800.0;
    }
    s.energy.weights.insert("speed".into(), 0.6);
    out.push(Preset { name: "Club".into(), about: "Filtered breakdown when calm, drops into fights, hard gun pump.", sound: s, builtin: true });

    // Events only: no calm state, music stays normal until something happens.
    let mut s = def.clone();
    s.base.clear();
    s.mix.routes.retain(|r| r.signal != "energy");
    out.push(Preset { name: "Events only".into(), about: "No calm/energy layer — the track is normal, events still react.", sound: s, builtin: true });
    out
}

pub fn dir(cfg_path: &Path) -> PathBuf {
    cfg_path.parent().map(Path::to_path_buf).unwrap_or_default().join("presets")
}

pub fn load_user(dir: &Path) -> Vec<Preset> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "toml") {
            let Some(name) = p.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
            if let Ok(sound) = std::fs::read_to_string(&p).map_err(anyhow::Error::from).and_then(|t| toml::from_str::<Sound>(&t).map_err(Into::into)) {
                out.push(Preset { name, about: "Your preset", sound, builtin: false });
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn file_name(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).collect::<String>().trim().to_string()
}

pub fn save_user(dir: &Path, name: &str, sound: &Sound) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let p = dir.join(format!("{}.toml", file_name(name)));
    std::fs::write(&p, toml::to_string_pretty(sound)?).with_context(|| p.display().to_string())
}

pub fn delete_user(dir: &Path, name: &str) -> Result<()> {
    Ok(std::fs::remove_file(dir.join(format!("{}.toml", file_name(name))))?)
}

#[cfg(test)]
mod tests {
    #[test]
    fn builtins_validate() {
        for p in super::builtin() {
            let mut c = crate::config::Config::default_config();
            c.set_sound(p.sound);
            c.validate().unwrap_or_else(|e| panic!("{}: {e}", p.name));
        }
    }
}
