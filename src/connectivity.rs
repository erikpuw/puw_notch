//! Connectivity watcher: Wi-Fi / Bluetooth / mobile radios on-off, airplane mode, the network in use
//! (and whether it reaches the internet) and Bluetooth devices connecting. WinRT calls block, so a
//! worker takes a snapshot, diffs it against the last and queues events for the notch window. It looks
//! when Windows says something changed (`subscribe`), and every 30 s for what Windows does not say
//! (a device's battery).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::core::{w, IInspectable, Interface, GUID, HSTRING};
use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind, DeviceInformationUpdate, DeviceWatcher};
use windows_collections::IIterable;
use windows::Foundation::{IReference, TypedEventHandler};
use windows::Devices::Radios::{Radio, RadioKind, RadioState};
use windows::Networking::Connectivity::{NetworkConnectivityLevel, NetworkInformation, NetworkStatusChangedEventHandler};
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Posted when events are queued; read them with `take_events`.
pub const WM_CONN: u32 = WM_APP + 6;
const POLL: Duration = Duration::from_secs(30);

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
    /// A VPN's own network (Tailscale and the like) is up. Only counts with a network under it, see `changes`.
    pub vpn: bool,
    /// Connected Bluetooth devices: id -> (name, kind, battery percent if the device reports one).
    pub devices: BTreeMap<String, (String, DeviceKind, Option<u8>)>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Event {
    Radio(RadioType, bool),
    Airplane(bool),
    Net(Net),
    /// A VPN came up (a network is under it) or went down (the network went, or the VPN did).
    Vpn(bool),
    /// Connected (with its battery when known) or disconnected.
    Device { name: String, kind: DeviceKind, up: bool, battery: Option<u8> },
    /// A connected device's battery: reported after it connected, or just dropped to `LOW_BATTERY`.
    DeviceBattery { name: String, kind: DeviceKind, percent: u8 },
}

/// A Bluetooth device at or under this percent gets a low-battery banner.
pub const LOW_BATTERY: u8 = 20;

