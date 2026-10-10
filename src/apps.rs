// Per-application routing: which Sonar channel (or Windows default) an app's audio goes to.
// Uses the undocumented AudioPolicyConfig factory (same one Windows Settings/EarTrumpet use). On Win11 24H2 it answers to
// IID ab3d4648-...; vtable layout verified by reading a live process: 3 IUnknown + 3 IInspectable + 19 other + Set(25), Get(26).
use std::ffi::c_void;
use windows::{
    Media::Control::{GlobalSystemMediaTransportControlsSessionManager as Smtc, GlobalSystemMediaTransportControlsSessionPlaybackStatus as Pb},
    core::*,
    Win32::{Foundation::*, Media::Audio::*, System::{Com::*, Threading::*, WinRT::*}},
};

const IID: &str = "ab3d4648-e242-459f-b02f-541c70306324";
const RENDER_IF: &str = "e6327cad-dcec-4949-ae8a-991e976a79d2";

pub struct Policy(IUnknown);

type GetFn = unsafe extern "system" fn(*mut c_void, u32, i32, i32, *mut HSTRING) -> HRESULT;
type SetFn = unsafe extern "system" fn(*mut c_void, u32, i32, i32, *mut c_void) -> HRESULT;

impl Policy {
    pub fn new() -> Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); // may already be initialised on this thread
            let f: IInspectable = RoGetActivationFactory(&HSTRING::from("Windows.Media.Internal.AudioPolicyConfig"))?;
            let mut p: *mut c_void = std::ptr::null_mut();
            f.query(&GUID::from(IID), &mut p).ok()?;
            Ok(Self(IUnknown::from_raw(p)))
        }
    }

    fn slot(&self, i: usize) -> usize { unsafe { *(*(self.0.as_raw() as *const *const usize)).add(i) } }

    /// Endpoint guid the process is pinned to, None = follows the Windows default.
    pub fn get(&self, pid: u32) -> Option<String> {
        unsafe {
            let f: GetFn = std::mem::transmute(self.slot(26));
            let mut s = HSTRING::new();
            f(self.0.as_raw(), pid, 0, 1, &mut s).ok().ok()?;
            let s = s.to_string();
            Some(s.split_once(".{")?.1.split_once("}#")?.0.to_lowercase())
        }
    }

    /// Pin the process to an endpoint guid, or None to go back to the Windows default.
    pub fn set(&self, pid: u32, guid: Option<&str>) -> Result<()> {
        unsafe {
            let f: SetFn = std::mem::transmute(self.slot(25));
            let id = guid.map(|g| HSTRING::from(format!(r"\\?\SWD#MMDEVAPI#{{0.0.0.00000000}}.{{{g}}}#{{{RENDER_IF}}}")));
            let raw = id.as_ref().map_or(std::ptr::null_mut(), |h| std::mem::transmute_copy::<HSTRING, *mut c_void>(h));
            for role in 0..3 { f(self.0.as_raw(), pid, 0, role, raw).ok()?; } // console, multimedia, communications
            Ok(())
        }
    }
}

/// One audio session: process id, exe name and the endpoint (guid) it actually plays on.
pub struct Session { pub pid: u32, pub exe: String, pub endpoint: String, pub active: bool }

/// Render audio sessions of all processes, with the endpoint each one really lives on (a process can have several).
pub fn sessions() -> Vec<Session> { sessions_on(eRender) }

/// Apps that are recording (capture sessions), with the capture endpoint each one reads from.
pub fn capture_sessions() -> Vec<Session> { sessions_on(eCapture) }

fn sessions_on(flow: EDataFlow) -> Vec<Session> {
    let mut out: Vec<Session> = vec![];
    let _ = (|| -> Result<()> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let devs = en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
            for i in 0..devs.GetCount()? {
                let dev = devs.Item(i)?;
                let id = dev.GetId().ok().and_then(|p| { let s = p.to_string().ok(); CoTaskMemFree(Some(p.0 as _)); s }).unwrap_or_default();
                let endpoint = id.split_once(".{").map(|x| x.1.trim_end_matches('}').to_lowercase()).unwrap_or_default();
                let Ok(m) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
                let se = m.GetSessionEnumerator()?;
                for j in 0..se.GetCount()? {
                    let Ok(c) = se.GetSession(j)?.cast::<IAudioSessionControl2>() else { continue };
                    let pid = c.GetProcessId().unwrap_or(0);
                    if pid != 0 && !out.iter().any(|o| o.pid == pid && o.endpoint == endpoint) {
                        if let Some(exe) = exe_name(pid).filter(|n| !n.eq_ignore_ascii_case("audiodg.exe")) { out.push(Session { pid, exe, endpoint: endpoint.clone(), active: c.GetState().map_or(false, |st| st == AudioSessionStateActive) }); }
                    }
                }
            }
            Ok(())
        }
    })();
    out.sort_by_key(|o| (o.exe.to_lowercase(), o.pid));
    out
}

