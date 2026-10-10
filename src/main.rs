#![windows_subsystem = "windows"]
// Sonar Lite GUI: MASTER + GAME / CHAT / MEDIA / AUX strips and MIC, in the style of SteelSeries Sonar's classic mixer,
// plus a streamer mode with a second (stream) fader per strip.
// The UI is drawn from a read-only view of the state and returns a list of `Act`s; `App::apply` performs them afterwards.
// Every change goes straight to the registry / audio policy; running apps are then refreshed so they pick it up.
use eframe::egui::{self, Color32, RichText};
use sonar_lite::{apps, autostart, eq, i18n::{self, tr, tr_fmt}, mic, mixer, settings, stream, vad, *};
use std::sync::{atomic::Ordering::Relaxed, Arc};
use std::time::{Duration, Instant};
use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};

const BG: Color32 = Color32::from_rgb(33, 37, 43);
const HEADER: Color32 = Color32::from_rgb(27, 30, 35);
const BODY: Color32 = Color32::from_rgb(44, 48, 56);
const INSET: Color32 = Color32::from_rgb(34, 37, 43);
const IDLE: Color32 = Color32::from_rgb(58, 63, 73);
const WEAK: Color32 = Color32::from_rgb(139, 146, 156);
const ACCENT: Color32 = Color32::from_rgb(255, 91, 36);
const MASTER_COL: Color32 = Color32::from_rgb(112, 112, 235);
const MIC_COL: Color32 = Color32::from_rgb(232, 165, 58);

/// key, title, icon, colour (display order; registry keys stay game/media/aux/chat)
const STRIPS: [(&str, &str, &str, Color32); 4] = [
    ("game", "GAME", "🎮", Color32::from_rgb(61, 206, 160)),
    ("chat", "CHAT", "💬", Color32::from_rgb(74, 163, 224)),
    ("media", "MEDIA", "▶", Color32::from_rgb(224, 86, 138)),
    ("aux", "AUX", "🔌", Color32::from_rgb(160, 102, 224)),
];

/// Colours of the 10 EQ bands (as in Sonar's equalizer).
const BAND_COLORS: [Color32; 10] = [
    Color32::from_rgb(150, 90, 230), Color32::from_rgb(80, 110, 235), Color32::from_rgb(236, 90, 160), Color32::from_rgb(255, 96, 96),
    Color32::from_rgb(255, 140, 66), Color32::from_rgb(255, 200, 66), Color32::from_rgb(170, 230, 60), Color32::from_rgb(40, 214, 170),
    Color32::from_rgb(60, 210, 230), Color32::from_rgb(60, 170, 255),
];

fn col(key: &str) -> Color32 { STRIPS.iter().find(|s| s.0 == key).map_or(WEAK, |s| s.3) }

/// An app with an audio session. `to` = Sonar channel it really plays on (None = a normal Windows device, named in `dev`).
struct AppRow { pid: u32, exe: String, to: Option<&'static str>, dev: String }

/// Everything the UI can ask for; performed by `App::apply` after drawing.
enum Act {
    Fader(&'static str, f32),
    Mute(&'static str, bool),
    Out(&'static str, String),
    Master(f32),
    ChatMix(f32),
    MoveApp(u32, Option<&'static str>),
    Preset(&'static str, String),
    /// (channel, name) - one of the own presets
    UserPreset(&'static str, String),
    SavePreset(String),
    MicUserPreset(String),
    MicSavePreset(String),
    /// (microphone?, name) - remove one of the own presets
    DeletePreset(bool, String),
    Bands(Vec<eq::Band>),
    EqOn(bool),
    Open(Option<&'static str>),
    MicGain(f32),
    MicMute(bool),
    MicDev(String),
    MicGate(bool),
    MicGateDb(f32),
    MicNc(bool),
    MicNcPct(f32),
    MicEq(String),
    MicOut(String),
    AllOut(String),
    OpenDev(Option<&'static str>),
    Link(bool),
    SetDevice(&'static str, String, String),
    /// drag a device (guid) of a channel: Some(i) = to position i of the active list, None = switch it off
    DevMove(&'static str, String, Option<usize>),
    MicGateAuto(bool),
    MicComp(bool),
    MicCompLevel(f32),
    MicEqOn(bool),
    MicBands(Vec<eq::Band>),
    StreamOn(bool),
    StreamDev(String),
    SFader(&'static str, f32),
    SMute(&'static str, bool),
    SMaster(f32),
    Autostart(bool),
    Refresh,
    Prefs(bool),
    Quit,
    SetLang(i18n::Lang),
    CloseToTray(bool),
    StartInTray(bool),
    OpenConfigDir,
    /// hide the "Sonar not found" popup until the next start
    HideMissing,
}

struct App {
    hklm: RegKey,
    eps: Vec<Ep>,
    chans: Vec<Chan>,
    err: Option<String>,
    mixer: mixer::Mixer,
    settings: Option<&'static str>,
    device_win: Option<&'static str>,
    sel: usize,
    bands: Vec<eq::Band>,
    eq_on: bool,
    presets: Vec<String>,
    mic_presets: Vec<String>,
    rows: Vec<AppRow>,
    /// apps currently recording from "Sonar - Microphone" (e.g. Discord), shown in the MIC strip
    mic_apps: Vec<String>,
    rows_at: Instant,
    pol: Option<apps::Policy>,
    dirty: Vec<&'static str>,
    missing_hidden: bool,
    mic_cfg: mic::Cfg,
    mic: Option<mic::MicRun>,
    mic_pending: Option<std::sync::mpsc::Receiver<Result<mic::MicRun, String>>>,
    /// (inputs, outputs) refreshed by a background thread - enumerating audio devices takes ~250 ms
    mic_inputs: Vec<String>,
    mic_outputs: Vec<String>,
    devs: Arc<std::sync::Mutex<(Vec<String>, Vec<String>)>>,
    sh: Arc<stream::Shared>,
    stream: Option<stream::StreamRun>,
    stream_pending: Option<std::sync::mpsc::Receiver<Result<stream::StreamRun, String>>>,
    obs_hint: String,
    autostart: bool,
    has_tray: bool,
    _tray: Option<tray_icon::TrayIcon>,
    tray_items: Option<(tray_icon::menu::MenuItem, tray_icon::menu::MenuItem)>,
    prefs: settings::Settings,
    prefs_open: bool,
    shot: Option<String>,
    frames: u32,
    start_hidden: bool,
}

// ---------------------------------------------------------------- helpers

/// "Słuchawki (SteelSeries Arctis Nova 5)" -> "Słuchawki"
fn short(name: &str) -> &str { name.split(" (").next().unwrap_or(name) }

/// Shorten a label so it never widens its container.
fn trunc(s: &str, n: usize) -> String { if s.chars().count() > n { format!("{}…", s.chars().take(n - 1).collect::<String>()) } else { s.to_string() } }

/// Minimal PNG encoder (stored deflate blocks) for the `--shot` diagnostic.
fn write_png(path: &str, img: &egui::ColorImage) {
    fn crc(d: &[u8]) -> u32 { let mut c = !0u32; for &b in d { c ^= b as u32; for _ in 0..8 { c = (c >> 1) ^ (0xEDB88320 & (!(c & 1)).wrapping_add(1)); } } !c }
    fn chunk(o: &mut Vec<u8>, t: &[u8; 4], d: &[u8]) { o.extend((d.len() as u32).to_be_bytes()); let mut b = t.to_vec(); b.extend(d); o.extend(&b); o.extend(crc(&b).to_be_bytes()); }
    let (w, h) = (img.size[0], img.size[1]);
    let mut raw = Vec::with_capacity((w * 4 + 1) * h);
    for y in 0..h { raw.push(0); for p in &img.pixels[y * w..(y + 1) * w] { raw.extend([p.r(), p.g(), p.b(), p.a()]); } }
    let (mut z, mut a, mut b) = (vec![0x78, 0x01], 1u32, 0u32);
    for (i, blk) in raw.chunks(65535).enumerate() {
        z.push((i == (raw.len() - 1) / 65535) as u8); z.extend((blk.len() as u16).to_le_bytes()); z.extend((!(blk.len() as u16)).to_le_bytes()); z.extend(blk);
    }
    for &x in &raw { a = (a + x as u32) % 65521; b = (b + a) % 65521; }
    z.extend(((b << 16) | a).to_be_bytes());
    let mut o = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10];
    let mut ihdr = (w as u32).to_be_bytes().to_vec(); ihdr.extend((h as u32).to_be_bytes()); ihdr.extend([8, 6, 0, 0, 0]);
    chunk(&mut o, b"IHDR", &ihdr); chunk(&mut o, b"IDAT", &z); chunk(&mut o, b"IEND", &[]);
    let _ = std::fs::write(path, o);
}

fn icon_rgba() -> (Vec<u8>, u32) {
    let n = 32u32;
    let mut px = vec![0u8; (n * n * 4) as usize];
    for y in 0..n {
        for x in 0..n {
            // flat white glyph like the other Windows tray icons: a dot with three radio-wave arcs on each side, anti-aliased
            let (fx, fy) = (x as f32 + 0.5 - 16.0, y as f32 + 0.5 - 16.0);
            let (ax, r) = (fx.abs(), (fx * fx + fy * fy).sqrt()); // both sides are drawn from the right one (symmetric)
            let span = 42f32.to_radians();
            let arc = |big: f32| {
                let d = if fy.atan2(ax).abs() <= span { (r - big).abs() } else { // beyond the arc's end: distance to the rounded cap
                    let (ex, ey) = (big * span.cos(), big * span.sin() * fy.signum());
                    ((ax - ex).powi(2) + (fy - ey).powi(2)).sqrt()
                };
                (0.5 - (d - 1.2)).clamp(0.0, 1.0)
            };
            let cover = [6.5f32, 10.5, 14.5].into_iter().map(arc).fold((0.5 - (r - 2.4)).clamp(0.0, 1.0), f32::max);
            let i = ((y * n + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[255, 255, 255, (cover * 255.0) as u8]);
        }
    }
    (px, n)
}

/// Set once the language list has been opened: its entries are in several scripts, so all CJK fonts are loaded from then on.
static ALL_FONTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// egui's built-in fonts have no CJK glyphs. For Chinese / Japanese / Korean (and once the language list was opened) the Windows
/// system fonts are added as fallbacks. Reloads only when the language or that need changes.
fn sync_fonts(ctx: &egui::Context) {
    use std::sync::atomic::AtomicU16;
    static DONE: AtomicU16 = AtomicU16::new(u16::MAX);
    let (lang, all) = (i18n::lang(), ALL_FONTS.load(Relaxed));
    let key = lang as u16 | (all as u16) << 8;
    if DONE.swap(key, Relaxed) == key { return; }
    let mut order: Vec<&str> = match lang { i18n::Lang::Zh => vec!["msyh.ttc"], i18n::Lang::Ja => vec!["YuGothM.ttc"], i18n::Lang::Ko => vec!["malgun.ttf"], _ => vec![] };
    if all {
        let more: Vec<&str> = ["msyh.ttc", "YuGothM.ttc", "malgun.ttf"].into_iter().filter(|f| !order.contains(f)).collect();
        order.extend(more);
    }
    let dir = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:/Windows".into())).join("Fonts");
    let mut fonts = egui::FontDefinitions::default();
    for (i, f) in order.iter().enumerate() {
        let Ok(bytes) = std::fs::read(dir.join(f)) else { continue };
        let name = format!("cjk{i}");
        fonts.font_data.insert(name.clone(), egui::FontData::from_owned(bytes).tweak(egui::FontTweak { y_offset_factor: 0.12, ..Default::default() })); // CJK glyphs sit high next to Latin ones
        for fam in fonts.families.values_mut() { fam.push(name.clone()); }
    }
    ctx.set_fonts(fonts);
}

fn style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = BODY;
    v.extreme_bg_color = INSET;
    v.selection.bg_fill = ACCENT;
    v.selection.stroke.color = Color32::WHITE;
    v.widgets.noninteractive.bg_fill = BODY;
    v.widgets.inactive.bg_fill = IDLE;
    v.widgets.inactive.weak_bg_fill = IDLE;
    v.widgets.hovered.bg_fill = Color32::from_rgb(78, 84, 96);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(78, 84, 96);
    v.widgets.active.bg_fill = ACCENT;
    v.widgets.active.weak_bg_fill = ACCENT;
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] { w.rounding = 5.0.into(); }
    ctx.set_visuals(v);
    ctx.style_mut(|s| { s.spacing.item_spacing = egui::vec2(8.0, 6.0); s.spacing.button_padding = egui::vec2(10.0, 4.0); });
}

/// Small headphones icon centred on `c` (arc over two ear cups).
fn headphones(p: &egui::Painter, c: egui::Pos2, col: Color32) {
    let pts = (0..=12).map(|i| { let a = std::f32::consts::PI * (1.0 + i as f32 / 12.0); c + egui::vec2(5.0 * a.cos(), 1.0 + 5.0 * a.sin()) }).collect();
    p.add(egui::Shape::line(pts, egui::Stroke::new(1.5f32, col)));
    for sx in [-1.0f32, 1.0] { p.rect_filled(egui::Rect::from_center_size(c + egui::vec2(5.0 * sx, 3.5), egui::vec2(3.0, 5.0)), 1.0, col); }
}

/// Full-width app chip: headphones icon + name.
fn app_chip(ui: &mut egui::Ui, label: &str, fill: Color32) -> egui::Response {
    let ink = Color32::from_rgb(20, 22, 26);
    let r = ui.add(egui::Button::new(RichText::new(format!("      {label}")).size(12.0).color(ink)).fill(fill).min_size(egui::vec2(ui.available_width(), 22.0)));
    headphones(ui.painter(), r.rect.left_center() + egui::vec2(15.0, 0.0), ink);
    r
}

/// Square icon button for the tab bar: 0 refresh (circular arrow), 1 settings (gear).
fn icon_btn(ui: &mut egui::Ui, kind: u8, tip: &str) -> bool {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(30.0, 26.0), egui::Sense::click());
    let hov = r.hovered();
    if hov { ui.painter().rect_filled(rect, 4.0, IDLE); }
    let col = if hov { Color32::WHITE } else { WEAK };
    let (c, p) = (rect.center(), ui.painter());
    if kind == 1 {
        p.text(c, egui::Align2::CENTER_CENTER, "⚙", egui::FontId::proportional(17.0), col);
    } else {
        let at = |a: f32| c + egui::vec2(6.0 * a.cos(), 6.0 * a.sin());
        let (a0, a1) = (0.9f32, 5.9f32); // clockwise arc leaving a gap at the right, arrow head at its end
        p.add(egui::Shape::line((0..=20).map(|i| at(a0 + (a1 - a0) * i as f32 / 20.0)).collect(), egui::Stroke::new(1.6f32, col)));
        let (e, t, n) = (at(a1), egui::vec2(-a1.sin(), a1.cos()), egui::vec2(a1.cos(), a1.sin()));
        p.add(egui::Shape::convex_polygon(vec![e + t * 3.5, e - t * 1.5 + n * 3.5, e - t * 1.5 - n * 3.5], col, egui::Stroke::NONE));
    }
    r.on_hover_text(tip).clicked()
}

/// Title-bar window button (0 minimise, 1 maximise / restore, 2 close), icons painted by hand (the fonts lack the glyphs).
fn win_btn(ui: &mut egui::Ui, kind: u8, maxed: bool) -> bool {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(46.0, 36.0), egui::Sense::click());
    let hov = r.hovered();
    if hov { ui.painter().rect_filled(rect, 0.0, if kind == 2 { Color32::from_rgb(196, 43, 28) } else { IDLE }); }
    let s = egui::Stroke::new(1.2f32, if hov { Color32::WHITE } else { WEAK });
    let (m, p) = (rect.center(), ui.painter());
    match kind {
        0 => { p.hline(m.x - 5.0..=m.x + 5.0, m.y, s); }
        1 if maxed => {
            let f = egui::Rect::from_center_size(m + egui::vec2(-1.0, 1.0), egui::vec2(8.0, 8.0));
            p.rect_stroke(f, 0.0, s);
            let (a, b) = (f.min + egui::vec2(2.0, 0.0), f.max + egui::vec2(2.0, -2.0));
            p.add(egui::Shape::line(vec![a, egui::pos2(a.x, a.y - 2.0), egui::pos2(b.x, a.y - 2.0), b, egui::pos2(f.max.x, b.y)], s));
        }
        1 => { p.rect_stroke(egui::Rect::from_center_size(m, egui::vec2(10.0, 10.0)), 0.0, s); }
        _ => { p.line_segment([m + egui::vec2(-5.0, -5.0), m + egui::vec2(5.0, 5.0)], s); p.line_segment([m + egui::vec2(-5.0, 5.0), m + egui::vec2(5.0, -5.0)], s); }
    }
    r.clicked()
}

/// Live level drawn on a fader track. `level` is a linear peak; `post_fader` = the fader still scales it (channels), false = already
/// final (mic). `thr_db` = noise-gate threshold marker, `open` = gate open (a closed gate greys the meter out).
struct Meter { level: f32, thr_db: Option<f32>, open: bool, post_fader: bool }

/// Vertical fader like Sonar's: the track is the level meter (teal / orange / red zones, dim = the fader's range, bright = live
/// level), light handle with a dash. `v` in 0..1.
fn fader(ui: &mut egui::Ui, v: f32, color: Color32, reset: f32, meter: Option<&Meter>) -> Option<f32> {
    let _ = color; // the strip colour is carried by the header; the track shows level zones
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 250.0), egui::Sense::click_and_drag());
    let hh = 26.0;
    let (top, bot, cx) = (rect.top() + hh / 2.0, rect.bottom() - hh / 2.0, rect.center().x);
    let mut out = None;
    if resp.double_clicked() { out = Some(reset); }
    else if resp.dragged() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() { out = Some(((bot - p.y) / (bot - top)).clamp(0.0, 1.0)); }
    }
    let shown = out.unwrap_or(v);
    let y = bot - (bot - top) * shown;
    let p = ui.painter();
    p.rect_filled(egui::Rect::from_min_max(egui::pos2(cx - 4.0, top), egui::pos2(cx + 4.0, bot)), 4.0, Color32::from_rgb(22, 24, 28));
    // zones as fractions of the track height: teal below, orange, red at the top
    let at = |f: f32| bot - (bot - top) * f;
    let lit = meter.map(|m| ((20.0 * (m.level * if m.post_fader { shown } else { 1.0 }).max(1e-4).log10() + 60.0) / 60.0).clamp(0.0, 1.0));
    // the fader's range below the handle: one plain green bar (a single shape, so no seams); zones only on the live level
    p.rect_filled(egui::Rect::from_min_max(egui::pos2(cx - 4.0, y), egui::pos2(cx + 4.0, bot)), 4.0, Color32::from_rgb(28, 190, 156).gamma_multiply(0.6));
    for (a, b, c) in [(0.0f32, 0.72f32, Color32::from_rgb(28, 190, 156)), (0.72, 0.88, Color32::from_rgb(244, 160, 48)), (0.88, 1.0, Color32::from_rgb(232, 84, 76))] {
        let zone = |hi: f32, col: Color32| if hi > a { p.rect_filled(egui::Rect::from_min_max(egui::pos2(cx - 4.0, at(hi.min(b))), egui::pos2(cx + 4.0, at(a))), if a == 0.0 { 4.0 } else { 0.0 }, col); };
        if let (Some(f), Some(m)) = (lit, meter) { zone(if m.post_fader { f.min(shown) } else { f }, if m.open { c } else { WEAK }); }
    }
    if let Some(t) = meter.and_then(|m| m.thr_db) {
        let ty = at(((t + 60.0) / 60.0).clamp(0.0, 1.0));
        p.line_segment([egui::pos2(cx - 9.0, ty), egui::pos2(cx + 9.0, ty)], (2.0, Color32::WHITE));
    }
    let h = egui::Rect::from_center_size(egui::pos2(cx, y), egui::vec2(26.0, hh));
    p.rect_filled(h, 4.0, if resp.hovered() || resp.dragged() { Color32::from_rgb(222, 225, 236) } else { Color32::from_rgb(196, 200, 214) });
    p.line_segment([egui::pos2(cx - 6.0, y), egui::pos2(cx + 6.0, y)], (2.5, Color32::from_rgb(72, 82, 160)));
    if resp.dragged() || resp.hovered() {
        p.text(egui::pos2(rect.right() - 2.0, y), egui::Align2::RIGHT_CENTER, format!("{:.0}%", shown * 100.0), egui::FontId::proportional(12.0), Color32::WHITE);
    }
    out
}

/// Strip header: icon and coloured title (Sonar's gear only configures shortcuts, which we do not have).
fn header(ui: &mut egui::Ui, icon: &str, title: &str, color: Color32) {
    egui::Frame::none().fill(HEADER).rounding(egui::Rounding { nw: 8.0, ne: 8.0, sw: 0.0, se: 0.0 }).inner_margin(egui::Margin::symmetric(10.0, 10.0)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon).size(22.0).color(color));
            ui.label(RichText::new(title).strong().size(16.0).color(color));
        });
    });
}

/// Sonar-style preset row: blue EQ badge + preset name. Click opens a dropdown (see `preset_dropdown`).
fn preset_row(ui: &mut egui::Ui, name: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 28.0), egui::Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 4.0, if resp.hovered() { Color32::from_rgb(34, 38, 45) } else { HEADER });
    let badge = egui::Rect::from_center_size(egui::pos2(rect.left() + 16.0, rect.center().y), egui::vec2(20.0, 20.0));
    p.rect_filled(badge, 4.0, Color32::from_rgb(76, 94, 224));
    p.text(badge.center(), egui::Align2::CENTER_CENTER, "EQ", egui::FontId::proportional(9.0), Color32::WHITE);
    p.text(egui::pos2(rect.center().x + 12.0, rect.center().y), egui::Align2::CENTER_CENTER, trunc(&tr(name), 15), egui::FontId::proportional(13.0), Color32::from_rgb(230, 232, 238));
    resp
}

