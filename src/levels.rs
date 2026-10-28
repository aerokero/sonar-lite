// Background worker for the four Sonar channel endpoints:
//  * live level: peak meter of each endpoint (what the apps play into the channel, before its volume), smoothed;
//  * live volume: the fader is the endpoint's Windows volume (set as a dB level, so 50% = half the amplitude). Unlike the APO's
//    registry gain it reaches streams that are already playing, so dragging a fader is smooth and needs no app refresh.
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::time::{Duration, Instant};
use windows::{core::HSTRING, Win32::{Media::Audio::{Endpoints::{IAudioEndpointVolume, IAudioMeterInformation}, *}, System::Com::*}};
use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};

const KEYS: [&str; 4] = ["game", "chat", "media", "aux"];
const NONE: u32 = f32::NAN.to_bits();
static LEVEL: [AtomicU32; 4] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)];
static WANT: [AtomicU32; 4] = [AtomicU32::new(NONE), AtomicU32::new(NONE), AtomicU32::new(NONE), AtomicU32::new(NONE)];
static DIRTY: [AtomicBool; 4] = [AtomicBool::new(false), AtomicBool::new(false), AtomicBool::new(false), AtomicBool::new(false)];

fn idx(key: &str) -> Option<usize> { KEYS.iter().position(|k| *k == key) }

/// Latest peak of a channel (0 = silence, 1 = full scale; 0 for an unknown key).
pub fn get(key: &str) -> f32 { idx(key).map_or(0.0, |i| f32::from_bits(LEVEL[i].load(Relaxed))) }

/// Fader position (0..1) -> linear amplitude. Cubic, so equal fader steps sound like equal loudness steps (50% = -18 dB).
pub fn fader_to_amp(v: f32) -> f32 { v.clamp(0.0, 1.0).powi(3) }

/// Set a channel's fader position (0..1; applied within ~30 ms, and again whenever the devices are re-created).
pub fn set_gain(key: &str, fader: f32) {
    if let Some(i) = idx(key) { WANT[i].store(fader_to_amp(fader).to_bits(), Relaxed); DIRTY[i].store(true, Relaxed); }
}

/// dB level for a linear gain, clamped to the endpoint's range (silence = its minimum).
fn to_db(gain: f32, lo: f32, hi: f32) -> f32 { if gain <= 0.0 { lo } else { (20.0 * gain.log10()).clamp(lo, hi) } }

pub fn spawn() {
    std::thread::spawn(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return };
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let mut meters: Vec<Option<IAudioMeterInformation>> = vec![None, None, None, None];
        let mut vols: Vec<Option<(IAudioEndpointVolume, f32, f32)>> = vec![None, None, None, None];
        let mut built = Instant::now() - Duration::from_secs(60);
        loop {
            if built.elapsed() > Duration::from_secs(3) { // devices can come and go (GG, enabling the channels)
                let eps = crate::endpoints(&hklm);
                for i in 0..4 {
                    let dev = crate::sonar_guid(&eps, KEYS[i]).and_then(|g| en.GetDevice(&HSTRING::from(format!("{{0.0.0.00000000}}.{{{g}}}"))).ok());
                    meters[i] = dev.as_ref().and_then(|d| d.Activate::<IAudioMeterInformation>(CLSCTX_ALL, None).ok());
                    let old = vols[i].is_some();
                    vols[i] = dev.and_then(|d| d.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()).map(|v| {
                        let (mut lo, mut hi, mut inc) = (-96.0f32, 0.0f32, 0.0f32);
                        let _ = v.GetVolumeRange(&mut lo, &mut hi, &mut inc);
                        (v, lo, hi)
                    });
                    if !old && vols[i].is_some() { DIRTY[i].store(true, Relaxed); } // a (re)created device starts at its own volume
                }
                built = Instant::now();
            }
            for i in 0..4 {
                let now = meters[i].as_ref().and_then(|m| m.GetPeakValue().ok()).unwrap_or(0.0);
                let old = f32::from_bits(LEVEL[i].load(Relaxed));
                LEVEL[i].store(now.max(old * 0.88).to_bits(), Relaxed); // quick attack, smooth release
                let want = f32::from_bits(WANT[i].load(Relaxed));
                if DIRTY[i].load(Relaxed) && !want.is_nan() {
                    if let Some((v, lo, hi)) = &vols[i] {
                        DIRTY[i].store(false, Relaxed);
                        let _ = v.SetMasterVolumeLevel(to_db(want, *lo, *hi), std::ptr::null());
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(33));
        }
    });
}

#[cfg(test)]
mod t {
    use super::*;
    #[test]
    fn gain_maps_to_db_within_the_endpoint_range() {
        assert_eq!(to_db(1.0, -96.0, 0.0), 0.0);
        assert!((to_db(0.5, -96.0, 0.0) + 6.02).abs() < 0.01); // half the amplitude
        assert_eq!(to_db(0.0, -96.0, 0.0), -96.0); // silence = the lowest the device allows
        assert_eq!(to_db(1e-9, -60.0, 0.0), -60.0); // never below the range
        assert!((to_db(fader_to_amp(0.5), -96.0, 0.0) + 18.06).abs() < 0.01); // fader 50% = -18 dB
        assert_eq!(fader_to_amp(1.0), 1.0);
        assert_eq!(idx("aux"), Some(3));
        assert_eq!(idx("mic"), None);
    }
}
