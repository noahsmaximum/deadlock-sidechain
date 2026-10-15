//! Settings window (egui) + tray icon. Closing the window hides it to the tray; the engine keeps
//! running. Every edit applies live; Save writes sidechain.toml.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2, vec2,
};

use crate::config::{Config, PARAMS, Param, ParamValues, Route, RuleCfg, STEM_PARAMS, SignalCfg};
use crate::hub::{Cmd, Hub, Snapshot, log_lines};
use crate::presets::{self, Preset};
use crate::{CLEANED, STOP};

// ---------------------------------------------------------------------------- palette

const ACCENT: Color32 = Color32::from_rgb(240, 168, 48); // brass
const CALM: Color32 = Color32::from_rgb(74, 123, 208);
const GOOD: Color32 = Color32::from_rgb(98, 196, 120);
const BAD: Color32 = Color32::from_rgb(226, 92, 80);
const DIM: Color32 = Color32::from_rgb(130, 136, 148);
const PANEL: Color32 = Color32::from_rgb(24, 26, 31);
const BG: Color32 = Color32::from_rgb(16, 17, 21);

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

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
            // bars: x positions at 0.3, 0.5, 0.7 of width; heights 0.35, 0.6, 0.45
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

// ---------------------------------------------------------------------------- app

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Live,
    Energy,
    Signals,
    Routes,
    Rules,
    Audio,
    Log,
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
    tab: Tab,
    presets: Vec<Preset>,
    preset_dir: PathBuf,
    preset_sel: String,
    new_preset: String,
    naming_preset: bool,
    devices: Arc<Mutex<Vec<String>>>,
    route_filter: String,
    visible: Arc<AtomicBool>,
    _tray: Option<Tray>,
    told_tray: bool,
    freqs: Vec<f32>,
    status_msg: Option<(Instant, String)>,
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

impl App {
    fn new(cc: &eframe::CreationContext<'_>, hub: Arc<Hub>, start_hidden: bool) -> Self {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(h) = cc.window_handle() {
            if let RawWindowHandle::Win32(w) = h.as_raw() {
                HWND.store(w.hwnd.get(), Ordering::Relaxed);
            }
        }
        let _ = hub.ui.set(cc.egui_ctx.clone());
        style(&cc.egui_ctx);

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
        App {
            hub,
            draft: cfg.clone(),
            saved: cfg.clone(),
            last_sent: cfg,
            draft_gen: generation,
            last_send: Instant::now(),
            error: None,
            disk_changed: false,
            tab: match std::env::var("SIDECHAIN_TAB").unwrap_or_default().as_str() {
                "energy" => Tab::Energy,
                "signals" => Tab::Signals,
                "routes" => Tab::Routes,
                "rules" => Tab::Rules,
                "audio" => Tab::Audio,
                "log" => Tab::Log,
                _ => Tab::Live,
            },
            presets: all,
            preset_dir,
            preset_sel: String::new(),
            new_preset: String::new(),
            naming_preset: false,
            devices,
            route_filter: String::new(),
            visible,
            _tray: tray,
            told_tray: false,
            freqs,
            status_msg: None,
        }
    }

    fn flash(&mut self, s: impl Into<String>) {
        self.status_msg = Some((Instant::now(), s.into()));
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

    fn signal_names(&self) -> Vec<String> {
        self.draft.signals.keys().cloned().collect()
    }
}

fn style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(10, 11, 14);
    v.selection.bg_fill = ACCENT.linear_multiply(0.55);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.linear_multiply(0.6));
    v.slider_trailing_fill = true;
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.slider_width = 170.0;
    });
}

// ---------------------------------------------------------------------------- widgets

fn meter(ui: &mut Ui, v: f32, size: Vec2, color: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(3), ui.visuals().extreme_bg_color);
    let mut r = rect;
    r.set_width(rect.width() * v.clamp(0.0, 1.0));
    if r.width() > 0.5 {
        p.rect_filled(r, CornerRadius::same(3), color);
    }
    resp
}

fn pill(ui: &mut Ui, ok: Option<bool>, text: &str) {
    let color = match ok {
        Some(true) => GOOD,
        Some(false) => BAD,
        None => DIM,
    };
    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 4.0, color);
    ui.label(RichText::new(text).color(Color32::from_gray(210)));
}

fn card<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(PANEL)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.label(RichText::new(title.to_uppercase()).size(11.0).color(DIM).strong());
            ui.add_space(4.0);
            add(ui)
        })
        .inner
}

/// Static targets for a config at a given energy with no other signals — for the reference curves.
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

fn eq_plot(ui: &mut Ui, freqs: &[f32], curves: &[(Vec<f32>, Stroke)], height: f32) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, height), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(6), Color32::from_rgb(12, 13, 16));
    let (db_top, db_bot) = (12.0f32, -30.0f32);
    let x_of = |f: f32| rect.left() + rect.width() * ((f / 20.0).log10() / 3.0);
    let y_of = |db: f32| rect.top() + rect.height() * ((db_top - db.clamp(db_bot, db_top)) / (db_top - db_bot));
    let grid = Stroke::new(1.0, Color32::from_gray(38));
    for db in [6.0, 0.0, -6.0, -12.0, -18.0, -24.0] {
        let y = y_of(db);
        p.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], if db == 0.0 { Stroke::new(1.0, Color32::from_gray(70)) } else { grid });
        p.text(Pos2::new(rect.left() + 4.0, y - 1.0), Align2::LEFT_BOTTOM, format!("{db:+.0}"), FontId::proportional(10.0), DIM);
    }
    for (f, lbl) in [(50.0, "50"), (100.0, "100"), (200.0, ""), (500.0, "500"), (1000.0, "1k"), (2000.0, ""), (5000.0, "5k"), (10000.0, "10k")] {
        let x = x_of(f);
        p.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], grid);
        if !lbl.is_empty() {
            p.text(Pos2::new(x + 3.0, rect.bottom() - 2.0), Align2::LEFT_BOTTOM, lbl, FontId::proportional(10.0), DIM);
        }
    }
    for (dbs, stroke) in curves {
        let pts: Vec<Pos2> = freqs.iter().zip(dbs).map(|(&f, &d)| Pos2::new(x_of(f), y_of(d))).collect();
        p.add(egui::Shape::line(pts, *stroke));
    }
}

