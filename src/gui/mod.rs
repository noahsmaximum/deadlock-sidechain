//! Settings window (egui) + tray icon. Closing the window hides it to the tray; the engine keeps
//! running. Every edit applies live; Save writes sidechain.toml.
//!
//! Four places: **Now** (watch it work), **Sound** (calm vs full energy, stems, what builds
//! energy), **Moments** (each game event: what it does to the music, its feel, a test button) and
//! **Setup** (connection, devices, stem engine, diagnostics).

mod catalog;
mod moments;
mod now;
mod setup;
mod sound;
mod theme;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use eframe::egui::{self, Color32, CornerRadius, RichText, Sense, Stroke, Ui, pos2, vec2};

use crate::config::{Config, PARAMS, Param, ParamValues};
use crate::hub::{Cmd, Hub, Snapshot};
use crate::presets::{self, Preset};
use crate::{CLEANED, STOP};
use theme::*;

// ---------------------------------------------------------------------------- icon

/// Procedural icon: dark disc with three brass EQ bars.
fn icon_rgba(size: u32) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    let c = s / 2.0;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let d = ((fx - c).powi(2) + (fy - c).powi(2)).sqrt();
            let a = (c - d).clamp(0.0, 1.0);
            if a <= 0.0 {
                continue;
            }
            let mut col = [22u8, 24, 30];
            for (bx, bh) in [(0.30, 0.34), (0.50, 0.58), (0.70, 0.44)] {
                let half_w = s * 0.075;
                let bottom = s * 0.74;
                let top = bottom - s * bh;
                if (fx - s * bx).abs() < half_w && fy > top && fy < bottom {
                    col = [240, 168, 48];
                }
            }
            let i = ((y * size + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[col[0], col[1], col[2], (a * 255.0) as u8]);
        }
    }
    px
}

// ---------------------------------------------------------------------------- window plumbing

static HWND: AtomicIsize = AtomicIsize::new(0);

fn show_window(show: bool) {
    use windows::Win32::Foundation::HWND as WinHwnd;
    use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_RESTORE, SetForegroundWindow, ShowWindow};
    let h = HWND.load(Ordering::Relaxed);
    if h == 0 {
        return;
    }
    let hwnd = WinHwnd(h as *mut core::ffi::c_void);
    unsafe {
        let _ = ShowWindow(hwnd, if show { SW_RESTORE } else { SW_HIDE });
        if show {
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

fn quit_soon(hub: &Hub) {
    hub.send(Cmd::Quit);
    STOP.store(true, Ordering::SeqCst);
    std::thread::spawn(|| {
        let t = Instant::now();
        while !CLEANED.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::process::exit(0);
    });
}

struct Tray {
    _icon: tray_icon::TrayIcon,
    bypass: tray_icon::menu::CheckMenuItem,
}

fn make_tray(hub: Arc<Hub>, visible: Arc<AtomicBool>, ctx: egui::Context) -> Option<Tray> {
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let open = MenuItem::new("Open Sidechain", true, None);
    let bypass = CheckMenuItem::new("Bypass (dry music)", true, false, None);
    let quit = MenuItem::new("Quit (gives Spotify back)", true, None);
    let menu = Menu::new();
    menu.append_items(&[&open, &bypass, &PredefinedMenuItem::separator(), &quit]).ok()?;
    let icon = Icon::from_rgba(icon_rgba(32), 32, 32).ok()?;
    let tray = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("Sidechain").with_icon(icon).build().ok()?;

    let (open_id, bypass_id, quit_id) = (open.id().clone(), bypass.id().clone(), quit.id().clone());
    let (h1, v1, c1) = (hub.clone(), visible.clone(), ctx.clone());
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == open_id {
            v1.store(true, Ordering::Relaxed);
            show_window(true);
            c1.request_repaint();
        } else if e.id == bypass_id {
            let b = !h1.shared.bypass.load(Ordering::Relaxed);
            h1.send(Cmd::Bypass(b));
            c1.request_repaint();
        } else if e.id == quit_id {
            quit_soon(&h1);
        }
    }));
    let (v2, c2) = (visible, ctx);
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            let show = !v2.load(Ordering::Relaxed);
            v2.store(show, Ordering::Relaxed);
            show_window(show);
            c2.request_repaint();
        }
    }));
    let _ = hub;
    Some(Tray { _icon: tray, bypass })
}

