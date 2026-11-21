//! The Nova 5 receiver stays active in Windows when the wireless headset is off.
use hidapi::{HidApi, HidDevice};
use std::{sync::{Arc, atomic::{AtomicU8, Ordering::Relaxed}}, thread, time::Duration};

const UNKNOWN: u8 = 0;
const OFFLINE: u8 = 0x02;
const ONLINE: u8 = 0x03;

fn connection(report: &[u8], opcode: u8) -> Option<u8> {
    match report {
        [op, state @ (OFFLINE | ONLINE), ..] if *op == opcode => Some(*state),
        _ => None,
    }
}

fn open(api: &HidApi, interface: i32) -> Option<HidDevice> {
    api.device_list()
        .find(|d| (d.vendor_id(), d.product_id(), d.interface_number()) == (0x1038, 0x2232, interface))?
        .open_device(api).ok()
}

fn publish(state: &AtomicU8, value: u8, wake: &impl Fn()) {
    if state.swap(value, Relaxed) != value { wake(); }
}

fn monitor(api: &HidApi, state: &AtomicU8, wake: &impl Fn()) {
    let Some(events) = open(api, 5) else { return };
    let mut report = [0; 64];
    // Query the current link once, then use the receiver's unsolicited power reports.
    // Repeated status requests can interrupt audio on this model.
    // https://github.com/loteran/Arctis-Sound-Manager/issues/348
    if let Some(commands) = open(api, 3) {
        if commands.write(&[0, 0xb0]).is_ok() {
            if let Ok(n) = commands.read_timeout(&mut report, 1000) {
                if let Some(value) = connection(&report[..n], 0xb0) { publish(state, value, wake); }
            }
        }
    }
    while let Ok(n) = events.read_timeout(&mut report, 1000) {
        if let Some(value) = connection(&report[..n], 0xb9) { publish(state, value, wake); }
    }
}

#[derive(Default)]
pub struct Link(Arc<AtomicU8>);

impl Link {
    pub fn start(&self, wake: impl Fn() + Send + 'static) {
        let state = self.0.clone();
        thread::spawn(move || {
            let Ok(mut api) = HidApi::new() else { return };
            loop {
                monitor(&api, &state, &wake);
                publish(&state, UNKNOWN, &wake);
                thread::sleep(Duration::from_secs(3));
                let _ = api.refresh_devices();
            }
        });
    }

    pub fn permits(&self, endpoint: &str) -> bool {
        !self.is_offline() || !endpoint.ends_with("(SteelSeries Arctis Nova 5)")
    }

    pub fn is_offline(&self) -> bool { self.0.load(Relaxed) == OFFLINE }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_reports_filter_only_the_nova_5_output() {
        let link = Link::default();
        let headset = "Słuchawki (SteelSeries Arctis Nova 5)";
        assert!(link.permits(headset));
        link.0.store(connection(&[0xb0, 2, 0], 0xb0).unwrap(), Relaxed);
        assert!(!link.permits(headset));
        assert!(link.permits("G27QC (NVIDIA High Definition Audio)"));
        link.0.store(connection(&[0xb9, 3, 2], 0xb9).unwrap(), Relaxed);
        assert!(link.permits(headset));
        assert_eq!(connection(&[0xb7, 2], 0xb9), None);
        assert_eq!(connection(&[0xb9], 0xb9), None);
    }
}
