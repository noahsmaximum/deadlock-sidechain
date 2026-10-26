//! Plain-language names for the engine's signals ("moments") and DSP parameters, so the window
//! talks about "Kill" and "Muffle" instead of `kill` and `lowpass_oct`.

use crate::config::Param;

use super::theme::icon;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Gun,
    Fight,
    You,
    Map,
    State,
    Energy,
}

impl Group {
    pub const ALL: [Group; 6] = [Group::Gun, Group::Fight, Group::You, Group::Map, Group::State, Group::Energy];
    pub fn title(self) -> &'static str {
        match self {
            Group::Gun => "Your gun",
            Group::Fight => "Fighting",
            Group::You => "You",
            Group::Map => "Objectives",
            Group::State => "Movement & menus",
            Group::Energy => "Energy",
        }
    }
}

pub struct Moment {
    pub key: &'static str,
    pub title: &'static str,
    pub about: &'static str,
    pub glyph: &'static str,
    pub group: Group,
    /// Follows a game state (on while it lasts) rather than firing once.
    pub state: bool,
}

const fn m(key: &'static str, title: &'static str, glyph: &'static str, group: Group, state: bool, about: &'static str) -> Moment {
    Moment { key, title, about, glyph, group, state }
}

pub const MOMENTS: &[Moment] = &[
    m("fire", "Shooting", icon::CROSSHAIR, Group::Gun, false, "Every shot you fire (read from your ammo counter)."),
    m("hit", "Hitting", icon::TARGET, Group::Gun, false, "The hit marker flashes on an enemy."),
    m("crit", "Headshot", icon::STAR, Group::Gun, false, "A critical / headshot hit marker."),
    m("reload", "Reloading", icon::ARROWS_CLOCKWISE, Group::Gun, false, "Your ammo refills."),
    m("damage_dealt", "Damaging a hero", icon::SWORD, Group::Fight, false, "An enemy hero takes your damage for the first time in a fight."),
    m("kill", "Kill", icon::SKULL, Group::Fight, false, "You get a kill (from your own kill count)."),
    m("assist", "Assist", icon::HAND_FIST, Group::Fight, false, "You get an assist."),
    m("ability", "Ability", icon::MAGIC_WAND, Group::Fight, false, "You cast any ability."),
    m("ultimate", "Ultimate", icon::LIGHTNING, Group::Fight, false, "You cast your ultimate."),
    m("combat", "In combat", icon::SHIELD, Group::Fight, true, "While the game says you're in combat."),
    m("damage", "Taking damage", icon::DROP, Group::You, false, "You lose health. Bigger hits hit harder."),
    m("heal", "Healing", icon::HEART, Group::You, false, "You gain health."),
    m("low_health", "Low health", icon::HEARTBEAT, Group::You, true, "While your health is below the threshold, stronger the lower it gets."),
    m("death", "Dying", icon::GHOST, Group::You, false, "The moment you die."),
    m("dead", "While dead", icon::TIMER, Group::You, true, "The whole time you're waiting to respawn."),
    m("respawn", "Respawn", icon::ARROW_COUNTER_CLOCKWISE, Group::You, false, "You come back."),
    m("objective_win", "Objective taken", icon::FLAG, Group::Map, false, "Your team destroys a guardian, walker, shrine or base objective."),
    m("objective_loss", "Objective lost", icon::WARNING_CIRCLE, Group::Map, false, "The enemy destroys one of yours."),
    m("midboss", "Mid-Boss", icon::CROWN, Group::Map, false, "The Mid-Boss falls / the Rejuvenator is taken."),
    m("patron_win", "Patron down", icon::FIRE, Group::Map, false, "The enemy Patron is destroyed."),
    m("speed", "Moving fast", icon::PERSON_SIMPLE_RUN, Group::State, true, "How fast you're moving, from walking to dashing."),
    m("shop", "Shop open", icon::SHOPPING_CART, Group::State, true, "While the shop is open."),
    m("scoreboard", "Scoreboard open", icon::LIST_BULLETS, Group::State, true, "While you hold the scoreboard."),
    m("max_energy", "Max energy", icon::SPARKLE, Group::Energy, false, "Energy crosses 90%: a lift so you can hear you got there."),
];