fn refresh_devices(out: Arc<Mutex<Vec<String>>>) {
    std::thread::spawn(move || {
        let _ = wasapi::initialize_mta();
        let mut v = Vec::new();
        if let Ok(en) = wasapi::DeviceEnumerator::new() {
            if let Ok(c) = en.get_device_collection(&wasapi::Direction::Render) {
                for i in 0..c.get_nbr_devices().unwrap_or(0) {
                    if let Ok(name) = c.get_device_at_index(i).and_then(|d| d.get_friendlyname()) {
                        v.push(name);
                    }
                }
            }
        }
        *out.lock().unwrap() = v;
    });
}

/// Static targets for a config at a given energy with no other signals (for the reference curves).
fn targets_at(cfg: &Config, energy: f32) -> ParamValues {
    let mut v = cfg.base_values();
    for r in cfg.mix.routes.iter().filter(|r| r.signal == "energy") {
        v[r.param.index()] += cfg.master.intensity * r.amount * r.shape(energy);
    }
    v[Param::GainDb.index()] += cfg.master.trim_db;
    for p in PARAMS {
        v[p.index()] = p.clamp(v[p.index()]);
    }
    v
}

// ---------------------------------------------------------------------------- app

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Now,
    Sound,
    Moments,
    Setup,
}

impl Page {
    const ALL: [(Page, &'static str, &'static str, &'static str); 4] = [
        (Page::Now, icon::GAUGE, "Now", "What the music is doing right now"),
        (Page::Sound, icon::SLIDERS_HORIZONTAL, "Sound", "How the music sounds when calm, and at full energy"),
        (Page::Moments, icon::LIGHTNING, "Moments", "What each game event does to the music"),
        (Page::Setup, icon::GEAR_SIX, "Setup", "Connection, audio devices and the stem engine"),
    ];
}

struct App {
    hub: Arc<Hub>,
    draft: Config,
    saved: Config,
    last_sent: Config,
    draft_gen: u64,
    last_send: Instant,
    error: Option<String>,
    disk_changed: bool,
    page: Page,
    presets: Vec<Preset>,
    preset_dir: PathBuf,
    preset_sel: String,
    /// Some(name being typed) while the "save as preset" dialog is open.
    naming_preset: Option<String>,
    devices: Arc<Mutex<Vec<String>>>,
    visible: Arc<AtomicBool>,
    _tray: Option<Tray>,
    told_tray: bool,
    freqs: Vec<f32>,
    toast: Option<(Instant, String)>,
    /// Moments page: selected signal.
    moment: String,
    show_log: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, hub: Arc<Hub>, start_hidden: bool) -> Self {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(h) = cc.window_handle() {
            if let RawWindowHandle::Win32(w) = h.as_raw() {
                HWND.store(w.hwnd.get(), Ordering::Relaxed);
            }
        }
        let _ = hub.ui.set(cc.egui_ctx.clone());
        theme::install(&cc.egui_ctx);

        let (generation, cfg) = hub.active.lock().unwrap().clone();
        let visible = Arc::new(AtomicBool::new(!start_hidden));
        let tray = make_tray(hub.clone(), visible.clone(), cc.egui_ctx.clone());
        let preset_dir = presets::dir(&hub.cfg_path);
        let mut all = presets::builtin();
        all.extend(presets::load_user(&preset_dir));
        let devices = Arc::new(Mutex::new(Vec::new()));
        refresh_devices(devices.clone());
        let freqs = (0..240).map(|i| 20.0 * 1000f32.powf(i as f32 / 239.0)).collect();
        if start_hidden {
            show_window(false);
        }
        // SIDECHAIN_TAB picks the start page (screenshots); old tab names still work
        let page = match std::env::var("SIDECHAIN_TAB").unwrap_or_default().as_str() {
            "sound" | "energy" => Page::Sound,
            "moments" | "signals" | "routes" | "rules" => Page::Moments,
            "setup" | "audio" | "log" => Page::Setup,
            _ => Page::Now,
        };
        App {
            hub,
            draft: cfg.clone(),
            saved: cfg.clone(),
            last_sent: cfg,
            draft_gen: generation,
            last_send: Instant::now(),
            error: None,
            disk_changed: false,
            page,
            presets: all,
            preset_dir,
            preset_sel: String::new(),
            naming_preset: None,
            devices,
            visible,
            _tray: tray,
            told_tray: false,
            freqs,
            toast: None,
            moment: std::env::var("SIDECHAIN_MOMENT").unwrap_or_else(|_| "kill".into()),
            show_log: std::env::var("SIDECHAIN_TAB").is_ok_and(|t| t == "log"),
        }
    }

