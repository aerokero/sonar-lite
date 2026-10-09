// Microphone channel. GG used to feed "SteelSeries Sonar - Microphone"; without it we do that ourselves:
// capture the chosen physical mic, clean it up, and play it on the "Sonar - Microphone" render endpoint, which the Sonar driver
// loops back to the "Sonar - Microphone" capture device that apps (Discord, games) use. Runs only while Sonar Lite is running.
// Chain: mono -> RNNoise denoiser (optional, 48 kHz) -> noise gate (optional, manual or automatic threshold) -> compressor (optional)
//        -> EQ -> volume -> (resample) -> output.
use crate::eq::{self, Band};
use crate::stream;
use cpal::traits::*;
use nnnoiseless::{DenoiseState, FRAME_SIZE};
use std::{
    collections::VecDeque,
    fs,
    path::PathBuf,
    sync::{atomic::*, Arc, Mutex},
};

/// Simple noise gate: opens above the threshold, closes 3 dB lower after a 100 ms hold (hysteresis), gain ramps to avoid clicks.
struct Gate { env: f32, hold: u32, g: f32, open: bool, env_rel: f32, att: f32, rel: f32, hold_n: u32, floor: f32 }

impl Gate {
    fn new(sr: f32) -> Self {
        Gate { env: 0.0, hold: 0, g: 1.0, open: true, env_rel: (-1.0 / (0.01 * sr)).exp(), att: 1.0 - (-1.0 / (0.005 * sr)).exp(),
               rel: 1.0 - (-1.0 / (0.15 * sr)).exp(), hold_n: (0.1 * sr) as u32, floor: 0.01 }
    }
    fn reset(&mut self) { self.g = 1.0; self.open = true; }
    /// `a` = sample magnitude, `thr` = linear threshold (ignored when `auto`); returns the gain to apply.
    /// Auto mode follows the noise floor (minimum of the envelope, creeping up ~2 dB/s so pauses in speech re-measure it)
    /// and opens 10 dB above it, limited to -65..-25 dBFS.
    fn step(&mut self, a: f32, thr: f32, auto: bool) -> f32 {
        self.env = a.max(self.env * self.env_rel);
        self.floor = self.env.min(self.floor * 1.000004).max(1e-5);
        let thr = if auto { (self.floor * 3.16).clamp(0.000562, 0.0562) } else { thr };
        if self.env > if self.open { thr * 0.7 } else { thr } { self.open = true; self.hold = self.hold_n } else if self.hold > 0 { self.hold -= 1 } else { self.open = false }
        self.g += (self.open as u8 as f32 - self.g) * if self.open { self.att } else { self.rel };
        self.g
    }
}

/// Feed-forward compressor, 4:1 with half automatic make-up gain. `level` 0..1 moves the threshold from 0 to -30 dB.
struct Comp { env: f32, g: f32, env_rel: f32, att: f32, rel: f32 }

impl Comp {
    fn new(sr: f32) -> Self {
        Comp { env: 0.0, g: 1.0, env_rel: (-1.0 / (0.02 * sr)).exp(), att: 1.0 - (-1.0 / (0.005 * sr)).exp(), rel: 1.0 - (-1.0 / (0.12 * sr)).exp() }
    }
    fn step(&mut self, x: f32, level: f32) -> f32 {
        if level < 0.005 { self.g = 1.0; return x; }
        self.env = x.abs().max(self.env * self.env_rel);
        let thr_db = -30.0 * level;
        let over = (20.0 * self.env.max(1e-6).log10() - thr_db).max(0.0);
        let target = 10f32.powf(-over * 0.75 / 20.0); // ratio 4:1 -> removes 3/4 of what exceeds the threshold
        self.g += (target - self.g) * if target < self.g { self.att } else { self.rel };
        x * self.g * 10f32.powf(-thr_db * 0.75 * 0.5 / 20.0)
    }
}

/// Direct-form-II-transposed biquad (state in f64 so low-frequency shelves stay accurate).
struct Biquad { b: [f64; 3], a: [f64; 2], z: [f64; 2] }