/// Quick preset switch under a preset row: built-in presets, own presets, and a link to the full EQ page. Returns the chosen
/// (builtin, own, open page) so the caller can turn it into actions for a channel or for the microphone.
fn preset_dropdown(ui: &mut egui::Ui, id: egui::Id, row: &egui::Response, cur: &str, builtins: Vec<&'static str>, custom: &[String]) -> (Option<String>, Option<String>, bool) {
    let (mut b, mut c, mut open) = (None, None, false);
    popup_menu(ui, id, row, |ui| {
        for n in builtins { if ui.selectable_label(n == cur, n).clicked() { b = Some(n.to_string()); } }
        if !custom.is_empty() { ui.separator(); }
        for n in custom { if ui.selectable_label(n == cur, n).clicked() { c = Some(n.clone()); } }
        ui.separator();
        if ui.button(tr("Edytuj EQ…")).clicked() { open = true; }
    });
    (b, c, open)
}

/// Sonar-style device row: link icon, device name, percentage. Returns (name response, link response).
fn device_row(ui: &mut egui::Ui, salt: &str, linked: bool, name: &str, pct: f32) -> (egui::Response, egui::Response) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::hover());
    let link_rect = egui::Rect::from_min_size(rect.min, egui::vec2(24.0, 24.0));
    let name_rect = egui::Rect::from_min_max(egui::pos2(rect.left() + 24.0, rect.top()), rect.max);
    let id = ui.id().with(("devrow", salt));
    let l = ui.interact(link_rect, id.with("l"), egui::Sense::click()).on_hover_text(tr("Połącz wszystkie kanały na jednym urządzeniu"));
    let n = ui.interact(name_rect, id.with("n"), egui::Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 4.0, if n.hovered() { Color32::from_rgb(34, 38, 45) } else { HEADER });
    // chain icon: two interlocked rounded links
    let lc = if linked { Color32::from_rgb(40, 214, 170) } else { Color32::from_rgb(90, 96, 108) };
    let c = link_rect.center();
    for dx in [-3.5f32, 3.5] {
        p.rect_stroke(egui::Rect::from_center_size(egui::pos2(c.x + dx, c.y), egui::vec2(9.0, 6.0)), 3.0, egui::Stroke::new(1.6f32, lc));
    }
    p.text(egui::pos2(name_rect.left() + 2.0, rect.center().y), egui::Align2::LEFT_CENTER, trunc(name, 17), egui::FontId::proportional(12.0), Color32::from_rgb(190, 196, 208));
    p.text(egui::pos2(rect.right() - 6.0, rect.center().y), egui::Align2::RIGHT_CENTER, format!("{pct:.0}%"), egui::FontId::proportional(10.0), WEAK);
    (n, l)
}

/// Toggle switch like Sonar's section headers.
fn switch(ui: &mut egui::Ui, on: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(40.0, 20.0), egui::Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 10.0, if on { ACCENT } else { Color32::from_rgb(70, 76, 88) });
    let x = if on { rect.right() - 10.0 } else { rect.left() + 10.0 };
    p.circle_filled(egui::pos2(x, rect.center().y), 7.5, Color32::WHITE);
    resp.clicked()
}

/// A settings card with a header row (optional switch + title). `on` = None for cards without a switch.
fn card_header(ui: &mut egui::Ui, title: impl AsRef<str>, on: Option<bool>) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        if let Some(o) = on { clicked = switch(ui, o); }
        ui.label(RichText::new(title.as_ref()).strong().size(14.0));
    });
    ui.separator();
    clicked
}

fn body<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::none().fill(BODY).rounding(egui::Rounding { nw: 0.0, ne: 0.0, sw: 8.0, se: 8.0 }).inner_margin(egui::Margin::same(8.0)).show(ui, |ui| { ui.set_width(ui.available_width()); add(ui) }).inner
}

fn mute_button(ui: &mut egui::Ui, muted: bool) -> bool {
    ui.vertical_centered(|ui| ui.add(egui::Button::new(RichText::new(if muted { "🔇" } else { "🔊" }).size(15.0)).fill(if muted { ACCENT } else { INSET }).min_size(egui::vec2(34.0, 28.0))).clicked()).inner
}

// ---------------------------------------------------------------- equalizer page (shared by the channels and the microphone)

static EQ_SEL_DEBUG: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// What the user did in an EQ section this frame.
#[derive(Default)]
struct EqOut { bands: bool, builtin: Option<String>, custom: Option<String>, save: Option<String>, delete: Option<String> }

/// Header of the equalizer card: switch, title and a menu. Returns (switch clicked, reset chosen).
fn eq_head(ui: &mut egui::Ui, on: bool) -> (bool, bool) {
    let (mut tg, mut reset) = (false, false);
    ui.horizontal(|ui| {
        tg = switch(ui, on);
        ui.label(RichText::new("EQUALIZER").strong().size(14.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (rect, r) = ui.allocate_exact_size(egui::vec2(30.0, 26.0), egui::Sense::click());
            if r.hovered() { ui.painter().rect_filled(rect, 4.0, IDLE); }
            for dy in [-5.0f32, 0.0, 5.0] { ui.painter().circle_filled(rect.center() + egui::vec2(0.0, dy), 1.7, WEAK); }
            popup_menu(ui, ui.id().with("eqmenu"), &r, |ui| { if ui.button(tr("Resetuj EQ (Flat)")).clicked() { reset = true; } });
        });
    });
    ui.separator();
    (tg, reset)
}

/// The "PRESET" card, the browse button and the 10 slots for own presets.
fn preset_bar(ui: &mut egui::Ui, salt: &str, cur: &str, builtins: &[&'static str], custom: &[String], out: &mut EqOut) {
    let id = ui.id().with(("presetbar", salt));
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.vertical(|ui| {
            ui.label(RichText::new("PRESET").size(11.0).color(Color32::from_rgb(40, 214, 170)));
            let (rect, _) = ui.allocate_exact_size(egui::vec2(300.0, 76.0), egui::Sense::hover());
            let p = ui.painter();
            p.rect_filled(rect, 6.0, Color32::from_rgb(38, 42, 50));
            let badge = egui::Rect::from_min_size(rect.min, egui::vec2(76.0, 76.0));
            p.rect_filled(badge, 6.0, Color32::from_rgb(76, 94, 224));
            p.text(badge.center(), egui::Align2::CENTER_CENTER, "EQ", egui::FontId::proportional(24.0), Color32::WHITE);
            p.text(egui::pos2(badge.right() + 14.0, rect.center().y), egui::Align2::LEFT_CENTER, trunc(&tr(cur), 16), egui::FontId::proportional(17.0), Color32::from_rgb(230, 232, 238));
        });
        ui.vertical(|ui| {
            ui.add_space(18.0);
            let (rect, r) = ui.allocate_exact_size(egui::vec2(76.0, 76.0), egui::Sense::click());
            let p = ui.painter();
            p.rect_filled(rect, 6.0, if r.hovered() { Color32::from_rgb(38, 42, 50) } else { INSET });
            p.rect_stroke(rect, 6.0, egui::Stroke::new(1.0f32, IDLE));
            let (c, s) = (rect.center(), egui::Stroke::new(2.5f32, Color32::WHITE));
            p.circle_stroke(c + egui::vec2(-4.0, -4.0), 11.0, s);
            p.line_segment([c + egui::vec2(4.0, 4.0), c + egui::vec2(15.0, 15.0)], s);
            let r = r.on_hover_text(tr("Wszystkie presety"));
            popup_menu(ui, id.with("browse"), &r, |ui| {
                ui.label(RichText::new(tr("Wbudowane")).color(WEAK).size(11.0));
                for n in builtins { if ui.selectable_label(*n == cur, *n).clicked() { out.builtin = Some(n.to_string()); } }
                if !custom.is_empty() { ui.separator(); ui.label(RichText::new(tr("Moje")).color(WEAK).size(11.0)); }
                for n in custom { if ui.selectable_label(n == cur, n).clicked() { out.custom = Some(n.clone()); } }
            });
        });
        ui.vertical(|ui| {
            ui.label(RichText::new(format!("{} ({}/{})", tr("MOJE PRESETY"), custom.len(), eq::MAX_PRESETS)).size(11.0).color(WEAK));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let w = ((ui.available_width() - 6.0 * (eq::MAX_PRESETS as f32 - 1.0)) / eq::MAX_PRESETS as f32).clamp(40.0, 76.0);
                for slot in 0..eq::MAX_PRESETS {
                    let (rect, r) = ui.allocate_exact_size(egui::vec2(w, 76.0), egui::Sense::click());
                    let p = ui.painter();
                    if let Some(name) = custom.get(slot) {
                        p.rect_filled(rect, 6.0, if r.hovered() { Color32::from_rgb(70, 76, 88) } else { IDLE });
                        if name == cur { p.rect_stroke(rect, 6.0, egui::Stroke::new(1.5f32, ACCENT)); }
                        p.text(rect.center(), egui::Align2::CENTER_CENTER, trunc(name, 9), egui::FontId::proportional(11.0), Color32::WHITE);
                        if r.clicked() { out.custom = Some(name.clone()); }
                        let r = r.on_hover_text(name);
                        r.context_menu(|ui| { if ui.button(tr("Usuń")).clicked() { out.delete = Some(name.clone()); ui.close_menu(); } });
                    } else {
                        p.rect_filled(rect, 6.0, if r.hovered() { Color32::from_rgb(34, 38, 45) } else { INSET });
                        p.rect_stroke(rect, 6.0, egui::Stroke::new(1.0f32, IDLE));
                        if slot == custom.len() { p.text(rect.center(), egui::Align2::CENTER_CENTER, "+", egui::FontId::proportional(20.0), WEAK); }
                        let r = r.on_hover_text(tr("Zapisz bieżący EQ jako preset"));
                        let pid = id.with(("save", slot));
                        if r.clicked() { ui.memory_mut(|m| m.toggle_popup(pid)); }
                        egui::popup_below_widget(ui, pid, &r, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                            ui.set_min_width(180.0);
                            let nid = pid.with("name");
                            let mut name: String = ui.data(|d| d.get_temp(nid)).unwrap_or_default();
                            let te = ui.add(egui::TextEdit::singleline(&mut name).hint_text(tr("nazwa presetu")).desired_width(170.0));
                            ui.data_mut(|d| d.insert_temp(nid, name.clone()));
                            let go = ui.button(tr("Zapisz")).clicked() || (te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                            if go && !name.trim().is_empty() {
                                out.save = Some(name.trim().to_string());
                                ui.data_mut(|d| d.insert_temp(nid, String::new()));
                                ui.memory_mut(|m| m.close_popup());
                            }
                        });
                    }
                }
            });
        });
    });
}