    fn flash(&mut self, s: impl Into<String>) {
        self.toast = Some((Instant::now(), s.into()));
    }

    fn reload_presets(&mut self) {
        let mut all = presets::builtin();
        all.extend(presets::load_user(&self.preset_dir));
        self.presets = all;
    }

    fn sync_from_engine(&mut self, snap: &Snapshot) {
        if snap.cfg_gen == self.draft_gen {
            return;
        }
        self.draft_gen = snap.cfg_gen;
        let active = self.hub.active.lock().unwrap().1.clone();
        if active == self.last_sent {
            return; // our own apply coming back
        }
        if self.draft == self.saved {
            self.draft = active.clone();
            self.saved = active.clone();
            self.last_sent = active;
        } else {
            self.disk_changed = true;
        }
    }

    fn push_draft(&mut self) {
        if self.draft == self.last_sent || self.last_send.elapsed() < Duration::from_millis(50) {
            return;
        }
        match self.draft.validate() {
            Ok(()) => {
                self.hub.send(Cmd::Apply(self.draft.clone()));
                self.last_sent = self.draft.clone();
                self.last_send = Instant::now();
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    fn save(&mut self) {
        self.hub.send(Cmd::Save(self.draft.clone()));
        self.saved = self.draft.clone();
        self.last_sent = self.draft.clone();
        self.flash("Saved");
    }

    fn signal_names(&self) -> Vec<String> {
        self.draft.signals.keys().cloned().collect()
    }

    fn test(&self, name: &str) {
        self.hub.send(Cmd::Test(name.to_string()));
    }
}

// ---------------------------------------------------------------------------- overall status

/// One line for "is everything working", worst problem first.
fn status(snap: &Snapshot) -> (Color32, String, &'static str) {
    let hook_live = snap.last_line_secs.is_some_and(|s| s < 10.0);
    if !snap.game_running {
        (TEXT_3, "Waiting for Deadlock".into(), "Start Deadlock and Sidechain takes over the music")
    } else if !hook_live {
        (WARN, "No game data yet".into(), "The hook addon isn't reporting: check the addon and launch options in Setup")
    } else if !snap.app_ok {
        (WARN, snap.app.clone(), "Music isn't flowing through Sidechain yet")
    } else if snap.live {
        (GOOD, "Live · in a match".into(), "Everything is connected")
    } else {
        (GOOD, "Live · in menus".into(), "Everything is connected")
    }
}

// ---------------------------------------------------------------------------- frame

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if CLEANED.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if ctx.input(|i| i.viewport().close_requested()) && !STOP.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.visible.store(false, Ordering::Relaxed);
            show_window(false);
            if !self.told_tray {
                crate::log("window hidden — Sidechain keeps running in the tray (right-click it to quit)");
                self.told_tray = true;
            }
        }
        if self.hub.show_request.swap(false, Ordering::Relaxed) {
            self.visible.store(true, Ordering::Relaxed);
            show_window(true);
        }

        let snap = self.hub.snap.lock().unwrap().clone();
        self.sync_from_engine(&snap);
        if let Some(t) = &self._tray {
            if t.bypass.is_checked() != snap.bypass {
                t.bypass.set_checked(snap.bypass);
            }
        }
        let dirty = self.draft != self.saved;
        if dirty && ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::S)) {
            self.save();
        }

        self.rail(ctx, &snap);
        if dirty || self.toast.as_ref().is_some_and(|(t, _)| t.elapsed() < Duration::from_secs(3)) {
            self.save_bar(ctx, dirty);
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin { left: 28, right: 28, top: 20, bottom: 12 })).show(ctx, |ui| {
            self.header(ui, &snap);
            ui.add_space(14.0);
            self.banners(ui);
            match self.page {
                Page::Moments => self.page_moments(ui, &snap),
                page => {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 12.0;
                        match page {
                            Page::Now => self.page_now(ui, &snap),
                            Page::Sound => self.page_sound(ui, &snap),
                            _ => self.page_setup(ui, &snap),
                        }
                        ui.add_space(12.0);
                    });
                }
            }
        });
        self.preset_dialog(ctx);

        self.push_draft();
        if self.visible.load(Ordering::Relaxed) {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }
}

