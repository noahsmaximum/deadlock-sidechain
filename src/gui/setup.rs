//! Setup: is everything connected, which devices, the stem engine, diagnostics, files.

use eframe::egui::{self, Color32, RichText, Ui, vec2};

use super::theme::*;
use super::{App, refresh_devices};
use crate::config::{Config, GpuPriority, StemMode};
use crate::hub::{Snapshot, log_lines};

/// One checklist row: status icon, name, detail, optional extra widgets.
fn check_row(ui: &mut Ui, state: Option<bool>, name: &str, detail: &str, extra: impl FnOnce(&mut Ui)) {
    let (glyph, color) = match state {
        Some(true) => (icon::CHECK_CIRCLE, GOOD),
        Some(false) => (icon::WARNING_CIRCLE, WARN),
        None => (icon::CIRCLE, TEXT_3),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(glyph).size(18.0).color(color));
        ui.vertical(|ui| {
            ui.label(RichText::new(name).font(semi(13.5)));
            ui.add_space(-6.0);
            ui.label(RichText::new(detail).size(12.0).color(TEXT_2));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), extra);
    });
}

impl App {
    pub(super) fn page_setup(&mut self, ui: &mut Ui, snap: &Snapshot) {
        self.connection_card(ui, snap);
        self.audio_card(ui);
        self.engine_card(ui, snap);
        ui.columns(2, |cols| {
            self.input_card(&mut cols[0], snap);
            self.files_card(&mut cols[1]);
        });
        self.diagnostics_card(ui, snap);
    }

    fn connection_card(&mut self, ui: &mut Ui, snap: &Snapshot) {
        card(ui, |ui| {
            card_head(ui, icon::PLUGS_CONNECTED, "Connection", None);
            check_row(ui, Some(snap.game_running).filter(|r| *r).or(None), "Deadlock", if snap.game_running { "Running" } else { "Not running. Sidechain waits for it." }, |_| {});
            ui.separator();
            let live = snap.last_line_secs.is_some_and(|s| s < 10.0);
            let hook = match (&snap.hook, snap.last_line_secs) {
                (_, Some(s)) if s < 10.0 => format!("Reporting{} via {}", snap.hook.as_deref().map(|v| format!(" (v{v})")).unwrap_or_default(), snap.input),
                (_, Some(_)) => format!("Quiet for now ({} lines so far)", snap.hook_lines),
                _ if snap.game_running => "Nothing from the game yet: add dist\\sidechain-hook.vpk in Deadlock Mod Manager".into(),
                _ => "Starts reporting once you're in Deadlock".into(),
            };
            check_row(ui, if snap.game_running { Some(live) } else { None }, "Game data (hook addon)", &hook, |_| {});
            ui.separator();
            let opts = format!("-netconport {} -condebug", if self.draft.input.netcon_port > 0 { self.draft.input.netcon_port } else { 29000 });
            check_row(ui, None, "Launch options", &format!("Steam → Deadlock → Properties → Launch options:  {opts}"), |ui| {
                if ghost_button(ui, format!("{}  Copy", icon::COPY)).clicked() {
                    ui.ctx().copy_text(opts.clone());
                }
            });
            ui.separator();
            check_row(ui, Some(snap.app_ok), "Music", &snap.app, |_| {});
        });
    }