impl Biquad {
    fn new(c: [f64; 6]) -> Self { Biquad { b: [c[0] / c[3], c[1] / c[3], c[2] / c[3]], a: [c[4] / c[3], c[5] / c[3]], z: [0.0; 2] } }
    fn step(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// Settings shared between the UI thread and the audio callback.
pub struct Params {
    pub gain: AtomicU32,
    pub mute: AtomicBool,
    pub gate_on: AtomicBool,
    pub gate_db: AtomicU32,
    pub gate_auto: AtomicBool,
    pub comp_on: AtomicBool,
    pub comp_level: AtomicU32,
    pub nc_on: AtomicBool,
    /// denoiser strength 0..1 (wet/dry mix)
    pub nc_mix: AtomicU32,
    /// bumped whenever `eq` changes so the callback rebuilds its filters
    pub eq_ver: AtomicU32,
    pub eq: Mutex<Vec<Band>>,
    /// raw input peak (before any processing)
    pub peak: AtomicU32,
    /// true while the gate lets sound through
    pub open: AtomicBool,
    /// speech probability of the last denoiser frame
    pub vad: AtomicU32,
}

impl Params {
    /// EQ of the current settings: nothing when switched off, the hand-edited bands if there are any, else the named preset.
    pub fn set_eq_cfg(&self, c: &Cfg) {
        *self.eq.lock().unwrap() = if !c.eq_on { vec![] } else { c.bands.clone().or_else(|| eq::builtin(&c.eq)).unwrap_or_default() };
        self.eq_ver.fetch_add(1, Ordering::Relaxed);
    }
    pub fn apply(&self, c: &Cfg) {
        self.gain.store(c.gain.to_bits(), Ordering::Relaxed);
        self.mute.store(c.mute, Ordering::Relaxed);
        self.gate_on.store(c.gate, Ordering::Relaxed);
        self.gate_db.store(c.gate_db.to_bits(), Ordering::Relaxed);
        self.gate_auto.store(c.gate_auto, Ordering::Relaxed);
        self.comp_on.store(c.comp, Ordering::Relaxed);
        self.comp_level.store(c.comp_level.to_bits(), Ordering::Relaxed);
        self.nc_on.store(c.nc, Ordering::Relaxed);
        self.nc_mix.store((c.nc_pct / 100.0).clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    fn new(c: &Cfg) -> Self {
        let p = Params {
            gain: AtomicU32::new(0), mute: AtomicBool::new(false), gate_on: AtomicBool::new(false), gate_db: AtomicU32::new(0), gate_auto: AtomicBool::new(false),
            comp_on: AtomicBool::new(false), comp_level: AtomicU32::new(0), nc_on: AtomicBool::new(false),
            nc_mix: AtomicU32::new(0), eq_ver: AtomicU32::new(0), eq: Mutex::new(vec![]), peak: AtomicU32::new(0), open: AtomicBool::new(true), vad: AtomicU32::new(0),
        };
        p.apply(c);
        p.set_eq_cfg(c);
        p
    }
}

pub struct Mic {
    _i: cpal::Stream,
    _o: cpal::Stream,
    pub p: Arc<Params>,
}

#[derive(Clone)]
pub struct Cfg {
    pub enabled: bool,
    pub device: String,
    pub gain: f32,
    pub mute: bool,
    pub gate: bool,
    pub gate_db: f32,
    pub nc: bool,
    pub nc_pct: f32,
    /// built-in EQ preset name
    pub eq: String,
    pub eq_on: bool,
    /// hand-edited EQ bands; when present they replace the preset
    pub bands: Option<Vec<Band>>,
    pub gate_auto: bool,
    pub comp: bool,
    pub comp_level: f32,
    /// output device (substring of its name) the processed mic is played on; apps record its capture twin
    pub out: String,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg { enabled: false, device: String::new(), gain: 1.0, mute: false, gate: false, gate_db: -45.0, nc: false, nc_pct: 100.0, eq: "Flat".into(), out: "Sonar - Microphone".into(),
            eq_on: true, bands: None, gate_auto: false, comp: false, comp_level: 0.4 }
    }
}

fn cfg_path() -> PathBuf { PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("SonarLite").join("mic.txt") }

pub fn load_cfg() -> Cfg { parse_cfg(&fs::read_to_string(cfg_path()).unwrap_or_default()) }

fn parse_cfg(text: &str) -> Cfg {
    let mut c = Cfg::default();
    for l in text.lines() {
        match l.split_once('=') {
            Some(("enabled", v)) => c.enabled = v == "1",
            Some(("device", v)) => c.device = v.to_string(),
            Some(("gain", v)) => c.gain = v.parse().unwrap_or(1.0),
            Some(("mute", v)) => c.mute = v == "1",
            Some(("gate", v)) => c.gate = v == "1",
            Some(("gate_db", v)) => c.gate_db = v.parse().unwrap_or(-45.0),
            Some(("nc", v)) => c.nc = v == "1",
            Some(("nc_pct", v)) => c.nc_pct = v.parse().unwrap_or(100.0),
            Some(("eq", v)) => c.eq = v.to_string(),
            Some(("out", v)) => c.out = v.to_string(),
            Some(("eq_on", v)) => c.eq_on = v == "1",
            Some(("gate_auto", v)) => c.gate_auto = v == "1",
            Some(("comp", v)) => c.comp = v == "1",
            Some(("comp_level", v)) => c.comp_level = v.parse().unwrap_or(0.4),
            Some(("eqb", v)) => {
                let b: Vec<Band> = v.split(';').filter_map(|e| {
                    let f: Vec<&str> = e.split(':').collect();
                    Some(Band { kind: f.first()?.parse().ok()?, freq: f.get(1)?.parse().ok()?, gain: f.get(2)?.parse().ok()?, q: f.get(3)?.parse().ok()?, on: *f.get(4)? == "1" })
                }).collect();
                if b.len() == 10 { c.bands = Some(b); }
            }
            _ => {}
        }
    }
    c
}

fn serialize_cfg(c: &Cfg) -> String {
    let mut s = format!("enabled={}\ndevice={}\ngain={}\nmute={}\ngate={}\ngate_db={}\nnc={}\nnc_pct={}\neq={}\nout={}\n",
        c.enabled as u8, c.device, c.gain, c.mute as u8, c.gate as u8, c.gate_db, c.nc as u8, c.nc_pct, c.eq, c.out);
    s += &format!("eq_on={}\ngate_auto={}\ncomp={}\ncomp_level={}\n", c.eq_on as u8, c.gate_auto as u8, c.comp as u8, c.comp_level);
    if let Some(b) = &c.bands {
        s += &format!("eqb={}\n", b.iter().map(|x| format!("{}:{}:{}:{}:{}", x.kind, x.freq, x.gain, x.q, x.on as u8)).collect::<Vec<_>>().join(";"));
    }
    s
}

pub fn save_cfg(c: &Cfg) {
    let _ = fs::create_dir_all(cfg_path().parent().unwrap());
    let _ = fs::write(cfg_path(), serialize_cfg(c));
}

/// Capture devices that can feed the mic chain: physical mics and virtual cables (Sonar's own mic is excluded - it would feed back).
pub fn inputs() -> Vec<String> {
    cpal::default_host().input_devices().map(|d| d.filter_map(|d| d.name().ok()).filter(|n| !n.contains("Sonar")).collect()).unwrap_or_default()
}

/// Render devices the processed mic can be played on: Sonar's virtual mic (default) and any normal device or virtual cable.
pub fn outputs() -> Vec<String> {
    cpal::default_host().output_devices().map(|d| {
        d.filter_map(|d| d.name().ok()).filter(|n| !n.contains("Sonar") || n.contains("Sonar - Microphone")).collect()
    }).unwrap_or_default()
}

/// The processing chain; fed one mono sample at a time, hands back finished samples.
struct Chain {
    sr: f32,
    gate: Gate,
    comp: Comp,
    eq: Vec<Biquad>,
    eq_ver: u32,
    den: Box<DenoiseState<'static>>,
    acc: [f32; FRAME_SIZE],
    out: [f32; FRAME_SIZE],
    n: usize,
    ready: VecDeque<f32>,
}

impl Chain {
    fn new(sr: f32) -> Self {
        Chain { sr, gate: Gate::new(sr), comp: Comp::new(sr), eq: vec![], eq_ver: u32::MAX, den: DenoiseState::new(), acc: [0.0; FRAME_SIZE], out: [0.0; FRAME_SIZE], n: 0, ready: VecDeque::new() }
    }

    /// Stage 1: denoiser (works in 10 ms frames, so output lags by up to a frame while it is on).
    fn push(&mut self, x: f32, nc: bool, mix: f32, p: &Params) {
        if nc && (self.sr - 48000.0).abs() < 1.0 {
            self.acc[self.n] = x * 32768.0; // RNNoise works on 16-bit-scaled floats
            self.n += 1;
            if self.n == FRAME_SIZE {
                let vad = self.den.process_frame(&mut self.out, &self.acc);
                p.vad.store(vad.to_bits(), Ordering::Relaxed);
                for i in 0..FRAME_SIZE {
                    let (wet, dry) = (self.out[i] / 32768.0, self.acc[i] / 32768.0);
                    self.ready.push_back(dry + (wet - dry) * mix);
                }
                self.n = 0;
            }
        } else {
            for i in 0..self.n { self.ready.push_back(self.acc[i] / 32768.0); } // denoiser just switched off: don't lose the partial frame
            self.n = 0;
            self.ready.push_back(x);
        }
    }

    /// Stage 2: gate and EQ (volume is applied by the caller so the stream mix can use the pre-volume signal).
    fn post(&mut self, x: f32, p: &Params) -> f32 {
        let mut y = x;
        if p.gate_on.load(Ordering::Relaxed) {
            y *= self.gate.step(y.abs(), 10f32.powf(f32::from_bits(p.gate_db.load(Ordering::Relaxed)) / 20.0), p.gate_auto.load(Ordering::Relaxed));
        } else {
            self.gate.reset();
        }
        p.open.store(self.gate.open, Ordering::Relaxed);
        if p.comp_on.load(Ordering::Relaxed) { y = self.comp.step(y, f32::from_bits(p.comp_level.load(Ordering::Relaxed))); } else { self.comp.g = 1.0; }
        let v = p.eq_ver.load(Ordering::Relaxed);
        if v != self.eq_ver {
            self.eq_ver = v;
            self.eq = p.eq.lock().unwrap().iter().filter(|b| b.on && b.gain.abs() > 0.05).map(|b| Biquad::new(eq::coeffs(b, self.sr as f64))).collect();
        }
        let mut d = y as f64;
        for b in &mut self.eq { d = b.step(d); }
        d as f32
    }
}

/// A mic forwarder running on its own thread. cpal streams are not `Send` and building them takes hundreds of milliseconds
/// (device enumeration), so the UI must not do it: it asks `spawn` and polls the receiver. Dropping `MicRun` stops the forwarder.
pub struct MicRun {
    pub p: Arc<Params>,
    _stop: std::sync::mpsc::Sender<()>,
}

pub fn spawn(input: String, cfg: Cfg, bus: Option<Arc<stream::Shared>>) -> std::sync::mpsc::Receiver<Result<MicRun, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || match start(&input, &cfg, bus) {
        Ok(m) => {
            let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
            let _ = tx.send(Ok(MicRun { p: m.p.clone(), _stop: stop_tx }));
            let _ = stop_rx.recv(); // returns when the MicRun is dropped
            drop(m);
        }
        Err(e) => { let _ = tx.send(Err(e)); }
    });
    rx
}

pub fn start(input: &str, cfg: &Cfg, bus: Option<Arc<stream::Shared>>) -> Result<Mic, String> {
    let host = cpal::default_host();
    let e = |e: &dyn std::fmt::Display| e.to_string();
    let inp = host.input_devices().map_err(|x| e(&x))?.find(|d| d.name().ok().as_deref() == Some(input)).ok_or_else(|| crate::i18n::tr("nie znaleziono wejścia"))?;
    let out = host.output_devices().map_err(|x| e(&x))?
        .find(|d| d.name().map_or(false, |n| n.contains(&cfg.out))).ok_or_else(|| crate::i18n::tr_fmt("brak wyjścia „{}”", &[cfg.out.clone()]))?;
    let (ic, oc) = (inp.default_input_config().map_err(|x| e(&x))?, out.default_output_config().map_err(|x| e(&x))?);
    if oc.channels() != 2 { return Err(crate::i18n::tr("wyjście mikrofonu musi być stereo")); }
    if ic.sample_format() != cpal::SampleFormat::F32 || oc.sample_format() != cpal::SampleFormat::F32 { return Err(crate::i18n::tr("wymagany format f32")); }

    let p = Arc::new(Params::new(cfg));
    let buf = Arc::new(Mutex::new(VecDeque::<f32>::new()));
    let (nch, ratio) = (ic.channels() as usize, ic.sample_rate().0 as f64 / oc.sample_rate().0 as f64);
    let cap = oc.sample_rate().0 as usize * 2 / 10; // ~100 ms of stereo samples, oldest dropped on overrun
    let (w, pp) = (buf.clone(), p.clone());
    let mut chain = Chain::new(ic.sample_rate().0 as f32);
    let (mut pos, mut last) = (0.0f64, 0.0f32);
    let _i = inp.build_input_stream(&ic.config(), move |d: &[f32], _| {
        let (nc, mix) = (pp.nc_on.load(Ordering::Relaxed), f32::from_bits(pp.nc_mix.load(Ordering::Relaxed)));
        let mut pk = 0f32;
        for f in d.chunks_exact(nch) {
            let m = if nch == 1 { f[0] } else { (f[0] + f[1]) * 0.5 };
            pk = pk.max(m.abs());
            chain.push(m, nc, mix, &pp);
        }
        pp.peak.store(pk.to_bits(), Ordering::Relaxed);
        let mut b = w.lock().unwrap();
        let k = if pp.mute.load(Ordering::Relaxed) { 0.0 } else { f32::from_bits(pp.gain.load(Ordering::Relaxed)) };
        while let Some(x) = chain.ready.pop_front() {
            let pre = chain.post(x, &pp);
            if let Some(bus) = bus.as_ref().filter(|b| b.mic_on.load(Ordering::Relaxed)) {
                let mut m = bus.mic.lock().unwrap();
                m.push_back(pre);
                m.push_back(pre);
                while m.len() > 9600 { m.pop_front(); }
            }
            let y = pre * k;
            // ponytail: linear resampler (rates are normally both 48 kHz); swap for rubato if aliasing is audible
            pos += 1.0;
            while pos >= ratio {
                pos -= ratio;
                let t = (1.0 - pos / ratio) as f32;
                let s = last + (y - last) * t;
                b.push_back(s);
                b.push_back(s);
            }
            last = y;
        }
        while b.len() > cap { b.pop_front(); }
    }, |e| eprintln!("mic in: {e}"), None).map_err(|x| e(&x))?;
    let r = buf.clone();
    let _o = out.build_output_stream(&oc.config(), move |d: &mut [f32], _| {
        let mut b = r.lock().unwrap();
        for s in d { *s = b.pop_front().unwrap_or(0.0); }
    }, |e| eprintln!("mic out: {e}"), None).map_err(|x| e(&x))?;
    _i.play().map_err(|x| e(&x))?;
    _o.play().map_err(|x| e(&x))?;
    Ok(Mic { _i, _o, p })
}

#[cfg(test)]
mod t {
    use super::*;

