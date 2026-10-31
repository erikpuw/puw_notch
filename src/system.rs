//! Small OS bits: clock text, battery, settings file, start-with-Windows.

use std::path::PathBuf;
use crate::weather::Place;
use windows::core::{s, w, GUID, PCWSTR};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::Registry::*;
use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};

const WEEKDAYS: [&str; 7] = ["CN", "T2", "T3", "T4", "T5", "T6", "T7"];
const WEEKDAYS_EN: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Local time as yyyymmddHHMM (to-do reminders).
pub fn now_stamp() -> u64 {
    let t = unsafe { GetLocalTime() };
    crate::todo::stamp(t.wYear as u32 * 10_000 + t.wMonth as u32 * 100 + t.wDay as u32, t.wHour as u32, t.wMinute as u32)
}

/// Local time minus UTC in minutes (notification arrival times are stored in UTC).
pub fn utc_offset_min() -> i64 {
    let (local, utc) = unsafe { (GetLocalTime(), GetSystemTime()) };
    let total = |t: SYSTEMTIME| crate::todo::minutes(crate::todo::stamp(t.wYear as u32 * 10_000 + t.wMonth as u32 * 100 + t.wDay as u32, t.wHour as u32, t.wMinute as u32));
    total(local) - total(utc)
}

/// Local date as yyyymmdd (to-do "done today").
pub fn today() -> u32 {
    let t = unsafe { GetLocalTime() };
    t.wYear as u32 * 10_000 + t.wMonth as u32 * 100 + t.wDay as u32
}

/// ("16:04", "T7, 24/10" / "Sat, 24/10"): time for the pill, short date for the panel header.
pub fn clock() -> (String, String) {
    let t = unsafe { GetLocalTime() };
    let days = if crate::i18n::english() { WEEKDAYS_EN } else { WEEKDAYS };
    let day = days[t.wDayOfWeek as usize % 7];
    (format!("{:02}:{:02}", t.wHour, t.wMinute), format!("{day}, {:02}/{:02}", t.wDay, t.wMonth))
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Battery {
    pub percent: u8,
    pub charging: bool,
    /// Windows battery saver / energy saver is on.
    pub saver: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PowerEvent {
    Plugged,
    Unplugged,
    SaverOn,
    SaverOff,
    Low,
    Critical,
    Full,
}

const LOW: u8 = 20;
const CRITICAL: u8 = 10;

/// Battery changes worth a banner between two readings.
pub fn power_events(old: Battery, new: Battery) -> Vec<PowerEvent> {
    let mut out = Vec::new();
    if old.charging != new.charging {
        out.push(if new.charging { PowerEvent::Plugged } else { PowerEvent::Unplugged });
    }
    if old.saver != new.saver {
        out.push(if new.saver { PowerEvent::SaverOn } else { PowerEvent::SaverOff });
    }
    if !new.charging {
        if old.percent > CRITICAL && new.percent <= CRITICAL {
            out.push(PowerEvent::Critical);
        } else if old.percent > LOW && new.percent <= LOW {
            out.push(PowerEvent::Low);
        }
    }
    if new.charging && new.percent == 100 && old.percent < 100 {
        out.push(PowerEvent::Full);
    }
    out
}

/// `PowerGetEffectiveOverlayScheme` (the power mode now in effect; 0 = success). It is in neither
/// the `windows` crate nor every SDK's import library, so it is looked up in powrprof.dll once.
fn effective_overlay() -> Option<GUID> {
    type Get = unsafe extern "system" fn(*mut GUID) -> i32;
    static GET: std::sync::OnceLock<Option<Get>> = std::sync::OnceLock::new();
    let get = (*GET.get_or_init(|| unsafe {
        let lib = LoadLibraryW(w!("powrprof.dll")).ok()?;
        GetProcAddress(lib, s!("PowerGetEffectiveOverlayScheme")).map(|p| std::mem::transmute::<_, Get>(p))
    }))?;
    let mut guid = GUID::zeroed();
    (unsafe { get(&mut guid) } == 0).then_some(guid)
}

/// The "Best power efficiency" power mode, which Windows 11's Energy saver switches to.
const BEST_EFFICIENCY: GUID = GUID::from_u128(0x961cc777_2547_4f9d_8174_7d86181b8a7a);

/// Battery saver is on: the classic status flag, or (Windows 11 energy saver, which leaves that
/// flag at 0, measured) the effective power mode being "Best power efficiency".
fn saver_on(status_flag: u8, overlay: Option<GUID>) -> bool {
    status_flag == 1 || overlay == Some(BEST_EFFICIENCY)
}

/// None on a desktop PC (no system battery): nothing battery-related is shown then.
pub fn battery() -> Option<Battery> {
    let mut s = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut s) }.is_err() || s.BatteryLifePercent == 255 || s.BatteryFlag & 128 != 0 {
        return None;
    }
    Some(Battery { percent: s.BatteryLifePercent.min(100), charging: s.ACLineStatus == 1, saver: saver_on(s.SystemStatusFlag, effective_overlay()) })
}