/// Bass / Voice / Treble sliders: they drive bands 6-8 (low shelf, mid peak, high shelf), so they show in the curve and in presets.
fn tone_sliders(ui: &mut egui::Ui, bands: &mut [eq::Band]) -> bool {
    let mut changed = false;
    ui.columns(3, |cols| {
        for (k, name) in ["Bass", "Voice", "Treble"].into_iter().enumerate() {
            let ui = &mut cols[k];
            let (kind, freq, q) = eq::TONE[k];
            let b = &mut bands[eq::MARKERS + k];
            let mut g = b.gain;
            ui.horizontal(|ui| {
                ui.label(RichText::new(name).color(WEAK).size(12.0));
                ui.spacing_mut().slider_width = (ui.available_width() - 110.0).max(60.0);
                let s = ui.add(egui::Slider::new(&mut g, -12.0..=12.0).step_by(0.5).show_value(false));
                let d = ui.add(egui::DragValue::new(&mut g).range(-12.0..=12.0).speed(0.1).fixed_decimals(1).suffix(" dB"));
                if s.changed() || d.changed() { *b = eq::Band { on: true, kind, freq, gain: g, q }; changed = true; }
            });
        }
    });
    changed
}

/// Sonar-style EQ graph: frequency ranges on top, dB / Hz axes, one marker per band. Click or drag a marker for a popup with exact values.
fn eq_graph(ui: &mut egui::Ui, salt: &str, bands: &mut [eq::Band], on: bool, h: f32) -> bool {
    let mut changed = false;
    let sid = ui.id().with(("eqsel", salt));
    let mut sel: Option<usize> = ui.data(|d| d.get_temp::<Option<usize>>(sid)).flatten();
    let dbg = EQ_SEL_DEBUG.swap(-1, Relaxed); // `--eqsel N` (screenshots): start with marker N selected
    if dbg >= 0 { sel = Some(dbg as usize); }
    let (left, top, bot) = (62.0, 32.0, 26.0);
    let (outer, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h + top + bot), egui::Sense::hover());
    let rect = egui::Rect::from_min_max(egui::pos2(outer.left() + left, outer.top() + top), egui::pos2(outer.right(), outer.top() + top + h));
    let (lo, hi) = (16.5f32.ln(), 24000f32.ln());
    let x = |f: f32| rect.left() + rect.width() * ((f.ln() - lo) / (hi - lo)).clamp(0.0, 1.0);
    let y = |db: f32| rect.bottom() - rect.height() * ((db + 14.0) / 28.0).clamp(0.0, 1.0);
    let p = ui.painter().clone();
    let (grid, grid2) = (Color32::from_rgb(36, 41, 48), Color32::from_rgb(48, 54, 63));
    // frequency ranges
    let edges = [16.5f32, 60.0, 250.0, 500.0, 2000.0, 6000.0, 24000.0];
    for (i, name) in ["SUB BASS", "BASS", "LOW MIDS", "MID RANGE", "UPPER MIDS", "HIGHS"].into_iter().enumerate() {
        let r = egui::Rect::from_min_max(egui::pos2(x(edges[i]) + 1.0, outer.top()), egui::pos2(x(edges[i + 1]) - 1.0, outer.top() + top - 6.0));
        p.rect_filled(r, 3.0, Color32::from_rgb(33, 37, 44));
        p.text(r.center(), egui::Align2::CENTER_CENTER, name, egui::FontId::proportional(11.0), WEAK);
    }
    p.rect_filled(rect, 0.0, Color32::from_rgb(27, 31, 37));
    for d in [10.0f32, 100.0, 1000.0, 10000.0] {
        for m in 1..10 { let f = d * m as f32; if (17.0..=23000.0).contains(&f) { p.vline(x(f), rect.y_range(), egui::Stroke::new(1.0f32, grid)); } }
    }
    for (f, t) in [(20.0, "20Hz"), (50.0, "50Hz"), (100.0, "100Hz"), (200.0, "200Hz"), (500.0, "500Hz"), (1000.0, "1kHz"), (2000.0, "2kHz"), (5000.0, "5kHz"), (10000.0, "10kHz"), (20000.0, "20kHz")] {
        p.vline(x(f), rect.y_range(), egui::Stroke::new(1.0f32, grid2));
        p.text(egui::pos2(x(f), rect.bottom() + 7.0), egui::Align2::CENTER_TOP, t, egui::FontId::proportional(11.0), WEAK);
    }
    for db in [-12.0f32, -6.0, 0.0, 6.0, 12.0] {
        p.hline(rect.x_range(), y(db), egui::Stroke::new(1.0f32, grid2));
        p.text(egui::pos2(rect.left() - 10.0, y(db)), egui::Align2::RIGHT_CENTER, if db == 0.0 { "0 dB".to_string() } else { format!("{db:+} dB") }, egui::FontId::proportional(11.0), WEAK);
    }
    let curve = |bs: &[eq::Band]| -> Vec<egui::Pos2> {
        (0..=300).map(|i| { let f = (lo + (hi - lo) * i as f32 / 300.0).exp(); egui::pos2(x(f), y(if on { eq::response_db(bs, f).clamp(-14.0, 14.0) } else { 0.0 })) }).collect()
    };
    if let Some(i) = sel.filter(|&i| i < eq::MARKERS && bands[i].on && on) {
        p.add(egui::Shape::line(curve(&bands[i..=i]), egui::Stroke::new(1.5f32, BAND_COLORS[i])));
    }
    p.add(egui::Shape::line(curve(bands), egui::Stroke::new(1.5f32, if on { Color32::from_rgb(226, 229, 236) } else { WEAK })));
    // click on the empty graph = close the popup (markers are registered after this, so they win)
    if ui.interact(rect, ui.id().with(("eqbg", salt)), egui::Sense::click()).clicked() { sel = None; }
    for (i, b) in bands.iter_mut().take(eq::MARKERS).enumerate() {
        let c = egui::pos2(x(b.freq), y(b.gain));
        let r = ui.interact(egui::Rect::from_center_size(c, egui::vec2(22.0, 22.0)), ui.id().with(("band", salt, i)), egui::Sense::click_and_drag());
        if r.clicked() || r.drag_started() { sel = Some(i); }
        if r.dragged() {
            let m = r.interact_pointer_pos().unwrap_or(c);
            b.freq = (lo + (hi - lo) * ((m.x - rect.left()) / rect.width()).clamp(0.0, 1.0)).exp().clamp(20.0, 20000.0).round();
            b.gain = (((rect.bottom() - m.y) / rect.height() * 28.0 - 14.0).clamp(-12.0, 12.0) * 2.0).round() / 2.0;
            changed = true;
        }
        let active = r.hovered() || r.dragged() || sel == Some(i);
        let col = if b.on { BAND_COLORS[i] } else { WEAK };
        p.circle(egui::pos2(x(b.freq), y(b.gain)), if active { 7.5 } else { 5.5 }, if b.on { col } else { Color32::from_rgb(27, 31, 37) }, egui::Stroke::new(2.0f32, if active { Color32::WHITE } else { col }));
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) { sel = None; }
    if let Some(i) = sel.filter(|&i| i < eq::MARKERS) {
        let c = egui::pos2(x(bands[i].freq), y(bands[i].gain));
        let pw = 300.0;
        let cx = if rect.width() > pw { c.x.clamp(rect.left() + pw / 2.0, rect.right() - pw / 2.0) } else { c.x };
        let (pos, pivot) = if c.y - 130.0 < outer.top() { (egui::pos2(cx, c.y + 16.0), egui::Align2::CENTER_TOP) } else { (egui::pos2(cx, c.y - 16.0), egui::Align2::CENTER_BOTTOM) };
        egui::Area::new(ui.id().with(("eqpop", salt))).order(egui::Order::Foreground).fixed_pos(pos).pivot(pivot).show(ui.ctx(), |ui| {
            egui::Frame::none().fill(Color32::from_rgb(30, 34, 41)).stroke(egui::Stroke::new(1.0f32, IDLE)).rounding(8.0).inner_margin(egui::Margin::same(10.0)).show(ui, |ui| {
                ui.set_width(pw - 20.0);
                let b = &mut bands[i];
                ui.horizontal(|ui| {
                    let (dr, dresp) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
                    ui.painter().circle(dr.center(), 8.5, if b.on { BAND_COLORS[i] } else { Color32::TRANSPARENT }, egui::Stroke::new(2.0f32, BAND_COLORS[i]));
                    if dresp.clicked() { b.on = !b.on; changed = true; }
                    ui.label(RichText::new(tr("Włączony")).size(13.0));
                    ui.add_space(12.0);
                    let kind = eq::KINDS.iter().find(|k| k.0 == b.kind).map_or(format!("{} {}", tr("typ"), b.kind), |k| format!("{} EQ", k.1));
                    egui::ComboBox::from_id_salt(("eqkind", salt)).selected_text(kind).width(120.0).show_ui(ui, |ui| {
                        for (id, name) in eq::KINDS { changed |= ui.selectable_value(&mut b.kind, id, format!("{name} EQ")).changed(); }
                    });
                });
                ui.add_space(8.0);
                ui.columns(3, |c| {
                    let cap = |ui: &mut egui::Ui, t: &str| ui.label(RichText::new(t).size(11.0).color(WEAK));
                    c[0].vertical_centered(|ui| { changed |= ui.add(egui::DragValue::new(&mut b.gain).range(-12.0..=12.0).speed(0.1).fixed_decimals(1).suffix(" dB")).changed(); cap(ui, &tr("Wzm.")); });
                    c[1].vertical_centered(|ui| {
                        changed |= ui.add(egui::DragValue::new(&mut b.freq).range(20.0..=20000.0).speed(2.0)
                            .custom_formatter(|v, _| if v >= 1000.0 { format!("{:.2} kHz", v / 1000.0) } else { format!("{v:.0} Hz") })
                            .custom_parser(|s| { let s = s.trim().to_lowercase(); let k = s.contains('k'); let n: f64 = s.trim_end_matches("hz").trim_end_matches('k').trim().parse().ok()?; Some(if k { n * 1000.0 } else { n }) })).changed();
                        cap(ui, &tr("Częst."));
                    });
                    c[2].vertical_centered(|ui| { changed |= ui.add(egui::DragValue::new(&mut b.q).range(0.1..=10.0).speed(0.01).fixed_decimals(3)).changed(); cap(ui, "Q"); });
                });
            });
        });
    }
    ui.data_mut(|d| d.insert_temp(sid, sel));
    changed
}

/// Whole EQ body: preset bar, graph and tone sliders. `bands` must hold 10 bands.
fn eq_section(ui: &mut egui::Ui, salt: &str, bands: &mut Vec<eq::Band>, on: bool, cur: &str, builtins: Vec<&'static str>, custom: &[String]) -> EqOut {
    let mut out = EqOut::default();
    if let Some(flat) = eq::builtin("Flat") { for i in bands.len()..10 { bands.push(flat[i]); } }
    preset_bar(ui, salt, cur, &builtins, custom, &mut out);
    ui.add_space(10.0);
    out.bands |= eq_graph(ui, salt, bands, on, 300.0);
    ui.add_space(8.0);
    out.bands |= tone_sliders(ui, bands);
    out
}

/// Generic clickable label that opens a popup below it.
fn popup_menu(ui: &mut egui::Ui, id: egui::Id, resp: &egui::Response, items: impl FnOnce(&mut egui::Ui)) {
    if resp.clicked() { ui.memory_mut(|m| m.toggle_popup(id)); }
    egui::popup_below_widget(ui, id, resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
        ui.set_min_width(160.0);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // long device names widen the popup instead of wrapping
        items(ui);
    });
}

/// Box that apps can be dropped into (`here` = channel key, None = leave to Windows). Returns the dropped pid.
fn drop_zone(ui: &mut egui::Ui, h: f32, add: impl FnOnce(&mut egui::Ui)) -> Option<u32> {
    let frame = egui::Frame::none().fill(INSET).rounding(5.0).inner_margin(egui::Margin::same(6.0));
    let (_, dropped) = ui.dnd_drop_zone::<u32, ()>(frame, |ui| { ui.set_width(ui.available_width()); ui.set_min_height(h); add(ui) });
    dropped.map(|p| *p)
}

/// Two faders side by side: monitor (what you hear) and stream. Returns the events.
struct Pair { mon: Option<f32>, strm: Option<f32>, mon_mute: bool, strm_mute: bool }

// ---------------------------------------------------------------- tray / window plumbing

/// Hide the main window (the app keeps running in the tray).
fn hide() {
    use windows::{core::w, Win32::UI::WindowsAndMessaging::*};
    unsafe { if let Ok(h) = FindWindowW(None, w!("Sonar Lite")) { let _ = ShowWindow(h, SW_HIDE); } }
}

/// Show + focus the main window through Win32. egui's own Visible(true) command is only handled while the window draws
/// frames, and a hidden window does not, so it cannot bring itself back from the tray.
fn reveal(ctx: &egui::Context) {
    use windows::{core::w, Win32::UI::WindowsAndMessaging::*};
    unsafe {
        if let Ok(h) = FindWindowW(None, w!("Sonar Lite")) {
            let _ = ShowWindow(h, SW_SHOW);
            let _ = ShowWindow(h, SW_RESTORE);
            let _ = SetForegroundWindow(h);
        }
    }
    ctx.request_repaint();
}