fn signal_combo(ui: &mut Ui, id: impl std::hash::Hash, value: &mut String, names: &[String]) {
    egui::ComboBox::from_id_salt(id).selected_text(value.as_str()).width(140.0).show_ui(ui, |ui| {
        for n in names {
            ui.selectable_value(value, n.clone(), n);
        }
    });
}

fn param_combo(ui: &mut Ui, id: impl std::hash::Hash, value: &mut Param) {
    egui::ComboBox::from_id_salt(id).selected_text(value.label()).width(130.0).show_ui(ui, |ui| {
        for p in PARAMS {
            ui.selectable_value(value, p, p.label());
        }
    });
}

fn signal_color(name: &str) -> Color32 {
    match name {
        "energy" => ACCENT,
        "damage" | "dead" | "low_health" | "objective_loss" | "death" => BAD,
        "kill" | "assist" | "objective_win" | "midboss" | "patron_win" | "respawn" => GOOD,
        _ => Color32::from_rgb(120, 160, 230),
    }
}

const LEVELS: [&str; 7] = ["dead", "low_health", "speed", "shop", "scoreboard", "combat", "energy"];

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

        self.top_bar(ctx, &snap);
        self.bottom_bar(ctx, &snap);
        egui::SidePanel::left("tabs").exact_width(132.0).resizable(false).frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::same(10))).show(ctx, |ui| {
            ui.add_space(4.0);
            for (t, name) in [
                (Tab::Live, "Live"),
                (Tab::Energy, "Energy"),
                (Tab::Signals, "Signals"),
                (Tab::Routes, "Routes"),
                (Tab::Rules, "Text rules"),
                (Tab::Audio, "Audio"),
                (Tab::Log, "Log"),
            ] {
                let sel = self.tab == t;
                let txt = RichText::new(name).size(14.0).color(if sel { ACCENT } else { Color32::from_gray(200) });
                if ui.add_sized([112.0, 30.0], egui::Button::selectable(sel, txt)).clicked() {
                    self.tab = t;
                }
            }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                ui.label(RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).size(10.0).color(DIM));
            });
        });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::same(14))).show(ctx, |ui| {
            if let Some(e) = &self.error {
                ui.colored_label(BAD, format!("Not applied: {e}"));
            }
            if self.disk_changed {
                ui.horizontal(|ui| {
                    ui.colored_label(ACCENT, "sidechain.toml changed on disk.");
                    if ui.button("Load it").clicked() {
                        let a = self.hub.active.lock().unwrap().1.clone();
                        self.draft = a.clone();
                        self.saved = a.clone();
                        self.last_sent = a;
                        self.disk_changed = false;
                    }
                    if ui.button("Keep mine").clicked() {
                        self.disk_changed = false;
                        self.last_sent = Config::default_config(); // force re-apply of the draft
                    }
                });
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.tab {
                Tab::Live => self.tab_live(ui, &snap),
                Tab::Energy => self.tab_energy(ui, &snap),
                Tab::Signals => self.tab_signals(ui, &snap),
                Tab::Routes => self.tab_routes(ui),
                Tab::Rules => self.tab_rules(ui),
                Tab::Audio => self.tab_audio(ui, &snap),
                Tab::Log => self.tab_log(ui),
            });
        });

        self.push_draft();
        if self.visible.load(Ordering::Relaxed) {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }
}

// ---------------------------------------------------------------------------- bars

