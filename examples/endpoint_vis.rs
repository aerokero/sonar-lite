// usage: endpoint_vis <endpoint id like {0.0.0.00000000}.{guid}> <0|1>
// Same call GG makes: IPolicyConfig::SetEndpointVisibility(id, short) on CLSID 870AF99C-171D-4F9E-AF0D-E63DF40C2BC9
// (IPolicyConfig F8679F50-..., vtable: 3 IUnknown + 8 unused + GetPropertyValue, SetPropertyValue, SetDefaultEndpoint, SetEndpointVisibility = slot 14).
use std::ffi::c_void;
use windows::{core::*, Win32::System::Com::*};

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (id, vis) = (a[0].clone(), a[1] == "1");
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let unk: IUnknown = CoCreateInstance(&GUID::from("870AF99C-171D-4F9E-AF0D-E63DF40C2BC9"), None, CLSCTX_ALL)?;
        let mut p: *mut c_void = std::ptr::null_mut();
        let hr = unk.query(&GUID::from("F8679F50-850A-41CF-9C72-430F290290C8"), &mut p);
        println!("QI IPolicyConfig: {hr:?}");
        hr.ok()?;
        let vt = *(p as *const *const usize);
        type SetVis = unsafe extern "system" fn(*mut c_void, PCWSTR, i16) -> HRESULT;
        let f: SetVis = std::mem::transmute(*vt.add(14));
        let w = HSTRING::from(id.as_str());
        let r = f(p, PCWSTR(w.as_ptr()), vis as i16);
        println!("SetEndpointVisibility({id}, {vis}) -> {r:?}");
    }
    Ok(())
}
