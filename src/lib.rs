// Sonar Lite core: reads/writes SteelSeries Sonar.APO channel routing in the registry (no GG needed).
// Writing HKLM needs admin; the APO picks changes up live.
pub mod apps;
pub mod eq;
pub mod i18n;
pub mod levels;
pub mod settings;
pub mod autostart;
pub mod mic;
pub mod mixer;
pub mod stream;
pub mod vad;
use std::io;
use winreg::{enums::*, RegKey, RegValue};

const APO: &str = r"SOFTWARE\SteelSeries ApS\Sonar.APO";
const MM: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render";

/// (cli name, registry channel, value-name prefix)
pub const CHANNELS: [(&str, &str, &str); 4] = [
    ("game", "Game", "kSet_StreamRedirection"),
    ("media", "Media", "kSet_StreamRedirection"),
    ("aux", "Aux", "kSet_StreamRedirection"),
    ("chat", "ChatRender", "kSet_StreamRedirectionChat"),
];

#[derive(Clone)]
pub struct Ep { pub guid: String, pub name: String, pub virt: bool }

pub struct Chan { pub key: &'static str, pub(crate) ch: &'static str, p: &'static str, pub guid: String, pub gain: f32, pub mute: bool }

/// Endpoint id as the APO stores it: UTF-16LE "{0.0.0.00000000}.{guid}" + NUL.
pub fn id_bytes(guid: &str) -> Vec<u8> {
    format!("{{0.0.0.00000000}}.{{{guid}}}\0").encode_utf16().flat_map(u16::to_le_bytes).collect()
}

pub fn guid_of(b: &[u8]) -> Option<String> {
    let u: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
    let s = String::from_utf16(&u).ok()?;
    Some(s.split_once(".{")?.1.trim_end_matches('}').to_lowercase())
}

/// Active capture endpoints (guid, "name (adapter)").
pub fn capture_endpoints(hklm: &RegKey) -> Vec<(String, String)> {
    let Ok(mm) = hklm.open_subkey(MM.replace("Render", "Capture")) else { return vec![] };
    mm.enum_keys().flatten().filter_map(|g| {
        let k = mm.open_subkey(&g).ok()?;
        if k.get_value::<u32, _>("DeviceState").ok()? != 1 { return None; }
        let p = k.open_subkey("Properties").ok()?;
        let n = p.get_value::<String, _>("{a45c254e-df1c-4efd-8020-67d146a850e0},2").unwrap_or_default();
        let a = p.get_value::<String, _>("{b3f8fa53-0004-438e-9003-51a46e139bfc},6").unwrap_or_default();
        Some((g.trim_matches(|c| c == '{' || c == '}').to_lowercase(), format!("{n} ({a})")))
    }).collect()
}

/// Linear factor that undoes the Windows volume of a render endpoint (loopback captures are scaled by it).
pub fn endpoint_comp(guid: &str) -> f32 {
    use windows::{core::HSTRING, Win32::{Media::Audio::{Endpoints::IAudioEndpointVolume, *}, System::Com::*}};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return 1.0 };
        let Ok(d) = en.GetDevice(&HSTRING::from(format!("{{0.0.0.00000000}}.{{{guid}}}"))) else { return 1.0 };
        let Ok(v) = d.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) else { return 1.0 };
        let db = v.GetMasterVolumeLevel().unwrap_or(0.0);
        10f32.powf(-db / 20.0).clamp(1.0, 100.0)
    }
}

/// Active render endpoints; `virt` marks Sonar's own virtual devices (not valid outputs).
pub fn endpoints(hklm: &RegKey) -> Vec<Ep> {
    let Ok(mm) = hklm.open_subkey(MM) else { return vec![] };
    mm.enum_keys().flatten().filter_map(|g| {
        let k = mm.open_subkey(&g).ok()?;
        if k.get_value::<u32, _>("DeviceState").ok()? != 1 { return None; }
        let p = k.open_subkey("Properties").ok()?;
        let n = p.get_value::<String, _>("{a45c254e-df1c-4efd-8020-67d146a850e0},2").unwrap_or_default();
        let a = p.get_value::<String, _>("{b3f8fa53-0004-438e-9003-51a46e139bfc},6").unwrap_or_default();
        let virt = a.contains("Sonar Virtual Audio Device");
        Some(Ep { guid: g.trim_matches(|c| c == '{' || c == '}').to_lowercase(), name: format!("{n} ({a})"), virt })
    }).collect()
}