/// What changed between two snapshots, in the order it should be told: the airplane switch, the
/// radios, the network, the VPN, then devices. A banner groups what comes together (see main.rs);
/// here only the knock-on noise is left out: "offline" and "headphones disconnected" after a radio
/// or the airplane switch went off.
pub fn changes(old: &Snapshot, new: &Snapshot) -> Vec<Event> {
    let mut out = Vec::new();
    let airplane_flip = old.airplane != new.airplane;
    if airplane_flip {
        out.push(Event::Airplane(new.airplane));
    }
    let mut radio_off = BTreeSet::new();
    for (kind, on) in &new.radios {
        if old.radios.get(kind).is_some_and(|was| was != on) {
            out.push(Event::Radio(*kind, *on));
            if !on {
                radio_off.insert(*kind);
            }
        }
    }
    let quiet_offline = new.airplane || !radio_off.is_empty();
    if old.net != new.net && !(new.net == Net::Offline && quiet_offline) {
        out.push(Event::Net(new.net.clone()));
    }
    // A VPN lives on a network: it is "up" only with one under it, so it is told after the network
    // that carries it came back, and told down when that network goes (it is not told alone then).
    let vpn_up = |s: &Snapshot| s.vpn && s.net != Net::Offline;
    if vpn_up(old) != vpn_up(new) {
        out.push(Event::Vpn(vpn_up(new)));
    }
    // Devices, by name (a headset can show up over classic and LE at once, one of them with the battery).
    let names = |s: &Snapshot| {
        let mut m: BTreeMap<String, (DeviceKind, Option<u8>)> = BTreeMap::new();
        for (n, k, b) in s.devices.values() {
            let e = m.entry(n.clone()).or_insert((*k, None));
            e.1 = e.1.max(*b);
        }
        m
    };
    let (before, after) = (names(old), names(new));
    let quiet_disconnect = new.airplane || radio_off.contains(&RadioType::Bluetooth);
    for (name, &(kind, battery)) in &after {
        match before.get(name) {
            None => out.push(Event::Device { name: name.clone(), kind, up: true, battery }),
            Some(&(_, was)) => {
                let late = was.is_none() && battery.is_some();
                let dropped_low = was.is_some_and(|w| w > LOW_BATTERY) && battery.is_some_and(|b| b <= LOW_BATTERY);
                if let (true, Some(percent)) = (late || dropped_low, battery) {
                    out.push(Event::DeviceBattery { name: name.clone(), kind, percent });
                }
            }
        }
    }
    if !quiet_disconnect {
        for (name, &(kind, _)) in &before {
            if !after.contains_key(name) {
                out.push(Event::Device { name: name.clone(), kind, up: false, battery: None });
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

/// How long "offline" after a radio went off counts as part of it: the network goes a poll later.
const KNOCK_ON: Duration = Duration::from_secs(4);

/// Drops "offline" right after a radio went off (the radio's own event says it).
fn drop_knock_on(events: &mut Vec<Event>, radio_off_recent: bool) {
    events.retain(|e| !(radio_off_recent && *e == Event::Net(Net::Offline)));
}

static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());

pub fn take_events() -> Vec<Event> {
    EVENTS.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

/// Start the watcher. What is already on / connected at start is not announced.
/// Wakes the watcher before its next poll: the default speaker changed, which is what a headset
/// connecting or leaving does first. The watcher then looks again for a few seconds, quickly.
static NUDGE: (Mutex<bool>, std::sync::Condvar) = (Mutex::new(false), std::sync::Condvar::new());
const QUICK: Duration = Duration::from_millis(500);
const QUICK_POLLS: u32 = 6;

pub fn nudge() {
    if let Ok(mut n) = NUDGE.0.lock() {
        *n = true;
        NUDGE.1.notify_one();
    }
}

/// Sleep until the next poll, or until nudged; true when nudged.
fn wait(period: Duration) -> bool {
    let Ok(guard) = NUDGE.0.lock() else {
        std::thread::sleep(period);
        return false;
    };
    NUDGE.1.wait_timeout_while(guard, period, |n| !*n).map_or(false, |(mut n, _)| std::mem::take(&mut *n))
}

/// Windows says when the network changes, a radio is switched, or a Bluetooth device connects or goes:
/// each wakes the watcher (`nudge`). The returned radios and watchers must stay alive.
fn subscribe() -> (Vec<Radio>, Vec<DeviceWatcher>) {
    let _ = NetworkInformation::NetworkStatusChanged(&NetworkStatusChangedEventHandler::new(|_| {
        nudge();
        Ok(())
    }));
    let radios: Vec<Radio> = Radio::GetRadiosAsync().and_then(|op| op.get()).map(|list| list.into_iter().collect()).unwrap_or_default();
    for r in &radios {
        let _ = r.StateChanged(&TypedEventHandler::<Radio, IInspectable>::new(|_, _| {
            nudge();
            Ok(())
        }));
    }
    let mut watchers = Vec::new();
    let selectors = [
        BluetoothDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected),
        BluetoothLEDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected),
    ];
    for sel in selectors.into_iter().flatten() {
        let Ok(w) = DeviceInformation::CreateWatcherAqsFilter(&sel) else { continue };
        let _ = w.Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(|_, _| {
            nudge();
            Ok(())
        }));
        for_update(&w);
        let _ = w.Start();
        watchers.push(w);
    }
    (radios, watchers)
}

/// Updated and Removed carry a `DeviceInformationUpdate`.
fn for_update(w: &DeviceWatcher) {
    let _ = w.Updated(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(|_, _| {
        nudge();
        Ok(())
    }));
    let _ = w.Removed(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(|_, _| {
        nudge();
        Ok(())
    }));
}

pub fn watch(hwnd: HWND) {
    let target = hwnd.0 as isize;
    crate::system::spawn("connectivity", move || {
        unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
        let _ = Radio::RequestAccessAsync().and_then(|op| op.get());
        let _keep = subscribe();
        // Class lookups are a round trip each; remember them per device id.
        let mut kinds: BTreeMap<String, DeviceKind> = BTreeMap::new();
        let mut last = snapshot(&mut kinds);
        let mut quick = 0;
        let mut radio_off_at: Option<Instant> = None;
        loop {
            if wait(if quick > 0 { QUICK } else { POLL }) {
                quick = QUICK_POLLS;
            } else {
                quick = quick.saturating_sub(1);
            }
            let now = snapshot(&mut kinds);
            let mut events = changes(&last, &now);
            last = now;
            drop_knock_on(&mut events, radio_off_at.is_some_and(|t| t.elapsed() < KNOCK_ON));
            if events.iter().any(|e| matches!(e, Event::Radio(_, false))) {
                radio_off_at = Some(Instant::now());
            }
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
    let profiles = profiles();
    Snapshot { radios: radios(), airplane: airplane(), net: net_of(&profiles), vpn: vpn_of(&profiles), devices: devices(kinds) }
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

/// A connection profile, as far as the network and the VPN are told from it.
struct Profile {
    name: String,
    /// IANA interface type: 6 Ethernet, 71 Wi-Fi, 243/244 mobile broadband; a VPN's is something else (Tailscale: 53).
    iftype: u32,
    wlan: bool,
    level: NetworkConnectivityLevel,
}

impl Profile {
    fn physical(&self) -> bool {
        matches!(self.iftype, 6 | 71 | 243 | 244)
    }
}

fn profiles() -> Vec<Profile> {
    let Ok(list) = NetworkInformation::GetConnectionProfiles() else { return Vec::new() };
    list.into_iter().map(|p| {
        let wlan = p.IsWlanConnectionProfile().unwrap_or(false);
        let ssid = if wlan { p.WlanConnectionProfileDetails().and_then(|d| d.GetConnectedSsid()).ok() } else { None };
        Profile {
            name: ssid.or_else(|| p.ProfileName().ok()).map(|n| n.to_string()).unwrap_or_default(),
            iftype: p.NetworkAdapter().and_then(|a| a.IanaInterfaceType()).unwrap_or(0),
            wlan,
            level: p.GetNetworkConnectivityLevel().unwrap_or(NetworkConnectivityLevel::None),
        }
    }).collect()
}

/// The network the PC is on: the first Wi-Fi / Ethernet / mobile one that reaches the internet, else
/// a Wi-Fi one without internet. A VPN's own network (it has "local access" only) never counts: with
/// Wi-Fi off it used to be what was left, and the banner said "no internet, Tailscale".
fn net_of(profiles: &[Profile]) -> Net {
    if let Some(p) = profiles.iter().find(|p| p.physical() && p.level == NetworkConnectivityLevel::InternetAccess) {
        return Net::Online { name: p.name.clone(), wifi: p.wlan };
    }
    match profiles.iter().find(|p| p.physical() && p.wlan && p.level != NetworkConnectivityLevel::None) {
        Some(p) => Net::Limited { name: p.name.clone(), wifi: true },
        None => Net::Offline,
    }
}

fn vpn_of(profiles: &[Profile]) -> bool {
    profiles.iter().any(|p| !p.physical() && p.level != NetworkConnectivityLevel::None)
}

fn devices(kinds: &mut BTreeMap<String, DeviceKind>) -> BTreeMap<String, (String, DeviceKind, Option<u8>)> {
    let mut out = BTreeMap::new();
    let selectors = [
        BluetoothDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected),
        BluetoothLEDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected),
    ];
    let props = IIterable::<HSTRING>::from(vec![HSTRING::from(CONTAINER_KEY)]);
    for sel in selectors.into_iter().flatten() {
        let Ok(found) = DeviceInformation::FindAllAsyncAqsFilterAndAdditionalProperties(&sel, &props).and_then(|op| op.get()) else { continue };
        for d in found {
            let (Ok(id), Ok(name)) = (d.Id(), d.Name()) else { continue };
            let (id, name) = (id.to_string(), name.to_string());
            let kind = *kinds.entry(id.clone()).or_insert_with(|| {
                let (major, minor) = class_of(&id);
                device_kind(major, minor, &name)
            });
            let battery = container_of(&d).and_then(battery_of);
            out.insert(id, (name, kind, battery));
        }
    }
    out
}

const CONTAINER_KEY: &str = "System.Devices.Aep.ContainerId";

fn container_of(d: &DeviceInformation) -> Option<GUID> {
    d.Properties().ok()?.Lookup(&HSTRING::from(CONTAINER_KEY)).ok()?.cast::<IReference<GUID>>().ok()?.Value().ok()
}

/// Battery percent Windows knows for the Bluetooth device in this container (what Settings shows:
/// hands-free or LE battery reports). None when the device reports none.
fn battery_of(container: GUID) -> Option<u8> {
    let aqs = HSTRING::from(format!("System.Devices.ContainerId:=\"{{{container:?}}}\""));
    let props = IIterable::<HSTRING>::from(vec![HSTRING::from(BATTERY_KEY)]);
    let found = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&aqs, &props, DeviceInformationKind::Device).and_then(|op| op.get()).ok()?;
    found.into_iter().find_map(|d| d.Properties().ok()?.Lookup(&HSTRING::from(BATTERY_KEY)).ok()?.cast::<IReference<u8>>().ok()?.Value().ok())
}

/// DEVPKEY_Bluetooth_Battery: the battery percent as a byte.
const BATTERY_KEY: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";

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
        snap_b(wifi, bt, airplane, net, &devices.iter().map(|&(i, n, k)| (i, n, k, None)).collect::<Vec<_>>())
    }

    fn snap_b(wifi: bool, bt: bool, airplane: bool, net: Net, devices: &[(&str, &str, DeviceKind, Option<u8>)]) -> Snapshot {
        Snapshot {
            radios: [(RadioType::Wifi, wifi), (RadioType::Bluetooth, bt)].into_iter().collect(),
            airplane,
            net,
            vpn: false,
            devices: devices.iter().map(|(i, n, k, b)| (i.to_string(), (n.to_string(), *k, *b))).collect(),
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

    fn with_vpn(mut s: Snapshot) -> Snapshot {
        s.vpn = true;
        s
    }

    #[test]
    fn airplane_on_lists_the_radios_and_the_vpn_but_not_the_offline_or_the_headset() {
        let a = with_vpn(snap(true, true, false, home(), &[("h", "Tai nghe", Headphones)]));
        let b = snap(false, false, true, Net::Offline, &[]);
        assert_eq!(
            changes(&a, &with_vpn(b)),
            vec![Event::Airplane(true), Event::Radio(RadioType::Wifi, false), Event::Radio(RadioType::Bluetooth, false), Event::Vpn(false)]
        );
    }

    #[test]
    fn airplane_off_lists_the_radios_then_the_network_then_the_vpn() {
        let on = with_vpn(snap(false, false, true, Net::Offline, &[]));
        let radios = snap(true, true, false, Net::Offline, &[]);
        assert_eq!(changes(&on, &with_vpn(radios.clone())), vec![Event::Airplane(false), Event::Radio(RadioType::Wifi, true), Event::Radio(RadioType::Bluetooth, true)]);
        // the VPN is not up while there is no network under it; it comes with the network
        let online = with_vpn(snap(true, true, false, home(), &[]));
        assert_eq!(changes(&with_vpn(radios), &online), vec![Event::Net(home()), Event::Vpn(true)]);
    }

    #[test]
    fn a_vpn_goes_down_with_its_network_and_comes_up_after_it() {
        let a = with_vpn(snap(true, true, false, home(), &[]));
        // Wi-Fi off: the radio, and the VPN that was on it; "offline" is the radio's doing
        assert_eq!(changes(&a, &with_vpn(snap(false, true, false, Net::Offline, &[]))), vec![Event::Radio(RadioType::Wifi, false), Event::Vpn(false)]);
        // the VPN switched off by itself, the network stays
        assert_eq!(changes(&a, &snap(true, true, false, home(), &[])), vec![Event::Vpn(false)]);
        // a VPN adapter alone, with no network, is not "connected"
        let lonely = with_vpn(snap(true, true, false, Net::Offline, &[]));
        assert!(changes(&snap(true, true, false, Net::Offline, &[]), &lonely).is_empty());
    }

    #[test]
    fn offline_after_a_radio_went_off_is_not_a_second_story() {
        // Wi-Fi off and the network gone are one poll apart.
        let mut e = changes(&snap(false, true, false, home(), &[]), &snap(false, true, false, Net::Offline, &[]));
        assert_eq!(e, vec![Event::Net(Net::Offline)]);
        drop_knock_on(&mut e, true);
        assert!(e.is_empty());
        let mut limited = vec![Event::Net(Net::Limited { name: "Cafe".into(), wifi: true })];
        drop_knock_on(&mut limited, true);
        assert_eq!(limited.len(), 1, "a network with no internet is still news");
    }

    #[test]
    fn the_network_is_wifi_or_ethernet_never_the_vpn() {
        let p = |name: &str, iftype, wlan, level| Profile { name: name.into(), iftype, wlan, level };
        use NetworkConnectivityLevel as L;
        // what this PC reports (Wi-Fi up, Tailscale up, the rest down)
        let now = [p("Ethernet", 6, false, L::None), p("Tailscale", 53, false, L::LocalAccess), p("JihoonKim", 71, true, L::InternetAccess)];
        assert_eq!(net_of(&now), Net::Online { name: "JihoonKim".into(), wifi: true });
        assert!(vpn_of(&now));
        // Wi-Fi off: only the VPN's own network is left, which is neither a network nor "limited"
        let off = [p("Ethernet", 6, false, L::None), p("Tailscale", 53, false, L::LocalAccess), p("JihoonKim", 71, true, L::None)];
        assert_eq!(net_of(&off), Net::Offline);
        // a Wi-Fi without internet is limited; a VPN down is not there
        let cafe = [p("Cafe", 71, true, L::LocalAccess), p("Tailscale", 53, false, L::None)];
        assert_eq!(net_of(&cafe), Net::Limited { name: "Cafe".into(), wifi: true });
        assert!(!vpn_of(&cafe));
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
            vec![Event::Device { name: "Loa JBL".into(), kind: Speaker, up: true, battery: None }, Event::Device { name: "MX Master".into(), kind: Mouse, up: false, battery: None }]
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
    fn device_battery() {
        let none = snap_b(true, true, false, home(), &[]);
        let at = |b: Option<u8>| snap_b(true, true, false, home(), &[("hc", "WH-1000", Headphones, None), ("hl", "WH-1000", Headphones, b)]);
        let ev = |percent| Event::DeviceBattery { name: "WH-1000".into(), kind: Headphones, percent };
        // connecting: the battery rides on the connect banner (from whichever id reports it)
        assert_eq!(changes(&none, &at(Some(80))), vec![Event::Device { name: "WH-1000".into(), kind: Headphones, up: true, battery: Some(80) }]);
        // reported a poll after connecting: its own banner, once
        assert_eq!(changes(&at(None), &at(Some(80))), vec![ev(80)]);
        assert!(changes(&at(Some(80)), &at(Some(79))).is_empty(), "small drops are quiet");
        assert_eq!(changes(&at(Some(21)), &at(Some(20))), vec![ev(20)], "low");
        assert!(changes(&at(Some(20)), &at(Some(19))).is_empty(), "low is said once");
    }

    /// Battery of each connected device: `cargo test battery_live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn battery_live() {
        unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
        let sel = BluetoothDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected).unwrap();
        let props = IIterable::<HSTRING>::from(vec![HSTRING::from(CONTAINER_KEY)]);
        for d in DeviceInformation::FindAllAsyncAqsFilterAndAdditionalProperties(&sel, &props).unwrap().get().unwrap() {
            let container = container_of(&d);
            eprintln!("{} container {container:?} battery {:?}", d.Name().unwrap(), container.and_then(battery_of));
        }
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
