//! Game facts -> named signals (0..1 envelopes) -> DSP parameter targets via the route matrix.

use std::collections::BTreeMap;

use regex::Regex;

use crate::config::{Config, PARAMS, Param, ParamValues};
use crate::game::{HookEvent, HookLine};

/// One signal's envelope. Levels follow `input` (exponential attack/release); pulses stack onto
/// the current value and ramp up linearly over attack_ms to `peak`, then release.
#[derive(Default, Clone)]
struct Env {
    input: f32,
    peak: f32,
    value: f32,
}

struct Rule {
    source: String,
    re: Regex,
    positive: Option<bool>,
    signal: String,
}

pub struct Control {
    pub cfg: Config,
    envs: BTreeMap<String, Env>,
    rules: Vec<Rule>,
    // last hook facts
    pub hp: Option<f32>,
    pub hpm: Option<f32>,
    pub ammo: Option<f32>,
    pub ammom: Option<f32>,
    pub spd: Option<f32>,
    pub dead: bool,
    pub live: bool,
    // kill bookkeeping: impact "kill" is authoritative, kill-streak "ks" is a fallback that an
    // assist vetoes (the streak popup also pops for assists)
    t_kill: Option<f64>,
    t_assist: Option<f64>,
    pending_ks: Vec<f64>,
    pub log_events: bool,
    /// pulses fired per signal (scan summary)
    pub counts: BTreeMap<String, u32>,
    /// Level signals forced to 1 until the instant (settings-window "Test" on a level).
    holds: BTreeMap<String, f64>,
    /// Last real value of event-driven levels (shop / scoreboard / combat).
    facts: BTreeMap<String, f32>,
    /// Lowest / highest non-zero speed readout seen (for calibrating speed min/max).
    pub spd_range: Option<(f32, f32)>,
    /// Most recent discrete events, newest last (settings-window feed).
    pub recent: std::collections::VecDeque<(f64, String)>,
    /// Seconds of game time this controller has ticked (deterministic: replays use it too).
    pub clock: f64,
    /// `clock` when the hook last reported live gameplay facts (hp / ammo / speed / dead).
    last_fact: f64,
    /// `max_energy` fires when energy crosses 0.9 and re-arms once it falls below 0.6.
    max_armed: bool,
    /// Stem gains after the stem fade-in / fade-out smoothing (dB): drums, vocals, music.
    stem_db: Option<[f32; 4]>,
}

impl Control {
    pub fn new(cfg: Config) -> Self {
        let mut c = Self {
            rules: Vec::new(),
            envs: BTreeMap::new(),
            cfg,
            hp: None,
            hpm: None,
            ammo: None,
            ammom: None,
            spd: None,
            dead: false,
            live: false,
            t_kill: None,
            t_assist: None,
            pending_ks: Vec::new(),
            log_events: true,
            counts: BTreeMap::new(),
            holds: BTreeMap::new(),
            facts: BTreeMap::new(),
            spd_range: None,
            recent: std::collections::VecDeque::new(),
            clock: 0.0,
            last_fact: f64::NEG_INFINITY,
            max_armed: true,
            stem_db: None,
        };
        c.apply_config();
        c
    }

    pub fn set_config(&mut self, cfg: Config) {
        self.cfg = cfg;
        self.apply_config();
    }

    fn apply_config(&mut self) {
        self.rules = self
            .cfg
            .text
            .rules
            .iter()
            .map(|r| Rule {
                source: r.source.clone(),
                re: Regex::new(&r.pattern).expect("validated at load"),
                positive: r.positive,
                signal: r.signal.clone(),
            })
            .collect();
        for name in self.cfg.signals.keys() {
            self.envs.entry(name.clone()).or_default();
        }
    }

    fn note(&self, s: impl AsRef<str>) {
        if self.log_events {
            crate::log(s);
        }
    }

    /// Fire a pulse signal. `amount` is pre-strength (1 = one event).
    pub fn pulse(&mut self, name: &str, amount: f32) {
        let Some(sc) = self.cfg.signals.get(name) else { return };
        let add = amount * sc.strength;
        *self.counts.entry(name.to_string()).or_default() += 1;
        if !matches!(name, "fire" | "hit" | "crit" | "ability" | "heal") {
            self.recent.push_back((self.clock, name.to_string()));
            if self.recent.len() > 40 {
                self.recent.pop_front();
            }
        }
        if let Some(e) = self.envs.get_mut(name) {
            e.peak = (e.peak.max(e.value) + add).min(1.0);
        }
    }

    /// Set a level signal's input.
    pub fn level(&mut self, name: &str, v: f32) {
        if let Some(e) = self.envs.get_mut(name) {
            e.input = v.clamp(0.0, 1.0);
        }
    }