    #[test]
    fn gate_closes_on_quiet_opens_on_loud_and_holds() {
        let sr = 48000.0;
        let thr = 10f32.powf(-45.0 / 20.0);
        let mut g = Gate::new(sr);
        let run = |g: &mut Gate, a: f32, n: usize| (0..n).map(|_| g.step(a, thr, false)).last().unwrap();
        assert!(run(&mut g, 0.3, 4800) > 0.99); // loud: open
        assert!(run(&mut g, 0.0, 2000) > 0.9); // hold (100 ms = 4800 samples) keeps it open
        assert!(run(&mut g, 0.0, 48000) < 0.01); // long silence: closed
        assert!(run(&mut g, 0.3, 2400) > 0.99); // loud again: reopens fast
    }

    #[test]
    fn config_survives_save_and_load() {
        let bands = eq::builtin("Podcast");
        let c = Cfg { enabled: true, device: "Mic (X)".into(), gain: 1.5, mute: true, gate: true, gate_db: -50.0, nc: true, nc_pct: 70.0, eq: "Podcast".into(), out: "Virtual Audio Cable".into(),
                      eq_on: false, bands: bands.clone(), gate_auto: true, comp: true, comp_level: 0.7 };
        let r = parse_cfg(&serialize_cfg(&c));
        assert!(r.enabled && r.device == "Mic (X)" && r.gain == 1.5 && r.mute && r.gate && r.gate_db == -50.0 && r.nc && r.nc_pct == 70.0 && r.eq == "Podcast" && r.out == "Virtual Audio Cable");
        assert!(!r.eq_on && r.gate_auto && r.comp && r.comp_level == 0.7);
        let (a, b) = (r.bands.unwrap(), bands.unwrap());
        assert!(a.len() == 10 && a.iter().zip(&b).all(|(x, y)| x.kind == y.kind && x.freq == y.freq && x.gain == y.gain && x.q == y.q && x.on == y.on));
    }