/// Waits for a second launch (see main) and shows the window.
fn watch_show(ctx: &egui::Context) {
    use windows::{core::w, Win32::System::Threading::{CreateEventW, WaitForSingleObject}};
    let ctx = ctx.clone();
    let Ok(ev) = (unsafe { CreateEventW(None, false, false, w!(r"Local\SonarLiteShow")) }) else { return };
    let ev = ev.0 as usize; // HANDLE is not Send
    std::thread::spawn(move || loop {
        let h = windows::Win32::Foundation::HANDLE(ev as *mut _);
        if unsafe { WaitForSingleObject(h, 500) }.0 == 0 { reveal(&ctx); }
    });
}

/// A newer build asks this (older) instance to quit; it does, so only one build ever runs.
fn watch_quit() {
    use windows::{core::w, Win32::System::Threading::{CreateEventW, WaitForSingleObject}};
    let Ok(ev) = (unsafe { CreateEventW(None, false, false, w!(r"Local\SonarLiteQuit")) }) else { return };
    let ev = ev.0 as usize;
    std::thread::spawn(move || {
        let h = windows::Win32::Foundation::HANDLE(ev as *mut _);
        loop { if unsafe { WaitForSingleObject(h, 500) }.0 == 0 { std::process::exit(0); } }
    });
}

/// Built on the thread that owns the window; a helper thread turns tray clicks into window commands.
fn make_tray(ctx: &egui::Context) -> Option<(tray_icon::TrayIcon, tray_icon::menu::MenuItem, tray_icon::menu::MenuItem)> {
    use tray_icon::{menu::{Menu, MenuEvent, MenuItem}, Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let menu = Menu::new();
    let (show, quit) = (MenuItem::new(tr("Pokaż Sonar Lite"), true, None), MenuItem::new(tr("Wyjdź"), true, None));
    menu.append_items(&[&show, &quit]).ok()?;
    let (px, n) = icon_rgba();
    let tray = TrayIconBuilder::new().with_icon(Icon::from_rgba(px, n, n).ok()?).with_tooltip("Sonar Lite").with_menu(Box::new(menu)).with_menu_on_left_click(false).build().ok()?;
    let (ctx, show_id, quit_id) = (ctx.clone(), show.id().clone(), quit.id().clone());
    std::thread::spawn(move || loop {
        while let Ok(e) = MenuEvent::receiver().try_recv() {
            if e.id == quit_id { std::process::exit(0) } else if e.id == show_id { reveal(&ctx) }
        }
        while let Ok(e) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e { reveal(&ctx) }
        }
        std::thread::sleep(Duration::from_millis(60));
    });
    Some((tray, show, quit))
}

// ---------------------------------------------------------------- state

impl App {
    fn reload(&mut self) {
        self.eps = endpoints(&self.hklm);
        self.chans = read_all(&self.hklm);
        for c in &self.chans { self.mixer.base.entry(c.key.to_string()).or_insert(c.gain); }
        self.presets = eq::list_presets();
        self.mic_presets = eq::list_presets_in(true);
        self.load_eq();
        self.load_apps();
        self.obs_hint = self.compute_obs_hint();
    }