fn settings_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("PuwNotch").join("settings.txt"))
}

/// Largest notch.log may grow before it starts over.
const LOG_MAX: u64 = 64 * 1024;

fn log_path() -> Option<PathBuf> {
    settings_path().map(|p| p.with_file_name("notch.log"))
}

/// First line of notch.log: the version that wrote it.
fn log_header() -> String {
    format!("PUW Notch v{}", env!("CARGO_PKG_VERSION"))
}

/// A log written by another version is of no use for this one: start it over.
pub fn log_open() {
    let Some(p) = log_path() else { return };
    let first = std::fs::read_to_string(&p).ok().and_then(|s| s.lines().next().map(str::to_string));
    if first.as_deref() != Some(&log_header()) {
        let _ = std::fs::remove_file(&p);
    }
}

/// One line in %APPDATA%\PuwNotch\notch.log: starts, exits and failures, so a start that dies
/// before anything shows (a start with Windows) can be traced. Best effort, never fails.
pub fn log(line: &str) {
    use std::io::Write;
    let Some(p) = log_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let fresh = std::fs::metadata(&p).map_or(true, |m| m.len() > LOG_MAX);
    let file = std::fs::OpenOptions::new().create(true).write(true).append(!fresh).truncate(fresh).open(&p);
    let t = unsafe { GetLocalTime() };
    if let Ok(mut f) = file {
        if fresh {
            let _ = writeln!(f, "{}", log_header());
        }
        let _ = writeln!(f, "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {line}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
    }
}

/// The PC's region setting counts in US units (Control Panel > Region > measurement system, the
/// registry's iMeasure = 1): the US and a few more, and where Fahrenheit is used.
pub fn us_measurement() -> bool {
    let mut value = [0u16; 8];
    let mut bytes = std::mem::size_of_val(&value) as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, w!("Control Panel\\International"), w!("iMeasure"), RRF_RT_REG_SZ, None, Some(value.as_mut_ptr().cast()), Some(&mut bytes))
    };
    r == ERROR_SUCCESS && value[0] == u16::from(b'1')
}

/// Tray choices and the weather's city and unit, kept in settings.txt.
#[derive(Clone, PartialEq, Debug)]
pub struct Settings {
    pub minimal: bool,
    pub english: bool,
    /// Idle robot eyes (on unless turned off).
    pub robot: bool,
    /// Volume keys handled by the app so only the notch shows (Windows' OSD stays away).
    pub volume_keys: bool,
    /// The weather's city, looked up once (None: weather off).
    pub place: Option<Place>,
    /// °C or °F, once chosen (None: follow the region setting).
    pub unit: Option<crate::weather::Unit>,
    /// Notifications up to this id were cleared on the Notifications page (hidden there; Windows keeps them).
    pub notes_cleared: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { minimal: false, english: false, robot: true, volume_keys: false, place: None, unit: None, notes_cleared: 0 }
    }
}

fn parse_settings(text: &str) -> Settings {
    let mut s = Settings::default();
    for line in text.lines() {
        match line.trim() {
            "minimal=1" => s.minimal = true,
            "lang=en" => s.english = true,
            "robot=0" => s.robot = false,
            "volkeys=1" => s.volume_keys = true,
            other => {
                if let Some(place) = other.strip_prefix("city=") {
                    s.place = crate::weather::parse_place(place);
                } else if let Some(unit) = other.strip_prefix("unit=") {
                    s.unit = crate::weather::Unit::from_tag(unit);
                } else if let Some(id) = other.strip_prefix("notes_cleared=") {
                    s.notes_cleared = id.parse().unwrap_or(0);
                }
            }
        }
    }
    s
}

pub fn load_settings() -> Settings {
    settings_path().and_then(|p| std::fs::read_to_string(p).ok()).map_or_else(Settings::default, |t| parse_settings(&t))
}

pub fn save_settings(s: Settings) {
    if let Some(p) = settings_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let city = s.place.as_ref().map_or(String::new(), |p| format!("city={}\n", crate::weather::place_line(p)));
        let unit = s.unit.map_or(String::new(), |u| format!("unit={}\n", u.tag()));
        let cleared = if s.notes_cleared > 0 { format!("notes_cleared={}\n", s.notes_cleared) } else { String::new() };
        let _ = std::fs::write(p, format!("minimal={}\nlang={}\nrobot={}\nvolkeys={}\n{city}{unit}{cleared}", s.minimal as u8, if s.english { "en" } else { "vi" }, s.robot as u8, s.volume_keys as u8));
    }
}

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_NAME: PCWSTR = w!("PuwNotch");

