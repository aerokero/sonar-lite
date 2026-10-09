// Mixer settings that Sonar.APO has no notion of: MASTER volume, CHATMIX balance, the volume the user set per channel,
// and which EQ preset a channel uses. The gain written to the APO is the product of them (see `effective`).
use std::{collections::BTreeMap, fs, path::PathBuf};

#[derive(Clone)]
pub struct Mixer {
    pub master: f32,
    /// 0 = only GAME, 0.5 = both full, 1 = only CHAT
    pub chatmix: f32,
    /// volume of each channel as shown on its fader (0..1)
    pub base: BTreeMap<String, f32>,
    /// name of the EQ preset in use per channel ("Własny" = hand-edited)
    pub eq: BTreeMap<String, String>,
    /// streamer mode: a second mix for OBS (see stream.rs)
    pub stream_on: bool,
    /// name (substring) of the output device that carries the stream mix
    pub stream_dev: String,
    pub smaster: f32,
    /// stream fader per source (game/chat/media/aux/mic) and its mute
    pub sgain: BTreeMap<String, f32>,
    pub smute: BTreeMap<String, bool>,
    /// one output device for every channel (Sonar's "link" icon)
    pub link_all: bool,
    /// per channel: output devices in priority order as (guid, name); the first one that is present is used (fallback)
    pub prio: BTreeMap<String, Vec<(String, String)>>,
    /// per channel: devices the user switched off (kept so they can be dragged back, even while unplugged)
    pub prio_off: BTreeMap<String, Vec<(String, String)>>,
}

impl Default for Mixer { fn default() -> Self { Mixer { master: 1.0, chatmix: 0.5, base: BTreeMap::new(), eq: BTreeMap::new(), stream_on: false, stream_dev: "Virtual Audio Cable".into(), smaster: 1.0, sgain: BTreeMap::new(), smute: BTreeMap::new(), link_all: false, prio: BTreeMap::new(), prio_off: BTreeMap::new() } } }

fn path() -> PathBuf { PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("SonarLite").join("mixer.txt") }

pub fn load() -> Mixer {
    let mut m = parse(&fs::read_to_string(path()).unwrap_or_default());
    // older versions multiplied the faders by MASTER; fold that into the faders so the channels keep the same gain
    if m.master < 1.0 { let k = m.master; for g in m.base.values_mut() { *g *= k; } m.master = 1.0; }
    m
}

fn parse(text: &str) -> Mixer {
    let mut m = Mixer::default();
    for l in text.lines() {
        let Some((k, v)) = l.split_once('=') else { continue };
        match k {
            "master" => m.master = v.parse().unwrap_or(1.0),
            "chatmix" => m.chatmix = v.parse().unwrap_or(0.5),
            "stream_on" => m.stream_on = v == "1",
            "stream_dev" => m.stream_dev = v.into(),
            "smaster" => m.smaster = v.parse().unwrap_or(1.0),
            "link" => m.link_all = v == "1",
            _ => {
                if let Some(c) = k.strip_prefix("gain.") { if let Ok(g) = v.parse() { m.base.insert(c.into(), g); } }
                if let Some(c) = k.strip_prefix("eq.") { m.eq.insert(c.into(), v.into()); }
                if let Some(c) = k.strip_prefix("sgain.") { if let Ok(g) = v.parse() { m.sgain.insert(c.into(), g); } }
                if let Some(c) = k.strip_prefix("smute.") { m.smute.insert(c.into(), v == "1"); }
                for (pre, map) in [("prio.", &mut m.prio), ("prio_off.", &mut m.prio_off)] {
                    if let Some(c) = k.strip_prefix(pre) {
                        let list = v.split('|').filter_map(|e| e.split_once('~')).map(|(g, n)| (g.to_string(), n.to_string())).collect();
                        map.insert(c.into(), list);
                    }
                }
            }
        }
    }
    m
}

pub fn save(m: &Mixer) {
    let _ = fs::create_dir_all(path().parent().unwrap());
    let _ = fs::write(path(), serialize(m));
}

fn serialize(m: &Mixer) -> String {
    let mut s = format!("master={}\nchatmix={}\n", m.master, m.chatmix);
    for (c, g) in &m.base { s += &format!("gain.{c}={g}\n"); }
    for (c, n) in &m.eq { s += &format!("eq.{c}={n}\n"); }
    s += &format!("stream_on={}\nstream_dev={}\nsmaster={}\n", m.stream_on as u8, m.stream_dev, m.smaster);
    for (c, g) in &m.sgain { s += &format!("sgain.{c}={g}\n"); }
    for (c, x) in &m.smute { s += &format!("smute.{c}={}\n", *x as u8); }
    s += &format!("link={}\n", m.link_all as u8);
    for (c, l) in &m.prio { s += &format!("prio.{c}={}\n", l.iter().map(|(g, n)| format!("{g}~{n}")).collect::<Vec<_>>().join("|")); }
    s
}