    /// Switch Sonar's virtual devices back on if GG disabled them (it does so when it exits normally); waits briefly for them to appear.
    fn ensure_vads(&mut self) -> bool {
        if vad::ensure_enabled(&self.hklm) == 0 { return false; }
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(150));
            self.eps = endpoints(&self.hklm);
            if !self.sonar_missing() { break; }
        }
        true
    }

    /// True when Windows has none of Sonar's virtual channel devices (e.g. after GG was exited normally: it removes them).
    fn sonar_missing(&self) -> bool { STRIPS.iter().all(|s| sonar_guid(&self.eps, s.0).is_none()) }

    fn idx(&self, key: &str) -> Option<usize> { self.chans.iter().position(|c| c.key == key) }

    fn load_eq(&mut self) {
        if let Some(c) = self.chans.get(self.sel) {
            self.bands = (0..10).map(|i| c.band(&self.hklm, i)).collect();
            self.eq_on = c.eq_on(&self.hklm);
        }
    }

    /// Which Sonar channel a session's endpoint is (None = a normal Windows device).
    fn chan_of_endpoint(&self, endpoint: &str) -> Option<&'static str> {
        CHANNELS.iter().map(|c| c.0).find(|k| sonar_guid(&self.eps, k).as_deref() == Some(endpoint))
    }

    /// Apps with an audio session. The channel shown is where the audio really plays: an app pinned to a channel but playing
    /// elsewhere (e.g. it picked its own output device) is listed outside Sonar, with that device's name.
    fn load_apps(&mut self) {
        self.rows_at = Instant::now();
        let me = std::process::id();
        let mic_eps: Vec<String> = capture_endpoints(&self.hklm).into_iter().filter(|c| c.1.contains("Sonar - Microphone")).map(|c| c.0).collect();
        self.mic_apps = apps::capture_sessions().into_iter().filter(|s| s.pid != me && s.active && mic_eps.contains(&s.endpoint)).map(|s| s.exe.trim_end_matches(".exe").to_string()).collect();
        self.mic_apps.sort();
        self.mic_apps.dedup();
        let Some(pol) = &self.pol else { return };
        let sessions: Vec<apps::Session> = apps::sessions().into_iter().filter(|s| s.pid != me && !s.exe.eq_ignore_ascii_case("SonarLite.exe")).collect();
        let mut rows: Vec<AppRow> = vec![];
        for s in &sessions {
            if rows.iter().any(|r| r.pid == s.pid) { continue; }
            let mine: Vec<&apps::Session> = sessions.iter().filter(|x| x.pid == s.pid).collect();
            let pin = pol.get(s.pid);
            let pin_key = pin.as_deref().and_then(|g| self.chan_of_endpoint(g));
            let on = |x: &&apps::Session| self.chan_of_endpoint(&x.endpoint);
            let to = pin_key.filter(|k| mine.iter().any(|x| on(x) == Some(*k)))
                .or_else(|| mine.iter().filter(|x| x.active).find_map(on))
                .or_else(|| mine.iter().find_map(on))
                .or_else(|| pin_key.filter(|_| mine.iter().all(|x| !x.active))); // idle: its next stream goes where it is pinned
            let dev = if to.is_none() {
                self.eps.iter().find(|e| e.guid == s.endpoint).map_or(String::new(), |e| short(&e.name).to_string())
            } else { String::new() };
            rows.push(AppRow { pid: s.pid, exe: s.exe.clone(), to, dev });
        }
        self.rows = rows;
    }

    /// Running apps keep the channel settings they started with; refresh them (in the background) once the user lets go of the mouse.
    fn flush_dirty(&mut self, ctx: &egui::Context) {
        if self.dirty.is_empty() || ctx.input(|i| i.pointer.any_down()) { return; }
        let eps = self.eps.clone();
        let mut keys = std::mem::take(&mut self.dirty);
        keys.sort();
        keys.dedup();
        std::thread::spawn(move || apps::refresh_channels(&eps, &keys));
    }

    /// Channels that share a device list: all four when linked, else just `key`.
    fn prio_keys(&self, key: &'static str) -> Vec<&'static str> {
        if self.mixer.link_all { STRIPS.iter().map(|s| s.0).collect() } else { vec![key] }
    }

    /// Make `guid` the primary output of the channel (all channels when linked) and remember it at the top of the priority list.
    fn set_device(&mut self, key: &'static str, guid: &str, name: &str) {
        for k in self.prio_keys(key) {
            let l = self.mixer.prio.entry(k.to_string()).or_default();
            l.retain(|(g, _)| g != guid);
            l.insert(0, (guid.to_string(), name.to_string()));
            if let Some(i) = self.idx(k) { let r = self.chans[i].set_out(&self.hklm, guid); self.fail(r); self.dirty.push(k); }
        }
        mixer::save(&self.mixer);
    }

    /// Fallback: each channel plays on the first device of its priority list that is present right now.
    fn enforce_priority(&mut self) {
        let present: Vec<String> = self.eps.iter().filter(|e| !e.virt).map(|e| e.guid.clone()).collect();
        for (k, ..) in STRIPS {
            let Some(list) = self.mixer.prio.get(k) else { continue };
            if let (Some(d), Some(i)) = (mixer::desired_device(list, &present), self.idx(k)) {
                if self.chans[i].guid != d { let r = self.chans[i].set_out(&self.hklm, &d); self.fail(r); self.dirty.push(k); }
            }
        }
    }

    /// Re-read devices and what each channel currently uses (cheap; done every few seconds), then apply the fallback rules.
    fn refresh_devices(&mut self) {
        self.eps = endpoints(&self.hklm);
        for c in read_all(&self.hklm) { if let Some(i) = self.idx(c.key) { self.chans[i].guid = c.guid; } }
        self.enforce_priority();
    }

    fn fail(&mut self, r: std::io::Result<()>) { if let Err(e) = r { self.err = Some(e.to_string()); } }

    /// Apply fader * chatmix to a channel: the endpoint's live Windows volume (see levels.rs). The APO's own gain stays at 1.0.
    fn write_gain(&mut self, key: &'static str) {
        levels::set_gain(key, self.mixer.effective(key));
        if self.stream.is_some() { self.sync_stream_comp(); } // the stream mix undoes the endpoint volume
    }

    /// Older versions put the fader into the APO's registry gain (it needs an app refresh to be heard); that gain goes back to 1.0.
    fn reset_apo_gain(&mut self) {
        for i in 0..self.chans.len() {
            if (self.chans[i].gain - 1.0).abs() > 1e-3 {
                let r = self.chans[i].set_gain(&self.hklm, 1.0);
                self.chans[i].gain = 1.0;
                self.dirty.push(self.chans[i].key);
                self.fail(r);
            }
        }
    }

    // ----- microphone

    /// (Re)start the mic forwarder to match the saved settings.
    fn apply_mic(&mut self) {
        self.mic = None;
        self.mic_pending = None;
        if self.mic_cfg.enabled { self.mic_pending = Some(mic::spawn(self.mic_cfg.device.clone(), self.mic_cfg.clone(), Some(self.sh.clone()))); }
        mic::save_cfg(&self.mic_cfg);
    }

    /// Collect the result of a mic / stream start that runs on a worker thread.
    fn poll_workers(&mut self) {
        use std::sync::mpsc::TryRecvError;
        if let Some(rx) = &self.mic_pending {
            match rx.try_recv() {
                Ok(Ok(m)) => { self.mic = Some(m); self.mic_pending = None; }
                Ok(Err(e)) => { self.err = Some(format!("{}: {e}", tr("Mikrofon"))); self.mic_pending = None; }
                Err(TryRecvError::Disconnected) => self.mic_pending = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.stream_pending {
            match rx.try_recv() {
                Ok(Ok(s)) => { self.stream = Some(s); self.stream_pending = None; }
                Ok(Err(e)) => { self.err = Some(format!("{}: {e}", tr("Tryb streamera"))); self.stream_pending = None; }
                Err(TryRecvError::Disconnected) => self.stream_pending = None,
                Err(TryRecvError::Empty) => {}
            }
        }
    }

    /// Push the current mic settings into the running forwarder and save them.
    fn mic_changed(&mut self) {
        if let Some(m) = &self.mic { m.p.apply(&self.mic_cfg); }
        mic::save_cfg(&self.mic_cfg);
    }

    fn mic_eq_changed(&mut self) {
        if let Some(m) = &self.mic { m.p.set_eq_cfg(&self.mic_cfg); }
        mic::save_cfg(&self.mic_cfg);
    }

    // ----- streamer mode

    fn sync_stream_gains(&self) {
        for (i, k) in stream::KEYS.iter().enumerate() { self.sh.set_gain(i, self.mixer.stream_gain(k)); }
        self.sh.set_gain(4, self.mixer.stream_gain("mic"));
    }

    /// The stream mix has its own EQ copy of each channel's registry EQ.
    fn sync_stream_eq(&self) {
        for (i, k) in stream::KEYS.iter().enumerate() {
            if let Some(ci) = self.idx(k) {
                let c = &self.chans[ci];
                self.sh.set_eq(i, if c.eq_on(&self.hklm) { (0..10).map(|b| c.band(&self.hklm, b)).collect() } else { vec![] });
            }
        }
    }

    /// Loopback captures are scaled by the endpoint's Windows volume; undo that.
    fn sync_stream_comp(&self) {
        for (i, k) in stream::KEYS.iter().enumerate() { if let Some(g) = sonar_guid(&self.eps, k) { self.sh.set_comp(i, endpoint_comp(&g)); } }
    }

    fn apply_stream(&mut self) {
        self.stream = None;
        self.stream_pending = None;
        self.sh.mic_on.store(self.mixer.stream_on, Relaxed);
        if self.mixer.stream_on {
            self.sync_stream_gains();
            self.sync_stream_eq();
            self.sync_stream_comp();
            self.stream_pending = Some(stream::spawn(self.mixer.stream_dev.clone(), self.sh.clone()));
        }
        mixer::save(&self.mixer);
    }

    /// Name of the capture device OBS should record: the one that shares the stream device's driver.
    fn compute_obs_hint(&self) -> String {
        let adapter = |n: &str| n.rsplit_once(" (").map(|x| x.1.trim_end_matches(')').to_string()).unwrap_or_default();
        let Some(out) = self.eps.iter().find(|e| !e.virt && e.name.contains(&self.mixer.stream_dev)) else { return String::new() };
        let a = adapter(&out.name);
        capture_endpoints(&self.hklm).into_iter().find(|c| adapter(&c.1) == a).map_or(String::new(), |c| short(&c.1).to_string())
    }

    fn apply(&mut self, acts: Vec<Act>) {
        for a in acts {
            match a {
                Act::Fader(k, v) => { self.mixer.base.insert(k.into(), v); self.write_gain(k); mixer::save(&self.mixer); }
                Act::Master(v) => { self.mixer.move_master(v); for (k, ..) in STRIPS { self.write_gain(k); } mixer::save(&self.mixer); }
                Act::ChatMix(v) => { self.mixer.chatmix = v; self.write_gain("game"); self.write_gain("chat"); mixer::save(&self.mixer); }
                Act::Mute(k, m) => {
                    if let Some(i) = self.idx(k) { let r = self.chans[i].set_mute(&self.hklm, m); self.fail(r); self.dirty.push(k); }
                }
                Act::Out(k, g) => {
                    if let Some(i) = self.idx(k) { let r = self.chans[i].set_out(&self.hklm, &g); self.fail(r); self.dirty.push(k); }
                }
                Act::MoveApp(pid, to) => {
                    if let Some(k) = to.filter(|k| sonar_guid(&self.eps, k).is_none()) {
                        self.err = Some(tr_fmt("Kanał {} jest niedostępny: w systemie nie ma urządzenia „SteelSeries Sonar - …”. Aplikacji nie da się do niego przypisać.", &[k.to_uppercase()]));
                        continue;
                    }
                    if let Some(pol) = &self.pol {
                        match pol.set(pid, to.and_then(|k| sonar_guid(&self.eps, k)).as_deref()) {
                            Ok(()) => self.load_apps(),
                            Err(e) => self.err = Some(e.to_string()),
                        }
                    }
                }
                Act::Preset(k, name) => {
                    if let (Some(i), Some(b)) = (self.idx(k), eq::builtin(&name)) {
                        let r = self.chans[i].set_bands(&self.hklm, &b);
                        self.fail(r);
                        self.mixer.eq.insert(k.into(), name);
                        mixer::save(&self.mixer);
                        self.dirty.push(k);
                        if i == self.sel { self.load_eq(); }
                        self.sync_stream_eq();
                    }
                }
                Act::UserPreset(key, name) => {
                    if let Some(i) = self.idx(key) {
                        let r = eq::load_preset(&name).and_then(|v| self.chans[i].apply(&self.hklm, &v));
                        self.fail(r);
                        self.mixer.eq.insert(key.into(), name);
                        mixer::save(&self.mixer);
                        self.dirty.push(key);
                        if i == self.sel { self.load_eq(); }
                        self.sync_stream_eq();
                    }
                }
                Act::SavePreset(name) => {
                    if let Some(c) = self.chans.get(self.sel) {
                        let (key, r) = (c.key, eq::save_preset(&name, &c.snapshot(&self.hklm)));
                        if r.is_ok() { self.mixer.eq.insert(key.into(), name); mixer::save(&self.mixer); }
                        self.fail(r);
                    }
                    self.presets = eq::list_presets();
                }
                Act::MicUserPreset(name) => match eq::load_preset_in(true, &name) {
                    Ok(v) => { self.mic_cfg.bands = Some(eq::vals_to_bands(&v)); self.mic_cfg.eq = name; self.mic_cfg.eq_on = true; self.mic_eq_changed(); }
                    Err(e) => self.err = Some(e.to_string()),
                },
                Act::MicSavePreset(name) => {
                    let b = self.mic_cfg.bands.clone().or_else(|| eq::builtin(&self.mic_cfg.eq)).unwrap_or_default();
                    let r = eq::save_preset_in(true, &name, &eq::bands_to_vals(&b));
                    if r.is_ok() { self.mic_cfg.bands = Some(b); self.mic_cfg.eq = name; self.mic_eq_changed(); }
                    self.fail(r);
                    self.mic_presets = eq::list_presets_in(true);
                }
                Act::DeletePreset(mic, name) => {
                    let r = eq::delete_preset_in(mic, &name);
                    self.fail(r);
                    self.presets = eq::list_presets();
                    self.mic_presets = eq::list_presets_in(true);
                }
                Act::Bands(b) => {
                    if let Some(c) = self.chans.get(self.sel) {
                        let key = c.key;
                        let mut r = Ok(());
                        for (i, x) in b.iter().enumerate() { r = r.and(c.set_band(&self.hklm, i, x)); }
                        self.fail(r);
                        self.mixer.eq.insert(key.into(), "Własny".into());
                        self.dirty.push(key);
                    }
                    self.bands = b;
                    self.sync_stream_eq();
                }
                Act::EqOn(on) => {
                    if let Some(c) = self.chans.get(self.sel) { let key = c.key; let r = c.set_eq_on(&self.hklm, on); self.fail(r); self.dirty.push(key); }
                    self.eq_on = on;
                    self.sync_stream_eq();
                }
                Act::Open(k) => {
                    self.prefs_open = false;
                    self.settings = k;
                    if let Some(i) = k.and_then(|k| self.idx(k)) { self.sel = i; self.load_eq(); }
                }
                Act::MicGain(g) => { self.mic_cfg.gain = g; self.mic_changed(); }
                Act::MicMute(m) => { self.mic_cfg.mute = m; self.mic_changed(); }
                Act::MicDev(d) => { self.mic_cfg.device = d; if self.mic_cfg.enabled { self.apply_mic() } else { mic::save_cfg(&self.mic_cfg) } }
                Act::MicGate(g) => { self.mic_cfg.gate = g; self.mic_changed(); }
                Act::MicGateDb(d) => { self.mic_cfg.gate_db = d; self.mic_changed(); }
                Act::MicNc(n) => { self.mic_cfg.nc = n; self.mic_changed(); }
                Act::MicNcPct(p) => { self.mic_cfg.nc_pct = p; self.mic_changed(); }
                Act::MicEq(name) => { self.mic_cfg.eq = name; self.mic_cfg.bands = None; self.mic_cfg.eq_on = true; self.mic_eq_changed(); }
                Act::MicEqOn(on) => { self.mic_cfg.eq_on = on; self.mic_eq_changed(); }
                Act::MicBands(b) => { self.mic_cfg.bands = Some(b); self.mic_cfg.eq = "Własny".into(); self.mic_eq_changed(); }
                Act::MicGateAuto(a) => { self.mic_cfg.gate_auto = a; self.mic_changed(); }
                Act::MicComp(c) => { self.mic_cfg.comp = c; self.mic_changed(); }
                Act::MicCompLevel(l) => { self.mic_cfg.comp_level = l; self.mic_changed(); }
                Act::OpenDev(k) => self.device_win = k,
                Act::Link(on) => {
                    self.mixer.link_all = on;
                    if on {
                        // share the list of the channel being edited (or GAME) with all channels
                        let src = self.device_win.unwrap_or("game");
                        let list = self.mixer.prio.get(src).cloned().unwrap_or_default();
                        for (k, ..) in STRIPS { self.mixer.prio.insert(k.to_string(), list.clone()); }
                    }
                    mixer::save(&self.mixer);
                    self.enforce_priority();
                }
                Act::SetDevice(k, g, n) => self.set_device(k, &g, &n),
                Act::DevMove(k, g, to) => {
                    let name = self.eps.iter().find(|e| e.guid == g).map(|e| e.name.clone())
                        .or_else(|| [&self.mixer.prio, &self.mixer.prio_off].iter().filter_map(|m| m.get(k)).flatten().find(|x| x.0 == g).map(|x| x.1.clone()))
                        .unwrap_or_default();
                    for kk in self.prio_keys(k) {
                        let on = self.mixer.prio.entry(kk.to_string()).or_default();
                        let was = on.iter().position(|x| x.0 == g);
                        on.retain(|x| x.0 != g);
                        if let Some(i) = to { let i = if was.is_some_and(|p| p < i) { i - 1 } else { i }; on.insert(i.min(on.len()), (g.clone(), name.clone())); }
                        let off = self.mixer.prio_off.entry(kk.to_string()).or_default();
                        off.retain(|x| x.0 != g);
                        if to.is_none() { off.push((g.clone(), name.clone())); }
                    }
                    mixer::save(&self.mixer);
                    self.enforce_priority();
                }
                Act::MicOut(o) => { self.mic_cfg.out = o; if self.mic_cfg.enabled { self.apply_mic() } else { mic::save_cfg(&self.mic_cfg) } }
                Act::AllOut(g) => {
                    let name = self.eps.iter().find(|e| e.guid == g).map(|e| e.name.clone()).unwrap_or_default();
                    for (k, ..) in STRIPS {
                        let l = self.mixer.prio.entry(k.to_string()).or_default();
                        l.retain(|(x, _)| *x != g);
                        l.insert(0, (g.clone(), name.clone()));
                        if let Some(i) = self.idx(k) { let r = self.chans[i].set_out(&self.hklm, &g); self.fail(r); self.dirty.push(k); }
                    }
                    mixer::save(&self.mixer);
                }
                Act::StreamOn(on) => { self.mixer.stream_on = on; self.apply_stream(); }
                Act::StreamDev(d) => { self.mixer.stream_dev = d; self.obs_hint = self.compute_obs_hint(); if self.mixer.stream_on { self.apply_stream() } else { mixer::save(&self.mixer) } }
                Act::SFader(k, v) => { self.mixer.sgain.insert(k.into(), v); self.sync_stream_gains(); mixer::save(&self.mixer); }
                Act::SMute(k, m) => { self.mixer.smute.insert(k.into(), m); self.sync_stream_gains(); mixer::save(&self.mixer); }
                Act::SMaster(v) => { self.mixer.smaster = v; self.sync_stream_gains(); mixer::save(&self.mixer); }
                Act::Autostart(on) => match autostart::set(on, self.prefs.start_in_tray) { Ok(()) => self.autostart = on, Err(e) => self.err = Some(e.to_string()) },
                Act::Prefs(b) => self.prefs_open = b,
                Act::Quit => std::process::exit(0),
                Act::SetLang(l) => {
                    self.prefs.lang = l;
                    i18n::set_lang(l);
                    settings::save(&self.prefs);
                    if let Some((show, quit)) = &self.tray_items { show.set_text(tr("Pokaż Sonar Lite")); quit.set_text(tr("Wyjdź")); }
                }
                Act::CloseToTray(b) => { self.prefs.close_to_tray = b; settings::save(&self.prefs); }
                Act::StartInTray(b) => {
                    self.prefs.start_in_tray = b;
                    settings::save(&self.prefs);
                    if self.autostart { if let Err(e) = autostart::set(true, b) { self.err = Some(e.to_string()); } } // rewrite the entry with the new arguments
                }
                Act::HideMissing => self.missing_hidden = true,
                Act::OpenConfigDir => { let _ = std::process::Command::new("explorer").arg(settings::dir()).spawn(); }
                Act::Refresh => { self.reload(); self.err = None; self.sync_stream_eq(); }
            }
        }
    }

    // ---------------------------------------------------------------- drawing (read-only; returns actions)

    /// App chips of one channel (None = outside Sonar). Chips can be dragged onto another channel's box; right-click also moves.
    fn chips(&self, ui: &mut egui::Ui, here: Option<&'static str>, acts: &mut Vec<Act>) {
        let mine: Vec<&AppRow> = self.rows.iter().filter(|r| r.to == here).collect();
        for r in mine {
            let c = here.map_or(Color32::from_rgb(150, 154, 166), col);
            let label = if r.dev.is_empty() { trunc(r.exe.trim_end_matches(".exe"), 18) } else { trunc(&format!("{} · {}", r.exe.trim_end_matches(".exe"), r.dev), 22) };
            let resp = ui.dnd_drag_source(egui::Id::new(("chip", r.pid)), r.pid, |ui| app_chip(ui, &label, c)).response;
            let resp = if r.dev.is_empty() { resp } else { resp.on_hover_text(tr_fmt("{} gra bezpośrednio na urządzeniu „{}” (ma własny wybór wyjścia). Ustaw w niej wyjście „Domyślne”, żeby przypisać ją do kanału.", &[r.exe.to_string(), r.dev.to_string()])) };
            resp.context_menu(|ui| {
                for (k, title, _, c) in STRIPS {
                    if here != Some(k) && ui.add(egui::Button::new(RichText::new(format!("→ {title}")).color(c)).frame(false)).clicked() { acts.push(Act::MoveApp(r.pid, Some(k))); ui.close_menu(); }
                }
                if here.is_some() && ui.add(egui::Button::new(tr("→ Domyślne (Windows)")).frame(false)).clicked() { acts.push(Act::MoveApp(r.pid, None)); ui.close_menu(); }
            });
        }
    }

    /// Fader(s) of a strip: one in classic mode, monitor + stream in streamer mode.
    fn faders(&self, ui: &mut egui::Ui, color: Color32, mon: f32, mon_reset: f32, mon_mute: bool, strm: f32, strm_mute: bool, meter: Option<Meter>) -> Pair {
        let mut out = Pair { mon: None, strm: None, mon_mute: false, strm_mute: false };
        if !self.mixer.stream_on {
            out.mon = fader(ui, mon, color, mon_reset, meter.as_ref());
            out.mon_mute = mute_button(ui, mon_mute);
        } else {
            ui.columns(2, |c| {
                c[0].vertical_centered(|ui| ui.label(RichText::new("MONITOR").size(9.0).color(WEAK)));
                out.mon = fader(&mut c[0], mon, color, mon_reset, meter.as_ref());
                out.mon_mute = mute_button(&mut c[0], mon_mute);
                c[1].vertical_centered(|ui| ui.label(RichText::new("STREAM").size(9.0).color(ACCENT)));
                out.strm = fader(&mut c[1], strm, color, 1.0, None);
                out.strm_mute = mute_button(&mut c[1], strm_mute);
            });
        }
        out
    }

    fn channel_strip(&self, ui: &mut egui::Ui, key: &'static str, title: &str, icon: &str, color: Color32, acts: &mut Vec<Act>) {
        let Some(i) = self.idx(key) else { return };
        let c = &self.chans[i];
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            header(ui, icon, title, color);
            body(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                // preset row (click = advanced window) and device row (click = device window), like Sonar
                let cur = self.mixer.eq.get(key).map_or("— preset —", |s| s.as_str());
                let row = preset_row(ui, cur);
                let (pick_builtin, pick_own, edit) = preset_dropdown(ui, ui.id().with(("presets", key)), &row, cur, eq::builtin_names(), &self.presets);
                if let Some(n) = pick_builtin { acts.push(Act::Preset(key, n)); }
                if let Some(n) = pick_own { acts.push(Act::UserPreset(key, n)); }
                if edit { acts.push(Act::Open(Some(key))); }
                let dev = self.eps.iter().find(|e| e.guid == c.guid).map_or(tr("<nieznane>").to_string(), |e| short(&e.name).to_string());
                let base = self.mixer.base.get(key).copied().unwrap_or(1.0);
                let (n, l) = device_row(ui, key, self.mixer.link_all, &dev, base * 100.0);
                if n.clicked() { acts.push(Act::OpenDev(if self.device_win == Some(key) { None } else { Some(key) })); }
                if self.device_win == Some(key) {
                    // floating over the strip (not pushing the fader down), directly under the device row
                    let (pos, w) = (l.rect.left_bottom() + egui::vec2(0.0, 2.0), n.rect.right() - l.rect.left());
                    egui::Area::new(ui.id().with(("devpop", key))).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| { ui.set_width(w - 14.0); self.device_list(ui, key, acts); });
                    });
                }
                if l.clicked() { acts.push(Act::Link(!self.mixer.link_all)); }
                let sg = self.mixer.sgain.get(key).copied().unwrap_or(1.0);
                let meter = Meter { level: levels::get(key), thr_db: None, open: !c.mute, post_fader: true };
                let p = self.faders(ui, color, base, 1.0, c.mute, sg, self.mixer.smute.get(key).copied().unwrap_or(false), Some(meter));
                if let Some(v) = p.mon { acts.push(Act::Fader(key, v)); }
                if p.mon_mute { acts.push(Act::Mute(key, !c.mute)); }
                if let Some(v) = p.strm { acts.push(Act::SFader(key, v)); }
                if p.strm_mute { acts.push(Act::SMute(key, !self.mixer.smute.get(key).copied().unwrap_or(false))); }
                ui.label(RichText::new(tr("Aplikacje")).size(11.0).color(WEAK));
                if let Some(pid) = drop_zone(ui, 110.0, |ui| self.chips(ui, Some(key), acts)) { acts.push(Act::MoveApp(pid, Some(key))); }
            });
        });
    }

    fn master_strip(&self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            header(ui, "🔊", "MASTER", MASTER_COL);
            body(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                // default output for everything, and (streamer mode) the device that carries the stream mix
                let guids: Vec<&str> = self.chans.iter().map(|c| c.guid.as_str()).collect();
                let all = guids.first().filter(|g| guids.iter().all(|x| x == *g)).and_then(|g| self.eps.iter().find(|e| e.guid == *g)).map_or(tr("różne").to_string(), |e| short(&e.name).to_string());
                let (n, _) = device_row(ui, "all", false, &format!("{}: {all}", tr("Odsłuch")), self.mixer.master * 100.0);
                popup_menu(ui, ui.id().with("alldev"), &n, |ui| {
                    for e in self.eps.iter().filter(|e| !e.virt) { if ui.selectable_label(false, &e.name).clicked() { acts.push(Act::AllOut(e.guid.clone())); } }
                });
                if self.mixer.stream_on {
                    let (n, _) = device_row(ui, "stream", false, &format!("Stream: {}", self.mixer.stream_dev), self.mixer.smaster * 100.0);
                    popup_menu(ui, ui.id().with("streamdev"), &n, |ui| {
                        for e in self.eps.iter().filter(|e| !e.virt) { if ui.selectable_label(e.name.contains(&self.mixer.stream_dev), &e.name).clicked() { acts.push(Act::StreamDev(short(&e.name).to_string())); } }
                    });
                    ui.add_space(4.0); // the channel strips have a preset row (28 px) above their device row: keep MASTER as tall
                } else {
                    ui.add_space(34.0);
                }
                // master shows the loudest channel (each already scaled by its own fader)
                let loud = STRIPS.iter().map(|s| levels::get(s.0) * self.mixer.base.get(s.0).copied().unwrap_or(1.0)).fold(0.0f32, f32::max);
                let p = self.faders(ui, MASTER_COL, self.mixer.master, 1.0, false, self.mixer.smaster, false, Some(Meter { level: loud, thr_db: None, open: true, post_fader: false }));
                if let Some(v) = p.mon { acts.push(Act::Master(v)); }
                if let Some(v) = p.strm { acts.push(Act::SMaster(v)); }
                ui.label(RichText::new(tr("Aplikacje do przypisania")).size(11.0).color(WEAK));
                if let Some(pid) = drop_zone(ui, 110.0, |ui| if self.pol.is_some() { self.chips(ui, None, acts) } else { ui.label(RichText::new(tr("niedostępne")).color(WEAK)); }) {
                    acts.push(Act::MoveApp(pid, None));
                }
            });
        });
    }

    fn mic_strip(&self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let cfg = &self.mic_cfg;
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            header(ui, "🎤", "MIC", MIC_COL);
            body(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                let cur = cfg.eq.as_str();
                let row = preset_row(ui, cur);
                let (pick_builtin, pick_own, edit) = preset_dropdown(ui, ui.id().with("micpresets"), &row, cur, eq::voice_names(), &self.mic_presets);
                if let Some(n) = pick_builtin { acts.push(Act::MicEq(n)); }
                if let Some(n) = pick_own { acts.push(Act::MicUserPreset(n)); }
                if edit { acts.push(Act::Open(Some("mic"))); }
                let dev = if cfg.device.is_empty() { tr("<wybierz mikrofon>").to_string() } else { short(&cfg.device).to_string() };
                let (n, _) = device_row(ui, "mic", false, &dev, cfg.gain * 100.0);
                popup_menu(ui, ui.id().with("micdev"), &n, |ui| {
                    for d in &self.mic_inputs { if ui.selectable_label(*d == cfg.device, d).clicked() { acts.push(Act::MicDev(d.clone())); } }
                });
                let sg = self.mixer.sgain.get("mic").copied().unwrap_or(1.0);
                let meter = self.mic.as_ref().map(|m| Meter { level: f32::from_bits(m.p.peak.load(Relaxed)), thr_db: cfg.gate.then_some(cfg.gate_db), open: m.p.open.load(Relaxed) || !cfg.gate, post_fader: false });
                let p = self.faders(ui, MIC_COL, cfg.gain / 2.0, 0.5, cfg.mute, sg, self.mixer.smute.get("mic").copied().unwrap_or(false), meter);
                if let Some(v) = p.mon { acts.push(Act::MicGain(v * 2.0)); }
                if p.mon_mute { acts.push(Act::MicMute(!cfg.mute)); }
                if let Some(v) = p.strm { acts.push(Act::SFader("mic", v)); }
                if p.strm_mute { acts.push(Act::SMute("mic", !self.mixer.smute.get("mic").copied().unwrap_or(false))); }
                // apps recording from the Sonar microphone; noise reduction / gate live in the MIC page
                ui.label(RichText::new(tr("Aplikacje")).size(11.0).color(WEAK));
                let _ = drop_zone(ui, 110.0, |ui| { for a in &self.mic_apps { app_chip(ui, &trunc(a, 18), MIC_COL); } });
            });
        });
    }

    /// CHATMIX: 0..100 = share of CHAT (50 = both full). Snaps to 50 near the middle; exact value can be typed.
    fn chatmix(&self, ui: &mut egui::Ui, w: f32, acts: &mut Vec<Act>) {
        ui.allocate_ui_with_layout(egui::vec2(w, 64.0), egui::Layout::top_down(egui::Align::Center), |ui| {
            egui::Frame::none().fill(BODY).rounding(8.0).inner_margin(egui::Margin::symmetric(12.0, 6.0)).show(ui, |ui| {
                ui.set_width(w - 24.0);
                let mut pct = (self.mixer.chatmix * 100.0).round();
                let (g, c) = ((2.0 * (1.0 - self.mixer.chatmix)).min(1.0) * 100.0, (2.0 * self.mixer.chatmix).min(1.0) * 100.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("CHATMIX").size(11.0).color(WEAK));
                    ui.label(RichText::new(format!("GAME {g:.0}%  ·  CHAT {c:.0}%")).size(11.0).color(WEAK));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("%").size(11.0).color(WEAK));
                        if ui.add(egui::DragValue::new(&mut pct).range(0.0..=100.0).speed(0.5).max_decimals(0)).changed() { acts.push(Act::ChatMix(pct / 100.0)); }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("🎮").size(18.0).color(col("game")));
                    ui.spacing_mut().slider_width = w - 24.0 - 90.0;
                    let mut v = pct;
                    let r = ui.add(egui::Slider::new(&mut v, 0.0..=100.0).step_by(1.0).show_value(false));
                    if r.double_clicked() { acts.push(Act::ChatMix(0.5)); }
                    else if r.changed() {
                        if (v - 50.0).abs() <= 3.0 { v = 50.0; } // snap to the centre
                        acts.push(Act::ChatMix(v / 100.0));
                    }
                    ui.label(RichText::new("💬").size(18.0).color(col("chat")));
                });
            });
        });
    }

    /// The channel/mic page of Sonar is a tab (see the tab bar in draw); clicking a preset row switches to it.
    fn page_card<R>(&self, ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
        egui::Frame::none().fill(BODY).rounding(8.0).inner_margin(egui::Margin::same(14.0)).show(ui, |ui| { ui.set_width(ui.available_width()); add(ui) }).inner
    }

    /// Channel page: Equalizer card (more cards - Spatial Audio, Volume Boost - once their APO settings are verified).
    fn channel_page(&self, ui: &mut egui::Ui, key: &'static str, acts: &mut Vec<Act>) {
        let cur = self.mixer.eq.get(key).map_or("—", |s| s.as_str());
        self.page_card(ui, |ui| {
            let (tg, reset) = eq_head(ui, self.eq_on);
            if tg { acts.push(Act::EqOn(!self.eq_on)); }
            if reset { acts.push(Act::Preset(key, "Flat".into())); }
            let mut bands = self.bands.clone();
            let o = eq_section(ui, key, &mut bands, self.eq_on, cur, eq::builtin_names(), &self.presets);
            if o.bands { acts.push(Act::Bands(bands)); }
            if let Some(n) = o.builtin { acts.push(Act::Preset(key, n)); }
            if let Some(n) = o.custom { acts.push(Act::UserPreset(key, n)); }
            if let Some(n) = o.save { acts.push(Act::SavePreset(n)); }
            if let Some(n) = o.delete { acts.push(Act::DeletePreset(false, n)); }
        });
    }

    /// Mic page: Clearcast-like AI noise cancellation, noise gate, compressor, equalizer.
    fn mic_page(&self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let cfg = &self.mic_cfg;
        self.page_card(ui, |ui| {
            ui.columns(2, |c| {
                c[0].label(RichText::new(tr("Wejście")).color(WEAK).size(12.0));
                egui::ComboBox::from_id_salt("micwin").selected_text(trunc(&cfg.device, 60)).width(c[0].available_width()).show_ui(&mut c[0], |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    for n in &self.mic_inputs { if ui.selectable_label(*n == cfg.device, n).clicked() { acts.push(Act::MicDev(n.clone())); } }
                });
                c[1].label(RichText::new(tr("Wyjście (domyślnie „Sonar - Microphone”)")).color(WEAK).size(12.0));
                egui::ComboBox::from_id_salt("micwinout").selected_text(trunc(&cfg.out, 60)).width(c[1].available_width()).show_ui(&mut c[1], |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    for n in &self.mic_outputs { if ui.selectable_label(n.contains(&cfg.out), n).clicked() { acts.push(Act::MicOut(short(n).to_string())); } }
                });
            });
        });
        ui.add_space(8.0);
        self.page_card(ui, |ui| {
            if card_header(ui, tr("REDUKCJA SZUMÓW AI (RNNoise)"), Some(cfg.nc)) { acts.push(Act::MicNc(!cfg.nc)); }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Min").color(WEAK).size(11.0));
                let mut pct = cfg.nc_pct;
                ui.spacing_mut().slider_width = ui.available_width() - 120.0;
                if ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).integer().show_value(false)).changed() { acts.push(Act::MicNcPct(pct)); }
                ui.label(RichText::new(format!("Max   {pct:.0}%")).color(WEAK).size(11.0));
            });
            if let Some(m) = &self.mic { if cfg.nc { ui.label(RichText::new(format!("{}: {:.0}%", tr("rozpoznana mowa"), f32::from_bits(m.p.vad.load(Relaxed)) * 100.0)).color(WEAK).size(11.0)); } }
        });
        ui.add_space(8.0);
        ui.columns(2, |c| {
            self.page_card(&mut c[0], |ui| {
                if card_header(ui, tr("KOMPRESOR"), Some(cfg.comp)) { acts.push(Act::MicComp(!cfg.comp)); }
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr("Poziom")).color(WEAK).size(11.0));
                    let mut l = cfg.comp_level;
                    if ui.add(egui::Slider::new(&mut l, 0.0..=1.0).step_by(0.01)).changed() { acts.push(Act::MicCompLevel(l)); }
                });
            });
            self.page_card(&mut c[1], |ui| {
                if card_header(ui, tr("BRAMKA SZUMÓW"), Some(cfg.gate)) { acts.push(Act::MicGate(!cfg.gate)); }
                // one row like the compressor card: threshold slider, then the automatic-threshold switch to its right
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!cfg.gate_auto, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(tr("Próg")).color(WEAK).size(11.0));
                            ui.spacing_mut().slider_width = (ui.available_width() - 330.0).clamp(60.0, 400.0);
                            let mut db = cfg.gate_db;
                            if ui.add(egui::Slider::new(&mut db, -80.0..=-10.0).suffix(" dB")).changed() { acts.push(Act::MicGateDb(db)); }
                        });
                    });
                    let mut a = cfg.gate_auto;
                    if ui.checkbox(&mut a, tr("Oblicz próg automatycznie")).changed() { acts.push(Act::MicGateAuto(a)); }
                });
            });
        });
        ui.add_space(8.0);
        self.page_card(ui, |ui| {
            let (tg, reset) = eq_head(ui, cfg.eq_on);
            if tg { acts.push(Act::MicEqOn(!cfg.eq_on)); }
            if reset { acts.push(Act::MicEq("Flat".into())); }
            let mut bands = cfg.bands.clone().or_else(|| eq::builtin(&cfg.eq)).unwrap_or_default();
            let o = eq_section(ui, "mic", &mut bands, cfg.eq_on, &cfg.eq, eq::voice_names(), &self.mic_presets);
            if o.bands { acts.push(Act::MicBands(bands)); }
            if let Some(n) = o.builtin { acts.push(Act::MicEq(n)); }
            if let Some(n) = o.custom { acts.push(Act::MicUserPreset(n)); }
            if let Some(n) = o.save { acts.push(Act::MicSavePreset(n)); }
            if let Some(n) = o.delete { acts.push(Act::DeletePreset(true, n)); }
        });
    }

    /// Device list of a channel, shown inside its column. Drag devices to reorder (order = priority: the channel plays on the first
    /// available one) or into "Wyłączone" to switch them off; nothing is ever deleted, switched-off devices can be dragged back.
    fn device_list(&self, ui: &mut egui::Ui, key: &'static str, acts: &mut Vec<Act>) {
        let list = self.mixer.prio.get(key).cloned().unwrap_or_default();
        let mut off = self.mixer.prio_off.get(key).cloned().unwrap_or_default();
        off.retain(|x| !list.iter().any(|l| l.0 == x.0));
        for e in self.eps.iter().filter(|e| !e.virt && !list.iter().any(|l| l.0 == e.guid)) {
            if !off.iter().any(|o| o.0 == e.guid) { off.push((e.guid.clone(), e.name.clone())); }
        }
        let cur = self.idx(key).map(|i| self.chans[i].guid.clone()).unwrap_or_default();
        let mut link = self.mixer.link_all;
        if ui.checkbox(&mut link, tr("Połącz kanały")).changed() { acts.push(Act::Link(link)); }
        ui.label(RichText::new(tr("Przeciągnij: kolejność = priorytet")).color(WEAK).size(11.0));
        let chip = |ui: &mut egui::Ui, n: &(String, String), label: String| {
            let present = self.eps.iter().any(|e| e.guid == n.0 && !e.virt);
            let c = if n.0 == cur { ACCENT } else if present { Color32::WHITE } else { WEAK };
            ui.dnd_drag_source(egui::Id::new(("dev", key, &n.0)), n.0.clone(), |ui| {
                ui.add(egui::Button::new(RichText::new(trunc(&label, 22)).size(12.0).color(c)).fill(IDLE).min_size(egui::vec2(ui.available_width(), 22.0)))
            }).response.rect
        };
        let frame = egui::Frame::none().fill(INSET).rounding(5.0).inner_margin(egui::Margin::same(6.0));
        let mut rects = vec![];
        let (inner, dropped) = ui.dnd_drop_zone::<String, ()>(frame, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(28.0);
            for (i, n) in list.iter().enumerate() { rects.push(chip(ui, n, format!("{}. {}", i + 1, n.1))); }
        });
        let pos = ui.ctx().pointer_interact_pos();
        let at = pos.map_or(list.len(), |p| rects.iter().position(|r| p.y < r.center().y).unwrap_or(rects.len()));
        if let (Some(p), true) = (pos, egui::DragAndDrop::has_payload_of_type::<String>(ui.ctx())) {
            if inner.response.rect.contains(p) {
                let y = rects.get(at).map_or_else(|| rects.last().map_or(inner.response.rect.top() + 4.0, |r| r.bottom() + 2.0), |r| r.top() - 2.0);
                ui.painter().hline(inner.response.rect.x_range(), y, egui::Stroke::new(2.0f32, ACCENT));
            }
        }
        if let Some(g) = dropped { acts.push(Act::DevMove(key, (*g).clone(), Some(at))); }
        ui.label(RichText::new(tr("Wyłączone")).size(11.0).color(WEAK));
        let (_, dropped) = ui.dnd_drop_zone::<String, ()>(frame, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(28.0);
            for n in &off { chip(ui, n, n.1.clone()); }
        });
        if let Some(g) = dropped { acts.push(Act::DevMove(key, (*g).clone(), None)); }
    }

    /// Program settings as one scrolling page inside the window: every section is shown at once, the list on the left jumps to one.
    fn settings_page(&self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let (jid, aid) = (egui::Id::new("prefs_jump"), egui::Id::new("prefs_active"));
        let cats = [tr("Ogólne"), tr("Uruchamianie"), tr("Urządzenia i aplikacje"), tr("Dane programu"), tr("O programie")];
        let active: usize = ui.data(|d| d.get_temp(aid)).unwrap_or(0);
        let mut jump: Option<usize> = ui.data(|d| d.get_temp::<Option<usize>>(jid)).flatten();
        let mut seen = active;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            ui.allocate_ui_with_layout(egui::vec2(220.0, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                egui::Frame::none().fill(BODY).rounding(8.0).inner_margin(egui::Margin::same(8.0)).show(ui, |ui| {
                    ui.set_width(204.0);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for (i, name) in cats.iter().enumerate() {
                        let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), egui::Sense::click());
                        let p = ui.painter();
                        if i == active || r.hovered() { p.rect_filled(rect, 5.0, if i == active { IDLE } else { Color32::from_rgb(52, 57, 66) }); }
                        if i == active { p.rect_filled(egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 6.0), egui::vec2(3.0, rect.height() - 12.0)), 1.5, ACCENT); }
                        p.text(rect.left_center() + egui::vec2(16.0, 0.0), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(14.0), if i == active { Color32::WHITE } else { Color32::from_rgb(190, 196, 208) });
                        if r.clicked() { jump = Some(i); seen = i; }
                    }
                    ui.add_space(40.0);
                    ui.separator();
                    if ui.add(egui::Button::new(RichText::new(tr("Wyjdź z programu")).size(13.0)).min_size(egui::vec2(ui.available_width(), 32.0))).clicked() { acts.push(Act::Quit); }
                });
            });
            let (w, h) = (ui.available_width(), ui.available_height());
            ui.allocate_ui_with_layout(egui::vec2(w, h), egui::Layout::top_down(egui::Align::Min), |ui| {
                let out = egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let top = ui.clip_rect().top();
                    let mut tops = [0.0f32; 5];
                    let row = |ui: &mut egui::Ui, on: bool, text: String| -> bool {
                        ui.horizontal(|ui| { let c = switch(ui, on); ui.label(RichText::new(text).size(13.0)); c }).inner
                    };
                    for (i, name) in cats.iter().enumerate() {
                        self.page_card(ui, |ui| {
                            tops[i] = ui.cursor().top();
                            if jump == Some(i) { ui.scroll_to_cursor(Some(egui::Align::TOP)); }
                            ui.label(RichText::new(name.as_ref() as &str).strong().size(16.0));
                            ui.add_space(4.0);
                            ui.spacing_mut().item_spacing.y = 10.0;
                            match i {
                                0 => {
                                    let cur = self.prefs.lang.name();
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(tr("Język")).color(WEAK).size(13.0));
                                        egui::ComboBox::from_id_salt("lang").selected_text(cur).width(180.0).show_ui(ui, |ui| {
                                            ALL_FONTS.store(true, Relaxed); // the names are in several scripts
                                            for l in i18n::Lang::ALL {
                                                if ui.selectable_label(self.prefs.lang == l, l.name()).clicked() { acts.push(Act::SetLang(l)); }
                                            }
                                        });
                                    });
                                }
                                1 => {
                                    if row(ui, self.autostart, tr("Uruchamiaj z Windows").to_string()) { acts.push(Act::Autostart(!self.autostart)); }
                                    if row(ui, self.prefs.start_in_tray, tr("Start w zasobniku").to_string()) { acts.push(Act::StartInTray(!self.prefs.start_in_tray)); }
                                    if row(ui, self.prefs.close_to_tray, tr("Zamknięcie chowa do zasobnika").to_string()) { acts.push(Act::CloseToTray(!self.prefs.close_to_tray)); }
                                }
                                2 => { if ui.button(tr("Odśwież urządzenia i aplikacje")).clicked() { acts.push(Act::Refresh); } }
                                3 => { if ui.button(tr("Otwórz folder ustawień")).clicked() { acts.push(Act::OpenConfigDir); } }
                                _ => { ui.label(RichText::new(format!("Sonar Lite {} (build {})", env!("CARGO_PKG_VERSION"), env!("SONAR_BUILD"))).color(WEAK).size(12.0)); }
                            }
                        });
                        ui.add_space(12.0);
                    }
                    // the section nearest the top of the view is the one highlighted on the left
                    (0..5).rev().find(|&i| tops[i] <= top + 40.0).unwrap_or(0)
                });
                if jump.is_none() { seen = out.inner; }
            });
        });
        ui.data_mut(|d| { d.insert_temp(aid, seen); d.insert_temp(jid, None::<usize>); });
    }

    fn draw(&self, ctx: &egui::Context) -> Vec<Act> {
        let mut acts = vec![];
        // 1px frame: the window has no system border any more
        ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("winframe"))).rect_stroke(ctx.screen_rect().shrink(0.5), 0.0, egui::Stroke::new(1.0f32, IDLE));
        // own title bar: drag to move, double-click to maximise, app buttons on the right, then minimise / maximise / close
        egui::TopBottomPanel::top("titlebar").exact_height(36.0).frame(egui::Frame::none().fill(INSET)).show(ctx, |ui| {
            let drag = ui.interact(ui.max_rect(), egui::Id::new("titledrag"), egui::Sense::click_and_drag());
            let maxed = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            if drag.drag_started_by(egui::PointerButton::Primary) { ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag); }
            if drag.double_clicked() { ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maxed)); }
            ui.horizontal_centered(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new("SONAR LITE").strong().size(13.0).color(Color32::WHITE));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if win_btn(ui, 2, maxed) { ctx.send_viewport_cmd(egui::ViewportCommand::Close); } // close_to_tray is handled in update()
                    if win_btn(ui, 1, maxed) { ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maxed)); }
                    if win_btn(ui, 0, maxed) { ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)); }
                });
            });
        });
        let hint = if self.mixer.stream_on && !self.obs_hint.is_empty() { Some(format!("OBS: {}", self.obs_hint)) } else { None };
        if hint.is_some() || self.err.is_some() {
            egui::TopBottomPanel::bottom("foot").frame(egui::Frame::none().fill(BG).inner_margin(egui::Margin::symmetric(14.0, 6.0))).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if let Some(h) = hint { ui.label(RichText::new(h).color(WEAK).size(12.0)); }
                    if let Some(e) = &self.err { ui.colored_label(Color32::LIGHT_RED, e); }
                });
            });
        }
        // Sonar-style tab bar: Mixer + one advanced page per channel (and MIC)
        egui::TopBottomPanel::top("tabs").frame(egui::Frame::none().fill(BG).inner_margin(egui::Margin::symmetric(14.0, 8.0))).show(ctx, |ui| {
            ui.horizontal(|ui| {
                let tabs = std::iter::once((None, tr("Mikser").to_string(), ACCENT))
                    .chain(STRIPS.iter().map(|&(k, t, _, c)| (Some(k), t.to_string(), c)))
                    .chain(std::iter::once((Some("mic"), "MIC".to_string(), MIC_COL)));
                for (k, name, c) in tabs {
                    let sel = self.settings == k && !self.prefs_open;
                    let r = ui.add(egui::Button::new(RichText::new(name).size(14.0).color(if sel { c } else { WEAK })).frame(false));
                    if sel { ui.painter().hline(r.rect.x_range(), r.rect.bottom() + 2.0, egui::Stroke::new(2.0f32, c)); }
                    if r.clicked() { acts.push(Act::Open(k)); }
                    if k.is_none() { // separator between the mixer and the per-channel pages, like the line under MASTER
                        ui.add_space(4.0);
                        let (sr, _) = ui.allocate_exact_size(egui::vec2(1.0, 18.0), egui::Sense::hover());
                        ui.painter().vline(sr.center().x, sr.y_range(), egui::Stroke::new(1.0f32, IDLE));
                        ui.add_space(4.0);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if icon_btn(ui, 1, &tr("Ustawienia")) { acts.push(Act::Prefs(!self.prefs_open)); }
                    if icon_btn(ui, 0, &tr("Odśwież")) { acts.push(Act::Refresh); }
                    let on = self.mixer.stream_on;
                    if ui.add(egui::Button::new(RichText::new(if on { tr("TRYB STREAMERA: WŁĄCZONY") } else { tr("TRYB STREAMERA") }).strong().size(12.0)).fill(if on { ACCENT } else { IDLE }).min_size(egui::vec2(0.0, 24.0))).clicked() { acts.push(Act::StreamOn(!on)); }
                });
            });
        });
        egui::CentralPanel::default().frame(egui::Frame::none().fill(BG).inner_margin(egui::Margin::symmetric(14.0, 12.0))).show(ctx, |ui| {
            if self.prefs_open {
                self.settings_page(ui, &mut acts); // scrolls by itself
                return;
            }
            if let Some(k) = self.settings {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if k == "mic" { self.mic_page(ui, &mut acts); } else { self.channel_page(ui, k, &mut acts); }
                });
                return;
            }
            if self.sonar_missing() && !self.missing_hidden {
                // bottom-right popup; `none` = no Sonar.APO settings at all: GG/Sonar was never installed (first run)
                let none = self.chans.is_empty();
                egui::Area::new(egui::Id::new("sonar_missing")).anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-14.0, -14.0)).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).fill(Color32::from_rgb(86, 40, 36)).rounding(8.0).inner_margin(egui::Margin::same(14.0)).show(ui, |ui| {
                        ui.set_width(520.0f32.min(ui.ctx().screen_rect().width() - 60.0)); // wide enough that no line of either language wraps
                        ui.label(RichText::new(tr(if none { "Nie znaleziono SteelSeries Sonar!" } else { "Kanały Sonara nie istnieją w systemie" })).strong().size(15.0).color(Color32::from_rgb(255, 190, 170)));
                        ui.add_space(4.0);
                        let body = tr(if none { "Sonar Lite korzysta ze sterownika Sonara, który instaluje SteelSeries GG.\n1) Upewnij się, że SteelSeries GG jest zainstalowane i uruchomione przynajmniej raz,\n2) Zamknij GG całkowicie,\n3) Kliknij „Sprawdź ponownie” albo uruchom Sonar Lite ponownie.\nPotem GG nie jest już potrzebne i nie powinno działać razem z Sonar Lite.\nUpewnij się, że GG nie uruchamia się automatycznie z Windows." } else { "Windows nie widzi urządzeń „SteelSeries Sonar - Gaming/Chat/Media/Aux/Microphone”. GG wyłącza je przy zamknięciu, a Sonar Lite włącza je z powrotem sam. Jeśli ten komunikat nie znika, uruchom Sonar Lite jako administrator (wymaga tego włączanie urządzeń) i kliknij „Sprawdź ponownie”." });
                        for l in body.split('\n') { ui.label(RichText::new(l).color(Color32::from_rgb(235, 215, 208)).size(13.0)); ui.add_space(3.0); }
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui.button(tr("Sprawdź ponownie")).clicked() { acts.push(Act::Refresh); }
                            if none && ui.button(tr("Pobierz SteelSeries GG")).clicked() { let _ = std::process::Command::new("explorer").arg("https://steelseries.com/gg").spawn(); }
                            if ui.button(tr("Ukryj")).clicked() { acts.push(Act::HideMissing); }
                        });
                    });
                });
            }
            let gap = 8.0;
            let sep = 12.0; // extra room for the vertical line between MASTER and the channels
            let w = (ui.available_width() - 5.0 * gap - sep) / 6.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                let lay = egui::Layout::top_down(egui::Align::Min);
                let m = ui.allocate_ui_with_layout(egui::vec2(w, 0.0), lay, |ui| self.master_strip(ui, &mut acts)).response.rect;
                ui.painter().vline(m.right() + (gap + sep) / 2.0, m.y_range(), egui::Stroke::new(1.0f32, IDLE));
                ui.add_space(sep);
                for (k, t, i, c) in STRIPS { ui.allocate_ui_with_layout(egui::vec2(w, 0.0), lay, |ui| self.channel_strip(ui, k, t, i, c, &mut acts)); }
                ui.allocate_ui_with_layout(egui::vec2(w, 0.0), lay, |ui| self.mic_strip(ui, &mut acts));
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(w + gap + sep);
                self.chatmix(ui, 2.0 * w + gap, &mut acts);
            });
        });
        acts
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        sync_fonts(ctx);
        if ctx.input(|i| i.viewport().close_requested()) && self.has_tray && self.prefs.close_to_tray {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            hide();
        }
        if self.start_hidden && self.has_tray {
            // eframe shows the window after its first frame no matter what the builder said, so hide it again
            self.start_hidden = false;
            hide();
        }
        if let Some(path) = self.shot.clone() {
            self.frames += 1;
            if self.frames == 40 { ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot); }
            ctx.request_repaint();
            for e in ctx.input(|i| i.raw.events.clone()) {
                if let egui::Event::Screenshot { image, .. } = e { write_png(&path, &image); std::process::exit(0); }
            }
        }
        if self.rows_at.elapsed() > Duration::from_secs(3) {
            if self.sonar_missing() || vad::disabled_channel_endpoints(&self.hklm).len() > 0 {
                if self.ensure_vads() { self.reload(); }
            }
            self.refresh_devices();
            { let d = self.devs.lock().unwrap().clone(); if !d.0.is_empty() || !d.1.is_empty() { self.mic_inputs = d.0; self.mic_outputs = d.1; } }
            // things that could not start while the devices were missing start as soon as they are back
            if self.mic_cfg.enabled && self.mic.is_none() && self.mic_pending.is_none() && self.mic_outputs.iter().any(|o| o.contains(&self.mic_cfg.out)) { self.apply_mic(); }
            if self.mixer.stream_on && self.stream.is_none() && self.stream_pending.is_none() && !self.sonar_missing() { self.apply_stream(); }
            self.load_apps();
            if self.stream.is_some() { self.sync_stream_comp(); }
        }
        ctx.request_repaint_after(Duration::from_millis(if self.mic.is_some() { 60 } else { 3000 }));
        self.poll_workers();
        let acts = self.draw(ctx);
        self.apply(acts);
        self.flush_dirty(ctx);
    }
}

