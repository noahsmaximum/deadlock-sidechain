//! Sound: the core idea. How the music sounds calm, how it sounds at full energy, when each stem
//! comes back, and what builds energy.

use eframe::egui::{self, RichText, Ui, vec2};

use super::catalog::{self, TONE, fmt_value, phint, pname};
use super::theme::*;
use super::{App, Page};
use crate::config::{Param, Route, STEM_PARAMS, StemMode};
use crate::hub::Snapshot;

fn sig(snap: &Snapshot, n: &str) -> f32 {
    snap.signals.iter().find(|(k, _)| k == n).map_or(0.0, |(_, v)| *v)
}

impl App {
    pub(super) fn page_sound(&mut self, ui: &mut Ui, snap: &Snapshot) {
        self.calm_full_card(ui);
        self.stems_card(ui, snap);
        self.energy_sources_card(ui, snap);
        self.output_card(ui);
    }

    fn energy_route(&self, p: Param) -> Option<usize> {
        self.draft.mix.routes.iter().position(|r| r.signal == "energy" && r.param == p)
    }

    /// Full-energy value = calm + the energy route's amount (at 100% intensity).
    fn set_full_amount(&mut self, p: Param, amount: f32) {
        match self.energy_route(p) {
            Some(i) if amount.abs() < 1e-4 => {
                self.draft.mix.routes.remove(i);
            }
            Some(i) => self.draft.mix.routes[i].amount = amount,
            None if amount.abs() >= 1e-4 => self.draft.mix.routes.push(Route::new("energy", p, amount)),
            None => {}
        }
    }

