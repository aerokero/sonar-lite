// Streamer mode: a second, independent mix for OBS/Discord screen-share etc. ("Sonar Stream").
// Sonar.APO's own second route does not work without GG, so we build the stream mix ourselves: every Sonar channel endpoint is
// tapped with a WASAPI loopback (raw app audio, scaled by the endpoint's Windows volume), run through our own EQ, scaled by the
// channel's *stream* fader, summed together with the microphone and played on a virtual cable that OBS records.
use crate::eq::{self, Band};
use cpal::traits::*;
use std::{
    collections::VecDeque,
    sync::{atomic::*, Arc, Mutex},
};

pub const KEYS: [&str; 4] = ["game", "chat", "media", "aux"];
const ENDPOINT: [&str; 4] = ["Sonar - Gaming", "Sonar - Chat", "Sonar - Media", "Sonar - Aux"];

/// State shared between the UI thread and the audio callbacks. Index 0..4 = KEYS, 4 = microphone.
pub struct Shared {
    /// final linear stream gain per source (fader * master, 0 when muted)
    pub gain: [AtomicU32; 5],
    /// undo of each endpoint's Windows volume (loopback is scaled by it)
    pub comp: [AtomicU32; 4],
    pub eq_ver: [AtomicU32; 4],
    pub eq: [Mutex<Vec<Band>>; 4],
    /// peak per source (before the stream gain) for the UI meters
    pub peak: [AtomicU32; 5],
    /// processed microphone samples (stereo interleaved) pushed by the mic chain
    pub mic: Mutex<VecDeque<f32>>,
    pub mic_on: AtomicBool,
}

impl Shared {
    pub fn new() -> Arc<Self> {
        let one = || AtomicU32::new(1f32.to_bits());
        Arc::new(Shared {
            gain: [one(), one(), one(), one(), one()], comp: [one(), one(), one(), one()],
            eq_ver: [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)],
            eq: [Mutex::new(vec![]), Mutex::new(vec![]), Mutex::new(vec![]), Mutex::new(vec![])],
            peak: [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)],
            mic: Mutex::new(VecDeque::new()),
            mic_on: AtomicBool::new(false),
        })
    }
    pub fn set_gain(&self, i: usize, g: f32) { self.gain[i].store(g.to_bits(), Ordering::Relaxed); }
    pub fn set_comp(&self, i: usize, c: f32) { self.comp[i].store(c.to_bits(), Ordering::Relaxed); }
    /// `bands` empty = no EQ (flat / disabled)
    pub fn set_eq(&self, i: usize, bands: Vec<Band>) {
        *self.eq[i].lock().unwrap() = bands;
        self.eq_ver[i].fetch_add(1, Ordering::Relaxed);
    }
}

pub struct Stream {
    _s: Vec<cpal::Stream>,
    pub sh: Arc<Shared>,
}

/// Transposed direct-form II biquad.
struct Bq { b: [f64; 3], a: [f64; 2], z: [f64; 2] }

impl Bq {
    fn new(c: [f64; 6]) -> Self { Bq { b: [c[0] / c[3], c[1] / c[3], c[2] / c[3]], a: [c[4] / c[3], c[5] / c[3]], z: [0.0; 2] } }
    fn step(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// One tap: downmix to stereo, resample to the output rate, EQ, gain.
struct Tap { nch: usize, ratio: f64, pos: f64, prev: [f32; 2], last: [f32; 2], eq: [Vec<Bq>; 2], eq_ver: u32, sr_out: f64 }

impl Tap {
    fn new(nch: usize, sr_in: f64, sr_out: f64) -> Self {
        Tap { nch, ratio: sr_in / sr_out, pos: 0.0, prev: [0.0; 2], last: [0.0; 2], eq: [vec![], vec![]], eq_ver: u32::MAX, sr_out }
    }

    /// Process one capture block; appends finished stereo samples to `out`.
    fn feed(&mut self, d: &[f32], sh: &Shared, i: usize, out: &mut VecDeque<f32>) {
        let v = sh.eq_ver[i].load(Ordering::Relaxed);
        if v != self.eq_ver {
            self.eq_ver = v;
            let bands = sh.eq[i].lock().unwrap().clone();
            let build = || bands.iter().filter(|b| b.on && b.gain.abs() > 0.05).map(|b| Bq::new(eq::coeffs(b, self.sr_out))).collect::<Vec<_>>();
            self.eq = [build(), build()];
        }
        let k = f32::from_bits(sh.gain[i].load(Ordering::Relaxed)) * f32::from_bits(sh.comp[i].load(Ordering::Relaxed).max(1));
        let mut pk = 0f32;
        for f in d.chunks_exact(self.nch) {
            // 7.1 order: FL FR FC LFE BL BR SL SR; mono is duplicated, other layouts use the first two channels
            let mut st = if self.nch == 8 {
                let c = f[2] * 0.707;
                [(f[0] + c + (f[4] + f[6]) * 0.707) * 0.5, (f[1] + c + (f[5] + f[7]) * 0.707) * 0.5]
            } else if self.nch == 1 { [f[0], f[0]] } else { [f[0], f[1]] };
            pk = pk.max(st[0].abs()).max(st[1].abs());
            let raw = st;
            if self.ratio >= 1.5 { st = [(st[0] + self.prev[0]) * 0.5, (st[1] + self.prev[1]) * 0.5]; } // crude anti-alias before decimating
            self.prev = raw;
            self.pos += 1.0;
            while self.pos >= self.ratio {
                self.pos -= self.ratio;
                let t = (1.0 - self.pos / self.ratio) as f32;
                for c in 0..2 {
                    let mut y = (self.last[c] + (st[c] - self.last[c]) * t) as f64;
                    for b in &mut self.eq[c] { y = b.step(y); }
                    out.push_back(y as f32 * k);
                }
            }
            self.last = st;
        }
        sh.peak[i].store(pk.to_bits(), Ordering::Relaxed);
    }
}

/// The stream mix running on its own thread (see `mic::MicRun`). Dropping it stops the mix.
pub struct StreamRun { _stop: std::sync::mpsc::Sender<()> }

pub fn spawn(dest: String, sh: Arc<Shared>) -> std::sync::mpsc::Receiver<Result<StreamRun, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || match start(&dest, sh) {
        Ok(st) => {
            let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
            let _ = tx.send(Ok(StreamRun { _stop: stop_tx }));
            let _ = stop_rx.recv();
            drop(st);
        }
        Err(e) => { let _ = tx.send(Err(e)); }
    });
    rx
}

