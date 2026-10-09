// epset <game|chat|media|aux> <linear gain 0..1> : set a Sonar channel endpoint's Windows volume so that amplitude = gain (dB level).
use windows::{core::*, Win32::{Media::Audio::{Endpoints::IAudioEndpointVolume, *}, System::Com::*}};
fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let id = match a.first().map(String::as_str) {
        Some("game") => "c3f8c90f-385b-455d-b8ef-55d5296f319f", Some("chat") => "616607be-396f-4520-ac5b-b3ac4afa6e58",
        Some("media") => "aa848f1f-00ae-4ab3-a347-9d9d2eab8af0", Some("aux") => "20ed651a-80a4-49b1-ada6-addcceafc8f4", _ => panic!("channel"),
    };
    let g: f32 = a.get(1).and_then(|v| v.parse().ok()).expect("gain");
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let v: IAudioEndpointVolume = en.GetDevice(&HSTRING::from(format!("{{0.0.0.00000000}}.{{{id}}}")))?.Activate(CLSCTX_ALL, None)?;
        let (mut lo, mut hi, mut inc) = (0f32, 0f32, 0f32);
        v.GetVolumeRange(&mut lo, &mut hi, &mut inc)?;
        println!("range {lo} .. {hi} dB step {inc}; was {:.2} dB", v.GetMasterVolumeLevel()?);
        let db = if g <= 0.0 { lo } else { (20.0 * g.log10()).clamp(lo, hi) };
        v.SetMasterVolumeLevel(db, std::ptr::null())?;
        println!("now {:.2} dB", v.GetMasterVolumeLevel()?);
    }
    Ok(())
}