impl App {
    fn top_bar(&mut self, ctx: &egui::Context, snap: &Snapshot) {
        egui::TopBottomPanel::top("top").frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(14, 10))).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("SIDECHAIN").size(20.0).strong().color(ACCENT));
                ui.add_space(10.0);
                let mut bypass = snap.bypass;
                let txt = if bypass { RichText::new("  BYPASSED  ").strong().color(Color32::BLACK) } else { RichText::new("  ACTIVE  ").strong() };
                let b = egui::Button::new(txt).fill(if bypass { ACCENT } else { Color32::from_rgb(40, 70, 48) }).min_size(vec2(96.0, 28.0));
                if ui.add(b).on_hover_text("A/B the dry track (b in the console)").clicked() {
                    bypass = !bypass;
                    self.hub.send(Cmd::Bypass(bypass));
                }
                ui.add_space(12.0);
                ui.label("Intensity");
                ui.add(egui::Slider::new(&mut self.draft.master.intensity, 0.0..=2.0).fixed_decimals(2).show_value(true));
                ui.label("Trim");
                ui.add(egui::Slider::new(&mut self.draft.master.trim_db, -12.0..=6.0).suffix(" dB").fixed_decimals(1));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let dirty = self.draft != self.saved;
                    if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked() {
                        self.draft = self.saved.clone();
                    }
                    let save = egui::Button::new(RichText::new("Save").strong()).fill(if dirty { ACCENT.linear_multiply(0.45) } else { Color32::from_gray(45) });
                    if ui.add_enabled(dirty, save).on_hover_text("Write sidechain.toml").clicked() {
                        self.hub.send(Cmd::Save(self.draft.clone()));
                        self.saved = self.draft.clone();
                        self.last_sent = self.draft.clone();
                        self.flash("saved");
                    }
                    if dirty {
                        ui.label(RichText::new("unsaved").color(ACCENT).size(11.0));
                    }
                    ui.separator();
                    self.preset_menu(ui);
                });
            });
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                pill(ui, Some(snap.game_running), if snap.game_running { "Deadlock running" } else { "Deadlock not running" });
                ui.add_space(8.0);
                let hook_ok = snap.last_line_secs.is_some_and(|s| s < 10.0);
                let hook_txt = match (&snap.hook, snap.last_line_secs) {
                    (_, Some(s)) if s < 10.0 => format!("hook live{} via {}", snap.hook.as_deref().map(|v| format!(" v{v}")).unwrap_or_default(), snap.input),
                    (_, Some(_)) => format!("hook quiet ({} lines so far)", snap.hook_lines),
                    _ => "no hook lines yet".into(),
                };
                pill(ui, if snap.game_running { Some(hook_ok) } else { None }, &hook_txt);
                ui.add_space(8.0);
                pill(ui, Some(snap.app_ok), &snap.app);
                if snap.live {
                    ui.add_space(8.0);
                    pill(ui, Some(true), "in match");
                }
            });
        });
    }

    fn preset_menu(&mut self, ui: &mut Ui) {
        if self.naming_preset {
            if ui.button("Cancel").clicked() {
                self.naming_preset = false;
            }
            let ok = !self.new_preset.trim().is_empty();
            if ui.add_enabled(ok, egui::Button::new("Save preset")).clicked() {
                let name = self.new_preset.trim().to_string();
                match presets::save_user(&self.preset_dir, &name, &self.draft.sound()) {
                    Ok(()) => {
                        self.flash(format!("preset '{name}' saved"));
                        self.preset_sel = name;
                        self.reload_presets();
                    }
                    Err(e) => self.flash(format!("preset save failed: {e:#}")),
                }
                self.naming_preset = false;
            }
            ui.add(egui::TextEdit::singleline(&mut self.new_preset).hint_text("preset name").desired_width(120.0));
        } else if ui.button("Save as preset…").clicked() {
            self.naming_preset = true;
        }
        let label = if self.preset_sel.is_empty() { "Presets".to_string() } else { self.preset_sel.clone() };
        let mut pick: Option<Preset> = None;
        let mut delete: Option<String> = None;
        egui::ComboBox::from_id_salt("presets").selected_text(label).width(150.0).show_ui(ui, |ui| {
            for p in &self.presets {
                ui.horizontal(|ui| {
                    if ui.selectable_label(self.preset_sel == p.name, &p.name).on_hover_text(p.about).clicked() {
                        pick = Some(p.clone());
                    }
                    if !p.builtin && ui.small_button("delete").on_hover_text("Delete preset").clicked() {
                        delete = Some(p.name.clone());
                    }
                });
            }
        });
        if let Some(p) = pick {
            self.draft.set_sound(p.sound);
            self.preset_sel = p.name.clone();
            self.flash(format!("preset '{}' loaded (Save to keep it)", p.name));
        }
        if let Some(n) = delete {
            let _ = presets::delete_user(&self.preset_dir, &n);
            self.reload_presets();
        }
    }

    fn bottom_bar(&mut self, ctx: &egui::Context, snap: &Snapshot) {
        egui::TopBottomPanel::bottom("bottom").frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(14, 6))).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("OUT").size(11.0).color(DIM));
                let v = ((snap.peak_db + 48.0) / 48.0).clamp(0.0, 1.0);
                let c = if snap.peak_db > -1.5 { BAD } else { GOOD };
                meter(ui, v, vec2(140.0, 8.0), c);
                ui.label(RichText::new(format!("{:>5.1} dB", snap.peak_db)).monospace().size(11.0));
                ui.separator();
                ui.label(RichText::new(format!("playing on {}", if snap.output.is_empty() { "…" } else { &snap.output })).size(11.0).color(DIM));
                ui.separator();
                ui.label(RichText::new(format!("drift {:+} ppm · xruns {}", snap.drift_ppm, snap.xruns)).size(11.0).color(DIM));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some((t, m)) = &self.status_msg {
                        if t.elapsed() < Duration::from_secs(4) {
                            ui.label(RichText::new(m).color(ACCENT).size(11.0));
                        }
                    }
                });
            });
        });
    }
}

// ---------------------------------------------------------------------------- tabs

impl App {
    fn test_button(&self, ui: &mut Ui, name: &str, label: &str) {
        let hold = LEVELS.contains(&name);
        if ui.button(label).on_hover_text(if hold { "Hold this state for 1.5 s" } else { "Fire this event" }).clicked() {
            self.hub.send(Cmd::Test(name.to_string()));
        }
    }