    pub fn signal(&self, name: &str) -> f32 {
        self.envs.get(name).map_or(0.0, |e| e.value)
    }

    pub fn all_signals(&self) -> Vec<(String, f32)> {
        self.envs.iter().map(|(k, e)| (k.clone(), e.value)).collect()
    }

    pub fn active_signals(&self, min: f32) -> Vec<(&str, f32)> {
        self.envs.iter().filter(|(_, e)| e.value >= min).map(|(k, e)| (k.as_str(), e.value)).collect()
    }

    pub fn hook(&mut self, l: &HookLine) {
        if l.hp.is_some() || l.ammo.is_some() || l.spd.is_some() || l.dead.is_some() || l.hpm.is_some() {
            self.last_fact = self.clock;
        }
        if let Some(v) = &l.ready {
            self.note(format!("hook ready (v{v})"));
        }
        if let Some(e) = &l.err {
            self.note(format!("hook error: {e}"));
        }
        if let Some(live) = l.live {
            let live = live != 0;
            if live != self.live {
                self.note(if live { "match: in game" } else { "match: left game / menus" });
                self.live = live;
                if !live {
                    for e in self.envs.values_mut() {
                        e.input = 0.0;
                        e.peak = 0.0;
                    }
                    self.facts.clear();
                }
            }
        }
        if let Some(m) = l.hpm {
            self.hpm = Some(m);
        }
        if let Some(hp) = l.hp {
            if let (Some(prev), Some(max)) = (self.hp, self.hpm.filter(|m| *m > 0.0)) {
                let d = (hp - prev) / max;
                if d < 0.0 && !self.dead {
                    let sc = self.cfg.signals.get("damage").map_or(1.0, |s| s.scale);
                    self.pulse("damage", -d * sc);
                } else if d > 0.0 && prev > 0.0 {
                    let sc = self.cfg.signals.get("heal").map_or(1.0, |s| s.scale);
                    self.pulse("heal", d * sc);
                }
            }
            self.hp = Some(hp);
        }
        if let Some(m) = l.ammom {
            self.ammom = Some(m);
        }
        if let Some(a) = l.ammo {
            if let Some(prev) = self.ammo {
                if a < prev {
                    self.pulse("fire", prev - a);
                } else if a > prev + 0.5 {
                    self.pulse("reload", 1.0);
                }
            }
            self.ammo = Some(a);
        }
        if let Some(s) = l.spd {
            self.spd = Some(s);
            if s > 0.5 {
                self.spd_range = Some(match self.spd_range {
                    Some((lo, hi)) => (lo.min(s), hi.max(s)),
                    None => (s, s),
                });
            }
        }
        if let Some(d) = l.dead {
            let d = d != 0;
            if d != self.dead {
                if d {
                    self.note("death");
                    self.pulse("death", 1.0);
                } else {
                    self.note("respawn");
                    self.pulse("respawn", 1.0);
                }
                self.dead = d;
            }
        }
        for (key, v) in [("shop", l.shop), ("scoreboard", l.sb), ("combat", l.combat)] {
            if let Some(v) = v {
                self.set_fact(key, v as f32);
            }
        }
        if let Some(mb) = &l.mb {
            if !mb.is_empty() {
                self.note(format!("midboss timer: {mb}"));
            }
        }
        for ev in l.events() {
            self.event(ev);
        }
    }

    fn event(&mut self, ev: HookEvent) {
        let now = self.clock;
        match ev {
            HookEvent::Simple(name) => match name.as_str() {
                "kill" => {
                    self.note("kill");
                    self.t_kill = Some(now);
                    self.pulse("kill", 1.0);
                }
                "assist" => {
                    self.note("assist");
                    self.t_assist = Some(now);
                    self.pulse("assist", 1.0);
                }
                "ks" => self.pending_ks.push(now),
                "hit" => self.pulse("hit", 1.0),
                "crit" => self.pulse("crit", 1.0),
                "dmgout" => self.pulse("damage_dealt", 1.0),
                other => self.pulse(other, 1.0),
            },
            HookEvent::Ability { slot, name } => {
                self.pulse("ability", 1.0);
                if slot == 4 {
                    self.note(format!("ultimate: {name}"));
                    self.pulse("ultimate", 1.0);
                }
            }
            HookEvent::Txt { source, main, sub, positive } => {
                let text = format!("{main} {sub}");
                let hit = self.rules.iter().find(|r| {
                    r.source == source
                        && r.re.is_match(&text)
                        && (r.positive.is_none() || positive.is_none() || r.positive == positive)
                });
                let target = hit.map(|r| r.signal.clone());
                let pol = match positive {
                    Some(true) => " (+)",
                    Some(false) => " (-)",
                    None => "",
                };
                self.note(format!("{source}: {main} / {sub}{pol} -> {}", target.as_deref().unwrap_or("(no rule)")));
                if let Some(t) = target {
                    self.pulse(&t, 1.0);
                }
            }
            HookEvent::Engine(args) => self.note(format!("engine event: {}", args.join(" "))),
        }
    }