pub fn moment(key: &str) -> Option<&'static Moment> {
    MOMENTS.iter().find(|m| m.key == key)
}

/// Display name for any signal (custom ones get a tidied version of their key).
pub fn title(key: &str) -> String {
    if key == "energy" {
        return "Energy".into();
    }
    moment(key).map(|m| m.title.to_string()).unwrap_or_else(|| {
        let s = key.replace('_', " ");
        let mut c = s.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    })
}

pub fn glyph(key: &str) -> &'static str {
    if key == "energy" {
        return icon::GAUGE;
    }
    moment(key).map_or(icon::CIRCLE, |m| m.glyph)
}

// ---------------------------------------------------------------------------- parameters

/// The tone parameters in the order they're shown.
pub const TONE: [Param; 9] =
    [Param::GainDb, Param::Width, Param::BassDb, Param::MidDb, Param::TrebleDb, Param::LowpassOct, Param::HighpassOct, Param::Resonance, Param::Drive];

pub fn pname(p: Param) -> &'static str {
    match p {
        Param::GainDb => "Volume",
        Param::Width => "Stereo width",
        Param::BassDb => "Bass",
        Param::MidDb => "Mids",
        Param::TrebleDb => "Treble",
        Param::LowpassOct => "Muffle",
        Param::HighpassOct => "Thin out",
        Param::Resonance => "Resonance",
        Param::Drive => "Grit",
        Param::DrumsDb => "Drums",
        Param::VocalsDb => "Vocals",
        Param::MusicDb => "Music",
        Param::BassStemDb => "Bassline",
    }
}

pub fn phint(p: Param) -> &'static str {
    match p {
        Param::GainDb => "Overall level",
        Param::Width => "0% = mono, 100% = as recorded, 200% = extra wide",
        Param::BassDb => "Low shelf at 120 Hz",
        Param::MidDb => "Bell at 1 kHz",
        Param::TrebleDb => "High shelf at 6 kHz",
        Param::LowpassOct => "Low-pass filter: cuts the highs, like hearing through a wall",
        Param::HighpassOct => "High-pass filter: cuts the lows, like a phone speaker",
        Param::Resonance => "Peak at the muffle filter's cutoff (the 'underwater' sound)",
        Param::Drive => "Soft saturation",
        Param::DrumsDb => "Drums stem level",
        Param::VocalsDb => "Vocals stem level",
        Param::MusicDb => "Everything that isn't drums, vocals or the bassline",
        Param::BassStemDb => "Bassline stem level (neural stems only)",
    }
}

fn hz(f: f32) -> String {
    if f >= 1000.0 { format!("{:.1} kHz", f / 1000.0) } else { format!("{f:.0} Hz") }
}

/// A parameter's absolute value, in the units people think in.
pub fn fmt_value(p: Param, v: f32) -> String {
    match p {
        Param::LowpassOct => {
            if v > -0.02 { "off".into() } else { hz(20000.0 * 2f32.powf(v)) }
        }
        Param::HighpassOct => {
            if v < 0.02 { "off".into() } else { hz(20.0 * 2f32.powf(v)) }
        }
        Param::Width => format!("{:.0}%", v * 100.0),
        Param::Resonance | Param::Drive => format!("{:.0}%", v * 100.0),
        _ => format!("{v:+.1} dB"),
    }
}

/// How much an effect moves a parameter.
pub fn fmt_amount(p: Param, d: f32) -> String {
    match p {
        Param::LowpassOct | Param::HighpassOct => format!("{d:+.1} oct"),
        Param::Width | Param::Resonance | Param::Drive => format!("{:+.0}%", d * 100.0),
        _ => format!("{d:+.1} dB"),
    }
}
