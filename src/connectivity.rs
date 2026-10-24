//! Connectivity watcher: Wi-Fi / Bluetooth / mobile radios on-off, airplane mode, the network in use
//! (and whether it reaches the internet) and Bluetooth devices connecting. WinRT calls block, so a
//! worker polls every 2 s, diffs against the last snapshot and queues events for the notch window.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::Duration;

use windows::core::w;
use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::DeviceInformation;
use windows::Devices::Radios::{Radio, RadioKind, RadioState};
use windows::Networking::Connectivity::{NetworkConnectivityLevel, NetworkInformation};
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Posted when events are queued; read them with `take_events`.
pub const WM_CONN: u32 = WM_APP + 6;
const POLL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum RadioType {
    Wifi,
    Bluetooth,
    Mobile,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub enum Net {
    #[default]
    Offline,
    /// Reaches the internet. `name` = Wi-Fi SSID or the connection name (e.g. "Ethernet").
    Online { name: String, wifi: bool },
    /// Connected to a network that has no internet.
    Limited { name: String, wifi: bool },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DeviceKind {
    Headphones,
    Speaker,
    Phone,
    Mouse,
    Keyboard,
    Gamepad,
    Other,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Snapshot {
    pub radios: BTreeMap<RadioType, bool>,
    pub airplane: bool,
    pub net: Net,
    /// Connected Bluetooth devices: id -> (name, kind).
    pub devices: BTreeMap<String, (String, DeviceKind)>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Event {
    Radio(RadioType, bool),
    Airplane(bool),
    Net(Net),
    Device { name: String, kind: DeviceKind, up: bool },
}

/// What changed between two snapshots, without the knock-on noise: turning airplane mode on
/// is one event, not also "Wi-Fi off", "offline" and "headphones disconnected".
pub fn changes(old: &Snapshot, new: &Snapshot) -> Vec<Event> {
    let mut out = Vec::new();
    let airplane_flip = old.airplane != new.airplane;
    if airplane_flip {
        out.push(Event::Airplane(new.airplane));
    }
    let mut radio_off = BTreeSet::new();
    if !airplane_flip {
        for (kind, on) in &new.radios {
            if old.radios.get(kind).is_some_and(|was| was != on) {
                out.push(Event::Radio(*kind, *on));
                if !on {
                    radio_off.insert(*kind);
                }
            }
        }
    }
    let quiet_offline = airplane_flip || !radio_off.is_empty();
    if old.net != new.net && !(new.net == Net::Offline && quiet_offline) {
        out.push(Event::Net(new.net.clone()));
    }
    // Devices, by name (a headset can show up over classic and LE at once).
    let names = |s: &Snapshot| s.devices.values().map(|(n, k)| (n.clone(), *k)).collect::<BTreeMap<_, _>>();
    let (before, after) = (names(old), names(new));
    let quiet_disconnect = new.airplane || radio_off.contains(&RadioType::Bluetooth);
    for (name, kind) in &after {
        if !before.contains_key(name) {
            out.push(Event::Device { name: name.clone(), kind: *kind, up: true });
        }
    }
    if !quiet_disconnect {
        for (name, kind) in &before {
            if !after.contains_key(name) {
                out.push(Event::Device { name: name.clone(), kind: *kind, up: false });
            }
        }
    }
    out
}

/// Kind from the Bluetooth class of device (major/minor numbers, 0 = unknown, e.g. LE devices)
/// and, where the class says nothing useful (keyboards and mice share one class), the name.
pub fn device_kind(major: i32, minor: i32, name: &str) -> DeviceKind {
    use DeviceKind::*;
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    match major {
        // AudioVideo: loudspeaker, hi-fi, display+speaker -> speaker; the rest is worn
        4 => return if matches!(minor, 5 | 10 | 15) { Speaker } else { Headphones },
        2 => return Phone,
        5 if matches!(minor, 1 | 2) => return Gamepad,
        _ => {}
    }
    if has(&["mouse", "chuột", "mx master", "trackball"]) {
        Mouse
    } else if has(&["keyboard", "bàn phím", "keys"]) {
        Keyboard
    } else if has(&["buds", "airpods", "headphone", "headset", "tai nghe", "hitune", "wh-", "wf-", "earbud"]) {
        Headphones
    } else if has(&["speaker", "loa", "jbl", "soundlink"]) {
        Speaker
    } else if has(&["iphone", "phone", "galaxy", "pixel", "redmi", "oppo"]) {
        Phone
    } else if has(&["controller", "gamepad", "xbox", "dualsense", "dualshock"]) {
        Gamepad
    } else {
        Other
    }
}

static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());

pub fn take_events() -> Vec<Event> {
    EVENTS.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

/// Start the watcher. What is already on / connected at start is not announced.
pub fn watch(hwnd: HWND) {
    let target = hwnd.0 as isize;
    std::thread::spawn(move || {
        unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
        let _ = Radio::RequestAccessAsync().and_then(|op| op.get());
        // Class lookups are a round trip each; remember them per device id.
        let mut kinds: BTreeMap<String, DeviceKind> = BTreeMap::new();
        let mut last = snapshot(&mut kinds);
        loop {
            std::thread::sleep(POLL);
            let now = snapshot(&mut kinds);
            let events = changes(&last, &now);
            last = now;
            if events.is_empty() {
                continue;
            }
            if let Ok(mut q) = EVENTS.lock() {
                q.extend(events);
            }
            unsafe { let _ = PostMessageW(Some(HWND(target as _)), WM_CONN, WPARAM(0), LPARAM(0)); }
        }
    });
}

fn snapshot(kinds: &mut BTreeMap<String, DeviceKind>) -> Snapshot {
    Snapshot { radios: radios(), airplane: airplane(), net: network(), devices: devices(kinds) }
}

fn radios() -> BTreeMap<RadioType, bool> {
    let mut out = BTreeMap::new();
    let Ok(list) = Radio::GetRadiosAsync().and_then(|op| op.get()) else { return out };
    for r in list {
        let kind = match r.Kind() {
            Ok(RadioKind::WiFi) => RadioType::Wifi,
            Ok(RadioKind::Bluetooth) => RadioType::Bluetooth,
            Ok(RadioKind::MobileBroadband) => RadioType::Mobile,
            _ => continue,
        };
        if let Ok(state) = r.State() {
            out.insert(kind, state == RadioState::On);
        }
    }
    out
}

/// Windows keeps the airplane switch here: 1 = on.
fn airplane() -> bool {
    let mut value = 0u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SYSTEM\\CurrentControlSet\\Control\\RadioManagement\\SystemRadioState"),
            None,
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    r == ERROR_SUCCESS && value == 1
}

fn network() -> Net {
    let Ok(p) = NetworkInformation::GetInternetConnectionProfile() else { return Net::Offline };
    let level = p.GetNetworkConnectivityLevel().unwrap_or(NetworkConnectivityLevel::None);
    let wifi = p.IsWlanConnectionProfile().unwrap_or(false);
    let ssid = if wifi { p.WlanConnectionProfileDetails().and_then(|d| d.GetConnectedSsid()).ok() } else { None };
    let name = ssid.or_else(|| p.ProfileName().ok()).map(|n| n.to_string()).unwrap_or_default();
    match level {
        NetworkConnectivityLevel::InternetAccess => Net::Online { name, wifi },
        NetworkConnectivityLevel::None => Net::Offline,
        _ => Net::Limited { name, wifi },
    }
}

fn devices(kinds: &mut BTreeMap<String, DeviceKind>) -> BTreeMap<String, (String, DeviceKind)> {
    let mut out = BTreeMap::new();
    let selectors = [
        BluetoothDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected),
        BluetoothLEDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected),
    ];
    for sel in selectors.into_iter().flatten() {
        let Ok(found) = DeviceInformation::FindAllAsyncAqsFilter(&sel).and_then(|op| op.get()) else { continue };
        for d in found {
            let (Ok(id), Ok(name)) = (d.Id(), d.Name()) else { continue };
            let (id, name) = (id.to_string(), name.to_string());
            let kind = *kinds.entry(id.clone()).or_insert_with(|| {
                let (major, minor) = class_of(&id);
                device_kind(major, minor, &name)
            });
            out.insert(id, (name, kind));
        }
    }
    out
}

/// Class of device for a classic Bluetooth id; LE ids and failures give (0, 0).
fn class_of(id: &str) -> (i32, i32) {
    let Ok(dev) = BluetoothDevice::FromIdAsync(&id.into()).and_then(|op| op.get()) else { return (0, 0) };
    let Ok(c) = dev.ClassOfDevice() else { return (0, 0) };
    (c.MajorClass().map_or(0, |m| m.0), c.MinorClass().map_or(0, |m| m.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use DeviceKind::*;

    fn snap(wifi: bool, bt: bool, airplane: bool, net: Net, devices: &[(&str, &str, DeviceKind)]) -> Snapshot {
        Snapshot {
            radios: [(RadioType::Wifi, wifi), (RadioType::Bluetooth, bt)].into_iter().collect(),
            airplane,
            net,
            devices: devices.iter().map(|(i, n, k)| (i.to_string(), (n.to_string(), *k))).collect(),
        }
    }

    fn home() -> Net {
        Net::Online { name: "JihoonKim".into(), wifi: true }
    }

    #[test]
    fn nothing_changed() {
        let a = snap(true, true, false, home(), &[("h", "UGREEN HiTune Max5c", Headphones)]);
        assert!(changes(&a, &a.clone()).is_empty());
    }

    #[test]
    fn wifi_off_is_one_event() {
        let a = snap(true, true, false, home(), &[]);
        let b = snap(false, true, false, Net::Offline, &[]);
        assert_eq!(changes(&a, &b), vec![Event::Radio(RadioType::Wifi, false)]);
        // and back on, then the network comes up a poll later
        let c = snap(true, true, false, Net::Offline, &[]);
        assert_eq!(changes(&b, &c), vec![Event::Radio(RadioType::Wifi, true)]);
        assert_eq!(changes(&c, &a), vec![Event::Net(home())]);
    }

    #[test]
    fn airplane_swallows_the_rest() {
        let a = snap(true, true, false, home(), &[("h", "Tai nghe", Headphones)]);
        let b = snap(false, false, true, Net::Offline, &[]);
        assert_eq!(changes(&a, &b), vec![Event::Airplane(true)]);
        let c = snap(true, true, false, Net::Offline, &[]);
        assert_eq!(changes(&b, &c), vec![Event::Airplane(false)]);
    }

    #[test]
    fn network_changes() {
        let a = snap(true, true, false, home(), &[]);
        let limited = snap(true, true, false, Net::Limited { name: "Cafe".into(), wifi: true }, &[]);
        assert_eq!(changes(&a, &limited), vec![Event::Net(Net::Limited { name: "Cafe".into(), wifi: true })]);
        let lost = snap(true, true, false, Net::Offline, &[]);
        assert_eq!(changes(&a, &lost), vec![Event::Net(Net::Offline)]);
    }

    #[test]
    fn devices_by_name() {
        let a = snap(true, true, false, home(), &[("m", "MX Master", Mouse), ("hc", "WH-1000", Headphones)]);
        // headset also over LE: same device, nothing new
        let same = snap(true, true, false, home(), &[("m", "MX Master", Mouse), ("hc", "WH-1000", Headphones), ("hl", "WH-1000", Headphones)]);
        assert!(changes(&a, &same).is_empty());
        let b = snap(true, true, false, home(), &[("hl", "WH-1000", Headphones), ("j", "Loa JBL", Speaker)]);
        assert_eq!(
            changes(&a, &b),
            vec![Event::Device { name: "Loa JBL".into(), kind: Speaker, up: true }, Event::Device { name: "MX Master".into(), kind: Mouse, up: false }]
        );
        // Bluetooth switched off: one event, devices dropping with it are not listed
        let off = snap(true, false, false, home(), &[]);
        assert_eq!(changes(&a, &off), vec![Event::Radio(RadioType::Bluetooth, false)]);
    }

    /// Read-only look at this machine: `cargo test snapshot_live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn snapshot_live() {
        unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
        let _ = Radio::RequestAccessAsync().and_then(|op| op.get());
        eprintln!("{:#?}", snapshot(&mut BTreeMap::new()));
    }

    #[test]
    fn kinds() {
        assert_eq!(device_kind(4, 6, "UGREEN HiTune Max5c"), Headphones);
        assert_eq!(device_kind(4, 5, "Living room"), Speaker);
        assert_eq!(device_kind(2, 3, "iPhone"), Phone);
        assert_eq!(device_kind(5, 2, "Pad"), Gamepad);
        assert_eq!(device_kind(5, 0, "MX Master 3S"), Mouse);
        assert_eq!(device_kind(0, 0, "UGREEN HiTune Max5c"), Headphones);
        assert_eq!(device_kind(0, 0, "Keychron K2 Keyboard"), Keyboard);
        assert_eq!(device_kind(0, 0, "Thiết bị lạ"), Other);
    }
}
