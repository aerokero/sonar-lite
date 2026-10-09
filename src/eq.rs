// Per-channel parametric EQ (10 bands) and presets. The APO reads these from the same registry Store as the routing.
// Floats are stored as their IEEE bits in a DWORD. A band's values may be absent from the registry = defaults below.
use crate::{store, Chan};
use std::{fs, io, path::PathBuf};
use winreg::RegKey;

/// Default centre frequencies of the 10 bands (from GG's "Flat" preset).
pub const DEF_FREQ: [f32; 10] = [35., 120., 1000., 6000., 18000., 1000., 2000., 4000., 8000., 16000.];
/// Filter type ids as the APO stores them. GG also has highPass/lowPass/notch; their ids are not known yet.
pub const KINDS: [(u32, &str); 3] = [(7, "Peaking"), (8, "Low shelf"), (9, "High shelf")];

#[derive(Clone, Copy)]
pub struct Band { pub on: bool, pub kind: u32, pub freq: f32, pub gain: f32, pub q: f32 }

// Only these values belong to a preset; routing, gain, mute and device ids are never touched by presets.
const PRESET_PREFIXES: [&str; 14] = [
    "kSet_ParametricEq", "kSet_ParamEq", "kSet_BassBoost", "kSet_MidBoost", "kSet_TrebleBoost", "kSet_Reverb", "kSet_GeneralGain",
    "kSet_VirtualSurround", "kSet_AdvancedSurround", "kSet_ChannelPosition", "kSet_Compressor", "kSet_Limiter", "kSet_VolumeBoost", "kSet_GainDB",
];

fn f(v: u32) -> f32 { f32::from_bits(v) }

impl Chan {
    fn rd(&self, hklm: &RegKey, name: &str) -> Option<u32> { store(hklm, self.ch, false).ok()?.get_value(name).ok() }
    fn wr(&self, hklm: &RegKey, name: &str, v: u32) -> io::Result<()> { store(hklm, self.ch, true)?.set_value(name, &v) }

    pub fn eq_on(&self, hklm: &RegKey) -> bool { self.rd(hklm, "kSet_ParametricEqState").map_or(true, |v| v != 0) }
    pub fn set_eq_on(&self, hklm: &RegKey, on: bool) -> io::Result<()> { self.wr(hklm, "kSet_ParametricEqState", on as u32) }

    pub fn band(&self, hklm: &RegKey, i: usize) -> Band {
        let n = |s: &str| format!("kSet_ParametricEqFilter{}{s}", i + 1);
        Band {
            on: self.rd(hklm, &n("State")).map_or(i < 5, |v| v != 0), // ponytail: default for absent State guessed (bands 1-5 on, 6-10 off)
            kind: self.rd(hklm, &n("Type")).unwrap_or(7),
            freq: self.rd(hklm, &n("FreqHz")).map_or(DEF_FREQ[i], f),
            gain: self.rd(hklm, &n("GainDb")).map_or(0.0, f),
            q: self.rd(hklm, &n("Q")).map_or(0.7071, f),
        }
    }

    pub fn set_band(&self, hklm: &RegKey, i: usize, b: &Band) -> io::Result<()> {
        let n = |s: &str| format!("kSet_ParametricEqFilter{}{s}", i + 1);
        self.wr(hklm, &n("State"), b.on as u32)?;
        self.wr(hklm, &n("Type"), b.kind)?;
        self.wr(hklm, &n("FreqHz"), b.freq.to_bits())?;
        self.wr(hklm, &n("GainDb"), b.gain.to_bits())?;
        self.wr(hklm, &n("Q"), b.q.to_bits())
    }

    /// All preset-relevant DWORD values currently set for this channel.
    pub fn snapshot(&self, hklm: &RegKey) -> Vec<(String, u32)> {
        let Ok(k) = store(hklm, self.ch, false) else { return vec![] };
        let mut v: Vec<_> = k.enum_values().flatten()
            .filter(|(n, _)| PRESET_PREFIXES.iter().any(|p| n.starts_with(p)))
            .filter_map(|(n, raw)| Some((n, u32::from_le_bytes(raw.bytes.get(..4)?.try_into().ok()?))))
            .collect();
        v.sort();
        v
    }