fn main() -> eframe::Result {
    // One instance only (two mic forwarders would fight over "Sonar - Microphone"). Starting the SAME build again just shows the
    // running window; starting a DIFFERENT build asks the old one to quit and takes over, so a rebuilt exe really replaces it.
    let diagnostic = std::env::args().any(|a| a == "--shot" || a == "--selftest"); // diagnostics may run next to a normal instance
    if !diagnostic {
        use windows::{core::{w, HSTRING}, Win32::{Foundation::*, System::Threading::*}};
        unsafe {
            let marker = HSTRING::from(format!(r"Local\SonarLiteBuild{}", env!("SONAR_BUILD")));
            let mut owner = CreateMutexW(None, false, w!(r"Local\SonarLiteSingleton")).ok();
            if GetLastError() == ERROR_ALREADY_EXISTS {
                if OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, &marker).is_ok() {
                    // the same build is already running (maybe hidden in the tray): show its window
                    if let Ok(ev) = OpenEventW(EVENT_MODIFY_STATE, false, w!(r"Local\SonarLiteShow")) { let _ = SetEvent(ev); }
                    return Ok(());
                }
                let Ok(quit) = OpenEventW(EVENT_MODIFY_STATE, false, w!(r"Local\SonarLiteQuit")) else {
                    // a build from before this mechanism existed: it cannot be replaced from here, so just show it
                    if let Ok(ev) = OpenEventW(EVENT_MODIFY_STATE, false, w!(r"Local\SonarLiteShow")) { let _ = SetEvent(ev); }
                    return Ok(());
                };
                let _ = SetEvent(quit);
                if let Some(h) = owner.take() { let _ = CloseHandle(h); } // we must not keep the mutex alive ourselves
                for _ in 0..40 {
                    std::thread::sleep(Duration::from_millis(150));
                    owner = CreateMutexW(None, false, w!(r"Local\SonarLiteSingleton")).ok();
                    if GetLastError() != ERROR_ALREADY_EXISTS { break; }
                    if let Some(h) = owner.take() { let _ = CloseHandle(h); }
                }
                if owner.is_none() { return Ok(()); } // the old instance would not go away
            }
            std::mem::forget(CreateMutexW(None, false, &marker)); // marks "this build is running"
        }
    }
    let tray_start = std::env::args().any(|a| a == "--tray");
    let shot = std::env::args().skip_while(|a| a != "--shot").nth(1);
    let mut app = App {
        hklm: RegKey::predef(HKEY_LOCAL_MACHINE), eps: vec![], chans: vec![], err: None, mixer: mixer::load(), settings: None, device_win: None, sel: 0,
        bands: vec![], eq_on: true, presets: vec![], mic_presets: vec![], rows: vec![], mic_apps: vec![], rows_at: Instant::now(), pol: apps::Policy::new().ok(), dirty: vec![], missing_hidden: false,
        mic_cfg: mic::load_cfg(), mic: None, mic_pending: None, mic_inputs: mic::inputs(), mic_outputs: vec![], devs: Default::default(), sh: stream::Shared::new(), stream: None, stream_pending: None, obs_hint: String::new(),
        autostart: autostart::enabled(), has_tray: false, _tray: None, tray_items: None, prefs: settings::load(), prefs_open: false,
        start_hidden: tray_start && shot.is_none(), shot, frames: 0,
    };
    i18n::set_lang(app.prefs.lang);
    // diagnostics: `--lang en` forces a language for this run only, `--prefs` opens the settings window
    if let Some(l) = std::env::args().skip_while(|a| a != "--lang").nth(1) { i18n::set_lang(i18n::Lang::from_code(&l)); app.prefs.lang = i18n::Lang::from_code(&l); }
    if std::env::args().any(|a| a == "--prefs") { app.prefs_open = true; }
    if let Some(n) = std::env::args().skip_while(|a| a != "--eqsel").nth(1).and_then(|n| n.parse::<i32>().ok()) { EQ_SEL_DEBUG.store(n, Relaxed); }
    app.mic_cfg.enabled = true; // the mic forwarder always runs; the speaker button mutes it
    levels::spawn();
    app.ensure_vads();
    app.reload();
    app.reset_apo_gain();
    for (k, ..) in STRIPS { app.write_gain(k); } // the faders live on the endpoints: put them where the saved mixer says
    if app.mic_cfg.device.is_empty() { app.mic_cfg.device = app.mic_inputs.first().cloned().unwrap_or_default(); }
    app.apply_stream();
    app.apply_mic();
    {
        // device lists for the pickers: enumerated in the background, the UI only reads the latest result
        let d = app.devs.clone();
        std::thread::spawn(move || loop {
            let r = (mic::inputs(), mic::outputs());
            *d.lock().unwrap() = r;
            std::thread::sleep(Duration::from_secs(4));
        });
    }
    // `--open <key>` opens a settings window at start (used for screenshots)
    if let Some(k) = std::env::args().skip_while(|a| a != "--open").nth(1) {
        app.settings = ["mic", "game", "chat", "media", "aux"].into_iter().find(|s| *s == k);
        if let Some(i) = app.settings.and_then(|k| app.idx(k)) { app.sel = i; app.load_eq(); }
    }
    if let Some(k) = std::env::args().skip_while(|a| a != "--opendev").nth(1) { app.device_win = ["game", "chat", "media", "aux"].into_iter().find(|s| *s == k); }
    if let Some(out) = std::env::args().skip_while(|a| a != "--bench").nth(1) { std::fs::write(out, bench(&mut app)).ok(); return Ok(()); }
    if let Some(out) = std::env::args().skip_while(|a| a != "--selftest").nth(1) { std::fs::write(out, selftest(&mut app)).ok(); return Ok(()); }
    let (px, n) = icon_rgba();
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Sonar Lite").with_inner_size([1320.0, 880.0]).with_min_inner_size([1100.0, 700.0]).with_decorations(false)
            .with_icon(egui::IconData { rgba: px, width: n, height: n }).with_visible(!tray_start || app.shot.is_some()),
        ..Default::default()
    };
    eframe::run_native("Sonar Lite", opts, Box::new(move |cc| {
        style(&cc.egui_ctx);
        sync_fonts(&cc.egui_ctx);
        watch_show(&cc.egui_ctx);
        watch_quit();
        if let Some((t, a, b)) = make_tray(&cc.egui_ctx) { app._tray = Some(t); app.tray_items = Some((a, b)); }
        app.has_tray = app._tray.is_some();
        Ok(Box::new(app))
    }))
}

