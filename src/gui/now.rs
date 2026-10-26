//! Now: what the music is doing this second, and why.

use eframe::egui::{self, RichText, Stroke, Ui, vec2};

use super::catalog::{self, fmt_value, pname};
use super::theme::*;
use super::{App, Page, targets_at};
use crate::config::{Param, STEM_PARAMS};
use crate::hub::{Cmd, Snapshot};

fn sig(snap: &Snapshot, n: &str) -> f32 {
    snap.signals.iter().find(|(k, _)| k == n).map_or(0.0, |(_, v)| *v)
}

fn mood(e: f32) -> (&'static str, &'static str) {
    if e >= 0.9 {
        ("Full energy", "loud · wide · full bass · extra air")
    } else if e >= 0.55 {
        ("Heating up", "opening up, drums on their way")
    } else if e >= 0.15 {
        ("Building", "vocals coming in, the track widening")
    } else {
        ("Calm", "quiet · narrow · no bass")
    }
}

impl App {
    pub(super) fn page_now(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let energy = sig(snap, "energy");
        self.energy_hero(ui, snap, energy);
        ui.columns(2, |cols| {
            self.tone_card(&mut cols[0], snap);
            self.stems_now_card(&mut cols[1], snap);
        });
        ui.columns(2, |cols| {
            self.game_card(&mut cols[0], snap);
            self.moments_feed(&mut cols[1], snap);
        });
    }

