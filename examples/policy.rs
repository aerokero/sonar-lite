// READ-ONLY probe: can we reach IAudioPolicyConfigFactory and read a process's persisted output device?
// usage: policy <pid>
use std::ffi::c_void;
use windows::{core::*, Win32::System::{Com::*, WinRT::*}};

fn main() -> Result<()> {
    let pid: u32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).expect("pid");
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let f: IInspectable = RoGetActivationFactory(&HSTRING::from("Windows.Media.Internal.AudioPolicyConfig"))?;
        println!("factory ok");
        for iid in ["2a59116d-6c4f-45e0-a74f-707e3fef9258", "ab3d4648-e242-459f-b02f-541c70306324", "f8679f50-850a-41cf-9c72-430f290290c8"] {
            let g = GUID::from(iid);
            let mut p: *mut c_void = std::ptr::null_mut();
            let hr = f.query(&g, &mut p);
            println!("QI {iid}: {hr:?}");
            if hr.is_ok() {
                // vtable: 3 IUnknown + 3 IInspectable + 19 unused + Set(25) + Get(26)
                let vt = *(p as *const *const usize);
                type Get = unsafe extern "system" fn(*mut c_void, u32, i32, i32, *mut HSTRING) -> HRESULT;
                let get: Get = std::mem::transmute(*vt.add(26));
                for (flow, role) in [(0, 0), (0, 1), (0, 2), (1, 0)] {
                    let mut s = HSTRING::new();
                    let hr = get(p, pid, flow, role, &mut s);
                    println!("  Get(pid={pid}, flow={flow}, role={role}) -> {hr:?} {:?}", s.to_string());
                }
            }
        }
    }
    Ok(())
}