/// `--selftest out.txt`: drive the real actions against the real registry and report what was written (then restore).
fn selftest(app: &mut App) -> String {
    let mut log = String::new();
    let i = app.idx("aux").expect("aux channel");
    let (orig_gain, orig_mute, orig_bands, orig_eq) = (app.chans[i].gain, app.chans[i].mute, app.chans[i].snapshot(&app.hklm), app.mixer.clone());
    let orig_outs: Vec<(String, &'static str)> = app.chans.iter().map(|c| (c.guid.clone(), c.key)).collect();
    let reg = |app: &App, k: &str| { let c = read_all(&app.hklm).into_iter().find(|c| c.key == k).unwrap(); (c.gain, c.mute) };
    let mut check = |name: &str, ok: bool, detail: String| log += &format!("{} {name}: {detail}\n", if ok { "PASS" } else { "FAIL" });
    // faders live on the endpoints' Windows volume (levels.rs applies it within ~30 ms; the device steps in 1.5 dB)
    let epg = |app: &App, k: &str| { std::thread::sleep(Duration::from_millis(200)); 1.0 / endpoint_comp(&sonar_guid(&app.eps, k).unwrap_or_default()) };
    app.mixer.master = 1.0; app.mixer.chatmix = 0.5;
    app.apply(vec![Act::Fader("aux", 0.5)]);
    let g = epg(app, "aux"); check("fader 50% -> endpoint volume", (g - 0.5).abs() < 0.03, format!("{g}"));
    app.apply(vec![Act::Master(0.5)]);
    let g = epg(app, "aux"); check("master 50% moves the faders too: aux 50% -> 25%", (g - 0.25).abs() < 0.03 && (app.mixer.base["aux"] - 0.25).abs() < 1e-6, format!("{g}"));
    app.apply(vec![Act::ChatMix(1.0)]);
    let (gg, cg) = (epg(app, "game"), epg(app, "chat"));
    check("chatmix fully on CHAT: game ~0, chat = its fader", gg <= 0.011 && (cg - app.mixer.base["chat"]).abs() < 0.03, format!("game {gg} chat {cg}"));
    app.apply(vec![Act::Master(1.0), Act::ChatMix(0.5)]);
    let (gg, cg) = (epg(app, "game"), epg(app, "chat"));
    check("master 100% + chatmix centre: game/chat back to their faders", (gg - app.mixer.base["game"]).abs() < 0.03 && (cg - app.mixer.base["chat"]).abs() < 0.03, format!("game {gg} chat {cg}"));
    app.apply(vec![Act::Mute("aux", true)]);
    check("mute aux", reg(app, "aux").1, String::new());
    app.apply(vec![Act::Mute("aux", false)]);
    check("unmute aux", !reg(app, "aux").1, String::new());
    app.apply(vec![Act::Preset("aux", "Bass Boost".into())]);
    let b = app.chans[i].band(&app.hklm, 0);
    check("preset Bass Boost -> band 1 low shelf +6 dB @100 Hz", b.kind == 8 && (b.gain - 6.0).abs() < 1e-4 && (b.freq - 100.0).abs() < 1e-3, format!("type {} {} Hz {} dB", b.kind, b.freq, b.gain));
    check("preset remembered for the channel", app.mixer.eq.get("aux").map(String::as_str) == Some("Bass Boost"), format!("{:?}", app.mixer.eq.get("aux")));
    app.apply(vec![Act::SFader("media", 0.4), Act::SMaster(0.5)]);
    let sg = f32::from_bits(app.sh.gain[2].load(Relaxed)); check("stream fader 40% x stream master 50% -> 20% (media)", (sg - 0.2).abs() < 1e-6, format!("{sg}"));
    app.apply(vec![Act::SMute("media", true)]);
    check("stream mute media", f32::from_bits(app.sh.gain[2].load(Relaxed)) == 0.0, String::new());
    let chat_i = app.idx("chat").unwrap();
    let before = read_all(&app.hklm)[chat_i].guid.clone();
    let out_guid = app.eps.iter().find(|e| !e.virt && e.name.contains("Słuchawki")).map(|e| e.guid.clone());
    if let Some(g) = out_guid {
        app.apply(vec![Act::Out("chat", g.clone())]);
        let k = app.hklm.open_subkey(r"SOFTWARE\SteelSeries ApS\Sonar.APO\ChatRender\Settings\GlobalControl\Store").unwrap();
        let st: u32 = k.get_value("kSet_CaptureStreamRedirectionState").unwrap_or(99);
        let dev: Vec<u8> = k.get_raw_value("kSet_CaptureStreamRedirectionDeviceId").map(|v| v.bytes).unwrap_or_default();
        check("chat output switch moves the main Chat route (CaptureStreamRedirection) too", st == 1 && sonar_lite::guid_of(&dev).as_deref() == Some(g.as_str()), format!("state {st}, device {:?}", sonar_lite::guid_of(&dev)));
        let _ = before;
        app.apply(vec![Act::AllOut(g.clone())]);
        let all: Vec<String> = read_all(&app.hklm).into_iter().map(|c| c.guid).collect();
        check("monitor selector sets every channel's output", all.len() == 4 && all.iter().all(|x| *x == g), format!("{all:?}"));
    }
    // device switching with linked channels and a priority list that has the OTHER device on top (the situation seen in real use)
    {
        let hp = app.eps.iter().find(|e| !e.virt && e.name.contains("Słuchawki")).map(|e| (e.guid.clone(), e.name.clone()));
        let mon = app.eps.iter().find(|e| !e.virt && e.name.contains("G27QC")).map(|e| (e.guid.clone(), e.name.clone()));
        if let (Some(hp), Some(mon)) = (hp, mon) {
            app.mixer.link_all = true;
            for (k, ..) in STRIPS { app.mixer.prio.insert(k.to_string(), vec![mon.clone(), hp.clone()]); }
            app.apply(vec![Act::SetDevice("game", hp.0.clone(), hp.1.clone())]);
            let outs = |app: &App| read_all(&app.hklm).into_iter().map(|c| c.guid).collect::<Vec<_>>();
            check("link on: choosing headphones for GAME moves all four channels", outs(app).iter().all(|g| *g == hp.0), format!("{:?}", outs(app)));
            check("...and puts headphones first in every channel's priority list", STRIPS.iter().all(|s| app.mixer.prio[s.0].first().map(|x| x.0.as_str()) == Some(hp.0.as_str())), String::new());
            app.refresh_devices(); // what the 3-second tick does
            check("...and the next refresh does not revert it", outs(app).iter().all(|g| *g == hp.0), format!("{:?}", outs(app)));
            app.apply(vec![Act::SetDevice("chat", mon.0.clone(), mon.1.clone())]);
            app.refresh_devices();
            check("choosing the monitor for CHAT moves everything back and sticks", outs(app).iter().all(|g| *g == mon.0), format!("{:?}", outs(app)));
        }
    }
    // restore everything we touched
    app.mixer = orig_eq;
    mixer::save(&app.mixer);
    let _ = orig_gain; // the APO gain is not touched any more (the faders are the endpoint volumes)
    let _ = app.chans[i].set_mute(&app.hklm, orig_mute);
    let _ = app.chans[i].apply(&app.hklm, &orig_bands);
    for (g, k) in &orig_outs { if let Some(j) = app.idx(k) { let _ = app.chans[j].set_out(&app.hklm, g); } } // outputs are restored too
    for (k, ..) in STRIPS { app.write_gain(k); }
    log += &format!("restored: aux volume {:.2} mute {}\n", epg(app, "aux"), reg(app, "aux").1);
    log
}

/// `--bench out.txt`: how long the things the UI thread does every few seconds take (ms, average of 10 runs).
fn bench(app: &mut App) -> String {
    let mut out = String::new();
    let mut t = |name: &str, f: &mut dyn FnMut()| {
        f(); // warm up
        let t0 = Instant::now();
        for _ in 0..10 { f(); }
        out += &format!("{:>7.1} ms  {name}\n", t0.elapsed().as_secs_f64() * 100.0);
    };
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    t("endpoints() (registry scan of render devices)", &mut || { let _ = endpoints(&hklm); });
    t("read_all() (channel settings)", &mut || { let _ = read_all(&hklm); });
    t("vad::disabled_channel_endpoints() (registry scan, render+capture)", &mut || { let _ = vad::disabled_channel_endpoints(&hklm); });
    t("apps::sessions() (COM: every endpoint's audio sessions)", &mut || { let _ = apps::sessions(); });
    t("apps::default_render_guid()", &mut || { let _ = apps::default_render_guid(); });
    t("mic::inputs() (cpal enumeration)", &mut || { let _ = mic::inputs(); });
    t("mic::outputs() (cpal enumeration)", &mut || { let _ = mic::outputs(); });
    t("sync_stream_comp() (endpoint volume COM x4)", &mut || { app.sync_stream_comp(); });
    t("load_apps() (sessions + per-process policy lookups)", &mut || { app.load_apps(); });
    t("refresh_devices() (endpoints + read_all + fallback check)", &mut || { app.refresh_devices(); });
    t("reload() (everything above + EQ)", &mut || { app.reload(); });
    t("eq::list_presets() (disk)", &mut || { let _ = eq::list_presets(); });
    t("autostart::enabled() (spawns schtasks.exe)", &mut || { let _ = autostart::enabled(); });
    // cost of drawing one frame (egui layout + painting into shapes, no GPU), main view and with each window open
    let ctx = egui::Context::default();
    style(&ctx);
    let frame = |app: &App| {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1320.0, 880.0))), ..Default::default() };
        let _ = ctx.run(input, |c| { let _ = app.draw(c); });
    };
    app.settings = None; app.device_win = None;
    t("draw: main view (one frame)", &mut || frame(app));
    app.settings = Some("game"); app.sel = app.idx("game").unwrap_or(0); app.load_eq();
    t("draw: main + GAME advanced window (EQ graph)", &mut || frame(app));
    app.settings = Some("mic");
    t("draw: main + MIC window (graph, meters)", &mut || frame(app));
    app.settings = None; app.device_win = Some("game");
    t("draw: main + device window", &mut || frame(app));
    out
}
