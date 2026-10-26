//! Moments: every game event in one list; pick one to see and shape what it does to the music.
//! (Replaces the old Signals / Routes / Text rules tabs: everything about one event is in one place.)

use eframe::egui::{self, Color32, CornerRadius, RichText, Sense, Stroke, Ui, pos2, vec2};

use super::App;
use super::catalog::{self, Group, fmt_amount, phint, pname};
use super::theme::*;
use crate::config::{PARAMS, Param, Route, RuleCfg};
use crate::hub::Snapshot;

const RULES: &str = "__rules";

fn sig(snap: &Snapshot, n: &str) -> f32 {
    snap.signals.iter().find(|(k, _)| k == n).map_or(0.0, |(_, v)| *v)
}

impl App {
    pub(super) fn page_moments(&mut self, ui: &mut Ui, snap: &Snapshot) {
        if self.moment != RULES && !self.draft.signals.contains_key(&self.moment) {
            self.moment = "kill".into();
        }
        let h = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(258.0, h), egui::Layout::top_down(egui::Align::Min), |ui| {
                egui::Frame::new().fill(SURFACE).stroke(Stroke::new(1.0, BORDER)).corner_radius(CornerRadius::same(12)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                    ui.set_min_height(h - 18.0);
                    egui::ScrollArea::vertical().id_salt("moment_list").auto_shrink([false, false]).show(ui, |ui| self.moment_list(ui, snap));
                });
            });
            ui.add_space(12.0);
            ui.vertical(|ui| {
                egui::ScrollArea::vertical().id_salt("moment_detail").auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 12.0;
                    if self.moment == RULES {
                        self.rules_detail(ui);
                    } else {
                        let key = self.moment.clone();
                        self.moment_detail(ui, snap, &key);
                    }
                    ui.add_space(12.0);
                });
            });
        });
    }

    fn moment_list(&mut self, ui: &mut Ui, snap: &Snapshot) {
        ui.spacing_mut().item_spacing.y = 2.0;
        let counts = |k: &str| self.draft.mix.routes.iter().filter(|r| r.signal == k).count();
        for g in Group::ALL {
            let items: Vec<&catalog::Moment> = catalog::MOMENTS.iter().filter(|m| m.group == g && self.draft.signals.contains_key(m.key)).collect();
            if items.is_empty() {
                continue;
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                eyebrow(ui, g.title());
            });
            for m in items {
                let enabled = self.draft.signals.get(m.key).is_some_and(|s| s.enabled);
                let n = counts(m.key);
                let r = list_row(ui, m.glyph, m.title, if !enabled { "off".into() } else if n == 0 { String::new() } else { n.to_string() }, self.moment == m.key, enabled, sig(snap, m.key));
                if r.clicked() {
                    self.moment = m.key.to_string();
                }
            }
        }
        // signals added by hand that the catalogue doesn't know
        let custom: Vec<String> = self.draft.signals.keys().filter(|k| *k != "energy" && catalog::moment(k).is_none()).cloned().collect();
        if !custom.is_empty() {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                eyebrow(ui, "Custom");
            });
            for k in custom {
                let n = counts(&k);
                if list_row(ui, icon::CIRCLE, &catalog::title(&k), if n == 0 { String::new() } else { n.to_string() }, self.moment == k, true, sig(snap, &k)).clicked() {
                    self.moment = k;
                }
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            eyebrow(ui, "Advanced");
        });
        let n = self.draft.text.rules.len();
        if list_row(ui, icon::CHAT_TEXT, "Message triggers", n.to_string(), self.moment == RULES, true, 0.0).clicked() {
            self.moment = RULES.into();
        }
    }

    fn moment_detail(&mut self, ui: &mut Ui, snap: &Snapshot, key: &str) {
        let meta = catalog::moment(key);
        let state = meta.is_some_and(|m| m.state);
        let live = sig(snap, key);
        // header
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(catalog::glyph(key)).size(26.0).color(ACCENT));
                ui.label(title(&catalog::title(key), 20.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(s) = self.draft.signals.get_mut(key) {
                        toggle(ui, &mut s.enabled).on_hover_text("Off = this moment never changes the music");
                    }
                    ui.label(muted("On"));
                    ui.add_space(8.0);
                    if ghost_button(ui, format!("{}  Test", icon::PLAY)).on_hover_text(if state { "Hold this state for a moment" } else { "Fire it once" }).clicked() {
                        self.test(key);
                    }
                    meter(ui, live, vec2(70.0, 6.0), ACCENT).on_hover_text("How strongly it's happening right now");
                });
            });
            ui.add_space(-4.0);
            ui.label(muted(meta.map_or("A custom signal.", |m| m.about)));
        });

        // effects
        card(ui, |ui| {
            card_head(ui, icon::WAVEFORM, "What it does to the music", Some(if state { "Applied in proportion to how strongly it's happening." } else { "Kicks in when it happens, then fades back (see Feel)." }));
            let mut remove: Option<usize> = None;
            let idx: Vec<usize> = self.draft.mix.routes.iter().enumerate().filter(|(_, r)| r.signal == key).map(|(i, _)| i).collect();
            if idx.is_empty() {
                ui.label(muted("Nothing yet: this moment doesn't change the music."));
            } else {
            egui::Grid::new(("effects", key)).num_columns(4).spacing([14.0, 10.0]).show(ui, |ui| {
                for i in idx {
                    let r = &mut self.draft.mix.routes[i];
                    param_combo(ui, ("p", i), &mut r.param);
                    let (lo, hi) = r.param.amount_range();
                    let (lo, hi) = (lo.min(r.amount), hi.max(r.amount));
                    let p = r.param;
                    ui.spacing_mut().slider_width = 220.0;
                    ui.add(egui::Slider::new(&mut r.amount, lo..=hi).custom_formatter(move |v, _| fmt_amount(p, v as f32)).custom_parser(|_| None)).on_hover_text(phint(p));
                    if state {
                        let (mut a, mut b) = (r.from, r.to);
                        if range_slider(ui, &mut a, &mut b, 150.0, Some(live), true) {
                            (r.from, r.to) = (a, b);
                        }
                    } else {
                        ui.label("");
                    }
                    if icon_button(ui, icon::TRASH, "Remove this effect").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
            }
            if let Some(i) = remove {
                self.draft.mix.routes.remove(i);
            }
            let any = self.draft.mix.routes.iter().any(|r| r.signal == key);
            if ghost_button(ui, format!("{}  Add an effect", icon::PLUS)).clicked() {
                self.draft.mix.routes.push(Route::new(key, Param::BassDb, 3.0));
            }
            if state && any {
                ui.label(caption("The bar on each effect is the slice of the moment's strength it ramps over; the line is where it is now."));
            }
        });

        // feel
        let spd_seen = snap.spd_range;
        if let Some(s) = self.draft.signals.get_mut(key) {
            card(ui, |ui| {
                card_head(ui, icon::PULSE, "Feel", None);
                egui::Grid::new(("feel", key)).num_columns(2).spacing([18.0, 10.0]).show(ui, |ui| {
                    ui.label("Swells in over");
                    ms_field(ui, &mut s.attack_ms, 5.0);
                    ui.end_row();
                    ui.label("Fades back over");
                    ms_field(ui, &mut s.release_ms, 30.0);
                    ui.end_row();
                    if !state {
                        ui.label("Strength");
                        ui.spacing_mut().slider_width = 220.0;
                        ui.add(egui::Slider::new(&mut s.strength, 0.0..=2.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)).custom_parser(|_| None))
                            .on_hover_text("Repeats stack up to this, e.g. a burst of shots");
                        ui.end_row();
                    }
                    match key {
                        "damage" | "heal" => {
                            ui.label("Sensitivity");
                            ui.add(egui::Slider::new(&mut s.scale, 0.0..=20.0).fixed_decimals(1)).on_hover_text("How hard a given hit lands: health fraction × this");
                            ui.end_row();
                        }
                        "low_health" => {
                            let mut th = s.threshold.unwrap_or(0.35);
                            ui.label("Starts below");
                            if ui.add(egui::Slider::new(&mut th, 0.05..=1.0).custom_formatter(|v, _| format!("{:.0}% health", v * 100.0)).custom_parser(|_| None)).changed() {
                                s.threshold = Some(th);
                            }
                            ui.end_row();
                        }
                        "speed" => {
                            let (mut lo, mut hi) = (s.min.unwrap_or(100.0), s.max.unwrap_or(700.0));
                            ui.label("Counts from … to");
                            ui.horizontal(|ui| {
                                let a = ui.add(egui::DragValue::new(&mut lo).range(0.0..=2000.0).speed(2.0));
                                ui.label(muted("to"));
                                let b = ui.add(egui::DragValue::new(&mut hi).range(1.0..=2000.0).speed(2.0));
                                ui.label(caption("HUD speed (walk ≈ 360, sprint ≈ 580, dash ≈ 700)"));
                                if a.changed() || b.changed() {
                                    s.min = Some(lo);
                                    s.max = Some(hi.max(lo + 1.0));
                                }
                            });
                            ui.end_row();
                            if let Some((a, b)) = spd_seen {
                                ui.label("");
                                if ghost_button(ui, format!("Use what you've hit: {a:.0} – {b:.0}")).clicked() {
                                    s.min = Some(a);
                                    s.max = Some(b);
                                }
                                ui.end_row();
                            }
                        }
                        _ => {}
                    }
                });
            });
        }

        // energy
        card(ui, |ui| {
            card_head(ui, icon::GAUGE, "Energy", None);
            match self.draft.energy.weights.get_mut(key) {
                Some(w) => {
                    let mut drop = false;
                    ui.horizontal(|ui| {
                        ui.label("Builds energy by");
                        ui.spacing_mut().slider_width = 220.0;
                        ui.add(egui::Slider::new(w, -1.0..=1.5).custom_formatter(|v, _| format!("{:+.0}%", v * 100.0)).custom_parser(|_| None));
                        drop = icon_button(ui, icon::X, "Stop it building energy").clicked();
                    });
                    if drop {
                        self.draft.energy.weights.remove(key);
                    }
                }
                None => {
                    ui.horizontal(|ui| {
                        ui.label(muted("Doesn't build energy."));
                        if ghost_button(ui, format!("{}  Make it build energy", icon::PLUS)).clicked() {
                            self.draft.energy.weights.insert(key.to_string(), 0.3);
                        }
                    });
                }
            }
        });

        // message triggers that fire this moment
        let has_rules = self.draft.text.rules.iter().any(|r| r.signal == key);
        if has_rules || meta.is_some_and(|m| m.group == Group::Map) {
            card(ui, |ui| {
                card_head(ui, icon::CHAT_TEXT, "Triggered by on-screen messages", Some("Objective banners and announcements whose text matches."));
                let names = self.signal_names();
                self.rules_grid(ui, Some(key), &names);
                if ghost_button(ui, format!("{}  Add a trigger", icon::PLUS)).clicked() {
                    self.draft.text.rules.push(RuleCfg { source: "toast".into(), pattern: "(?i)".into(), positive: None, signal: key.to_string() });
                }
            });
        }
    }

    fn rules_detail(&mut self, ui: &mut Ui) {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::CHAT_TEXT).size(30.0).color(ACCENT));
                ui.vertical(|ui| {
                    ui.label(title("Message triggers", 20.0));
                    ui.add_space(-6.0);
                    ui.label(muted("Turn on-screen text into moments. Banners = objective messages (good or bad for you); announcements = the game's toasts. The first match wins."));
                });
            });
        });
        card(ui, |ui| {
            let names = self.signal_names();
            self.rules_grid(ui, None, &names);
            if ghost_button(ui, format!("{}  Add a trigger", icon::PLUS)).clicked() {
                self.draft.text.rules.push(RuleCfg { source: "toast".into(), pattern: "(?i)".into(), positive: None, signal: "midboss".into() });
            }
            ui.label(caption("Patterns are regular expressions; (?i) ignores case, empty matches anything. The Setup page's activity log shows every message that arrives."));
        });
    }

    /// Rules table; `only` limits it to rules firing one signal (and hides the signal column).
    fn rules_grid(&mut self, ui: &mut Ui, only: Option<&str>, names: &[String]) {
        let n = self.draft.text.rules.len();
        let mut remove: Option<usize> = None;
        let mut swap: Option<(usize, usize)> = None;
        egui::Grid::new(("rules", only)).num_columns(5).spacing([12.0, 10.0]).show(ui, |ui| {
            for (i, r) in self.draft.text.rules.iter_mut().enumerate() {
                if only.is_some_and(|k| r.signal != k) {
                    continue;
                }
                egui::ComboBox::from_id_salt(("src", i)).selected_text(if r.source == "team" { "Banner" } else { "Announcement" }).width(120.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut r.source, "team".to_string(), "Banner (objectives)");
                    ui.selectable_value(&mut r.source, "toast".to_string(), "Announcement");
                });
                let ok = regex::Regex::new(&r.pattern).is_ok();
                ui.add_sized([220.0, 26.0], egui::TextEdit::singleline(&mut r.pattern).hint_text("any text").text_color(if ok { TEXT } else { BAD }))
                    .on_hover_text(if ok { "Regular expression" } else { "Not a valid regular expression" });
                let pol = match r.positive {
                    None => "either side",
                    Some(true) => "good for you",
                    Some(false) => "bad for you",
                };
                egui::ComboBox::from_id_salt(("pol", i)).selected_text(pol).width(110.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut r.positive, None, "either side");
                    ui.selectable_value(&mut r.positive, Some(true), "good for you");
                    ui.selectable_value(&mut r.positive, Some(false), "bad for you");
                });
                if only.is_none() {
                    signal_combo(ui, ("sig", i), &mut r.signal, names);
                }
                ui.horizontal(|ui| {
                    if only.is_none() {
                        if i > 0 && icon_button(ui, icon::ARROW_UP, "Check this one earlier").clicked() {
                            swap = Some((i, i - 1));
                        }
                        if i + 1 < n && icon_button(ui, icon::ARROW_DOWN, "Check this one later").clicked() {
                            swap = Some((i, i + 1));
                        }
                    }
                    if icon_button(ui, icon::TRASH, "Remove").clicked() {
                        remove = Some(i);
                    }
                });
                ui.end_row();
            }
        });
        if let Some((a, b)) = swap {
            self.draft.text.rules.swap(a, b);
        }
        if let Some(i) = remove {
            self.draft.text.rules.remove(i);
        }
    }
}