    #[test]
    fn auto_gate_learns_the_noise_floor_and_opens_for_speech() {
        let sr = 48000.0;
        let mut g = Gate::new(sr);
        let mut seed = 7u32;
        let mut noise = |amp: f32| { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); amp * ((seed >> 8) as f32 / 8388608.0 - 1.0) };
        let mut last = 1.0;
        for _ in 0..(sr as usize * 3) { last = g.step(noise(0.003).abs(), 0.0, true); } // 3 s of room noise (about -50 dBFS)
        assert!(last < 0.05, "gate should be closed on noise, gain {last}");
        let mut opened = 0.0;
        for _ in 0..(sr as usize / 10) { opened = g.step(noise(0.2).abs() + 0.05, 0.0, true); } // speech at about -14 dBFS
        assert!(opened > 0.95, "gate should open for speech, gain {opened}");
    }

    #[test]
    fn compressor_reduces_loud_signals_and_does_nothing_at_level_zero() {
        let sr = 48000.0;
        let sine = |i: usize, amp: f32| amp * (std::f32::consts::TAU * 440.0 * i as f32 / sr).sin();
        let peak = |c: &mut Comp, amp: f32, level: f32| (0..48000).map(|i| c.step(sine(i, amp), level)).skip(24000).fold(0f32, |m, v| m.max(v.abs()));
        assert!((peak(&mut Comp::new(sr), 0.5, 0.0) - 0.5).abs() < 1e-4); // off: untouched
        let loud = peak(&mut Comp::new(sr), 0.5, 1.0); // -6 dBFS into a -30 dB threshold
        assert!(loud < 0.5 * 0.9 && loud > 0.05, "got {loud}"); // clearly reduced, not crushed
        let quiet = peak(&mut Comp::new(sr), 0.005, 1.0); // -46 dBFS, below threshold: only the make-up gain
        assert!(quiet > 0.005 && quiet < 0.005 * 5.0, "got {quiet}");
    }

