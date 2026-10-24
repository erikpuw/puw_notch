//! Bluetooth: radio on/off (Windows.Devices.Radios) and devices connecting / disconnecting
//! (Windows.Devices.Enumeration). The WinRT calls block, so they run on worker threads that post
//! back to the notch window.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::DeviceInformation;
use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// Posted to the window with WPARAM = `Bt as usize`.
pub const WM_BT: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 2;
/// Posted when devices connected or disconnected; read them with `take_device_events`.
pub const WM_BT_DEVICE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 6;
const DEVICE_POLL: Duration = Duration::from_secs(2);

/// (device name, true = connected / false = disconnected), filled by the watcher thread.
static DEVICE_EVENTS: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());

pub fn take_device_events() -> Vec<(String, bool)> {
    DEVICE_EVENTS.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

/// Connected devices by id -> name, classic and Low Energy.
fn connected() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let selectors = [
        BluetoothDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected),
        BluetoothLEDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected),
    ];
    for sel in selectors.into_iter().flatten() {
        let Ok(found) = DeviceInformation::FindAllAsyncAqsFilter(&sel).and_then(|op| op.get()) else { continue };
        for d in found {
            if let (Ok(id), Ok(name)) = (d.Id(), d.Name()) {
                out.insert(id.to_string(), name.to_string());
            }
        }
    }
    out
}

/// Names that appeared (true) or disappeared (false) between two snapshots.
/// A device seen over both classic and LE with the same name is reported once.
pub fn diff(before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> Vec<(String, bool)> {
    let names = |m: &BTreeMap<String, String>| m.values().cloned().collect::<std::collections::BTreeSet<_>>();
    let (b, a) = (names(before), names(after));
    let mut out: Vec<_> = a.difference(&b).map(|n| (n.clone(), true)).collect();
    out.extend(b.difference(&a).map(|n| (n.clone(), false)));
    out
}

/// Watch for devices connecting / disconnecting; devices already connected at start are not announced.
pub fn watch_devices(hwnd: HWND) {
    let target = hwnd.0 as isize;
    std::thread::spawn(move || {
        unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
        let mut last = connected();
        loop {
            std::thread::sleep(DEVICE_POLL);
            let now = connected();
            let events = diff(&last, &now);
            last = now;
            if events.is_empty() {
                continue;
            }
            if let Ok(mut q) = DEVICE_EVENTS.lock() {
                q.extend(events);
            }
            unsafe { let _ = PostMessageW(Some(HWND(target as _)), WM_BT_DEVICE, WPARAM(0), LPARAM(0)); }
        }
    });
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Bt {
    Unknown = 0,
    Off = 1,
    On = 2,
    /// No Bluetooth adapter, or Windows refused access.
    Missing = 3,
}

impl Bt {
    pub fn from_wparam(w: usize) -> Bt {
        match w {
            1 => Bt::Off,
            2 => Bt::On,
            3 => Bt::Missing,
            _ => Bt::Unknown,
        }
    }
}

fn radio() -> windows::core::Result<Option<Radio>> {
    if Radio::RequestAccessAsync()?.get()? != RadioAccessStatus::Allowed {
        return Ok(None);
    }
    let radios = Radio::GetRadiosAsync()?.get()?;
    for r in radios {
        if r.Kind()? == RadioKind::Bluetooth {
            return Ok(Some(r));
        }
    }
    Ok(None)
}

fn state(toggle: bool) -> Bt {
    let Ok(Some(r)) = radio() else { return Bt::Missing };
    let Ok(mut on) = r.State().map(|s| s == RadioState::On) else { return Bt::Missing };
    if toggle {
        let want = if on { RadioState::Off } else { RadioState::On };
        if r.SetStateAsync(want).and_then(|op| op.get()).is_ok() {
            on = !on;
        }
    }
    if on { Bt::On } else { Bt::Off }
}

/// Read (and optionally flip) the radio in the background, then post WM_BT to `hwnd`.
pub fn request(hwnd: HWND, toggle: bool) {
    let target = hwnd.0 as isize;
    std::thread::spawn(move || {
        unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
        let s = state(toggle);
        unsafe { let _ = PostMessageW(Some(HWND(target as _)), WM_BT, WPARAM(s as usize), LPARAM(0)); }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(items: &[(&str, &str)]) -> BTreeMap<String, String> {
        items.iter().map(|(i, n)| (i.to_string(), n.to_string())).collect()
    }

    #[test]
    fn connect_and_disconnect() {
        let before = map(&[("a", "Chuột MX"), ("b-classic", "Tai nghe WH-1000")]);
        // headphones also show up over LE: still one device, nothing new
        let same = map(&[("a", "Chuột MX"), ("b-classic", "Tai nghe WH-1000"), ("b-le", "Tai nghe WH-1000")]);
        assert!(diff(&before, &same).is_empty());
        let after = map(&[("b-le", "Tai nghe WH-1000"), ("c", "Loa JBL")]);
        assert_eq!(diff(&before, &after), vec![("Loa JBL".to_string(), true), ("Chuột MX".to_string(), false)]);
    }
}