pub fn start(dest: &str, sh: Arc<Shared>) -> Result<Stream, String> {
    let host = cpal::default_host();
    let e = |e: &dyn std::fmt::Display| e.to_string();
    let outdev = host.output_devices().map_err(|x| e(&x))?.find(|d| d.name().map_or(false, |n| n.contains(dest))).ok_or_else(|| crate::i18n::tr_fmt("brak urządzenia „{}”", &[dest.to_string()]))?;
    let oc = outdev.default_output_config().map_err(|x| e(&x))?;
    if oc.channels() != 2 || oc.sample_format() != cpal::SampleFormat::F32 { return Err(crate::i18n::tr("wyjście streamu musi być stereo f32")); }
    let sr_out = oc.sample_rate().0 as f64;
    let cap = (sr_out as usize) * 2 / 10; // ~100 ms of stereo samples per source
    let bufs: Vec<Arc<Mutex<VecDeque<f32>>>> = (0..4).map(|_| Arc::new(Mutex::new(VecDeque::new()))).collect();
    let mut streams = vec![];
    for i in 0..4 {
        let dev = host.output_devices().map_err(|x| e(&x))?.find(|d| d.name().map_or(false, |n| n.contains(ENDPOINT[i]))).ok_or_else(|| crate::i18n::tr_fmt("brak {}", &[ENDPOINT[i].to_string()]))?;
        let ic = dev.default_output_config().map_err(|x| e(&x))?;
        if ic.sample_format() != cpal::SampleFormat::F32 { return Err(crate::i18n::tr("kanał Sonara nie jest f32")); }
        let (b, sh2) = (bufs[i].clone(), sh.clone());
        let mut tap = Tap::new(ic.channels() as usize, ic.sample_rate().0 as f64, sr_out);
        // on Windows cpal builds a WASAPI loopback stream when an input stream is opened on a render device
        let s = dev.build_input_stream(&ic.config(), move |d: &[f32], _| {
            let mut q = b.lock().unwrap();
            tap.feed(d, &sh2, i, &mut q);
            while q.len() > cap { q.pop_front(); }
        }, |e| eprintln!("stream tap: {e}"), None).map_err(|x| e(&x))?;
        s.play().map_err(|x| e(&x))?;
        streams.push(s);
    }
    let sh2 = sh.clone();
    let out = outdev.build_output_stream(&oc.config(), move |d: &mut [f32], _| {
        let mut guards: Vec<_> = bufs.iter().map(|b| b.lock().unwrap()).collect();
        let mut mic = sh2.mic.lock().unwrap();
        let mg = f32::from_bits(sh2.gain[4].load(Ordering::Relaxed));
        let mut mpk = 0f32;
        for f in d.chunks_exact_mut(2) {
            let (mut l, mut r) = (0f32, 0f32);
            for g in guards.iter_mut() {
                if g.len() >= 2 { l += g.pop_front().unwrap(); r += g.pop_front().unwrap(); }
            }
            if mic.len() >= 2 { let (ml, mr) = (mic.pop_front().unwrap(), mic.pop_front().unwrap()); mpk = mpk.max(ml.abs()).max(mr.abs()); l += ml * mg; r += mr * mg; }
            f[0] = l.clamp(-1.0, 1.0);
            f[1] = r.clamp(-1.0, 1.0);
        }
        sh2.peak[4].store(mpk.to_bits(), Ordering::Relaxed);
    }, |e| eprintln!("stream out: {e}"), None).map_err(|x| e(&x))?;
    out.play().map_err(|x| e(&x))?;
    streams.push(out);
    Ok(Stream { _s: streams, sh })
}

#[cfg(test)]
mod t {
    use super::*;

    #[test]
    fn tap_applies_gain_and_decimates_96k_to_48k() {
        let sh = Shared::new();
        sh.set_gain(0, 0.5);
        let mut tap = Tap::new(2, 96000.0, 48000.0);
        let mut out = VecDeque::new();
        let block: Vec<f32> = (0..2 * 9600).map(|_| 0.4).collect(); // 100 ms of constant stereo 0.4 at 96 kHz
        tap.feed(&block, &sh, 0, &mut out);
        assert!((out.len() as i32 - 2 * 4800).abs() <= 4, "got {} samples", out.len()); // half as many samples
        assert!((out[out.len() - 1] - 0.2).abs() < 0.01, "got {}", out[out.len() - 1]); // 0.4 * 0.5
    }

    #[test]
    fn tap_downmixes_7_1_like_the_monitor_path() {
        let sh = Shared::new();
        let mut tap = Tap::new(8, 48000.0, 48000.0);
        let mut out = VecDeque::new();
        let f: Vec<f32> = (0..8 * 480).map(|_| 0.1).collect(); // 0.1 on all eight channels
        tap.feed(&f, &sh, 0, &mut out);
        let y = out[out.len() - 1];
        assert!((y - 0.156).abs() < 0.002, "got {y}"); // matches the 0.156 measured on the real endpoint
    }
}