    fn tab_live(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let sig = |n: &str| snap.signals.iter().find(|(k, _)| k == n).map_or(0.0, |(_, v)| *v);
        let energy = sig("energy");

        card(ui, "Energy", |ui| {
            ui.horizontal(|ui| {
                let txt = if energy >= 0.9 { "MAX".to_string() } else { format!("{:>3.0}%", energy * 100.0) };
                ui.label(RichText::new(txt).size(28.0).strong().color(lerp_color(CALM, ACCENT, energy)));
                ui.vertical(|ui| {
                    ui.add_space(6.0);
                    let w = ui.available_width();
                    let (rect, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(rect, CornerRadius::same(4), ui.visuals().extreme_bg_color);
                    let n = 48;
                    for i in 0..n {
                        let t0 = i as f32 / n as f32;
                        if t0 > energy {
                            break;
                        }
                        let x0 = rect.left() + rect.width() * t0;
                        let x1 = rect.left() + rect.width() * ((i + 1) as f32 / n as f32).min(energy);
                        p.rect_filled(Rect::from_min_max(Pos2::new(x0, rect.top()), Pos2::new(x1, rect.bottom())), CornerRadius::ZERO, lerp_color(CALM, ACCENT, t0));
                    }
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("calm: quiet · mono · no bass").size(11.0).color(CALM));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new("full: loud · wide · bass + air").size(11.0).color(ACCENT));
                        });
                    });
                });
            });
        });
        ui.add_space(8.0);

        card(ui, "What the music is doing", |ui| {
            let fs = crate::audio::RATE as f32;
            let calm = crate::dsp::response_db(&targets_at(&self.draft, 0.0), fs, &self.freqs);
            let full = crate::dsp::response_db(&targets_at(&self.draft, 1.0), fs, &self.freqs);
            let live = crate::dsp::response_db(&snap.targets, fs, &self.freqs);
            eq_plot(
                ui,
                &self.freqs,
                &[(calm, Stroke::new(1.0, CALM.linear_multiply(0.7))), (full, Stroke::new(1.0, ACCENT.linear_multiply(0.45))), (live, Stroke::new(2.5, Color32::WHITE))],
                190.0,
            );
            ui.horizontal(|ui| {
                for (name, c, w) in [("live", Color32::WHITE, 2.5), ("calm", CALM, 1.5), ("full energy", ACCENT, 1.5)] {
                    let (r, _) = ui.allocate_exact_size(vec2(18.0, 10.0), Sense::hover());
                    ui.painter().line_segment([r.left_center(), r.right_center()], Stroke::new(w, c));
                    ui.label(RichText::new(name).color(c).size(11.0));
                }
            });
            let t = &snap.targets;
            let g = |p: Param| t[p.index()];
            ui.horizontal_wrapped(|ui| {
                for (label, val) in [
                    ("Level", format!("{:+.1} dB", g(Param::GainDb))),
                    ("Width", format!("{:.2}", g(Param::Width))),
                    ("Bass", format!("{:+.1}", g(Param::BassDb))),
                    ("Mid", format!("{:+.1}", g(Param::MidDb))),
                    ("Treble", format!("{:+.1}", g(Param::TrebleDb))),
                    ("Low-pass", format!("{:.0} Hz", 20000.0 * 2f32.powf(g(Param::LowpassOct)))),
                    ("High-pass", format!("{:.0} Hz", 20.0 * 2f32.powf(g(Param::HighpassOct)))),
                    ("Drive", format!("{:.2}", g(Param::Drive))),
                ]
                .into_iter()
                .chain(
                    snap.stems
                        .then(|| STEM_PARAMS.map(|p| (p.name(), format!("{:+.0} dB", g(p)))))
                        .into_iter()
                        .flatten()
                        .map(|(n, v)| (match n { "drums" => "Drums", "vocals" => "Vocals", _ => "Music" }, v)),
                ) {
                    // plain labels (sized before layout) so the row wraps; frames can't
                    let chip = RichText::new(format!(" {label} {val} ")).size(12.0).monospace().background_color(Color32::from_rgb(32, 35, 42));
                    ui.add(egui::Label::new(chip).extend());
                }
            });
        });
        ui.add_space(8.0);
        if snap.stems {
            card(ui, "Stems", |ui| {
                ui.columns(3, |cols| {
                    for (i, p) in STEM_PARAMS.into_iter().enumerate() {
                        let gain = snap.targets[p.index()];
                        let lvl = snap.stem_levels[i];
                        let ui = &mut cols[i];
                        {
                            let name = ["DRUMS", "VOCALS", "MUSIC"][i];
                            let on = gain > -20.0;
                            ui.label(RichText::new(name).strong().color(if on { ACCENT } else { DIM }));
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("level").size(11.0).color(DIM));
                                meter(ui, (gain + 40.0) / 46.0, vec2(110.0, 9.0), lerp_color(CALM, ACCENT, (gain + 40.0) / 40.0));
                                ui.label(RichText::new(format!("{gain:+.0} dB")).monospace().size(11.0));
                            });
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("signal").size(11.0).color(DIM));
                                meter(ui, (lvl + 60.0) / 60.0, vec2(110.0, 5.0), Color32::from_gray(150));
                            });
                        }
                    }
                });
            });
            ui.add_space(8.0);
        }

        ui.columns(2, |cols| {
            card(&mut cols[0], "Game", |ui| {
                if !snap.game_running {
                    ui.colored_label(DIM, "Deadlock isn't running.");
                } else if !snap.live {
                    ui.colored_label(DIM, "Waiting for a match (hook reports in-game state).");
                }
                egui::Grid::new("game").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label("Health");
                    match (snap.hp, snap.hpm) {
                        (Some(h), Some(m)) if m > 0.0 => {
                            ui.horizontal(|ui| {
                                meter(ui, h / m, vec2(150.0, 10.0), lerp_color(BAD, GOOD, h / m));
                                ui.label(RichText::new(format!("{h:.0} / {m:.0}")).monospace().size(11.0));
                            });
                        }
                        _ => {
                            ui.colored_label(DIM, "—");
                        }
                    }
                    ui.end_row();
                    ui.label("Ammo");
                    ui.label(match (snap.ammo, snap.ammom) {
                        (Some(a), Some(m)) => format!("{a:.0} / {m:.0}"),
                        (Some(a), None) => format!("{a:.0}"),
                        _ => "—".into(),
                    });
                    ui.end_row();
                    ui.label("Speed");
                    ui.horizontal(|ui| {
                        let sc = self.draft.signals.get("speed");
                        let (lo, hi) = (sc.and_then(|s| s.min).unwrap_or(0.0), sc.and_then(|s| s.max).unwrap_or(700.0));
                        let v = snap.spd.unwrap_or(0.0);
                        meter(ui, (v - lo) / (hi - lo).max(1.0), vec2(150.0, 10.0), ACCENT);
                        ui.label(RichText::new(format!("{v:.0}")).monospace().size(11.0));
                    });
                    ui.end_row();
                    ui.label("Seen");
                    ui.horizontal(|ui| {
                        match snap.spd_range {
                            Some((a, b)) => {
                                ui.label(RichText::new(format!("{a:.0} – {b:.0}")).size(11.0).color(DIM));
                                if ui.small_button("use as speed range").on_hover_text("Set speed min/max to what you've actually hit").clicked() {
                                    if let Some(s) = self.draft.signals.get_mut("speed") {
                                        s.min = Some(a);
                                        s.max = Some(b);
                                    }
                                }
                            }
                            None => {
                                ui.colored_label(DIM, "—");
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("State");
                    ui.label(if snap.dead { RichText::new("dead").color(BAD) } else if snap.live { RichText::new("alive").color(GOOD) } else { RichText::new("—").color(DIM) });
                    ui.end_row();
                });
                ui.add_space(6.0);
                ui.label(RichText::new("RECENT").size(11.0).color(DIM));
                if snap.recent.is_empty() {
                    ui.colored_label(DIM, "nothing yet");
                }
                for (ago, name) in snap.recent.iter().take(8) {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(name).color(signal_color(name)));
                        ui.label(RichText::new(format!("{ago:.0}s ago")).size(11.0).color(DIM));
                    });
                }
            });
            card(&mut cols[1], "Signals now", |ui| {
                let mut active: Vec<&(String, f32)> = snap.signals.iter().filter(|(_, v)| *v > 0.01).collect();
                active.sort_by(|a, b| b.1.total_cmp(&a.1));
                if active.is_empty() {
                    ui.colored_label(DIM, "all quiet");
                }
                for (name, v) in active.iter().take(12) {
                    ui.horizontal(|ui| {
                        ui.add_sized([96.0, 14.0], egui::Label::new(RichText::new(name.as_str()).size(12.0)));
                        meter(ui, *v, vec2(ui.available_width() - 40.0, 10.0), signal_color(name));
                        ui.label(RichText::new(format!("{:.2}", v)).monospace().size(11.0));
                    });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("TRY IT").size(11.0).color(DIM));
                    match snap.demo {
                        Some(t) => {
                            ui.label(RichText::new(format!("demo playing {t:.0}/{:.0}s", crate::engine::DEMO_SECS)).color(ACCENT));
                        }
                        None => {
                            let b = ui.button(RichText::new("Play demo match").color(ACCENT));
                            if b.on_hover_text("25 s scripted fight: walk, sprint, shoot, kill, ult, die, respawn, objective").clicked() {
                                self.hub.send(Cmd::Demo);
                            }
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    for (n, l) in [
                        ("fire", "Fire"),
                        ("hit", "Hit"),
                        ("crit", "Crit"),
                        ("damage", "Damage"),
                        ("kill", "Kill"),
                        ("assist", "Assist"),
                        ("ultimate", "Ult"),
                        ("speed", "Sprint"),
                        ("energy", "Full energy"),
                        ("low_health", "Low HP"),
                        ("dead", "Dead"),
                        ("objective_win", "Objective +"),
                        ("objective_loss", "Objective −"),
                        ("midboss", "Mid-Boss"),
                        ("shop", "Shop"),
                    ] {
                        self.test_button(ui, n, l);
                    }
                });
            });
        });
    }

    fn tab_energy(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let sig = |n: &str| snap.signals.iter().find(|(k, _)| k == n).map_or(0.0, |(_, v)| *v);
        let energy = sig("energy");
        ui.label(RichText::new("Energy is how much is going on. With nothing happening the track sits at the calm state; as energy rises, the 'full energy' moves are added on top.").color(DIM));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Energy {:>3.0}%", energy * 100.0)).strong().color(lerp_color(CALM, ACCENT, energy)));
            meter(ui, energy, vec2(ui.available_width() - 120.0, 14.0), lerp_color(CALM, ACCENT, energy));
            self.test_button(ui, "energy", "Test full");
        });
        ui.add_space(8.0);
        self.stems_card(ui);
        ui.add_space(8.0);
        ui.columns(2, |cols| {
            card(&mut cols[0], "Calm state (nothing happening)", |ui| {
                egui::Grid::new("base").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    for p in PARAMS.into_iter().filter(|p| !p.is_stem()) {
                        let (lo, hi) = p.range();
                        let mut v = *self.draft.base.get(&p).unwrap_or(&p.neutral());
                        ui.label(p.label());
                        if ui.add(egui::Slider::new(&mut v, lo..=hi).fixed_decimals(2)).changed() {
                            if (v - p.neutral()).abs() < 1e-4 {
                                self.draft.base.remove(&p);
                            } else {
                                self.draft.base.insert(p, v);
                            }
                        }
                        ui.end_row();
                    }
                });
            });
            card(&mut cols[1], "Full energy adds", |ui| {
                egui::Grid::new("full").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    for p in PARAMS.into_iter().filter(|p| !p.is_stem()) {
                        let (lo, hi) = p.amount_range();
                        let (lo, hi) = if p == Param::HighpassOct || p == Param::LowpassOct { (-7.0, 7.0) } else { (lo.min(-hi), hi.max(-lo)) };
                        let idx = self.draft.mix.routes.iter().position(|r| r.signal == "energy" && r.param == p);
                        let mut v = idx.map_or(0.0, |i| self.draft.mix.routes[i].amount);
                        ui.label(p.label());
                        if ui.add(egui::Slider::new(&mut v, lo..=hi).fixed_decimals(2)).changed() {
                            match idx {
                                Some(i) if v.abs() < 1e-4 => {
                                    self.draft.mix.routes.remove(i);
                                }
                                Some(i) => self.draft.mix.routes[i].amount = v,
                                None => self.draft.mix.routes.push(Route::new("energy", p, v)),
                            }
                        }
                        ui.end_row();
                    }
                });
                ui.add_space(6.0);
                if let Some(e) = self.draft.signals.get_mut("energy") {
                    ui.horizontal(|ui| {
                        ui.label("Rise");
                        ui.add(egui::DragValue::new(&mut e.attack_ms).range(0.0..=5000.0).speed(10.0).suffix(" ms"));
                        ui.label("Fall");
                        ui.add(egui::DragValue::new(&mut e.release_ms).range(0.0..=20000.0).speed(25.0).suffix(" ms"));
                    });
                }
            });
        });
        ui.add_space(8.0);
        card(ui, "What builds energy", |ui| {
            ui.label(RichText::new("Each signal (0–1) x weight is summed and clamped to 0–1. Negative weights pull energy down.").size(11.0).color(DIM));
            let mut remove: Option<String> = None;
            egui::Grid::new("weights").num_columns(4).striped(true).spacing([12.0, 6.0]).show(ui, |ui| {
                for (name, w) in self.draft.energy.weights.iter_mut() {
                    ui.label(RichText::new(name.as_str()).color(signal_color(name)));
                    ui.add(egui::Slider::new(w, -1.0..=1.5).fixed_decimals(2));
                    let contrib = sig(name) * *w;
                    meter(ui, contrib.abs(), vec2(120.0, 8.0), if contrib < 0.0 { BAD } else { ACCENT });
                    if ui.small_button("x").clicked() {
                        remove = Some(name.clone());
                    }
                    ui.end_row();
                }
            });
            if let Some(n) = remove {
                self.draft.energy.weights.remove(&n);
            }
            let missing: Vec<String> = self.signal_names().into_iter().filter(|n| n != "energy" && !self.draft.energy.weights.contains_key(n)).collect();
            if !missing.is_empty() {
                egui::ComboBox::from_id_salt("add_weight").selected_text("+ add signal").show_ui(ui, |ui| {
                    for n in missing {
                        if ui.selectable_label(false, &n).clicked() {
                            self.draft.energy.weights.insert(n, 0.3);
                        }
                    }
                });
            }
        });
    }

    /// Each stem: a calm level, and the slice of energy over which it comes back to 0 dB.
    fn stems_card(&mut self, ui: &mut Ui) {
        card(ui, "Stems", |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.draft.stems.enabled, RichText::new("Split the track into drums / vocals / music").strong());
                ui.label(RichText::new("real-time approximation on the CPU, adds ~43 ms").size(11.0).color(DIM));
            });
            if !self.draft.stems.enabled {
                return;
            }
            ui.add_space(4.0);
            egui::Grid::new("stems").num_columns(4).spacing([14.0, 8.0]).show(ui, |ui| {
                for h in ["", "calm level", "comes in from", "fully in at"] {
                    ui.label(RichText::new(h).size(11.0).color(DIM));
                }
                ui.end_row();
                for p in STEM_PARAMS {
                    let name = match p {
                        Param::DrumsDb => "Drums",
                        Param::VocalsDb => "Vocals",
                        _ => "Music",
                    };
                    ui.label(RichText::new(name).strong());
                    let mut calm = *self.draft.base.get(&p).unwrap_or(&0.0);
                    let changed = ui.add(egui::Slider::new(&mut calm, -40.0..=0.0).suffix(" dB").fixed_decimals(0)).changed();
                    let idx = self.draft.mix.routes.iter().position(|r| r.signal == "energy" && r.param == p);
                    let (mut from, mut to) = idx.map_or((0.0, 1.0), |i| (self.draft.mix.routes[i].from, self.draft.mix.routes[i].to));
                    let f = ui.add_enabled(calm < -0.5, egui::Slider::new(&mut from, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0}% energy", v * 100.0)));
                    let t = ui.add_enabled(calm < -0.5, egui::Slider::new(&mut to, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0}% energy", v * 100.0)));
                    ui.end_row();
                    if changed || f.changed() || t.changed() {
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
            ui.label(RichText::new("Each stem sits at its calm level and rises back to full as energy passes through its range. Other routes (e.g. kill -> drums) still add on top.").size(11.0).color(DIM));
        });
    }

    fn tab_signals(&mut self, ui: &mut Ui, snap: &Snapshot) {
        ui.label(RichText::new("Pulses jump on an event and fall back; levels follow a game state. Untick to mute a signal everywhere.").color(DIM));
        ui.add_space(6.0);
        let routes_per: std::collections::BTreeMap<String, usize> =
            self.draft.mix.routes.iter().fold(Default::default(), |mut m, r| {
                *m.entry(r.signal.clone()).or_default() += 1;
                m
            });
        let names = self.signal_names();
        egui::Grid::new("signals").num_columns(9).striped(true).spacing([10.0, 6.0]).show(ui, |ui| {
            for h in ["", "signal", "now", "", "attack", "release", "strength", "scale / extra", "routes"] {
                ui.label(RichText::new(h).size(11.0).color(DIM));
            }
            ui.end_row();
            for name in &names {
                let v = snap.signals.iter().find(|(k, _)| k == name).map_or(0.0, |(_, v)| *v);
                let hub = self.hub.clone();
                let Some(s): Option<&mut SignalCfg> = self.draft.signals.get_mut(name) else { continue };
                ui.checkbox(&mut s.enabled, "");
                ui.label(RichText::new(name).color(if s.enabled { signal_color(name) } else { DIM }));
                meter(ui, v, vec2(80.0, 8.0), signal_color(name));
                if ui.small_button("test").clicked() {
                    hub.send(Cmd::Test(name.clone()));
                }
                ui.add(egui::DragValue::new(&mut s.attack_ms).range(0.0..=5000.0).speed(5.0).suffix(" ms"));
                ui.add(egui::DragValue::new(&mut s.release_ms).range(0.0..=30000.0).speed(20.0).suffix(" ms"));
                if LEVELS.contains(&name.as_str()) {
                    ui.label(RichText::new("level").size(11.0).color(DIM));
                } else {
                    ui.add(egui::DragValue::new(&mut s.strength).range(0.0..=2.0).speed(0.01));
                }
                ui.horizontal(|ui| match name.as_str() {
                    "damage" | "heal" => {
                        ui.add(egui::DragValue::new(&mut s.scale).range(0.0..=20.0).speed(0.05)).on_hover_text("hp fraction x scale");
                    }
                    "low_health" => {
                        let mut th = s.threshold.unwrap_or(0.35);
                        if ui.add(egui::DragValue::new(&mut th).range(0.05..=1.0).speed(0.01)).on_hover_text("hp fraction where it starts").changed() {
                            s.threshold = Some(th);
                        }
                    }
                    "speed" => {
                        let (mut lo, mut hi) = (s.min.unwrap_or(100.0), s.max.unwrap_or(700.0));
                        let a = ui.add(egui::DragValue::new(&mut lo).range(0.0..=2000.0).speed(2.0).prefix("min "));
                        let b = ui.add(egui::DragValue::new(&mut hi).range(1.0..=2000.0).speed(2.0).prefix("max "));
                        if a.changed() || b.changed() {
                            s.min = Some(lo);
                            s.max = Some(hi.max(lo + 1.0));
                        }
                    }
                    _ => {}
                });
                ui.label(RichText::new(routes_per.get(name).map_or("—".to_string(), |n| n.to_string())).color(DIM));
                ui.end_row();
            }
        });
    }

    fn tab_routes(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Each route adds amount x signal to a parameter (scaled by Intensity). This is the whole sound design.").color(DIM));
        ui.add_space(6.0);
        let names = self.signal_names();
        ui.horizontal(|ui| {
            ui.label("Show");
            let txt = if self.route_filter.is_empty() { "all signals".to_string() } else { self.route_filter.clone() };
            egui::ComboBox::from_id_salt("route_filter").selected_text(txt).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.route_filter, String::new(), "all signals");
                for n in &names {
                    ui.selectable_value(&mut self.route_filter, n.clone(), n);
                }
            });
            if ui.button("+ Add route").clicked() {
                let sig = if self.route_filter.is_empty() { "kill".to_string() } else { self.route_filter.clone() };
                self.draft.mix.routes.push(Route::new(&sig, Param::BassDb, 3.0));
            }
        });
        ui.add_space(6.0);
        let mut remove: Option<usize> = None;
        egui::Grid::new("routes").num_columns(5).striped(true).spacing([10.0, 6.0]).show(ui, |ui| {
            for h in ["signal", "parameter", "amount", "active over signal range", ""] {
                ui.label(RichText::new(h).size(11.0).color(DIM));
            }
            ui.end_row();
            for (i, r) in self.draft.mix.routes.iter_mut().enumerate() {
                if !self.route_filter.is_empty() && r.signal != self.route_filter {
                    continue;
                }
                signal_combo(ui, ("rs", i), &mut r.signal, &names);
                param_combo(ui, ("rp", i), &mut r.param);
                let (lo, hi) = r.param.amount_range();
                let (lo, hi) = (lo.min(r.amount), hi.max(r.amount));
                ui.add(egui::Slider::new(&mut r.amount, lo..=hi).fixed_decimals(2));
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut r.from).range(0.0..=1.0).speed(0.01).fixed_decimals(2));
                    ui.label("to");
                    ui.add(egui::DragValue::new(&mut r.to).range(0.0..=1.0).speed(0.01).fixed_decimals(2));
                })
                .response
                .on_hover_text("Below 'from' the route adds nothing; at 'to' it adds the full amount");
                if ui.small_button("x").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some(i) = remove {
            self.draft.mix.routes.remove(i);
        }
    }

    fn tab_rules(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Text rules turn HUD messages into signals. 'team' = objective banners (positive = good for you); 'toast' = game announcements. First match wins. Watch the Log tab to see what messages arrive.").color(DIM));
        ui.add_space(6.0);
        let names = self.signal_names();
        let mut remove: Option<usize> = None;
        let n = self.draft.text.rules.len();
        let mut swap: Option<(usize, usize)> = None;
        egui::Grid::new("rules").num_columns(6).striped(true).spacing([10.0, 6.0]).show(ui, |ui| {
            for h in ["source", "regex (empty = anything)", "polarity", "signal", "", ""] {
                ui.label(RichText::new(h).size(11.0).color(DIM));
            }
            ui.end_row();
            for (i, r) in self.draft.text.rules.iter_mut().enumerate() {
                egui::ComboBox::from_id_salt(("src", i)).selected_text(r.source.as_str()).width(70.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut r.source, "team".to_string(), "team");
                    ui.selectable_value(&mut r.source, "toast".to_string(), "toast");
                });
                let ok = regex::Regex::new(&r.pattern).is_ok();
                ui.add(egui::TextEdit::singleline(&mut r.pattern).desired_width(220.0).text_color(if ok { Color32::from_gray(220) } else { BAD }));
                let pol = match r.positive {
                    None => "any",
                    Some(true) => "good for you",
                    Some(false) => "bad for you",
                };
                egui::ComboBox::from_id_salt(("pol", i)).selected_text(pol).width(110.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut r.positive, None, "any");
                    ui.selectable_value(&mut r.positive, Some(true), "good for you");
                    ui.selectable_value(&mut r.positive, Some(false), "bad for you");
                });
                signal_combo(ui, ("rsig", i), &mut r.signal, &names);
                ui.horizontal(|ui| {
                    if i > 0 && ui.small_button("up").clicked() {
                        swap = Some((i, i - 1));
                    }
                    if i + 1 < n && ui.small_button("down").clicked() {
                        swap = Some((i, i + 1));
                    }
                });
                if ui.small_button("x").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some((a, b)) = swap {
            self.draft.text.rules.swap(a, b);
        }
        if let Some(i) = remove {
            self.draft.text.rules.remove(i);
        }
        if ui.button("+ Add rule").clicked() {
            self.draft.text.rules.push(RuleCfg { source: "toast".into(), pattern: "(?i)".into(), positive: None, signal: "midboss".into() });
        }
    }

    fn tab_audio(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let devices = self.devices.lock().unwrap().clone();
        card(ui, "Routing", |ui| {
            ui.horizontal(|ui| pill(ui, Some(snap.app_ok), &snap.app));
            ui.add_space(6.0);
            egui::Grid::new("audio").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Music app");
                ui.add(egui::TextEdit::singleline(&mut self.draft.audio.app).desired_width(200.0)).on_hover_text("Process name, e.g. Spotify.exe");
                ui.end_row();
                ui.label("Silent sink");
                egui::ComboBox::from_id_salt("sink").selected_text(self.draft.audio.sink.as_str()).width(320.0).show_ui(ui, |ui| {
                    for d in &devices {
                        ui.selectable_value(&mut self.draft.audio.sink, d.clone(), d);
                    }
                })
                .response
                .on_hover_text("A device you never listen to. The app is moved here while Sidechain runs.");
                ui.end_row();
                ui.label("Play processed on");
                let out_txt = if self.draft.audio.output.is_empty() { "the app's previous device".to_string() } else { self.draft.audio.output.clone() };
                egui::ComboBox::from_id_salt("output").selected_text(out_txt).width(320.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.draft.audio.output, String::new(), "the app's previous device");
                    for d in &devices {
                        if *d != self.draft.audio.sink {
                            ui.selectable_value(&mut self.draft.audio.output, d.clone(), d);
                        }
                    }
                });
                ui.end_row();
                ui.label("Latency");
                ui.add(egui::Slider::new(&mut self.draft.audio.latency_ms, 10.0..=120.0).suffix(" ms").fixed_decimals(0));
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.draft.audio.only_with_game, "Only take over the app while Deadlock is running");
                ui.end_row();
            });
            if ui.small_button("refresh device list").clicked() {
                refresh_devices(self.devices.clone());
            }
        });
        ui.add_space(8.0);
        card(ui, "Game input (restart to apply)", |ui| {
            egui::Grid::new("input").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Netconsole port");
                ui.add(egui::DragValue::new(&mut self.draft.input.netcon_port).range(0..=65535)).on_hover_text("Deadlock launch option -netconport <port>; 0 = off");
                ui.end_row();
                ui.label("console.log");
                ui.add(egui::TextEdit::singleline(&mut self.draft.input.console_log).hint_text("auto-detect (needs -condebug)").desired_width(360.0));
                ui.end_row();
            });
            ui.label(RichText::new(format!("Now reading: {} · {} hook lines", snap.input, snap.hook_lines)).size(11.0).color(DIM));
        });
        ui.add_space(8.0);
        card(ui, "Files", |ui| {
            ui.label(RichText::new(format!("Config: {}", self.hub.cfg_path.display())).size(11.0).monospace());
            ui.label(RichText::new(format!("Presets: {}", self.preset_dir.display())).size(11.0).monospace());
            ui.horizontal(|ui| {
                if ui.button("Reset everything to defaults").clicked() {
                    let mut d = Config::default_config();
                    d.audio = self.draft.audio.clone();
                    d.input = self.draft.input.clone();
                    self.draft = d;
                    self.flash("defaults loaded (Save to keep)");
                }
                if ui.button("Open config folder").clicked() {
                    if let Some(dir) = self.hub.cfg_path.parent() {
                        let _ = std::process::Command::new("explorer").arg(dir).spawn();
                    }
                }
            });
        });
    }

    fn tab_log(&mut self, ui: &mut Ui) {
        let lines = log_lines();
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
            for l in lines {
                ui.label(RichText::new(l).monospace().size(12.0));
            }
        });
    }
}

// ---------------------------------------------------------------------------- entry

pub fn run(hub: Arc<Hub>, start_hidden: bool) -> Result<()> {
    let icon = egui::IconData { rgba: icon_rgba(64), width: 64, height: 64 };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Sidechain")
        .with_inner_size([1080.0, 760.0])
        .with_min_inner_size([860.0, 560.0])
        .with_icon(Arc::new(icon));
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

#[allow(dead_code)]
fn _unused(_: StrokeKind) {}