// ---------------------------------------------------------------------------- chrome

impl App {
    fn rail(&mut self, ctx: &egui::Context, snap: &Snapshot) {
        egui::SidePanel::left("rail")
            .exact_width(206.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(RAIL).inner_margin(egui::Margin { left: 14, right: 14, top: 18, bottom: 14 }).stroke(Stroke::new(1.0, BORDER)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new(icon::WAVEFORM).size(22.0).color(ACCENT));
                    ui.label(title("Sidechain", 18.0));
                });
                ui.add_space(22.0);
                for (page, glyph, name, _) in Page::ALL {
                    if nav_item(ui, glyph, name, self.page == page).clicked() {
                        self.page = page;
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.label(caption(format!("v{}", env!("CARGO_PKG_VERSION"))));
                    ui.add_space(4.0);
                    let (color, text, tip) = status(snap);
                    let r = egui::Frame::new()
                        .fill(SURFACE)
                        .stroke(Stroke::new(1.0, BORDER))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                dot(ui, color);
                                ui.add(egui::Label::new(RichText::new(text).size(12.5).color(TEXT)).wrap());
                            });
                            if snap.bypass {
                                ui.horizontal(|ui| {
                                    dot(ui, WARN);
                                    ui.label(RichText::new("Effects off (bypass)").size(12.5).color(WARN));
                                });
                            }
                        })
                        .response
                        .interact(Sense::click())
                        .on_hover_text(format!("{tip}\nClick for Setup"));
                    if r.clicked() {
                        self.page = Page::Setup;
                    }
                });
            });
    }

    fn header(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let (_, _, name, sub) = Page::ALL.iter().find(|p| p.0 == self.page).copied().unwrap();
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(title(name, 24.0));
                ui.add_space(-6.0);
                ui.label(muted(sub));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // effects on/off = bypass
                let mut on = !snap.bypass;
                if toggle(ui, &mut on).on_hover_text("Turn the effects off to compare with the dry track (tray: Bypass)").changed() {
                    self.hub.send(Cmd::Bypass(!on));
                }
                ui.label(RichText::new("Effects").color(if on { TEXT } else { WARN }));
                ui.add_space(14.0);
                ui.spacing_mut().slider_width = 110.0;
                ui.add(egui::Slider::new(&mut self.draft.master.intensity, 0.0..=2.0).show_value(false))
                    .on_hover_text("Scales every move: 0% = dry, 100% = as designed, 200% = double");
                ui.label(RichText::new(format!("{:.0}%", self.draft.master.intensity * 100.0)).monospace().color(TEXT_2));
                ui.label("Intensity");
                ui.add_space(14.0);
                self.preset_menu(ui);
            });
        });
    }

    fn preset_menu(&mut self, ui: &mut Ui) {
        let label = if self.preset_sel.is_empty() { "Presets".to_string() } else { self.preset_sel.clone() };
        let mut pick: Option<Preset> = None;
        let mut delete: Option<String> = None;
        let mut save_as = false;
        egui::ComboBox::from_id_salt("presets").selected_text(format!("{}  {label}", icon::STACK)).width(170.0).show_ui(ui, |ui| {
            ui.set_min_width(240.0);
            for (builtin, head) in [(true, "Built in"), (false, "Yours")] {
                let list: Vec<&Preset> = self.presets.iter().filter(|p| p.builtin == builtin).collect();
                if list.is_empty() {
                    continue;
                }
                eyebrow(ui, head);
                for p in list {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(self.preset_sel == p.name, &p.name).on_hover_text(p.about).clicked() {
                            pick = Some(p.clone());
                        }
                        if !p.builtin {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if icon_button(ui, icon::TRASH, "Delete this preset").clicked() {
                                    delete = Some(p.name.clone());
                                }
                            });
                        }
                    });
                }
                ui.add_space(4.0);
            }
            ui.separator();
            if ui.selectable_label(false, format!("{}  Save current sound as preset…", icon::PLUS)).clicked() {
                save_as = true;
            }
        });
        if let Some(p) = pick {
            self.draft.set_sound(p.sound);
            self.preset_sel = p.name.clone();
            self.flash(format!("Loaded '{}' — Save to keep it", p.name));
        }
        if let Some(n) = delete {
            let _ = presets::delete_user(&self.preset_dir, &n);
            self.reload_presets();
        }
        if save_as {
            self.naming_preset = Some(self.preset_sel.clone());
        }
    }

    fn preset_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut name) = self.naming_preset.take() else { return };
        let mut keep = true;
        egui::Modal::new(egui::Id::new("save_preset")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.label(title("Save as preset", 16.0));
            ui.label(muted("Keeps the whole sound (calm, energy, moments). Audio setup isn't included."));
            ui.add_space(6.0);
            let r = ui.add(egui::TextEdit::singleline(&mut name).hint_text("Preset name").desired_width(f32::INFINITY));
            r.request_focus();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ok = !name.trim().is_empty();
                let enter = ok && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.add_enabled(ok, egui::Button::new(RichText::new("Save").font(semi(13.5)).color(Color32::from_rgb(24, 20, 12))).fill(ACCENT)).clicked() || enter {
                    let n = name.trim().to_string();
                    match presets::save_user(&self.preset_dir, &n, &self.draft.sound()) {
                        Ok(()) => {
                            self.flash(format!("Preset '{n}' saved"));
                            self.preset_sel = n;
                            self.reload_presets();
                        }
                        Err(e) => self.flash(format!("Couldn't save the preset: {e:#}")),
                    }
                    keep = false;
                }
                if ghost_button(ui, "Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    keep = false;
                }
            });
        });
        if keep {
            self.naming_preset = Some(name);
        }
    }

    fn banners(&mut self, ui: &mut Ui) {
        if let Some(e) = &self.error {
            banner(ui, BAD, icon::WARNING_CIRCLE, &format!("Not applied: {e}"));
        }
        if self.disk_changed {
            let mut load = false;
            let mut keep = false;
            egui::Frame::new().fill(SURFACE).stroke(Stroke::new(1.0, WARN.gamma_multiply(0.5))).corner_radius(CornerRadius::same(10)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::FILE_TEXT).color(WARN));
                    ui.label("sidechain.toml changed on disk while you had unsaved edits.");
                    load = ghost_button(ui, "Load the file").clicked();
                    keep = ghost_button(ui, "Keep mine").clicked();
                });
            });
            ui.add_space(10.0);
            if load {
                let a = self.hub.active.lock().unwrap().1.clone();
                self.draft = a.clone();
                self.saved = a.clone();
                self.last_sent = a;
                self.disk_changed = false;
            }
            if keep {
                self.disk_changed = false;
                self.last_sent = Config::default_config(); // force re-apply of the draft
            }
        }
    }

    fn save_bar(&mut self, ctx: &egui::Context, dirty: bool) {
        egui::TopBottomPanel::bottom("save")
            .frame(egui::Frame::new().fill(SURFACE).inner_margin(egui::Margin::symmetric(28, 10)).stroke(Stroke::new(1.0, BORDER)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if dirty {
                        dot(ui, WARN);
                        ui.label("Unsaved changes");
                        ui.label(caption("They're already playing; Save keeps them for next time"));
                    } else if let Some((_, m)) = &self.toast {
                        ui.label(RichText::new(icon::CHECK_CIRCLE).color(GOOD));
                        ui.label(m.as_str());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if dirty {
                            if primary_button(ui, &format!("{}  Save", icon::FLOPPY_DISK)).on_hover_text("Ctrl+S").clicked() {
                                self.save();
                            }
                            if ghost_button(ui, "Revert").on_hover_text("Go back to the saved settings").clicked() {
                                self.draft = self.saved.clone();
                            }
                            if let Some((t, m)) = &self.toast {
                                if t.elapsed() < Duration::from_secs(4) {
                                    ui.label(RichText::new(m.as_str()).color(TEXT_2));
                                }
                            }
                        }
                    });
                });
            });
    }
}

