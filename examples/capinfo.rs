// READ-ONLY: every render and capture audio session: endpoint (id + name from the registry), process, state.
use windows::{core::*, Win32::{Media::Audio::*, System::Com::*, System::Threading::*, Foundation::*}};
use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};
fn pname(pid: u32) -> String {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return "?".into() };
        let mut b = [0u16; 520]; let mut n = b.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(b.as_mut_ptr()), &mut n); let _ = CloseHandle(h);
        if r.is_err() { return "?".into(); }
        String::from_utf16_lossy(&b[..n as usize]).rsplit('\\').next().unwrap_or("?").to_string()
    }
}
fn dev_name(kind: &str, guid: &str) -> String {
    let k = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\{kind}\{{{guid}}}\Properties"));
    k.ok().map(|k| format!("{} ({})", k.get_value::<String, _>("{a45c254e-df1c-4efd-8020-67d146a850e0},2").unwrap_or_default(), k.get_value::<String, _>("{b3f8fa53-0004-438e-9003-51a46e139bfc},6").unwrap_or_default())).unwrap_or_default()
}
fn main() -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        for (flow, label) in [(eRender, "Render"), (eCapture, "Capture")] {
            let devs = en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
            for i in 0..devs.GetCount()? {
                let d = devs.Item(i)?;
                let id = d.GetId()?.to_string()?;
                let guid = id.split_once(".{").map(|x| x.1.trim_end_matches('}').to_string()).unwrap_or_default();
                let n = dev_name(label, &guid);
                let Ok(m) = d.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
                let se = m.GetSessionEnumerator()?;
                for j in 0..se.GetCount()? {
                    let c = se.GetSession(j)?; let c2 = c.cast::<IAudioSessionControl2>()?;
                    let pid = c2.GetProcessId().unwrap_or(0);
                    if pid != 0 { println!("{label:7} {n:62} {:>6} {:<18} state {}", pid, pname(pid), c.GetState()?.0); }
                }
            }
        }
    }
    Ok(())
}