    /// Advance envelopes by `dt` seconds and resolve the kill-streak fallback.
    pub fn tick(&mut self, dt: f32) {
        self.clock += dt as f64;
        let now = self.clock;
        let near = |t: Option<f64>, at: f64, win: f64| t.is_some_and(|t| (t - at).abs() < win);
        let mut fire_kill = 0;
        self.pending_ks.retain(|&at| {
            if now - at < 0.4 {
                return true;
            }
            if !near(self.t_kill, at, 1.5) && !near(self.t_assist, at, 2.0) {
                fire_kill += 1;
            }
            false
        });
        for _ in 0..fire_kill {
            self.note("kill (streak counter)");
            self.t_kill = Some(now);
            self.pulse("kill", 1.0);
        }

        // continuous levels
        let lh = match (self.hp, self.hpm) {
            (Some(hp), Some(m)) if m > 0.0 && !self.dead => {
                let th = self.cfg.signals.get("low_health").and_then(|s| s.threshold).unwrap_or(0.35);
                ((th - hp / m) / th).clamp(0.0, 1.0)
            }
            _ => 0.0,
        };
        self.level("low_health", lh);
        self.level("dead", if self.dead { 1.0 } else { 0.0 });
        let sp = match (self.spd, self.cfg.signals.get("speed")) {
            (Some(v), Some(sc)) if !self.dead => {
                let (lo, hi) = (sc.min.unwrap_or(7.0), sc.max.unwrap_or(13.0));
                ((v - lo) / (hi - lo).max(1e-3)).clamp(0.0, 1.0)
            }
            _ => 0.0,
        };
        self.level("speed", sp);
        // energy: how much is going on right now (previous tick's envelopes, energy itself excluded)
        let e: f32 = self
            .cfg
            .energy
            .weights
            .iter()
            .filter(|(k, _)| k.as_str() != "energy")
            .map(|(k, w)| w * self.signal(k))
            .sum();
        let on = self.in_match() || self.holds.contains_key("energy");
        self.level("energy", if on { e.clamp(0.0, 1.0) } else { 0.0 });
        let facts: Vec<(String, f32)> = self.facts.iter().map(|(k, v)| (k.clone(), *v)).collect();
        for (k, v) in facts {
            self.level(&k, v);
        }
        self.holds.retain(|_, until| *until > now);
        let held: Vec<String> = self.holds.keys().cloned().collect();
        for name in held {
            self.level(&name, 1.0);
        }

        for (name, e) in self.envs.iter_mut() {
            let Some(sc) = self.cfg.signals.get(name) else { continue };
            if e.peak > e.value {
                // pulse rising: linear, full scale in attack_ms
                let step = if sc.attack_ms <= 0.0 { 1.0 } else { dt * 1000.0 / sc.attack_ms };
                e.value = (e.value + step).min(e.peak);
                if e.value >= e.peak {
                    e.peak = 0.0;
                }
                continue;
            }
            e.peak = 0.0;
            let ms = if e.input > e.value { sc.attack_ms } else { sc.release_ms };
            e.value = if ms <= 0.0 {
                e.input
            } else {
                e.value + (e.input - e.value) * (1.0 - (-dt * 1000.0 / ms).exp())
            };
            if e.value < 1e-4 && e.input == 0.0 {
                e.value = 0.0;
            }
        }
        // stems swell in / die away over their own fade times, in dB
        let raw = self.raw_targets();
        let want = crate::config::STEM_PARAMS.map(|p| raw[p.index()]);
        let cur = self.stem_db.get_or_insert(want);
        for i in 0..want.len() {
            let ms = if want[i] > cur[i] { self.cfg.stems.fade_in_ms } else { self.cfg.stems.fade_out_ms };
            cur[i] = if ms <= 0.0 { want[i] } else { cur[i] + (want[i] - cur[i]) * (1.0 - (-dt * 1000.0 / ms).exp()) };
        }

        let energy = self.signal("energy");
        if self.max_armed && energy >= 0.9 {
            self.max_armed = false;
            self.pulse("max_energy", 1.0);
        } else if energy < 0.6 {
            self.max_armed = true;
        }
    }

    /// In a match: the hook said so, or it has been reporting gameplay facts recently. (The hook
    /// only sends `live` when it changes, so an engine started mid-match never hears it.)
    pub fn in_match(&self) -> bool {
        self.live || self.clock - self.last_fact < 15.0
    }