    /// Replace the channel's preset-relevant values with `vals` (anything not listed falls back to the APO default).
    pub fn apply(&self, hklm: &RegKey, vals: &[(String, u32)]) -> io::Result<()> {
        let k = store(hklm, self.ch, true)?;
        for (n, _) in self.snapshot(hklm) { k.delete_value(n)?; }
        for (n, v) in vals { k.set_value(n, v)?; }
        Ok(())
    }
}

/// RBJ biquad coefficients (b0, b1, b2, a0, a1, a2) of one band at sample rate `sr`.
pub fn coeffs(b: &Band, sr: f64) -> [f64; 6] {
    let (w0, a) = (std::f64::consts::TAU * (b.freq as f64).clamp(10.0, 20000.0) / sr, 10f64.powf(b.gain as f64 / 40.0));
    let (c, al) = (w0.cos(), w0.sin() / (2.0 * (b.q as f64).max(0.05)));
    let be = 2.0 * a.sqrt() * al;
    match b.kind {
        8 => [a * ((a + 1.0) - (a - 1.0) * c + be), 2.0 * a * ((a - 1.0) - (a + 1.0) * c), a * ((a + 1.0) - (a - 1.0) * c - be),
              (a + 1.0) + (a - 1.0) * c + be, -2.0 * ((a - 1.0) + (a + 1.0) * c), (a + 1.0) + (a - 1.0) * c - be],
        9 => [a * ((a + 1.0) + (a - 1.0) * c + be), -2.0 * a * ((a - 1.0) + (a + 1.0) * c), a * ((a + 1.0) + (a - 1.0) * c - be),
              (a + 1.0) - (a - 1.0) * c + be, 2.0 * ((a - 1.0) - (a + 1.0) * c), (a + 1.0) - (a - 1.0) * c - be],
        _ => [1.0 + al * a, -2.0 * c, 1.0 - al * a, 1.0 + al / a, -2.0 * c, 1.0 - al / a],
    }
}

/// Combined magnitude response in dB at `f` Hz of all enabled bands (at 48 kHz).
pub fn response_db(bands: &[Band], f: f32) -> f32 {
    // f64 + sin^2(w/2) form: at low frequencies cos(w) ~ 1 and the naive formula loses the result to cancellation
    let s2 = (std::f64::consts::TAU * f as f64 / 48000.0 / 2.0).sin().powi(2);
    bands.iter().filter(|b| b.on).map(|b| {
        let [b0, b1, b2, a0, a1, a2] = coeffs(b, 48000.0);
        let m = |x0: f64, x1: f64, x2: f64| (x0 + x1 + x2).powi(2) + 4.0 * s2 * (x0 * x2 * 4.0 * s2 - x1 * (x0 + x2) - 4.0 * x0 * x2);
        (10.0 * (m(b0, b1, b2) / m(a0, a1, a2)).log10()) as f32
    }).sum()
}

/// Built-in general-purpose EQ presets: (name, [(type, Hz, dB, Q)]). Unlisted bands stay flat.
const BUILTIN: [(&str, &[(u32, f32, f32, f32)]); 8] = [
    ("Flat", &[]),
    ("Gaming", &[(8, 60.0, 2.0, 0.7), (7, 250.0, -2.0, 1.0), (7, 3000.0, 2.0, 1.0), (9, 8000.0, 3.0, 0.7)]),
    ("Bass Boost", &[(8, 100.0, 6.0, 0.7), (7, 60.0, 2.0, 1.0)]),
    ("Clear Voice", &[(8, 100.0, -3.0, 0.7), (7, 250.0, -2.5, 1.2), (7, 3000.0, 3.0, 1.0), (9, 6000.0, 2.0, 0.7)]),
    ("Deeper Voice", &[(8, 150.0, 4.0, 0.7), (7, 300.0, 2.0, 1.0), (9, 6000.0, -2.0, 0.7)]),
    ("Clear Dialogs", &[(7, 200.0, -2.0, 1.0), (7, 2000.0, 3.0, 1.0), (7, 4000.0, 2.0, 1.0), (9, 9000.0, 1.0, 0.7)]),
    ("Immersion", &[(8, 80.0, 4.0, 0.7), (7, 1000.0, -2.0, 0.8), (9, 8000.0, 3.0, 0.7)]),
    ("Footsteps", &[(8, 100.0, -4.0, 0.7), (7, 3000.0, 4.0, 1.0), (7, 6000.0, 3.0, 1.0)]),
];