/// One entry in the moments list: icon, name, right-aligned badge, live strength underline.
fn list_row(ui: &mut Ui, glyph: &str, name: &str, badge: String, selected: bool, enabled: bool, live: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, CornerRadius::same(8), SURFACE_2);
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(8), Color32::from_rgb(28, 30, 37));
    }
    let c = if !enabled { TEXT_3 } else if selected { TEXT } else { TEXT_2 };
    p.text(pos2(rect.left() + 12.0, rect.center().y), egui::Align2::LEFT_CENTER, glyph, egui::FontId::proportional(16.0), if selected && enabled { ACCENT } else { c });
    p.text(pos2(rect.left() + 38.0, rect.center().y), egui::Align2::LEFT_CENTER, name, if selected { semi(13.5) } else { egui::FontId::proportional(13.5) }, c);
    if !badge.is_empty() {
        p.text(pos2(rect.right() - 12.0, rect.center().y), egui::Align2::RIGHT_CENTER, badge, egui::FontId::proportional(11.5), TEXT_3);
    }
    // live strength: a small meter left of the badge
    if live > 0.01 && enabled {
        let track = egui::Rect::from_min_size(pos2(rect.right() - 66.0, rect.center().y - 2.0), vec2(36.0, 4.0));
        p.rect_filled(track, CornerRadius::same(2), SURFACE_3);
        let mut f = track;
        f.set_width(track.width() * live.clamp(0.0, 1.0));
        p.rect_filled(f, CornerRadius::same(2), ACCENT);
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

fn param_combo(ui: &mut Ui, id: impl std::hash::Hash, value: &mut Param) {
    egui::ComboBox::from_id_salt(id).selected_text(pname(*value)).width(140.0).show_ui(ui, |ui| {
        for p in PARAMS {
            ui.selectable_value(value, p, pname(p)).on_hover_text(phint(p));
        }
    });
}

fn signal_combo(ui: &mut Ui, id: impl std::hash::Hash, value: &mut String, names: &[String]) {
    egui::ComboBox::from_id_salt(id).selected_text(catalog::title(value)).width(150.0).show_ui(ui, |ui| {
        for n in names {
            ui.selectable_value(value, n.clone(), format!("{}  {}", catalog::glyph(n), catalog::title(n)));
        }
    });
}

fn ms_field(ui: &mut Ui, ms: &mut f32, max_s: f32) {
    let mut s = *ms / 1000.0;
    if ui.add(egui::DragValue::new(&mut s).range(0.0..=max_s).speed(0.01).fixed_decimals(2).suffix(" s")).changed() {
        *ms = s * 1000.0;
    }
}