    pub fn set_fact(&mut self, name: &str, v: f32) {
        self.facts.insert(name.to_string(), v);
    }

    /// Settings-window "Test": pulses fire once, levels are held at 1 for `secs`.
    pub fn test(&mut self, name: &str, secs: f32) {
        const LEVELS: [&str; 7] = ["dead", "low_health", "speed", "shop", "scoreboard", "combat", "energy"];
        if LEVELS.contains(&name) {
            self.holds.insert(name.to_string(), self.clock + secs as f64);
        } else {
            let sc = self.cfg.signals.get(name).map_or(1.0, |s| s.strength.max(1e-3));
            self.pulse(name, 1.0 / sc); // full-scale regardless of per-event strength
        }
    }

    /// Targets with the stems' fades applied (what the audio thread should play).
    pub fn targets(&self) -> ParamValues {
        let mut v = self.raw_targets();
        if let Some(s) = self.stem_db {
            for (i, p) in crate::config::STEM_PARAMS.into_iter().enumerate() {
                v[p.index()] = s[i];
            }
        }
        v
    }

    /// Where the routes say every parameter should be right now (stems not yet faded).
    pub fn raw_targets(&self) -> ParamValues {
        let mut v = self.cfg.base_values();
        let k = self.cfg.master.intensity;
        for r in &self.cfg.mix.routes {
            if self.cfg.signals.get(&r.signal).is_some_and(|s| !s.enabled) {
                continue;
            }
            let s = r.shape(self.signal(&r.signal));
            let i = r.param.index();
            if r.param.is_stem() {
                // intensity is applied to stems below, as depth
                v[i] = stem_fade_db(v[i], r.amount, s);
            } else {
                v[i] += k * r.amount * s;
            }
        }
        v[Param::GainDb.index()] += self.cfg.master.trim_db;
        // stems: intensity scales how far below full they sit (0 dB stays 0 dB, never above)
        for p in crate::config::STEM_PARAMS {
            let i = p.index();
            v[i] = if v[i] < 0.0 { v[i] * k } else { v[i] };
        }
        for p in PARAMS {
            v[p.index()] = p.clamp(v[p.index()]);
        }
        v
    }

    pub fn status_line(&self, t: &ParamValues) -> String {
        let mut s = String::new();
        if let (Some(hp), Some(m)) = (self.hp, self.hpm) {
            s += &format!("hp {hp:.0}/{m:.0} ");
        }
        if let Some(a) = self.ammo {
            s += &format!("ammo {a:.0} ");
        }
        if let Some(v) = self.spd {
            s += &format!("spd {v:.1} ");
        }
        s += "| ";
        for (k, v) in self.active_signals(0.02) {
            s += &format!("{k} {v:.2} ");
        }
        s += "| ";
        for p in PARAMS {
            let x = t[p.index()];
            if (x - p.neutral()).abs() > 0.05 {
                s += &format!("{} {:+.1} ", p.name(), x - p.neutral());
            }
        }
        s
    }
}

/// Stem routes fade in even steps of perceived loudness, eased at both ends: half-way through its
/// range a stem fading up from silence sits about 10 dB down (sounds "half as loud"). A dB-linear
/// ramp would stay inaudible for most of the range and then snap in at the end.
pub fn stem_fade_db(from_db: f32, amount_db: f32, shape: f32) -> f32 {
    let s = shape.clamp(0.0, 1.0);
    let s = s * s * (3.0 - 2.0 * s); // smoothstep: no kink where the range starts / ends
    // even steps in perceived loudness (~ amplitude^0.6), not in amplitude or dB
    let s = s.powf(1.0 / 0.6);
    let a = 10f32.powf(from_db / 20.0);
    let b = 10f32.powf((from_db + amount_db) / 20.0);
    20.0 * (a + (b - a) * s).max(1e-6).log10()
}

#[cfg(test)]
mod tests {
    use super::stem_fade_db;

    #[test]
    fn stems_fade_gradually() {
        // -40 dB calm, +40 over the route range
        let at = |s: f32| stem_fade_db(-40.0, 40.0, s);
        assert!((at(0.0) + 40.0).abs() < 0.01);
        assert!(at(1.0).abs() < 0.01);
        assert!((at(0.5) + 10.0).abs() < 0.6, "midpoint {}", at(0.5)); // ~half as loud
        assert!(at(0.25) > -26.0 && at(0.25) < -14.0, "quarter {}", at(0.25)); // audible, not full
        assert!(at(0.75) > -5.0, "three quarters {}", at(0.75));
        let mut prev = f32::MIN;
        for i in 0..=20 {
            let v = at(i as f32 / 20.0);
            assert!(v >= prev);
            prev = v;
        }
    }
}