/// Presets for the microphone: only things that make sense for a voice.
const VOICE: [(&str, &[(u32, f32, f32, f32)]); 7] = [
    ("Flat", &[]),
    ("Clear Voice", &[(8, 100.0, -3.0, 0.7), (7, 250.0, -2.5, 1.2), (7, 3000.0, 3.0, 1.0), (9, 6000.0, 2.0, 0.7)]),
    ("Deeper Voice", &[(8, 150.0, 4.0, 0.7), (7, 300.0, 2.0, 1.0), (9, 6000.0, -2.0, 0.7)]),
    ("Podcast", &[(8, 90.0, -3.0, 0.7), (7, 200.0, 2.5, 1.0), (7, 4000.0, 2.0, 1.0), (9, 9000.0, -1.0, 0.7)]),
    ("Bright Voice", &[(8, 120.0, -2.0, 0.7), (7, 3500.0, 3.0, 1.0), (9, 8000.0, 3.0, 0.7)]),
    ("Reduce Boom", &[(8, 80.0, -4.0, 0.7), (7, 250.0, -3.5, 1.2)]),
    ("Radio", &[(8, 150.0, -6.0, 0.7), (7, 1000.0, 2.0, 1.0), (7, 3000.0, 3.0, 1.0), (9, 7000.0, -3.0, 0.7)]),
];

pub fn voice_names() -> Vec<&'static str> { VOICE.iter().map(|p| p.0).collect() }

pub fn builtin_names() -> Vec<&'static str> { BUILTIN.iter().map(|p| p.0).collect() }

/// Bands 6-8 carry the Bass / Voice / Treble sliders: (type, Hz, Q) of low shelf, mid peak and high shelf.
pub const TONE: [(u32, f32, f32); 3] = [(8, 100.0, 0.7), (7, 2000.0, 0.8), (9, 8000.0, 0.7)];
/// Number of bands shown as markers on the graph (the rest is the tone sliders and spare).
pub const MARKERS: usize = 5;

/// The 10 bands of a built-in preset (None if unknown).
pub fn builtin(name: &str) -> Option<Vec<Band>> {
    let (_, def) = BUILTIN.iter().chain(VOICE.iter()).find(|p| p.0 == name)?;
    let mut v: Vec<Band> = (0..10).map(|i| Band { on: true, kind: 7, freq: DEF_FREQ[i], gain: 0.0, q: 0.7071 }).collect();
    for (i, &(kind, freq, q)) in TONE.iter().enumerate() { v[MARKERS + i] = Band { on: true, kind, freq, gain: 0.0, q }; }
    for (i, &(kind, freq, gain, q)) in def.iter().enumerate() { v[i] = Band { on: true, kind, freq, gain, q }; }
    Some(v)
}

/// Bands as the registry-style name/value list a preset file holds (used for the microphone, whose EQ is not in the registry).
pub fn bands_to_vals(b: &[Band]) -> Vec<(String, u32)> {
    let mut v = vec![("kSet_ParametricEqState".to_string(), 1)];
    for (i, x) in b.iter().enumerate() {
        let n = |s: &str| format!("kSet_ParametricEqFilter{}{s}", i + 1);
        v.extend([(n("State"), x.on as u32), (n("Type"), x.kind), (n("FreqHz"), x.freq.to_bits()), (n("GainDb"), x.gain.to_bits()), (n("Q"), x.q.to_bits())]);
    }
    v
}

/// Inverse of `bands_to_vals`; anything missing is a flat band.
pub fn vals_to_bands(v: &[(String, u32)]) -> Vec<Band> {
    let mut b = builtin("Flat").unwrap_or_default();
    for (n, x) in v {
        let Some(rest) = n.strip_prefix("kSet_ParametricEqFilter") else { continue };
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        let Some(band) = digits.parse::<usize>().ok().and_then(|i| b.get_mut(i.wrapping_sub(1))) else { continue };
        match &rest[digits.len()..] {
            "State" => band.on = *x != 0,
            "Type" => band.kind = *x,
            "FreqHz" => band.freq = f(*x),
            "GainDb" => band.gain = f(*x),
            "Q" => band.q = f(*x),
            _ => {}
        }
    }
    b
}

impl Chan {
    /// Write a full set of bands and switch the EQ on.
    pub fn set_bands(&self, hklm: &RegKey, bands: &[Band]) -> io::Result<()> {
        for (i, b) in bands.iter().enumerate() { self.set_band(hklm, i, b)?; }
        self.set_eq_on(hklm, true)
    }
}