    fn calm_full_card(&mut self, ui: &mut Ui) {
        card(ui, |ui| {
            card_head(ui, icon::SLIDERS_HORIZONTAL, "Calm and full energy", Some("With nothing going on the music sits at Calm. As energy builds it slides all the way to Full."));
            let col = ((ui.available_width() - 150.0) / 2.0).max(160.0);
            egui::Grid::new("calm_full").num_columns(3).spacing([18.0, 10.0]).min_col_width(120.0).show(ui, |ui| {
                ui.label("");
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::MOON_STARS).color(CALM));
                    ui.label(RichText::new("Calm").font(semi(13.0)).color(CALM));
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::FIRE).color(ACCENT));
                    ui.label(RichText::new("Full energy").font(semi(13.0)).color(ACCENT));
                });
                ui.end_row();
                for p in TONE {
                    let (lo, hi) = p.range();
                    let mut calm = *self.draft.base.get(&p).unwrap_or(&p.neutral());
                    let amount = self.energy_route(p).map_or(0.0, |i| self.draft.mix.routes[i].amount);
                    let mut full = (calm + amount).clamp(lo, hi);
                    ui.label(pname(p)).on_hover_text(phint(p));
                    ui.spacing_mut().slider_width = col - 70.0;
                    let c = ui.add(egui::Slider::new(&mut calm, lo..=hi).custom_formatter(|v, _| fmt_value(p, v as f32)).custom_parser(|_| None));
                    let f = ui.add(egui::Slider::new(&mut full, lo..=hi).custom_formatter(|v, _| fmt_value(p, v as f32)).custom_parser(|_| None));
                    ui.end_row();
                    if c.changed() {
                        // keep Full where it was: only the starting point moves
                        let keep_full = calm_full_target(amount, *self.draft.base.get(&p).unwrap_or(&p.neutral()));
                        if (calm - p.neutral()).abs() < 1e-3 {
                            self.draft.base.remove(&p);
                        } else {
                            self.draft.base.insert(p, calm);
                        }
                        self.set_full_amount(p, keep_full - calm);
                    }
                    if f.changed() {
                        self.set_full_amount(p, full - calm);
                    }
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if let Some(e) = self.draft.signals.get_mut("energy") {
                    ui.label(muted("Energy rises over"));
                    secs(ui, &mut e.attack_ms, 5.0);
                    ui.label(muted("and falls back over"));
                    secs(ui, &mut e.release_ms, 20.0);
                }
                if (self.draft.master.intensity - 1.0).abs() > 0.01 {
                    ui.label(caption(format!("· Intensity {:.0}% scales how far it moves", self.draft.master.intensity * 100.0)));
                }
            });
        });
    }

    fn stems_card(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let energy = sig(snap, "energy");
        card(ui, |ui| {
            let enabled = &mut self.draft.stems.enabled;
            card_head_with(ui, icon::MUSIC_NOTES, "Stems", Some("Strip parts of the song out when calm and bring them back as energy builds."), |ui| {
                toggle(ui, enabled).on_hover_text("Split the music into drums, vocals, bassline and the rest");
            });
            if !self.draft.stems.enabled {
                ui.label(muted("Off: the whole track moves together."));
                return;
            }
            let neural = self.draft.stems.mode == StemMode::Neural;
            egui::Grid::new("stems").num_columns(3).spacing([18.0, 12.0]).min_col_width(90.0).show(ui, |ui| {
                ui.label("");
                ui.label(caption("Level when calm"));
                ui.label(caption("Comes back as energy goes from … to …"));
                ui.end_row();
                for p in STEM_PARAMS {
                    let live = neural || p != Param::BassStemDb;
                    let name = RichText::new(p.stem_title()).font(semi(13.5)).color(if live { TEXT } else { TEXT_3 });
                    let lab = ui.label(name);
                    if !live {
                        lab.on_hover_text("Neural stems only: the Fast stems can't pull the bassline out of the music");
                    } else {
                        lab.on_hover_text(phint(p));
                    }
                    let mut calm = *self.draft.base.get(&p).unwrap_or(&0.0);
                    ui.spacing_mut().slider_width = 170.0;
                    let changed = ui
                        .add_enabled(live, egui::Slider::new(&mut calm, -40.0..=0.0).custom_formatter(|v, _| if v <= -39.5 { "out".into() } else if v >= -0.5 { "full".into() } else { format!("{v:.0} dB") }))
                        .changed();
                    let idx = self.energy_route(p);
                    let (mut from, mut to) = idx.map_or((0.0, 1.0), |i| (self.draft.mix.routes[i].from, self.draft.mix.routes[i].to));
                    let ranged = live && calm < -0.5;
                    let moved = ui
                        .horizontal(|ui| {
                            if live && !ranged {
                                ui.label(caption("Always in: lower its calm level to fade it with energy"));
                                return false;
                            }
                            let m = range_slider(ui, &mut from, &mut to, 260.0, Some(energy), ranged);
                            ui.label(RichText::new(if ranged { format!("{:.0}% – {:.0}%", from * 100.0, to * 100.0) } else if live { "always in".into() } else { "neural only".into() }).monospace().size(12.0).color(TEXT_2));
                            m
                        })
                        .inner;
                    ui.end_row();
                    if changed || moved {
                        if calm.abs() < 0.5 {
                            self.draft.base.remove(&p);
                            if let Some(i) = idx {
                                self.draft.mix.routes.remove(i);
                            }
                        } else {
                            self.draft.base.insert(p, calm);
                            let to = to.max(from + 0.01);
                            match idx {
                                Some(i) => {
                                    let r = &mut self.draft.mix.routes[i];
                                    (r.amount, r.from, r.to) = (-calm, from, to);
                                }
                                None => self.draft.mix.routes.push(Route { from, to, ..Route::new("energy", p, -calm) }),
                            }
                        }
                    }
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(muted("Stems swell in over"));
                secs(ui, &mut self.draft.stems.fade_in_ms, 10.0);
                ui.label(muted("and fade out over"));
                secs(ui, &mut self.draft.stems.fade_out_ms, 20.0);
            });
            ui.horizontal(|ui| {
                ui.label(caption(format!(
                    "Separation: {}",
                    if neural { "Neural (clean, music ~4 s behind Spotify)" } else { "Fast (instant, rough)" }
                )));
                if ui.link(RichText::new("change in Setup").size(11.5)).clicked() {
                    self.page = Page::Setup;
                }
            });
        });
    }

    fn energy_sources_card(&mut self, ui: &mut Ui, snap: &Snapshot) {
        card(ui, |ui| {
            card_head(ui, icon::GAUGE, "What builds energy", Some("Each of these adds its share while it's happening. Energy is the total, capped at 100%."));
            let mut remove: Option<String> = None;
            let keys: Vec<String> = self.draft.energy.weights.keys().cloned().collect();
            egui::Grid::new("weights").num_columns(4).spacing([16.0, 8.0]).min_col_width(40.0).show(ui, |ui| {
                for k in keys {
                    let live = sig(snap, &k);
                    let w = self.draft.energy.weights.get_mut(&k).unwrap();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(catalog::glyph(&k)).color(TEXT_2));
                        ui.label(catalog::title(&k));
                    });
                    ui.spacing_mut().slider_width = 220.0;
                    ui.add(egui::Slider::new(w, -1.0..=1.5).custom_formatter(|v, _| format!("{:+.0}%", v * 100.0)).custom_parser(|_| None))
                        .on_hover_text("How much energy this adds at its strongest. Negative pulls energy down.");
                    let contrib = live * *w;
                    meter(ui, contrib.abs(), vec2(120.0, 6.0), if contrib < 0.0 { BAD } else { energy_color(contrib) })
                        .on_hover_text(format!("Adding {:+.0}% right now", contrib * 100.0));
                    if icon_button(ui, icon::X, "Stop this from building energy").clicked() {
                        remove = Some(k.clone());
                    }
                    ui.end_row();
                }
            });
            if let Some(n) = remove {
                self.draft.energy.weights.remove(&n);
            }
            let missing: Vec<String> = self.signal_names().into_iter().filter(|n| n != "energy" && !self.draft.energy.weights.contains_key(n)).collect();
            if !missing.is_empty() {
                egui::ComboBox::from_id_salt("add_weight").selected_text(format!("{}  Add a moment", icon::PLUS)).width(200.0).show_ui(ui, |ui| {
                    for n in missing {
                        if ui.selectable_label(false, format!("{}  {}", catalog::glyph(&n), catalog::title(&n))).clicked() {
                            self.draft.energy.weights.insert(n, 0.3);
                        }
                    }
                });
            }
        });
    }

    fn output_card(&mut self, ui: &mut Ui) {
        card(ui, |ui| {
            card_head(ui, icon::SPEAKER_HIGH, "Output", None);
            ui.horizontal(|ui| {
                ui.label("Trim");
                ui.spacing_mut().slider_width = 260.0;
                ui.add(egui::Slider::new(&mut self.draft.master.trim_db, -12.0..=6.0).suffix(" dB").fixed_decimals(1));
                ui.label(caption("Overall level after everything else, to match your other audio"));
            });
        });
    }
}

/// What Full was before the calm value changed.
fn calm_full_target(amount: f32, old_calm: f32) -> f32 {
    old_calm + amount
}

/// A millisecond field shown in seconds.
fn secs(ui: &mut Ui, ms: &mut f32, max_s: f32) {
    let mut s = *ms / 1000.0;
    if ui.add(egui::DragValue::new(&mut s).range(0.0..=max_s).speed(0.02).fixed_decimals(1).suffix(" s")).changed() {
        *ms = s * 1000.0;
    }
}