fn exe_name(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut b = [0u16; 520];
        let mut n = b.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(b.as_mut_ptr()), &mut n);
        let _ = CloseHandle(h);
        r.ok()?;
        let p = String::from_utf16_lossy(&b[..n as usize]);
        Some(p.rsplit('\\').next()?.to_string())
    }
}

/// Guid of the Windows default render endpoint (apps without a pin follow it).
pub fn default_render_guid() -> Option<String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let p = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?.GetId().ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        Some(s?.split_once(".{")?.1.trim_end_matches('}').to_lowercase())
    }
}

/// Running APO instances keep the settings they started with. Moving an app to another Sonar channel and back creates a
/// fresh instance that reads the channel's current registry values. All apps of the channel move together (one wait, not one
/// per app). Returns how many apps were refreshed.
pub fn refresh_channel(pol: &Policy, eps: &[crate::Ep], key: &str) -> usize {
    let park = crate::CHANNELS.iter().map(|c| c.0).filter(|k| *k != key).find_map(|k| crate::sonar_guid(eps, k));
    let (Some(home), Some(park)) = (crate::sonar_guid(eps, key), park) else { return 0 };
    let def = default_render_guid();
    let mut moved: Vec<(u32, Option<String>)> = vec![];
    for Session { pid, .. } in sessions() {
        if moved.iter().any(|m| m.0 == pid) { continue; }
        let pin = pol.get(pid);
        let on = match &pin { Some(g) => *g == home, None => def.as_deref() == Some(home.as_str()) };
        if on && pol.set(pid, Some(&park)).is_ok() { moved.push((pid, pin)); }
    }
    if !moved.is_empty() { std::thread::sleep(std::time::Duration::from_millis(150)); }
    for (pid, pin) in &moved { let _ = pol.set(*pid, pin.as_deref()); }
    moved.len()
}

/// Refresh the apps of several channels. Re-creating a stream makes some players (Spotify) pause; media that was playing
/// before and is paused afterwards is resumed through the system media controls (SMTC), so the user only hears a short gap.
pub fn refresh_channels(eps: &[crate::Ep], keys: &[&str]) {
    let Ok(pol) = Policy::new() else { return };
    let was = std::thread::spawn(|| media(|s| s.PlaybackStatus().map_or(false, |p| p == Pb::Playing))).join().unwrap_or_default();
    for k in keys { refresh_channel(&pol, eps, k); }
    if was.is_empty() { return; }
    let _ = std::thread::spawn(move || {
        let mut left = was;
        for _ in 0..12 { // up to ~2 s: players pause a moment after their stream was re-created
            std::thread::sleep(std::time::Duration::from_millis(150));
            let Ok(ms) = Smtc::RequestAsync().and_then(|o| o.get()).and_then(|m| m.GetSessions()) else { return };
            for s in ms {
                let Ok(id) = s.SourceAppUserModelId().map(|h| h.to_string()) else { continue };
                let Some(i) = left.iter().position(|l| *l == id) else { continue };
                match s.GetPlaybackInfo().and_then(|p| p.PlaybackStatus()) {
                    Ok(Pb::Playing) => { left.remove(i); }
                    Ok(Pb::Paused) => { let _ = s.TryPlayAsync().and_then(|o| o.get()); left.remove(i); }
                    _ => {}
                }
            }
            if left.is_empty() { return; }
        }
    }).join();
}

/// Source app ids of the media sessions whose playback info matches `f`.
fn media(f: impl Fn(&windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackInfo) -> bool) -> Vec<String> {
    let Ok(ms) = Smtc::RequestAsync().and_then(|o| o.get()).and_then(|m| m.GetSessions()) else { return vec![] };
    ms.into_iter().filter(|s| s.GetPlaybackInfo().map_or(false, |p| f(&p))).filter_map(|s| s.SourceAppUserModelId().ok().map(|h| h.to_string())).collect()
}