/// Guid of Sonar's virtual endpoint for a channel key ("game" -> "SteelSeries Sonar - Gaming").
pub fn sonar_guid(eps: &[Ep], key: &str) -> Option<String> {
    let n = match key { "game" => "- Gaming", "media" => "- Media", "aux" => "- Aux", "chat" => "- Chat", _ => return None };
    eps.iter().find(|e| e.virt && e.name.contains(n)).map(|e| e.guid.clone())
}

pub(crate) fn store(hklm: &RegKey, ch: &str, write: bool) -> io::Result<RegKey> {
    let flags = if write { KEY_READ | KEY_WRITE } else { KEY_READ };
    hklm.open_subkey_with_flags(format!(r"{APO}\{ch}\Settings\GlobalControl\Store"), flags)
}

pub fn read_all(hklm: &RegKey) -> Vec<Chan> {
    CHANNELS.iter().filter_map(|&(key, ch, p)| {
        let k = store(hklm, ch, false).ok()?;
        let guid = k.get_raw_value(format!("{p}DeviceId")).ok().and_then(|v| guid_of(&v.bytes)).unwrap_or_default();
        let gain = f32::from_bits(k.get_value(format!("{p}GainLin")).unwrap_or(1.0f32.to_bits()));
        let mute = k.get_value::<u32, _>(format!("{p}Mute")).unwrap_or(0) != 0;
        Some(Chan { key, ch, p, guid, gain, mute })
    }).collect()
}

impl Chan {
    /// Tell running APO instances that settings changed (they otherwise read the registry only when a stream starts).
    /// Each live stream has Settings/Streams/<id> with Modified* bitsets (one bit per setting); setting bits makes it reload them.
    /// `bits` = setting ids to flag; empty = all (but reloading everything made a live stream go silent - always pass bits).
    pub fn notify(&self, hklm: &RegKey, bits: &[usize]) -> io::Result<()> {
        let streams = hklm.open_subkey_with_flags(format!(r"{APO}\{}\Settings\Streams", self.ch), KEY_READ)?;
        for id in streams.enum_keys().flatten() {
            let k = streams.open_subkey_with_flags(&id, KEY_READ | KEY_WRITE)?;
            for (name, raw) in k.enum_values().flatten().collect::<Vec<_>>() {
                if name.starts_with("Modified") && raw.vtype == REG_BINARY {
                    let mut b = raw.bytes.clone();
                    if bits.is_empty() { b.fill(0xff) } else { for &i in bits { if i / 8 < b.len() { b[i / 8] |= 1 << (i % 8) } } }
                    k.set_raw_value(&name, &RegValue { bytes: b, vtype: REG_BINARY })?;
                }
            }
        }
        Ok(())
    }

    pub fn set_out(&mut self, hklm: &RegKey, guid: &str) -> io::Result<()> {
        let k = store(hklm, self.ch, true)?;
        let b = id_bytes(guid);
        if self.ch == "ChatRender" {
            // The Chat APO has two routes: StreamRedirectionChat (weak, auxiliary) and CaptureStreamRedirection, which actually
            // carries what you hear (GG had it on the monitor). Both must follow the chosen device, otherwise Chat plays elsewhere.
            k.set_value("kSet_CaptureStreamRedirectionState", &1u32)?;
            k.set_value("kSet_CaptureStreamRedirectionDeviceIdCount", &((b.len() / 2) as u32))?;
            k.set_raw_value("kSet_CaptureStreamRedirectionDeviceId", &RegValue { bytes: b.clone(), vtype: REG_BINARY })?;
        }
        k.set_value(format!("{}DeviceIdCount", self.p), &((b.len() / 2) as u32))?;
        k.set_raw_value(format!("{}DeviceId", self.p), &RegValue { bytes: b, vtype: REG_BINARY })?;
        self.guid = guid.into();
        Ok(())
    }
    pub fn set_gain(&mut self, hklm: &RegKey, g: f32) -> io::Result<()> {
        store(hklm, self.ch, true)?.set_value(format!("{}GainLin", self.p), &g.to_bits())?;
        self.gain = g;
        Ok(())
    }
    pub fn set_mute(&mut self, hklm: &RegKey, m: bool) -> io::Result<()> {
        store(hklm, self.ch, true)?.set_value(format!("{}Mute", self.p), &(m as u32))?;
        self.mute = m;
        Ok(())
    }
}

#[cfg(test)]
mod t {
    use super::*;
    #[test]
    fn id_roundtrip_and_gain_bits() {
        let g = "cb722606-fe1a-4bec-ad58-b3f2000f355a";
        let b = id_bytes(g);
        assert_eq!(b.len(), 112); // matches what the APO writes (Count = 56 chars)
        assert_eq!(guid_of(&b).as_deref(), Some(g));
        assert_eq!(1.0f32.to_bits(), 1065353216); // value observed in registry for gain 1.0
    }
}
