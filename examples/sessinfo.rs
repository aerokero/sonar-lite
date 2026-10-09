// READ-ONLY: every render audio session with its endpoint, state, per-session volume/mute and current peak.
use windows::{core::*, Win32::{Media::Audio::{Endpoints::IAudioMeterInformation, *}, System::Com::*}};
fn main() -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let devs = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        for i in 0..devs.GetCount()? {
            let dev = devs.Item(i)?;
            let id = dev.GetId()?.to_string()?;
            let dpeak = dev.Activate::<IAudioMeterInformation>(CLSCTX_ALL, None).and_then(|m| m.GetPeakValue()).unwrap_or(-1.0);
            let Ok(m) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
            let se = m.GetSessionEnumerator()?;
            println!("{} peak {dpeak:.3} sessions {}", &id[id.len().saturating_sub(37)..], se.GetCount()?);
            for j in 0..se.GetCount()? {
                let c = se.GetSession(j)?;
                let c2 = c.cast::<IAudioSessionControl2>()?;
                let v = c.cast::<ISimpleAudioVolume>()?;
                let pk = c.cast::<IAudioMeterInformation>().and_then(|m| m.GetPeakValue()).unwrap_or(-1.0);
                println!("    pid {:>6} state {:?} vol {:.2} mute {} peak {pk:.3}", c2.GetProcessId().unwrap_or(0), c.GetState()?.0, v.GetMasterVolume()?, v.GetMute()?.as_bool());
            }
        }
    }
    Ok(())
}
