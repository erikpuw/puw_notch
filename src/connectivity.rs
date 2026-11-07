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
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_CONN: u32 = WM_APP + 6;
const POLL: Duration = Duration::from_secs(4);

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
    Online { name: String, wifi: bool },
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
    pub vpn: bool,
    pub devices: BTreeMap<String, (String, DeviceKind, Option<u8>)>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Event {
    Radio(RadioType, bool),
    Airplane(bool),
    Net(Net),
    Vpn(bool),
    Device { name: String, kind: DeviceKind, up: bool, battery: Option<u8> },
    DeviceBattery { name: String, kind: DeviceKind, percent: u8 },
}

pub const LOW_BATTERY: u8 = 20;

pub fn changes(old: &Snapshot, new: &Snapshot) -> Vec<Event> {
    let mut out = Vec::new();
    let airplane_flip = old.airplane != new.airplane;
    if airplane_flip {
        out.push(Event::Airplane(new.airplane));
    }
    let mut radio_off = BTreeSet::new();
    let kinds: BTreeSet<RadioType> = old.radios.keys().chain(new.radios.keys()).copied().collect();
    for kind in kinds {
        let (was, on) = (old.radios.get(&kind).copied().unwrap_or(false), new.radios.get(&kind).copied().unwrap_or(false));
        if was != on {
            out.push(Event::Radio(kind, on));
            if !on {
                radio_off.insert(kind);
            }
        }
    }
    let quiet_offline = new.airplane || !radio_off.is_empty();
    if old.net != new.net && !(new.net == Net::Offline && quiet_offline) {
        out.push(Event::Net(new.net.clone()));
    }
    let vpn_up = |s: &Snapshot| s.vpn && matches!(s.net, Net::Online { .. });
    if vpn_up(old) != vpn_up(new) {
        out.push(Event::Vpn(vpn_up(new)));
    }
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

pub fn device_kind(major: i32, minor: i32, name: &str) -> DeviceKind {
    use DeviceKind::*;
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    match major {
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

const LIMITED_AFTER: Duration = Duration::from_secs(8);

fn settle(net: Net, last: &Net, since: &mut Option<Instant>, now: Instant) -> Net {
    if !matches!(net, Net::Limited { .. }) {
        *since = None;
        return net;
    }
    let start = *since.get_or_insert(now);
    if now.saturating_duration_since(start) >= LIMITED_AFTER { net } else { last.clone() }
}

const KNOCK_ON: Duration = Duration::from_secs(4);

const DEVICE_SETTLE: Duration = Duration::from_secs(10);

fn drop_knock_on(events: &mut Vec<Event>, radio_off_recent: bool, bluetooth_settling: bool) {
    events.retain(|e| match e {
        Event::Net(Net::Offline) => !radio_off_recent,
        Event::Device { up: false, .. } => !bluetooth_settling,
        _ => true,
    });
}

static KNOWN: Mutex<Vec<(String, DeviceKind)>> = Mutex::new(Vec::new());

pub fn expected_devices() -> Vec<(String, DeviceKind)> {
    KNOWN.lock().map(|k| k.clone()).unwrap_or_default()
}

fn remember(known: &mut Vec<(String, DeviceKind)>, events: &[Event]) {
    for e in events {
        match e {
            Event::Device { name, kind, up: true, .. } if !known.iter().any(|(n, _)| n == name) => known.push((name.clone(), *kind)),
            Event::Device { name, up: false, .. } => known.retain(|(n, _)| n != name),
            _ => {}
        }
    }
}

static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());

pub fn take_events() -> Vec<Event> {
    EVENTS.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

static NUDGE: (Mutex<bool>, std::sync::Condvar) = (Mutex::new(false), std::sync::Condvar::new());
const QUICK: Duration = Duration::from_millis(500);
const QUICK_POLLS: u32 = 6;

static DEVICES_DIRTY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn wake() {
    if let Ok(mut n) = NUDGE.0.lock() {
        *n = true;
        NUDGE.1.notify_one();
    }
}

pub fn nudge() {
    DEVICES_DIRTY.store(true, std::sync::atomic::Ordering::Relaxed);
    wake();
}

fn wait(period: Duration) -> bool {
    let Ok(guard) = NUDGE.0.lock() else {
        std::thread::sleep(period);
        return false;
    };
    NUDGE.1.wait_timeout_while(guard, period, |n| !*n).map_or(false, |(mut n, _)| std::mem::take(&mut *n))
}

const COALESCE: Duration = Duration::from_millis(300);
const DEVICES_MIN: Duration = Duration::from_millis(1500);

fn coalesce() {
    std::thread::sleep(COALESCE);
    if let Ok(mut n) = NUDGE.0.lock() {
        *n = false;
    }
}

fn subscribe() -> (Vec<Radio>, Vec<DeviceWatcher>) {
    let _ = NetworkInformation::NetworkStatusChanged(&NetworkStatusChangedEventHandler::new(|_| {
        wake();
        Ok(())
    }));
    let radios: Vec<Radio> = Radio::GetRadiosAsync().and_then(|op| op.get()).map(|list| list.into_iter().collect()).unwrap_or_default();
    for r in &radios {
        let _ = r.StateChanged(&TypedEventHandler::<Radio, IInspectable>::new(|radio, _| {
            match radio.as_ref().and_then(|r| r.Kind().ok()) {
                Some(RadioKind::Bluetooth) => nudge(),
                _ => wake(),
            }
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
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        }
        let _ = Radio::RequestAccessAsync().and_then(|op| op.get());
        let _keep = subscribe();
        let mut kinds: BTreeMap<String, DeviceKind> = BTreeMap::new();
        let mut last = snapshot(&mut kinds);
        if let Ok(mut known) = KNOWN.lock() {
            for (name, kind, _) in last.devices.values() {
                if !known.iter().any(|(n, _)| n == name) {
                    known.push((name.clone(), *kind));
                }
            }
        }
        let mut quick = 0;
        let mut pump = Pump::default();
        let mut devices_at = Instant::now();
        loop {
            let nudged = wait(if quick > 0 { QUICK } else { POLL });
            if nudged {
                coalesce();
            }
            let fast_started = Instant::now();
            let switch = airplane();
            if switch != last.airplane {
                let early = Snapshot { airplane: switch, ..last.clone() };
                pump.step(&mut last, early, target);
            }
            quick = if nudged { QUICK_POLLS } else { quick.saturating_sub(1) };
            let read = radios();
            let mut now = Snapshot { radios: if read.is_empty() { last.radios.clone() } else { read }, airplane: switch, net: Net::Offline, vpn: false, devices: last.devices.clone() };
            let profiles = profiles();
            (now.net, now.vpn) = (net_of(&profiles), vpn_of(&profiles));
            let bt_off = now.airplane || now.radios.get(&RadioType::Bluetooth) == Some(&false);
            if bt_off {
                now.devices.clear();
            }
            if pump.step(&mut last, now, target) {
                quick = quick.max(2);
            }
            let fast_ms = fast_started.elapsed().as_millis();
            let due = devices_at.elapsed();
            let dirty = DEVICES_DIRTY.load(std::sync::atomic::Ordering::Relaxed);
            if !bt_off && (due >= DEVICES_EVERY || (due >= DEVICES_MIN && dirty)) {
                DEVICES_DIRTY.store(false, std::sync::atomic::Ordering::Relaxed);
                devices_at = Instant::now();
                let mut now = last.clone();
                now.devices = devices(&mut kinds);
                let devices_ms = devices_at.elapsed().as_millis();
                if devices_ms >= 40 || fast_ms >= 40 {
                    crate::system::log(&format!("watcher: radios, airplane and network {fast_ms} ms, {} devices read in {devices_ms} ms", now.devices.len()));
                }
                pump.step(&mut last, now, target);
            } else if fast_ms >= 40 {
                crate::system::log(&format!("watcher: radios, airplane and network {fast_ms} ms"));
            }
        }
    });
}

const DEVICES_EVERY: Duration = Duration::from_secs(30);

#[derive(Default)]
struct Pump {
    radio_off_at: Option<Instant>,
    bt_on_at: Option<Instant>,
    limited_since: Option<Instant>,
}

impl Pump {
    fn step(&mut self, last: &mut Snapshot, mut now: Snapshot, target: isize) -> bool {
        let settled = settle(now.net.clone(), &last.net, &mut self.limited_since, Instant::now());
        let waiting = settled != now.net;
        now.net = settled;
        let mut events = changes(last, &now);
        *last = now;
        if events.iter().any(|e| matches!(e, Event::Radio(RadioType::Bluetooth, true) | Event::Airplane(false))) {
            self.bt_on_at = Some(Instant::now());
        }
        drop_knock_on(&mut events, self.radio_off_at.is_some_and(|t| t.elapsed() < KNOCK_ON), self.bt_on_at.is_some_and(|t| t.elapsed() < DEVICE_SETTLE));
        if events.iter().any(|e| matches!(e, Event::Radio(_, false))) {
            self.radio_off_at = Some(Instant::now());
        }
        if let Ok(mut known) = KNOWN.lock() {
            remember(&mut known, &events);
        }
        if !events.is_empty() {
            crate::system::log(&format!("connectivity: {events:?}"));
            if let Ok(mut q) = EVENTS.lock() {
                q.extend(events);
            }
            unsafe { let _ = PostMessageW(Some(HWND(target as _)), WM_CONN, WPARAM(0), LPARAM(0)); }
        }
        waiting
    }
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

struct Profile {
    name: String,
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
            let battery = cached_battery(&id, || container_of(&d).and_then(battery_of));
            out.insert(id, (name, kind, battery));
        }
    }
    out
}

static BATTERIES: Mutex<BTreeMap<String, (Instant, Option<u8>)>> = Mutex::new(BTreeMap::new());
const BATTERY_KEPT: Duration = Duration::from_secs(10);
const BATTERY_MISSING_KEPT: Duration = Duration::from_millis(1500);

fn cached_battery(id: &str, read: impl FnOnce() -> Option<u8>) -> Option<u8> {
    if let Some(&(at, value)) = BATTERIES.lock().ok().and_then(|m| m.get(id).copied()).as_ref() {
        if at.elapsed() < if value.is_some() { BATTERY_KEPT } else { BATTERY_MISSING_KEPT } {
            return value;
        }
    }
    let value = read();
    if let Ok(mut m) = BATTERIES.lock() {
        m.insert(id.to_string(), (Instant::now(), value));
    }
    value
}

const CONTAINER_KEY: &str = "System.Devices.Aep.ContainerId";

fn container_of(d: &DeviceInformation) -> Option<GUID> {
    d.Properties().ok()?.Lookup(&HSTRING::from(CONTAINER_KEY)).ok()?.cast::<IReference<GUID>>().ok()?.Value().ok()
}

fn battery_of(container: GUID) -> Option<u8> {
    let aqs = HSTRING::from(format!("System.Devices.ContainerId:=\"{{{container:?}}}\""));
    let props = IIterable::<HSTRING>::from(vec![HSTRING::from(BATTERY_KEY)]);
    let found = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&aqs, &props, DeviceInformationKind::Device).and_then(|op| op.get()).ok()?;
    found.into_iter().find_map(|d| d.Properties().ok()?.Lookup(&HSTRING::from(BATTERY_KEY)).ok()?.cast::<IReference<u8>>().ok()?.Value().ok())
}

const BATTERY_KEY: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";

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
        let online = with_vpn(snap(true, true, false, home(), &[]));
        assert_eq!(changes(&with_vpn(radios), &online), vec![Event::Net(home()), Event::Vpn(true)]);
    }

    #[test]
    fn a_vpn_goes_down_with_its_network_and_comes_up_after_it() {
        let a = with_vpn(snap(true, true, false, home(), &[]));
        assert_eq!(changes(&a, &with_vpn(snap(false, true, false, Net::Offline, &[]))), vec![Event::Radio(RadioType::Wifi, false), Event::Vpn(false)]);
        assert_eq!(changes(&a, &snap(true, true, false, home(), &[])), vec![Event::Vpn(false)]);
        let lonely = with_vpn(snap(true, true, false, Net::Offline, &[]));
        assert!(changes(&snap(true, true, false, Net::Offline, &[]), &lonely).is_empty());
    }

    #[test]
    fn offline_after_a_radio_went_off_is_not_a_second_story() {
        let mut e = changes(&snap(false, true, false, home(), &[]), &snap(false, true, false, Net::Offline, &[]));
        assert_eq!(e, vec![Event::Net(Net::Offline)]);
        drop_knock_on(&mut e, true, false);
        assert!(e.is_empty());
        let mut limited = vec![Event::Net(Net::Limited { name: "Cafe".into(), wifi: true })];
        drop_knock_on(&mut limited, true, false);
        assert_eq!(limited.len(), 1, "a network with no internet is still news");
    }

    #[test]
    fn a_device_that_drops_while_bluetooth_settles_is_not_told() {
        let down = Event::Device { name: "UGREEN".into(), kind: Headphones, up: false, battery: None };
        let up = Event::Device { name: "UGREEN".into(), kind: Headphones, up: true, battery: Some(80) };
        let mut e = vec![down.clone(), up.clone()];
        drop_knock_on(&mut e, false, true);
        assert_eq!(e, vec![up], "the drop is left out, the connection is still told");
        let mut e = vec![down.clone()];
        drop_knock_on(&mut e, false, false);
        assert_eq!(e, vec![down]);
    }

    #[test]
    fn a_wifi_adapter_that_vanishes_and_returns_is_told_off_then_on() {
        let on = snap(true, true, false, home(), &[]);
        let gone = Snapshot { radios: [(RadioType::Bluetooth, true)].into_iter().collect(), net: Net::Offline, ..on.clone() };
        assert_eq!(changes(&on, &gone), vec![Event::Radio(RadioType::Wifi, false)]);
        let back = Snapshot { net: home(), ..on.clone() };
        assert_eq!(changes(&gone, &back), vec![Event::Radio(RadioType::Wifi, true), Event::Net(home())]);
    }

    #[test]
    fn a_radio_is_told_before_the_devices_are_read() {
        let mut last = snap(true, true, false, home(), &[("h", "UGREEN", Headphones)]);
        let mut pump = Pump::default();
        EVENTS.lock().unwrap().clear();
        let first = Snapshot { radios: [(RadioType::Wifi, false), (RadioType::Bluetooth, true)].into_iter().collect(), net: Net::Offline, ..last.clone() };
        pump.step(&mut last, first, 0);
        assert_eq!(*EVENTS.lock().unwrap(), vec![Event::Radio(RadioType::Wifi, false)]);
        EVENTS.lock().unwrap().clear();
        let second = Snapshot { devices: Default::default(), ..last.clone() };
        pump.step(&mut last, second, 0);
        assert_eq!(take_events(), vec![Event::Device { name: "UGREEN".into(), kind: Headphones, up: false, battery: None }]);
    }

    #[test]
    fn devices_are_remembered_until_they_go_by_themselves() {
        let mut known = Vec::new();
        let up = |n: &str| Event::Device { name: n.into(), kind: Headphones, up: true, battery: None };
        let down = |n: &str| Event::Device { name: n.into(), kind: Headphones, up: false, battery: None };
        remember(&mut known, &[up("WH-1000XM5"), up("MX Master 3"), up("WH-1000XM5")]);
        assert_eq!(known.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["WH-1000XM5", "MX Master 3"]);
        let a = with_vpn(snap(true, true, false, home(), &[("h", "WH-1000XM5", Headphones)]));
        assert!(changes(&a, &snap(false, false, true, Net::Offline, &[])).iter().all(|e| !matches!(e, Event::Device { .. })));
        remember(&mut known, &[down("MX Master 3")]);
        assert_eq!(known.len(), 1);
    }

    #[test]
    fn a_vpn_waits_for_a_network_with_internet() {
        let limited = Net::Limited { name: "JihoonKim".into(), wifi: true };
        let vpn_first = with_vpn(snap(true, true, false, limited, &[]));
        assert!(changes(&snap(true, true, false, Net::Offline, &[]), &vpn_first).iter().all(|e| !matches!(e, Event::Vpn(_))));
        let online = with_vpn(snap(true, true, false, home(), &[]));
        assert_eq!(changes(&vpn_first, &online), vec![Event::Net(home()), Event::Vpn(true)]);
    }

    #[test]
    fn a_network_that_has_no_internet_for_a_moment_is_not_told() {
        let t0 = Instant::now();
        let limited = || Net::Limited { name: "JihoonKim".into(), wifi: true };
        let mut since = None;
        assert_eq!(settle(limited(), &Net::Offline, &mut since, t0), Net::Offline);
        assert_eq!(settle(limited(), &Net::Offline, &mut since, t0 + Duration::from_secs(7)), Net::Offline);
        assert_eq!(settle(home(), &Net::Offline, &mut since, t0 + Duration::from_secs(7)), home());
        assert!(since.is_none());
        let mut since = None;
        settle(limited(), &Net::Offline, &mut since, t0);
        assert_eq!(settle(limited(), &Net::Offline, &mut since, t0 + LIMITED_AFTER), limited());
    }

    #[test]
    fn the_network_is_wifi_or_ethernet_never_the_vpn() {
        let p = |name: &str, iftype, wlan, level| Profile { name: name.into(), iftype, wlan, level };
        use NetworkConnectivityLevel as L;
        let now = [p("Ethernet", 6, false, L::None), p("Tailscale", 53, false, L::LocalAccess), p("JihoonKim", 71, true, L::InternetAccess)];
        assert_eq!(net_of(&now), Net::Online { name: "JihoonKim".into(), wifi: true });
        assert!(vpn_of(&now));
        let off = [p("Ethernet", 6, false, L::None), p("Tailscale", 53, false, L::LocalAccess), p("JihoonKim", 71, true, L::None)];
        assert_eq!(net_of(&off), Net::Offline);
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
        let same = snap(true, true, false, home(), &[("m", "MX Master", Mouse), ("hc", "WH-1000", Headphones), ("hl", "WH-1000", Headphones)]);
        assert!(changes(&a, &same).is_empty());
        let b = snap(true, true, false, home(), &[("hl", "WH-1000", Headphones), ("j", "Loa JBL", Speaker)]);
        assert_eq!(
            changes(&a, &b),
            vec![Event::Device { name: "Loa JBL".into(), kind: Speaker, up: true, battery: None }, Event::Device { name: "MX Master".into(), kind: Mouse, up: false, battery: None }]
        );
        let off = snap(true, false, false, home(), &[]);
        assert_eq!(changes(&a, &off), vec![Event::Radio(RadioType::Bluetooth, false)]);
    }

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
        assert_eq!(changes(&none, &at(Some(80))), vec![Event::Device { name: "WH-1000".into(), kind: Headphones, up: true, battery: Some(80) }]);
        assert_eq!(changes(&at(None), &at(Some(80))), vec![ev(80)]);
        assert!(changes(&at(Some(80)), &at(Some(79))).is_empty(), "small drops are quiet");
        assert_eq!(changes(&at(Some(21)), &at(Some(20))), vec![ev(20)], "low");
        assert!(changes(&at(Some(20)), &at(Some(19))).is_empty(), "low is said once");
    }

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