fn nav_item(ui: &mut Ui, glyph: &str, name: &str, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, CornerRadius::same(9), SURFACE_2);
        p.rect_filled(egui::Rect::from_min_size(pos2(rect.left(), rect.top() + 10.0), vec2(3.0, 20.0)), CornerRadius::same(2), ACCENT);
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(9), SURFACE);
    }
    let c = if selected { TEXT } else if resp.hovered() { TEXT } else { TEXT_2 };
    p.text(pos2(rect.left() + 16.0, rect.center().y), egui::Align2::LEFT_CENTER, glyph, egui::FontId::proportional(18.0), if selected { ACCENT } else { c });
    p.text(pos2(rect.left() + 44.0, rect.center().y), egui::Align2::LEFT_CENTER, name, semi(14.0), c);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.add_space(2.0);
    resp
}

fn banner(ui: &mut Ui, color: Color32, glyph: &str, text: &str) {
    egui::Frame::new().fill(SURFACE).stroke(Stroke::new(1.0, color.gamma_multiply(0.5))).corner_radius(CornerRadius::same(10)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(glyph).color(color));
            ui.label(RichText::new(text).color(TEXT));
        });
    });
    ui.add_space(10.0);
}

// ---------------------------------------------------------------------------- entry

pub fn run(hub: Arc<Hub>, start_hidden: bool) -> Result<()> {
    let icon = egui::IconData { rgba: icon_rgba(64), width: 64, height: 64 };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Sidechain")
        .with_inner_size([1120.0, 780.0])
        .with_min_inner_size([940.0, 600.0])
        .with_icon(Arc::new(icon));
    // SIDECHAIN_WINDOW_SIZE=w,h for full-page screenshots
    if let Some((w, h)) = std::env::var("SIDECHAIN_WINDOW_SIZE").ok().and_then(|v| {
        let (a, b) = v.split_once(',')?;
        Some((a.trim().parse::<f32>().ok()?, b.trim().parse::<f32>().ok()?))
    }) {
        viewport = viewport.with_inner_size([w, h]);
    }
    // SIDECHAIN_WINDOW_POS=x,y places the window (and doesn't activate it) — for screenshots.
    if let Some((x, y)) = std::env::var("SIDECHAIN_WINDOW_POS").ok().and_then(|v| {
        let (a, b) = v.split_once(',')?;
        Some((a.trim().parse::<f32>().ok()?, b.trim().parse::<f32>().ok()?))
    }) {
        viewport = viewport.with_position([x, y]).with_active(false);
    }
    let options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Glow, ..Default::default() };
    eframe::run_native("Sidechain", options, Box::new(move |cc| Ok(Box::new(App::new(cc, hub, start_hidden)))))
        .map_err(|e| anyhow!("window: {e}"))
}