    fn audio_card(&mut self, ui: &mut Ui) {
        let devices = self.devices.lock().unwrap().clone();
        card(ui, |ui| {
            let devs = self.devices.clone();
            card_head_with(ui, icon::SPEAKER_HIGH, "Audio", Some("Sidechain moves the music app to a device nobody listens to, processes it, and plays it where you hear it."), |ui| {
                if icon_button(ui, icon::ARROWS_CLOCKWISE, "Refresh the device list").clicked() {
                    refresh_devices(devs);
                }
            });
            egui::Grid::new("audio").num_columns(2).spacing([18.0, 10.0]).show(ui, |ui| {
                ui.label("Music app");
                ui.add(egui::TextEdit::singleline(&mut self.draft.audio.app).desired_width(240.0)).on_hover_text("Process name, e.g. Spotify.exe");
                ui.end_row();
                ui.label("Silent device");
                egui::ComboBox::from_id_salt("sink").selected_text(self.draft.audio.sink.as_str()).width(340.0).show_ui(ui, |ui| {
                    for d in &devices {
                        ui.selectable_value(&mut self.draft.audio.sink, d.clone(), d);
                    }
                })
                .response
                .on_hover_text("A device you never listen to. The music app is moved here while Sidechain runs.");
                ui.end_row();
                ui.label("Play through");
                let out_txt = if self.draft.audio.output.is_empty() { "where the music app was playing".to_string() } else { self.draft.audio.output.clone() };
                egui::ComboBox::from_id_salt("output").selected_text(out_txt).width(340.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.draft.audio.output, String::new(), "where the music app was playing");
                    for d in &devices {
                        if *d != self.draft.audio.sink {
                            ui.selectable_value(&mut self.draft.audio.output, d.clone(), d);
                        }
                    }
                });
                ui.end_row();
                ui.label("Buffer");
                ui.spacing_mut().slider_width = 240.0;
                ui.add(egui::Slider::new(&mut self.draft.audio.latency_ms, 10.0..=120.0).suffix(" ms").fixed_decimals(0)).on_hover_text("Raise it if you hear crackles");
                ui.end_row();
                ui.label("Only during Deadlock");
                ui.horizontal(|ui| {
                    toggle(ui, &mut self.draft.audio.only_with_game);
                    ui.label(caption("Leave the music app alone unless Deadlock is running"));
                });
                ui.end_row();
            });
        });
    }

    fn engine_card(&mut self, ui: &mut Ui, snap: &Snapshot) {
        card(ui, |ui| {
            card_head(ui, icon::CPU, "Stem engine", Some("How the music is split into drums, vocals, bassline and the rest."));
            if !self.draft.stems.enabled {
                ui.horizontal(|ui| {
                    ui.label(muted("Stems are off."));
                    if ghost_button(ui, "Turn them on").clicked() {
                        self.draft.stems.enabled = true;
                    }
                });
                return;
            }
            segmented(ui, &mut self.draft.stems.mode, &[(StemMode::Neural, "Neural"), (StemMode::Dsp, "Fast")]);
            let neural = self.draft.stems.mode == StemMode::Neural;
            ui.label(muted(if neural {
                "HT-Demucs on your GPU: clean stems including the bassline. The music plays a few seconds behind Spotify (game reactions stay instant)."
            } else {
                "A real-time approximation on the CPU: no delay and no GPU, but the stems bleed into each other and there's no bassline stem."
            }));
            if !neural {
                return;
            }
            ui.add_space(4.0);
            egui::Grid::new("neural").num_columns(2).spacing([18.0, 10.0]).show(ui, |ui| {
                ui.label("Separate every");
                ui.horizontal(|ui| {
                    ui.spacing_mut().slider_width = 200.0;
                    ui.add(egui::Slider::new(&mut self.draft.stems.neural_hop_s, 1.0..=6.0).suffix(" s").fixed_decimals(1));
                    let hop = self.draft.stems.neural_hop_s;
                    let ms = if snap.neural_ms > 0 { snap.neural_ms as f32 } else { 150.0 };
                    ui.label(caption(format!("music ~{:.1} s behind · GPU busy ~{:.0}%", crate::neural::NeuralStems::latency_s(hop), 100.0 * ms / 1000.0 / hop)));
                });
                ui.end_row();
                ui.label("GPU priority");
                ui.horizontal(|ui| {
                    segmented(ui, &mut self.draft.stems.neural_priority, &[(GpuPriority::Auto, "Auto"), (GpuPriority::Normal, "Normal"), (GpuPriority::High, "High")]);
                    ui.label(caption(match self.draft.stems.neural_priority {
                        GpuPriority::Auto => "Cuts ahead of the game only while separations run late",
                        GpuPriority::Normal => "Never cuts ahead: smoothest frames",
                        GpuPriority::High => "Always cuts ahead: fastest, a short hitch each time",
                    }));
                });
                ui.end_row();
            });
            ui.add_space(4.0);
            if snap.stems_neural {
                let behind = snap.neural_ready && snap.neural_ms as f32 > self.draft.stems.neural_hop_s * 1000.0 * 0.9;
                let (state, text) = if !snap.neural_error.is_empty() {
                    (Some(false), format!("Error: {}", snap.neural_error))
                } else if snap.neural_ready {
                    let detail = if snap.neural_ms == 0 { "waiting for music".to_string() } else { format!("{:.2} s per separation", snap.neural_ms as f32 / 1000.0) };
                    (Some(!behind && snap.neural_coverage >= 90), format!("{} · {detail} · {}% with full look-ahead", snap.neural_device, snap.neural_coverage))
                } else {
                    (None, "Loading the model…".to_string())
                };
                check_row(ui, state, "Separator", &text, |_| {});
                if snap.neural_misses > 0 {
                    ui.label(caption(format!("{:.1} s played unseparated so far (separations fell behind)", snap.neural_misses as f32 / 44_100.0)));
                }
                if behind {
                    banner_inline(ui, &format!(
                        "Can't keep up: each separation takes {:.1} s but you asked for one every {:.1} s. {}",
                        snap.neural_ms as f32 / 1000.0,
                        self.draft.stems.neural_hop_s,
                        if self.draft.stems.neural_priority == GpuPriority::High {
                            format!("Separate every {:.0} s or more.", (snap.neural_ms as f32 / 1000.0 * 1.3).ceil())
                        } else {
                            "Set GPU priority to High.".to_string()
                        }
                    ));
                }
            } else if snap.stems {
                banner_inline(ui, &format!("Model not found ({}); using Fast for now. See the README's neural stems setup.", self.draft.stems.neural_model));
            }
        });
    }

    fn input_card(&mut self, ui: &mut Ui, snap: &Snapshot) {
        card(ui, |ui| {
            card_head(ui, icon::GAME_CONTROLLER, "Game input", Some("Applies after a restart."));
            egui::Grid::new("input").num_columns(2).spacing([14.0, 10.0]).show(ui, |ui| {
                ui.label("Netconsole port");
                ui.add(egui::DragValue::new(&mut self.draft.input.netcon_port).range(0..=65535)).on_hover_text("Matches -netconport in the launch options; 0 = off");
                ui.end_row();
                ui.label("console.log");
                ui.add(egui::TextEdit::singleline(&mut self.draft.input.console_log).hint_text("found automatically").desired_width(f32::INFINITY));
                ui.end_row();
            });
            ui.label(caption(format!("Reading {} · {} lines", snap.input, snap.hook_lines)));
        });
    }

    fn files_card(&mut self, ui: &mut Ui) {
        card(ui, |ui| {
            card_head(ui, icon::FOLDER_OPEN, "Files", None);
            let path = self.hub.cfg_path.display().to_string();
            ui.add(egui::Label::new(RichText::new(&path).size(12.0).color(TEXT_2)).truncate()).on_hover_text(&path);
            ui.horizontal(|ui| {
                if ghost_button(ui, format!("{}  Open folder", icon::FOLDER_OPEN)).clicked() {
                    if let Some(dir) = self.hub.cfg_path.parent() {
                        let _ = std::process::Command::new("explorer").arg(dir).spawn();
                    }
                }
                if ghost_button(ui, format!("{}  Reset sound to defaults", icon::ARROW_COUNTER_CLOCKWISE)).on_hover_text("Keeps your audio and input setup").clicked() {
                    let mut d = Config::default_config();
                    d.audio = self.draft.audio.clone();
                    d.input = self.draft.input.clone();
                    self.draft = d;
                    self.flash("Defaults loaded — Save to keep them");
                }
            });
        });
    }

    fn diagnostics_card(&mut self, ui: &mut Ui, snap: &Snapshot) {
        card(ui, |ui| {
            card_head(ui, icon::PULSE, "Diagnostics", None);
            ui.horizontal(|ui| {
                ui.label(muted("Output"));
                let v = ((snap.peak_db + 48.0) / 48.0).clamp(0.0, 1.0);
                meter(ui, v, vec2(160.0, 6.0), if snap.peak_db > -1.5 { BAD } else { GOOD });
                ui.label(RichText::new(format!("{:.1} dB", snap.peak_db)).monospace().size(12.0).color(TEXT_2));
                ui.add_space(12.0);
                ui.label(caption(format!(
                    "on {} · clock drift {:+} ppm · dropouts {}",
                    if snap.output.is_empty() { "…" } else { &snap.output },
                    snap.drift_ppm,
                    snap.xruns
                )));
            });
            let open = egui::CollapsingHeader::new(RichText::new("Activity log").font(semi(13.0))).default_open(self.show_log).show(ui, |ui| {
                egui::Frame::new().fill(Color32::from_rgb(16, 17, 21)).corner_radius(egui::CornerRadius::same(8)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(260.0).stick_to_bottom(true).auto_shrink([false, true]).show(ui, |ui| {
                        for l in log_lines() {
                            ui.label(RichText::new(l).monospace().size(11.5).color(TEXT_2));
                        }
                    });
                });
            });
            self.show_log = open.fully_open();
        });
    }
}

fn banner_inline(ui: &mut Ui, text: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(icon::WARNING_CIRCLE).color(WARN));
        ui.label(RichText::new(text).color(WARN));
    });
}
