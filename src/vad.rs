// Sonar's virtual audio devices ("SteelSeries Sonar - Gaming/Chat/Media/Aux/Microphone") are ordinary Windows audio endpoints that
// GG switches on at start and *disables* when it exits normally. Without GG they must be switched back on by us, with the same
// call GG uses (found by decompiling its SoundStage.Interop.Services): IPolicyConfig::SetEndpointVisibility(id, short).
use std::ffi::c_void;
use windows::{core::*, Win32::System::Com::*};
use winreg::RegKey;

const MM: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio";

/// Enable or disable one endpoint. `id` is "{0.0.0.00000000}.{guid}" for render and "{0.0.1.00000000}.{guid}" for capture devices.
pub fn set_visible(id: &str, visible: bool) -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        // CLSID of CPolicyConfigClient (Windows 10+) and IID of IPolicyConfig
        let unk: IUnknown = CoCreateInstance(&GUID::from("870AF99C-171D-4F9E-AF0D-E63DF40C2BC9"), None, CLSCTX_ALL)?;
        let mut p: *mut c_void = std::ptr::null_mut();
        unk.query(&GUID::from("F8679F50-850A-41CF-9C72-430F290290C8"), &mut p).ok()?;
        // vtable: 3 IUnknown + 8 unused + GetPropertyValue, SetPropertyValue, SetDefaultEndpoint, then SetEndpointVisibility = slot 14
        let vt = *(p as *const *const usize);
        type SetVis = unsafe extern "system" fn(*mut c_void, PCWSTR, i16) -> HRESULT;
        let f: SetVis = std::mem::transmute(*vt.add(14));
        let w = HSTRING::from(id);
        let r = f(p, PCWSTR(w.as_ptr()), visible as i16);
        drop(IUnknown::from_raw(p)); // balance the reference taken by query
        r.ok()
    }
}

/// Endpoints of Sonar's channels that Windows currently has switched off. Returns their ids.
/// (Stream endpoints are left alone: the stream mix goes to a virtual cable, and GG only enables them in its own streamer mode.)
pub fn disabled_channel_endpoints(hklm: &RegKey) -> Vec<String> {
    let mut out = vec![];
    for (flow, prefix) in [("Render", "{0.0.0.00000000}"), ("Capture", "{0.0.1.00000000}")] {
        let Ok(root) = hklm.open_subkey(format!(r"{MM}\{flow}")) else { continue };
        for g in root.enum_keys().flatten() {
            let Ok(k) = root.open_subkey(&g) else { continue };
            let state: u32 = k.get_value("DeviceState").unwrap_or(0);
            let name: String = k.open_subkey("Properties").ok()
                .and_then(|p| p.get_value::<String, _>("{a45c254e-df1c-4efd-8020-67d146a850e0},2").ok()).unwrap_or_default();
            // 0x10000001 = "disabled"; plain 1 = active
            if name.starts_with("SteelSeries Sonar - ") && !name.contains("Stream") && state == 0x1000_0001 {
                out.push(format!("{prefix}.{g}"));
            }
        }
    }
    out
}

/// Switch every disabled Sonar channel endpoint back on. Returns how many were enabled (needs administrator rights).
pub fn ensure_enabled(hklm: &RegKey) -> usize {
    disabled_channel_endpoints(hklm).iter().filter(|id| set_visible(id, true).is_ok()).count()
}