    #[test]
    fn biquad_gain_at_centre_matches_the_band() {
        let b = Band { on: true, kind: 7, freq: 1000.0, gain: 6.0, q: 1.0 };
        let mut f = Biquad::new(eq::coeffs(&b, 48000.0));
        let y: Vec<f64> = (0..48000).map(|i| f.step((std::f64::consts::TAU * 1000.0 * i as f64 / 48000.0).sin())).collect();
        let peak = y[24000..].iter().fold(0f64, |m, v| m.max(v.abs()));
        assert!((20.0 * peak.log10() - 6.0).abs() < 0.1, "got {} dB", 20.0 * peak.log10());
    }

    #[test]
    fn denoiser_suppresses_steady_noise() {
        let p = Params::new(&Cfg::default());
        let mut c = Chain::new(48000.0);
        let mut seed = 12345u32;
        let noise = |s: &mut u32| { *s = s.wrapping_mul(1664525).wrapping_add(1013904223); (*s >> 8) as f32 / 8388608.0 - 1.0 };
        let (mut e_in, mut e_out, mut lp) = (0f64, 0f64, 0f32);
        for i in 0..96000 {
            lp = 0.9 * lp + 0.1 * noise(&mut seed); // low-passed, like fan rumble / room tone
            let x = 0.1 * lp;
            c.push(x, true, 1.0, &p);
            while let Some(y) = c.ready.pop_front() { if i > 48000 { e_out += (y * y) as f64; e_in += (x * x) as f64; } }
        }
        assert!(e_out < e_in * 0.1, "noise energy only fell to {:.0}%", 100.0 * e_out / e_in);
    }
}