/// Own presets are kept per kind: channels in `presets`, the microphone in `presets\mic` (a voice EQ is not a channel EQ).
pub fn preset_dir() -> PathBuf { preset_dir_for(false) }

fn preset_dir_for(mic: bool) -> PathBuf {
    let d = PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("SonarLite").join("presets");
    if mic { d.join("mic") } else { d }
}

/// How many own presets the UI offers room for.
pub const MAX_PRESETS: usize = 10;

pub fn list_presets() -> Vec<String> { list_presets_in(false) }

pub fn list_presets_in(mic: bool) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(preset_dir_for(mic)).into_iter().flatten().flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".txt").map(String::from)).collect();
    v.sort_by_key(|s| s.to_lowercase());
    v
}

fn preset_path(name: &str, mic: bool) -> io::Result<PathBuf> {
    let ok = !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_alphanumeric() || " -_.".contains(c)) && !name.starts_with('.');
    if ok { Ok(preset_dir_for(mic).join(format!("{name}.txt"))) } else { Err(io::Error::new(io::ErrorKind::InvalidInput, crate::i18n::tr("nazwa presetu: litery, cyfry, spacja, - _ ."))) }
}

pub fn save_preset(name: &str, vals: &[(String, u32)]) -> io::Result<()> { save_preset_in(false, name, vals) }

pub fn save_preset_in(mic: bool, name: &str, vals: &[(String, u32)]) -> io::Result<()> {
    let p = preset_path(name, mic)?;
    if !p.exists() && list_presets_in(mic).len() >= MAX_PRESETS { return Err(io::Error::new(io::ErrorKind::Other, crate::i18n::tr("maksymalnie 10 własnych presetów - usuń któryś"))); }
    fs::create_dir_all(preset_dir_for(mic))?;
    fs::write(p, vals.iter().map(|(n, v)| format!("{n}={v}\n")).collect::<String>())
}

pub fn delete_preset_in(mic: bool, name: &str) -> io::Result<()> { fs::remove_file(preset_path(name, mic)?) }

pub fn load_preset(name: &str) -> io::Result<Vec<(String, u32)>> { load_preset_in(false, name) }

pub fn load_preset_in(mic: bool, name: &str) -> io::Result<Vec<(String, u32)>> {
    Ok(fs::read_to_string(preset_path(name, mic)?)?.lines().filter_map(|l| {
        let (n, v) = l.split_once('=')?;
        // never let a preset file write outside the preset-relevant values
        PRESET_PREFIXES.iter().any(|p| n.starts_with(p)).then_some((n.to_string(), v.trim().parse().ok()?))
    }).collect())
}

#[cfg(test)]
mod t {
    use super::*;
    #[test]
    fn preset_names_and_float_bits() {
        assert!(preset_path("Moj preset-1", false).is_ok());
        assert!(preset_path("..\\evil", false).is_err() && preset_path("", true).is_err() && preset_path("a/b", false).is_err());
        let bass = builtin("Bass Boost").unwrap();
        let back = vals_to_bands(&bands_to_vals(&bass)); // bands survive the preset-file form
        assert!(bass.iter().zip(&back).all(|(a, b)| a.on == b.on && a.kind == b.kind && a.freq == b.freq && a.gain == b.gain && a.q == b.q));
        assert!(back[MARKERS].kind == 8 && back[MARKERS + 2].kind == 9); // tone bands are in place
        assert_eq!(f(1117782016), 80.0); // Overwatch band-1 frequency as seen in the registry
        let band = |kind, gain| Band { on: true, kind, freq: 1000.0, gain, q: 1.0 };
        assert!((response_db(&[band(7, 6.0)], 1000.0) - 6.0).abs() < 0.1); // peaking +6 dB at its centre
        assert!(response_db(&[band(7, 0.0)], 300.0).abs() < 0.01); // flat band = no change
        assert!(response_db(&[band(9, 6.0)], 18000.0) > 5.0 && response_db(&[band(9, 6.0)], 100.0).abs() < 0.5); // high shelf
        let bass = builtin("Bass Boost").unwrap();
        assert!(bass.len() == 10 && response_db(&bass, 50.0) > 5.0 && response_db(&bass, 5000.0).abs() < 1.0); // bass boost lifts lows only
        assert!(builtin("Nope").is_none() && builtin_names().contains(&"Gaming"));
        assert!(voice_names().iter().all(|n| builtin(n).is_some()) && !voice_names().contains(&"Bass Boost")); // voice list is voice-only
    }
}