/// The Run entry's command for this exe: the path in quotes (spaces in a folder name).
fn autostart_command(exe: &std::path::Path) -> String {
    format!("\"{}\"", exe.display())
}

/// What the Run entry holds now, None when there is none.
fn autostart_value() -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut bytes)) };
    (r == ERROR_SUCCESS).then(|| String::from_utf16_lossy(&buf[..(bytes as usize / 2).saturating_sub(1)]))
}

/// On only when the entry starts this very exe: one left from another folder (an older version, a
/// deleted download) is not "on", it starts nothing.
pub fn autostart_enabled() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    autostart_value().is_some_and(|v| v.eq_ignore_ascii_case(&autostart_command(&exe)))
}

/// An entry that points somewhere else is pointed at this exe, so a new version unpacked in a new
/// folder keeps starting with Windows without the switch being turned off and on again.
pub fn refresh_autostart() {
    if autostart_value().is_some() && !autostart_enabled() {
        set_autostart(true);
    }
}

pub fn set_autostart(on: bool) {
    unsafe {
        if !on {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME);
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let value: Vec<u16> = autostart_command(&exe).encode_utf16().chain([0]).collect();
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME, REG_SZ.0, Some(value.as_ptr().cast()), (value.len() * 2) as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(percent: u8, charging: bool, saver: bool) -> Battery {
        Battery { percent, charging, saver }
    }

    #[test]
    fn autostart_command_quotes_the_path() {
        assert_eq!(autostart_command(std::path::Path::new(r"C:\Users\A B\Telegram Desktop\puw_notch.exe")), r#""C:\Users\A B\Telegram Desktop\puw_notch.exe""#);
    }

    #[test]
    fn saver_is_the_old_flag_or_the_best_efficiency_power_mode() {
        let other = GUID::from_u128(0x3af9b8d9_7c97_431d_ad78_34a8bfea439f); // "Balanced" overlay
        assert!(!saver_on(0, None));
        assert!(!saver_on(0, Some(GUID::zeroed())), "default mode");
        assert!(!saver_on(0, Some(other)));
        assert!(saver_on(1, None), "classic battery saver flag");
        assert!(saver_on(1, Some(other)));
        // Windows 11 energy saver: only the power mode changes, the flag stays 0.
        assert!(saver_on(0, Some(BEST_EFFICIENCY)));
        assert_eq!(BEST_EFFICIENCY, GUID::from_u128(0x961cc777_2547_4f9d_8174_7d86181b8a7a));
    }

    #[test]
    fn settings_file() {
        assert_eq!(parse_settings("minimal=1\nlang=en\nrobot=0\nvolkeys=1\n"), Settings { minimal: true, english: true, robot: false, volume_keys: true, place: None, unit: None, notes_cleared: 0 });
        assert_eq!(parse_settings("notes_cleared=42\n").notes_cleared, 42);
        assert_eq!(parse_settings("notes_cleared=x\n").notes_cleared, 0);
        assert_eq!(parse_settings("unit=f\n").unit, Some(crate::weather::Unit::F));
        assert_eq!(parse_settings("unit=c\n").unit, Some(crate::weather::Unit::C));
        assert_eq!(parse_settings("unit=x\n").unit, None, "unknown: follow the region");
        assert_eq!(parse_settings("minimal=0\n").unit, None, "old files: follow the region");
        let hue = crate::weather::Place { name: "Huế".into(), lat: 16.4619, lon: 107.59546 };
        assert_eq!(parse_settings("robot=1\ncity=16.46190,107.59546,Huế\n").place, Some(hue));
        assert_eq!(parse_settings("minimal=0\n"), Settings::default(), "old files: Vietnamese, idle robot on, volume keys left to Windows");
        assert!(Settings::default().robot && !Settings::default().volume_keys);
    }

    #[test]
    fn power_changes() {
        assert!(power_events(b(50, false, false), b(49, false, false)).is_empty());
        assert_eq!(power_events(b(50, false, false), b(50, true, false)), vec![PowerEvent::Plugged]);
        assert_eq!(power_events(b(50, true, false), b(50, false, false)), vec![PowerEvent::Unplugged]);
        assert_eq!(power_events(b(50, false, false), b(50, false, true)), vec![PowerEvent::SaverOn]);
        assert_eq!(power_events(b(21, false, false), b(20, false, false)), vec![PowerEvent::Low]);
        assert!(power_events(b(20, false, false), b(19, false, false)).is_empty(), "only when crossing");
        assert_eq!(power_events(b(11, false, false), b(10, false, false)), vec![PowerEvent::Critical]);
        assert!(power_events(b(21, true, false), b(20, true, false)).is_empty(), "charging: not low");
        assert_eq!(power_events(b(99, true, false), b(100, true, false)), vec![PowerEvent::Full]);
    }
}
