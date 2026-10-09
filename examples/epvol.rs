// READ-ONLY: master volume (0..1) and mute of the Sonar - Microphone render and capture endpoints.
use windows::{core::*, Win32::{Media::Audio::{Endpoints::IAudioEndpointVolume, *}, System::Com::*}};
fn main() -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        for (name, id) in [("render  Sonar - Gaming    ", "{0.0.0.00000000}.{c3f8c90f-385b-455d-b8ef-55d5296f319f}"),
                           ("render  Sonar - Chat      ", "{0.0.0.00000000}.{616607be-396f-4520-ac5b-b3ac4afa6e58}"),
                           ("render  Sonar - Media     ", "{0.0.0.00000000}.{aa848f1f-00ae-4ab3-a347-9d9d2eab8af0}"),
                           ("render  Sonar - Aux       ", "{0.0.0.00000000}.{20ed651a-80a4-49b1-ada6-addcceafc8f4}"),("render  Sonar - Microphone", "{0.0.0.00000000}.{36c4ef42-424b-49a7-b553-d9a65cc49749}"),
                           ("capture Sonar - Microphone", "{0.0.1.00000000}.{d10b0391-ad84-4792-8e28-659e4bb0b442}"),
                           ("capture Arctis mic        ", "{0.0.1.00000000}.{50c2b6d5-b8a8-4742-b0d1-794121a456eb}")] {
            let d = en.GetDevice(&HSTRING::from(id))?;
            let v: IAudioEndpointVolume = d.Activate(CLSCTX_ALL, None)?;
            println!("{name}: volume {:.2}  mute {}", v.GetMasterVolumeLevelScalar()?, v.GetMute()?.as_bool());
        }
    }
    Ok(())
}