/// First device of the priority list that is currently present (None if the list is empty or nothing is available).
pub fn desired_device(list: &[(String, String)], present: &[String]) -> Option<String> {
    list.iter().find(|(g, _)| present.contains(g)).map(|(g, _)| g.clone())
}

impl Mixer {
    /// Gain to write into the APO for a channel: fader * chatmix factor (chatmix only touches game and chat). MASTER is not a
    /// factor: moving it moves the other faders (see `move_master`), so what a fader shows is what the channel gets.
    pub fn effective(&self, key: &str) -> f32 {
        let chat = match key { "game" => (2.0 * (1.0 - self.chatmix)).min(1.0), "chat" => (2.0 * self.chatmix).min(1.0), _ => 1.0 };
        self.base.get(key).copied().unwrap_or(1.0) * chat
    }

    /// Move MASTER to `v`: every channel fader follows by the same ratio (all at `v` when MASTER was at the bottom).
    pub fn move_master(&mut self, v: f32) {
        let old = self.master;
        for g in self.base.values_mut() { *g = if old < 0.02 { v } else { (*g * v / old).clamp(0.0, 1.0) }; }
        self.master = v;
    }
}

impl Mixer {
    /// Gain of one source in the stream mix (fader * stream master, 0 when muted).
    pub fn stream_gain(&self, key: &str) -> f32 {
        if self.smute.get(key).copied().unwrap_or(false) { 0.0 } else { self.sgain.get(key).copied().unwrap_or(1.0) * self.smaster }
    }
}

#[cfg(test)]
mod t {
    use super::*;
    #[test]
    fn chatmix_and_master_scale_the_written_gain() {
        let mut m = Mixer::default();
        m.base.insert("game".into(), 0.8);
        assert_eq!(m.effective("game"), 0.8); // centred: untouched
        m.chatmix = 1.0;
        assert_eq!(m.effective("game"), 0.0); // fully on chat: game silent
        assert_eq!(m.effective("chat"), 1.0);
        m.chatmix = 0.25;
        assert!((m.effective("chat") - 0.5).abs() < 1e-6 && m.effective("game") == 0.8); // half-way: chat halved, game full
        m.base.insert("media".into(), 1.0);
        m.move_master(0.5);
        assert!((m.effective("media") - 0.5).abs() < 1e-6 && (m.effective("game") - 0.4).abs() < 1e-6); // master moves every fader
        m.move_master(1.0);
        assert!((m.effective("media") - 1.0).abs() < 1e-6 && (m.effective("game") - 0.8).abs() < 1e-6); // and back again
        m.move_master(0.0);
        m.move_master(0.5);
        assert!((m.effective("media") - 0.5).abs() < 1e-6 && (m.effective("game") - 0.5).abs() < 1e-6); // from the bottom all start together
        m.sgain.insert("game".into(), 0.5);
        m.smaster = 0.5;
        assert!((m.stream_gain("game") - 0.25).abs() < 1e-6 && (m.stream_gain("chat") - 0.5).abs() < 1e-6); // stream mix is separate
        m.smute.insert("chat".into(), true);
        assert_eq!(m.stream_gain("chat"), 0.0);
    }

    #[test]
    fn everything_survives_save_and_load() {
        let mut m = Mixer::default();
        m.master = 0.8; m.chatmix = 0.3; m.stream_on = true; m.stream_dev = "Some Cable".into(); m.smaster = 0.6;
        m.base.insert("game".into(), 0.7);
        m.eq.insert("chat".into(), "Clear Voice".into());
        m.sgain.insert("mic".into(), 0.4);
        m.smute.insert("aux".into(), true);
        let r = parse(&serialize(&m));
        assert!(r.master == 0.8 && r.chatmix == 0.3 && r.stream_on && r.stream_dev == "Some Cable" && r.smaster == 0.6);
        assert!(r.base["game"] == 0.7 && r.eq["chat"] == "Clear Voice" && r.sgain["mic"] == 0.4 && r.smute["aux"]);
        m.link_all = true;
        m.prio.insert("game".into(), vec![("aaa".into(), "Słuchawki (Arctis)".into()), ("bbb".into(), "G27QC (NVIDIA)".into())]);
        let r = parse(&serialize(&m));
        assert!(r.link_all && r.prio["game"] == m.prio["game"]);
    }

    #[test]
    fn fallback_picks_the_first_present_device() {
        let l = vec![("aaa".to_string(), "headset".to_string()), ("bbb".to_string(), "monitor".to_string())];
        assert_eq!(desired_device(&l, &["aaa".into(), "bbb".into()]).as_deref(), Some("aaa")); // preferred device present
        assert_eq!(desired_device(&l, &["bbb".into()]).as_deref(), Some("bbb")); // headset off -> fall back to the monitor
        assert_eq!(desired_device(&l, &["ccc".into()]), None); // nothing from the list: leave things as they are
        assert_eq!(desired_device(&[], &["aaa".into()]), None);
    }
}