    fn energy_hero(&mut self, ui: &mut Ui, snap: &Snapshot, energy: f32) {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(190.0);
                    eyebrow(ui, "Energy");
                    let (word, desc) = mood(energy);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{:.0}%", energy * 100.0)).font(semi(40.0)).color(energy_color(energy)));
                    });
                    ui.add_space(-6.0);
                    ui.label(RichText::new(word).font(semi(15.0)).color(TEXT));
                    ui.label(RichText::new(desc).size(12.0).color(TEXT_2));
                });
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.add_space(22.0);
                    // mark where each stem starts coming back in
                    let mut ticks: Vec<(f32, &str)> = Vec::new();
                    if self.draft.stems.enabled {
                        for p in STEM_PARAMS {
                            if p == Param::BassStemDb && !snap.stems_neural {
                                continue;
                            }
                            if let Some(r) = self.draft.mix.routes.iter().find(|r| r.signal == "energy" && r.param == p) {
                                ticks.push((r.from, p.stem_title()));
                            }
                        }
                    }
                    let w = ui.available_width();
                    energy_bar(ui, energy, vec2(w, 22.0), &ticks);
                    ui.add_space(20.0);
                    // what is feeding it right now
                    let mut parts: Vec<(String, f32)> = self
                        .draft
                        .energy
                        .weights
                        .iter()
                        .map(|(k, w)| (catalog::title(k), sig(snap, k) * w))
                        .filter(|(_, v)| v.abs() > 0.02)
                        .collect();
                    parts.sort_by(|a, b| b.1.total_cmp(&a.1));
                    ui.horizontal_wrapped(|ui| {
                        ui.label(caption(if parts.is_empty() { "Nothing is building energy right now." } else { "Building it:" }));
                        for (name, v) in parts.iter().take(6) {
                            chip(ui, &format!("{name} {:+.0}%", v * 100.0), if *v < 0.0 { BAD } else { TEXT_2 });
                        }
                    });
                });
            });
        });
    }

    fn tone_card(&mut self, ui: &mut Ui, snap: &Snapshot) {
        card(ui, |ui| {
            card_head(ui, icon::WAVEFORM, "Tone", None);
            let fs = crate::audio::RATE as f32;
            let calm = crate::dsp::response_db(&targets_at(&self.draft, 0.0), fs, &self.freqs);
            let full = crate::dsp::response_db(&targets_at(&self.draft, 1.0), fs, &self.freqs);
            let live = crate::dsp::response_db(&snap.targets, fs, &self.freqs);
            eq_plot(ui, &self.freqs, &[(calm, Stroke::new(1.5, CALM.gamma_multiply(0.7))), (full, Stroke::new(1.5, ACCENT.gamma_multiply(0.55))), (live, Stroke::new(2.5, TEXT))], 118.0);
            legend(ui, &[("now", TEXT, 2.5), ("calm", CALM, 1.5), ("full energy", ACCENT, 1.5)]);
            let t = &snap.targets;
            ui.horizontal_wrapped(|ui| {
                for p in [Param::GainDb, Param::Width, Param::BassDb, Param::LowpassOct] {
                    chip(ui, &format!("{} {}", pname(p), fmt_value(p, t[p.index()])), TEXT_2);
                }
            });
        });
    }

    fn stems_now_card(&mut self, ui: &mut Ui, snap: &Snapshot) {
        card(ui, |ui| {
            card_head(ui, icon::MUSIC_NOTES, "Stems", None);
            if !snap.stems {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(muted("Stems are off, so the whole track moves together."));
                    if ghost_button(ui, "Set up stems in Sound").clicked() {
                        self.page = Page::Sound;
                    }
                });
                return;
            }
            let n = if snap.stems_neural { STEM_PARAMS.len() } else { 3 };
            let h = 118.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let w = (ui.available_width() / n as f32).floor();
                for (i, p) in STEM_PARAMS.into_iter().take(n).enumerate() {
                    let gain = snap.targets[p.index()];
                    let lvl = snap.stem_levels[i];
                    ui.allocate_ui(vec2(w, h + 44.0), |ui| {
                        ui.vertical_centered(|ui| {
                            let on = gain > -20.0;
                            vmeter(ui, (gain + 40.0) / 40.0, (lvl + 60.0) / 60.0, vec2(30.0, h), energy_color((gain + 40.0) / 40.0))
                                .on_hover_text("Tall bar: how far this stem is turned up. Thin bar: how loud it actually is in the song right now.");
                            ui.label(RichText::new(p.stem_title()).font(semi(13.0)).color(if on { TEXT } else { TEXT_3 }));
                            ui.add_space(-6.0);
                            let g = if gain <= -39.5 { "out".to_string() } else if gain > -0.5 { "full".to_string() } else { format!("{gain:.0} dB") };
                            ui.label(RichText::new(g).size(11.5).monospace().color(TEXT_2));
                        });
                    });
                }
            });
            if snap.stems_neural && !snap.neural_ready {
                ui.label(caption("Neural model loading…"));
            }
        });
    }

    fn game_card(&mut self, ui: &mut Ui, snap: &Snapshot) {
        card(ui, |ui| {
            card_head(ui, icon::GAME_CONTROLLER, "Game", None);
            if !snap.game_running {
                ui.label(muted("Deadlock isn't running."));
                return;
            }
            if !snap.live {
                ui.label(muted("In menus. Join a match and this fills in."));
            }
            let state = if snap.spec {
                ("Dead · spectating", BAD)
            } else if snap.dead {
                ("Dead", BAD)
            } else if snap.live {
                ("Alive", GOOD)
            } else {
                ("—", TEXT_3)
            };
            egui::Grid::new("game").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                ui.label(muted("State"));
                ui.horizontal(|ui| {
                    dot(ui, state.1);
                    ui.label(state.0);
                    if let Some(k) = snap.kills {
                        ui.add_space(8.0);
                        chip(ui, &format!("{} {k:.0} kills", icon::SKULL), TEXT_2);
                    }
                });
                ui.end_row();
                ui.label(muted("Health"));
                match (snap.hp, snap.hpm) {
                    (Some(h), Some(m)) if m > 0.0 => {
                        ui.horizontal(|ui| {
                            meter(ui, h / m, vec2(170.0, 8.0), lerp_color(BAD, GOOD, h / m));
                            ui.label(RichText::new(format!("{h:.0} / {m:.0}")).monospace().size(12.0).color(TEXT_2));
                        });
                    }
                    _ => {
                        ui.label(caption("—"));
                    }
                }
                ui.end_row();
                ui.label(muted("Ammo"));
                ui.label(match (snap.ammo, snap.ammom) {
                    (Some(a), Some(m)) => format!("{a:.0} / {m:.0}"),
                    (Some(a), None) => format!("{a:.0}"),
                    _ => "—".into(),
                });
                ui.end_row();
                ui.label(muted("Speed"));
                ui.horizontal(|ui| {
                    let v = sig(snap, "speed");
                    meter(ui, v, vec2(170.0, 8.0), ACCENT);
                    ui.label(RichText::new(format!("{:.0}", snap.spd.unwrap_or(0.0))).monospace().size(12.0).color(TEXT_2));
                });
                ui.end_row();
            });
        });
    }

    fn moments_feed(&mut self, ui: &mut Ui, snap: &Snapshot) {
        card(ui, |ui| {
            card_head_with(ui, icon::LIGHTNING, "Moments", None, |ui| {
                {
                    ui.menu_button(RichText::new(format!("{}  Try one", icon::PLAY)).color(TEXT), |ui| {
                        ui.set_min_width(220.0);
                        match snap.demo {
                            Some(t) => {
                                ui.label(muted(format!("Demo match playing… {t:.0}/{:.0} s", crate::engine::DEMO_SECS)));
                            }
                            None => {
                                if ui.button(RichText::new(format!("{}  Play a 25 s demo match", icon::PLAY)).color(ACCENT)).clicked() {
                                    self.hub.send(Cmd::Demo);
                                    ui.close();
                                }
                            }
                        }
                        ui.separator();
                        if ui.button(format!("{}  Full energy", icon::GAUGE)).clicked() {
                            self.test("energy");
                        }
                        for m in catalog::MOMENTS {
                            if ui.button(format!("{}  {}", m.glyph, m.title)).clicked() {
                                self.test(m.key);
                            }
                        }
                    });
                }
            });
            if snap.recent.is_empty() {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| ui.label(muted("Nothing has happened yet.")));
                return;
            }
            // collapse repeats: "Damaging a hero ×3"
            let mut rows: Vec<(f32, &String, usize)> = Vec::new();
            for (ago, name) in &snap.recent {
                match rows.last_mut() {
                    Some(last) if last.1 == name => last.2 += 1,
                    _ => rows.push((*ago, name, 1)),
                }
            }
            for (ago, name, n) in rows.into_iter().take(7) {
                let ago = &ago;
                // "kill (streak counter)" style notes carry extra words; key on the first
                let key = name.split_whitespace().next().unwrap_or(name);
                let is_moment = catalog::moment(key).is_some();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(catalog::glyph(key)).size(15.0).color(if is_moment { ACCENT } else { TEXT_3 }));
                    ui.label(if is_moment { RichText::new(catalog::title(key)) } else { muted(name.as_str()) });
                    if n > 1 {
                        ui.label(caption(format!("×{n}")));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(caption(if *ago < 1.0 { "now".to_string() } else if *ago < 60.0 { format!("{ago:.0}s ago") } else { format!("{:.0}m ago", ago / 60.0) }));
                    });
                });
            }
        });
    }
}
